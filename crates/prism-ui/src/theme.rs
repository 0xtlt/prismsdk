//! Themes: colors, shape and type scale shared by every component, plus
//! per-component overrides.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

use prism_core::{App, Appearance, Color, Element, rgb};

use crate::Component;

/// Look and feel of every `prism-ui` component. Install it with
/// `cx.set_global(Theme::light().accent(rgb(0x0A84FF)))`.
#[derive(Clone)]
pub struct Theme {
    pub accent: Color,
    /// Text/icon color on top of `accent`.
    pub on_accent: Color,
    pub background: Color,
    pub surface: Color,
    pub surface_alt: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub danger: Color,
    pub success: Color,
    pub radius: f32,
    pub spacing: f32,
    pub font_size: f32,
    /// Render `Button`/`Switch` as the platform's native controls instead of
    /// custom-drawn ones (per instance: `.native()`).
    pub prefer_native: bool,
    pub dark: bool,
    overrides: HashMap<TypeId, Rc<dyn Any>>,
}

type Override<C> = Rc<dyn Fn(C, &mut App) -> Element>;

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}

impl Theme {
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
            overrides: HashMap::new(),
        }
    }

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

    pub fn accent(mut self, accent: Color) -> Self {
        self.accent = accent;
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

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
/// to its light/dark appearance.
pub fn set_theme(cx: &mut App, theme: Theme) {
    cx.set_appearance(if theme.dark {
        Appearance::Dark
    } else {
        Appearance::Light
    });
    cx.set_global(theme);
}

/// Edits the installed theme in place (see [`set_theme`]).
pub fn update_theme(cx: &mut App, f: impl FnOnce(&mut Theme)) {
    let mut theme = Theme::of(cx);
    f(&mut theme);
    set_theme(cx, theme);
}
