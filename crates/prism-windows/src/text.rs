//! Text as XAML `TextBlock`s: prism-native's `StyledText` becomes a
//! TextBlock's properties and `Run` inlines (fonts and weights per run,
//! colors, underline and strikethrough, inline Segoe Fluent Icons, CSS line
//! heights, letter spacing, wrapping, line clamps with an ellipsis,
//! alignment). Links are `Hyperlink`s: Tab reaches them, Enter or a click
//! reports the link (`UiEvent::MenuSelect`), UI Automation sees links.
//!
//! Measuring uses the same code on a scratch TextBlock outside the tree
//! (`Measurer`): XAML lays text out with DirectWrite itself, so what is
//! measured is what is drawn, wrapping included. Sizes are cached per
//! node and width until its text or style changes.

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::{Color, FontFamily, NodeId, TextAlign};
use prism_native::text::{StyledRun, StyledText};
use windows::Foundation::Size;
use windows::UI::Text::{FontStyle, FontWeight, TextDecorations};
use windows_core::Result;

use crate::fluent::Palette;
use crate::winui::xaml::Controls::TextBlock;
use crate::winui::xaml::Documents::{Hyperlink, HyperlinkClickEventArgs, Run, UnderlineStyle};
use crate::winui::xaml::Media::FontFamily as XamlFont;
use crate::winui::xaml::{LineStackingStrategy, TextAlignment, TextTrimming, TextWrapping};
use crate::winui::{brush, h};

/// The fonts of Windows 11, with Windows 10's as fallbacks (XAML takes a
/// comma-separated list).
pub const TEXT_FONT: &str = "Segoe UI Variable Text, Segoe UI";
pub const DISPLAY_FONT: &str = "Segoe UI Variable Display, Segoe UI";
pub const ICON_FONT: &str = "Segoe Fluent Icons, Segoe MDL2 Assets";

/// The XAML font family for a Prism family at `size`.
pub fn family_name(family: &FontFamily, size: f32) -> String {
    match prism_native::fonts::resolve(family) {
        FontFamily::System | FontFamily::Rounded => {
            if size >= 20. {
                DISPLAY_FONT.into()
            } else {
                TEXT_FONT.into()
            }
        }
        FontFamily::Monospace => "Cascadia Mono, Consolas, Courier New".into(),
        FontFamily::Serif => "Georgia, Cambria, Times New Roman".into(),
        // A run-time font by its file (`fonts.rs`).
        FontFamily::Named(name) => match crate::fonts::source(&name) {
            Some(file) => format!("{file}, Segoe UI"),
            None => format!("{name}, Segoe UI"),
        },
    }
}

thread_local! {
    /// XAML font families by name (they are immutable: shared).
    static FAMILIES: RefCell<HashMap<String, XamlFont>> = RefCell::new(HashMap::new());
}

pub fn font(name: &str) -> Result<XamlFont> {
    FAMILIES.with(|f| {
        if let Some(font) = f.borrow().get(name) {
            return Ok(font.clone());
        }
        let font = XamlFont::CreateInstanceWithName(&h(name))?;
        f.borrow_mut().insert(name.to_string(), font.clone());
        Ok(font)
    })
}

pub fn weight(w: prism_core::FontWeight) -> FontWeight {
    FontWeight { Weight: w.css() }
}

pub fn style(italic: bool) -> FontStyle {
    if italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    }
}

fn alignment(a: TextAlign) -> TextAlignment {
    match a {
        TextAlign::Center => TextAlignment::Center,
        TextAlign::Right => TextAlignment::Right,
        _ => TextAlignment::Left,
    }
}

/// A run's text as drawn: its icon's Fluent glyph when it has one.
fn run_text(r: &StyledRun) -> (String, bool) {
    if let Some(icon) = &r.icon
        && let Some(c) = crate::keys::fluent_icon(icon)
    {
        return (c.to_string(), true);
    }
    (r.text.clone(), false)
}

