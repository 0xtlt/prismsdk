//! Where the time goes, with `PRISM_TRACE=1`: the startup phases (WinUI,
//! the app's tree, XAML elements, measuring, layout, XAML's own layout,
//! the first frame), the memory at each, and how long each update takes
//! to reach the screen (a tab switch, a click).
//!
//! The backend's work is split by kind ([`Cost`]) with spans that count
//! their own time only: a style applied while placing a node counts as
//! styling, not placing. Off (the default), a span is one thread-local
//! read.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use crate::win::log;

/// A kind of backend work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cost {
    /// XAML elements for new nodes (`Widgets::create`).
    Create,
    /// Styles, text, values, configs on the elements.
    Style,
    /// Natural sizes: text, controls (cache hits included).
    Measure,
    /// Text laid out by XAML to measure it (a cache miss).
    TextLayout,
    /// Frames on the elements (`Canvas.Left/Top`, sizes).
    Place,
    /// Children in, out and in order.
    Children,
    /// Bars, pages shown, overlays, sticky views (`flush`).
    Flush,
}

const COSTS: [Cost; 7] = [
    Cost::Create,
    Cost::Style,
    Cost::Measure,
    Cost::TextLayout,
    Cost::Place,
    Cost::Children,
    Cost::Flush,
];

impl Cost {
    fn name(self) -> &'static str {
        match self {
            Cost::Create => "create",
            Cost::Style => "style",
            Cost::Measure => "measure",
            Cost::TextLayout => "text layout",
            Cost::Place => "place",
            Cost::Children => "children",
            Cost::Flush => "flush",
        }
    }
}

#[derive(Default)]
struct Totals {
    time: [Duration; COSTS.len()],
    calls: [u32; COSTS.len()],
}

thread_local! {
    static ON: Cell<Option<bool>> = const { Cell::new(None) };
    /// What the spans counted outside patches (layout, flush).
    static TOTALS: RefCell<Totals> = RefCell::new(Totals::default());
    /// What they counted while patches applied.
    static APPLIED: RefCell<Totals> = RefCell::new(Totals::default());
    /// Patches are being applied.
    static APPLYING: Cell<bool> = const { Cell::new(false) };
    /// Time applying patches and laying out, each without the other.
    static STAGES: Cell<(Duration, Duration)> = const { Cell::new((Duration::ZERO, Duration::ZERO)) };
    /// The span running now and when it (re)started.
    static CURRENT: Cell<Option<(Cost, Instant)>> = const { Cell::new(None) };
    /// When the update waiting for its frame started, and its name.
    static PENDING: RefCell<Option<(Instant, String)>> = const { RefCell::new(None) };
}

/// Whether timings are on (`PRISM_TRACE`).
pub fn on() -> bool {
    ON.with(|on| match on.get() {
        Some(v) => v,
        None => {
            let v = crate::win::env("PRISM_TRACE").is_some();
            on.set(Some(v));
            v
        }
    })
}

/// Counts the time until it drops as `cost` (minus the spans inside it).
pub struct Span {
    outer: Option<(Cost, Instant)>,
    live: bool,
}

pub fn span(cost: Cost) -> Span {
    if !on() {
        return Span {
            outer: None,
            live: false,
        };
    }
    let now = Instant::now();
    let outer = CURRENT.get();
    if let Some((c, since)) = outer {
        add(c, now - since, false);
    }
    CURRENT.set(Some((cost, now)));
    add(cost, Duration::ZERO, true);
    Span { outer, live: true }
}

impl Drop for Span {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        let now = Instant::now();
        if let Some((c, since)) = CURRENT.get() {
            add(c, now - since, false);
        }
        CURRENT.set(self.outer.map(|(c, _)| (c, now)));
    }
}

fn add(cost: Cost, d: Duration, call: bool) {
    let bucket = if APPLYING.get() { &APPLIED } else { &TOTALS };
    bucket.with(|t| {
        let mut t = t.borrow_mut();
        let i = COSTS.iter().position(|c| *c == cost).unwrap_or(0);
        t.time[i] += d;
        t.calls[i] += u32::from(call);
    })
}

/// Applies patches (`f`), counted as such.
pub fn patches<R>(f: impl FnOnce() -> R) -> R {
    if !on() {
        return f();
    }
    let t = Instant::now();
    let was = APPLYING.replace(true);
    if !was {
        crate::profile::phase("patches");
    }
    let r = f();
    APPLYING.set(was);
    if !was {
        let (a, l) = STAGES.get();
        STAGES.set((a + t.elapsed(), l));
    }
    r
}

