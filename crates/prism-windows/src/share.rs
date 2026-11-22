//! The share sheet (`share`): Windows' Share dialog for the app's window,
//! through `IDataTransferManagerInterop` (`GetForWindow`,
//! `ShowShareUIForWindow`), the desktop apps' way to `DataTransferManager`;
//! no package identity needed. Text, a link (`SetWebLink`, and after the
//! text, on its own line, for targets that only read text, as on Android)
//! and files (read-only `StorageFile`s; one that cannot be read is left
//! out, and the rest still goes).
//! As on Apple platforms, nothing answers: the dialog's outcome is the
//! user's.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;

use prism_core::Share;
use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Storage::{IStorageItem, StorageFile};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
use windows::core::{AgileReference, HSTRING, Interface, factory};
use windows_collections::IIterable;

use crate::win::log;

/// What the next share shows.
#[derive(Clone)]
struct Content {
    title: String,
    text: Option<String>,
    url: Option<String>,
    /// Agile: Windows asks for the data from its own thread.
    files: Vec<AgileReference<StorageFile>>,
}

/// Set before the dialog opens, read when Windows asks for the data.
static CONTENT: Mutex<Option<Content>> = Mutex::new(None);

thread_local! {
    /// Windows whose `DataRequested` is handled.
    static HANDLED: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
}

/// Whether this Windows has the Share dialog for desktop apps.
pub fn supported() -> bool {
    DataTransferManager::IsSupported().unwrap_or(true)
}

fn window_title(owner: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(owner, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Opens the Share dialog for `s` over `owner`; `app` titles it when the
/// window has no title. UI thread.
pub fn share(owner: HWND, app: &str, s: Share) {
    let title = Some(window_title(owner))
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| app.to_string());
    let owner = crate::win::hwnd_value(owner);
    // Files are opened off the UI thread.
    std::thread::spawn(move || {
        let files = s
            .files
            .iter()
            .filter_map(|p| {
                StorageFile::GetFileFromPathAsync(&HSTRING::from(p.as_str()))
                    .and_then(|op| op.join())
                    .and_then(|f| AgileReference::new(&f))
                    .map_err(|e| log!("share: {}: {}", crate::device::file_name(p), e.message()))
                    .ok()
            })
            .collect();
        *CONTENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Content {
            title,
            text: s.text,
            url: s.url,
            files,
        });
        crate::shell::on_ui(move || {
            if let Err(e) = show(crate::win::hwnd(owner)) {
                log!("share: {}", e.message());
            }
        });
    });
}

fn show(owner: HWND) -> windows::core::Result<()> {
    let interop = factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    let key = crate::win::hwnd_value(owner);
    if !HANDLED.with(|h| h.borrow().contains(&key)) {
        let manager: DataTransferManager = unsafe { interop.GetForWindow(owner)? };
        manager.DataRequested(&TypedEventHandler::<
            DataTransferManager,
            DataRequestedEventArgs,
        >::new(|_, args| {
            if let Some(args) = args.as_ref() {
                fill(args)?;
            }
            Ok(())
        }))?;
        HANDLED.with(|h| h.borrow_mut().insert(key));
    }
    if crate::perf::on() {
        log!("share: the Share dialog asked for over the window");
    }
    unsafe { interop.ShowShareUIForWindow(owner) }
}

/// What `PRISM_DEVICE_REPORT` says of sharing: whether Windows has the
/// Share dialog, and the window's `DataTransferManager` with the app's
/// handler (the dialog itself is not opened).
pub fn report(owner: HWND) -> serde_json::Value {
    let manager = factory::<DataTransferManager, IDataTransferManagerInterop>()
        .and_then(|i| unsafe { i.GetForWindow::<DataTransferManager>(owner) });
    serde_json::json!({
        "supported": DataTransferManager::IsSupported().map_or_else(|e| serde_json::json!(e.message()), |s| s.into()),
        "window_manager": manager.map_or_else(|e| format!("error 0x{:08X}: {}", e.code().0 as u32, e.message()), |_| "ok".into()),
    })
}

fn fill(args: &DataRequestedEventArgs) -> windows::core::Result<()> {
    let request = args.Request()?;
    let content = CONTENT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(c) = content else {
        return request.FailWithDisplayText(&HSTRING::from("Nothing to share"));
    };
    let data = request.Data()?;
    data.Properties()?
        .SetTitle(&HSTRING::from(c.title.as_str()))?;
    if let Some(text) = crate::device::share_text(c.text.as_deref(), c.url.as_deref()) {
        data.SetText(&HSTRING::from(text))?;
    }
    if let Some(url) = &c.url {
        match Uri::CreateUri(&HSTRING::from(url.as_str())) {
            Ok(uri) => data.SetWebLink(&uri)?,
            Err(e) => log!("share: {url}: {}", e.message()),
        }
    }
    // A file that cannot be had here is left out: a `None` in the list
    // would fail `SetStorageItems`, and the share with it.
    let items: Vec<Option<IStorageItem>> = c
        .files
        .iter()
        .filter_map(|f| {
            f.resolve()
                .and_then(|f| f.cast::<IStorageItem>())
                .map_err(|e| log!("share: a file is left out: {}", e.message()))
                .ok()
        })
        .map(Some)
        .collect();
    if !items.is_empty() {
        data.SetStorageItems(&IIterable::<IStorageItem>::from(items), true)?;
    }
    Ok(())
}
