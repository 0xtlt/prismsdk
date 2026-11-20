//! What the backend draws for each kind, in the Windows 11 (Fluent) look
//! of WinUI 3: text, buttons, text boxes, toggles, check boxes, radio
//! buttons, sliders, progress bars and rings, combo boxes, date pickers,
//! symbol images, pictures, shapes, a stack's header, a tab bar (a
//! `NavigationView` in top mode), overlay scroll bars and focus rings. Also
//! their natural sizes and baselines (`Widgets::measure`, `baseline`).
//!
//! An app that styles a button or a text field (a background, a border,
//! `.border(0.)`) draws it itself: only the title or the text is ours.

use prism_core::controls::{DateConfig, PickerConfig, RadioConfig, SliderConfig};
use prism_core::{Color, FontWeight, NodeId, NodeKind, Style, TextAlign};
use prism_native::text::StyledText;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN,
};
use windows::Win32::Graphics::Direct2D::*;
use windows::core::Result;
use windows_numerics::Matrix3x2;

use crate::fluent::metrics::*;
use crate::fluent::{Palette, over};
use crate::gfx::Gfx;
use crate::paint::{Ctx, Part, rectf};
use crate::scene::{HEADER, Node, Rect, Scene, TAB_BAR};
use crate::text::{TextCache, plain};

/// Whether a button keeps the Fluent look: the app gave it no background,
/// gradient or border of its own.
pub fn native_button(s: &Style) -> bool {
    s.background.is_none()
        && s.gradient.is_none()
        && s.border_width <= 0.
        && !s.border_none
        && s.border_sides.is_empty()
}

/// What a control insets its content by: the app's padding when it set
/// one, else Fluent's (left, top, right, bottom).
pub fn content_inset(n: &Node) -> (f32, f32, f32, f32) {
    let p = n.style.padding;
    if n.style.has_padding() {
        return (p.left, p.top, p.right, p.bottom);
    }
    match n.kind {
        NodeKind::Button => BUTTON_PADDING,
        k if is_field(k) => {
            if n.style.own_field_look() {
                (0., 0., 0., 0.)
            } else {
                FIELD_PADDING
            }
        }
        _ => (0., 0., 0., 0.),
    }
}

pub fn is_field(k: NodeKind) -> bool {
    matches!(
        k,
        NodeKind::TextInput | NodeKind::TextArea | NodeKind::RichInput
    )
}

/// A text node's styled text.
pub fn styled(n: &Node) -> StyledText {
    StyledText::new(&n.text, &n.config, &n.style)
}

/// A control's title in its node's font.
fn title_text(n: &Node, text: &str, align: TextAlign) -> StyledText {
    let s = &n.style;
    let mut st = plain(text, s.font_size, s.font_weight, align);
    st.runs[0].font.family = s.font_family.clone();
    st.runs[0].font.italic = s.italic;
    st
}

fn label_size(gfx: &Gfx, texts: &TextCache, st: &StyledText, max: Option<f32>) -> (f32, f32) {
    texts.plain(gfx, st, max).map_or((0., 0.), |l| l.size())
}

/// The selected option of a picker, else its placeholder.
fn picker_text(n: &Node) -> String {
    let c = PickerConfig::decode(&n.config);
    let i = n.value;
    if i >= 0. && (i as usize) < c.options.len() {
        c.options[i as usize].clone()
    } else {
        n.placeholder.clone()
    }
}

fn date_text(n: &Node, locale: &prism_core::Locale) -> String {
    use prism_core::Formatter;
    let c = DateConfig::decode(&n.config);
    if n.value.is_nan() {
        return n.placeholder.clone();
    }
    let f = prism_core::BasicFormatter::default();
    match c.mode {
        prism_core::controls::DateMode::Date => {
            f.date(locale, n.value, prism_core::DateStyle::Medium)
        }
        prism_core::controls::DateMode::Time | prism_core::controls::DateMode::TimeOfDay => {
            f.time(locale, n.value)
        }
        prism_core::controls::DateMode::DateTime => format!(
            "{} {}",
            f.date(locale, n.value, prism_core::DateStyle::Medium),
            f.time(locale, n.value)
        ),
    }
}

