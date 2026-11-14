//! `cx.device_info()` on the web: the visitor's device and browser.
//!
//! 1. The server parses the `User-Agent` (and the `prism_dev` cookie the page
//!    wrote on an earlier visit), so the first HTML is already right.
//! 2. The page then reports what only it knows: touch, installed as an app,
//!    screen and orientation, foldable posture and viewport segments, client
//!    hints (`navigator.userAgentData`). It reports again on rotation, fold
//!    and display-mode changes.

use prism_core::device_info::{estimate_diagonal, parse_user_agent};
use prism_core::{
    DeviceInfo, DeviceKind, Fold, Orientation, OsName, Posture, Rotation, Version, WindowMode,
};
use serde_json::Value;

/// `prism_dev=<touch>_<standalone>_<screen w>_<screen h>_<pixel ratio>`,
/// written by the page.
struct Cookie {
    touch: bool,
    standalone: bool,
    screen: (f32, f32),
    scale: f32,
}

fn cookie(header: &str) -> Option<Cookie> {
    let v = header
        .split(';')
        .find_map(|kv| kv.trim().strip_prefix("prism_dev="))?;
    let mut parts = v.split('_');
    let mut next = || parts.next()?.parse::<f32>().ok();
    Some(Cookie {
        touch: next()? > 0.,
        standalone: next()? > 0.,
        screen: (next()?, next()?),
        scale: next()?,
    })
}

/// The server's view of the visitor's device, before the page reports.
pub(crate) fn from_request(user_agent: &str, cookie_header: Option<&str>) -> DeviceInfo {
    let c = cookie_header.and_then(cookie);
    let mut info = parse_user_agent(user_agent, c.as_ref().map(|c| c.touch));
    if let Some(c) = c {
        set_screen(&mut info, c.screen.0, c.screen.1, c.scale);
        set_standalone(&mut info, c.standalone);
    }
    info
}

fn set_screen(info: &mut DeviceInfo, w: f32, h: f32, scale: f32) {
    if w <= 0. || h <= 0. {
        return;
    }
    info.screen.width = w;
    info.screen.height = h;
    info.screen.scale = scale.max(1.);
    info.screen.diagonal = estimate_diagonal(info.kind, w, h);
    info.orientation = Orientation::of(w, h);
}

fn set_standalone(info: &mut DeviceInfo, standalone: bool) {
    if let Some(b) = info.browser.as_mut() {
        b.standalone = standalone;
    }
    info.window_mode = if standalone && matches!(info.kind, DeviceKind::Phone | DeviceKind::Tablet)
    {
        WindowMode::Fullscreen
    } else {
        WindowMode::Windowed
    };
}

/// Applies the page's `{"t":"device", …}` report to `info`.
pub(crate) fn apply_report(info: &mut DeviceInfo, v: &Value) {
    let num = |k: &str| v.get(k).and_then(Value::as_f64).map(|n| n as f32);
    let flag = |k: &str| v.get(k).and_then(Value::as_bool);

    // Touch decides whether a "Mac" is an iPad: parse the agent again.
    if let (Some(touch), Some(ua)) = (flag("touch"), v.get("ua").and_then(Value::as_str)) {
        let standalone = info.browser.as_ref().is_some_and(|b| b.standalone);
        *info = parse_user_agent(ua, Some(touch));
        set_standalone(info, standalone);
    }
    // Client hints (Chromium): exact OS version, model, mobile.
    if let Some(hints) = v.get("hints") {
        let s = |k: &str| hints.get(k).and_then(Value::as_str).unwrap_or("");
        if let Some(name) = Some(OsName::from_name(s("platform"))).filter(|n| *n != OsName::Unknown)
            && name == info.os.name
        {
            let version = Version::parse(s("platformVersion"));
            if version.is_known() {
                info.os.version = if name == OsName::Windows {
                    // platformVersion 13+ is Windows 11.
                    Version::new(if version.major >= 13 { 11 } else { 10 }, 0, 0)
                } else {
                    version
                };
            }
        }
        if !s("model").is_empty() {
            info.model = Some(s("model").to_string());
        }
        if let Some(b) = info.browser.as_mut() {
            let full = hints.get("fullVersionList").and_then(Value::as_array);
            if let Some(v) = full.into_iter().flatten().find_map(|e| {
                (e.get("brand").and_then(Value::as_str) == Some(brand_of(&b.name)))
                    .then(|| e.get("version").and_then(Value::as_str))
                    .flatten()
            }) {
                b.version = Version::parse(v);
            }
        }
    }
    if let Some(standalone) = flag("standalone") {
        set_standalone(info, standalone);
    }
    if let (Some(w), Some(h)) = (num("sw"), num("sh")) {
        let (mut w, mut h) = (w, h);
        // iOS keeps `screen.width` in portrait whatever the rotation.
        let landscape = v
            .get("orientation")
            .and_then(Value::as_str)
            .map(|o| o.starts_with("landscape"));
        if landscape == Some(w < h) {
            std::mem::swap(&mut w, &mut h);
        }
        set_screen(info, w, h, num("dpr").unwrap_or(1.));
    }
    if let Some(angle) = num("angle") {
        // `screen.orientation.angle` turns the content counter-clockwise.
        info.rotation = Rotation(((360 - (angle as i32).rem_euclid(360)) % 360) as u16);
    }

    // Foldables: viewport segments (two panes) and the posture.
    let segments: Vec<[f32; 4]> = v
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let a = s.as_array()?;
            let n = |i: usize| a.get(i).and_then(Value::as_f64).map(|v| v as f32);
            Some([n(0)?, n(1)?, n(2)?, n(3)?])
        })
        .collect();
    info.fold = match segments.as_slice() {
        [a, b] => {
            let side_by_side = b[0] >= a[0] + a[2] - 1.;
            Some(if side_by_side {
                let x = a[0] + a[2];
                Fold {
                    x,
                    y: 0.,
                    width: (b[0] - x).max(0.),
                    height: a[3],
                    vertical: true,
                    occluding: b[0] - x > 0.,
                }
            } else {
                let y = a[1] + a[3];
                Fold {
                    x: 0.,
                    y,
                    width: a[2],
                    height: (b[1] - y).max(0.),
                    vertical: false,
                    occluding: b[1] - y > 0.,
                }
            })
        }
        _ => None,
    };
    info.posture = match v.get("posture").and_then(Value::as_str) {
        Some("folded") => match info.fold {
            Some(f) if !f.vertical => Posture::Tabletop,
            Some(_) => Posture::Book,
            // Folded without spanning: the hinge runs across the long side.
            None if info.orientation == Orientation::Landscape => Posture::Tabletop,
            None => Posture::Book,
        },
        _ => Posture::Flat,
    };
}

