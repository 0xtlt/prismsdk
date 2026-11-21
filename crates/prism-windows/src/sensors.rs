//! Motion (`watch_motion`): the PC's motion sensors
//! (`Windows.Devices.Sensors`), which tablets and 2-in-1s have and most
//! laptops and desktops do not: `Accelerometer` (its fused `Gravity` and
//! `Linear` readings where Windows has them), `Gyrometer`, `Inclinometer`.
//! They need no permission and no package identity. A watch reads them
//! every `interval` on its own thread (`ReportInterval` set to it, or to
//! the sensors' minimum) and sends Prism's units (`device::Fusion`).
//!
//! `capabilities().motion` says the PC has an accelerometer or a
//! gyrometer; without one, `watch_motion` answers `Unavailable`, as on a
//! desktop browser.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use prism_core::{DeviceError, DeviceEvent};
use windows::Devices::Sensors::{Accelerometer, AccelerometerReadingType, Gyrometer, Inclinometer};

use crate::device::{Fusion, Sensors};
use crate::win::log;

/// The sensors a watch reads.
struct Devices {
    acceleration: Option<Accelerometer>,
    gravity: Option<Accelerometer>,
    linear: Option<Accelerometer>,
    rotation: Option<Gyrometer>,
    inclination: Option<Inclinometer>,
}

impl Devices {
    fn find() -> Devices {
        let fused = |t| Accelerometer::GetDefaultWithAccelerometerReadingType(t).ok();
        Devices {
            acceleration: Accelerometer::GetDefault().ok(),
            gravity: fused(AccelerometerReadingType::Gravity),
            linear: fused(AccelerometerReadingType::Linear),
            rotation: Gyrometer::GetDefault().ok(),
            inclination: Inclinometer::GetDefault().ok(),
        }
    }

    fn any(&self) -> bool {
        self.acceleration.is_some() || self.rotation.is_some()
    }

    /// Asks each sensor for readings every `ms` (its minimum if larger);
    /// 0 lets them rest.
    fn report_every(&self, ms: u32) {
        let set = |min: windows::core::Result<u32>,
                   set: &dyn Fn(u32) -> windows::core::Result<()>| {
            let ms = if ms == 0 { 0 } else { ms.max(min.unwrap_or(0)) };
            let _ = set(ms);
        };
        for a in [&self.acceleration, &self.gravity, &self.linear]
            .into_iter()
            .flatten()
        {
            set(a.MinimumReportInterval(), &|ms| a.SetReportInterval(ms));
        }
        if let Some(g) = &self.rotation {
            set(g.MinimumReportInterval(), &|ms| g.SetReportInterval(ms));
        }
        if let Some(i) = &self.inclination {
            set(i.MinimumReportInterval(), &|ms| i.SetReportInterval(ms));
        }
    }

    fn read(&self) -> Sensors {
        let accel = |a: &Option<Accelerometer>| {
            let r = a.as_ref()?.GetCurrentReading().ok()?;
            Some([
                r.AccelerationX().ok()?,
                r.AccelerationY().ok()?,
                r.AccelerationZ().ok()?,
            ])
        };
        Sensors {
            acceleration: accel(&self.acceleration),
            gravity: accel(&self.gravity),
            linear: accel(&self.linear),
            rotation_deg: self.rotation.as_ref().and_then(|g| {
                let r = g.GetCurrentReading().ok()?;
                Some([
                    r.AngularVelocityX().ok()?,
                    r.AngularVelocityY().ok()?,
                    r.AngularVelocityZ().ok()?,
                ])
            }),
            inclination_deg: self.inclination.as_ref().and_then(|i| {
                let r = i.GetCurrentReading().ok()?;
                Some([
                    r.PitchDegrees().ok()? as f64,
                    r.RollDegrees().ok()? as f64,
                    r.YawDegrees().ok()? as f64,
                ])
            }),
        }
    }
}

/// Whether the PC has an accelerometer or a gyrometer. Blocking: called
/// off the UI thread at startup.
pub fn present() -> bool {
    Devices::find().any()
}

/// What `PRISM_DEVICE_REPORT` says of motion: which sensors Windows has,
/// and a watch's first sample's parts when it has one (no values).
pub fn report() -> serde_json::Value {
    let d = Devices::find();
    let sample = d.any().then(|| {
        d.report_every(50);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut fusion = Fusion::default();
        let found = loop {
            if let Some(m) = fusion.sample(&d.read(), 0.05) {
                break Some(m);
            }
            if Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        d.report_every(0);
        found.is_some()
    });
    serde_json::json!({
        "accelerometer": d.acceleration.is_some(),
        "gravity_sensor": d.gravity.is_some(),
        "linear_acceleration_sensor": d.linear.is_some(),
        "gyrometer": d.rotation.is_some(),
        "inclinometer": d.inclination.is_some(),
        "first_sample": sample,
    })
}

thread_local! {
    /// Running watches by call id: their stop flags. UI thread.
    static WATCHES: RefCell<HashMap<u64, Arc<AtomicBool>>> = RefCell::new(HashMap::new());
}

/// Starts watch `id`: a sample about every `interval` to `send`, on the UI
/// thread, until `stop(id)`. UI thread.
pub fn watch(id: u64, interval: Duration, send: fn(u64, DeviceEvent)) {
    let stop = Arc::new(AtomicBool::new(false));
    WATCHES.with(|w| w.borrow_mut().insert(id, stop.clone()));
    let every = interval.max(Duration::from_millis(10));
    std::thread::spawn(move || {
        let d = Devices::find();
        if !d.any() {
            return crate::shell::on_ui(move || {
                deliver(id, DeviceEvent::Error(DeviceError::Unavailable), send)
            });
        }
        d.report_every(every.as_millis().min(u32::MAX as u128) as u32);
        let mut fusion = Fusion::default();
        let mut last = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let dt = now.duration_since(last).as_secs_f64();
            last = now;
            if let Some(m) = fusion.sample(&d.read(), dt) {
                crate::shell::on_ui(move || deliver(id, DeviceEvent::Motion(m), send));
            }
            std::thread::sleep(every);
        }
        d.report_every(0);
        log!("motion: watch {id} stopped");
    });
}

/// Sends a watch's event unless it was stopped meanwhile. UI thread.
fn deliver(id: u64, e: DeviceEvent, send: fn(u64, DeviceEvent)) {
    if WATCHES.with(|w| w.borrow().contains_key(&id)) {
        if matches!(e, DeviceEvent::Error(_)) {
            WATCHES.with(|w| w.borrow_mut().remove(&id));
        }
        send(id, e);
    }
}

/// Stops watch `id` (nothing if it is not one). UI thread.
pub fn stop(id: u64) {
    if let Some(flag) = WATCHES.with(|w| w.borrow_mut().remove(&id)) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Stops every watch (quitting). UI thread.
pub fn stop_all() {
    for (_, flag) in WATCHES.with(|w| w.borrow_mut().drain().collect::<Vec<_>>()) {
        flag.store(true, Ordering::Relaxed);
    }
}