/// Natural size of a leaf (`Widgets::measure`).
pub fn measure(
    gfx: &Gfx,
    texts: &mut TextCache,
    images: &crate::images::Images,
    scene: &Scene,
    id: NodeId,
    max_width: Option<f32>,
    locale: &prism_core::Locale,
) -> (f32, f32) {
    let Some(n) = scene.get(id) else {
        return (0., 0.);
    };
    let s = &n.style;
    match n.kind {
        NodeKind::Text | NodeKind::Timer => {
            let st = styled(n);
            if st.text().is_empty() {
                return (0., line_height(gfx, texts, s));
            }
            texts
                .get(gfx, id, &st, max_width)
                .map_or((0., 0.), |l| l.size())
        }
        NodeKind::Button => {
            let (l, t, r, b) = content_inset(n);
            let (w, h) = label_size(gfx, texts, &title_text(n, &n.text, TextAlign::Center), None);
            if s.has_padding() {
                (w, h)
            } else {
                (w + l + r, (h + t + b).max(CONTROL_HEIGHT))
            }
        }
        k if is_field(k) => {
            let (l, t, r, b) = content_inset(n);
            let lh = line_height(gfx, texts, s);
            let rows = prism_core::controls::InputConfig::decode(&n.config)
                .rows
                .unwrap_or(1)
                .max(1) as f32;
            let content_h = if k == NodeKind::TextInput {
                lh
            } else {
                // Grows with its text (then scrolls, capped by max_h).
                let w = max_width.map(|w| (w - l - r).max(10.));
                let mut st = title_text(n, &n.text, TextAlign::Left);
                st.no_wrap = false;
                let text_h = if n.text.is_empty() {
                    lh
                } else {
                    texts.plain(gfx, &st, w).map_or(lh, |laid| laid.size().1)
                };
                text_h.max(lh * rows)
            };
            let own = s.has_padding() || s.own_field_look();
            let w = max_width.map_or(200., |m| m.min(200.));
            if own {
                (w, content_h)
            } else {
                (w + l + r, (content_h + t + b).max(CONTROL_HEIGHT))
            }
        }
        NodeKind::Toggle => TOGGLE,
        NodeKind::Checkbox => {
            if n.text.is_empty() {
                return (CHECKBOX, CHECKBOX);
            }
            let (w, h) = label_size(gfx, texts, &title_text(n, &n.text, TextAlign::Left), None);
            (CHECKBOX + 8. + w, h.max(CHECKBOX))
        }
        NodeKind::RadioGroup => {
            let c = RadioConfig::decode(&n.config);
            let sizes: Vec<(f32, f32)> = c
                .options
                .iter()
                .map(|o| label_size(gfx, texts, &title_text(n, o, TextAlign::Left), None))
                .collect();
            let item = |(w, _h): &(f32, f32)| CHECKBOX + 8. + w;
            if c.horizontal {
                let w: f32 = sizes.iter().map(item).sum::<f32>()
                    + 16. * sizes.len().saturating_sub(1) as f32;
                (w, CONTROL_HEIGHT)
            } else {
                let w = sizes.iter().map(item).fold(0., f32::max);
                (w, CONTROL_HEIGHT * sizes.len() as f32)
            }
        }
        NodeKind::Slider => (max_width.map_or(200., |m| m.min(200.)), CONTROL_HEIGHT),
        NodeKind::Progress => (max_width.map_or(200., |m| m.min(200.)), 4.),
        NodeKind::Spinner => {
            let d = if n.value == 1. { SPINNER } else { 20. };
            (d, d)
        }
        NodeKind::Picker | NodeKind::DatePicker => {
            let text = if n.kind == NodeKind::Picker {
                // As wide as its widest option, so choosing does not move it.
                let c = PickerConfig::decode(&n.config);
                c.options
                    .iter()
                    .chain(std::iter::once(&n.placeholder))
                    .max_by_key(|o| o.chars().count())
                    .cloned()
                    .unwrap_or_default()
            } else {
                date_text(n, locale)
            };
            let (w, h) = label_size(gfx, texts, &title_text(n, &text, TextAlign::Left), None);
            (w + 11. + 38., (h + 11.).max(CONTROL_HEIGHT))
        }
        NodeKind::Image => {
            let d = s.font_size.max(1.) * 1.2;
            (d, d)
        }
        NodeKind::Picture => images.size_of(&n.text).unwrap_or((0., 0.)),
        _ => (0., 0.),
    }
}

/// The CSS-less natural line height of a node's font.
fn line_height(gfx: &Gfx, texts: &TextCache, s: &Style) -> f32 {
    if let Some(lh) = s.line_height_points() {
        return lh;
    }
    let mut st = plain("Ag", s.font_size, s.font_weight, TextAlign::Left);
    st.runs[0].font.family = s.font_family.clone();
    label_size(gfx, texts, &st, None).1
}

/// Distance from the top of a measured leaf to its first baseline.
pub fn baseline(
    gfx: &Gfx,
    texts: &mut TextCache,
    scene: &Scene,
    id: NodeId,
    width: f32,
    height: f32,
) -> Option<f32> {
    let n = scene.get(id)?;
    match n.kind {
        NodeKind::Text | NodeKind::Timer => {
            let st = styled(n);
            texts.get(gfx, id, &st, Some(width))?.baseline()
        }
        NodeKind::Button | NodeKind::Picker | NodeKind::DatePicker | NodeKind::Checkbox => {
            let text = match n.kind {
                NodeKind::Picker => picker_text(n),
                _ => n.text.clone(),
            };
            let laid = texts.plain(gfx, &title_text(n, &text, TextAlign::Left), None)?;
            let (_, h) = laid.size();
            Some((height - h) / 2. + laid.baseline()?)
        }
        k if is_field(k) => {
            let (_, t, _, _) = content_inset(n);
            let laid = texts.plain(gfx, &title_text(n, "Ag", TextAlign::Left), None)?;
            let (_, h) = laid.size();
            let top = if k == NodeKind::TextInput {
                (height - h) / 2.
            } else {
                t
            };
            Some(top + laid.baseline()?)
        }
        _ => None,
    }
}

