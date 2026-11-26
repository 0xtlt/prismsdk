//! Rich inputs (`rich_input()`, #115): WinUI's `RichEditBox` (RichEdit,
//! the Text Object Model) showing the runs as character formats: weight,
//! italic, underline (its style), strikethrough, colors, background,
//! font family and size; links look like their runs.
//!
//! Like the macOS backend, edits reach the app as the plain text
//! (`UiEvent::Input`): prism-core gives typed text the style of the run
//! around it and keeps the other runs. The app's runs come back in the
//! same turn: when they hold the text the field shows (the echo of the
//! typing, or the app restyling it), their formats are applied in place,
//! the caret, the selection and the undo history untouched; other runs
//! replace the text, the caret keeping its distance from the end (#73).
//! Return calls `on_submit`, Shift+Return inserts a line break; pastes
//! are plain text. A chip (#115) is a picture embedded in the text
//! (`chips.rs`): one character the caret jumps over.
//! The editing hooks (#217: copies, pastes, clicks, the selection's
//! rectangle) are `edit.rs`'.
//!
//! Text without a color of its own gets the theme's text color, set on
//! each run and as the document's default (what typing into an empty
//! field takes), and again when the theme changes: RichEdit's "auto"
//! color is the system's window text, black even in a dark theme.

use std::cell::RefCell;

use prism_core::{NodeId, Style, TextRun, TextTransform, UiEvent, UnderlineStyle, decode_runs};
use prism_native::text::StyledText;
use windows::System::VirtualKey;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
use windows_core::{HSTRING, Interface, Result};

use crate::fluent::Palette;
use crate::imp::dispatch;
use crate::winui::bindings::Microsoft::UI::Text::{
    FormatEffect, ITextRange, RichEditTextDocument, TextConstants, TextGetOptions, TextSetOptions,
    UnderlineType,
};
use crate::winui::xaml::Controls::{Canvas, RichEditBox, TextControlPasteEventHandler};
use crate::winui::xaml::Input::KeyEventHandler;
use crate::winui::xaml::{RoutedEventHandler, TextWrapping};
use crate::winui::{h, win_color};

/// `CF_UNICODETEXT`: pastes as plain text.
const PLAIN_TEXT: i32 = 13;

thread_local! {
    /// Each rich input's chips with `on_click`: their first UTF-16 unit and
    /// link id (#327), for its taps.
    static LINKS: RefCell<std::collections::HashMap<NodeId, Vec<(i32, u64)>>> =
        RefCell::default();
}

impl Drop for Rich {
    fn drop(&mut self) {
        LINKS.with(|l| l.borrow_mut().remove(&self.id));
    }
}

/// A rich input node's editor and the runs it shows.
pub struct Rich {
    pub element: RichEditBox,
    id: NodeId,
    /// The text the app asked for (`set_text`), shown with the next runs.
    replace: RefCell<Option<String>>,
    /// The chips shown, in order.
    chips: RefCell<Vec<crate::chips::Look>>,
    /// The runs last shown (encoded), formatted again in a new theme.
    runs: RefCell<String>,
}

