//! What the system says: light or dark, the accent color, animations, the
//! locale, the OS version, the data directory.

use std::ffi::c_void;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Foundation::HWND;
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT,
    COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SystemParametersInfoW,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::contrast::SystemColors;
use crate::fluent::Accent;

fn reg_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(v)
}

/// Whether apps should be dark (`AppsUseLightTheme` = 0; under a
/// contrast theme, the theme's), unless `PRISM_APPEARANCE=light|dark`
/// says otherwise (development, smoke tests: the app's own choice still
/// wins).
pub fn dark_mode() -> bool {
    match std::env::var("PRISM_APPEARANCE").as_deref() {
        Ok("dark") => return true,
        Ok("light") => return false,
        _ => {}
    }
    if let Some(hc) = high_contrast() {
        return hc.dark();
    }
    reg_dword(
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("AppsUseLightTheme"),
    ) == Some(0)
}

/// The user's accent color shades, else Windows' default blue.
pub fn accent() -> Accent {
    let mut buf = [0u8; 32];
    let mut size = buf.len() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent"),
            w!("AccentPalette"),
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    if r == ERROR_SUCCESS {
        Accent::from_palette(&buf[..size as usize]).unwrap_or_default()
    } else {
        Accent::default()
    }
}

/// « Animation effects » off in Settings → Accessibility (`PRISM_REDUCE_MOTION=1`
/// or `0` in its place, for this app only).
pub fn reduce_motion() -> bool {
    if let Some(v) = crate::win::env("PRISM_REDUCE_MOTION") {
        return v == "1";
    }
    let mut on = BOOL(1);
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut BOOL as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && !on.as_bool()
}