// --- Layouts of the bars ------------------------------------------------------

/// Where a stack header's parts are (the stack's coordinates).
pub struct Header {
    pub back: Option<Rect>,
    pub closes: bool,
    pub title: Rect,
    /// Bar buttons (#30): (index in the screen's items, where, face).
    pub items: Vec<(usize, Rect, String, Option<char>)>,
}

fn bar_face(item: &prism_core::navbar::BarItem) -> (String, Option<char>) {
    match crate::keys::fluent_icon(&item.icon) {
        Some(c) if !item.icon.is_empty() => (String::new(), Some(c)),
        _ if !item.title.is_empty() => (item.title.clone(), None),
        _ => (prism_core::glyphs::glyph(&item.icon).to_string(), None),
    }
}

pub fn header(gfx: &Gfx, texts: &TextCache, scene: &Scene, stack: NodeId) -> Header {
    let width = scene.get(stack).map_or(0., |n| n.frame.w);
    let pages = scene.pages(stack);
    let top = pages.last().copied();
    let closes = pages.len() <= 1;
    let back = scene.has_back(stack).then(|| Rect::new(4., 6., 40., 36.));
    let config = top
        .and_then(|t| scene.get(t))
        .map(|s| prism_core::navbar::ScreenConfig::decode(&s.config))
        .unwrap_or_default();
    let mut items = Vec::new();
    let mut x = back.map_or(8., |b| b.right() + 4.);
    let item_w = |text: &str, icon: Option<char>| -> f32 {
        if icon.is_some() {
            40.
        } else {
            label_size(
                gfx,
                texts,
                &plain(text, 14., FontWeight::Regular, TextAlign::Center),
                None,
            )
            .0 + 24.
        }
    };
    for (i, item) in config.leading() {
        let (text, icon) = bar_face(item);
        let w = item_w(&text, icon);
        items.push((i, Rect::new(x, 6., w, 36.), text, icon));
        x += w + 4.;
    }
    let title_x = if back.is_some() || x > 8. {
        x + 8.
    } else {
        16.
    };
    let mut right = width - 8.;
    for (i, item) in config.trailing().collect::<Vec<_>>().into_iter().rev() {
        let (text, icon) = bar_face(item);
        let w = item_w(&text, icon);
        right -= w;
        items.push((i, Rect::new(right, 6., w, 36.), text, icon));
        right -= 4.;
    }
    Header {
        back,
        closes: closes && back.is_some(),
        title: Rect::new(title_x, 0., (right - title_x).max(0.), HEADER),
        items,
    }
}

/// The tab bar's items (the tab bar's coordinates).
pub fn tab_items(gfx: &Gfx, texts: &TextCache, scene: &Scene, tabs: NodeId) -> Vec<Rect> {
    let mut x = 8.;
    let mut out = Vec::new();
    for t in scene.pages(tabs) {
        let Some(tab) = scene.get(t) else {
            out.push(Rect::default());
            continue;
        };
        let (w, _) = label_size(
            gfx,
            texts,
            &plain(&tab.text, 14., FontWeight::Regular, TextAlign::Left),
            None,
        );
        let icon = if tab.icon.is_empty() { 0. } else { 16. + 8. };
        let width = 12. + icon + w + 12.;
        out.push(Rect::new(x, 4., width, TAB_BAR - 8.));
        x += width + 4.;
    }
    out
}

/// Which part of a stack's header or a tab bar is at a point (the node's
/// coordinates).
pub fn part_at(
    gfx: &Gfx,
    texts: &TextCache,
    scene: &Scene,
    id: NodeId,
    x: f32,
    y: f32,
) -> Option<Part> {
    let n = scene.get(id)?;
    match n.kind {
        NodeKind::Tabs if y < TAB_BAR => tab_items(gfx, texts, scene, id)
            .iter()
            .position(|r| r.contains(x, y))
            .map(Part::Tab),
        NodeKind::Stack if y < HEADER && scene.header_shown(id) => {
            let h = header(gfx, texts, scene, id);
            if h.back.is_some_and(|b| b.contains(x, y)) {
                return Some(Part::Back);
            }
            h.items
                .iter()
                .find(|(_, r, ..)| r.contains(x, y))
                .map(|(i, ..)| Part::BarItem(*i))
        }
        _ => None,
    }
}

// --- Drawing ----------------------------------------------------------------

/// The opaque color under a node: the nearest opaque background around
/// it, else the window's. Text fields paint their native editor with it.
pub fn backdrop_color(scene: &Scene, pal: &Palette, id: NodeId) -> Color {
    let mut layers = Vec::new();
    for a in std::iter::once(id).chain(scene.ancestors(id)) {
        if let Some(bg) = scene.get(a).and_then(|n| n.style.background) {
            let bg = pal.resolve(bg);
            layers.push(bg);
            if bg.a >= 1. {
                break;
            }
        }
    }
    layers
        .into_iter()
        .rev()
        .fold(pal.window, |acc, c| over(c, acc))
}