impl Rich {
    pub fn new(id: NodeId) -> Result<Rich> {
        let r = RichEditBox::new()?;
        r.SetMinWidth(0.)?;
        r.SetMinHeight(0.)?;
        r.SetIsTextScaleFactorEnabled(false)?;
        r.SetTextWrapping(TextWrapping::Wrap)?;
        r.SetAcceptsReturn(true)?;
        r.TextChanged(&RoutedEventHandler::new(move |s, _| {
            if let Some(r) = s.as_ref().and_then(|s| s.cast::<RichEditBox>().ok()) {
                let text = text(&r.Document()?)?;
                if crate::win::env("PRISM_TRACE_INPUT").is_some() {
                    crate::win::log!("input: rich input {id} shows {text:?}");
                }
                crate::imp::typed(id, text);
                crate::edit::changed(id, &r);
                crate::carets::changed(id);
                crate::overlay::caret_moved();
            }
            Ok(())
        }))?;
        r.SelectionChanged(&RoutedEventHandler::new(move |s, _| {
            if let Some(r) = s.as_ref().and_then(|s| s.cast::<RichEditBox>().ok()) {
                let doc = r.Document()?;
                let sel = doc.Selection()?;
                let (a, b) = (sel.StartPosition()?, sel.EndPosition()?);
                crate::imp::selected(id, &text(&doc)?, a.max(0) as usize, b.max(0) as usize);
                crate::edit::changed(id, &r);
                crate::overlay::caret_moved();
            }
            Ok(())
        }))?;
        // Return submits; Shift+Return is a line break (RichEdit's own).
        r.PreviewKeyDown(&KeyEventHandler::new(move |_, a| {
            if let Some(a) = a.as_ref()
                && a.Key()? == VirtualKey::Enter
                && unsafe { GetKeyState(VK_SHIFT.0 as i32) } >= 0
            {
                a.SetHandled(true)?;
                dispatch(UiEvent::Submit(id));
            }
            Ok(())
        }))?;
        // Plain text only: the styles are the app's.
        r.Paste(&TextControlPasteEventHandler::new(move |s, a| {
            if let (Some(r), Some(a)) = (
                s.as_ref().and_then(|s| s.cast::<RichEditBox>().ok()),
                a.as_ref(),
            ) {
                a.SetHandled(true)?;
                // The app's (`on_paste`), else plain text.
                if !crate::edit::paste(id, &r)? {
                    r.Document()?.Selection()?.Paste(PLAIN_TEXT)?;
                }
            }
            Ok(())
        }))?;
        // The hooks: without them the field still edits.
        if let Err(e) = crate::edit::connect(id, &r) {
            crate::win::log!("rich input {id}: editing hooks: {e}");
        }
        if let Err(e) = crate::across::connect(id, &r.cast()?) {
            crate::win::log!("rich input {id}: drags across fields: {e}");
        }
        // A tap on a chip with `on_click` (the box takes its taps).
        let e: crate::winui::xaml::UIElement = r.cast()?;
        e.AddHandler(
            &crate::winui::xaml::UIElement::TappedEvent()?,
            &crate::boxed::boxed(crate::winui::xaml::Input::TappedEventHandler::new(
                move |s, a| {
                    if let (Some(s), Some(a)) = (s.as_ref(), a.as_ref())
                        && let Ok(r) = s.cast::<RichEditBox>()
                    {
                        let links = LINKS.with(|l| l.borrow().get(&id).cloned());
                        crate::edit::chip_tapped(&r, a, links.as_deref().unwrap_or_default())?;
                    }
                    Ok(())
                },
            )),
            true,
        )?;
        Ok(Rich {
            element: r,
            id,
            replace: RefCell::new(None),
            chips: RefCell::new(Vec::new()),
            runs: RefCell::new(String::new()),
        })
    }

    /// The app's text (`set_text`): shown with the runs that follow.
    pub fn set_text(&self, text: &str) {
        *self.replace.borrow_mut() = Some(text.to_string());
    }

    /// The field's plain text.
    pub fn text(&self) -> Option<String> {
        self.element.Document().ok().and_then(|d| text(&d).ok())
    }

    /// Selects `start..end` (UTF-16).
    pub fn select(&self, start: usize, end: usize) -> Result<()> {
        self.element
            .Document()?
            .Selection()?
            .SetRange(start as i32, end as i32)
    }

    /// The runs shown (`set_rich_value`'s, encoded).
    pub fn runs(&self) -> String {
        self.runs.borrow().clone()
    }