thread_local! {
    /// `PRISM_CLICK=contrast:<value>`: `PRISM_HIGH_CONTRAST` changed while
    /// the app runs.
    static CONTRAST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Sets the value `PRISM_HIGH_CONTRAST` stands for from now on (the
/// `contrast:` test step), as if the user changed the contrast theme.
pub fn override_contrast(value: &str) {
    CONTRAST.with(|c| *c.borrow_mut() = Some(value.to_string()));
}

/// The contrast theme's colors while one is on (Settings › Accessibility
/// › Contrast themes), else `None`. `PRISM_HIGH_CONTRAST` stands in for
/// the system's setting, for this app only: `0` (off), `1` (on, with the
/// system colors as they are) or the name of one of Windows 11's themes
/// (`aquatic`, `desert`, `dusk`, `night-sky`: their colors).
pub fn high_contrast() -> Option<SystemColors> {
    let forced = CONTRAST
        .with(|c| c.borrow().clone())
        .or_else(|| crate::win::env("PRISM_HIGH_CONTRAST"));
    match forced.as_deref() {
        Some("0") => None,
        Some("1") => Some(system_colors()),
        Some(name) => SystemColors::theme(name).or_else(|| Some(system_colors())),
        None => high_contrast_on().then(system_colors),
    }
}

/// `SPI_GETHIGHCONTRAST`: a contrast theme is on.
fn high_contrast_on() -> bool {
    /// `HIGHCONTRASTW`.
    #[repr(C)]
    struct HighContrast {
        size: u32,
        flags: u32,
        scheme: *mut u16,
    }
    const HCF_HIGHCONTRASTON: u32 = 1;
    let mut hc = HighContrast {
        size: size_of::<HighContrast>() as u32,
        flags: 0,
        scheme: std::ptr::null_mut(),
    };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.size,
            Some(&mut hc as *mut HighContrast as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && hc.flags & HCF_HIGHCONTRASTON != 0
}

/// The system colors (`GetSysColor`): the contrast theme's while one is on.
fn system_colors() -> SystemColors {
    let c = |i: SYS_COLOR_INDEX| {
        // COLORREF: 0x00BBGGRR.
        let v = unsafe { GetSysColor(i) };
        prism_core::Color {
            r: (v & 0xff) as f32 / 255.,
            g: ((v >> 8) & 0xff) as f32 / 255.,
            b: ((v >> 16) & 0xff) as f32 / 255.,
            a: 1.,
            dark: None,
        }
    };
    SystemColors {
        window: c(COLOR_WINDOW),
        window_text: c(COLOR_WINDOWTEXT),
        hotlight: c(COLOR_HOTLIGHT),
        gray_text: c(COLOR_GRAYTEXT),
        highlight: c(COLOR_HIGHLIGHT),
        highlight_text: c(COLOR_HIGHLIGHTTEXT),
        button_face: c(COLOR_BTNFACE),
        button_text: c(COLOR_BTNTEXT),
    }
}

/// The user's locale (`fr-FR`).
pub fn locale_name() -> String {
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if n <= 1 {
        return "en-US".into();
    }
    String::from_utf16_lossy(&buf[..n as usize - 1])
}

pub fn locale() -> prism_core::Locale {
    let tag = std::env::var("PRISM_LOCALE").unwrap_or_else(|_| locale_name());
    prism_core::Locale::new(&tag)
}

/// The environment the app sees (`cx.environment()`).
pub fn environment() -> prism_core::Environment {
    prism_core::Environment {
        locale: locale(),
        text_scale: text_scale(),
        reduce_motion: reduce_motion(),
        ..Default::default()
    }
}

/// `%LOCALAPPDATA%\<title>` (or the bundle id set by packaging later).
pub fn data_dir(title: &str) -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let id = std::env::var("PRISM_APP_ID").unwrap_or_else(|_| {
        title
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect()
    });
    base.join(id)
}

/// Windows' version (`RtlGetVersion` is exact; `GetVersionEx` lies
/// without a manifest).
pub fn os_version() -> prism_core::Version {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    let mut info = OsVersionInfo {
        size: size_of::<OsVersionInfo>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        csd: [0; 128],
    };
    unsafe { RtlGetVersion(&mut info) };
    prism_core::Version {
        major: info.major,
        minor: info.minor,
        patch: info.build,
    }
}

/// The text size the user chose (Settings → Accessibility → Text size,
/// 100–225 %), or `PRISM_TEXT_SCALE`.
pub fn text_scale() -> f32 {
    if let Some(s) = std::env::var("PRISM_TEXT_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        return s;
    }
    reg_dword(
        w!("Software\\Microsoft\\Accessibility"),
        w!("TextScaleFactor"),
    )
    .filter(|v| (100..=225).contains(v))
    .map_or(1., |v| v as f32 / 100.)
}

/// What the app sees of the device: a desktop, Windows' version, the
/// monitor showing `hwnd` (`monitor.rs`) at the app's `scale`.
pub fn device_info(hwnd: HWND, scale: f32) -> prism_core::DeviceInfo {
    let mut info = prism_core::DeviceInfo::guess(prism_core::Platform::Windows, (0., 0.));
    info.os.version = os_version();
    if let Some(screen) = screen(hwnd, scale) {
        info.orientation = prism_core::Orientation::of(screen.width, screen.height);
        info.screen = screen;
    }
    info
}

thread_local! {
    /// The monitor and scale `device_info` last reported.
    static SCREEN: std::cell::Cell<(isize, u32)> = const { std::cell::Cell::new((0, 0)) };
}

/// `device_info` when the window is on another monitor, at another scale,
/// or the monitor changed (`force`: a resolution or refresh rate change).
pub fn device_info_changed(hwnd: HWND, scale: f32, force: bool) -> Option<prism_core::DeviceInfo> {
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromWindow};
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
    let now = (monitor.0 as isize, scale.to_bits());
    if SCREEN.replace(now) == now && !force {
        return None;
    }
    Some(device_info(hwnd, scale))
}

/// The monitor showing `hwnd`: its pixels, the size its EDID declares,
/// its refresh rate.
fn screen(hwnd: HWND, scale: f32) -> Option<prism_core::ScreenInfo> {
    use windows::Win32::Graphics::Gdi::{
        DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW,
        EnumDisplaySettingsW, GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
        MONITORINFOEXW, MonitorFromWindow,
    };
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY) };
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    unsafe { GetMonitorInfoW(monitor, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) }
        .ok()
        .ok()?;
    let r = mi.monitorInfo.rcMonitor;
    let pixels = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let device = PCWSTR(mi.szDevice.as_ptr());
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let hz = unsafe { EnumDisplaySettingsW(device, ENUM_CURRENT_SETTINGS, &mut mode) }
        .as_bool()
        .then_some(mode.dmDisplayFrequency);
    let mut dd = DISPLAY_DEVICEW {
        cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    let mm = unsafe { EnumDisplayDevicesW(device, 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME) }
        .as_bool()
        .then(|| {
            let end = dd
                .DeviceID
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(dd.DeviceID.len());
            String::from_utf16_lossy(&dd.DeviceID[..end])
        })
        .and_then(|id| crate::monitor::edid_key(&id))
        .and_then(|key| reg_binary(HKEY_LOCAL_MACHINE, &key, "EDID"))
        .and_then(|edid| crate::monitor::edid_size_mm(&edid));
    Some(crate::monitor::screen_info(pixels, scale, mm, hz))
}