fn hovered(ctx: &Ctx, id: NodeId, part: Part) -> bool {
    ctx.hover == Some((id, part))
}

fn pressed(ctx: &Ctx, id: NodeId, part: Part) -> bool {
    ctx.pressed == Some((id, part))
}

/// Draws a node's own content (after its box, before its children).
pub fn draw_content(ctx: &mut Ctx, id: NodeId, r: Rect, m: &Matrix3x2) -> Result<()> {
    let scene = ctx.scene;
    let n = scene.get(id).expect("node");
    let s = &n.style;
    let disabled = n.a11y.disabled;
    match n.kind {
        NodeKind::Text | NodeKind::Timer => {
            let p = s.padding;
            let st = styled(n);
            let w = (r.w - p.left - p.right).max(0.);
            if let Some(laid) = ctx.texts.get(ctx.gfx, id, &st, Some(w)) {
                let pal = *ctx.pal;
                let color = ctx.text_color(s);
                laid.draw(
                    ctx.dc,
                    p.left,
                    p.top,
                    w,
                    color,
                    &|c| pal.resolve(c),
                    st.no_wrap || st.max_lines.is_some(),
                )?;
            }
        }
        NodeKind::Button => draw_button(ctx, n, id, r)?,
        k if is_field(k) => draw_field(ctx, n, r),
        NodeKind::Toggle => draw_toggle(ctx, n, r),
        NodeKind::Checkbox => draw_checkbox(ctx, n, r)?,
        NodeKind::RadioGroup => draw_radios(ctx, n, r)?,
        NodeKind::Slider => draw_slider(ctx, n, r),
        NodeKind::Progress => {
            let y = (r.h - PROGRESS) / 2.;
            ctx.fill_rounded(
                Rect::new(0., r.h / 2. - 0.5, r.w, 1.),
                0.5,
                ctx.pal.strong_stroke,
            );
            let v = n.value.clamp(0., 1.) as f32;
            if v > 0. {
                ctx.fill_rounded(
                    Rect::new(0., y, r.w * v, PROGRESS),
                    PROGRESS / 2.,
                    accent(ctx, s),
                );
            }
        }
        NodeKind::Spinner => draw_ring(ctx, s, r)?,
        NodeKind::Picker | NodeKind::DatePicker => {
            draw_chrome(ctx, n, id, r, disabled);
            let text = if n.kind == NodeKind::Picker {
                picker_text(n)
            } else {
                date_text(n, ctx.locale)
            };
            let empty = (n.kind == NodeKind::Picker && n.value < 0.) || n.value.is_nan();
            let color = if disabled {
                ctx.pal.text_disabled
            } else if empty {
                ctx.pal.text_secondary
            } else {
                ctx.text_color(s)
            };
            ctx.label(
                &text,
                Rect::new(11., 0., (r.w - 11. - 38.).max(0.), r.h),
                s.font_size,
                s.font_weight,
                TextAlign::Left,
                color,
            )?;
            let glyph = if n.kind == NodeKind::Picker {
                '\u{E70D}'
            } else {
                '\u{E787}'
            };
            let c = ctx.pal.text_secondary;
            ctx.icon(glyph, Rect::new(r.w - 38., 0., 38., r.h), 12., c)?;
        }
        NodeKind::Image => {
            let color = s
                .text_color
                .or(s.tint)
                .map_or(ctx.pal.text, |c| ctx.resolve(c));
            let size = s.font_size.max(1.);
            match crate::keys::fluent_icon(&n.icon) {
                Some(g) => ctx.icon(g, r, size, color)?,
                None => {
                    let g = prism_core::glyphs::glyph(&n.icon);
                    ctx.label(g, r, size, FontWeight::Regular, TextAlign::Center, color)?;
                }
            }
        }
        NodeKind::Picture => draw_picture(ctx, n, id, r)?,
        NodeKind::Shape => draw_shape(ctx, n)?,
        NodeKind::Stack if scene.header_shown(id) => draw_header(ctx, id, r)?,
        NodeKind::Tabs => draw_tab_bar(ctx, id, r)?,
        NodeKind::Scene3d | NodeKind::WebView | NodeKind::Video => {
            // Phase 3: D3D11 scenes, WebView2, Media Foundation.
            ctx.fill_rect(r, ctx.pal.control_disabled);
            let what = match n.kind {
                NodeKind::Scene3d => "3D scene",
                NodeKind::WebView => "Web view",
                _ => "Video",
            };
            let c = ctx.pal.text_tertiary;
            ctx.label(what, r, 12., FontWeight::Regular, TextAlign::Center, c)?;
        }
        _ => {}
    }
    let _ = m;
    Ok(())
}

