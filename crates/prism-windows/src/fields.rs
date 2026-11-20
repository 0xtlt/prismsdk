//! Text fields: RichEdit (`RICHEDIT50W`, msftedit.dll) child windows
//! placed over their node, plain text. They bring IME (TSF), selection,
//! undo and the clipboard. The backend draws the field's Fluent chrome
//! under them and their placeholder (RichEdit has none), reports edits
//! (`EN_CHANGE`), Enter (`Submit`), Tab (focus moves on) and focus changes.
//!
//! Child windows sit above the scene: they are clipped by hand to the
//! scroll views around them and hidden under open overlays. The upgrade
//! path is windowless RichEdit (`ITextServices2::TxDrawD2D`).

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{Color, NodeId};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CombineRgn, CreateFontW, CreateRectRgn,
    DEFAULT_CHARSET, DT_END_ELLIPSIS, DT_NOPREFIX, DT_SINGLELINE, DT_WORDBREAK, DeleteObject,
    DrawTextW, FONT_QUALITY, GetDC, HDC, HFONT, HGDIOBJ, InvalidateRect, OUT_DEFAULT_PRECIS,
    RGN_DIFF, ReleaseDC, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::RichEdit::{
    CFM_COLOR, CHARFORMAT2W, CHARRANGE, EM_EXGETSEL, EM_EXLIMITTEXT, EM_EXSETSEL, EM_SETBKGNDCOLOR,
    EM_SETCHARFORMAT, EM_SETEVENTMASK, EM_SETTEXTMODE, ENM_CHANGE, ENM_SELCHANGE, MSFTEDIT_CLASS,
    SCF_ALL, SCF_DEFAULT, TM_PLAINTEXT,
};
use windows::Win32::UI::Controls::{EM_GETRECT, EM_SETMARGINS, EM_SETPASSWORDCHAR};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_RETURN, VK_SHIFT, VK_TAB};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

use crate::win::{hwnd_value, log, wide};

/// What the subclass procedure needs about a field.
struct Field {
    id: NodeId,
    original: WNDPROC,
    multiline: bool,
    placeholder: Vec<u16>,
    placeholder_color: COLORREF,
    font: HFONT,
    empty: bool,
    /// The text last reported, so one edit is reported once.
    last: String,
}

thread_local! {
    static FIELDS: RefCell<HashMap<isize, Field>> = RefCell::new(HashMap::new());
}

fn colorref(c: Color) -> COLORREF {
    let v = |x: f32| (x.clamp(0., 1.) * 255.).round() as u32;
    COLORREF(v(c.r) | (v(c.g) << 8) | (v(c.b) << 16))
}

/// The node a field window belongs to.
pub fn node_of(h: HWND) -> Option<NodeId> {
    FIELDS.with(|f| f.borrow().get(&hwnd_value(h)).map(|f| f.id))
}

/// Creates the native editor of node `id` (hidden until placed).
pub fn create(parent: HWND, id: NodeId, multiline: bool) -> Option<HWND> {
    unsafe {
        let style = WS_CHILD
            | WS_CLIPSIBLINGS
            | WINDOW_STYLE(if multiline {
                (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32
            } else {
                ES_AUTOHSCROLL as u32
            });
        let instance = GetModuleHandleW(None).ok()?;
        let h = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            MSFTEDIT_CLASS,
            w!(""),
            style,
            0,
            0,
            10,
            10,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                log!("RichEdit field: {e}");
                return None;
            }
        };
        SendMessageW(
            h,
            EM_SETTEXTMODE,
            Some(WPARAM(TM_PLAINTEXT.0 as usize)),
            None,
        );
        SendMessageW(
            h,
            EM_SETEVENTMASK,
            Some(WPARAM(0)),
            Some(LPARAM((ENM_CHANGE | ENM_SELCHANGE) as isize)),
        );
        // Our padding places it: no margins of its own.
        SendMessageW(h, EM_SETMARGINS, Some(WPARAM(3)), Some(LPARAM(0)));
        let original = SetWindowLongPtrW(h, GWLP_WNDPROC, subclass as *const () as isize);
        FIELDS.with(|f| {
            f.borrow_mut().insert(
                hwnd_value(h),
                Field {
                    id,
                    original: std::mem::transmute::<isize, WNDPROC>(original),
                    multiline,
                    placeholder: Vec::new(),
                    placeholder_color: COLORREF(0x808080),
                    font: HFONT::default(),
                    empty: true,
                    last: String::new(),
                },
            )
        });
        Some(h)
    }
}

pub fn destroy(h: HWND) {
    let font = FIELDS.with(|f| f.borrow_mut().remove(&hwnd_value(h)).map(|f| f.font));
    unsafe {
        let _ = DestroyWindow(h);
        if let Some(font) = font
            && !font.is_invalid()
        {
            let _ = DeleteObject(HGDIOBJ(font.0));
        }
    }
}

pub fn text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(h, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got])
            .replace("\r\n", "\n")
            .replace('\r', "\n")
    }
}

