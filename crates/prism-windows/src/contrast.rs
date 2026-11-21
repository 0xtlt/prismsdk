//! High contrast (Settings › Accessibility › Contrast themes): the colors
//! Prism draws give way to the theme's system colors, as WinUI's controls
//! do and as Edge forces a page's colors.
//!
//! - **Fills** become the window's color (a clickable view's: the button
//!   face), keeping their opacity. A strong fill (one that stands out from
//!   the theme's background the app was made for) stays visible: a colored
//!   one (an accent, a badge) and the fill of a selected item become the
//!   highlight, a gray or black one (a switch's knob) the text's color, as
//!   in WinUI's controls. A disabled view's never stands out.
//! - **Text** is the window text; on a highlight, the highlight text; on
//!   a text-colored fill, the window's color; in a link, the link color;
//!   in a clickable view, the button text; when disabled, the gray text.
//! - **Borders** take the text's color (gray when disabled), **shadows**
//!   go. Pictures, videos, web pages and 3D scenes keep theirs.
//!
//! No Win32 types: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::Color;

const fn rgb(hex: u32) -> Color {
    Color {
        r: ((hex >> 16) & 0xff) as f32 / 255.,
        g: ((hex >> 8) & 0xff) as f32 / 255.,
        b: (hex & 0xff) as f32 / 255.,
        a: 1.,
        dark: None,
    }
}

/// A contrast theme's colors (`GetSysColor`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemColors {
    /// `COLOR_WINDOW`: the background.
    pub window: Color,
    /// `COLOR_WINDOWTEXT`: text.
    pub window_text: Color,
    /// `COLOR_HOTLIGHT`: links.
    pub hotlight: Color,
    /// `COLOR_GRAYTEXT`: disabled text.
    pub gray_text: Color,
    /// `COLOR_HIGHLIGHT`: what is selected.
    pub highlight: Color,
    /// `COLOR_HIGHLIGHTTEXT`: text on it.
    pub highlight_text: Color,
    /// `COLOR_BTNFACE`: buttons.
    pub button_face: Color,
    /// `COLOR_BTNTEXT`: their text.
    pub button_text: Color,
}

impl SystemColors {
    /// Windows 11's contrast themes as they come (Settings › Accessibility
    /// › Contrast themes), by name: `aquatic`, `desert`, `dusk`,
    /// `night-sky`.
    pub fn theme(name: &str) -> Option<SystemColors> {
        let t = |window, text, link, gray, hl, hl_text, face, face_text| SystemColors {
            window: rgb(window),
            window_text: rgb(text),
            hotlight: rgb(link),
            gray_text: rgb(gray),
            highlight: rgb(hl),
            highlight_text: rgb(hl_text),
            button_face: rgb(face),
            button_text: rgb(face_text),
        };
        Some(
            match name.to_ascii_lowercase().replace([' ', '_'], "-").as_str() {
                "aquatic" => t(
                    0x202020, 0xFFFFFF, 0x75E9FC, 0xA6A6A6, 0x8EE3F0, 0x263B50, 0x202020, 0xFFFFFF,
                ),
                "desert" => t(
                    0xFFFAEF, 0x3D3D3D, 0x1C5E75, 0x676767, 0x903909, 0xFFF5E3, 0xFFFAEF, 0x3D3D3D,
                ),
                "dusk" => t(
                    0x2D3236, 0xFFFFFF, 0x70EBDE, 0xA6A6A6, 0xA1BFDE, 0x212D3B, 0x2D3236, 0xB6F6F0,
                ),
                "night-sky" | "nightsky" => t(
                    0x000000, 0xFFFFFF, 0x8080FF, 0xA6A6A6, 0xD6B4FD, 0x2B2B2B, 0x000000, 0xFFEE32,
                ),
                _ => return None,
            },
        )
    }

    /// A dark theme (light text on a dark background).
    pub fn dark(&self) -> bool {
        luminance(self.window) < luminance(self.window_text)
    }
}

/// What a fill becomes under a contrast theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fill {
    /// The window's color (the button face for a button): a card, a chip.
    #[default]
    Window,
    /// The highlight: an accent, a colored badge, a selected item.
    Highlight,
    /// The text's color: a strong gray or black (a knob, a dark badge).
    Ink,
}

