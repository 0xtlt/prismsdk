//! Key presses for Prism's key listeners (`on_key`, `on_key_global`,
//! `consume_keys`), seen in the message loop before XAML handles them:
//! keys typed into a text box stay the text box's; keys a listener
//! consumes never reach XAML.

use prism_core::{KeyEvent, Modifiers, UiEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardState, MAPVK_VK_TO_VSC, MapVirtualKeyW, ToUnicode, VK_CONTROL, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::controls::Content;
use crate::imp::{State, dispatch};

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

/// A key went down (or up): reported to the listeners that want it.
/// Returns whether XAML must not see it (a listener consumed or claimed
/// it).
pub fn key_event(s: &mut State, vk: u16, typed: Option<char>, repeat: bool, up: bool) -> bool {
    // Escape closes the topmost sheet or dialog (`overlay.rs`), once a
    // field's suggestions list is closed (the field closes it: `suggest.rs`).
    if vk == windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0
        && !up
        && !repeat
        && !crate::suggest::any_open()
        && crate::overlay::escape(&s.renderer.widgets)
    {
        return true;
    }
    // F5 and Ctrl+R refresh the refreshable scroll views (`refresh.rs`).
    if !up && !repeat && crate::refresh::key(&s.renderer.widgets.scene, vk) {
        return true;
    }
    let Some(name) = crate::keys::key_name(vk, typed) else {
        return false;
    };
    let modifiers = modifiers();
    let w = &s.renderer.widgets;
    let focus = w.focus;
    let scene = &w.scene;
    let any_global = scene.nodes.values().any(|n| n.keys_global);
    let target = focus.and_then(|f| scene.find_up(f, |n| n.keys_local));
    let field = focus
        .and_then(|f| w.views.get(&f))
        .and_then(|v| match &v.content {
            Content::Field(t) => Some(t.clone()),
            _ => None,
        });
    let editing = field.is_some()
        || focus
            .and_then(|f| scene.get(f))
            .is_some_and(|n| n.is_text_field());
    let typing =
        editing && crate::keys::types_text(&name, modifiers.control, modifiers.alt, modifiers.meta);
    if target.is_none() && (typing || !any_global) {
        return false;
    }
    let selection = field.and_then(|t| {
        let text = t.Text().ok()?.to_string();
        let a = t.SelectionStart().ok()?.max(0) as usize;
        let b = a + t.SelectionLength().ok()?.max(0) as usize;
        Some(prism_core::SelectionEvent::new(
            prism_native::fields::utf16_to_chars(&text, a),
            prism_native::fields::utf16_to_chars(&text, b),
        ))
    });
    let key = KeyEvent {
        key: name.clone(),
        modifiers,
        repeat,
        selection,
        edges: None,
    };
    if up {
        dispatch(UiEvent::KeyUp(target, key));
        return false;
    }
    let claimed = target.is_some_and(|t| Some(t) == focus && crate::keys::claims(&name));
    let consumed = s.runtime.consumes_key_event(focus, &key);
    dispatch(UiEvent::Key(target, key));
    claimed || consumed
}
