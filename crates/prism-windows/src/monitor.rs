//! The screen as `device_info()` reports it: the monitor showing the
//! window, in points, its diagonal and pixel density from its EDID (the
//! physical size it declares, which Windows keeps in the registry), its
//! refresh rate. No Win32: tested on every host.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::ScreenInfo;

/// The registry key (under `HKEY_LOCAL_MACHINE`) holding the EDID of the
/// monitor whose device interface is `interface`
/// (`\\?\DISPLAY#GSM5B08#5&1a2b3c&0&UID4352#{e6f07b5f-…}`, from
/// `EnumDisplayDevicesW` with `EDD_GET_DEVICE_INTERFACE_NAME`).
pub fn edid_key(interface: &str) -> Option<String> {
    let rest = interface.strip_prefix(r"\\?\")?;
    let mut parts = rest.split('#');
    let (class, model, instance) = (parts.next()?, parts.next()?, parts.next()?);
    if class.is_empty() || model.is_empty() || instance.is_empty() {
        return None;
    }
    Some(format!(
        r"SYSTEM\CurrentControlSet\Enum\{class}\{model}\{instance}\Device Parameters"
    ))
}

/// The physical size an EDID declares, in millimetres: its first detailed
/// timing's (to the millimetre), else its basic size (to the centimetre).
/// `None` for a projector or a virtual display, which declare none.
pub fn edid_size_mm(edid: &[u8]) -> Option<(f32, f32)> {
    const HEADER: [u8; 8] = [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0];
    if edid.len() < 128 || edid[..8] != HEADER {
        return None;
    }
    // A detailed timing descriptor has a pixel clock.
    let d = &edid[54..72];
    if d[0] != 0 || d[1] != 0 {
        let w = u16::from(d[12]) | (u16::from(d[14] >> 4) << 8);
        let h = u16::from(d[13]) | (u16::from(d[14] & 0x0F) << 8);
        if w > 0 && h > 0 {
            return Some((f32::from(w), f32::from(h)));
        }
    }
    let (w, h) = (edid[21], edid[22]);
    (w > 0 && h > 0).then(|| (f32::from(w) * 10., f32::from(h) * 10.))
}

/// The screen: `pixels` (the monitor's, physical) at `scale` pixels per
/// point, its declared size (`mm`) giving the diagonal in inches (to the
/// tenth) and the pixels per inch, its refresh rate (`hz`; 0 and 1 mean
/// the hardware's default, unknown).
pub fn screen_info(
    pixels: (f32, f32),
    scale: f32,
    mm: Option<(f32, f32)>,
    hz: Option<u32>,
) -> ScreenInfo {
    let scale = if scale > 0. { scale } else { 1. };
    let diagonal = mm
        .map(|(w, h)| ((w * w + h * h).sqrt() / 25.4 * 10.).round() / 10.)
        .filter(|d| *d > 1.);
    let (pw, ph) = pixels;
    ScreenInfo {
        width: pw / scale,
        height: ph / scale,
        scale,
        diagonal,
        ppi: diagonal.map(|d| ((pw * pw + ph * ph).sqrt() / d).round()),
        refresh_rate: hz.filter(|hz| *hz > 1).map(|hz| hz as f32),
        cutout: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An EDID header and a 597 × 336 mm detailed timing (a 27" panel),
    /// basic size 60 × 34 cm.
    fn edid(detailed: bool) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        e[21] = 60;
        e[22] = 34;
        if detailed {
            // Pixel clock 148.5 MHz.
            e[54] = 0x02;
            e[55] = 0x3A;
            // 597 = 0x255, 336 = 0x150.
            e[66] = 0x55;
            e[67] = 0x50;
            e[68] = 0x21;
        }
        e
    }

    #[test]
    fn the_registry_key_of_a_monitor() {
        let id = r"\\?\DISPLAY#GSM5B08#5&1a2b3c&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(
            edid_key(id).as_deref(),
            Some(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\GSM5B08\5&1a2b3c&0&UID4352\Device Parameters"
            )
        );
        assert_eq!(edid_key(r"\\.\DISPLAY1"), None);
        assert_eq!(edid_key(""), None);
    }

    #[test]
    fn the_size_an_edid_declares() {
        assert_eq!(edid_size_mm(&edid(true)), Some((597., 336.)));
        assert_eq!(edid_size_mm(&edid(false)), Some((600., 340.)));
        let mut none = edid(false);
        none[21] = 0;
        assert_eq!(edid_size_mm(&none), None);
        assert_eq!(edid_size_mm(&[0; 64]), None);
        let mut bad = edid(true);
        bad[0] = 1;
        assert_eq!(edid_size_mm(&bad), None);
    }

    #[test]
    fn screens_in_points_inches_and_pixels_per_inch() {
        // A 27" 4K monitor at 150 %, 60 Hz.
        let s = screen_info((3840., 2160.), 1.5, Some((597., 336.)), Some(60));
        assert_eq!((s.width, s.height, s.scale), (2560., 1440., 1.5));
        assert_eq!(s.diagonal, Some(27.));
        assert_eq!(s.ppi, Some(163.));
        assert_eq!(s.refresh_rate, Some(60.));
        // A projector: no size; the default refresh rate.
        let s = screen_info((1920., 1080.), 1., None, Some(1));
        assert_eq!((s.diagonal, s.ppi, s.refresh_rate), (None, None, None));
    }
}
