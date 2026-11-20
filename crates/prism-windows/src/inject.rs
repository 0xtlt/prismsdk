//! `PRISM_CLICK` steps that move the real mouse (`SendInput`), so the
//! input goes through XAML's pointer events like the user's would:
//!
//! - `hover:Label` moves the pointer to the middle of the view showing
//!   Label (`hover:Label=x,y`: to that point in it, in DIPs);
//! - `drag:Label=dx,dy` presses there, moves by (`dx`, `dy`) DIPs in small
//!   steps and releases (`drag:Label@x,y=dx,dy` starts at that point);
//! - `click:Label` (`click:Label@x,y`) presses and releases there;
//! - `type:Label=text` clicks there, goes to the end (End) and types
//!   `text` (Unicode keystrokes);
//! - `swipe:Label=dx` flicks horizontally (a fast drag of `dx` DIPs);
//! - `wheel:Label=dy` turns the wheel over it: `dy` points toward the end
//!   (100 a notch), `wheel:Label=dy,h` sideways; `zoom:Label=dy` the same
//!   with Ctrl held (a precision touchpad's pinch comes so);
//! - `ctrlclick:Label` (`@x,y`) clicks with Ctrl held;
//! - `chord:ctrl+c` presses keys together (`ctrl`, `shift`, `alt`, a
//!   letter or a digit, `home`, `end`, `left`, `right`);
//! - touches, from a synthetic touch device (`touch.rs`): `pinch:Label=k`
//!   spreads two fingers around its middle to `k` times their distance
//!   (below 1: pinches in), `tswipe:Label=dx` flicks a finger, and
//!   `tdrag:Label=dx,dy` drags one slowly (a pan).
//!
//! The steps play one after the other, after the other clicks and
//! `PRISM_SCROLL`; each first scrolls its view into sight. The window
//! stays on top meanwhile, so the input reaches it and no other window on
//! the desktop. Give the snapshot time (`PRISM_AFTER`).

use std::cell::RefCell;
use std::collections::VecDeque;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_END,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HWND_NOTOPMOST, HWND_TOPMOST, KillTimer, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOMOVE, SWP_NOSIZE,
    SetForegroundWindow, SetTimer, SetWindowPos,
};

use crate::imp::{main_hwnd, with_state};
use crate::scene::Target;
use crate::win::log;

/// One input, then a pause (ms).
#[derive(Clone, Debug)]
enum Act {
    /// A step to aim, once its view is in sight.
    Aim(String),
    /// Waits for the view of a step to stop moving (where it was, tries
    /// left), then aims it.
    Settle(String, Option<(i32, i32)>, u32),
    Move(i32, i32),
    Down,
    Up,
    Wheel(i32, bool),
    /// A virtual key, down or up (`true`).
    Key(u16, bool),
    /// A UTF-16 unit typed.
    Char(u16),
    /// A frame of touches (screen pixels).
    Touch(Vec<crate::touch::Contact>),
}

