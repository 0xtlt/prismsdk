//! The app's runtime on Android's main thread, driven by the Java host.

use std::cell::RefCell;
use std::sync::OnceLock;

use jni_sys::{JNIEnv, jbyteArray, jclass, jfloat, jsize, jstring};
use prism_core::{
    AppConfig, AppInit, Backend, DeviceInfo, DeviceKind, Orientation, Os, OsName, Runtime,
    ScreenInfo, UiEvent, WindowMode,
};
use prism_native::remote::{MeasureRequest, RemoteWidgets};
use prism_native::{Insets, NativeRenderer};
use serde_json::Value;

use crate::jni;

static APP: OnceLock<(AppConfig, AppInit)> = OnceLock::new();

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<RemoteWidgets>,
    services: crate::services::AndroidServices,
    /// The root's size in pixels.
    size: (f32, f32),
    // --- Safe area (#28) ---
    /// Around the root in the window (bars, cutouts, keyboard), and what of
    /// it the system covers for good (not the keyboard): pixels.
    pad: prism_core::Edges,
    safe: prism_core::Edges,
}

#[derive(Clone)]
struct Job {
    handle: u64,
    task: String,
    /// In the runtime without a screen (handles are per runtime).
    headless: bool,
}

/// Whether `job`'s task still runs.
fn job_running(job: &Job) -> bool {
    let mut running = false;
    if job.headless {
        HEADLESS.with(|h| {
            if let Ok(h) = h.try_borrow()
                && let Some(h) = h.as_ref()
            {
                running = h.runtime.is_task_running(job.handle);
            }
        });
    } else {
        with_state(|s| running = s.runtime.is_task_running(job.handle));
    }
    running
}

/// The app without a screen: the system started it for a background job.
struct Headless {
    runtime: Runtime,
    services: crate::services::AndroidServices,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static HEADLESS: RefCell<Option<Headless>> = const { RefCell::new(None) };
    /// Background jobs in progress.
    static JOBS: RefCell<Vec<Job>> = const { RefCell::new(Vec::new()) };
}

/// Called from the app's `main` (through `prism::android_main!`): the Java
/// host drives everything else.
pub fn run(config: AppConfig, init: AppInit) {
    let _ = APP.set((config, init));
}

fn measure(r: &MeasureRequest) -> (f32, f32) {
    // Images are drawn as their glyph (SF Symbol names do not exist here).
    let text = match r.kind {
        "image" => prism_core::glyphs::glyph(r.text),
        _ => r.text,
    };
    jni::measure(r.kind, text, r.font_size, r.weight, r.max_width, r.spec)
}

/// What the host tells about the device (JSON from `PrismHost.deviceInfo`).
fn device_info(v: &Value, w: f32, h: f32, density: f32) -> DeviceInfo {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let n = |k: &str| v.get(k).and_then(Value::as_f64).map(|x| x as f32);
    // Physical size from the screen's real pixels and dots per inch.
    let (pw, ph) = (n("widthPx").unwrap_or(w), n("heightPx").unwrap_or(h));
    let diagonal = match (n("xdpi"), n("ydpi")) {
        (Some(x), Some(y)) if x > 0. && y > 0. => {
            let d = ((pw / x).powi(2) + (ph / y).powi(2)).sqrt();
            Some((d * 10.).round() / 10.)
        }
        _ => None,
    };
    // The form factor, then Android's own rule: 600dp and wider is a tablet.
    let kind = match (s("uiType").as_deref(), n("smallestWidthDp")) {
        (Some("watch"), _) => DeviceKind::Watch,
        (Some("tv"), _) => DeviceKind::Tv,
        (_, Some(sw)) if sw >= 600. => DeviceKind::Tablet,
        _ => DeviceKind::Phone,
    };
    DeviceInfo {
        kind,
        os: Os {
            name: OsName::Android,
            version: prism_core::Version::parse(&s("release").unwrap_or_default()),
        },
        model: s("model"),
        model_name: match (s("manufacturer"), s("model")) {
            (Some(m), Some(model)) => Some(format!("{m} {model}")),
            _ => None,
        },
        simulator: v.get("emulator").and_then(Value::as_bool).unwrap_or(false),
        orientation: Orientation::of(pw, ph),
        screen: ScreenInfo {
            width: pw / density,
            height: ph / density,
            scale: density,
            diagonal,
            ppi: n("xdpi").map(f32::round),
            refresh_rate: n("refresh"),
            cutout: v.get("cutout").and_then(Value::as_bool).unwrap_or(false),
        },
        window_mode: if v
            .get("multiWindow")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            WindowMode::Split
        } else {
            WindowMode::Fullscreen
        },
        touch: v.get("touch").and_then(Value::as_bool).unwrap_or(true),
        ..Default::default()
    }
}

