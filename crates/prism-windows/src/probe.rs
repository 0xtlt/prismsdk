//! What the PC can do, asked in the background from the very start of
//! `imp::run`: Windows Hello, a camera and the Camera app, a location
//! provider, motion sensors, a vibration device, `webauthn.dll`, the Share
//! dialog. Each check runs on its own thread, and nothing waits for them:
//! the first frame comes first. Their answers go to a mailbox
//! (`device::Mailbox`); the services take what came before they start
//! (`open`), and each later answer is published as it comes
//! (`Runtime::set_capabilities`, which re-renders). Until a check answers,
//! its capability is false and a call to its API tries it anyway
//! (`device::may`), so a slow check costs nothing for good.

use crate::device::{Found, Mailbox};
use crate::win::log;

/// A device API's check: its name, the check, where its answer goes.
type Check = (&'static str, fn() -> bool, fn(&mut Found, bool));

/// The answers so far.
static FOUND: Mailbox<Found> = Mailbox::new(Found::NONE);

/// Starts the checks. Called once, at the start of `imp::run`.
pub fn start() {
    check("Windows Hello", || {
        let hello = crate::security::hello();
        let kind = hello.kind;
        let usable = hello.usable();
        (usable, move |f: &mut Found| f.hello = Some((kind, usable)))
    });
    let checks: [Check; 6] = [
        ("location", crate::location::present, |f, y| {
            f.location = Some(y)
        }),
        ("motion", crate::sensors::present, |f, y| f.motion = Some(y)),
        ("camera", crate::camera::present, |f, y| f.camera = Some(y)),
        ("haptics", crate::haptics::present, |f, y| {
            f.haptics = Some(y)
        }),
        ("passkeys", crate::webauthn::available, |f, y| {
            f.passkeys = Some(y)
        }),
        ("share", crate::share::supported, |f, y| f.share = Some(y)),
    ];
    for (name, present, set) in checks {
        check(name, move || {
            let yes = present();
            (yes, move |f: &mut Found| set(f, yes))
        });
    }
}

/// Runs `run` on its own thread and records its answer.
fn check<F>(name: &'static str, run: impl FnOnce() -> (bool, F) + Send + 'static)
where
    F: FnOnce(&mut Found),
{
    std::thread::spawn(move || {
        let (yes, set) = run();
        // `PRISM_TRACE`: when each check answers, next to the first frame.
        if crate::perf::on() {
            log!(
                "{name} checked ({}) after {:.0} ms",
                answer(yes),
                crate::imp::since_start()
            );
        }
        if FOUND.record(set) {
            // The services took the earlier answers: publish this one.
            crate::shell::on_ui(move || {
                crate::imp::defer(Box::new(move |s| {
                    if !s.services.take_found() {
                        return;
                    }
                    let caps = prism_core::PlatformServices::capabilities(&s.services);
                    let patches = s.runtime.set_capabilities(caps);
                    crate::imp::commit(s, patches);
                    if crate::perf::on() {
                        log!(
                            "{name}: capabilities given to the app after {:.0} ms",
                            crate::imp::since_start()
                        );
                    }
                }));
            });
        }
    });
}

fn answer(yes: bool) -> &'static str {
    if yes { "yes" } else { "no" }
}

/// The answers so far; each later one is published. The services call
/// it once, after the services window exists (`shell::create`).
pub fn open() -> Found {
    let found = FOUND.open();
    let pending = found.pending();
    if crate::perf::on() && !pending.is_empty() {
        log!(
            "checks not answered when the services start: {}",
            pending.join(", ")
        );
    }
    found
}

/// The answers so far.
pub fn found() -> Found {
    FOUND.get()
}

/// `PRISM_DEVICE_REPORT=<file.json>` (a development hook): writes what
/// each device API sees on this PC, without any prompt or system UI: the
/// capabilities once every check has answered (at most 15 s; one that has
/// not is `null`), then location, motion, camera, sharing, passkeys and
/// haptics (`report` in each module; sharing's from the window's thread,
/// the others' each on its own). Startup waits for it (a position and a
/// watch, when location is already allowed, take up to 40 s): with the
/// hook only.
pub fn report(owner: windows::Win32::Foundation::HWND, data: &std::path::Path) {
    let Some(path) = crate::win::env("PRISM_DEVICE_REPORT") else {
        return;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !found().pending().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let f = found();
    for name in f.pending() {
        log!("device report: {name} has not answered in 15 s");
    }
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
            "location": f.location,
            "motion": f.motion,
            "camera": f.camera,
            "haptics": f.haptics,
            "passkeys": f.passkeys,
            "share": f.share,
        },
        "location": location,
        "motion": motion,
        "camera": camera,
        "share": crate::share::report(owner),
        "passkeys": passkeys,
        "haptics": haptics,
    });
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    // The file's name only: its folders name the account.
    let name = crate::device::file_name(&path);
    match std::fs::write(&path, text) {
        Ok(()) => log!("device report written to {name}"),
        Err(e) => log!("PRISM_DEVICE_REPORT: {name}: {e}"),
    }
}

fn joined(h: std::thread::ScopedJoinHandle<'_, serde_json::Value>) -> serde_json::Value {
    h.join().unwrap_or_else(|_| "the check panicked".into())
}
