//! Frame callbacks for app code (#311): while the app wants frames, XAML's
//! `CompositionTarget::Rendering` (once per frame it renders, before
//! layout) runs them. The handler goes once none is left.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crate::winui::xaml::Media::CompositionTarget;

thread_local! {
    /// The `Rendering` handler's token while it is registered.
    static TICKING: Cell<Option<i64>> = const { Cell::new(None) };
}

/// Registers the handler when the runtime wants frames. Call it after each
/// commit.
pub fn sync(wanted: bool) {
    if !wanted || TICKING.with(|t| t.get().is_some()) {
        return;
    }
    let handler = windows::Foundation::EventHandler::<windows_core::IInspectable>::new(|_, _| {
        tick();
        Ok(())
    });
    match CompositionTarget::Rendering(&handler) {
        Ok(token) => TICKING.with(|t| t.set(Some(token))),
        Err(e) => crate::win::log!("frame callbacks: {e}"),
    }
}

fn tick() {
    let now = Instant::now();
    let mut more = true;
    crate::imp::with_state(|s| {
        let hz = s
            .runtime
            .app
            .device_info()
            .screen
            .refresh_rate
            .filter(|hz| *hz > 1.)
            .unwrap_or(60.);
        let target = now + Duration::from_secs_f32(1. / hz);
        let patches = s.runtime.frame(now, target);
        crate::imp::commit(s, patches);
        more = s.runtime.wants_frames();
    });
    if !more && let Some(token) = TICKING.with(Cell::take) {
        let _ = CompositionTarget::RemoveRendering(token);
    }
}
