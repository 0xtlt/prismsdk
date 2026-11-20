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
            run.SetForeground(&brush(pal.resolve(c))?)?;
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
    if let (Some(c), Some(pal)) = (r.color, pal) {
        a.SetForeground(&brush(pal.resolve(c))?)?;
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

/// Natural sizes of text, from a scratch TextBlock.
pub struct Measurer {
    tb: TextBlock,
    /// The paragraph settings the scratch TextBlock has now: set again
    /// only when they change (each is a call into XAML).
    paragraph: RefCell<Option<Paragraph>>,
    /// Sizes by text and width limit (labels of buttons, pickers…).
    sizes: RefCell<HashMap<String, (f32, f32)>>,
    /// A node's sizes by width limit (quarter DIPs), dropped when its text
    /// or style changes.
    cache: HashMap<NodeId, (StyledText, Vec<Sized>)>,
}

/// A size measured within a width limit (quarter DIPs).
type Sized = (Option<u32>, (f32, f32));

impl Measurer {
    pub fn new() -> Result<Measurer> {
        Ok(Measurer {
            tb: TextBlock::new()?,
            paragraph: RefCell::new(None),
            sizes: RefCell::new(HashMap::new()),
            cache: HashMap::new(),
        })
    }

    pub fn forget(&mut self, id: NodeId) {
        self.cache.remove(&id);
    }

    /// The size of node `id`'s text `st` within `max_width`.
    pub fn node(&mut self, id: NodeId, st: &StyledText, max_width: Option<f32>) -> (f32, f32) {
        let key = max_width.map(|w| (w * 4.).round().max(0.) as u32);
        if let Some((text, sizes)) = self.cache.get(&id)
            && text == st
            && let Some((_, size)) = sizes.iter().find(|(k, _)| *k == key)
        {
            return *size;
        }
        let size = self.measure(st, max_width);
        let entry = self
            .cache
            .entry(id)
            .or_insert_with(|| (st.clone(), Vec::new()));
        if entry.0 != *st {
            *entry = (st.clone(), Vec::new());
        }
        if entry.1.len() >= 4 {
            entry.1.remove(0);
        }
        entry.1.push((key, size));
        size
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
        if self.load(st).is_err() {
            return (0., 0.);
        }
        let size = self.desired(max_width.unwrap_or(f32::INFINITY));
        let mut sizes = self.sizes.borrow_mut();
        if sizes.len() > 4096 {
            sizes.clear();
        }
        sizes.insert(key, size);
        size
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

    /// The narrowest the text can be: its longest word.
    pub fn min_width(&self, st: &StyledText) -> f32 {
        if self.load(st).is_err() {
            return 0.;
        }
        let _ = self.tb.SetTextWrapping(TextWrapping::WrapWholeWords);
        // The wrapping changed behind the cached settings.
        *self.paragraph.borrow_mut() = None;
        self.desired(0.).0
    }

    /// Distance from the top to the first baseline of `st` at `width`.
    pub fn baseline(&self, st: &StyledText, width: f32) -> Option<f32> {
        self.load(st).ok()?;
        self.desired(width);
        let b = self.tb.BaselineOffset().ok()? as f32;
        (b > 0.).then_some(b)
    }
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
