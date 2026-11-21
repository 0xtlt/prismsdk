//! A one-line field's leading icon (`text_input().leading_icon("magnifyingglass")`):
//! its Segoe Fluent Icons glyph in the field's first 24 DIPs, in the
//! placeholder's color, the text starting after it. Like macOS (the text
//! at the leading padding, 32 at least) and GTK (the entry's primary icon).
//! WinUI's `TextBox` has no icon slot: the glyph is a `FontIcon` beside it
//! in the node's canvas, which mirrors itself right to left.

use prism_core::NodeKind;
use windows_core::{Interface, Result};

use crate::fluent::Palette;
use crate::scene::Node;
use crate::text::font;
use crate::winui::xaml::Controls::{Canvas, FontIcon};
use crate::winui::xaml::UIElement;
use crate::winui::{brush, h};

/// The icon's room at the field's leading edge.
const ROOM: f32 = 24.;

/// Whether `n` shows a leading icon.
pub fn has_icon(n: &Node) -> bool {
    n.kind == NodeKind::TextInput && !n.icon.is_empty()
}

/// The field's leading padding, with room for its icon: 32 at least when
/// the app draws the field (its padding), else the icon's room added to
/// WinUI's own.
pub fn leading(n: &Node) -> f32 {
    let s = &n.style;
    let own = s.own_field_look();
    let padding = match (own, s.has_padding()) {
        (true, true) => s.padding.left,
        (true, false) => 0.,
        (false, _) => crate::fluent::metrics::FIELD_PADDING.0,
    };
    match (has_icon(n), own) {
        (false, _) => padding,
        (true, true) => padding.max(32.),
        (true, false) => padding + ROOM,
    }
}

/// Shows, places or removes the icon of field `n` in its canvas `outer`
/// (`slot`: the view's).
pub fn sync(slot: &mut Option<FontIcon>, outer: &Canvas, n: &Node, pal: &Palette) -> Result<()> {
    if !has_icon(n) {
        if let Some(icon) = slot.take() {
            remove(outer, &icon)?;
        }
        return Ok(());
    }
    let icon = match slot {
        Some(icon) => icon.clone(),
        None => {
            let icon = FontIcon::new()?;
            icon.SetFontFamily(&font(crate::text::ICON_FONT)?)?;
            icon.SetIsHitTestVisible(false)?;
            icon.SetIsTextScaleFactorEnabled(false)?;
            outer.Children()?.Append(&icon)?;
            *slot = Some(icon.clone());
            icon
        }
    };
    let glyph = crate::keys::fluent_icon(&n.icon)
        .map(String::from)
        .unwrap_or_else(|| prism_core::glyphs::glyph(&n.icon).to_string());
    icon.SetGlyph(&h(&glyph))?;
    let size = n.style.font_size.clamp(12., 18.);
    icon.SetFontSize(size as f64)?;
    let color = n
        .style
        .text_color
        .map_or(pal.text_secondary, |c| c.with_alpha(c.a * 0.6));
    icon.SetForeground(&brush(pal.ink(color))?)?;
    let e: UIElement = icon.cast()?;
    let lead = leading(n);
    Canvas::SetLeft(
        &e,
        (lead - ROOM + (ROOM - size) / 2.).max(0.).round() as f64,
    )?;
    Canvas::SetTop(&e, ((n.frame.h - size) / 2.).max(0.).round() as f64)?;
    Ok(())
}

fn remove(parent: &Canvas, icon: &FontIcon) -> Result<()> {
    let children = parent.Children()?;
    let mut at = 0u32;
    if children.IndexOf(&icon.cast::<UIElement>()?, &mut at)? {
        children.RemoveAt(at)?;
    }
    Ok(())
}