/// A TextBlock's paragraph settings (fonts of the first run, wrapping,
/// clamping, line height, spacing, alignment).
#[derive(Clone, Debug, PartialEq)]
struct Paragraph {
    size: f32,
    family: String,
    weight: u16,
    italic: bool,
    align: TextAlign,
    no_wrap: bool,
    max_lines: Option<u32>,
    line_height: Option<f32>,
    spacing: i32,
}

impl Paragraph {
    fn of(st: &StyledText) -> Paragraph {
        let first = &st.runs[0];
        let size = first.font.size.max(1.);
        Paragraph {
            size,
            family: family_name(&first.font.family, size),
            weight: first.font.weight.css(),
            italic: first.font.italic,
            align: st.align,
            no_wrap: st.no_wrap,
            max_lines: st.max_lines,
            line_height: st.line_height,
            // XAML spaces characters in thousandths of an em.
            spacing: (st.letter_spacing / size * 1000.).round() as i32,
        }
    }

    fn set(&self, tb: &TextBlock) -> Result<()> {
        tb.SetFontSize(self.size as f64)?;
        tb.SetFontFamily(&font(&self.family)?)?;
        tb.SetFontWeight(FontWeight {
            Weight: self.weight,
        })?;
        tb.SetFontStyle(style(self.italic))?;
        tb.SetTextAlignment(alignment(self.align))?;
        // Prism scales text itself (`RenderEnvironment::text_scale`).
        tb.SetIsTextScaleFactorEnabled(false)?;
        tb.SetTextWrapping(if self.no_wrap {
            TextWrapping::NoWrap
        } else {
            TextWrapping::Wrap
        })?;
        tb.SetMaxLines(self.max_lines.unwrap_or(0) as i32)?;
        tb.SetTextTrimming(if self.max_lines.is_some() {
            TextTrimming::CharacterEllipsis
        } else if self.no_wrap {
            TextTrimming::Clip
        } else {
            TextTrimming::None
        })?;
        match self.line_height {
            Some(lh) => {
                tb.SetLineHeight(lh as f64)?;
                tb.SetLineStackingStrategy(LineStackingStrategy::BlockLineHeight)?;
            }
            None => {
                tb.SetLineHeight(0.)?;
                tb.SetLineStackingStrategy(LineStackingStrategy::MaxHeight)?;
            }
        }
        tb.SetCharacterSpacing(self.spacing)
    }
}

fn plain(st: &StyledText) -> bool {
    let first = &st.runs[0];
    st.runs.len() == 1
        && first.icon.is_none()
        && !first.underline
        && !first.strikethrough
        && first.color.is_none()
        && first.link.is_none()
}

/// The text itself: one string, or `Run`s (`pal`: their colors, unless
/// only measuring).
fn set_runs(tb: &TextBlock, st: &StyledText, pal: Option<&Palette>) -> Result<()> {
    let first = &st.runs[0];
    if plain(st) {
        tb.SetTextDecorations(TextDecorations::None)?;
        tb.SetText(&h(&first.text))?;
        return Ok(());
    }
    tb.SetText(&h(""))?;
    let inlines = tb.Inlines()?;
    inlines.Clear()?;
    for r in &st.runs {
        let run = Run::new()?;
        // Measuring lays a link out as its run: the same metrics.
        let link = r.link.filter(|_| pal.is_some());
        let (text, icon) = run_text(r);
        run.SetText(&h(&text))?;
        let size = r.font.size.max(1.);
        if (size - first.font.size).abs() > 0.01 {
            run.SetFontSize(size as f64)?;
        }
        let name = if icon {
            ICON_FONT.to_string()
        } else {
            family_name(&r.font.family, size)
        };
        run.SetFontFamily(&font(&name)?)?;
        run.SetFontWeight(weight(r.font.weight))?;
        run.SetFontStyle(style(r.font.italic))?;
        if let (Some(c), Some(pal)) = (r.color, pal) {
            run.SetForeground(&brush(pal.ink(c))?)?;
        }
        let mut deco = TextDecorations::None;
        if r.underline {
            deco |= TextDecorations::Underline;
        }
        if r.strikethrough {
            deco |= TextDecorations::Strikethrough;
        }
        match link {
            Some(link) => {
                let a = hyperlink(link, r, pal)?;
                // Its underline is the run's (Prism styles links itself).
                run.SetTextDecorations(deco & !TextDecorations::Underline)?;
                a.Inlines()?.Append(&run)?;
                inlines.Append(&a)?;
            }
            None => {
                run.SetTextDecorations(deco)?;
                inlines.Append(&run)?;
            }
        }
    }
    Ok(())
}

