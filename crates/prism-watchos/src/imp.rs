//! The app's runtime on the watch's main thread, driven by the SwiftUI host.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::sync::OnceLock;

use prism_core::{
    AppConfig, AppInit, DeviceInfo, DeviceKind, Orientation, Os, OsName, Runtime, ScreenInfo,
    UiEvent, WindowMode,
};
use serde_json::Value;

use prism_core::Backend;
use prism_native::{Insets, NativeRenderer};

use prism_native::remote::{MeasureRequest, RemoteWidgets};

unsafe extern "C" {
    /// Natural size of a leaf, measured by UIKit (string drawing) / SwiftUI.
    fn prism_watch_measure(
        kind: *const c_char,
        text: *const c_char,
        font: f64,
        weight: i32,
        max_width: f64,
        spec: *const c_char,
        out_w: *mut f64,
        out_h: *mut f64,
    );
    /// Runs the SwiftUI app; never returns.
    fn prism_watch_main();
    fn prism_watch_apply(json: *const c_char);
}

static APP: OnceLock<(AppConfig, AppInit)> = OnceLock::new();

struct State {
    runtime: Runtime,
    services: crate::services::WatchServices,
    /// Background tasks running for WatchKit: (task id, runtime task).
    background: Vec<(String, u64)>,
    renderer: NativeRenderer<RemoteWidgets>,
    /// The root's size in watch points (reported by the host).
    size: (f32, f32),
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn measure(r: &MeasureRequest) -> (f32, f32) {
    let kind = CString::new(r.kind).unwrap_or_default();
    let text = CString::new(r.text).unwrap_or_default();
    let spec = CString::new(r.spec).unwrap_or_default();
    let (mut w, mut h) = (0f64, 0f64);
    unsafe {
        prism_watch_measure(
            kind.as_ptr(),
            text.as_ptr(),
            r.font_size as f64,
            r.weight,
            r.max_width.map_or(f64::INFINITY, f64::from),
            spec.as_ptr(),
            &mut w,
            &mut h,
        )
    };
    (w as f32, h as f32)
}

/// Starts the app (call from `main`).
pub fn run(config: AppConfig, init: AppInit) {
    let _ = APP.set((config, init));
    unsafe { prism_watch_main() };
}

fn data_dir() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join("Library/Application Support/prism")
}

/// The watch, from its model and screen (called once the host is on screen).
fn device_info(width: f32, height: f32, scale: f32) -> DeviceInfo {
    let model = prism_native::apple::model_id();
    let known = model
        .as_deref()
        .and_then(prism_core::device_info::apple_model);
    let (pw, ph) = (width * scale, height * scale);
    let ppi = known
        .and_then(|m| m.screen(pw.round() as u32, ph.round() as u32))
        .map(|(_, p)| p);
    DeviceInfo {
        kind: DeviceKind::Watch,
        os: Os {
            name: OsName::Watchos,
            version: prism_native::apple::os_version(),
        },
        model_name: known.map(|m| m.name.to_string()),
        model,
        simulator: prism_native::apple::is_simulator(),
        orientation: Orientation::of(width, height),
        screen: ScreenInfo {
            width,
            height,
            scale,
            diagonal: ppi.and_then(|p| prism_core::device_info::diagonal_inches(pw, ph, p)),
            ppi,
            ..Default::default()
        },
        window_mode: WindowMode::Fullscreen,
        touch: true,
        ..Default::default()
    }
}

/// The host is on screen: start the app.
#[unsafe(no_mangle)]
pub extern "C" fn prism_watch_ready(
    width: f64,
    height: f64,
    scale: f64,
    zoom: f64,
    text_scale: f64,
) {
    let (config, init) = APP.get().expect("prism-watchos: run() was not called");
    let _ = config;
    let zoom = std::env::var("PRISM_ZOOM")
        .ok()
        .and_then(|z| z.parse().ok())
        .unwrap_or(zoom as f32)
        .max(0.1);
    let (w, h) = (width as f32, height as f32);
    // Dynamic Type (the watch's text size setting).
    let mut environment = prism_core::Environment {
        locale: prism_native::apple::system_locale(),
        utc_offset: prism_native::apple::utc_offset(),
        text_scale: text_scale as f32,
        ..Default::default()
    };
    if let Some(scale) = std::env::var("PRISM_TEXT_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        environment.text_scale = scale;
    }
    let services = crate::services::WatchServices::new();
    let cloud: Box<dyn prism_core::Storage> = match prism_native::apple::ICloudStorage::new() {
        Some(icloud) => Box::new(icloud),
        None => Box::new(prism_core::FileStorage::new(data_dir().join("cloud"))),
    };
    let runtime = Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            capabilities: prism_core::PlatformServices::capabilities(&services),
            cloud_storage: Some(cloud),
            path: std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into()),
            storage: Some(Box::new(prism_core::FileStorage::new(data_dir()))),
            wake_ui: Some(std::sync::Arc::new(|| {
                dispatch2::DispatchQueue::main().exec_async(poll)
            })),
            environment,
            formatter: Some(Box::new(prism_native::apple::FoundationFormatter)),
            data_dir: Some(data_dir()),
            // The app works in Prism points; the host draws them × zoom.
            viewport: Some((w / zoom, h / zoom)),
            device_info: Some(device_info(w, h, scale as f32)),
            ..Default::default()
        },
    );
    let mut renderer = NativeRenderer::new(RemoteWidgets::new(measure));
    renderer.set_zoom(zoom);
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            runtime,
            services,
            background: Vec::new(),
            renderer,
            size: (w, h),
        })
    });
    prism_native::apple::set_device_sink(on_device);
    with_state(|s| {
        let patches = s.runtime.render();
        commit(s, &patches);
    });
}

