//! The few JNI calls the backend needs, over `jni-sys`.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;
use std::sync::OnceLock;

use jni_sys::{
    JNI_OK, JNI_VERSION_1_6, JNIEnv, JavaVM, jclass, jfloat, jint, jlong, jmethodID, jstring,
    jvalue,
};

struct Vm(*mut JavaVM);
// SAFETY: a JavaVM pointer is valid on every thread (JNI spec).
unsafe impl Send for Vm {}
unsafe impl Sync for Vm {}

struct Host {
    class: jclass,
    apply: jmethodID,
    measure: jmethodID,
    post: jmethodID,
    job_finished: jmethodID,
}
// SAFETY: a global class reference and method ids are valid on every thread.
unsafe impl Send for Host {}
unsafe impl Sync for Host {}

static VM: OnceLock<Vm> = OnceLock::new();
static HOST: OnceLock<Host> = OnceLock::new();

/// Called by Android when the library loads.
#[unsafe(no_mangle)]
pub extern "system" fn JNI_OnLoad(vm: *mut JavaVM, _reserved: *mut c_void) -> jint {
    let _ = VM.set(Vm(vm));
    JNI_VERSION_1_6
}

/// The calling thread's `JNIEnv`, attaching it if needed.
pub(crate) fn env() -> Option<*mut JNIEnv> {
    let vm = VM.get()?.0;
    let mut env: *mut c_void = null_mut();
    unsafe {
        let iface = (**vm).v1_4;
        if (iface.GetEnv)(vm, &mut env, JNI_VERSION_1_6) != JNI_OK
            && (iface.AttachCurrentThread)(vm, &mut env, null_mut()) != JNI_OK
        {
            return None;
        }
    }
    Some(env.cast())
}

unsafe fn check(env: *mut JNIEnv) {
    unsafe {
        if ((**env).v1_6.ExceptionCheck)(env) {
            ((**env).v1_6.ExceptionDescribe)(env);
            ((**env).v1_6.ExceptionClear)(env);
        }
    }
}

/// Looks up `dev.prism.PrismHost` (from a Java thread: native threads cannot
/// see the app's classes).
pub(crate) fn init_host(env: *mut JNIEnv) {
    if HOST.get().is_some() {
        return;
    }
    unsafe {
        let e = (**env).v1_6;
        let local = (e.FindClass)(env, c"dev/prism/PrismHost".as_ptr());
        check(env);
        let class = (e.NewGlobalRef)(env, local) as jclass;
        let method = |name: &CStr, sig: &CStr| {
            (e.GetStaticMethodID)(env, class, name.as_ptr(), sig.as_ptr())
        };
        let host = Host {
            class,
            apply: method(c"apply", c"(Ljava/lang/String;)V"),
            measure: method(c"measure", c"(Ljava/lang/String;Ljava/lang/String;FIF)[F"),
            post: method(c"post", c"(J)V"),
            job_finished: method(c"jobFinished", c"(Ljava/lang/String;Z)V"),
        };
        check(env);
        let _ = HOST.set(host);
    }
}

unsafe fn string(env: *mut JNIEnv, s: &str) -> jstring {
    let c = CString::new(s.replace('\0', "")).unwrap_or_default();
    unsafe { ((**env).v1_6.NewStringUTF)(env, c.as_ptr()) }
}

/// A Java string as Rust.
pub(crate) unsafe fn rust_string(env: *mut JNIEnv, s: jstring) -> String {
    if s.is_null() {
        return String::new();
    }
    unsafe {
        let chars = ((**env).v1_6.GetStringUTFChars)(env, s, null_mut());
        let out = CStr::from_ptr(chars as *const c_char)
            .to_string_lossy()
            .into_owned();
        ((**env).v1_6.ReleaseStringUTFChars)(env, s, chars);
        out
    }
}

/// `PrismHost.apply(json)`: views to create, change, place.
pub(crate) fn apply(json: &str) {
    let (Some(env), Some(h)) = (env(), HOST.get()) else {
        return;
    };
    unsafe {
        let s = string(env, json);
        ((**env).v1_6.CallStaticVoidMethodA)(env, h.class, h.apply, [jvalue { l: s }].as_ptr());
        check(env);
        ((**env).v1_6.DeleteLocalRef)(env, s);
    }
}

/// `PrismHost.measure(...)`: a leaf's natural size in pixels.
pub(crate) fn measure(
    kind: &str,
    text: &str,
    size: f32,
    weight: i32,
    max_width: Option<f32>,
) -> (f32, f32) {
    let (Some(env), Some(h)) = (env(), HOST.get()) else {
        return (0., 0.);
    };
    unsafe {
        let (k, t) = (string(env, kind), string(env, text));
        let args = [
            jvalue { l: k },
            jvalue { l: t },
            jvalue { f: size },
            jvalue { i: weight },
            jvalue {
                f: max_width.unwrap_or(-1.),
            },
        ];
        let array = ((**env).v1_6.CallStaticObjectMethodA)(env, h.class, h.measure, args.as_ptr());
        check(env);
        let mut out: [jfloat; 2] = [0., 0.];
        if !array.is_null() {
            ((**env).v1_6.GetFloatArrayRegion)(env, array, 0, 2, out.as_mut_ptr());
            ((**env).v1_6.DeleteLocalRef)(env, array);
        }
        ((**env).v1_6.DeleteLocalRef)(env, k);
        ((**env).v1_6.DeleteLocalRef)(env, t);
        (out[0], out[1])
    }
}

/// `PrismHost.post(delay)`: calls `nativeWake` on the main thread later.
/// Works from any thread.
pub(crate) fn post(delay_ms: i64) {
    let (Some(env), Some(h)) = (env(), HOST.get()) else {
        return;
    };
    unsafe {
        ((**env).v1_6.CallStaticVoidMethodA)(
            env,
            h.class,
            h.post,
            [jvalue {
                j: delay_ms as jlong,
            }]
            .as_ptr(),
        );
        check(env);
    }
}

/// `PrismHost.jobFinished(task, ok)`: a background job is done.
pub(crate) fn job_finished(task: &str, ok: bool) {
    let (Some(env), Some(h)) = (env(), HOST.get()) else {
        return;
    };
    unsafe {
        let t = string(env, task);
        let args = [jvalue { l: t }, jvalue { z: ok }];
        ((**env).v1_6.CallStaticVoidMethodA)(env, h.class, h.job_finished, args.as_ptr());
        check(env);
        ((**env).v1_6.DeleteLocalRef)(env, t);
    }
}