/// Drawn above the children: scroll bars, the keyboard focus ring.
pub fn draw_over(ctx: &mut Ctx, id: NodeId, r: Rect) -> Result<()> {
    let scene = ctx.scene;
    let n = scene.get(id).expect("node");
    if n.kind == NodeKind::Scroll && n.style.scrollbar != prism_core::Scrollbar::Hidden {
        draw_scrollbar(ctx, n, id, r);
    }
    if n.focused && ctx.focus_visible && !is_field(n.kind) {
        let radius = n.style.corner_radius.max(CONTROL_RADIUS);
        ctx.stroke_rounded(r.inflate(3.), radius + 3., 2., ctx.pal.focus_outer);
        ctx.stroke_rounded(r.inflate(1.), radius + 1., 1., ctx.pal.focus_inner);
    }
    Ok(())
}

fn accent(ctx: &Ctx, s: &Style) -> Color {
    s.tint.map_or(ctx.pal.accent, |c| ctx.resolve(c))
}

/// A standard button's (or combo box's) body: fill, hairline border,
/// darker bottom edge.
fn draw_chrome(ctx: &Ctx, n: &Node, _id: NodeId, r: Rect, disabled: bool) {
    let pal = ctx.pal;
    let fill = if disabled {
        pal.control_disabled
    } else if n.pressed {
        pal.control_pressed
    } else if n.hover {
        pal.control_hover
    } else {
        pal.control
    };
    let radius = n.style.corner_radius.max(CONTROL_RADIUS);
    ctx.fill_rounded(r, radius, fill);
    ctx.stroke_rounded(r, radius, 1., pal.stroke);
    if !n.pressed && !disabled {
        // The elevation edge at the bottom.
        let g = ctx.box_geometry(r, radius, prism_core::Corners::ALL);
        if let Ok(g) = g {
            ctx.push_layer(Some(&g), 1.);
            ctx.fill_rect(
                Rect::new(r.x, r.bottom() - 1., r.w, 1.),
                pal.stroke_secondary,
            );
            ctx.pop_layer();
        }
    }
}

fn draw_button(ctx: &mut Ctx, n: &Node, id: NodeId, r: Rect) -> Result<()> {
    let s = &n.style;
    let disabled = n.a11y.disabled;
    let native = native_button(s);
    if native {
        draw_chrome(ctx, n, id, r, disabled);
    }
    let (l, t, rr, b) = content_inset(n);
    let color = if disabled {
        ctx.pal.text_disabled
    } else if native && s.text_color.is_none() {
        ctx.pal.text
    } else {
        ctx.text_color(s)
    };
    let st = title_text(n, &n.text, TextAlign::Center);
    let w = (r.w - l - rr).max(1.);
    if let Some(laid) = ctx.texts.plain(ctx.gfx, &st, Some(w)) {
        let (_, h) = laid.size();
        let y = t + ((r.h - t - b) - h) / 2.;
        let pal = *ctx.pal;
        laid.draw(ctx.dc, l, y, w, color, &|c| pal.resolve(c), true)?;
    }
    Ok(())
}

/// A text box: the field's chrome; the native editor draws the text and
/// the placeholder over it.
fn draw_field(ctx: &Ctx, n: &Node, r: Rect) {
    if n.style.own_field_look() {
        return;
    }
    let pal = ctx.pal;
    let radius = n.style.corner_radius.max(CONTROL_RADIUS);
    let fill = if n.focused {
        pal.input_active
    } else if n.hover {
        pal.control_hover
    } else {
        pal.control
    };
    ctx.fill_rounded(r, radius, fill);
    ctx.stroke_rounded(r, radius, 1., pal.stroke);
    // The underline: strong at rest, the accent (2 px) while editing.
    if let Ok(g) = ctx.box_geometry(r, radius, prism_core::Corners::ALL) {
        ctx.push_layer(Some(&g), 1.);
        if n.focused {
            ctx.fill_rect(
                Rect::new(r.x, r.bottom() - 2., r.w, 2.),
                accent(ctx, &n.style),
            );
        } else {
            ctx.fill_rect(Rect::new(r.x, r.bottom() - 1., r.w, 1.), pal.strong_stroke);
        }
        ctx.pop_layer();
    }
}

fn draw_toggle(ctx: &Ctx, n: &Node, r: Rect) {
    let pal = ctx.pal;
    let (w, h) = TOGGLE;
    let x = 0.;
    let y = (r.h - h) / 2.;
    let on = n.value >= 0.5;
    let track = Rect::new(x, y, w, h);
    let knob = if n.pressed || n.hover { 14. } else { 12. };
    if on {
        let c = if n.a11y.disabled {
            pal.text_disabled
        } else {
            accent(ctx, &n.style)
        };
        ctx.fill_rounded(track, h / 2., c);
        ctx.fill_ellipse(
            x + w - h / 2.,
            y + h / 2.,
            knob / 2.,
            knob / 2.,
            pal.on_accent,
        );
    } else {
        ctx.fill_rounded(track, h / 2., pal.alt_fill);
        ctx.stroke_rounded(track, h / 2., 1., pal.strong_stroke);
        ctx.fill_ellipse(
            x + h / 2.,
            y + h / 2.,
            knob / 2.,
            knob / 2.,
            pal.strong_fill,
        );
    }
}