    /// Shows `runs` (`set_rich_value`) in `style`, its colors from `pal`,
    /// its chips drawn at `scale` (pixels per DIP).
    pub fn set_runs(&self, runs: &str, style: &Style, pal: &Palette, scale: f32) -> Result<()> {
        runs.clone_into(&mut self.runs.borrow_mut());
        let runs: Vec<TextRun> = decode_runs(runs);
        let shown: String = runs.iter().map(|r| r.text.as_str()).collect();
        let doc = self.element.Document()?;
        let current = text(&doc)?;
        let replace = self.replace.borrow_mut().take();
        let replaced = shown != current;
        if replaced {
            // Not the text shown: the app's own (else a stale echo of the
            // typing, which the next one corrects).
            if replace.as_deref() != Some(shown.as_str()) {
                return Ok(());
            }
            let sel = doc.Selection()?;
            let len = |s: &str| s.encode_utf16().count() as i32;
            let from_end = (len(&current) - sel.EndPosition()?).max(0);
            doc.SetText(TextSetOptions::None, &h(&shown))?;
            let caret = (len(&shown) - from_end).max(0);
            sel.SetRange(caret, caret)?;
        }
        // The text the user edits is the value: no case transform.
        let style = Style {
            text_transform: TextTransform::None,
            ..style.clone()
        };
        let st = StyledText::from_runs(&shown, &runs, &style);
        let ink = pal.ink(style.text_color.unwrap_or(pal.text));
        let default = doc.GetDefaultCharacterFormat()?;
        if default.ForegroundColor()? != win_color(ink) {
            default.SetForegroundColor(win_color(ink))?;
            doc.SetDefaultCharacterFormat(&default)?;
        }
        format(&doc, &st, &runs, pal)?;
        // Underline colors, which RichEdit lacks: drawn over the field.
        let marks = crate::underlay::colored(crate::underline::marks(&st, true), pal, ink);
        if let Some(outer) = self
            .element
            .Parent()
            .ok()
            .and_then(|p| p.cast::<Canvas>().ok())
        {
            let field = crate::carets::Field::Rich(self.element.clone());
            crate::carets::set_underlines(self.id, &outer, field, marks, pal);
        }
        self.place_chips(&doc, &st, &runs, pal, scale, replaced)
    }

    /// The theme changed: the runs' colors (and chips) again.
    pub fn retheme(&self, style: &Style, pal: &Palette, scale: f32) -> Result<()> {
        let runs = self.runs.borrow().clone();
        self.set_runs(&runs, style, pal, scale)
    }

    /// Puts the chips' pictures in place of their characters: all of them
    /// after new text, else those whose look changed. When their number
    /// changed (the user deleted one, an undo brought one back), the
    /// pictures in the text are already the right ones.
    fn place_chips(
        &self,
        doc: &RichEditTextDocument,
        st: &StyledText,
        runs: &[TextRun],
        pal: &Palette,
        scale: f32,
        replaced: bool,
    ) -> Result<()> {
        let want: Vec<(i32, crate::chips::Look)> = st
            .runs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let t = runs.get(i).filter(|t| t.chip.is_some())?;
                let color = pal.ink(r.color.unwrap_or(pal.text));
                let family = crate::text::family_name(
                    &r.font.family,
                    r.font.size,
                    r.font.weight.css(),
                    r.font.italic,
                );
                let look = crate::chips::Look {
                    label: t.chip_label.clone().unwrap_or_default(),
                    icon: t.chip_icon.as_deref().and_then(crate::keys::fluent_icon),
                    family: family.split(',').next().unwrap_or("").trim().to_string(),
                    size: r.font.size.max(1.),
                    weight: r.font.weight.css(),
                    color,
                    background: r
                        .background
                        .map_or_else(|| crate::chips::tint(color), |c| pal.fill(c)),
                    scale,
                };
                Some((r.utf16_start as i32, look))
            })
            .collect();
        let links: Vec<(i32, u64)> = st
            .runs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let t = runs.get(i).filter(|t| t.chip.is_some())?;
                Some((r.utf16_start as i32, t.link?))
            })
            .collect();
        LINKS.with(|l| l.borrow_mut().insert(self.id, links));
        let had = self.chips.borrow().clone();
        let redo: Vec<usize> = if replaced {
            (0..want.len()).collect()
        } else if had.len() == want.len() {
            (0..want.len()).filter(|&i| had[i] != want[i].1).collect()
        } else {
            Vec::new()
        };
        *self.chips.borrow_mut() = want.iter().map(|(_, l)| l.clone()).collect();
        if redo.is_empty() {
            return Ok(());
        }
        doc.BatchDisplayUpdates()?;
        let r = (|| -> Result<()> {
            for i in redo {
                let (at, look) = &want[i];
                let range: ITextRange = doc.GetRange(*at, at + 1)?.cast()?;
                crate::chips::place(&range, look)?;
            }
            Ok(())
        })();
        doc.ApplyDisplayUpdates()?;
        r
    }
}

