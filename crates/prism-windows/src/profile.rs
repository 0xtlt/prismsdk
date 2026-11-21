//! `PRISM_PROFILE=stacks.txt` (development, x64): samples the UI thread's
//! stack every millisecond from startup until the first frame (and
//! `PRISM_PROFILE_MS` more, default 0), then writes the stacks folded, one
//! line per distinct stack, innermost frame last:
//!
//! ```text
//! <phase> counter.exe+0x12ab0;Microsoft.UI.Xaml.dll+0x3f00 17
//! ```
//!
//! The phase is the last startup phase logged (`imp::trace`).
//! `scripts/perf/profile.py` names the executable's frames with its
//! symbols (build with them: `--config profile.release.strip=false`).
//!
//! The sampler suspends the UI thread and unwinds its stack with the
//! modules' unwind tables (`RtlVirtualUnwind`), allocating nothing while
//! the thread is stopped. Windows' profilers need administrator rights;
//! this does not. x64 only: elsewhere `PRISM_PROFILE` does nothing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HMODULE,
};
use windows::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_FULL_AMD64, GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind,
    UNW_FLAG_NHANDLER,
};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::ProcessStatus::{EnumProcessModules, GetModuleInformation, MODULEINFO};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, GetCurrentProcess,
    GetCurrentThread, INFINITE, ResumeThread, SetWaitableTimer, SuspendThread, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};

use crate::win::{env, log};

const DEPTH: usize = 48;
const MAX_SAMPLES: usize = 60_000;

static RUNNING: AtomicBool = AtomicBool::new(false);
/// When sampling stops (ms since start), once the first frame is known.
static STOP_AT: AtomicU64 = AtomicU64::new(u64::MAX);
/// The startup phase now, as an index into `PHASES`.
static PHASE: AtomicU64 = AtomicU64::new(0);
static PHASES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Starts sampling the calling (UI) thread, with `PRISM_PROFILE`.
#[cfg(not(target_arch = "x86_64"))]
pub fn start() {}

/// Starts sampling the calling (UI) thread, with `PRISM_PROFILE`.
#[cfg(target_arch = "x86_64")]
pub fn start() {
    let Some(path) = env("PRISM_PROFILE") else {
        return;
    };
    let mut ui = HANDLE::default();
    unsafe {
        let process = GetCurrentProcess();
        if DuplicateHandle(
            process,
            GetCurrentThread(),
            process,
            &mut ui,
            0,
            false,
            DUPLICATE_SAME_ACCESS,
        )
        .is_err()
        {
            return;
        }
    }
    RUNNING.store(true, Ordering::SeqCst);
    phase("start");
    let ui = ui.0 as usize;
    std::thread::spawn(move || sample(HANDLE(ui as *mut _), &path));
}

/// A startup phase began (`imp::trace`).
pub fn phase(name: &str) {
    if !RUNNING.load(Ordering::Relaxed) {
        return;
    }
    if let Ok(mut p) = PHASES.lock() {
        p.push(name.to_string());
        PHASE.store(p.len() as u64 - 1, Ordering::SeqCst);
    }
}

/// The first frame was shown: sampling stops `PRISM_PROFILE_MS` later.
pub fn first_frame(now_ms: f64) {
    let extra = crate::win::env_secs("PRISM_PROFILE_MS", 0.);
    STOP_AT.store((now_ms + extra) as u64, Ordering::SeqCst);
}

#[cfg(target_arch = "x86_64")]
struct Samples {
    frames: Vec<u64>,
    depths: Vec<u8>,
    phases: Vec<u16>,
}

#[cfg(target_arch = "x86_64")]
fn sample(ui: HANDLE, path: &str) {
    let mut s = Samples {
        frames: Vec::with_capacity(MAX_SAMPLES * DEPTH),
        depths: Vec::with_capacity(MAX_SAMPLES),
        phases: Vec::with_capacity(MAX_SAMPLES),
    };
    let timer = unsafe {
        CreateWaitableTimerExW(
            None,
            None,
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS.0,
        )
    };
    let Ok(timer) = timer else {
        log!("PRISM_PROFILE: no high-resolution timer");
        return;
    };
    let mut stack = [0u64; DEPTH];
    while s.depths.len() < MAX_SAMPLES {
        if crate::imp::since_start() as u64 >= STOP_AT.load(Ordering::SeqCst) {
            break;
        }
        // 1 ms (in 100 ns units, relative).
        let due = -10_000i64;
        unsafe {
            if SetWaitableTimer(timer, &due, 0, None, None, false).is_err() {
                break;
            }
            WaitForSingleObject(timer, INFINITE);
        }
        let n = unsafe { walk(ui, &mut stack) };
        if n == 0 {
            continue;
        }
        s.frames.extend_from_slice(&stack[..n]);
        s.depths.push(n as u8);
        s.phases.push(PHASE.load(Ordering::SeqCst) as u16);
    }
    unsafe {
        let _ = CloseHandle(timer);
        let _ = CloseHandle(ui);
    }
    RUNNING.store(false, Ordering::SeqCst);
    match write(&s, path) {
        Ok(()) => log!(
            "PRISM_PROFILE: {} samples written to {path}",
            s.depths.len()
        ),
        Err(e) => log!("PRISM_PROFILE: {path}: {e}"),
    }
}

