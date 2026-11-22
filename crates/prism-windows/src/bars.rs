//! A navigation stack's header and a tab bar, from WinUI controls: the
//! header is a back button (`NavigationBackButtonNormalStyle`, the one
//! `NavigationView` and the title bar use), the top screen's title and its
//! bar buttons; the tab bar is a `SelectorBar` (the Windows 11 top tabs,
//! with their sliding selection pill).
//!
//! A bar's tint (`Style::tint`: `Router::tint`, else the app's accent
//! color) takes the system accent's place: the selection pill of the tab
//! bar, Back and the bar buttons of the header, in every state (their
//! theme resources, set before the control loads its template).

use prism_core::navbar::ScreenConfig;
use prism_core::{Color, NodeId, UiEvent};
use windows::Foundation::TypedEventHandler;
use windows_core::{Interface, Result};

use crate::fluent::Palette;
use crate::imp::{changed, defer, dispatch};
use crate::scene::{HEADER, Scene, TAB_BAR};
use crate::text::{DISPLAY_FONT, ICON_FONT, font, weight};
use crate::winui::xaml::Controls::{
    Border, Button, Canvas, FontIcon, Grid, Orientation, SelectorBar, SelectorBarItem,
    SelectorBarSelectionChangedEventArgs, StackPanel, TextBlock,
};
use crate::winui::xaml::Media::Brush;
use crate::winui::xaml::{
    Application, HorizontalAlignment, RoutedEventHandler, Style as XamlStyle, TextTrimming,
    UIElement, VerticalAlignment,
};
use crate::winui::{boxed, brush, h, thickness, uniform};

/// A stack's header bar.
pub struct Header {
    pub root: Border,
    stack: NodeId,
    left: StackPanel,
    back: Button,
    /// The tint Back was made with (its states' brushes).
    back_tint: Option<Color>,
    title: TextBlock,
    leading: StackPanel,
    trailing: StackPanel,
    /// What it shows now: rebuilt only when that changes.
    shown: Option<(bool, bool, String, String, bool, Option<Color>)>,
}

fn resource(key: &str) -> Option<windows_core::IInspectable> {
    let app = Application::Current().ok()?;
    app.Resources().ok()?.Lookup(&boxed(key).ok()?).ok()
}

/// The theme resources Back's template colors its glyph with when hovered
/// and pressed (`NavigationBackButtonNormalStyle`).
const BACK_STATES: [&str; 2] = [
    "NavigationViewButtonForegroundPointerOver",
    "NavigationViewButtonForegroundPressed",
];

/// A button's text in every state (`DefaultButtonStyle`).
const BUTTON_TEXT: [&str; 3] = [
    "ButtonForeground",
    "ButtonForegroundPointerOver",
    "ButtonForegroundPressed",
];

/// The tint of a tab bar or a stack (`Style::tint`), resolved.
fn tint_of(scene: &Scene, id: NodeId, pal: &Palette) -> Option<Color> {
    pal.tint(scene.get(id).and_then(|n| n.style.tint))
}

/// A button's text in `tint`, hovered and pressed too; pressed is dimmed
/// like WinUI's secondary text.
fn tint_button(b: &Button, tint: Color, states: &[&str]) -> Result<()> {
    let fg: Brush = brush(tint)?.cast()?;
    b.SetForeground(&fg)?;
    let res = b.Resources()?;
    for k in states {
        let c = if k.ends_with("Pressed") {
            tint.with_alpha(tint.a * 0.786)
        } else {
            tint
        };
        res.Insert(&boxed(k)?, &brush(c)?)?;
    }
    Ok(())
}