thread_local! {
    static QUEUE: RefCell<VecDeque<(Act, u32)>> = const { RefCell::new(VecDeque::new()) };
    static PLAYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Queues `step` if it is one of these; `None` for other steps.
pub fn step(step: &str) -> Option<bool> {
    let (kind, rest) = step.split_once(':')?;
    if !matches!(
        kind,
        "hover"
            | "click"
            | "type"
            | "drag"
            | "swipe"
            | "wheel"
            | "zoom"
            | "ctrlclick"
            | "chord"
            | "pinch"
            | "tswipe"
            | "tdrag"
    ) {
        return None;
    }
    if kind == "chord" {
        let Some(keys) = chord(rest) else {
            return Some(false);
        };
        QUEUE.with(|q| {
            let mut q = q.borrow_mut();
            for vk in &keys {
                q.push_back((Act::Key(*vk, false), 30));
            }
            for vk in keys.iter().rev() {
                q.push_back((Act::Key(*vk, true), 30));
            }
            q.push_back((Act::Key(0, true), 150));
        });
    } else {
        let (target, _) = rest.split_once('=').unwrap_or((rest, ""));
        let label = target.split_once('@').map_or(target, |(l, _)| l);
        if find(label).is_none() {
            return Some(false);
        }
        QUEUE.with(|q| q.borrow_mut().push_back((Act::Aim(step.to_string()), 0)));
    }
    if !PLAYING.replace(true) {
        unsafe {
            let h = main_hwnd();
            let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            let _ = SetForegroundWindow(h);
            SetTimer(None::<HWND>, 0, 150, Some(play));
        }
    }
    Some(true)
}

/// The label a step aims at.
fn label_of(step: &str) -> &str {
    step.split_once(':')
        .map(|(_, r)| r.split(['=', '@']).next().unwrap_or(r))
        .unwrap_or_default()
}

/// The node showing `label`.
fn find(label: &str) -> Option<prism_core::NodeId> {
    with_state(|s| {
        let scene = &s.renderer.widgets.scene;
        match scene.find_by_label(label) {
            Some(Target::Node(id)) => Some(id),
            // A text showing it, or any view labelled so (a 3D scene).
            _ => scene.shown_in_order().into_iter().find(|&id| {
                scene.own_text(id).as_deref() == Some(label)
                    || scene
                        .get(id)
                        .is_some_and(|n| n.a11y.label.as_deref() == Some(label))
            }),
        }
    })
    .flatten()
}

/// The inputs of `step`, its view now in sight.
fn aim(step: &str) -> Vec<(Act, u32)> {
    let Some((kind, rest)) = step.split_once(':') else {
        return Vec::new();
    };
    let (target, args) = rest.split_once('=').unwrap_or((rest, ""));
    let (label, at) = match target.split_once('@') {
        Some((l, at)) => (l, pair(at)),
        None => (target, None),
    };
    let Some(((x, y), scale)) = locate(label, at) else {
        return Vec::new();
    };
    if crate::win::env("PRISM_TRACE_INPUT").is_some() {
        log!("PRISM_CLICK: {kind} on {label:?} from ({x}, {y}) px");
    }
    let nums: Vec<f32> = args
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let px = |v: f32| (v * scale).round() as i32;
    let mut acts = vec![(Act::Move(x, y), 60)];
    match kind {
        "click" => {
            acts.push((Act::Down, 60));
            acts.push((Act::Up, 120));
        }
        "type" => {
            acts.push((Act::Down, 60));
            acts.push((Act::Up, 150));
            acts.push((Act::Key(VK_END.0, false), 20));
            acts.push((Act::Key(VK_END.0, true), 60));
            for c in args.encode_utf16() {
                acts.push((Act::Char(c), 30));
            }
        }
        "hover" => {
            if let [dx, dy] = nums[..] {
                acts = vec![(Act::Move(x + px(dx), y + px(dy)), 60)];
            }
        }
        "ctrlclick" => {
            acts.push((Act::Key(0x11, false), 40));
            acts.push((Act::Down, 60));
            acts.push((Act::Up, 60));
            acts.push((Act::Key(0x11, true), 120));
        }
        "pinch" | "tswipe" | "tdrag" => {
            use crate::touch::glide;
            let n = |i: usize, or: f32| nums.get(i).copied().unwrap_or(or);
            let (frames, pause) = match kind {
                "pinch" => {
                    // Two fingers 80 DIPs apart, to k times that.
                    let (d0, d1) = (px(40.), px(40. * n(0, 2.)));
                    (
                        glide(
                            &[((x - d0, y), (x - d1, y)), ((x + d0, y), (x + d1, y))],
                            16,
                        ),
                        16,
                    )
                }
                // A flick: 80 ms.
                "tswipe" => (glide(&[((x, y), (x + px(n(0, -160.)), y))], 5), 16),
                _ => (
                    glide(&[((x, y), (x + px(n(0, 0.)), y + px(n(1, 0.))))], 24),
                    16,
                ),
            };
            acts = frames.into_iter().map(|f| (Act::Touch(f), pause)).collect();
            acts.push((Act::Touch(Vec::new()), 200));
        }
        "drag" | "swipe" => {
            let (dx, dy) = (
                px(nums.first().copied().unwrap_or(0.)),
                px(nums.get(1).copied().unwrap_or(0.)),
            );
            // A drag moves for 300 ms; a swipe flicks in 60.
            let (steps, pause) = if kind == "drag" { (15, 20) } else { (4, 15) };
            acts.push((Act::Down, if kind == "drag" { 120 } else { 30 }));
            for i in 1..=steps {
                acts.push((Act::Move(x + dx * i / steps, y + dy * i / steps), pause));
            }
            acts.push((Act::Up, 120));
        }
        _ => {
            let horizontal = args.ends_with(",h");
            let ctrl = kind == "zoom";
            if ctrl {
                acts.push((Act::Key(0x11, false), 40));
            }
            let amount = nums.first().copied().unwrap_or(100.);
            // A notch is 120 and scrolls 100 points; positive turns the
            // wheel away (up), or tilts it right.
            let delta = (amount / 100. * 120.).round() as i32;
            acts.push((
                Act::Wheel(if horizontal { delta } else { -delta }, horizontal),
                120,
            ));
            if ctrl {
                acts.push((Act::Key(0x11, true), 120));
            }
        }
    }
    acts
}

/// The virtual keys of `ctrl+shift+c`.
fn chord(keys: &str) -> Option<Vec<u16>> {
    keys.split('+')
        .map(|k| {
            Some(match k.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => 0x11,
                "shift" => 0x10,
                "alt" => 0x12,
                "home" => 0x24,
                "end" => 0x23,
                "left" => 0x25,
                "right" => 0x27,
                "up" => 0x26,
                "down" => 0x28,
                "enter" => 0x0D,
                k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => {
                    k.to_ascii_uppercase().as_bytes()[0] as u16
                }
                _ => return None,
            })
        })
        .collect()
}

