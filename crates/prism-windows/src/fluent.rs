//! The Windows 11 (Fluent) colors Prism's own views and bars use beside
//! the WinUI controls: WinUI 3's theme colors (`*FillColor*`,
//! `*StrokeColor*` resources) for light and dark, the system accent, and
//! the controls' metrics (for measuring them). No Win32 types.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::Color;

const fn rgba(hex: u32, a: f32) -> Color {
    Color {
        r: ((hex >> 16) & 0xff) as f32 / 255.,
        g: ((hex >> 8) & 0xff) as f32 / 255.,
        b: (hex & 0xff) as f32 / 255.,
        a,
        dark: None,
    }
}

const fn rgb(hex: u32) -> Color {
    rgba(hex, 1.)
}

/// The system accent color's shades (`AccentPalette` in the registry):
/// WinUI fills with `dark1` in light mode and `light2` in dark mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub light2: Color,
    pub light1: Color,
    pub base: Color,
    pub dark1: Color,
    pub dark2: Color,
}

impl Default for Accent {
    /// Windows 11's default blue.
    fn default() -> Accent {
        Accent {
            light2: rgb(0x60CDFF),
            light1: rgb(0x0093F9),
            base: rgb(0x0078D4),
            dark1: rgb(0x005FB8),
            dark2: rgb(0x004C87),
        }
    }
}

impl Accent {
    /// From the 32 bytes of `HKCU\…\Explorer\Accent\AccentPalette`: eight
    /// RGBA colors, lightest first (light3, light2, light1, base, dark1,
    /// dark2, dark3, unused).
    pub fn from_palette(bytes: &[u8]) -> Option<Accent> {
        if bytes.len() < 28 {
            return None;
        }
        let c = |i: usize| {
            let p = &bytes[i * 4..i * 4 + 3];
            rgb(((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32)
        };
        Some(Accent {
            light2: c(1),
            light1: c(2),
            base: c(3),
            dark1: c(4),
            dark2: c(5),
        })
    }
}

/// WinUI 3's theme colors, resolved for light or dark.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// `SolidBackgroundFillColorBase`: the window (Mica's fallback).
    pub window: Color,
    /// `LayerFillColorDefault` over the window: headers, tab bars.
    pub layer: Color,
    /// `CardBackgroundFillColorDefault`.
    pub card: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_tertiary: Color,
    pub text_disabled: Color,
    /// `ControlFillColorDefault`, `…Secondary` (hover), `…Tertiary` (pressed).
    pub control: Color,
    pub control_hover: Color,
    pub control_pressed: Color,
    pub control_disabled: Color,
    /// `ControlFillColorInputActive`: a focused text box.
    pub input_active: Color,
    /// `ControlStrokeColorDefault` and `…Secondary` (a control's bottom edge).
    pub stroke: Color,
    pub stroke_secondary: Color,
    /// `ControlStrongStrokeColorDefault`: check boxes, a text box's
    /// underline, the off toggle's outline.
    pub strong_stroke: Color,
    /// `ControlStrongFillColorDefault`: the off toggle's knob, slider rails.
    pub strong_fill: Color,
    /// `ControlAltFillColorSecondary`: the off toggle's inside.
    pub alt_fill: Color,
    /// `SubtleFillColorSecondary` / `…Tertiary`: hovered and pressed
    /// navigation items, bar buttons.
    pub subtle_hover: Color,
    pub subtle_pressed: Color,
    pub divider: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_pressed: Color,
    /// `TextOnAccentFillColorPrimary`.
    pub on_accent: Color,
    /// The two-tone keyboard focus ring.
    pub focus_outer: Color,
    pub focus_inner: Color,
    /// Scroll bar thumbs.
    pub scrollbar: Color,
    /// The selection highlight of text fields.
    pub selection: Color,
}

impl Palette {
    pub fn new(dark: bool, accent: Accent) -> Palette {
        if dark {
            let a = accent.light2;
            Palette {
                dark,
                window: rgb(0x202020),
                layer: rgb(0x272727),
                card: rgba(0xFFFFFF, 0.0512),
                text: rgb(0xFFFFFF),
                text_secondary: rgba(0xFFFFFF, 0.786),
                text_tertiary: rgba(0xFFFFFF, 0.545),
                text_disabled: rgba(0xFFFFFF, 0.363),
                control: rgba(0xFFFFFF, 0.0605),
                control_hover: rgba(0xFFFFFF, 0.0837),
                control_pressed: rgba(0xFFFFFF, 0.0326),
                control_disabled: rgba(0xFFFFFF, 0.0419),
                input_active: rgba(0x1E1E1E, 0.7),
                stroke: rgba(0xFFFFFF, 0.0698),
                stroke_secondary: rgba(0xFFFFFF, 0.0930),
                strong_stroke: rgba(0xFFFFFF, 0.544),
                strong_fill: rgba(0xFFFFFF, 0.544),
                alt_fill: rgba(0x000000, 0.1),
                subtle_hover: rgba(0xFFFFFF, 0.0605),
                subtle_pressed: rgba(0xFFFFFF, 0.0419),
                divider: rgba(0xFFFFFF, 0.0837),
                accent: a,
                accent_hover: Color { a: 0.9, ..a },
                accent_pressed: Color { a: 0.8, ..a },
                on_accent: rgb(0x000000),
                focus_outer: rgb(0xFFFFFF),
                focus_inner: rgba(0x000000, 0.7),
                scrollbar: rgba(0xFFFFFF, 0.545),
                selection: accent.light1,
            }
        } else {
            let a = accent.dark1;
            Palette {
                dark,
                window: rgb(0xF3F3F3),
                layer: rgb(0xF9F9F9),
                card: rgba(0xFFFFFF, 0.7),
                text: rgba(0x000000, 0.894),
                text_secondary: rgba(0x000000, 0.62),
                text_tertiary: rgba(0x000000, 0.447),
                text_disabled: rgba(0x000000, 0.361),
                control: rgba(0xFFFFFF, 0.7),
                control_hover: rgba(0xF9F9F9, 0.5),
                control_pressed: rgba(0xF9F9F9, 0.3),
                control_disabled: rgba(0xF9F9F9, 0.3),
                input_active: rgb(0xFFFFFF),
                stroke: rgba(0x000000, 0.0578),
                stroke_secondary: rgba(0x000000, 0.1622),
                strong_stroke: rgba(0x000000, 0.447),
                strong_fill: rgba(0x000000, 0.447),
                alt_fill: rgba(0x000000, 0.0241),
                subtle_hover: rgba(0x000000, 0.0373),
                subtle_pressed: rgba(0x000000, 0.0241),
                divider: rgba(0x000000, 0.0803),
                accent: a,
                accent_hover: Color { a: 0.9, ..a },
                accent_pressed: Color { a: 0.8, ..a },
                on_accent: rgb(0xFFFFFF),
                focus_outer: rgba(0x000000, 0.894),
                focus_inner: rgb(0xFFFFFF),
                scrollbar: rgba(0x000000, 0.447),
                selection: accent.base,
            }
        }
    }

