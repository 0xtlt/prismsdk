//! `PRISM_TRACE=1`: the startup phases (the app's tree, the views and
//! their layout, the first frame) and the memory, logged like
//! prism-windows' so `scripts/macos-perf.sh` compares them. Times count
//! from the process's creation.

use std::ffi::c_void;
use std::time::{Duration, Instant};

/// Whether `PRISM_TRACE` is set.
pub fn on() -> bool {
    std::env::var_os("PRISM_TRACE").is_some_and(|v| !v.is_empty())
}

/// Logs `what` with the time since the process started.
pub fn log(what: &str) {
    if on() {
        eprintln!("prism-macos: {what} after {:.0} ms", since_start());
    }
}

/// Milliseconds since `t`.
pub fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}

/// `rusage_info_v4`: a UUID, then 64-bit counters (more room than the
/// kernel fills).
#[repr(C)]
struct Usage {
    uuid: [u8; 16],
    counters: [u64; 64],
}

const RUSAGE_INFO_V4: i32 = 4;
const RESIDENT: usize = 6;
const FOOTPRINT: usize = 7;
const STARTED: usize = 8;

unsafe extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut c_void) -> i32;
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut [u32; 2]) -> i32;
}

fn usage() -> Option<Usage> {
    let mut u = Usage {
        uuid: [0; 16],
        counters: [0; 64],
    };
    let r = unsafe {
        proc_pid_rusage(
            std::process::id() as i32,
            RUSAGE_INFO_V4,
            &mut u as *mut Usage as *mut c_void,
        )
    };
    (r == 0).then_some(u)
}

/// Milliseconds since the process was created.
pub fn since_start() -> f64 {
    let Some(u) = usage() else { return 0. };
    let mut base = [0u32; 2];
    unsafe {
        mach_timebase_info(&mut base);
    }
    let ticks = unsafe { mach_absolute_time() }.saturating_sub(u.counters[STARTED]);
    let ns = ticks as u128 * base[0].max(1) as u128 / base[1].max(1) as u128;
    Duration::from_nanos(ns as u64).as_secs_f64() * 1000.
}

/// The memory, named like Windows' counters: the resident size as the
/// working set, the footprint (what Activity Monitor shows) as private.
pub fn memory() -> String {
    let Some(u) = usage() else {
        return String::new();
    };
    let mb = |b: u64| b as f64 / 1048576.;
    format!(
        "working set {:.1} MB, private {:.1} MB",
        mb(u.counters[RESIDENT]),
        mb(u.counters[FOOTPRINT])
    )
}