/// The host's first layout: start the app.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativeReady(
    env: *mut JNIEnv,
    _class: jclass,
    width: jfloat,
    height: jfloat,
    density: jfloat,
    info: jstring,
) {
    jni::init_host(env);
    let info: Value =
        serde_json::from_str(&unsafe { jni::rust_string(env, info) }).unwrap_or_default();
    let Some((_, init)) = APP.get() else {
        eprintln!("prism-android: the app's main did not call Application::run");
        return;
    };
    let density = density.max(0.5);
    let services = crate::services::AndroidServices::new(&info);
    let runtime = make_runtime(
        init,
        &info,
        &services,
        Some((width / density, height / density)),
        Some(device_info(&info, width, height, density)),
    );
    let mut renderer = NativeRenderer::new(RemoteWidgets::new(measure));
    renderer.set_zoom(density);
    STATE.with(|st| {
        *st.borrow_mut() = Some(State {
            runtime,
            renderer,
            services,
            size: (width, height),
            pad: prism_core::Edges::default(),
            safe: prism_core::Edges::default(),
        })
    });
    with_state(|s| {
        let patches = s.runtime.render();
        commit(s, &patches);
    });
}

/// A runtime from what the host tells (`info`): storage, language, path.
/// `viewport` is in dp; `None` without a screen.
fn make_runtime(
    init: &AppInit,
    info: &Value,
    services: &crate::services::AndroidServices,
    viewport: Option<(f32, f32)>,
    device_info: Option<DeviceInfo>,
) -> Runtime {
    let s = |k: &str| info.get(k).and_then(Value::as_str).map(str::to_string);
    let data_dir = std::path::PathBuf::from(s("dataDir").unwrap_or_else(|| ".".into()));
    let environment = prism_core::Environment {
        locale: prism_core::Locale::new(
            &std::env::var("PRISM_LOCALE")
                .ok()
                .or(s("locale"))
                .unwrap_or_else(|| "en".into()),
        ),
        utc_offset: info.get("utcOffset").and_then(Value::as_i64).unwrap_or(0) as i32,
        text_scale: info.get("fontScale").and_then(Value::as_f64).unwrap_or(1.) as f32,
        reduce_motion: info
            .get("reduceMotion")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    };
    Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            path: s("path").unwrap_or_else(|| "/".into()),
            capabilities: prism_core::PlatformServices::capabilities(services),
            storage: Some(Box::new(prism_core::FileStorage::new(data_dir.clone()))),
            // files/cloud, backed up key by key (PrismBackup.java).
            cloud_storage: Some(Box::new(BackedUp(prism_core::FileStorage::new(
                data_dir.join("cloud"),
            )))),
            wake_ui: Some(std::sync::Arc::new(|| jni::post(0))),
            environment,
            data_dir: Some(data_dir),
            // The app works in dp; the renderer hands pixels to the host.
            viewport,
            device_info,
            ..Default::default()
        },
    )
}

/// `cloud_storage`: files the Backup Service backs up; each change tells it.
struct BackedUp(prism_core::FileStorage);

impl prism_core::Storage for BackedUp {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key)
    }
    fn set(&mut self, key: &str, value: &str) {
        self.0.set(key, value);
        jni::cloud_changed();
    }
    fn remove(&mut self, key: &str) {
        self.0.remove(key);
        jni::cloud_changed();
    }
    fn keys(&self) -> Vec<String> {
        self.0.keys()
    }
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

