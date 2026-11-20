//! What the system says: light or dark, the accent color, animations, the
//! locale, the OS version, the data directory.

use std::ffi::c_void;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD, RegGetValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{BOOL, PCWSTR, w};

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

/// Whether apps should be dark (`AppsUseLightTheme` = 0), unless
/// `PRISM_APPEARANCE=light|dark` says otherwise (development, smoke tests:
/// the app's own choice still wins).
pub fn dark_mode() -> bool {
    match std::env::var("PRISM_APPEARANCE").as_deref() {
        Ok("dark") => return true,
        Ok("light") => return false,
        _ => {}
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

/// « Animation effects » off in Settings → Accessibility.
pub fn reduce_motion() -> bool {
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
/// primary monitor.
pub fn device_info(width: f32, height: f32, scale: f32) -> prism_core::DeviceInfo {
    let mut info = prism_core::DeviceInfo::guess(prism_core::Platform::Windows, (0., 0.));
    info.os.version = os_version();
    info.orientation = prism_core::Orientation::of(width, height);
    info.screen = prism_core::ScreenInfo {
        width,
        height,
        scale,
        ..Default::default()
    };
    info
}