/// The client-hints brand for a browser name.
fn brand_of(name: &str) -> &str {
    match name {
        "Chrome" => "Google Chrome",
        "Edge" => "Microsoft Edge",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const IPAD_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15";

    #[test]
    fn the_page_report_refines_the_user_agent() {
        // The first request looks like a Mac.
        let mut info = from_request(IPAD_UA, None);
        assert_eq!(info.kind, DeviceKind::Desktop);
        // The page says it is a touch screen, installed on the home screen,
        // in landscape.
        apply_report(
            &mut info,
            &json!({"t": "device", "ua": IPAD_UA, "touch": true, "standalone": true,
                    "sw": 820, "sh": 1180, "dpr": 2, "orientation": "landscape-primary", "angle": 90}),
        );
        assert_eq!(
            (info.kind, info.os.name),
            (DeviceKind::Tablet, OsName::Ipados)
        );
        assert_eq!(info.orientation, Orientation::Landscape);
        assert_eq!((info.screen.width, info.screen.height), (1180., 820.));
        assert_eq!(info.rotation, Rotation(270));
        assert_eq!(info.window_mode, WindowMode::Fullscreen);
        assert!(info.browser.as_ref().unwrap().standalone);
        assert!(
            info.screen
                .diagonal
                .is_some_and(|d| (10. ..=12.).contains(&d))
        );
        // Next visit: the cookie gets it right from the first HTML.
        let again = from_request(IPAD_UA, Some("prism_sid=x; prism_dev=1_1_820_1180_2"));
        assert_eq!(again.kind, DeviceKind::Tablet);
    }

    #[test]
    fn foldables_report_segments_and_posture() {
        let ua = "Mozilla/5.0 (Linux; Android 14; SM-F956B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";
        let mut info = from_request(ua, None);
        // Half open like a book: two panes side by side.
        apply_report(
            &mut info,
            &json!({"posture": "folded", "segments": [[0, 0, 420, 880], [440, 0, 420, 880]],
                    "hints": {"platform": "Android", "platformVersion": "14.0.0", "model": "SM-F956B",
                              "fullVersionList": [{"brand": "Google Chrome", "version": "131.0.6778.81"}]}}),
        );
        assert_eq!(info.posture, Posture::Book);
        let fold = info.fold.unwrap();
        assert!(fold.vertical && fold.occluding);
        assert_eq!((fold.x, fold.width), (420., 20.));
        assert_eq!(
            info.browser.as_ref().unwrap().version,
            Version::new(131, 0, 6778)
        );
        // On a table: panes on top of each other.
        apply_report(
            &mut info,
            &json!({"posture": "folded", "segments": [[0, 0, 880, 400], [0, 400, 880, 420]]}),
        );
        assert_eq!(info.posture, Posture::Tabletop);
        assert!(!info.fold.unwrap().occluding);
        // Fully open again.
        apply_report(&mut info, &json!({"posture": "continuous"}));
        assert_eq!((info.posture, info.fold), (Posture::Flat, None));
    }
}