/// The header's Back (or Close) button, in the bar's tint.
fn back_button(stack: NodeId, tint: Option<Color>) -> Result<Button> {
    let back = Button::new()?;
    if let Some(style) =
        resource("NavigationBackButtonNormalStyle").and_then(|s| s.cast::<XamlStyle>().ok())
    {
        back.SetStyle(&style)?;
    }
    if let Some(t) = tint {
        tint_button(&back, t, &BACK_STATES)?;
    }
    back.Click(&RoutedEventHandler::new(move |_, _| {
        defer(Box::new(move |s| {
            let depth = s.renderer.widgets.scene.pages(stack).len();
            dispatch(UiEvent::Back(stack, depth.saturating_sub(1)));
        }));
        Ok(())
    }))?;
    Ok(back)
}

impl Header {
    pub fn new(stack: NodeId) -> Result<Header> {
        let root = Border::new()?;
        let grid = Grid::new()?;
        root.SetChild(&grid)?;
        let left = StackPanel::new()?;
        left.SetOrientation(Orientation::Horizontal)?;
        left.SetVerticalAlignment(VerticalAlignment::Center)?;
        left.SetSpacing(4.)?;
        left.SetPadding(thickness(4., 0., 0., 0.))?;
        let back = back_button(stack, None)?;
        let leading = StackPanel::new()?;
        leading.SetOrientation(Orientation::Horizontal)?;
        let title = TextBlock::new()?;
        title.SetFontFamily(&font(DISPLAY_FONT)?)?;
        title.SetFontSize(18.)?;
        title.SetFontWeight(weight(prism_core::FontWeight::Semibold))?;
        title.SetVerticalAlignment(VerticalAlignment::Center)?;
        title.SetTextTrimming(TextTrimming::CharacterEllipsis)?;
        title.SetIsTextScaleFactorEnabled(false)?;
        title.SetMargin(thickness(12., 0., 0., 0.))?;
        let children = left.Children()?;
        children.Append(&back)?;
        children.Append(&leading)?;
        children.Append(&title)?;
        let trailing = StackPanel::new()?;
        trailing.SetOrientation(Orientation::Horizontal)?;
        trailing.SetHorizontalAlignment(HorizontalAlignment::Right)?;
        trailing.SetVerticalAlignment(VerticalAlignment::Center)?;
        trailing.SetSpacing(4.)?;
        trailing.SetPadding(thickness(0., 0., 8., 0.))?;
        grid.Children()?.Append(&left)?;
        grid.Children()?.Append(&trailing)?;
        // Along the window's top edge, its empty part moves the window
        // (`titlebar.rs`): where its buttons are follows them.
        for e in [root.cast()?, left.cast()?, trailing.cast()?] {
            crate::titlebar::watch(&e)?;
        }
        Ok(Header {
            root,
            stack,
            left,
            back,
            back_tint: None,
            title,
            leading,
            trailing,
            shown: None,
        })
    }

