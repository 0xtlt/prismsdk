//! The Components gallery's "About this device" card: `cx.device_info()`.
//! It re-renders by itself on rotation, fold or split view.

use prism::WindowMode;
use prism::prelude::*;
use prism::ui;

use crate::strings;

fn kind(cx: &App, d: &DeviceInfo) -> String {
    match d.kind {
        DeviceKind::Phone => strings::about_kind_phone(cx),
        DeviceKind::Tablet => strings::about_kind_tablet(cx),
        DeviceKind::Desktop => strings::about_kind_desktop(cx),
        DeviceKind::Tv => strings::about_kind_tv(cx),
        DeviceKind::Watch => strings::about_kind_watch(cx),
        DeviceKind::Unknown => strings::about_kind_unknown(cx),
    }
}

/// `iPhone 16 Pro`, `Mac15,6`, `Tablet`.
fn model(cx: &App, d: &DeviceInfo) -> String {
    let name = d
        .model_name
        .clone()
        .or_else(|| d.model.clone())
        .unwrap_or_else(|| kind(cx, d));
    if d.simulator {
        strings::about_simulator(cx, name)
    } else {
        name
    }
}

/// `6.3″ · 402 × 874 pt · @3x · 120 Hz`.
fn screen(cx: &App, d: &DeviceInfo) -> String {
    let s = &d.screen;
    let mut parts = Vec::new();
    if let Some(inches) = s.diagonal {
        parts.push(format!("{}″", cx.format_number(inches as f64, 1)));
    }
    if s.width > 0. {
        parts.push(format!("{:.0} × {:.0} pt", s.width, s.height));
    }
    if s.scale > 1. {
        parts.push(format!("@{}x", cx.format_number(s.scale as f64, 0)));
    }
    if let Some(hz) = s.refresh_rate {
        parts.push(format!("{hz:.0} Hz"));
    }
    if parts.is_empty() {
        // The web page has not reported yet.
        return strings::about_unknown(cx);
    }
    parts.join(" · ")
}

fn orientation(cx: &App, d: &DeviceInfo) -> String {
    let name = match d.orientation {
        Orientation::Portrait => strings::about_portrait(cx),
        Orientation::Landscape => strings::about_landscape(cx),
    };
    match d.rotation.0 {
        0 => name,
        degrees => strings::about_rotated(cx, name, degrees),
    }
}

fn window(cx: &App, d: &DeviceInfo) -> String {
    match d.window_mode {
        WindowMode::Fullscreen => strings::about_fullscreen(cx),
        WindowMode::Split => strings::about_split(cx),
        WindowMode::SlideOver => strings::about_slide_over(cx),
        WindowMode::Windowed => strings::about_windowed(cx),
    }
}

fn posture(cx: &App, d: &DeviceInfo) -> String {
    let posture = match d.posture {
        Posture::Flat => strings::about_flat(cx),
        Posture::Book => strings::about_book(cx),
        Posture::Tabletop => strings::about_tabletop(cx),
        Posture::Closed => strings::about_closed(cx),
    };
    let posture = match d.hinge_angle {
        Some(angle) => format!("{posture} · {angle:.0}°"),
        None => posture,
    };
    match d.fold {
        Some(f) => {
            let at = if f.vertical { f.x } else { f.y };
            format!(
                "{posture} · {}",
                strings::about_hinge(cx, format!("{at:.0}"))
            )
        }
        None => posture,
    }
}

fn browser(cx: &App, d: &DeviceInfo) -> Option<String> {
    let b = d.browser.as_ref()?;
    let name = if b.name.is_empty() {
        strings::about_unknown(cx)
    } else if b.version.is_known() {
        format!("{} {} ({})", b.name, b.version.major, b.engine.name())
    } else {
        b.name.clone()
    };
    Some(if b.standalone {
        strings::about_installed(cx, name)
    } else {
        name
    })
}

pub fn card(cx: &mut App) -> impl IntoElement {
    let d = cx.device_info();
    let row = |title: String, detail: String| ui::list_row(title).detail(detail);
    ui::card()
        .child(ui::heading(strings::about_title(cx)))
        .child(row(strings::about_model(cx), model(cx, &d)))
        .child(row(strings::about_system(cx), d.os.to_string()))
        .when_some(browser(cx, &d), |c, b| {
            c.child(row(strings::about_browser(cx), b))
        })
        .child(row(strings::about_screen(cx), screen(cx, &d)))
        .child(row(strings::about_orientation(cx), orientation(cx, &d)))
        .child(row(strings::about_window(cx), window(cx, &d)))
        // Foldables only (or whenever the platform reports a posture).
        .when(d.is_folded(), |c| {
            c.child(row(strings::about_foldable(cx), posture(cx, &d)))
        })
}