/// A link run as a `Hyperlink`: focusable with Tab, activated with Enter
/// or a click (`UiEvent::MenuSelect(link)`), a link for UI Automation. It
/// keeps the run's own color and underline instead of the accent's.
fn hyperlink(link: u64, r: &StyledRun, pal: Option<&Palette>) -> Result<Hyperlink> {
    let a = Hyperlink::new()?;
    a.SetUnderlineStyle(if r.underline {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    })?;
    // Under a contrast theme, the theme's link color.
    if let Some(pal) = pal
        && (r.color.is_some() || pal.forced.is_some())
    {
        a.SetForeground(&brush(pal.link(r.color.unwrap_or(pal.accent)))?)?;
    }
    a.Click(&windows::Foundation::TypedEventHandler::<
        Hyperlink,
        HyperlinkClickEventArgs,
    >::new(move |_, _| {
        crate::imp::dispatch(prism_core::UiEvent::MenuSelect(link));
        Ok(())
    }))?;
    Ok(a)
}

/// Sets `tb` up to show `st` in `color` (the text's own color, resolved,
/// for runs without one).
pub fn apply(tb: &TextBlock, st: &StyledText, pal: &Palette, color: Color) -> Result<()> {
    Paragraph::of(st).set(tb)?;
    tb.SetForeground(&brush(color)?)?;
    set_runs(tb, st, Some(pal))
}

thread_local! {
    /// XAML text layouts by why (`PRISM_TRACE`): on one line, wrapped,
    /// narrowest, baseline, a control's label.
    static WHY: std::cell::Cell<[u32; 5]> = const { std::cell::Cell::new([0; 5]) };
}

fn count(why: usize) {
    if crate::perf::on() {
        let mut w = WHY.get();
        w[why] += 1;
        WHY.set(w);
    }
}

/// The XAML text layouts since the last call, by why.
pub fn take_counts() -> String {
    let [a, b, c, d, e] = WHY.replace([0; 5]);
    format!("one line {a}, wrapped {b}, narrowest {c}, baseline {d}, labels {e}")
}

/// Natural sizes of text, from a scratch TextBlock.
pub struct Measurer {
    tb: TextBlock,
    /// The paragraph settings the scratch TextBlock has now: set again
    /// only when they change (each is a call into XAML).
    paragraph: RefCell<Option<Paragraph>>,
    /// Sizes by text and width limit (labels of buttons, pickers…).
    sizes: RefCell<HashMap<String, (f32, f32)>>,
    /// First baselines by text and width (each layout asks again).
    baselines: RefCell<HashMap<String, Option<f32>>>,
    /// Each text node's sizes, kept while its text and style stay the
    /// same (a restyle that changes neither measures nothing).
    cache: HashMap<NodeId, Measured>,
}

/// What a node's text measured: on one line, at its narrowest, and within
/// a few width limits (quarter DIPs).
struct Measured {
    text: StyledText,
    natural: Option<(f32, f32)>,
    min: Option<f32>,
    within: Vec<(u32, (f32, f32))>,
}

impl Measurer {
    pub fn new() -> Result<Measurer> {
        Ok(Measurer {
            tb: TextBlock::new()?,
            paragraph: RefCell::new(None),
            sizes: RefCell::new(HashMap::new()),
            baselines: RefCell::new(HashMap::new()),
            cache: HashMap::new(),
        })
    }

