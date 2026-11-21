//! Touches injected for the smoke hooks (`inject.rs`: `pinch:`, `tswipe:`,
//! `tdrag:`): a synthetic touch device of the process
//! (`CreateSyntheticPointerDevice`, no administrator rights, no touch
//! screen needed), whose frames reach XAML as real touch pointers, as a
//! finger's would. Each frame holds every contact down.

use std::cell::Cell;

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, HSYNTHETICPOINTERDEVICE, POINTER_FEEDBACK_DEFAULT,
    POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_FLAGS, POINTER_INFO, POINTER_TOUCH_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PT_TOUCH, TOUCH_MASK_CONTACTAREA, TOUCH_MASK_ORIENTATION, TOUCH_MASK_PRESSURE,
};

use crate::win::log;

/// One finger in a frame: its id (0, 1…), screen point (pixels) and
/// phase.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

thread_local! {
    static DEVICE: Cell<Option<HSYNTHETICPOINTERDEVICE>> = const { Cell::new(None) };
}

fn device() -> Option<HSYNTHETICPOINTERDEVICE> {
    if let Some(d) = DEVICE.get() {
        return Some(d);
    }
    match unsafe { CreateSyntheticPointerDevice(PT_TOUCH, 2, POINTER_FEEDBACK_DEFAULT) } {
        Ok(d) => {
            DEVICE.set(Some(d));
            Some(d)
        }
        Err(e) => {
            log!("PRISM_CLICK: no synthetic touch device ({e})");
            None
        }
    }
}

/// Sends one frame of `contacts`.
pub fn frame(contacts: &[Contact]) {
    let Some(device) = device() else { return };
    let infos: Vec<POINTER_TYPE_INFO> = contacts
        .iter()
        .map(|c| {
            let flags: POINTER_FLAGS = match c.phase {
                Phase::Down => POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                Phase::Move => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                Phase::Up => POINTER_FLAG_UP,
            };
            let at = POINT { x: c.x, y: c.y };
            POINTER_TYPE_INFO {
                r#type: PT_TOUCH,
                Anonymous: POINTER_TYPE_INFO_0 {
                    touchInfo: POINTER_TOUCH_INFO {
                        pointerInfo: POINTER_INFO {
                            pointerType: PT_TOUCH,
                            pointerId: c.id,
                            pointerFlags: flags,
                            ptPixelLocation: at,
                            ..Default::default()
                        },
                        touchMask: TOUCH_MASK_CONTACTAREA
                            | TOUCH_MASK_ORIENTATION
                            | TOUCH_MASK_PRESSURE,
                        rcContact: RECT {
                            left: c.x - 4,
                            top: c.y - 4,
                            right: c.x + 4,
                            bottom: c.y + 4,
                        },
                        orientation: 90,
                        pressure: 32000,
                        ..Default::default()
                    },
                },
            }
        })
        .collect();
    if let Err(e) = unsafe { InjectSyntheticPointerInput(device, &infos) } {
        log!("PRISM_CLICK: a touch frame was not sent ({e})");
    }
}

/// A finger's start and end points (screen pixels).
pub type Path = ((i32, i32), (i32, i32));

/// The frames of `fingers` moving in `steps` from their start to their
/// end points (screen pixels): down, moves, up.
pub fn glide(fingers: &[Path], steps: i32) -> Vec<Vec<Contact>> {
    let at = |i: usize, k: i32, phase| {
        let ((x0, y0), (x1, y1)) = fingers[i];
        Contact {
            id: i as u32,
            x: x0 + (x1 - x0) * k / steps.max(1),
            y: y0 + (y1 - y0) * k / steps.max(1),
            phase,
        }
    };
    let mut frames = vec![(0..fingers.len()).map(|i| at(i, 0, Phase::Down)).collect()];
    for k in 1..=steps {
        frames.push((0..fingers.len()).map(|i| at(i, k, Phase::Move)).collect());
    }
    frames.push(
        (0..fingers.len())
            .map(|i| at(i, steps, Phase::Up))
            .collect(),
    );
    frames
}
