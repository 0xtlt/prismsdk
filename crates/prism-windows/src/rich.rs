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
//! are plain text. A chip (#115) is its object character, highlighted.

use std::cell::RefCell;

use prism_core::{NodeId, Style, TextRun, TextTransform, UiEvent, UnderlineStyle, decode_runs};
use prism_native::text::StyledText;
use windows::System::VirtualKey;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
use windows_core::{HSTRING, Interface, Result};

use crate::fluent::Palette;
use crate::imp::dispatch;
use crate::winui::bindings::Microsoft::UI::Text::{
    FormatEffect, RichEditTextDocument, TextConstants, TextGetOptions, TextSetOptions,
    UnderlineType,
};
use crate::winui::xaml::Controls::{RichEditBox, TextControlPasteEventHandler};
use crate::winui::xaml::Input::KeyEventHandler;
use crate::winui::xaml::{RoutedEventHandler, TextWrapping};
use crate::winui::{h, win_color};

/// `CF_UNICODETEXT`: pastes as plain text.
const PLAIN_TEXT: i32 = 13;

/// A rich input node's editor and the runs it shows.
pub struct Rich {
    pub element: RichEditBox,
    /// The text the app asked for (`set_text`), shown with the next runs.
    replace: RefCell<Option<String>>,
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
                crate::imp::typed(id, text(&r.Document()?)?);
            }
            Ok(())
        }))?;
        r.SelectionChanged(&RoutedEventHandler::new(move |s, _| {
            if let Some(r) = s.as_ref().and_then(|s| s.cast::<RichEditBox>().ok()) {
                let doc = r.Document()?;
                let sel = doc.Selection()?;
                let (a, b) = (sel.StartPosition()?, sel.EndPosition()?);
                crate::imp::selected(id, &text(&doc)?, a.max(0) as usize, b.max(0) as usize);
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
                r.Document()?.Selection()?.Paste(PLAIN_TEXT)?;
            }
            Ok(())
        }))?;
        Ok(Rich {
            element: r,
            replace: RefCell::new(None),
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

    /// Shows `runs` (`set_rich_value`) in `style`, its colors from `pal`.
    pub fn set_runs(&self, runs: &str, style: &Style, pal: &Palette) -> Result<()> {
        let runs: Vec<TextRun> = decode_runs(runs);
        let shown: String = runs.iter().map(|r| r.text.as_str()).collect();
        let doc = self.element.Document()?;
        let current = text(&doc)?;
        let replace = self.replace.borrow_mut().take();
        if shown != current {
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
        format(
            &doc,
            &StyledText::from_runs(&shown, &runs, &style),
            &runs,
            pal,
        )
    }
}

/// The document's plain text, `\n` between paragraphs, without RichEdit's
/// final paragraph mark.
fn text(doc: &RichEditTextDocument) -> Result<String> {
    let mut s = HSTRING::new();
    doc.GetText(TextGetOptions::UseLf, &mut s)?;
    // Shift+Return's soft line break is a line break too.
    Ok(s.to_string().replace('\u{b}', "\n"))
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
            let name = h(&crate::text::family_name(&r.font.family, r.font.size));
            if cf.Name()? != name {
                cf.SetName(&name)?;
            }
            let size = r.font.size.max(1.);
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
            let line = if r.underline || r.link.is_some() {
                underline(r.underline_style)
            } else {
                UnderlineType::None
            };
            if cf.Underline()? != line {
                cf.SetUnderline(line)?;
            }
            let fg = r.color.map_or(auto, |c| win_color(pal.resolve(c)));
            if cf.ForegroundColor()? != fg {
                cf.SetForegroundColor(fg)?;
            }
            let chip = runs.get(i).is_some_and(|t| t.chip.is_some());
            let background = r
                .background
                .or_else(|| chip.then(|| pal.accent.with_alpha(0.2)));
            let bg = background.map_or(auto, |c| win_color(pal.resolve(c)));
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