/// Lays out (`f`), counted as such (patches it applies are not).
pub fn layout<R>(f: impl FnOnce() -> R) -> R {
    if !on() || APPLYING.get() {
        return f();
    }
    let t = Instant::now();
    crate::profile::phase("layout");
    let before = STAGES.get().0;
    let r = f();
    let (a, l) = STAGES.get();
    STAGES.set((a, l + t.elapsed().saturating_sub(a - before)));
    r
}

fn list(t: &Totals) -> String {
    let parts: Vec<String> = COSTS
        .iter()
        .enumerate()
        .filter(|(i, _)| t.calls[*i] > 0)
        .map(|(i, c)| {
            format!(
                "{} {:.1} ms ({})",
                c.name(),
                t.time[i].as_secs_f64() * 1000.,
                t.calls[i]
            )
        })
        .collect();
    if parts.is_empty() {
        "nothing".into()
    } else {
        parts.join(", ")
    }
}

/// What the backend did since the last report, then starts over:
/// `patches 310.2 ms (create 120.4 ms (660), style…); layout…`. A stage's
/// time outside its spans is prism-native's own (the diff, flexbox).
pub fn take() -> String {
    let (apply, layout) = STAGES.replace((Duration::ZERO, Duration::ZERO));
    let a = APPLIED.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let l = TOTALS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    format!(
        "patches {:.1} ms ({}); layout {:.1} ms ({})",
        apply.as_secs_f64() * 1000.,
        list(&a),
        layout.as_secs_f64() * 1000.,
        list(&l)
    )
}

/// Milliseconds since `t`.
pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}

/// An update started at `started` was committed: the next frame shows it
/// ([`rendered`] logs how long it took).
pub fn committed(what: &str, started: Instant) {
    if on() {
        PENDING.with(|p| *p.borrow_mut() = Some((started, what.to_string())));
    }
}

/// A frame was rendered (`CompositionTarget::Rendered`): logs the update
/// it shows.
pub fn rendered() {
    if let Some((t, what)) = PENDING.with(|p| p.borrow_mut().take()) {
        log!("{what}: on screen after {:.1} ms", ms(t));
    }
}

/// How many XAML elements are in `root`'s visual tree (control templates
/// included).
pub fn count_elements(root: &crate::winui::xaml::DependencyObject) -> usize {
    use crate::winui::xaml::Media::VisualTreeHelper;
    let mut n = 0;
    let mut stack = vec![root.clone()];
    while let Some(e) = stack.pop() {
        n += 1;
        let count = VisualTreeHelper::GetChildrenCount(&e).unwrap_or(0);
        for i in 0..count {
            if let Ok(c) = VisualTreeHelper::GetChild(&e, i) {
                stack.push(c);
            }
        }
    }
    n
}

thread_local! {
    /// The time stamp counter and the clock when the UI thread first
    /// asked: its rate, measured against the clock.
    #[cfg(target_arch = "x86_64")]
    static TSC: Cell<Option<(u64, Instant)>> = const { Cell::new(None) };
}

/// Milliseconds of CPU the calling thread has used: unlike the clock,
/// other programs' load hardly changes it. Its cycles at the time stamp
/// counter's rate (x64), else its kernel and user times (counted in
/// clock ticks of about 16 ms).
pub fn cpu_ms() -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::_rdtsc;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::GetCurrentThread;
        // In the `windows` crate behind a feature the crate does not need
        // otherwise.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
        }
        let mut cycles = 0u64;
        unsafe {
            let _ = QueryThreadCycleTime(GetCurrentThread(), &mut cycles);
        }
        let (t0, i0) = TSC.get().unwrap_or_else(|| {
            let v = (unsafe { _rdtsc() }, Instant::now());
            TSC.set(Some(v));
            // A first rate: 2 ms of the clock.
            let until = Instant::now() + Duration::from_millis(2);
            while Instant::now() < until {}
            v
        });
        let rate = unsafe { _rdtsc() }.saturating_sub(t0) as f64 / i0.elapsed().as_secs_f64();
        cycles as f64 / rate.max(1.) * 1000.
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        let (mut c, mut e, mut k, mut u) = Default::default();
        unsafe {
            let _ = GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
        }
        (t(k) + t(u)) as f64 / 10_000.
    }
}