/// The document's plain text, `\n` between paragraphs, without RichEdit's
/// final paragraph mark. A chip's picture is U+FFFC (`OBJECT_CHAR`), as in
/// the runs: `UseLf` turns it into a space, which the core took for an
/// edit, so the app's runs lost the chip as soon as it was drawn.
pub(crate) fn text(doc: &RichEditTextDocument) -> Result<String> {
    let mut s = HSTRING::new();
    doc.GetText(TextGetOptions::None, &mut s)?;
    let s = s.to_string();
    let s = s.strip_suffix('\r').unwrap_or(&s);
    // Paragraph marks, and Shift+Return's soft line break, are line breaks.
    Ok(s.replace(['\r', '\u{b}'], "\n"))
}

/// Applies each run's character format to its range, without a redraw
/// in between. Only what differs is set: the echo of typing (already in
/// the style around it) leaves no format step in the undo history.
fn format(
    doc: &RichEditTextDocument,
    st: &StyledText,
    runs: &[TextRun],
    pal: &Palette,
) -> Result<()> {
    doc.BatchDisplayUpdates()?;
    let auto = TextConstants::AutoColor()?;
    let r = (|| -> Result<()> {
        for (i, r) in st.runs.iter().enumerate() {
            let start = r.utf16_start as i32;
            let end = start + r.utf16_len() as i32;
            if end <= start {
                continue;
            }
            let cf = doc.GetRange(start, end)?.CharacterFormat()?;
            // RichEdit takes one face (XAML a fallback list) and points
            // (XAML DIPs: 96 a inch, points 72).
            let family = crate::text::family_name(
                &r.font.family,
                r.font.size,
                r.font.weight.css(),
                r.font.italic,
            );
            let name = h(family.split(',').next().unwrap_or("").trim());
            if cf.Name()? != name {
                cf.SetName(&name)?;
            }
            let size = r.font.size.max(1.) * 0.75;
            if (cf.Size()? - size).abs() > 0.01 {
                cf.SetSize(size)?;
            }
            let weight = r.font.weight.css() as i32;
            if cf.Weight()? != weight {
                cf.SetWeight(weight)?;
            }
            if cf.Italic()? != effect(r.font.italic) {
                cf.SetItalic(effect(r.font.italic))?;
            }
            if cf.Strikethrough()? != effect(r.strikethrough) {
                cf.SetStrikethrough(effect(r.strikethrough))?;
            }
            // A colored underline is drawn over the field (`carets.rs`).
            let line = if crate::underline::drawn(r, true) {
                UnderlineType::None
            } else if r.underline || r.link.is_some() {
                underline(r.underline_style)
            } else {
                UnderlineType::None
            };
            if cf.Underline()? != line {
                cf.SetUnderline(line)?;
            }
            let fg = win_color(pal.ink(r.color.unwrap_or(pal.text)));
            if cf.ForegroundColor()? != fg {
                cf.SetForegroundColor(fg)?;
            }
            // A chip's picture draws its own background.
            let chip = runs.get(i).is_some_and(|t| t.chip.is_some());
            let background = r.background.filter(|_| !chip);
            let bg = background.map_or(auto, |c| win_color(pal.fill(c)));
            if cf.BackgroundColor()? != bg {
                cf.SetBackgroundColor(bg)?;
            }
        }
        Ok(())
    })();
    doc.ApplyDisplayUpdates()?;
    r
}

fn effect(on: bool) -> FormatEffect {
    if on {
        FormatEffect::On
    } else {
        FormatEffect::Off
    }
}

fn underline(s: UnderlineStyle) -> UnderlineType {
    match s {
        UnderlineStyle::Single => UnderlineType::Single,
        UnderlineStyle::Thick => UnderlineType::Thick,
        UnderlineStyle::Double => UnderlineType::Double,
        UnderlineStyle::Dotted => UnderlineType::Dotted,
        UnderlineStyle::Dashed => UnderlineType::Dash,
    }
}
