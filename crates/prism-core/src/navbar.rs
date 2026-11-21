//! A screen's navigation bar: large title on or off, bar buttons (#30).
//!
//! ```ignore
//! screen("Todos")
//!     .large_title(false)
//!     .trailing(bar_button("Add").icon("plus").on_click(navigate_to("/todos/new")))
//! ```
//!
//! The options travel as the screen's config ([`ScreenConfig`]); a tap on
//! bar button `i` comes back as `UiEvent::Change(screen, i)` (its index in
//! [`ScreenConfig::items`]), which runs the button's handler.
//!
//! | | large title | bar buttons |
//! |---|---|---|
//! | iOS, iPadOS, visionOS | `largeTitleDisplayMode` | `UIBarButtonItem`s |
//! | tvOS | none (no large titles) | `UIBarButtonItem`s |
//! | macOS | none | buttons in the header bar |
//! | Linux | none | buttons packed in the header |
//! | Android | none | buttons in the header |
//! | Windows | none | buttons in the header |
//! | watchOS | none | toolbar items |
//! | Web | none | buttons in `.prism-header` |

use crate::app::App;
use crate::element::{ClickEvent, Handler};
use crate::style::Color;

/// Which side of the navigation bar a button sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BarPlacement {
    /// Before the title (after Back, which stays): the left side, the right
    /// side in right-to-left languages.
    Leading,
    /// After the title: the right side, the left side right to left.
    #[default]
    Trailing,
}

/// One button of a navigation bar, as backends receive it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BarItem {
    /// Leading or trailing.
    pub placement: BarPlacement,
    /// The visible title (empty: the icon alone).
    pub title: String,
    /// An SF Symbol name (`plus`), a glyph on the web and on other
    /// platforms (see [`glyphs`](crate::glyphs)). Empty: the title alone.
    pub icon: String,
    /// What assistive technologies announce (empty: the title).
    pub label: String,
}

impl BarItem {
    /// The accessibility label: the label, else the title, else the icon name.
    pub fn spoken(&self) -> &str {
        [&self.label, &self.title, &self.icon]
            .into_iter()
            .find(|s| !s.is_empty())
            .map_or("", String::as_str)
    }
}

/// A screen's navigation bar options: its config string.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScreenConfig {
    /// `Some(false)`: a small title even where stacks show large titles
    /// (iOS); `Some(true)`: always large; `None`: the platform decides
    /// (large at the top of a scroll view, small once scrolled on iOS).
    pub large_title: Option<bool>,
    /// The bar buttons, in the order they were added; leading ones go
    /// before the title, trailing ones after it, each side in this order.
    pub items: Vec<BarItem>,
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

impl ScreenConfig {
    /// `large`, `inline` or nothing on the first line, then one button per
    /// line: `L` or `T`, icon, title and label separated by tabs. A screen
    /// without options has the empty config.
    pub fn encode(&self) -> String {
        let mut out = String::from(match self.large_title {
            Some(true) => "large",
            Some(false) => "inline",
            None => "",
        });
        for item in &self.items {
            let side = match item.placement {
                BarPlacement::Leading => 'L',
                BarPlacement::Trailing => 'T',
            };
            out.push('\n');
            out.push(side);
            for part in [&item.icon, &item.title, &item.label] {
                out.push('\t');
                out.push_str(&clean(part));
            }
        }
        out
    }

    /// Parses [`encode`](Self::encode)'s output (the empty string: no options).
    pub fn decode(s: &str) -> Self {
        let mut lines = s.split('\n');
        let large_title = match lines.next() {
            Some("large") => Some(true),
            Some("inline") => Some(false),
            _ => None,
        };
        let items = lines
            .map(|line| {
                let mut parts = line.split('\t');
                let placement = match parts.next() {
                    Some("L") => BarPlacement::Leading,
                    _ => BarPlacement::Trailing,
                };
                let mut next = || parts.next().unwrap_or("").to_string();
                BarItem {
                    placement,
                    icon: next(),
                    title: next(),
                    label: next(),
                }
            })
            .collect();
        ScreenConfig { large_title, items }
    }

    /// The leading buttons with their index in [`items`](Self::items) (the
    /// value of the change event their tap reports).
    pub fn leading(&self) -> impl Iterator<Item = (usize, &BarItem)> {
        self.side(BarPlacement::Leading)
    }

    /// The trailing buttons with their index in [`items`](Self::items).
    pub fn trailing(&self) -> impl Iterator<Item = (usize, &BarItem)> {
        self.side(BarPlacement::Trailing)
    }

    fn side(&self, placement: BarPlacement) -> impl Iterator<Item = (usize, &BarItem)> {
        self.items
            .iter()
            .enumerate()
            .filter(move |(_, i)| i.placement == placement)
    }
}

/// A button for a screen's navigation bar:
/// `screen("Todos").trailing(bar_button("Add").icon("plus").on_click(..))`.
/// With an icon and a title, platforms that show one or the other (iOS,
/// macOS) show the icon and announce the title.
pub fn bar_button(title: impl Into<String>) -> BarButton {
    BarButton {
        item: BarItem {
            title: title.into(),
            ..BarItem::default()
        },
        on_click: None,
    }
}

/// A navigation bar button. See [`bar_button`].
pub struct BarButton {
    pub(crate) item: BarItem,
    pub(crate) on_click: Option<Handler<ClickEvent>>,
}

impl BarButton {
    /// An SF Symbol (`plus`, `square.and.pencil`); a glyph on the web and
    /// on other platforms (see [`glyphs`](crate::glyphs)).
    pub fn icon(mut self, symbol: impl Into<String>) -> Self {
        self.item.icon = symbol.into();
        self
    }

    /// What assistive technologies announce (needed for an icon alone).
    pub fn accessibility_label(mut self, label: impl Into<String>) -> Self {
        self.item.label = label.into();
        self
    }

    /// Called when the button is tapped.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.on_click = Some(handler);
        self
    }
}

/// The app's accent color (a global): bars that have a tint take it.
struct AccentColor(Option<Color>);

impl App {
    /// Sets the app's accent color: the selected tab of the router's tab
    /// bar and the header bars' buttons (Back included) take it unless the
    /// router has its own ([`RouterBuilder::tint`](crate::RouterBuilder::tint)).
    /// `prism_ui::set_theme` sets it to the theme's accent. `None`: the
    /// platform's (system blue on Apple platforms).
    pub fn set_accent_color(&mut self, color: Option<Color>) {
        if self.accent_color() != color {
            self.set_global(AccentColor(color));
        }
    }

    /// The accent color set with [`set_accent_color`](App::set_accent_color).
    pub fn accent_color(&self) -> Option<Color> {
        self.try_global::<AccentColor>().and_then(|a| a.0)
    }
}