fn pair(s: &str) -> Option<(f32, f32)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// The screen point (pixels) of the view showing `label`: its middle, or
/// `at` (DIPs) in it; and the window's scale.
fn locate(label: &str, at: Option<(f32, f32)>) -> Option<((i32, i32), f32)> {
    let id = find(label)?;
    with_state(|s| {
        let w = &s.renderer.widgets;
        let r = w.scene.window_rect(id);
        let (x, y) = at.map_or((r.x + r.w / 2., r.y + r.h / 2.), |(ax, ay)| {
            (r.x + ax, r.y + ay)
        });
        let k = w.scale();
        let mut p = POINT {
            x: (x * k).round() as i32,
            y: (y * k).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(w.hwnd, &mut p);
        }
        Some(((p.x, p.y), k))
    })
    .flatten()
    .or_else(|| {
        log!("PRISM_CLICK: nothing labelled {label:?} to point at");
        None
    })
}

/// Plays the next input, then waits its pause.
unsafe extern "system" fn play(_: HWND, _: u32, timer: usize, _: u32) {
    unsafe {
        let _ = KillTimer(None::<HWND>, timer);
    }
    let Some((act, pause)) = QUEUE.with(|q| q.borrow_mut().pop_front()) else {
        PLAYING.set(false);
        unsafe {
            let _ = SetWindowPos(
                main_hwnd(),
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE,
            );
        }
        return;
    };
    let pause = match act {
        // In sight first (the scroll views around it move), then aimed.
        Act::Aim(step) => {
            if let Some(id) = find(label_of(&step)) {
                with_state(|s| {
                    if let Some(e) = s.renderer.widgets.outer(id) {
                        let _ = e.StartBringIntoView();
                    }
                });
            }
            QUEUE.with(|q| q.borrow_mut().push_front((Act::Settle(step, None, 30), 0)));
            150
        }
        Act::Settle(step, before, tries) => {
            let now = locate(label_of(&step), None).map(|(p, _)| p);
            if now.is_some() && (now == before || tries == 0) {
                let acts = aim(&step);
                QUEUE.with(|q| {
                    let mut q = q.borrow_mut();
                    for a in acts.into_iter().rev() {
                        q.push_front(a);
                    }
                });
                0
            } else {
                let next = Act::Settle(step, now, tries.saturating_sub(1));
                QUEUE.with(|q| q.borrow_mut().push_front((next, 0)));
                100
            }
        }
        act => {
            send(&act);
            pause
        }
    };
    unsafe {
        SetTimer(None::<HWND>, 0, pause.max(1), Some(play));
    }
}

fn send(act: &Act) {
    let (flags, data, at) = match *act {
        Act::Move(x, y) => (MOUSEEVENTF_MOVE, 0, Some((x, y))),
        Act::Down => (MOUSEEVENTF_LEFTDOWN, 0, None),
        Act::Up => (MOUSEEVENTF_LEFTUP, 0, None),
        Act::Wheel(d, false) => (MOUSEEVENTF_WHEEL, d, None),
        Act::Wheel(d, true) => (MOUSEEVENTF_HWHEEL, d, None),
        // A pause (the end of a chord).
        Act::Key(0, _) => return,
        Act::Key(vk, up) => {
            return key(
                vk,
                0,
                if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
            );
        }
        Act::Char(c) => {
            key(0, c, KEYEVENTF_UNICODE);
            return key(0, c, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        }
        Act::Touch(ref contacts) => {
            if !contacts.is_empty() {
                crate::touch::frame(contacts);
            }
            return;
        }
        Act::Aim(_) | Act::Settle(..) => return,
    };
    let (mut dx, mut dy, mut flags) = (0, 0, flags);
    if let Some((x, y)) = at {
        // Absolute coordinates: 0..65535 over the virtual desktop.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
            )
        };
        dx = ((x - vx) * 65535 + vw / 2) / (vw - 1).max(1);
        dy = ((y - vy) * 65535 + vh / 2) / (vh - 1).max(1);
        flags |= MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    }
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: MOUSE_EVENT_FLAGS(flags.0),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent != 1 {
        log!("PRISM_CLICK: the input was not sent ({act:?})");
    }
}

/// A key event: virtual key `vk`, or the UTF-16 unit `scan` with
/// `KEYEVENTF_UNICODE`.
fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    if unsafe { SendInput(&[input], size_of::<INPUT>() as i32) } != 1 {
        log!("PRISM_CLICK: the key was not sent");
    }
}
