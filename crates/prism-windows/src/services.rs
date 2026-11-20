//! Windows' system services (`PlatformServices`): the clipboard (Unicode
//! text), opening links with the default app, and an answer for every
//! device call. Notifications, alerts, the tray, menus bars, badges,
//! pickers and secure storage come in the next phase: their calls answer
//! "unavailable" so apps never wait.

use std::collections::HashSet;

use prism_core::surfaces::PlatformRequest;
use prism_core::{
    Capabilities, DeviceCall, DeviceError, DeviceValue, PermissionState, PlatformServices,
};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::win::{log, wide};

const CF_UNICODETEXT: u32 = 13;

pub struct WinServices {
    pub hwnd: HWND,
    /// Requests logged once as not done yet.
    told: HashSet<&'static str>,
}

impl WinServices {
    pub fn new(hwnd: HWND) -> WinServices {
        WinServices {
            hwnd,
            told: HashSet::new(),
        }
    }

    fn later(&mut self, what: &'static str) {
        if self.told.insert(what) {
            log!("{what}: not available on Windows yet");
        }
    }

    fn device(&mut self, id: u64, call: DeviceCall) {
        let result = match call {
            DeviceCall::CopyText(text) => write_text(self.hwnd, &text).map(|_| DeviceValue::Done),
            DeviceCall::WriteClipboard(item) => match item.as_text() {
                Some(t) => write_text(self.hwnd, &t).map(|_| DeviceValue::Done),
                None => Err(DeviceError::Unavailable),
            },
            DeviceCall::ReadClipboard => Ok(DeviceValue::Text(read_text(self.hwnd))),
            DeviceCall::ReadClipboardItem => Ok(DeviceValue::Clipboard(
                read_text(self.hwnd)
                    .map(|t| prism_core::ClipboardItem::new().text(t))
                    .unwrap_or_default(),
            )),
            DeviceCall::OpenUrl(url) | DeviceCall::OpenUrlIn(url, _) => open(&url),
            DeviceCall::PermissionStatus(_) | DeviceCall::RequestPermission(_) => {
                Ok(DeviceValue::Permission(PermissionState::Unavailable))
            }
            DeviceCall::StopWatch => Ok(DeviceValue::Done),
            other => {
                let name = format!("{other:?}");
                let what = name.split(['(', ' ', '{']).next().unwrap_or("device call");
                log!("{what}: not available on Windows yet");
                Err(DeviceError::Unavailable)
            }
        };
        crate::imp::defer(Box::new(move |s| {
            let patches = s.runtime.device_reply(id, result);
            crate::imp::commit(s, patches);
        }));
    }
}

impl PlatformServices for WinServices {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            clipboard: true,
            ..Default::default()
        }
    }

    fn handle(&mut self, request: PlatformRequest) {
        match request {
            PlatformRequest::Device { id, call } => self.device(id, call),
            PlatformRequest::Notify(_) | PlatformRequest::CancelNotification(_) => {
                self.later("notifications")
            }
            PlatformRequest::RequestNotificationPermission => {}
            PlatformRequest::ShowAlert(_) => self.later("alerts"),
            PlatformRequest::SetTray(_) | PlatformRequest::RemoveTray(_) => {
                self.later("tray icons")
            }
            PlatformRequest::SetAppMenu(_) | PlatformRequest::SetMenuBar(_) => {
                self.later("menu bars")
            }
            PlatformRequest::SetBadge(_) => self.later("badges"),
            PlatformRequest::RegisterPush | PlatformRequest::UnregisterPush => self.later("push"),
            PlatformRequest::ScheduleBackgroundTask(_) => self.later("background tasks"),
            // Handled by the window (`imp::commit`), or nothing to do on a
            // desktop.
            _ => {}
        }
    }
}

fn open(url: &str) -> Result<DeviceValue, DeviceError> {
    let target = wide(url);
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // > 32: success.
    if r.0 as isize > 32 {
        Ok(DeviceValue::Done)
    } else {
        Err(DeviceError::Failed(format!(
            "ShellExecute: {}",
            r.0 as isize
        )))
    }
}

fn write_text(owner: HWND, text: &str) -> Result<(), DeviceError> {
    let data: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        OpenClipboard(Some(owner)).map_err(|e| DeviceError::Failed(e.to_string()))?;
        let result = (|| {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2)?;
            let p = GlobalLock(mem) as *mut u16;
            if p.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0)))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result.map_err(|e| DeviceError::Failed(e.to_string()))
    }
}

fn read_text(owner: HWND) -> Option<String> {
    unsafe {
        IsClipboardFormatAvailable(CF_UNICODETEXT).ok()?;
        OpenClipboard(Some(owner)).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let mem = HGLOBAL(h.0);
            let p = GlobalLock(mem) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(mem);
            Some(s)
        });
        let _ = CloseClipboard();
        text
    }
}