    /// Shows the stack's top screen: Back (or Close), title, bar buttons;
    /// its trailing end keeps `inset` free (the caption buttons over it).
    pub fn update(
        &mut self,
        scene: &Scene,
        stack: NodeId,
        pal: &Palette,
        width: f32,
        inset: f32,
    ) -> Result<()> {
        self.root.SetWidth(width.max(0.) as f64)?;
        self.trailing
            .SetPadding(thickness(0., 0., 8. + inset, 0.))?;
        self.root.SetHeight(HEADER as f64)?;
        self.root.SetBackground(&brush(pal.window)?)?;
        self.root.SetBorderBrush(&brush(pal.divider)?)?;
        self.root.SetBorderThickness(thickness(0., 0., 0., 1.))?;
        self.title.SetForeground(&brush(pal.text)?)?;
        let pages = scene.pages(stack);
        let top = pages.last().and_then(|t| scene.get(*t).map(|n| (*t, n)));
        let title = top.map(|(_, n)| n.text.clone()).unwrap_or_default();
        let config = top.map(|(_, n)| n.config.clone()).unwrap_or_default();
        let back = scene.has_back(stack);
        let closes = pages.len() <= 1;
        let tint = tint_of(scene, stack, pal);
        let shown = Some((back, closes, title.clone(), config.clone(), pal.dark, tint));
        if self.shown == shown {
            return Ok(());
        }
        self.shown = shown;
        if tint != self.back_tint {
            // A new button: its template reads the states' brushes as it
            // loads.
            let fresh = back_button(self.stack, tint)?;
            let children = self.left.Children()?;
            let mut at = 0u32;
            if children.IndexOf(&self.back, &mut at)? {
                children.SetAt(at, &fresh)?;
            }
            self.back = fresh;
            self.back_tint = tint;
        }
        self.back.SetVisibility(if back {
            crate::winui::xaml::Visibility::Visible
        } else {
            crate::winui::xaml::Visibility::Collapsed
        })?;
        if closes {
            // The first screen of a closable stack closes it.
            let icon = FontIcon::new()?;
            icon.SetGlyph(&h("\u{E711}"))?;
            icon.SetFontSize(16.)?;
            self.back.SetContent(&icon)?;
        } else {
            self.back
                .ClearValue(&crate::winui::xaml::Controls::ContentControl::ContentProperty()?)?;
        }
        crate::winui::xaml::Automation::AutomationProperties::SetName(
            &self.back,
            &h(if closes { "Close" } else { "Back" }),
        )?;
        self.title.SetText(&h(&title))?;
        let config = ScreenConfig::decode(&config);
        let screen = top.map(|(id, _)| id);
        for (panel, items) in [
            (&self.leading, config.leading().collect::<Vec<_>>()),
            (&self.trailing, config.trailing().collect::<Vec<_>>()),
        ] {
            let children = panel.Children()?;
            children.Clear()?;
            for (i, item) in items {
                let b = bar_button(item, pal, tint)?;
                if let Some(screen) = screen {
                    b.Click(&RoutedEventHandler::new(move |_, _| {
                        dispatch(UiEvent::Change(screen, i as f64));
                        Ok(())
                    }))?;
                }
                children.Append(&b)?;
            }
        }
        Ok(())
    }

    /// What takes clicks in it: Back, the bar buttons.
    pub fn controls(&self) -> Vec<UIElement> {
        let mut out: Vec<UIElement> = self.back.cast().into_iter().collect();
        for panel in [&self.leading, &self.trailing] {
            if let Ok(children) = panel.Children() {
                out.extend(children);
            }
        }
        out
    }
}

/// A bar button: its Fluent icon, else its title; no chrome until hovered;
/// in the bar's tint.
fn bar_button(
    item: &prism_core::navbar::BarItem,
    pal: &Palette,
    tint: Option<Color>,
) -> Result<Button> {
    let b = Button::new()?;
    b.SetBackground(&crate::winui::transparent()?)?;
    b.SetBorderThickness(uniform(0.))?;
    b.SetMinHeight(36.)?;
    b.SetMinWidth(40.)?;
    if let Some(t) = tint {
        tint_button(&b, t, &BUTTON_TEXT)?;
    }
    match crate::keys::fluent_icon(&item.icon).filter(|_| !item.icon.is_empty()) {
        Some(g) => {
            let icon = FontIcon::new()?;
            icon.SetFontFamily(&font(ICON_FONT)?)?;
            icon.SetGlyph(&h(&g.to_string()))?;
            icon.SetFontSize(16.)?;
            icon.SetForeground(&brush(tint.unwrap_or(pal.text))?)?;
            b.SetContent(&icon)?;
        }
        None => {
            let text = if item.title.is_empty() {
                prism_core::glyphs::glyph(&item.icon).to_string()
            } else {
                item.title.clone()
            };
            b.SetContent(&boxed(&text)?)?;
        }
    }
    crate::winui::xaml::Automation::AutomationProperties::SetName(&b, &h(item.spoken()))?;
    Ok(b)
}

/// A tab bar.
pub struct TabBar {
    pub root: Border,
    bar: SelectorBar,
    /// The tabs it shows: (title, icon) each, and the selection.
    shown: Vec<(String, String)>,
    /// The tint its items were made with (their selection pill).
    tint: Option<Color>,
}