/// Answers and sensor events from the bridge (location, motion, photos…).
fn on_device(message: prism_native::apple::DeviceMessage) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        with_state(|s| {
            let patches = match message {
                prism_native::apple::DeviceMessage::Reply(id, result) => {
                    s.runtime.device_reply(id, result)
                }
                prism_native::apple::DeviceMessage::Event(id, event) => {
                    s.runtime.device_event(id, event)
                }
            };
            commit(s, &patches);
        })
    });
}

pub(crate) fn notification_response(
    response: prism_core::NotificationResponse,
    link: Option<String>,
) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        with_state(|s| {
            let patches = s.runtime.notification_response(response, link);
            commit(s, &patches);
        })
    });
}

pub(crate) fn push_registered(
    result: Result<prism_core::PushSubscription, prism_core::DeviceError>,
) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        with_state(|s| {
            let patches = s.runtime.push_registered(result);
            commit(s, &patches);
        })
    });
}

/// A push; silent ones run the app's job and complete WatchKit's handler
/// (`completion`) when it is done.
pub(crate) fn push_received(message: prism_core::PushMessage, completion: u64) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        let mut waiting = false;
        with_state(|s| {
            let patches = if message.event == prism_core::PushEvent::Background {
                if let Some(task) = s.runtime.run_background_push(message) {
                    s.background.push((format!("push:{completion}"), task));
                    waiting = true;
                }
                s.runtime.poll()
            } else {
                s.runtime.push_received(message, false)
            };
            commit(s, &patches);
        });
        if !waiting && completion != 0 {
            prism_native::apple::push::complete(completion, true);
        }
    });
}

/// `PRISM_BACKGROUND_INTERVAL=<seconds>` shortens background intervals (development).
pub(crate) fn background_interval(every: std::time::Duration) -> std::time::Duration {
    std::env::var("PRISM_BACKGROUND_INTERVAL")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .map_or(every, std::time::Duration::from_secs_f64)
}

/// WatchKit woke the app for task `id`: run it, tell WatchKit when done.
pub(crate) fn run_background(id: String) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        let mut started = false;
        with_state(|s| {
            if let Some(task) = s.runtime.run_background_task(&id) {
                s.background.push((id.clone(), task));
                started = true;
            }
            let patches = s.runtime.poll();
            commit(s, &patches);
        });
        if !started {
            crate::services::complete_background(&id);
        }
    })
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|cell| {
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(state) = guard.as_mut()
        {
            f(state)
        }
    });
}

/// Applies patches, lays out, sends the changes to the host and schedules
/// the next timer.
fn commit(s: &mut State, patches: &[prism_core::Patch]) {
    if !patches.is_empty() {
        s.renderer.apply(patches);
    }
    for request in s.runtime.take_requests() {
        prism_core::PlatformServices::handle(&mut s.services, request);
    }
    s.renderer.widgets.ops.append(&mut s.services.ops);
    relayout(s);
    // Background tasks that finished.
    let done: Vec<String> = s
        .background
        .iter()
        .filter(|(_, task)| !s.runtime.is_task_running(*task))
        .map(|(id, _)| id.clone())
        .collect();
    s.background.retain(|(id, _)| !done.contains(id));
    for id in done {
        crate::services::complete_background(&id);
    }
    if let Some(at) = s.runtime.next_timer() {
        let delay = at.saturating_duration_since(std::time::Instant::now());
        let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
        let _ = dispatch2::DispatchQueue::main().after(when, poll);
    }
}