#[cfg(target_arch = "x86_64")]
/// The UI thread's stack now, innermost first: stops it, unwinds, lets it
/// go. Nothing here allocates (the heap's lock may be held).
unsafe fn walk(ui: HANDLE, out: &mut [u64; DEPTH]) -> usize {
    unsafe {
        if SuspendThread(ui) == u32::MAX {
            return 0;
        }
        let mut ctx = CONTEXT {
            ContextFlags: CONTEXT_FULL_AMD64,
            ..Default::default()
        };
        let mut n = 0;
        if GetThreadContext(ui, &mut ctx).is_ok() {
            while n < DEPTH && ctx.Rip != 0 {
                out[n] = ctx.Rip;
                n += 1;
                let mut base = 0u64;
                let f = RtlLookupFunctionEntry(ctx.Rip, &mut base, None);
                if f.is_null() {
                    // A leaf function: the return address is on top.
                    if ctx.Rsp == 0 {
                        break;
                    }
                    ctx.Rip = *(ctx.Rsp as *const u64);
                    ctx.Rsp += 8;
                } else {
                    let mut data = std::ptr::null_mut();
                    let mut frame = 0u64;
                    RtlVirtualUnwind(
                        UNW_FLAG_NHANDLER,
                        base,
                        ctx.Rip,
                        f,
                        &mut ctx,
                        &mut data,
                        &mut frame,
                        None,
                    );
                }
            }
        }
        ResumeThread(ui);
        n
    }
}

#[cfg(target_arch = "x86_64")]
/// The modules loaded now: base, size, file name.
fn modules() -> Vec<(u64, u64, String)> {
    let mut list = vec![HMODULE::default(); 1024];
    let mut needed = 0u32;
    let mut out = Vec::new();
    unsafe {
        let process = GetCurrentProcess();
        if EnumProcessModules(
            process,
            list.as_mut_ptr(),
            (list.len() * size_of::<HMODULE>()) as u32,
            &mut needed,
        )
        .is_err()
        {
            return out;
        }
        list.truncate(needed as usize / size_of::<HMODULE>());
        for m in list {
            let mut info = MODULEINFO::default();
            if GetModuleInformation(process, m, &mut info, size_of::<MODULEINFO>() as u32).is_err()
            {
                continue;
            }
            let mut name = [0u16; 260];
            let len = GetModuleFileNameW(Some(m), &mut name) as usize;
            let full = String::from_utf16_lossy(&name[..len]);
            let file = full.rsplit('\\').next().unwrap_or(&full).to_string();
            out.push((info.lpBaseOfDll as u64, info.SizeOfImage as u64, file));
        }
    }
    out
}

#[cfg(target_arch = "x86_64")]
fn write(s: &Samples, path: &str) -> std::io::Result<()> {
    use std::collections::HashMap;
    use std::fmt::Write as _;
    let modules = modules();
    let name = |a: u64| match modules.iter().find(|(b, n, _)| a >= *b && a < b + n) {
        Some((b, _, file)) => format!("{file}+{:#x}", a - b),
        None => format!("?+{a:#x}"),
    };
    let phases = PHASES.lock().map(|p| p.clone()).unwrap_or_default();
    let mut folded: HashMap<String, u32> = HashMap::new();
    let mut at = 0;
    for (i, &d) in s.depths.iter().enumerate() {
        let frames = &s.frames[at..at + d as usize];
        at += d as usize;
        let phase = phases
            .get(s.phases[i] as usize)
            .map_or("?", String::as_str)
            .replace(' ', "_");
        let mut line = phase;
        line.push(' ');
        for (j, f) in frames.iter().rev().enumerate() {
            if j > 0 {
                line.push(';');
            }
            // Return addresses point after the call: name the call.
            let a = if j + 1 == frames.len() { *f } else { f - 1 };
            let _ = write!(line, "{}", name(a));
        }
        *folded.entry(line).or_default() += 1;
    }
    let mut lines: Vec<(String, u32)> = folded.into_iter().collect();
    lines.sort_by_key(|a| std::cmp::Reverse(a.1));
    let mut text = String::new();
    for (l, c) in lines {
        let _ = writeln!(text, "{l} {c}");
    }
    std::fs::write(path, text)
}