impl TabBar {
    pub fn new(tabs: NodeId) -> Result<TabBar> {
        let root = Border::new()?;
        let bar = SelectorBar::new()?;
        bar.SetVerticalAlignment(VerticalAlignment::Center)?;
        bar.SetMargin(thickness(4., 0., 4., 0.))?;
        root.SetChild(&bar)?;
        crate::titlebar::watch(&root.cast()?)?;
        bar.SelectionChanged(&TypedEventHandler::<
            SelectorBar,
            SelectorBarSelectionChangedEventArgs,
        >::new(move |s, _| {
            if let Some(bar) = s.as_ref() {
                let selected = bar.SelectedItem()?;
                let items = bar.Items()?;
                let mut index = 0u32;
                if items.IndexOf(&selected, &mut index)? {
                    changed(tabs, index as f64);
                }
            }
            Ok(())
        }))?;
        Ok(TabBar {
            root,
            bar,
            shown: Vec::new(),
            tint: None,
        })
    }

    /// Shows the tabs; its trailing end keeps `inset` free (the caption
    /// buttons over it).
    pub fn update(
        &mut self,
        scene: &Scene,
        tabs: NodeId,
        pal: &Palette,
        width: f32,
        inset: f32,
    ) -> Result<()> {
        self.root.SetWidth(width.max(0.) as f64)?;
        self.bar.SetMargin(thickness(4., 0., 4. + inset, 0.))?;
        self.root.SetHeight(TAB_BAR as f64)?;
        self.root.SetBackground(&brush(pal.window)?)?;
        self.root.SetBorderBrush(&brush(pal.divider)?)?;
        self.root.SetBorderThickness(thickness(0., 0., 0., 1.))?;
        let pages = scene.pages(tabs);
        let want: Vec<(String, String)> = pages
            .iter()
            .filter_map(|t| scene.get(*t))
            .map(|n| (n.text.clone(), n.icon.clone()))
            .collect();
        let items = self.bar.Items()?;
        let tint = tint_of(scene, tabs, pal);
        if want != self.shown || tint != self.tint {
            items.Clear()?;
            for (text, icon) in &want {
                let item = SelectorBarItem::new()?;
                if let Some(t) = tint {
                    // The selection pill (`SelectorBarItemPill`'s fill).
                    item.Resources()?
                        .Insert(&boxed("SelectorBarItemPillFill")?, &brush(t)?)?;
                }
                item.SetText(&h(text))?;
                crate::a11y::tab_item(&item)?;
                if let Some(g) = crate::keys::fluent_icon(icon) {
                    let fi = FontIcon::new()?;
                    fi.SetFontFamily(&font(ICON_FONT)?)?;
                    fi.SetGlyph(&h(&g.to_string()))?;
                    item.SetIcon(&fi)?;
                }
                crate::titlebar::watch(&item.cast()?)?;
                items.Append(&item)?;
            }
            self.shown = want;
            self.tint = tint;
        }
        let selected = scene.get(tabs).map_or(0, |n| n.selected());
        if let Ok(item) = items.GetAt(selected as u32) {
            let now = self.bar.SelectedItem().ok();
            if now.as_ref() != Some(&item) {
                self.bar.SetSelectedItem(&item)?;
            }
        }
        Ok(())
    }

    /// What takes clicks in it: its tabs.
    pub fn controls(&self) -> Vec<UIElement> {
        self.bar
            .Items()
            .map(|items| items.into_iter().filter_map(|i| i.cast().ok()).collect())
            .unwrap_or_default()
    }
}

/// Where a bar goes in its container's box.
pub fn place(root: &Border) -> Result<()> {
    let e: crate::winui::xaml::UIElement = root.cast()?;
    Canvas::SetLeft(&e, 0.)?;
    Canvas::SetTop(&e, 0.)?;
    Canvas::SetZIndex(&e, 1)?;
    Ok(())
}