    pub fn forget(&mut self, id: NodeId) {
        self.cache.remove(&id);
    }

    fn entry(&mut self, id: NodeId, st: &StyledText) -> &mut Measured {
        let m = self.cache.entry(id).or_insert_with(|| Measured {
            text: st.clone(),
            natural: None,
            min: None,
            within: Vec::new(),
        });
        if m.text != *st {
            *m = Measured {
                text: st.clone(),
                natural: None,
                min: None,
                within: Vec::new(),
            };
        }
        m
    }

    /// The size of node `id`'s text `st` within `max_width`. A text that
    /// fits on its lines (its natural width is within the limit) is not
    /// laid out again: it would not wrap.
    pub fn node(&mut self, id: NodeId, st: &StyledText, max_width: Option<f32>) -> (f32, f32) {
        let natural = match self.entry(id, st).natural {
            Some(n) => n,
            None => {
                count(0);
                let n = self.measure_uncached(st, None);
                self.entry(id, st).natural = Some(n);
                n
            }
        };
        let Some(w) = max_width else {
            return natural;
        };
        if w >= natural.0 {
            return natural;
        }
        let key = (w * 4.).round().max(0.) as u32;
        if let Some((_, size)) = self.entry(id, st).within.iter().find(|(k, _)| *k == key) {
            return *size;
        }
        count(1);
        let size = self.measure_uncached(st, Some(w));
        let m = self.entry(id, st);
        if m.within.len() >= 4 {
            m.within.remove(0);
        }
        m.within.push((key, size));
        size
    }

    /// The narrowest node `id`'s text `st` can be: its longest word.
    pub fn node_min(&mut self, id: NodeId, st: &StyledText) -> f32 {
        if let Some(min) = self.entry(id, st).min {
            return min;
        }
        count(2);
        let min = self.min_width(st);
        self.entry(id, st).min = Some(min);
        min
    }

    /// The scratch TextBlock showing `st` (no colors: not drawn).
    fn load(&self, st: &StyledText) -> Result<()> {
        let p = Paragraph::of(st);
        if self.paragraph.borrow().as_ref() != Some(&p) {
            p.set(&self.tb)?;
            *self.paragraph.borrow_mut() = Some(p);
        }
        set_runs(&self.tb, st, None)
    }

    /// The size of `st` within `max_width` (`None`: unbounded).
    pub fn measure(&self, st: &StyledText, max_width: Option<f32>) -> (f32, f32) {
        let key = format!("{:?}|{st:?}", max_width.map(|w| (w * 4.).round() as i64));
        if let Some(size) = self.sizes.borrow().get(&key) {
            return *size;
        }
        count(4);
        let size = self.measure_uncached(st, max_width);
        let mut sizes = self.sizes.borrow_mut();
        if sizes.len() > 4096 {
            sizes.clear();
        }
        sizes.insert(key, size);
        size
    }

    /// `st` laid out by XAML within `max_width`.
    fn measure_uncached(&self, st: &StyledText, max_width: Option<f32>) -> (f32, f32) {
        if self.load(st).is_err() {
            return (0., 0.);
        }
        let size = self.desired(max_width.unwrap_or(f32::INFINITY));
        // On one line: its first baseline comes with it (no second layout).
        if max_width.is_none() && uniform(st) {
            let b = self.tb.BaselineOffset().ok().map(|b| b as f32);
            self.remember_baseline(format!("{st:?}"), b.filter(|b| *b > 0.));
        }
        size
    }

    fn remember_baseline(&self, key: String, b: Option<f32>) {
        let mut baselines = self.baselines.borrow_mut();
        if baselines.len() > 4096 {
            baselines.clear();
        }
        baselines.insert(key, b);
    }

    fn desired(&self, width: f32) -> (f32, f32) {
        let _t = crate::perf::span(crate::perf::Cost::TextLayout);
        let _ = self.tb.Measure(Size {
            Width: width.max(0.),
            Height: f32::INFINITY,
        });
        self.tb
            .DesiredSize()
            .map_or((0., 0.), |s| (s.Width, s.Height))
    }

