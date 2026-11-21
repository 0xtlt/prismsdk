//! WinUI 3 in a plain Win32 process, self-contained (no installed Windows
//! App Runtime, no package identity, no MSBuild):
//!
//! 1. The runtime's WinRT classes are registered by the executable's
//!    manifest (`prism_build::link_app()` embeds it; the runtime DLLs are
//!    next to the executable). A manifest activated at run time
//!    (`CreateActCtx`) does not do: WinRT only reads the process's own.
//! 2. `Microsoft.WindowsAppRuntime.dll` is loaded and initialized, then a
//!    `DispatcherQueue` for the thread, the `Application` (aggregated by
//!    `outer.rs` so the Windows 11 control styles load), and
//!    `WindowsXamlManager`.
//! 3. Each window hosts its XAML in a `DesktopWindowXamlSource`.
//!
//! WinUI renders through DWM: in session 0 (SSH, services) the island
//! cannot be created (`DesktopWindowXamlSource` fail-fasts with
//! `E_ACCESSDENIED`), so the backend builds and lays out its tree there but
//! shows nothing ([`has_desktop`]).

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::DwmIsCompositionEnabled;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::UI::WindowsAndMessaging::MSG;
use windows::core::{BOOL, HRESULT, Interface, PCWSTR, Result, s, w};

use crate::win::log;
use crate::winui::bindings::Microsoft::UI::Content::DesktopChildSiteBridge;
use crate::winui::bindings::Microsoft::UI::Dispatching::DispatcherQueueController;
use crate::winui::bindings::Microsoft::UI::WindowId;
use crate::winui::xaml::Controls::XamlControlsResources;
use crate::winui::xaml::Hosting::{DesktopWindowXamlSource, WindowsXamlManager};
use crate::winui::xaml::Media::MicaBackdrop;
use crate::winui::xaml::{Application, UIElement};

/// The XAML runtime of the UI thread: kept until the app quits.
pub struct Runtime {
    pub app: Application,
    _manager: WindowsXamlManager,
    _queue: DispatcherQueueController,
}

/// Starts WinUI on this thread (an STA: `OleInitialize` ran).
pub fn start() -> std::result::Result<Runtime, String> {
    ensure_runtime_loaded()?;
    let queue = DispatcherQueueController::CreateOnCurrentThread()
        .map_err(|e| format!("DispatcherQueueController: {e}"))?;
    let app = crate::outer::create_app().map_err(|e| {
        format!(
            "the WinUI runtime is not registered ({e}): the app's build.rs must call \
             prism_build::link_app(), which embeds the manifest that registers it"
        )
    })?;
    // Run-time fonts in the app's data folder (before XAML's first lookup).
    if let Err(e) = crate::fonts::install_resolver(&app) {
        log!("run-time fonts: {e}");
    }
    let manager = WindowsXamlManager::InitializeForCurrentThread()
        .map_err(|e| format!("WindowsXamlManager: {e}"))?;
    // The Windows 11 styles of every control.
    let resources = XamlControlsResources::new()
        .and_then(|r| {
            app.Resources()?.MergedDictionaries()?.Append(&r)?;
            Ok(r)
        })
        .map_err(|e| format!("XamlControlsResources: {e}"))?;
    drop(resources);
    Ok(Runtime {
        app,
        _manager: manager,
        _queue: queue,
    })
}

/// Self-contained apps load the runtime DLL themselves (what
/// `UndockedRegFreeWinRT-AutoInitializer.cpp` does for C++ apps).
fn ensure_runtime_loaded() -> std::result::Result<(), String> {
    unsafe {
        let dll = LoadLibraryW(w!("Microsoft.WindowsAppRuntime.dll")).map_err(|e| {
            format!(
                "Microsoft.WindowsAppRuntime.dll: {e}. The Windows App SDK runtime must be \
                 next to the executable (cargo builds put it there)"
            )
        })?;
        if let Some(f) = GetProcAddress(dll, s!("WindowsAppRuntime_EnsureIsLoaded")) {
            let f: unsafe extern "system" fn() -> HRESULT = std::mem::transmute(f);
            f().ok()
                .map_err(|e| format!("WindowsAppRuntime_EnsureIsLoaded: {e}"))?;
        }
    }
    Ok(())
}

/// Whether windows can render here: a desktop session with DWM, not
/// session 0 (SSH, services).
pub fn has_desktop() -> bool {
    let mut session = 0u32;
    unsafe {
        let _ = ProcessIdToSessionId(std::process::id(), &mut session);
    }
    let composed = unsafe { DwmIsCompositionEnabled() }.is_ok_and(|b| b.as_bool());
    session != 0 && composed
}

/// A window's XAML content.
pub struct Island {
    pub source: DesktopWindowXamlSource,
    pub bridge: DesktopChildSiteBridge,
}