/// A binary registry value.
fn reg_binary(root: HKEY, key: &str, value: &str) -> Option<Vec<u8>> {
    let (key, value) = (
        windows::core::HSTRING::from(key),
        windows::core::HSTRING::from(value),
    );
    let mut size = 0u32;
    let r = unsafe {
        RegGetValueW(
            root,
            &key,
            &value,
            RRF_RT_REG_BINARY,
            None,
            None,
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS || size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let r = unsafe {
        RegGetValueW(
            root,
            &key,
            &value,
            RRF_RT_REG_BINARY,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then(|| {
        buf.truncate(size as usize);
        buf
    })
}

thread_local! {
    /// `PRISM_SCALE`, or the last simulated DPI change (`dpi:` step).
    static SCALE: std::cell::Cell<Option<f32>> = std::cell::Cell::new(scale_from_env());
}

/// `PRISM_SCALE=1.5` (or `150%`): the app's scale in place of its
/// monitor's, to check 125, 150 or 200 % on any screen without changing
/// the PC's settings.
fn scale_from_env() -> Option<f32> {
    let v = std::env::var("PRISM_SCALE").ok()?;
    let v = v.trim();
    let k = match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f32>().ok()? / 100.,
        None => v.parse::<f32>().ok()?,
    };
    (0.5..=4.).contains(&k).then_some(k)
}

/// The app's own scale (`PRISM_SCALE`, a simulated DPI change), if any:
/// XAML draws at it (the island's `OverrideScale`) and Prism counts its
/// pixels per DIP with it.
pub fn scale_override() -> Option<f32> {
    SCALE.get()
}

/// The window's DPI: the app's own scale's, else its monitor's.
pub fn window_dpi(hwnd: windows::Win32::Foundation::HWND) -> u32 {
    match scale_override() {
        Some(k) => (k * 96.).round() as u32,
        None => unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96),
    }
}

/// `WM_DPICHANGED`: an app with its own scale takes the new one (a
/// simulated move to another monitor sends the message itself).
pub fn dpi_changed(dpi: u32) {
    if SCALE.get().is_some() {
        SCALE.set(Some(dpi as f32 / 96.));
    }
}

/// Simulates moving the window to a monitor at `dpi`: the app takes that
/// scale, and the window gets `WM_DPICHANGED` with the rectangle Windows
/// would suggest (the same client size in DIPs), as on a real move. The
/// frame keeps its thickness (the monitor stays the same).
pub fn simulate_dpi(hwnd: windows::Win32::Foundation::HWND, dpi: u32) {
    use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, SendMessageW, WM_DPICHANGED,
    };
    let old = window_dpi(hwnd) as f32;
    SCALE.set(Some(old / 96.));
    unsafe {
        let (mut client, mut window) = (RECT::default(), RECT::default());
        let _ = GetClientRect(hwnd, &mut client);
        let _ = GetWindowRect(hwnd, &mut window);
        let k = dpi as f32 / old;
        let frame_w = (window.right - window.left) - client.right;
        let frame_h = (window.bottom - window.top) - client.bottom;
        let suggested = RECT {
            left: window.left,
            top: window.top,
            right: window.left + (client.right as f32 * k).round() as i32 + frame_w,
            bottom: window.top + (client.bottom as f32 * k).round() as i32 + frame_h,
        };
        let packed = (dpi & 0xffff) | ((dpi & 0xffff) << 16);
        SendMessageW(
            hwnd,
            WM_DPICHANGED,
            Some(WPARAM(packed as usize)),
            Some(LPARAM(&suggested as *const RECT as isize)),
        );
    }
}
