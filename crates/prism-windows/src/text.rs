//! DirectWrite text layouts of prism-native's `StyledText`: fonts and
//! weights per run, CSS line heights, letter spacing, tabular figures,
//! wrapping, line clamps with an ellipsis, alignment, inline icons (Segoe
//! Fluent Icons), underline and strikethrough. Emoji come from the system
//! font fallback and draw in color (`D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT`).
//!
//! Layouts are device independent: cached per node and width; colors
//! (device-dependent brushes) are set right before drawing.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{FontFamily, NodeId, TextAlign};
use prism_native::text::{StyledRun, StyledText};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, ID2D1DeviceContext,
};
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{BOOL, Interface, PCWSTR, Result};
use windows_numerics::Vector2;

use crate::gfx::{Gfx, color_f};
use crate::win::wide;

/// A run's text as laid out: its icon's Fluent glyph when it has one.
struct Piece {
    start: u32,
    len: u32,
    icon: bool,
}

/// A text laid out, with what it was made from.
pub struct Laid {
    pub layout: IDWriteTextLayout,
    pub text: StyledText,
    pieces: Vec<Piece>,
}

/// The font metrics a CSS line box needs, per family, weight, style.
#[derive(Clone, Copy)]
struct FontMetrics {
    ascent: f32,
    descent: f32,
}

#[derive(Default)]
pub struct TextCache {
    /// A node's layouts by width limit (a few: taffy measures under
    /// several), dropped when its text or style changes.
    nodes: HashMap<NodeId, Vec<Layout>>,
    metrics: MetricsCache,
}

/// A layout by its width limit (quarter DIPs).
type Layout = (Option<u32>, std::rc::Rc<Laid>);

/// Font metrics by family, weight and italic.
type MetricsCache = RefCell<HashMap<(String, u16, bool), FontMetrics>>;

impl TextCache {
    pub fn forget(&mut self, id: NodeId) {
        self.nodes.remove(&id);
    }

    /// The layout of node `id`'s `text` at `max_width` (`None`: one line
    /// per paragraph unless it wraps itself).
    pub fn get(
        &mut self,
        gfx: &Gfx,
        id: NodeId,
        text: &StyledText,
        max_width: Option<f32>,
    ) -> Option<std::rc::Rc<Laid>> {
        let key = max_width.map(|w| (w * 4.).round() as u32);
        let entries = self.nodes.entry(id).or_default();
        if entries.first().is_some_and(|(_, l)| l.text != *text) {
            entries.clear();
        }
        if let Some((_, l)) = entries.iter().find(|(k, _)| *k == key) {
            return Some(l.clone());
        }
        let laid = std::rc::Rc::new(build(gfx, &self.metrics, text, max_width).ok()?);
        if entries.len() >= 4 {
            entries.remove(0);
        }
        entries.push((key, laid.clone()));
        Some(laid)
    }

    /// A one-off layout (header titles, tab labels, placeholders).
    pub fn plain(&self, gfx: &Gfx, text: &StyledText, max_width: Option<f32>) -> Option<Laid> {
        build(gfx, &self.metrics, text, max_width).ok()
    }
}

/// The family DirectWrite gets for a Prism family.
pub fn family_name(gfx: &Gfx, family: &FontFamily, size: f32) -> String {
    match prism_native::fonts::resolve(family) {
        FontFamily::System | FontFamily::Rounded => {
            if size >= 20. {
                gfx.fonts.display.clone()
            } else {
                gfx.fonts.text.clone()
            }
        }
        FontFamily::Monospace => {
            first_installed(gfx, &["Cascadia Mono", "Consolas"], "Courier New")
        }
        FontFamily::Serif => first_installed(gfx, &["Georgia", "Cambria"], "Times New Roman"),
        FontFamily::Named(name) => name,
    }
}

fn first_installed(gfx: &Gfx, names: &[&str], fallback: &str) -> String {
    for n in names {
        let (mut index, mut exists) = (0u32, BOOL(0));
        let w = wide(n);
        let found = unsafe {
            gfx.system_fonts
                .FindFamilyName(PCWSTR(w.as_ptr()), &mut index, &mut exists)
                .is_ok()
        };
        if found && exists.as_bool() {
            return n.to_string();
        }
    }
    fallback.to_string()
}

fn weight(w: prism_core::FontWeight) -> DWRITE_FONT_WEIGHT {
    DWRITE_FONT_WEIGHT(w.css() as i32)
}

fn style(italic: bool) -> DWRITE_FONT_STYLE {
    if italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    }
}