/// Applies patches, lays out, sends the changes to the host, schedules the
/// next timer.
fn commit(s: &mut State, patches: &[prism_core::Patch]) {
    if !patches.is_empty() {
        s.renderer.apply(patches);
    }
    for request in s.runtime.take_requests() {
        if let prism_core::PlatformRequest::LoadFont(face) = request {
            load_font(s, face);
            continue;
        }
        prism_core::PlatformServices::handle(&mut s.services, request);
    }
    s.renderer.widgets.ops.append(&mut s.services.ops);
    relayout(s);
    if let Some(at) = s.runtime.next_timer() {
        jni::post(
            at.saturating_duration_since(std::time::Instant::now())
                .as_millis() as i64,
        );
    }
}

/// Registers a run-time font (`cx.load_font`): saved under the app's data
/// directory, handed to the host now (`PrismText.addFont`, so the next
/// measures use it), then every node's style is sent again. A URL comes back
/// here with its bytes once loaded.
fn load_font(s: &mut State, face: prism_core::FontFace) {
    let sources = prism_native::images::ImageSources::default();
    let Some(font) = prism_native::fonts::prepare(&mut s.runtime.app, face, &sources) else {
        return;
    };
    let dir = s
        .runtime
        .app
        .data_dir()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
        .join(".prism-fonts");
    let Some(path) = prism_native::fonts::save(&font, &dir) else {
        eprintln!("prism-android: font {:?}: cannot save it", font.family);
        return;
    };
    let op = serde_json::json!([{
        "op": "font",
        "path": path.to_string_lossy(),
        "alias": font.face.family,
    }]);
    jni::apply(&op.to_string());
    s.renderer.restyle_all();
}

fn relayout(s: &mut State) {
    let (w, h) = s.size;
    let zoom = s.renderer.zoom();
    // The window is the root plus what surrounds it; the app's viewport is
    // what the bars leave (the keyboard does not shrink it).
    let (p, i) = (s.pad, s.safe);
    let full = ((w + p.left + p.right) / zoom, (h + p.top + p.bottom) / zoom);
    let safe = prism_core::Edges {
        top: i.top / zoom,
        right: i.right / zoom,
        bottom: i.bottom / zoom,
        left: i.left / zoom,
    };
    if full != s.runtime.app.window_size() || safe != s.runtime.app.safe_area() {
        let patches = s.runtime.set_window(full.0, full.1, safe);
        if !patches.is_empty() {
            s.renderer.apply(&patches);
        }
    }
    s.renderer.layout(w, h, Insets::default());
    let ops = std::mem::take(&mut s.renderer.widgets.ops);
    if !ops.is_empty() {
        jni::apply(&Value::Array(ops).to_string());
    }
    // Elements that report their frame and moved (#192).
    let more = prism_native::report_layout(&mut s.runtime, &mut s.renderer);
    if !more.is_empty() {
        commit(s, &more);
    }
}

/// Async work or a timer is due.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativeWake(_env: *mut JNIEnv, _class: jclass) {
    with_state(|s| {
        for id in s.services.due_background() {
            s.runtime.run_background_task(&id);
        }
        let patches = s.runtime.poll();
        commit(s, &patches);
    });
    poll_headless();
    check_jobs();
}

/// `PrismHost.sceneBlob(id)`: a 3D scene's blob (mesh, instances, clip,
/// skin, environment, embedded image; `prism_core::scene3d::blobs`), or
/// null once nothing holds it. Any thread: PrismScene loads blobs off the
/// main thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_sceneBlob(
    env: *mut JNIEnv,
    _class: jclass,
    id: jstring,
) -> jbyteArray {
    let id = unsafe { jni::rust_string(env, id) };
    let Some((bytes, _)) = prism_core::scene3d::blobs::get(&id) else {
        return std::ptr::null_mut();
    };
    let Ok(len) = jsize::try_from(bytes.len()) else {
        return std::ptr::null_mut();
    };
    unsafe {
        let e = (**env).v1_6;
        // Null with an OutOfMemoryError pending: Java sees the error.
        let array = (e.NewByteArray)(env, len);
        if !array.is_null() {
            (e.SetByteArrayRegion)(env, array, 0, len, bytes.as_ptr().cast());
        }
        array
    }
}

// --- Background jobs (PrismJob.java) ---------------------------------------------------