fn relayout(s: &mut State) {
    let (w, h) = s.size;
    let zoom = s.renderer.zoom();
    if (w / zoom, h / zoom) != s.runtime.app.viewport() {
        let patches = s.runtime.set_viewport(w / zoom, h / zoom);
        if !patches.is_empty() {
            s.renderer.apply(&patches);
        }
    }
    s.renderer.layout(w, h, Insets::default());
    let ops = std::mem::take(&mut s.renderer.widgets.ops);
    if !ops.is_empty() {
        let json = CString::new(Value::Array(ops).to_string()).unwrap_or_default();
        unsafe { prism_watch_apply(json.as_ptr()) };
    }
}

fn poll() {
    with_state(|s| {
        let patches = s.runtime.poll();
        commit(s, &patches);
    });
}

/// An event from the host (JSON).
#[unsafe(no_mangle)]
pub extern "C" fn prism_watch_event(json: *const c_char) {
    let text = unsafe { CStr::from_ptr(json) }.to_string_lossy();
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let num = |k: &str| v.get(k).and_then(Value::as_f64).map(|n| n as f32);
    match v.get("t").and_then(Value::as_str) {
        // The root's size (safe area, rotation of the crown side…).
        Some("resize") => {
            if let (Some(w), Some(h)) = (num("w"), num("h")) {
                dispatch2::DispatchQueue::main().exec_async(move || {
                    with_state(|s| {
                        if s.size != (w, h) {
                            s.size = (w, h);
                            relayout(s);
                        }
                    })
                });
            }
            return;
        }
        // The area a stack or tab view gives its screens.
        Some("viewport") => {
            if let (Some(id), Some(w), Some(h)) =
                (v.get("id").and_then(Value::as_u64), num("w"), num("h"))
            {
                dispatch2::DispatchQueue::main().exec_async(move || {
                    with_state(|s| {
                        if s.renderer.widgets.viewports.insert(id, (w, h)) != Some((w, h)) {
                            s.renderer.mark_all_dirty();
                            relayout(s);
                        }
                    })
                });
            }
            return;
        }
        // On screen, interrupted, in the background (SwiftUI scene phase).
        Some("lifecycle") => {
            let phase = match v.get("state").and_then(Value::as_str) {
                Some("background") => prism_core::Lifecycle::Background,
                Some("inactive") => prism_core::Lifecycle::Inactive,
                _ => prism_core::Lifecycle::Active,
            };
            dispatch2::DispatchQueue::main().exec_async(move || {
                with_state(|s| {
                    let patches = s.runtime.set_lifecycle(phase);
                    commit(s, &patches);
                })
            });
            return;
        }
        // Pressed, focused (`.active(|s| …)`…): restyled here, drawn there.
        Some("state") => {
            if let Some((id, state, on)) = prism_native::remote::state_event(&v) {
                dispatch2::DispatchQueue::main().exec_async(move || {
                    with_state(|s| {
                        if s.renderer.set_interaction(id, state, on) {
                            relayout(s);
                        }
                    })
                });
            }
            return;
        }
        _ => {}
    }
    let Some(event) = parse(&v) else { return };
    // SwiftUI may call while it updates views: run on the next turn.
    dispatch2::DispatchQueue::main().exec_async(move || {
        with_state(|s| {
            let patches = s.runtime.dispatch(event);
            commit(s, &patches);
        })
    });
}

fn parse(v: &Value) -> Option<UiEvent> {
    // Clicks with a position, pointer, drag and key events (#17).
    if let Some(event) = prism_native::remote::parse_input_event(v) {
        return Some(event);
    }
    let id = v.get("id")?.as_u64()?;
    Some(match v.get("t")?.as_str()? {
        "click" => UiEvent::Click(id),
        "input" => UiEvent::Input(id, v.get("v")?.as_str()?.to_string()),
        "change" => UiEvent::Change(id, v.get("v")?.as_f64()?),
        // The return key of a text input (SwiftUI `onSubmit`).
        "submit" => UiEvent::Submit(id),
        "back" => UiEvent::Back(id, v.get("depth")?.as_u64()? as usize),
        "menu" => UiEvent::MenuSelect(id),
        "alert" => UiEvent::AlertAction(id, v.get("index")?.as_u64()? as usize),
        "scroll" => UiEvent::Scroll(
            id,
            v.get("o")?.as_f64()? as f32,
            v.get("v")?.as_f64()? as f32,
        ),
        "swipe" => UiEvent::Swipe(
            id,
            if v.get("dir")?.as_str()? == "left" {
                prism_core::SwipeDirection::Left
            } else {
                prism_core::SwipeDirection::Right
            },
        ),
        _ => return None,
    })
}
