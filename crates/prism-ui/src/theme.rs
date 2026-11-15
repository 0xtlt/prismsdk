//! Themes: colors, shape and type scale shared by every component, plus
//! per-component overrides.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

use prism_core::{App, Appearance, Color, Element, adaptive, rgb};

use crate::Component;

/// Look and feel of every `prism-ui` component. Install it with
/// `cx.set_global(Theme::light().accent(rgb(0x0A84FF)))`.
#[derive(Clone)]
pub struct Theme {
    /// Main color: primary buttons, switches, links, the first chart series.
    pub accent: Color,
    /// Text/icon color on top of `accent`.
    pub on_accent: Color,
    /// Screen background.
    pub background: Color,
    /// Cards and other raised surfaces.
    pub surface: Color,
    /// Secondary surfaces: secondary buttons, badges, empty charts.
    pub surface_alt: Color,
    /// Body text.
    pub text: Color,
    /// Secondary text (subtitles, captions, axis labels).
    pub muted: Color,
    /// Separators and outlines.
    pub border: Color,
    /// Destructive actions and errors.
    pub danger: Color,
    /// Positive values and confirmations.
    pub success: Color,
    /// Corner radius of controls (cards add 4 pt).
    pub radius: f32,
    /// Padding inside cards.
    pub spacing: f32,
    /// Body text size in points; other sizes derive from it.
    pub font_size: f32,
    /// Render `Button`/`Switch` as the platform's native controls instead of
    /// custom-drawn ones (per instance: `.native()`).
    pub prefer_native: bool,
    /// A fixed dark theme (see [`Theme::system`] for one that follows the OS).
    pub dark: bool,
    /// Colors are adaptive and native chrome follows the system appearance.
    pub follows_system: bool,
    overrides: HashMap<TypeId, Rc<dyn Any>>,
}

type Override<C> = Rc<dyn Fn(C, &mut App) -> Element>;

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}

impl Theme {
    /// The default light theme.
    pub fn light() -> Self {
        Theme {
            accent: rgb(0x5B5BD6),
            on_accent: rgb(0xFFFFFF),
            background: rgb(0xF4F4F8),
            surface: rgb(0xFFFFFF),
            surface_alt: rgb(0xEDEDF7),
            text: rgb(0x1C1C28),
            muted: rgb(0x6E6E80),
            border: rgb(0xE2E2EA),
            danger: rgb(0xD93636),
            success: rgb(0x2E9E5B),
            radius: 10.,
            spacing: 16.,
            font_size: 16.,
            prefer_native: false,
            dark: false,
            follows_system: false,
            overrides: HashMap::new(),
        }
    }

    /// A dark theme (native chrome switches to dark too).
    pub fn dark() -> Self {
        Theme {
            accent: rgb(0x8B8BFF),
            on_accent: rgb(0x111114),
            background: rgb(0x111114),
            surface: rgb(0x1C1C22),
            surface_alt: rgb(0x26262E),
            text: rgb(0xF2F2F7),
            muted: rgb(0x9A9AA8),
            border: rgb(0x2E2E38),
            danger: rgb(0xFF6B6B),
            success: rgb(0x4CD983),
            dark: true,
            ..Theme::light()
        }
    }

    /// Follows the system light/dark setting live: every color is adaptive,
    /// in the app, in widgets and in Live Activities.
    pub fn system() -> Self {
        let (l, d) = (Theme::light(), Theme::dark());
        Theme {
            accent: adaptive(l.accent, d.accent),
            on_accent: adaptive(l.on_accent, d.on_accent),
            background: adaptive(l.background, d.background),
            surface: adaptive(l.surface, d.surface),
            surface_alt: adaptive(l.surface_alt, d.surface_alt),
            text: adaptive(l.text, d.text),
            muted: adaptive(l.muted, d.muted),
            border: adaptive(l.border, d.border),
            danger: adaptive(l.danger, d.danger),
            success: adaptive(l.success, d.success),
            follows_system: true,
            ..l
        }
    }

    /// Sets the accent; on a system theme, `accent` may itself be adaptive.
    pub fn accent(mut self, accent: Color) -> Self {
        self.accent = accent;
        self
    }

    /// Sets the corner radius of controls.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Renders `Button` and `Switch` as the platform's native controls
    /// (per instance: `.native()` / `.custom()`).
    pub fn prefer_native(mut self, native: bool) -> Self {
        self.prefer_native = native;
        self
    }

    /// Replaces how a component renders, app-wide. The closure receives the
    /// component's props, so it can keep its behaviour and change its look:
    ///
    /// ```ignore
    /// Theme::light().override_component(|b: ui::Button, _cx| {
    ///     div().p(12.).bg(rgb(0x000000))
    ///         .accessibility_role(Role::Button)
    ///         .child(text(b.label.clone()).text_color(rgb(0xFFFFFF)))
    ///         .when_some(b.on_click.clone(), |d, h| d.on_click(h))
    ///         .into_element()
    /// })
    /// ```
    pub fn override_component<C: Component>(
        mut self,
        render: impl Fn(C, &mut App) -> Element + 'static,
    ) -> Self {
        let f: Override<C> = Rc::new(render);
        self.overrides.insert(TypeId::of::<C>(), Rc::new(f));
        self
    }

    pub(crate) fn override_for<C: Component>(&self) -> Option<Override<C>> {
        self.overrides
            .get(&TypeId::of::<C>())?
            .downcast_ref::<Override<C>>()
            .cloned()
    }

    /// The installed theme, or the light theme.
    pub fn of(cx: &App) -> Theme {
        cx.try_global::<Theme>().cloned().unwrap_or_default()
    }
}

/// Installs `theme` app-wide and matches native chrome (bars, native controls)
/// to its light/dark appearance; the router's tab bar and header bars take
/// its accent ([`App::set_accent_color`]).
pub fn set_theme(cx: &mut App, theme: Theme) {
    cx.set_appearance(if theme.follows_system {
        Appearance::System
    } else if theme.dark {
        Appearance::Dark
    } else {
        Appearance::Light
    });
    // The tab bar and header bars follow the accent (#30).
    cx.set_accent_color(Some(theme.accent));
    cx.set_global(theme);
}

/// Edits the installed theme in place (see [`set_theme`]).
pub fn update_theme(cx: &mut App, f: impl FnOnce(&mut Theme)) {
    let mut theme = Theme::of(cx);
    f(&mut theme);
    set_theme(cx, theme);
}