fn draw_check_box(ctx: &Ctx, n: &Node, at: Rect, on: bool) -> Result<()> {
    let pal = ctx.pal;
    if on {
        let c = if n.a11y.disabled {
            pal.text_disabled
        } else {
            accent(ctx, &n.style)
        };
        ctx.fill_rounded(at, CONTROL_RADIUS, c);
    } else {
        let fill = if n.hover {
            pal.control_hover
        } else {
            pal.alt_fill
        };
        ctx.fill_rounded(at, CONTROL_RADIUS, fill);
        ctx.stroke_rounded(at, CONTROL_RADIUS, 1., pal.strong_stroke);
    }
    Ok(())
}

fn draw_checkbox(ctx: &mut Ctx, n: &Node, r: Rect) -> Result<()> {
    let at = Rect::new(0., (r.h - CHECKBOX) / 2., CHECKBOX, CHECKBOX);
    let on = n.value >= 0.5;
    draw_check_box(ctx, n, at, on)?;
    if on {
        let c = ctx.pal.on_accent;
        ctx.icon('\u{E73E}', at, 12., c)?;
    }
    if !n.text.is_empty() {
        let color = if n.a11y.disabled {
            ctx.pal.text_disabled
        } else {
            ctx.text_color(&n.style)
        };
        let s = &n.style;
        ctx.label(
            &n.text,
            Rect::new(CHECKBOX + 8., 0., (r.w - CHECKBOX - 8.).max(0.), r.h),
            s.font_size,
            s.font_weight,
            TextAlign::Left,
            color,
        )?;
    }
    Ok(())
}

fn draw_radios(ctx: &mut Ctx, n: &Node, r: Rect) -> Result<()> {
    let c = RadioConfig::decode(&n.config);
    let selected = n.value;
    let s = &n.style;
    let mut x = 0.;
    let mut y = 0.;
    for (i, o) in c.options.iter().enumerate() {
        let on = selected >= 0. && selected as usize == i;
        let cy = y + CONTROL_HEIGHT / 2.;
        let rad = CHECKBOX / 2.;
        if on {
            ctx.fill_ellipse(x + rad, cy, rad, rad, accent(ctx, s));
            ctx.fill_ellipse(x + rad, cy, 4., 4., ctx.pal.on_accent);
        } else {
            ctx.fill_ellipse(x + rad, cy, rad, rad, ctx.pal.alt_fill);
            ctx.stroke_ellipse(x + rad, cy, rad - 0.5, 1., ctx.pal.strong_stroke);
        }
        let color = if n.a11y.disabled {
            ctx.pal.text_disabled
        } else {
            ctx.text_color(s)
        };
        let (w, _) = label_size(ctx.gfx, ctx.texts, &title_text(n, o, TextAlign::Left), None);
        ctx.label(
            o,
            Rect::new(x + CHECKBOX + 8., y, w + 1., CONTROL_HEIGHT),
            s.font_size,
            s.font_weight,
            TextAlign::Left,
            color,
        )?;
        if c.horizontal {
            x += CHECKBOX + 8. + w + 16.;
        } else {
            y += CONTROL_HEIGHT;
        }
    }
    let _ = r;
    Ok(())
}

/// Where a slider's value puts its thumb's center.
pub fn slider_x(n: &Node, width: f32) -> f32 {
    let c = SliderConfig::decode(&n.config);
    let t = if c.max > c.min {
        ((n.value - c.min) / (c.max - c.min)).clamp(0., 1.) as f32
    } else {
        0.
    };
    SLIDER_THUMB / 2. + t * (width - SLIDER_THUMB).max(0.)
}

fn draw_slider(ctx: &Ctx, n: &Node, r: Rect) {
    let pal = ctx.pal;
    let cy = r.h / 2.;
    let x = slider_x(n, r.w);
    let rail = Rect::new(
        SLIDER_THUMB / 2.,
        cy - SLIDER_RAIL / 2.,
        (r.w - SLIDER_THUMB).max(0.),
        SLIDER_RAIL,
    );
    ctx.fill_rounded(rail, SLIDER_RAIL / 2., pal.strong_fill);
    let a = if n.a11y.disabled {
        pal.text_disabled
    } else {
        accent(ctx, &n.style)
    };
    ctx.fill_rounded(
        Rect::new(rail.x, rail.y, x - rail.x, SLIDER_RAIL),
        SLIDER_RAIL / 2.,
        a,
    );
    let thumb = if pal.dark {
        prism_core::rgb(0x454545)
    } else {
        Color::WHITE
    };
    ctx.fill_ellipse(x, cy, SLIDER_THUMB / 2., SLIDER_THUMB / 2., thumb);
    ctx.stroke_ellipse(x, cy, SLIDER_THUMB / 2. - 0.5, 1., pal.stroke_secondary);
    let inner = if n.pressed {
        5.
    } else if n.hover {
        7.
    } else {
        6.
    };
    ctx.fill_ellipse(x, cy, inner, inner, a);
}

