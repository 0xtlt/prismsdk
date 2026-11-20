//! A navigation stack's header and a tab bar, from WinUI controls: the
//! header is a back button (`NavigationBackButtonNormalStyle`, the one
//! `NavigationView` and the title bar use), the top screen's title and its
//! bar buttons; the tab bar is a `SelectorBar` (the Windows 11 top tabs,
//! with their sliding selection pill).

use prism_core::navbar::ScreenConfig;
use prism_core::{NodeId, UiEvent};
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
use crate::winui::xaml::{
    Application, HorizontalAlignment, RoutedEventHandler, Style as XamlStyle, TextTrimming,
    VerticalAlignment,
};
use crate::winui::{boxed, brush, h, thickness, uniform};

/// A stack's header bar.
pub struct Header {
    pub root: Border,
    back: Button,
    title: TextBlock,
    leading: StackPanel,
    trailing: StackPanel,
    /// What it shows now: rebuilt only when that changes.
    shown: Option<(bool, bool, String, String, bool)>,
}

fn resource(key: &str) -> Option<windows_core::IInspectable> {
    let app = Application::Current().ok()?;
    app.Resources().ok()?.Lookup(&boxed(key).ok()?).ok()
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
        let back = Button::new()?;
        if let Some(style) =
            resource("NavigationBackButtonNormalStyle").and_then(|s| s.cast::<XamlStyle>().ok())
        {
            back.SetStyle(&style)?;
        }
        back.Click(&RoutedEventHandler::new(move |_, _| {
            defer(Box::new(move |s| {
                let depth = s.renderer.widgets.scene.pages(stack).len();
                dispatch(UiEvent::Back(stack, depth.saturating_sub(1)));
            }));
            Ok(())
        }))?;
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
        Ok(Header {
            root,
            back,
            title,
            leading,
            trailing,
            shown: None,
        })
    }

    /// Shows the stack's top screen: Back (or Close), title, bar buttons.
    pub fn update(
        &mut self,
        scene: &Scene,
        stack: NodeId,
        pal: &Palette,
        width: f32,
    ) -> Result<()> {
        self.root.SetWidth(width.max(0.) as f64)?;
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
        let shown = Some((back, closes, title.clone(), config.clone(), pal.dark));
        if self.shown == shown {
            return Ok(());
        }
        self.shown = shown;
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
                let b = bar_button(item, pal)?;
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
}

/// A bar button: its Fluent icon, else its title; no chrome until hovered.
fn bar_button(item: &prism_core::navbar::BarItem, pal: &Palette) -> Result<Button> {
    let b = Button::new()?;
    b.SetBackground(&crate::winui::transparent()?)?;
    b.SetBorderThickness(uniform(0.))?;
    b.SetMinHeight(36.)?;
    b.SetMinWidth(40.)?;
    match crate::keys::fluent_icon(&item.icon).filter(|_| !item.icon.is_empty()) {
        Some(g) => {
            let icon = FontIcon::new()?;
            icon.SetFontFamily(&font(ICON_FONT)?)?;
            icon.SetGlyph(&h(&g.to_string()))?;
            icon.SetFontSize(16.)?;
            icon.SetForeground(&brush(pal.text)?)?;
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
}

impl TabBar {
    pub fn new(tabs: NodeId) -> Result<TabBar> {
        let root = Border::new()?;
        let bar = SelectorBar::new()?;
        bar.SetVerticalAlignment(VerticalAlignment::Center)?;
        bar.SetMargin(thickness(4., 0., 4., 0.))?;
        root.SetChild(&bar)?;
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
        })
    }

    pub fn update(&mut self, scene: &Scene, tabs: NodeId, pal: &Palette, width: f32) -> Result<()> {
        self.root.SetWidth(width.max(0.) as f64)?;
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
        if want != self.shown {
            items.Clear()?;
            for (text, icon) in &want {
                let item = SelectorBarItem::new()?;
                item.SetText(&h(text))?;
                if let Some(g) = crate::keys::fluent_icon(icon) {
                    let fi = FontIcon::new()?;
                    fi.SetFontFamily(&font(ICON_FONT)?)?;
                    fi.SetGlyph(&h(&g.to_string()))?;
                    item.SetIcon(&fi)?;
                }
                items.Append(&item)?;
            }
            self.shown = want;
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
}

/// Where a bar goes in its container's box.
pub fn place(root: &Border) -> Result<()> {
    let e: crate::winui::xaml::UIElement = root.cast()?;
    Canvas::SetLeft(&e, 0.)?;
    Canvas::SetTop(&e, 0.)?;
    Canvas::SetZIndex(&e, 1)?;
    Ok(())
}