/// Replaces the text (the caller guards against reporting it back).
pub fn set_text(h: HWND, text: &str) {
    FIELDS.with(|f| {
        if let Some(f) = f.borrow_mut().get_mut(&hwnd_value(h)) {
            f.last = text.to_string();
        }
    });
    let t = wide(&text.replace('\n', "\r\n"));
    unsafe {
        let _ = SetWindowTextW(h, PCWSTR(t.as_ptr()));
    }
    refresh_empty(h);
}

pub fn set_placeholder(h: HWND, text: &str) {
    FIELDS.with(|f| {
        if let Some(f) = f.borrow_mut().get_mut(&hwnd_value(h)) {
            f.placeholder = text.encode_utf16().collect();
        }
    });
    unsafe {
        let _ = InvalidateRect(Some(h), None, true);
    }
}

/// The selection, in UTF-16 units of the text.
pub fn selection(h: HWND) -> (usize, usize) {
    let mut r = CHARRANGE::default();
    unsafe {
        SendMessageW(
            h,
            EM_EXGETSEL,
            None,
            Some(LPARAM(&mut r as *mut _ as isize)),
        );
    }
    (r.cpMin.max(0) as usize, r.cpMax.max(0) as usize)
}

pub fn set_selection(h: HWND, start: usize, end: usize) {
    let r = CHARRANGE {
        cpMin: start as i32,
        cpMax: end as i32,
    };
    unsafe {
        SendMessageW(h, EM_EXSETSEL, None, Some(LPARAM(&r as *const _ as isize)));
    }
}