/// An indeterminate progress ring: a 3/4 arc in the accent color.
fn draw_ring(ctx: &Ctx, s: &Style, r: Rect) -> Result<()> {
    let d = r.w.min(r.h);
    let stroke = (d / 8.).clamp(2., 4.);
    let rad = (d - stroke) / 2.;
    let (cx, cy) = (r.w / 2., r.h / 2.);
    unsafe {
        let g = ctx.gfx.d2d.CreatePathGeometry()?;
        let sink = g.Open()?;
        sink.BeginFigure(
            windows_numerics::Vector2 { X: cx, Y: cy - rad },
            D2D1_FIGURE_BEGIN_HOLLOW,
        );
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: windows_numerics::Vector2 { X: cx - rad, Y: cy },
            size: D2D_SIZE_F {
                width: rad,
                height: rad,
            },
            rotationAngle: 0.,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_LARGE,
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        let factory: &ID2D1Factory = &ctx.gfx.d2d;
        let style = factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                ..Default::default()
            },
            None,
        )?;
        ctx.dc
            .DrawGeometry(&g, ctx.solid(accent(ctx, s)), stroke, &style);
    }
    Ok(())
}

fn draw_picture(ctx: &mut Ctx, n: &Node, id: NodeId, r: Rect) -> Result<()> {
    let Some((generation, wic)) = ctx.images.bitmap(&n.text) else {
        return Ok(());
    };
    let bmp = match ctx.cache.bitmaps.get(&id) {
        Some((g, b)) if *g == generation => b.clone(),
        _ => {
            let b = unsafe { ctx.dc.CreateBitmapFromWicBitmap(&wic, None)? };
            ctx.cache.bitmaps.insert(id, (generation, b.clone()));
            b
        }
    };
    let (iw, ih) = ctx.images.size_of(&n.text).unwrap_or((1., 1.));
    if iw <= 0. || ih <= 0. || r.w <= 0. || r.h <= 0. {
        return Ok(());
    }
    // 1: fit (contain), 0: fill (cover), centered.
    let k = if n.value == 1. {
        (r.w / iw).min(r.h / ih)
    } else {
        (r.w / iw).max(r.h / ih)
    };
    let (w, h) = (iw * k, ih * k);
    let dest = rectf((r.w - w) / 2., (r.h - h) / 2., w, h);
    let s = &n.style;
    let g = ctx.box_geometry(r, s.corner_radius, s.rounded_corners)?;
    ctx.push_layer(Some(&g), 1.);
    unsafe {
        ctx.dc.DrawBitmap(
            &bmp,
            Some(&dest),
            1.,
            D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
            None,
            None,
        );
    }
    ctx.pop_layer();
    Ok(())
}

fn draw_shape(ctx: &Ctx, n: &Node) -> Result<()> {
    for p in &n.paths {
        let g = ctx.path_geometry(&p.segments, p.even_odd)?;
        unsafe {
            if let Some(fill) = p.fill {
                ctx.dc.FillGeometry(&g, ctx.solid(fill), None);
            }
            if let Some(stroke) = p.stroke {
                let cap = if p.round {
                    D2D1_CAP_STYLE_ROUND
                } else {
                    D2D1_CAP_STYLE_FLAT
                };
                let w = p.width.max(0.01);
                let dashes = p.dash.map(|(on, off)| [on.max(0.001) / w, off / w]);
                let factory: &ID2D1Factory = &ctx.gfx.d2d;
                let style = factory.CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: cap,
                        endCap: cap,
                        dashCap: cap,
                        lineJoin: if p.round {
                            D2D1_LINE_JOIN_ROUND
                        } else {
                            D2D1_LINE_JOIN_MITER
                        },
                        miterLimit: 10.,
                        dashStyle: if dashes.is_some() {
                            D2D1_DASH_STYLE_CUSTOM
                        } else {
                            D2D1_DASH_STYLE_SOLID
                        },
                        dashOffset: 0.,
                    },
                    dashes.as_ref().map(|d| &d[..]),
                )?;
                ctx.dc.DrawGeometry(&g, ctx.solid(stroke), w, &style);
            }
        }
    }
    Ok(())
}

fn draw_header(ctx: &mut Ctx, id: NodeId, r: Rect) -> Result<()> {
    let scene = ctx.scene;
    let pal = *ctx.pal;
    let n = scene.get(id).expect("stack");
    let bar = Rect::new(0., 0., r.w, HEADER);
    ctx.fill_rect(bar, pal.window);
    ctx.fill_rect(Rect::new(0., HEADER - 1., r.w, 1.), pal.divider);
    let h = header(ctx.gfx, ctx.texts, scene, id);
    let tint = n.style.tint.map(|c| pal.resolve(c));
    let subtle = |ctx: &Ctx, part: Part, at: Rect| {
        if pressed(ctx, id, part) {
            ctx.fill_rounded(at, CONTROL_RADIUS, pal.subtle_pressed);
        } else if hovered(ctx, id, part) {
            ctx.fill_rounded(at, CONTROL_RADIUS, pal.subtle_hover);
        }
    };
    if let Some(b) = h.back {
        subtle(ctx, Part::Back, b);
        let glyph = if h.closes { '\u{E711}' } else { '\u{E72B}' };
        ctx.icon(glyph, b, 16., tint.unwrap_or(pal.text))?;
    }
    for (i, at, text, icon) in &h.items {
        subtle(ctx, Part::BarItem(*i), *at);
        let c = tint.unwrap_or(pal.text);
        match icon {
            Some(g) => ctx.icon(*g, *at, 16., c)?,
            None => ctx.label(text, *at, 14., FontWeight::Regular, TextAlign::Center, c)?,
        }
    }
    let title = scene
        .pages(id)
        .last()
        .and_then(|t| scene.get(*t))
        .map(|s| s.text.clone())
        .unwrap_or_default();
    ctx.label(
        &title,
        h.title,
        18.,
        FontWeight::Semibold,
        TextAlign::Left,
        pal.text,
    )?;
    Ok(())
}