    /// The variant of an adaptive color for this theme.
    pub fn resolve(&self, c: Color) -> Color {
        if self.dark {
            c.dark_variant()
        } else {
            c.light_variant()
        }
    }
}

/// Control metrics, view units (DIPs), from WinUI 3's styles.
pub mod metrics {
    /// A button's padding: left, top, right, bottom (`ButtonPadding`).
    pub const BUTTON_PADDING: (f32, f32, f32, f32) = (11., 5., 11., 6.);
    pub const CONTROL_HEIGHT: f32 = 32.;
    /// A text box's padding (`TextControlThemePadding`).
    pub const FIELD_PADDING: (f32, f32, f32, f32) = (10., 5., 6., 6.);
    pub const TOGGLE: (f32, f32) = (40., 20.);
    pub const CHECKBOX: f32 = 20.;
    pub const SPINNER: f32 = 32.;
}

/// `a` over `b`, straight alpha.
#[cfg(test)]
fn over(a: Color, b: Color) -> Color {
    let out_a = a.a + b.a * (1. - a.a);
    if out_a <= 0. {
        return Color::TRANSPARENT;
    }
    let mix = |x: f32, y: f32| (x * a.a + y * b.a * (1. - a.a)) / out_a;
    Color {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: out_a,
        dark: None,
    }
}

/// Relative luminance (WCAG) of an opaque color.
#[cfg(test)]
fn luminance(c: Color) -> f32 {
    let l = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * l(c.r) + 0.7152 * l(c.g) + 0.0722 * l(c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_palette_from_registry_bytes() {
        let mut bytes = Vec::new();
        for (i, rgb) in [
            [0x99, 0xEB, 0xFF],
            [0x4C, 0xC2, 0xFF],
            [0x00, 0x91, 0xF8],
            [0x00, 0x78, 0xD4],
            [0x00, 0x67, 0xC0],
            [0x00, 0x3E, 0x92],
            [0x00, 0x1A, 0x68],
            [0xF7, 0x63, 0x0C],
        ]
        .iter()
        .enumerate()
        {
            bytes.extend_from_slice(rgb);
            bytes.push(i as u8);
        }
        let a = Accent::from_palette(&bytes).unwrap();
        assert_eq!(a.base, rgb(0x0078D4));
        assert_eq!(a.dark1, rgb(0x0067C0));
        assert_eq!(a.light2, rgb(0x4CC2FF));
        assert!(Accent::from_palette(&bytes[..10]).is_none());
    }

    #[test]
    fn themes_pick_the_right_accent_and_contrast() {
        let a = Accent::default();
        let light = Palette::new(false, a);
        let dark = Palette::new(true, a);
        assert_eq!(light.accent, a.dark1);
        assert_eq!(dark.accent, a.light2);
        let bg = |p: &Palette| luminance(over(p.window, Color::WHITE));
        assert!(bg(&light) > 0.8 && bg(&dark) < 0.05);
        // Text on accent stays readable.
        assert!(luminance(light.accent) < 0.2 && light.on_accent == Color::WHITE);
        assert!(luminance(dark.accent) > 0.4 && dark.on_accent == rgb(0));
    }

    #[test]
    fn adaptive_colors_resolve_per_theme() {
        let c = Color::adaptive(rgb(0xFFFFFF), rgb(0x111111));
        assert_eq!(
            Palette::new(false, Accent::default()).resolve(c),
            rgb(0xFFFFFF)
        );
        assert_eq!(
            Palette::new(true, Accent::default()).resolve(c),
            rgb(0x111111)
        );
    }

    #[test]
    fn compositing() {
        let half = Color {
            a: 0.5,
            ..Color::BLACK
        };
        let c = over(half, Color::WHITE);
        assert!((c.r - 0.5).abs() < 1e-6 && c.a == 1.);
        assert_eq!(
            over(Color::TRANSPARENT, Color::TRANSPARENT),
            Color::TRANSPARENT
        );
    }
}