/// Font, colors, password mode, length limit.
#[allow(clippy::too_many_arguments)]
pub fn set_look(
    h: HWND,
    family: &str,
    size_px: f32,
    weight: u16,
    italic: bool,
    text: Color,
    background: Color,
    placeholder: Color,
    secure: bool,
    max_length: Option<u32>,
) {
    unsafe {
        let face = wide(family);
        let font = CreateFontW(
            -(size_px.round() as i32).max(1),
            0,
            0,
            0,
            weight as i32,
            italic as u32,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            FONT_QUALITY(CLEARTYPE_QUALITY.0),
            0,
            PCWSTR(face.as_ptr()),
        );
        SendMessageW(
            h,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        let old = FIELDS.with(|f| {
            f.borrow_mut().get_mut(&hwnd_value(h)).map(|f| {
                f.placeholder_color = colorref(placeholder);
                std::mem::replace(&mut f.font, font)
            })
        });
        if let Some(old) = old
            && !old.is_invalid()
        {
            let _ = DeleteObject(HGDIOBJ(old.0));
        }
        let mut cf = CHARFORMAT2W::default();
        cf.Base.cbSize = size_of::<CHARFORMAT2W>() as u32;
        cf.Base.dwMask = CFM_COLOR;
        cf.Base.crTextColor = colorref(text);
        for scope in [SCF_DEFAULT, SCF_ALL] {
            SendMessageW(
                h,
                EM_SETCHARFORMAT,
                Some(WPARAM(scope as usize)),
                Some(LPARAM(&cf as *const _ as isize)),
            );
        }
        SendMessageW(
            h,
            EM_SETBKGNDCOLOR,
            Some(WPARAM(0)),
            Some(LPARAM(colorref(background).0 as isize)),
        );
        SendMessageW(
            h,
            EM_SETPASSWORDCHAR,
            Some(WPARAM(if secure { 0x25CF } else { 0 })),
            None,
        );
        SendMessageW(
            h,
            EM_EXLIMITTEXT,
            None,
            Some(LPARAM(max_length.map_or(0x7FFF_FFFF, |m| m as isize))),
        );
        let _ = InvalidateRect(Some(h), None, true);
    }
}

/// Whether the field itself is shown (`WS_VISIBLE`), whatever its
/// window is (`IsWindowVisible` is false for every child of a window
/// that is not on screen, as in session 0).
pub fn shown(h: HWND) -> bool {
    unsafe { GetWindowLongW(h, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0 }
}

/// Moves the editor to `r` (client pixels of the window), clipped to
/// `visible`; hides it when `r` is `None`.
pub fn place(h: HWND, r: Option<RECT>, visible: Option<RECT>, holes: &[RECT]) {
    unsafe {
        let Some(r) = r else {
            if shown(h) {
                let _ = ShowWindow(h, SW_HIDE);
            }
            return;
        };
        let (w, ht) = ((r.right - r.left).max(1), (r.bottom - r.top).max(1));
        let _ = SetWindowPos(
            h,
            None,
            r.left,
            r.top,
            w,
            ht,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
        );
        // Partly scrolled out, or under floating nodes: only what shows.
        let clipped = visible.filter(|v| {
            v.left > r.left || v.top > r.top || v.right < r.right || v.bottom < r.bottom
        });
        if clipped.is_none() && holes.is_empty() {
            SetWindowRgn(h, None, true);
        } else {
            let v = visible.unwrap_or(r);
            let rgn = CreateRectRgn(
                v.left - r.left,
                v.top - r.top,
                v.right - r.left,
                v.bottom - r.top,
            );
            for hole in holes {
                let cut = CreateRectRgn(
                    hole.left - r.left,
                    hole.top - r.top,
                    hole.right - r.left,
                    hole.bottom - r.top,
                );
                CombineRgn(Some(rgn), Some(rgn), Some(cut), RGN_DIFF);
                let _ = DeleteObject(HGDIOBJ(cut.0));
            }
            SetWindowRgn(h, Some(rgn), true);
        }
        if !shown(h) {
            let _ = ShowWindow(h, SW_SHOWNA);
        }
        let _ = InvalidateRect(Some(h), None, true);
    }
}

fn refresh_empty(h: HWND) {
    let empty = unsafe { GetWindowTextLengthW(h) == 0 };
    let changed = FIELDS.with(|f| {
        f.borrow_mut()
            .get_mut(&hwnd_value(h))
            .is_some_and(|f| std::mem::replace(&mut f.empty, empty) != empty)
    });
    if changed {
        unsafe {
            let _ = InvalidateRect(Some(h), None, true);
        }
    }
}

/// Draws the placeholder of an empty field on `hdc`.
fn draw_placeholder(h: HWND, hdc: HDC) {
    let info = FIELDS.with(|f| {
        f.borrow().get(&hwnd_value(h)).map(|f| {
            (
                f.placeholder.clone(),
                f.placeholder_color,
                f.font,
                f.multiline,
            )
        })
    });
    let Some((text, color, font, multiline)) = info else {
        return;
    };
    if text.is_empty() || unsafe { GetWindowTextLengthW(h) } != 0 {
        return;
    }
    unsafe {
        let mut r = RECT::default();
        SendMessageW(h, EM_GETRECT, None, Some(LPARAM(&mut r as *mut _ as isize)));
        if r.right <= r.left {
            let _ = GetClientRect(h, &mut r);
        }
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, color);
        let mut text = text;
        let flags = if multiline {
            DT_WORDBREAK | DT_NOPREFIX
        } else {
            DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS
        };
        DrawTextW(hdc, &mut text, &mut r, flags);
        SelectObject(hdc, old);
    }
}

fn original(h: HWND) -> WNDPROC {
    FIELDS.with(|f| f.borrow().get(&hwnd_value(h)).and_then(|f| f.original))
}

unsafe extern "system" fn subclass(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let call = |msg, wp, lp| unsafe { CallWindowProcW(original(h), h, msg, wp, lp) };
    let info = FIELDS.with(|f| f.borrow().get(&hwnd_value(h)).map(|f| (f.id, f.multiline)));
    let Some((id, multiline)) = info else {
        return unsafe { DefWindowProcW(h, msg, wp, lp) };
    };
    match msg {
        WM_PAINT => {
            // RichEdit paints, then the placeholder goes over an empty field.
            let r = call(msg, wp, lp);
            unsafe {
                let hdc = GetDC(Some(h));
                draw_placeholder(h, hdc);
                ReleaseDC(Some(h), hdc);
            }
            r
        }
        WM_PRINTCLIENT => {
            let r = call(msg, wp, lp);
            draw_placeholder(h, HDC(wp.0 as *mut _));
            r
        }
        WM_KEYDOWN if wp.0 == VK_TAB.0 as usize => {
            let back = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
            crate::imp::post_focus_step(back);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == VK_RETURN.0 as usize && !multiline => {
            crate::imp::dispatch(prism_core::UiEvent::Submit(id));
            LRESULT(0)
        }
        WM_CHAR if wp.0 == '\t' as usize || (wp.0 == '\r' as usize && !multiline) => LRESULT(0),
        WM_SETFOCUS => {
            let r = call(msg, wp, lp);
            crate::imp::field_focus(id, true);
            r
        }
        WM_KILLFOCUS => {
            let r = call(msg, wp, lp);
            crate::imp::field_focus(id, false);
            r
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCDESTROY => {
            let r = call(msg, wp, lp);
            FIELDS.with(|f| f.borrow_mut().remove(&hwnd_value(h)));
            r
        }
        _ => call(msg, wp, lp),
    }
}

/// `EN_CHANGE` from field `h`: the text it now shows.
pub fn changed(h: HWND) -> Option<(NodeId, String)> {
    refresh_empty(h);
    let now = text(h);
    FIELDS.with(|f| {
        let mut f = f.borrow_mut();
        let field = f.get_mut(&hwnd_value(h))?;
        (field.last != now).then(|| {
            field.last = now.clone();
            (field.id, now)
        })
    })
}

/// Paints a field into `hdc` (snapshots: `WM_PRINT` works without a
/// desktop compositor).
pub fn print(h: HWND, hdc: HDC) {
    unsafe {
        // PRF_CLIENT | PRF_ERASEBKGND | PRF_CHILDREN | PRF_NONCLIENT
        SendMessageW(
            h,
            WM_PRINT,
            Some(WPARAM(hdc.0 as usize)),
            Some(LPARAM(0x4 | 0x8 | 0x10 | 0x2)),
        );
    }
}