fn font_metrics(
    gfx: &Gfx,
    cache: &MetricsCache,
    family: &str,
    w: prism_core::FontWeight,
    italic: bool,
) -> FontMetrics {
    let key = (family.to_string(), w.css(), italic);
    if let Some(m) = cache.borrow().get(&key) {
        return *m;
    }
    // Segoe UI's: what most fallbacks look like.
    let mut m = FontMetrics {
        ascent: 1.079,
        descent: 0.251,
    };
    unsafe {
        let (mut index, mut exists) = (0u32, BOOL(0));
        let name = wide(family);
        if gfx
            .system_fonts
            .FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists)
            .is_ok()
            && exists.as_bool()
            && let Ok(fam) = gfx.system_fonts.GetFontFamily(index)
            && let Ok(font) =
                fam.GetFirstMatchingFont(weight(w), DWRITE_FONT_STRETCH_NORMAL, style(italic))
        {
            let mut fm = DWRITE_FONT_METRICS::default();
            font.GetMetrics(&mut fm);
            let em = fm.designUnitsPerEm.max(1) as f32;
            m = FontMetrics {
                ascent: fm.ascent as f32 / em,
                descent: fm.descent as f32 / em,
            };
        }
    }
    cache.borrow_mut().insert(key, m);
    m
}

fn range(start: u32, len: u32) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE {
        startPosition: start,
        length: len,
    }
}

fn build(
    gfx: &Gfx,
    metrics: &MetricsCache,
    st: &StyledText,
    max_width: Option<f32>,
) -> Result<Laid> {
    // The text as laid out: icons become their Fluent glyph.
    let mut utf16: Vec<u16> = Vec::new();
    let mut pieces = Vec::with_capacity(st.runs.len());
    for r in &st.runs {
        let start = utf16.len() as u32;
        let icon = r.icon.as_deref().and_then(crate::keys::fluent_icon);
        match icon {
            Some(c) => utf16.extend(c.encode_utf16(&mut [0; 2]).iter()),
            None => utf16.extend(r.text.encode_utf16()),
        }
        pieces.push(Piece {
            start,
            len: utf16.len() as u32 - start,
            icon: icon.is_some(),
        });
    }
    let base: &StyledRun = &st.runs[0];
    let base_family = family_name(gfx, &base.font.family, base.font.size);
    let size = base.font.size.max(1.);
    unsafe {
        let family = wide(&base_family);
        let format = gfx.dw.CreateTextFormat(
            PCWSTR(family.as_ptr()),
            None,
            weight(base.font.weight),
            style(base.font.italic),
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            PCWSTR(wide("").as_ptr()),
        )?;
        format.SetTextAlignment(match st.align {
            TextAlign::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            TextAlign::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            TextAlign::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        })?;
        let one_line = st.no_wrap || st.max_lines == Some(1);
        format.SetWordWrapping(if one_line || max_width.is_none() {
            DWRITE_WORD_WRAPPING_NO_WRAP
        } else {
            DWRITE_WORD_WRAPPING_WRAP
        })?;
        // CSS line boxes: the extra space split above and below.
        if let Some(lh) = st.line_height {
            let m = font_metrics(
                gfx,
                metrics,
                &base_family,
                base.font.weight,
                base.font.italic,
            );
            let content = (m.ascent + m.descent) * size;
            let baseline = (lh - content) / 2. + m.ascent * size;
            format.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, lh.max(1.), baseline)?;
        }
        let width = max_width.unwrap_or(1e6).max(1.);
        let layout = gfx.dw.CreateTextLayout(&utf16, &format, width, 1e6)?;
        let total = utf16.len() as u32;
        // Per-run fonts and decorations.
        for (r, p) in st.runs.iter().zip(&pieces) {
            if p.len == 0 {
                continue;
            }
            let rr = range(p.start, p.len);
            let fam = if p.icon {
                gfx.fonts.icons.clone()
            } else {
                family_name(gfx, &r.font.family, r.font.size)
            };
            if fam != base_family {
                let w = wide(&fam);
                layout.SetFontFamilyName(PCWSTR(w.as_ptr()), rr)?;
            }
            if r.font.size != base.font.size {
                layout.SetFontSize(r.font.size.max(1.), rr)?;
            }
            if r.font.weight != base.font.weight {
                layout.SetFontWeight(weight(r.font.weight), rr)?;
            }
            if r.font.italic != base.font.italic {
                layout.SetFontStyle(style(r.font.italic), rr)?;
            }
            if r.underline {
                layout.SetUnderline(true, rr)?;
            }
            if r.strikethrough {
                layout.SetStrikethrough(true, rr)?;
            }
            if r.font.tabular {
                let t = gfx.dw.CreateTypography()?;
                t.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES,
                    parameter: 1,
                })?;
                layout.SetTypography(&t, rr)?;
            }
        }
        if st.letter_spacing != 0.
            && let Ok(l1) = layout.cast::<IDWriteTextLayout1>()
        {
            l1.SetCharacterSpacing(0., st.letter_spacing, 0., range(0, total))?;
        }
        // Line clamps: an ellipsis at the end of the last line shown.
        if one_line || st.max_lines.is_some() {
            let sign = gfx.dw.CreateEllipsisTrimmingSign(&layout)?;
            layout.SetTrimming(
                &DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                },
                &sign,
            )?;
            if let Some(n) = st.max_lines.filter(|n| *n > 1) {
                let lines = line_metrics(&layout);
                if lines.len() > n as usize {
                    let h: f32 = lines[..n as usize].iter().map(|l| l.height).sum();
                    layout.SetMaxHeight(h)?;
                }
            }
        }
        Ok(Laid {
            layout,
            text: st.clone(),
            pieces,
        })
    }
}

