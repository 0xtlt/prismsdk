//! Key presses for Prism's key listeners (`on_key`, `on_key_global`,
//! `consume_keys`), seen in the message loop before XAML handles them:
//! keys typed into a text box stay the text box's; keys a listener
//! consumes never reach XAML.

use std::cell::RefCell;

use prism_core::{KeyEvent, Modifiers, UiEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardState, MAPVK_VK_TO_VSC, MapVirtualKeyW, ToUnicode, VK_CONTROL, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::controls::Content;
use crate::imp::{State, dispatch_traced};

thread_local! {
    /// The keys whose press was reported, until released (#159).
    static HELD: RefCell<crate::keys::Held> = RefCell::new(crate::keys::Held::default());
}

/// The window turned inactive: Windows sends the keys still down their
/// release in the window that is active then, so they are released now,
/// to the node their press went to (#159).
pub fn release_all() {
    let held = HELD.with(|h| h.borrow_mut().take_all());
    for (name, target) in held {
        dispatch_traced(UiEvent::KeyUp(target, KeyEvent::new(name)));
    }
}

pub fn modifiers() -> Modifiers {
    let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
    Modifiers {
        shift: down(VK_SHIFT.0),
        control: down(VK_CONTROL.0),
        alt: down(VK_MENU.0),
        meta: down(VK_LWIN.0) || down(VK_RWIN.0),
    }
}

/// The character a key types with the current layout, without touching
/// dead-key state.
pub fn typed_char(vk: u32) -> Option<char> {
    let mut state = [0u8; 256];
    unsafe {
        GetKeyboardState(&mut state).ok()?;
        let scan = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
        let mut buf = [0u16; 8];
        // Flag 4: leave the keyboard's dead-key state alone.
        let n = ToUnicode(vk, scan, Some(&state), &mut buf, 4);
        if n <= 0 {
            return None;
        }
        char::decode_utf16(buf[..n as usize].iter().copied())
            .next()
            .and_then(Result::ok)
    }
}

/// Key `code` (its virtual key `vk` and scan code) went down (or up):
/// reported to the listeners that want it. A release goes, under the name
/// it went down with, to the node its press went to, only if that press
/// was reported. Returns whether XAML must not see it (a listener
/// consumed or claimed it).
pub fn key_event(
    s: &mut State,
    code: u32,
    vk: u16,
    typed: Option<char>,
    repeat: bool,
    up: bool,
) -> bool {
    if up {
        if let Some((name, target)) = HELD.with(|h| h.borrow_mut().release(code)) {
            let key = KeyEvent {
                modifiers: modifiers(),
                ..KeyEvent::new(name)
            };
            dispatch_traced(UiEvent::KeyUp(target, key));
        }
        return false;
    }
    // Escape closes the topmost sheet or dialog (`overlay.rs`), once a
    // field's suggestions list is closed (the field closes it: `suggest.rs`).
    if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0
        && !repeat
        && !crate::suggest::any_open()
        && crate::overlay::escape(&s.renderer.widgets)
    {
        return true;
    }
    // F5 and Ctrl+R refresh the refreshable scroll views (`refresh.rs`).
    if !repeat && crate::refresh::key(&s.renderer.widgets.scene, vk) {
        return true;
    }
    let Some(name) = crate::keys::key_name(vk, typed) else {
        return false;
    };
    // Keyboard input: focus moved now shows the focus visual.
    s.renderer.widgets.focus_visible = true;
    let modifiers = modifiers();
    let w = &s.renderer.widgets;
    let focus = w.focus;
    let scene = &w.scene;
    let any_global = scene.nodes.values().any(|n| n.keys_global);
    let target = focus.and_then(|f| scene.find_up(f, |n| n.keys_local));
    let content = focus.and_then(|f| w.views.get(&f)).map(|v| &v.content);
    let field = matches!(content, Some(Content::Field(_) | Content::Rich(_)));
    let editing = field
        || focus
            .and_then(|f| scene.get(f))
            .is_some_and(|n| n.is_text_field());
    let typing =
        editing && crate::keys::types_text(&name, modifiers.control, modifiers.alt, modifiers.meta);
    if target.is_none() && (typing || !any_global) {
        return false;
    }
    // The field's selection and caret edges (#211), for `consume_key_at`.
    let (selection, edges) = content
        .and_then(crate::carets::caret_state)
        .map_or((None, None), |(s, e)| (Some(s), Some(e)));
    let key = KeyEvent {
        key: name.clone(),
        modifiers,
        repeat,
        selection,
        edges,
    };
    HELD.with(|h| h.borrow_mut().press(code, &name, target));
    let claimed = target.is_some_and(|t| Some(t) == focus && crate::keys::claims(&name));
    let consumed = s.runtime.consumes_key_event(focus, &key);
    dispatch_traced(UiEvent::Key(target, key));
    claimed || consumed
}