impl Island {
    /// Hosts `root` in `hwnd`'s client area, on Mica.
    pub fn new(hwnd: HWND, root: &impl Interface, _app: &Application) -> Result<Island> {
        let source = DesktopWindowXamlSource::new()?;
        source.Initialize(window_id(hwnd)?)?;
        let root: UIElement = root.cast()?;
        source.SetContent(&root)?;
        if std::env::var_os("PRISM_NO_MICA").is_none() {
            match MicaBackdrop::new() {
                Ok(m) => {
                    let _ = source.SetSystemBackdrop(&m);
                }
                Err(e) => log!("Mica: {e}"),
            }
        }
        let bridge = source.SiteBridge()?;
        if let Some(k) = crate::system::scale_override() {
            let _ = bridge.SetOverrideScale(k);
        }
        Ok(Island { source, bridge })
    }

    /// XAML's scale in place of the monitor's (`PRISM_SCALE`).
    pub fn set_scale(&self, k: f32) {
        let _ = self.bridge.SetOverrideScale(k);
    }

    /// The client area changed size (pixels).
    pub fn resize(&self, width: i32, height: i32) {
        let _ = self.bridge.MoveAndResize(windows::Graphics::RectInt32 {
            X: 0,
            Y: 0,
            Width: width,
            Height: height,
        });
    }

    pub fn show(&self) {
        let _ = self.bridge.Show();
    }

    /// The window was activated: keyboard input goes to the XAML content.
    pub fn focus(&self) {
        use crate::winui::bindings::Microsoft::UI::Xaml::Hosting::XamlSourceFocusNavigationReason;
        let _ = (|| -> Result<()> {
            let request =
                crate::winui::xaml::Hosting::XamlSourceFocusNavigationRequest::CreateInstance(
                    XamlSourceFocusNavigationReason::Programmatic,
                )?;
            self.source.NavigateFocus(&request)?;
            Ok(())
        })();
    }
}

impl Drop for Island {
    fn drop(&mut self) {
        let _ = self.source.Close();
    }
}

pub(crate) fn window_id(hwnd: HWND) -> Result<WindowId> {
    unsafe {
        let dll = LoadLibraryW(w!("Microsoft.Internal.FrameworkUdk.dll"))?;
        let f = GetProcAddress(dll, s!("Windowing_GetWindowIdFromWindow"))
            .ok_or_else(windows_core::Error::from_thread)?;
        let f: unsafe extern "system" fn(HWND, *mut WindowId) -> HRESULT = std::mem::transmute(f);
        let mut id = WindowId::default();
        f(hwnd, &mut id).ok()?;
        Ok(id)
    }
}

type PreTranslate = unsafe extern "system" fn(*const MSG) -> BOOL;

/// `ContentPreTranslateMessage`: keyboard accelerators, access keys and
/// Tab between islands, before `TranslateMessage`.
pub fn pre_translate() -> Option<PreTranslate> {
    for dll in [
        w!("Microsoft.UI.Windowing.Core.dll"),
        w!("Microsoft.UI.Input.dll"),
        w!("CoreMessagingXP.dll"),
        w!("Microsoft.UI.dll"),
    ] {
        unsafe {
            let m = GetModuleHandleW(dll).or_else(|_| LoadLibraryW(dll));
            if let Ok(m) = m
                && let Some(f) = GetProcAddress(m, s!("ContentPreTranslateMessage"))
            {
                return Some(std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    PreTranslate,
                >(f));
            }
        }
    }
    None
}

/// Logs WinRT fail-fasts (`STATUS_STOWED_EXCEPTION`, which end the process
/// without a word): the stowed errors and their text.
pub fn log_fail_fasts() {
    use windows::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_exception(info: *mut EXCEPTION_POINTERS) -> i32 {
        unsafe {
            let rec = &*(*info).ExceptionRecord;
            if rec.ExceptionCode.0 as u32 != 0xC000_027B {
                return 0;
            }
            let list = rec.ExceptionInformation[0] as *const *const u8;
            let count = rec.ExceptionInformation[1];
            for i in 0..count {
                let e = *list.add(i);
                let code = HRESULT(*(e.add(8) as *const i32));
                let form = *(e.add(12) as *const u32) & 3;
                let detail = if form == 2 {
                    let text = *(e.add(16) as *const *const u16);
                    if text.is_null() {
                        String::new()
                    } else {
                        PCWSTR(text).to_string().unwrap_or_default()
                    }
                } else {
                    String::new()
                };
                log!("WinRT fail-fast {i}: {code:?} {} {detail}", code.message());
            }
            0
        }
    }
    unsafe {
        AddVectoredExceptionHandler(1, Some(on_exception));
    }
}