fn line_metrics(layout: &IDWriteTextLayout) -> Vec<DWRITE_LINE_METRICS> {
    unsafe {
        let mut n = 0u32;
        let _ = layout.GetLineMetrics(None, &mut n);
        let mut lines = vec![DWRITE_LINE_METRICS::default(); n as usize];
        if layout.GetLineMetrics(Some(&mut lines), &mut n).is_err() {
            return Vec::new();
        }
        lines.truncate(n as usize);
        lines
    }
}

impl Laid {
    /// Width and height, rounded up (laying it out again at that width
    /// keeps its lines).
    pub fn size(&self) -> (f32, f32) {
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = self.layout.GetMetrics(&mut m);
        }
        // Lines cut by a clamp: the height of those shown.
        let max_h = unsafe { self.layout.GetMaxHeight() };
        let h = if max_h < 1e5 {
            m.height.min(max_h)
        } else {
            m.height
        };
        ((m.width + 0.01).ceil(), h.ceil())
    }

    /// The longest unbreakable piece (CSS min-content).
    pub fn min_width(&self) -> f32 {
        unsafe { self.layout.DetermineMinWidth().unwrap_or(0.).ceil() }
    }

    /// Distance from the top to the first line's baseline.
    pub fn baseline(&self) -> Option<f32> {
        line_metrics(&self.layout).first().map(|l| l.baseline)
    }

    #[allow(clippy::too_many_arguments)]
    /// Draws the text at `(x, y)` (in `dc`'s current transform), in `color`
    /// where its runs have none; `clip` cuts it at its box.
    pub fn draw(
        &self,
        dc: &ID2D1DeviceContext,
        x: f32,
        y: f32,
        width: f32,
        color: prism_core::Color,
        resolve: &dyn Fn(prism_core::Color) -> prism_core::Color,
        clip: bool,
    ) -> Result<()> {
        unsafe {
            let layout = &self.layout;
            // Laid out unbounded (measuring): give it its box now, so
            // alignment and wrapping use it.
            let max = layout.GetMaxWidth();
            if width > 0. && (max > 1e5 || (max - width).abs() > 0.5) {
                layout.SetMaxWidth(width.max(1.))?;
            }
            // Run backgrounds (highlights) under the text.
            for (r, p) in self.text.runs.iter().zip(&self.pieces) {
                let Some(bg) = r.background else { continue };
                let brush = dc.CreateSolidColorBrush(&color_f(resolve(bg)), None)?;
                let mut n = 0u32;
                let _ = layout.HitTestTextRange(p.start, p.len, x, y, None, &mut n);
                let mut hits = vec![DWRITE_HIT_TEST_METRICS::default(); n as usize];
                if layout
                    .HitTestTextRange(p.start, p.len, x, y, Some(&mut hits), &mut n)
                    .is_ok()
                {
                    for h in &hits[..n as usize] {
                        dc.FillRectangle(
                            &D2D_RECT_F {
                                left: h.left,
                                top: h.top,
                                right: h.left + h.width,
                                bottom: h.top + h.height,
                            },
                            &brush,
                        );
                    }
                }
            }
            let base = dc.CreateSolidColorBrush(&color_f(resolve(color)), None)?;
            for (r, p) in self.text.runs.iter().zip(&self.pieces) {
                if p.len == 0 {
                    continue;
                }
                let c = r.color.map(resolve);
                let effect: Option<windows::core::IUnknown> = match c {
                    Some(c) => Some(dc.CreateSolidColorBrush(&color_f(c), None)?.cast()?),
                    None => None,
                };
                layout.SetDrawingEffect(effect.as_ref(), range(p.start, p.len))?;
            }
            let mut options = D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT;
            if clip {
                options |= D2D1_DRAW_TEXT_OPTIONS_CLIP;
            }
            dc.DrawTextLayout(Vector2 { X: x, Y: y }, layout, &base, options);
        }
        Ok(())
    }
}

/// A plain run of `text` in the UI font: headers, tab labels, control
/// titles, placeholders.
pub fn plain(text: &str, size: f32, w: prism_core::FontWeight, align: TextAlign) -> StyledText {
    let style = prism_core::Style {
        font_size: size,
        font_weight: w,
        text_align: align,
        no_wrap: true,
        ..Default::default()
    };
    StyledText::from_runs(text, &[], &style)
}