fn draw_tab_bar(ctx: &mut Ctx, id: NodeId, r: Rect) -> Result<()> {
    let scene = ctx.scene;
    let pal = *ctx.pal;
    let n = scene.get(id).expect("tabs");
    ctx.fill_rect(Rect::new(0., 0., r.w, TAB_BAR), pal.window);
    ctx.fill_rect(Rect::new(0., TAB_BAR - 1., r.w, 1.), pal.divider);
    let items = tab_items(ctx.gfx, ctx.texts, scene, id);
    let selected = n.selected();
    let tint = n.style.tint.map_or(pal.accent, |c| pal.resolve(c));
    for (i, (t, at)) in scene.pages(id).into_iter().zip(&items).enumerate() {
        let Some(tab) = scene.get(t) else { continue };
        if pressed(ctx, id, Part::Tab(i)) {
            ctx.fill_rounded(*at, CONTROL_RADIUS, pal.subtle_pressed);
        } else if hovered(ctx, id, Part::Tab(i)) {
            ctx.fill_rounded(*at, CONTROL_RADIUS, pal.subtle_hover);
        }
        let on = i == selected;
        let color = if on { pal.text } else { pal.text_secondary };
        let mut x = at.x + 12.;
        if !tab.icon.is_empty() {
            let r = Rect::new(x, at.y, 16., at.h);
            match crate::keys::fluent_icon(&tab.icon) {
                Some(g) => ctx.icon(g, r, 16., if on { tint } else { color })?,
                None => ctx.label(
                    prism_core::glyphs::glyph(&tab.icon),
                    r,
                    14.,
                    FontWeight::Regular,
                    TextAlign::Center,
                    color,
                )?,
            }
            x += 24.;
        }
        let weight = if on {
            FontWeight::Semibold
        } else {
            FontWeight::Regular
        };
        ctx.label(
            &tab.text,
            Rect::new(x, at.y, (at.right() - x).max(0.), at.h),
            14.,
            weight,
            TextAlign::Left,
            color,
        )?;
        if on {
            // The selection pill under the item.
            let pill = Rect::new(at.x + at.w / 2. - 8., TAB_BAR - 5., 16., 3.);
            ctx.fill_rounded(pill, 1.5, tint);
        }
    }
    Ok(())
}

/// Thin overlay scroll bars (wider under the pointer).
fn draw_scrollbar(ctx: &Ctx, n: &Node, id: NodeId, r: Rect) {
    let vertical = n.style.scroll_axis == prism_core::ScrollAxis::Vertical;
    let (view, content, offset) = if vertical {
        (r.h, n.content.1, n.scroll.1)
    } else {
        (r.w, n.content.0, n.scroll.0)
    };
    if content <= view + 0.5 || view <= 0. {
        return;
    }
    let active = hovered(ctx, id, Part::ScrollBar) || pressed(ctx, id, Part::ScrollBar);
    let thick = if active { SCROLLBAR } else { SCROLLBAR_THIN };
    let track = view - 2. * SCROLLBAR_INSET;
    let thumb = (track * view / content).max(24.).min(track);
    let max = content - view;
    let pos = SCROLLBAR_INSET + (track - thumb) * (offset / max).clamp(0., 1.);
    let c = ctx.pal.scrollbar;
    let edge = SCROLLBAR_INSET + thick;
    let at = if vertical {
        Rect::new(r.w - edge, pos, thick, thumb)
    } else {
        Rect::new(pos, r.h - edge, thumb, thick)
    };
    ctx.fill_rounded(at, thick / 2., c);
}

/// The scroll bar's thumb area (for dragging and hover), in the scroll
/// view's coordinates.
pub fn scrollbar_hit(n: &Node, x: f32, y: f32) -> bool {
    let vertical = n.style.scroll_axis == prism_core::ScrollAxis::Vertical;
    let overflows = if vertical {
        n.content.1 > n.frame.h + 0.5
    } else {
        n.content.0 > n.frame.w + 0.5
    };
    let reach = SCROLLBAR + 2. * SCROLLBAR_INSET + 2.;
    overflows
        && n.style.scrollbar != prism_core::Scrollbar::Hidden
        && if vertical {
            x >= n.frame.w - reach && x <= n.frame.w
        } else {
            y >= n.frame.h - reach && y <= n.frame.h
        }
}