/// What fill `c` becomes: strong ones stay visible, colored ones as the
/// highlight, gray ones as the text's color.
pub fn classify(c: Color, reference: Color) -> Fill {
    if !strong(c, reference) {
        return Fill::Window;
    }
    let chroma = c.r.max(c.g).max(c.b) - c.r.min(c.g).min(c.b);
    if chroma < 0.15 {
        Fill::Ink
    } else {
        Fill::Highlight
    }
}

/// Where a node stands, for the colors it gets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scheme {
    /// Clickable: its fill is the button face.
    pub button: bool,
    /// In a clickable view (or one): button text.
    pub in_button: bool,
    /// In a link (or one).
    pub link: bool,
    /// Disabled (or in a disabled view).
    pub disabled: bool,
    /// Selected (or the current page): its fill is the highlight.
    pub selected: bool,
    /// What its text is drawn on: its own fill, or the nearest one behind
    /// it.
    pub behind: Fill,
}

/// Colors forced to a contrast theme's, for one node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Forced {
    pub sys: SystemColors,
    pub scheme: Scheme,
    /// The background the app's colors were made for (Fluent's window
    /// color, light or dark): a fill that stands out from it is strong.
    pub reference: Color,
}

/// Below this opacity a color counts as transparent.
const CLEAR: f32 = 0.02;

impl Forced {
    /// A fill (background, gradient stop).
    pub fn fill(&self, c: Color) -> Color {
        if c.a < CLEAR {
            return c;
        }
        let s = self.scheme;
        let base = if s.button {
            self.sys.button_face
        } else {
            self.sys.window
        };
        if s.disabled {
            return base.with_alpha(c.a);
        }
        if s.selected {
            return self.sys.highlight;
        }
        match classify(c, self.reference) {
            Fill::Window => base.with_alpha(c.a),
            Fill::Highlight => self.sys.highlight,
            Fill::Ink => self.text(),
        }
    }

    /// The text color where nothing stands out: in a button, its text.
    fn text(&self) -> Color {
        if self.scheme.in_button {
            self.sys.button_text
        } else {
            self.sys.window_text
        }
    }

    /// Text, glyphs, icons.
    pub fn ink(&self, c: Color) -> Color {
        if c.a < CLEAR {
            return c;
        }
        let s = self.scheme;
        if s.disabled {
            return self.sys.gray_text;
        }
        match s.behind {
            Fill::Highlight => self.sys.highlight_text,
            Fill::Ink if s.in_button => self.sys.button_face,
            Fill::Ink => self.sys.window,
            Fill::Window if s.link => self.sys.hotlight,
            Fill::Window => self.text(),
        }
    }

    /// Borders and outlines.
    pub fn edge(&self, c: Color) -> Color {
        if c.a < CLEAR {
            return c;
        }
        let s = self.scheme;
        if s.disabled {
            self.sys.gray_text
        } else if s.in_button {
            self.sys.button_text
        } else {
            self.sys.window_text
        }
    }
}

/// Whether fill `c` stands out from `reference` (contrast 2:1 or more):
/// accents, badges, knobs, but not cards, chips and hovers.
pub fn strong(c: Color, reference: Color) -> bool {
    contrast(over(c, reference), reference) >= 2.
}

/// `a` over an opaque `b`.
fn over(a: Color, b: Color) -> Color {
    let mix = |x: f32, y: f32| x * a.a + y * (1. - a.a);
    Color {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: 1.,
        dark: None,
    }
}

/// Relative luminance (WCAG).
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

