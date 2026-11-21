//! What the PC's device APIs can do, asked while WinUI starts (`imp::run`)
//! so the capabilities say the truth from the first frame: a camera and
//! the Camera app, a location provider, motion sensors, a vibration
//! device, `webauthn.dll`, the Share dialog. Each check runs on its own thread; startup waits for
//! them at most 1.5 s (they take tens of milliseconds), and a check that
//! has not answered by then says "no".

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crate::win::log;

/// The device APIs this PC has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub location: bool,
    pub motion: bool,
    pub camera: bool,
    pub haptics: bool,
    pub passkeys: bool,
    pub share: bool,
}

type Checks = Vec<(&'static str, Receiver<bool>)>;

static PENDING: Mutex<Option<Checks>> = Mutex::new(None);

fn checks() -> Checks {
    let run = |name: &'static str, f: fn() -> bool| {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        (name, rx)
    };
    vec![
        run("location", crate::location::present),
        run("motion", crate::sensors::present),
        run("camera", crate::camera::present),
        run("haptics", crate::haptics::present),
        run("passkeys", crate::webauthn::available),
        run("share", crate::share::supported),
    ]
}

/// Starts the checks.
pub fn prefetch() {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(checks());
}

/// `PRISM_DEVICE_REPORT=<file.json>` (a development hook): writes what
/// each device API sees on this PC, without any prompt or system UI: the
/// capabilities, then location, motion, camera, sharing, passkeys and haptics
/// (`report` in each module; sharing's from the window's thread, the
/// others' each on its own). Startup waits for it (a position and a
/// watch, when location is already allowed, take up to 40 s).
pub fn report(owner: windows::Win32::Foundation::HWND, data: &std::path::Path, p: Probe) {
    let Some(path) = crate::win::env("PRISM_DEVICE_REPORT") else {
        return;
    };
    let picked = data.join("picked");
    let (location, motion, camera, passkeys, haptics) = std::thread::scope(|s| {
        let location = s.spawn(crate::location::report);
        let motion = s.spawn(crate::sensors::report);
        let camera = s.spawn(|| crate::camera::report(&picked));
        let passkeys = s.spawn(crate::webauthn::report);
        let haptics = s.spawn(crate::haptics::report);
        (
            joined(location),
            joined(motion),
            joined(camera),
            joined(passkeys),
            joined(haptics),
        )
    });
    let report = serde_json::json!({
        "capabilities": {
            "location": p.location,
            "motion": p.motion,
            "camera": p.camera,
            "haptics": p.haptics,
            "passkeys": p.passkeys,
            "share": p.share,
        },
        "location": location,
        "motion": motion,
        "camera": camera,
        "share": crate::share::report(owner),
        "passkeys": passkeys,
        "haptics": haptics,
    });
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    match std::fs::write(&path, text) {
        Ok(()) => log!("device report written to {path}"),
        Err(e) => log!("PRISM_DEVICE_REPORT: {path}: {e}"),
    }
}

fn joined(h: std::thread::ScopedJoinHandle<'_, serde_json::Value>) -> serde_json::Value {
    h.join().unwrap_or_else(|_| "the check panicked".into())
}

/// The checks' answers, waiting at most 1.5 s more.
pub fn at_start() -> Probe {
    let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut p = Probe::default();
    for (name, rx) in pending.unwrap_or_else(checks) {
        let left = deadline.saturating_duration_since(Instant::now());
        let yes = rx.recv_timeout(left).unwrap_or_else(|_| {
            log!("{name}: no answer at startup, reported unavailable");
            false
        });
        match name {
            "location" => p.location = yes,
            "motion" => p.motion = yes,
            "camera" => p.camera = yes,
            "haptics" => p.haptics = yes,
            "passkeys" => p.passkeys = yes,
            _ => p.share = yes,
        }
    }
    p
}
