//! Haptic feedback (`haptic`): the PC's vibration device
//! (`Windows.Devices.Haptics.VibrationDevice`, its `SimpleHapticsController`),
//! the patterns as its waveforms (`device::pattern`). Most PCs have none,
//! and then `capabilities().haptics` is false and `haptic` does nothing.
//!
//! Not used: a pen's haptics (`PenDevice`, only while that pen touches the
//! screen, and Prism's `haptic` names no pointer) and haptic touchpads
//! (Windows plays their feedback itself; apps have no API for it).

use std::sync::OnceLock;

use prism_core::Haptic;
use windows::Devices::Haptics::{
    KnownSimpleHapticsControllerWaveforms, SimpleHapticsController,
    SimpleHapticsControllerFeedback, VibrationAccessStatus, VibrationDevice,
};
use windows::Foundation::TimeSpan;
use windows::core::AgileReference;

use crate::device::{self, Wave};
use crate::win::log;

/// The default vibration device (agile: found at startup off the UI thread).
fn find() -> Option<AgileReference<VibrationDevice>> {
    match VibrationDevice::RequestAccessAsync().and_then(|op| op.join()) {
        Ok(VibrationAccessStatus::Allowed) => {}
        Ok(status) => {
            log!("haptics: vibration access {}", access_name(status));
            return None;
        }
        Err(e) => {
            log!("haptics: {}", e.message());
            return None;
        }
    }
    VibrationDevice::GetDefaultAsync()
        .and_then(|op| op.join())
        .and_then(|d| AgileReference::new(&d))
        .ok()
}

fn access_name(s: VibrationAccessStatus) -> &'static str {
    match s {
        VibrationAccessStatus::Allowed => "Allowed",
        VibrationAccessStatus::DeniedByUser => "DeniedByUser",
        VibrationAccessStatus::DeniedBySystem => "DeniedBySystem",
        VibrationAccessStatus::DeniedByEnergySaver => "DeniedByEnergySaver",
        _ => "?",
    }
}

/// What `PRISM_DEVICE_REPORT` says of haptics: vibration access, the
/// vibration devices and their waveforms.
pub fn report() -> serde_json::Value {
    let access = VibrationDevice::RequestAccessAsync().and_then(|op| op.join());
    let devices = VibrationDevice::FindAllAsync().and_then(|op| op.join());
    let waveforms: Vec<u16> = vibration()
        .and_then(|d| d.resolve().ok())
        .and_then(|d| d.SimpleHapticsController().ok())
        .and_then(|c| c.SupportedFeedback().ok())
        .map(|f| f.into_iter().filter_map(|f| f.Waveform().ok()).collect())
        .unwrap_or_default();
    serde_json::json!({
        "access": access.map_or_else(|e| e.message(), |a| access_name(a).into()),
        "devices": devices.map_or_else(|e| serde_json::json!(e.message()), |d| d.Size().unwrap_or(0).into()),
        "default_device": vibration().is_some(),
        "waveforms": waveforms,
    })
}

static DEVICE: OnceLock<Option<AgileReference<VibrationDevice>>> = OnceLock::new();

fn vibration() -> Option<&'static AgileReference<VibrationDevice>> {
    DEVICE.get_or_init(find).as_ref()
}

/// Whether the PC has a vibration device the app may use. Blocking:
/// called off the UI thread at startup.
pub fn present() -> bool {
    vibration().is_some()
}

fn waveform(w: Wave) -> windows::core::Result<u16> {
    match w {
        Wave::Click => KnownSimpleHapticsControllerWaveforms::Click(),
        Wave::Press => KnownSimpleHapticsControllerWaveforms::Press(),
        Wave::Success => KnownSimpleHapticsControllerWaveforms::Success(),
        Wave::Error => KnownSimpleHapticsControllerWaveforms::Error(),
    }
}

fn send(
    c: &SimpleHapticsController,
    f: &SimpleHapticsControllerFeedback,
    p: &device::Pattern,
    wave: Wave,
) -> windows::core::Result<()> {
    let count = device::count(p, wave);
    if count > 1 && c.IsPlayCountSupported()? {
        c.SendHapticFeedbackForPlayCount(
            f,
            p.intensity,
            count,
            TimeSpan {
                Duration: p.pause_ms * 10_000,
            },
        )
    } else if c.IsIntensitySupported()? {
        c.SendHapticFeedbackWithIntensity(f, p.intensity)
    } else {
        c.SendHapticFeedback(f)
    }
}

fn play_on(d: &VibrationDevice, h: Haptic) -> windows::core::Result<()> {
    let c = d.SimpleHapticsController()?;
    let supported: Vec<SimpleHapticsControllerFeedback> =
        c.SupportedFeedback()?.into_iter().collect();
    let p = device::pattern(h);
    for &wave in p.waves {
        let id = waveform(wave)?;
        if let Some(f) = supported.iter().find(|f| f.Waveform().ok() == Some(id)) {
            return send(&c, f, &p, wave);
        }
    }
    Ok(())
}

/// Plays `h` (nothing without a vibration device).
pub fn play(h: Haptic) {
    if let Some(d) = vibration()
        && let Err(e) = d.resolve().and_then(|d| play_on(&d, h))
    {
        log!("haptics: {}", e.message());
    }
}