/// WCAG contrast ratio.
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHT: Color = rgb(0xF3F3F3);
    const DARK: Color = rgb(0x202020);

    fn forced(scheme: Scheme) -> Forced {
        Forced {
            sys: SystemColors::theme("night-sky").unwrap(),
            scheme,
            reference: LIGHT,
        }
    }

    #[test]
    fn the_four_contrast_themes() {
        for name in ["aquatic", "desert", "dusk", "night-sky", "Night sky"] {
            let t = SystemColors::theme(name).unwrap();
            // Text, links and selections are readable on their backgrounds.
            assert!(contrast(t.window_text, t.window) >= 7., "{name}");
            assert!(contrast(t.hotlight, t.window) >= 4.5, "{name}");
            assert!(contrast(t.highlight_text, t.highlight) >= 4.5, "{name}");
            assert!(contrast(t.button_text, t.button_face) >= 4.5, "{name}");
        }
        assert!(SystemColors::theme("night-sky").unwrap().dark());
        assert!(!SystemColors::theme("desert").unwrap().dark());
        assert!(SystemColors::theme("high").is_none());
    }

    #[test]
    fn strong_fills_stand_out_from_the_theme() {
        // Accents and badges are strong, cards and hovers are not.
        assert!(strong(rgb(0x005FB8), LIGHT));
        assert!(strong(rgb(0x0F7B0F), LIGHT));
        assert!(strong(rgb(0x60CDFF), DARK));
        assert!(!strong(rgb(0xFFFFFF), LIGHT));
        assert!(!strong(rgb(0xE5E5E5), LIGHT));
        assert!(!strong(Color::BLACK.with_alpha(0.06), LIGHT));
        assert!(!strong(rgb(0x2B2B2B), DARK));
    }

    #[test]
    fn fills_give_way_to_the_window_button_face_or_highlight() {
        let t = SystemColors::theme("night-sky").unwrap();
        let plain = forced(Scheme::default());
        assert_eq!(plain.fill(rgb(0xFFFFFF)), t.window);
        assert_eq!(plain.fill(rgb(0x005FB8)), t.highlight);
        // Opacity stays, transparent stays transparent.
        assert_eq!(plain.fill(rgb(0xFFFFFF).with_alpha(0.5)).a, 0.5);
        assert_eq!(plain.fill(Color::TRANSPARENT), Color::TRANSPARENT);
        let button = forced(Scheme {
            button: true,
            ..Default::default()
        });
        assert_eq!(button.fill(rgb(0xFBFBFB)), t.button_face);
        let selected = forced(Scheme {
            selected: true,
            ..Default::default()
        });
        assert_eq!(selected.fill(rgb(0xF0F0F0)), t.highlight);
        // A gray knob takes the text's color; a disabled accent stands out
        // no more.
        assert_eq!(plain.fill(rgb(0x8A8A8A)), t.window_text);
        let in_button = forced(Scheme {
            in_button: true,
            ..Default::default()
        });
        assert_eq!(in_button.fill(rgb(0x5C5C5C)), t.button_text);
        let disabled = forced(Scheme {
            disabled: true,
            ..Default::default()
        });
        assert_eq!(disabled.fill(rgb(0x005FB8)), t.window);
        assert_eq!(classify(rgb(0x005FB8), LIGHT), Fill::Highlight);
        assert_eq!(classify(rgb(0x5C5C5C), LIGHT), Fill::Ink);
        assert_eq!(classify(rgb(0xFFFFFF), LIGHT), Fill::Window);
    }

    #[test]
    fn text_and_borders_take_the_system_colors() {
        let t = SystemColors::theme("night-sky").unwrap();
        let ink = |s: Scheme| forced(s).ink(rgb(0x123456));
        assert_eq!(ink(Scheme::default()), t.window_text);
        let b = Scheme {
            in_button: true,
            ..Default::default()
        };
        assert_eq!(ink(b), t.button_text);
        assert_eq!(forced(b).edge(rgb(0x123456)), t.button_text);
        let link = Scheme {
            link: true,
            in_button: true,
            ..Default::default()
        };
        assert_eq!(ink(link), t.hotlight);
        let on = Scheme {
            behind: Fill::Highlight,
            link: true,
            ..Default::default()
        };
        assert_eq!(ink(on), t.highlight_text);
        // On a knob-like fill, the window's color.
        let knob = Scheme {
            behind: Fill::Ink,
            ..Default::default()
        };
        assert_eq!(ink(knob), t.window);
        let off = Scheme {
            disabled: true,
            behind: Fill::Highlight,
            ..Default::default()
        };
        assert_eq!(ink(off), t.gray_text);
        assert_eq!(forced(off).edge(rgb(0x123456)), t.gray_text);
        assert_eq!(forced(Scheme::default()).edge(rgb(0)), t.window_text);
        assert_eq!(
            forced(Scheme::default()).ink(Color::TRANSPARENT),
            Color::TRANSPARENT
        );
    }
}