/// The system runs background task `task` (JobScheduler). In the open app's
/// runtime, or in one without a screen when the app is closed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativeBackground(
    env: *mut JNIEnv,
    _class: jclass,
    task: jstring,
    info: jstring,
) {
    jni::init_host(env);
    let task = unsafe { jni::rust_string(env, task) };
    let mut started = None;
    let mut open = false;
    with_state(|s| {
        open = true;
        started = s.runtime.run_background_task(&task);
        let patches = s.runtime.poll();
        commit(s, &patches);
    });
    if !open {
        let Some((_, init)) = APP.get() else {
            jni::job_finished(&task, false);
            return;
        };
        let info: Value =
            serde_json::from_str(&unsafe { jni::rust_string(env, info) }).unwrap_or_default();
        HEADLESS.with(|h| {
            let mut h = h.borrow_mut();
            let headless = h.get_or_insert_with(|| {
                let services = crate::services::AndroidServices::new(&info);
                let runtime = make_runtime(init, &info, &services, None, None);
                Headless { runtime, services }
            });
            started = headless.runtime.run_background_task(&task);
        });
        poll_headless();
    }
    match started {
        Some(handle) => JOBS.with(|j| {
            j.borrow_mut().push(Job {
                handle,
                task,
                headless: !open,
            })
        }),
        // Not registered (any more): nothing to do.
        None => jni::job_finished(&task, true),
    }
    check_jobs();
}

/// A widget button was tapped (PrismWidget.Action, a broadcast that keeps
/// the process alive until `task` finishes): runs the app's
/// `on_widget_action` job in the open app's runtime, or in one without a
/// screen when the app is closed. `task` is reported done through
/// `PrismHost.jobFinished`, like a background job.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativeWidgetAction(
    env: *mut JNIEnv,
    _class: jclass,
    task: jstring,
    kind: jstring,
    action: jstring,
    payload: jstring,
    info: jstring,
) {
    jni::init_host(env);
    let task = unsafe { jni::rust_string(env, task) };
    let action = prism_core::WidgetAction {
        kind: unsafe { jni::rust_string(env, kind) },
        action: unsafe { jni::rust_string(env, action) },
        payload: unsafe { jni::rust_string(env, payload) },
    };
    let mut action = Some(action);
    let mut started = None;
    with_state(|s| {
        if let Some(a) = action.take() {
            started = Some(s.runtime.run_widget_action(a));
            let patches = s.runtime.poll();
            commit(s, &patches);
        }
    });
    let open = action.is_none();
    if let Some(a) = action {
        let Some((_, init)) = APP.get() else {
            jni::job_finished(&task, false);
            return;
        };
        let info: Value =
            serde_json::from_str(&unsafe { jni::rust_string(env, info) }).unwrap_or_default();
        HEADLESS.with(|h| {
            let mut h = h.borrow_mut();
            let headless = h.get_or_insert_with(|| {
                let services = crate::services::AndroidServices::new(&info);
                let runtime = make_runtime(init, &info, &services, None, None);
                Headless { runtime, services }
            });
            started = Some(headless.runtime.run_widget_action(a));
        });
        poll_headless();
    }
    if let Some(handle) = started {
        JOBS.with(|j| {
            j.borrow_mut().push(Job {
                handle,
                task,
                headless: !open,
            })
        });
    }
    check_jobs();
}

/// The car (Car App Library): connected, an item selected, Back. In the
/// open app's runtime, or one without a screen (the car can start the app).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativeCarEvent(
    env: *mut JNIEnv,
    _class: jclass,
    kind: jstring,
    value: jni_sys::jlong,
    info: jstring,
) {
    jni::init_host(env);
    let kind = unsafe { jni::rust_string(env, kind) };
    let event = move |rt: &mut Runtime| match kind.as_str() {
        "connected" => rt.car_connected(value != 0),
        "select" => rt.car_select(value as u64),
        "back" => rt.car_back(value as usize),
        _ => Vec::new(),
    };
    let mut event = Some(event);
    with_state(|s| {
        if let Some(f) = event.take() {
            let patches = f(&mut s.runtime);
            commit(s, &patches);
        }
    });
    let Some(f) = event else { return };
    let Some((_, init)) = APP.get() else { return };
    let info: Value =
        serde_json::from_str(&unsafe { jni::rust_string(env, info) }).unwrap_or_default();
    HEADLESS.with(|h| {
        let mut h = h.borrow_mut();
        let headless = h.get_or_insert_with(|| {
            let mut info = info.clone();
            info["car"] = Value::Bool(true);
            let services = crate::services::AndroidServices::new(&info);
            let runtime = make_runtime(init, &info, &services, None, None);
            Headless { runtime, services }
        });
        let _ = f(&mut headless.runtime);
    });
    poll_headless();
}