    /// The narrowest the text can be: its longest word (CSS min-content,
    /// #284). XAML clips a measured size to the width it is measured at,
    /// so the width comes from line counts instead: narrower than the
    /// longest word, `Wrap` breaks that word (one more line) where
    /// `WrapWholeWords` lets it stick out; from the longest word on, both
    /// break alike. A binary search finds where they start to agree. A
    /// text that cannot break (`09:35`, `N7`) is its one-line width.
    fn min_width(&self, st: &StyledText) -> f32 {
        let natural = self.measure(st, None).0.ceil();
        if natural <= 1. || !breakable(st) || self.load(st).is_err() {
            return natural;
        }
        let height = |wrapping, width| {
            let _ = self.tb.SetTextWrapping(wrapping);
            self.desired(width).1
        };
        let (mut lo, mut hi) = (0f32, natural);
        while hi - lo > 1. {
            let mid = (lo + hi) / 2.;
            let broken = height(TextWrapping::Wrap, mid);
            if (broken - height(TextWrapping::WrapWholeWords, mid)).abs() < 0.5 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        // Back to the cached settings' wrapping.
        if let Some(p) = self.paragraph.borrow().as_ref() {
            let _ = self.tb.SetTextWrapping(if p.no_wrap {
                TextWrapping::NoWrap
            } else {
                TextWrapping::Wrap
            });
        }
        hi.ceil()
    }

    /// Distance from the top to the first baseline of `st` at `width`. A
    /// text in one font size has its first line's baseline at any width:
    /// the one measured on one line.
    pub fn baseline(&self, st: &StyledText, width: f32) -> Option<f32> {
        let key = if uniform(st) {
            format!("{st:?}")
        } else {
            format!("{}|{st:?}", (width * 4.).round() as i64)
        };
        if let Some(b) = self.baselines.borrow().get(&key) {
            return *b;
        }
        count(3);
        let b = (|| {
            self.load(st).ok()?;
            self.desired(if uniform(st) { f32::INFINITY } else { width });
            let b = self.tb.BaselineOffset().ok()? as f32;
            (b > 0.).then_some(b)
        })();
        self.remember_baseline(key, b);
        b
    }
}

/// Whether every run of `st` has the first one's size and is text (not
/// an icon): its first baseline does not depend on where lines wrap.
fn uniform(st: &StyledText) -> bool {
    let size = st.runs[0].font.size;
    st.runs
        .iter()
        .all(|r| r.icon.is_none() && (r.font.size - size).abs() < 0.01)
}

/// Whether `st` may break between lines somewhere: a space, a hyphen, a
/// slash, a script that breaks between characters (Thai, CJK…). Times,
/// numbers and single words cannot (`09:35`, `N7`, `Line`, `+3,5 %`).
fn breakable(st: &StyledText) -> bool {
    st.runs.iter().flat_map(|r| r.text.chars()).any(|c| {
        !((c.is_alphanumeric() && c < '\u{0E00}')
            || matches!(c, ':' | '.' | ',' | '\'' | '%' | '#' | '+' | '°'))
    })
}

/// A one-run text in a control's font: a button's title, a picker's
/// option, a tab's label.
pub fn label(text: &str, s: &prism_core::Style) -> StyledText {
    let mut st = StyledText::new(text, "", s);
    st.align = TextAlign::Left;
    st.no_wrap = true;
    st.max_lines = None;
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_numbers_and_words_cannot_break() {
        let st = |t: &str| StyledText::new(t, "", &prism_core::Style::default());
        for t in ["09:35", "N7", "Line", "Départ", "43", "+3,5%", "Москва"] {
            assert!(!breakable(&st(t)), "{t}");
        }
        for t in ["Old Town", "well-known", "a/b", "東京駅", "x\ny"] {
            assert!(breakable(&st(t)), "{t}");
        }
    }
}