/// Remote push from the host (PrismPush / PrismFcm): a token or endpoint, an
/// error, or a message. In the open app's runtime, or one without a screen
/// (the push woke the app). Visible pushes become notifications unless the
/// app is on screen; silent ones run `on_background_push`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativePush(
    env: *mut JNIEnv,
    _class: jclass,
    json: jstring,
    info: jstring,
) {
    jni::init_host(env);
    let Ok(v) = serde_json::from_str::<Value>(&unsafe { jni::rust_string(env, json) }) else {
        return;
    };
    let info: Value =
        serde_json::from_str(&unsafe { jni::rust_string(env, info) }).unwrap_or_default();
    let data_dir =
        std::path::PathBuf::from(info.get("dataDir").and_then(Value::as_str).unwrap_or("."));
    let Some(event) = crate::services::push_event(&v, &data_dir) else {
        return;
    };
    let apply = |rt: &mut Runtime| match event {
        crate::services::PushEvent::Registered(result) => rt.push_registered(result),
        crate::services::PushEvent::Message(m) if m.event == prism_core::PushEvent::Background => {
            rt.run_background_push(m);
            rt.poll()
        }
        crate::services::PushEvent::Message(m) => rt.push_received(m, true),
    };
    let mut apply = Some(apply);
    with_state(|s| {
        if let Some(f) = apply.take() {
            let patches = f(&mut s.runtime);
            commit(s, &patches);
        }
    });
    let Some(f) = apply else { return };
    let Some((_, init)) = APP.get() else { return };
    HEADLESS.with(|h| {
        let mut h = h.borrow_mut();
        let headless = h.get_or_insert_with(|| {
            let services = crate::services::AndroidServices::new(&info);
            let mut runtime = make_runtime(init, &info, &services, None, None);
            // No screen: pushes are shown as notifications.
            runtime.set_lifecycle(prism_core::Lifecycle::Background);
            Headless { runtime, services }
        });
        let _ = f(&mut headless.runtime);
    });
    poll_headless();
}

/// The system stopped the job (its time ran out): stop the task.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativeBackgroundExpired(
    env: *mut JNIEnv,
    _class: jclass,
    task: jstring,
) {
    let task = unsafe { jni::rust_string(env, task) };
    let expired: Vec<Job> = JOBS.with(|j| {
        let mut j = j.borrow_mut();
        let expired = j.iter().filter(|job| job.task == task).cloned().collect();
        j.retain(|job| job.task != task);
        expired
    });
    for job in expired {
        if job.headless {
            HEADLESS.with(|h| {
                if let Some(h) = h.borrow_mut().as_mut() {
                    h.runtime.cancel_task(job.handle);
                }
            });
        } else {
            with_state(|s| s.runtime.cancel_task(job.handle));
        }
    }
}

/// Async work of the runtime without a screen, and what it asks the system
/// (notifications…).
fn poll_headless() {
    HEADLESS.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return;
        };
        let Some(h) = guard.as_mut() else {
            return;
        };
        let _ = h.runtime.poll();
        for request in h.runtime.take_requests() {
            prism_core::PlatformServices::handle(&mut h.services, request);
        }
        h.services.due_background();
        let ops = std::mem::take(&mut h.services.ops);
        if !ops.is_empty() {
            jni::apply(&Value::Array(ops).to_string());
        }
        if let Some(at) = h.runtime.next_timer() {
            jni::post(
                at.saturating_duration_since(std::time::Instant::now())
                    .as_millis() as i64,
            );
        }
    });
}

/// Tells the system about jobs whose task finished.
fn check_jobs() {
    let done: Vec<Job> = JOBS.with(|j| {
        let mut j = j.borrow_mut();
        let (done, running): (Vec<Job>, Vec<Job>) = j.drain(..).partition(|job| !job_running(job));
        *j = running;
        done
    });
    for job in done {
        jni::job_finished(&job.task, true);
    }
}

/// An event from the host (JSON), on the main thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_prism_PrismHost_nativeEvent(
    env: *mut JNIEnv,
    _class: jclass,
    json: jstring,
) {
    let text = unsafe { jni::rust_string(env, json) };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let num = |k: &str| v.get(k).and_then(Value::as_f64).map(|n| n as f32);
    with_state(|s| {
        let patches = match v.get("t").and_then(Value::as_str) {
            // The root's size (rotation, split screen, keyboard).
            Some("resize") => {
                if let (Some(w), Some(h)) = (num("w"), num("h")) {
                    s.size = (w, h);
                }
                let edges = |k: &str| {
                    let side = |i: usize| {
                        v.get(k)
                            .and_then(|a| a.get(i))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.) as f32
                    };
                    prism_core::Edges {
                        top: side(0),
                        right: side(1),
                        bottom: side(2),
                        left: side(3),
                    }
                };
                s.pad = edges("pad");
                s.safe = edges("safe");
                Vec::new()
            }
            // A video's size, once its player knows it (#207).
            Some("video_size") => {
                if let (Some(id), Some(w), Some(h)) =
                    (v.get("id").and_then(Value::as_u64), num("w"), num("h"))
                    && s.renderer.widgets.video_sizes.insert(id, (w, h)) != Some((w, h))
                {
                    s.renderer.mark_all_dirty();
                }
                Vec::new()
            }
            // The area a stack, tab view or modal gives its screens.
            Some("viewport") => {
                if let (Some(id), Some(w), Some(h)) =
                    (v.get("id").and_then(Value::as_u64), num("w"), num("h"))
                    && s.renderer.widgets.viewports.insert(id, (w, h)) != Some((w, h))
                {
                    s.renderer.mark_all_dirty();
                }
                Vec::new()
            }
            Some("lifecycle") => {
                s.runtime
                    .set_lifecycle(match v.get("state").and_then(Value::as_str) {
                        Some("background") => prism_core::Lifecycle::Background,
                        Some("inactive") => prism_core::Lifecycle::Inactive,
                        _ => prism_core::Lifecycle::Active,
                    })
            }
            Some("link") => match v.get("path").and_then(Value::as_str) {
                Some(path) => s.runtime.open_link(path),
                None => Vec::new(),
            },
            // Hovered, pressed, focused: restyled in its new states (the
            // style ops go out with the relayout below).
            Some("state") => {
                if let Some((id, state, on)) = prism_native::remote::state_event(&v) {
                    s.renderer.set_interaction(id, state, on);
                }
                // The app hears focus changes (#224).
                let mut patches = Vec::new();
                for e in s.renderer.take_focus_changes() {
                    patches.extend(s.runtime.dispatch(e));
                }
                patches
            }
            Some(t) if t.starts_with("device") || t == "notif" => {
                crate::services::event(&mut s.runtime, &v)
            }
            _ => match parse(&v) {
                Some(event) => s.runtime.dispatch(event),
                None => Vec::new(),
            },
        };
        commit(s, &patches);
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
        // The return key of a text field.
        "submit" => UiEvent::Submit(id),
        "input" => UiEvent::Input(id, v.get("v")?.as_str()?.to_string()),
        // `null`: an optional date picker was cleared (#89).
        "change" => UiEvent::Change(id, prism_native::remote::change_value(v.get("v")?)?),
        // An overlay closed by the user says how (#156).
        "back" => match v.get("reason").and_then(Value::as_str) {
            Some(reason) => UiEvent::Dismiss(id, prism_core::DismissReason::from_name(reason)),
            None => UiEvent::Back(id, v.get("depth")?.as_u64()? as usize),
        },
        "menu" => UiEvent::MenuSelect(id),
        // A video played to the end (#207).
        "ended" => UiEvent::Ended(id),
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
