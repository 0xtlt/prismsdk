//! Rich inputs (#115): a text area's `NSTextView` drawing the runs of a
//! [`rich_input`](prism_core::rich_input) as attributes, each chip as an
//! `NSTextAttachment` (one character, U+FFFC, that the caret, the selection
//! and Backspace treat as one).
//!
//! Edits reach the app as the plain text (`UiEvent::Input`): prism-core
//! gives the typed text the style of the run around it and keeps the other
//! runs. The app's runs come back synchronously, in the same turn of the
//! run loop: when they hold the text the view shows (the echo of the
//! typing, or the app restyling it), their attributes are applied in place,
//! caret and undo untouched; other runs replace the text, the caret keeping
//! its distance from the end (#73). Nothing changes during an IME
//! composition (marked text): its attributes land with the next edit.
//! Pastes are plain text; Return calls `on_submit`, Shift+Return inserts a
//! line break.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{AnyThread, DefinedClass, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSFont, NSImage, NSMutableParagraphStyle, NSTextAttachment, NSTextView,
};
use objc2_foundation::{
    NSAttributedString, NSAttributedStringKey, NSDictionary, NSMutableAttributedString, NSNumber,
    NSPoint, NSRange, NSRect, NSSize, NSString,
};
use prism_core::{Style, TextRun, TextTransform, decode_runs};
use prism_native::text::{StyledRun, StyledText};

use crate::imp::{PrismTextView, ns_color};

/// Sets `tv` up as a rich input: attributes and attachments, no font panel
/// or ruler (styles are the app's).
pub(crate) fn setup(tv: &PrismTextView) {
    tv.ivars().rich.set(true);
    tv.setRichText(true);
    tv.setImportsGraphics(false);
    tv.setUsesFontPanel(false);
    tv.setUsesRuler(false);
}

/// The runs to show (`Widgets::set_rich_value`).
pub(crate) fn set_runs(tv: &PrismTextView, runs: &str) {
    *tv.ivars().runs.borrow_mut() = runs.to_string();
    show(tv);
}

/// The field's style changed (font, color, alignment): keep it for the runs.
pub(crate) fn set_style(tv: &PrismTextView, style: &Style) {
    *tv.ivars().style.borrow_mut() = Some(style.clone());
    show(tv);
}

/// The text the runs hold (a chip is its U+FFFC).
fn runs_text(runs: &[TextRun]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// Draws the runs in `tv`: in place when it shows their text already.
pub(crate) fn show(tv: &PrismTextView) {
    let iv = tv.ivars();
    let runs = decode_runs(&iv.runs.borrow());
    let Some(style) = iv.style.borrow().clone() else {
        return;
    };
    // The text the user edits is the value: no case transform.
    let style = Style {
        text_transform: TextTransform::None,
        ..style
    };
    let text = runs_text(&runs);
    let styled = StyledText::from_runs(&text, &runs, &style);
    let marked: bool = unsafe { msg_send![tv, hasMarkedText] };
    if marked {
        return;
    }
    let Some(storage) = (unsafe { tv.textStorage() }) else {
        return;
    };
    let pieces: Vec<_> = styled
        .runs
        .iter()
        .zip(&runs)
        .map(|(s, r)| piece(s, r, &styled))
        .collect();
    let current = tv.string().to_string();
    if current == text {
        // The echo of the typing, or the app restyling it: same characters.
        storage.beginEditing();
        for (s, (_, attrs)) in styled.runs.iter().zip(&pieces) {
            let range = NSRange::new(s.utf16_start, s.utf16_len());
            unsafe { storage.setAttributes_range(Some(attrs), range) };
        }
        storage.endEditing();
    } else {
        let out = NSMutableAttributedString::new();
        for (s, attrs) in &pieces {
            let a = unsafe {
                NSAttributedString::initWithString_attributes(
                    NSAttributedString::alloc(),
                    &NSString::from_str(s),
                    Some(attrs),
                )
            };
            out.appendAttributedString(&a);
        }
        let old = tv.string().length();
        let r = tv.selectedRange();
        let caret = prism_native::fields::caret_from_end(old, r.location + r.length, out.length());
        storage.setAttributedString(&out);
        tv.setSelectedRange(NSRange::new(caret, 0));
        tv.breakUndoCoalescing();
    }
    // What the user types next: the field's own font and color (the app's
    // runs come back with the edit and restyle it).
    unsafe { tv.setTypingAttributes(&base_attributes(&style)) };
    tv.setNeedsDisplay(true);
}

type Attributes = Retained<NSDictionary<NSAttributedStringKey, AnyObject>>;

/// The field's font and color, for text typed before the app restyles it.
fn base_attributes(style: &Style) -> Attributes {
    use objc2_app_kit::{NSFontAttributeName, NSForegroundColorAttributeName};
    let font = crate::label::ns_font(&prism_native::text::FontSpec {
        family: style.font_family.clone(),
        size: style.font_size,
        weight: style.font_weight,
        italic: style.italic,
        tabular: style.tabular_numbers,
    });
    let color = style.text_color.map_or_else(NSColor::labelColor, ns_color);
    let (f, c): (&AnyObject, &AnyObject) = (&font, &color);
    unsafe {
        NSDictionary::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[f, c],
        )
    }
}

/// One run's characters and attributes: its text, or for a chip the
/// attachment character with the chip's image.
fn piece(s: &StyledRun, run: &TextRun, t: &StyledText) -> (String, Attributes) {
    use objc2_app_kit::{
        NSAttachmentAttributeName, NSBackgroundColorAttributeName, NSFontAttributeName,
        NSForegroundColorAttributeName, NSParagraphStyleAttributeName,
        NSStrikethroughStyleAttributeName, NSUnderlineStyleAttributeName,
    };
    let font = crate::label::ns_font(&s.font);
    let color = s.color.map_or_else(NSColor::labelColor, ns_color);
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(match t.align {
        prism_core::TextAlign::Left => objc2_app_kit::NSTextAlignment::Left,
        prism_core::TextAlign::Center => objc2_app_kit::NSTextAlignment::Center,
        prism_core::TextAlign::Right => objc2_app_kit::NSTextAlignment::Right,
    });
    let one = NSNumber::new_isize(1);
    let background = s.background.map(ns_color);
    let attachment = run.chip.is_some().then(|| {
        let bg = background
            .clone()
            .unwrap_or_else(|| NSColor::systemGrayColor().colorWithAlphaComponent(0.18));
        let (image, bounds) = chip_image(
            run.chip_label.as_deref().unwrap_or(""),
            run.chip_icon.as_deref(),
            &font,
            &color,
            &bg,
        );
        let a = NSTextAttachment::new();
        a.setImage(Some(&image));
        a.setBounds(bounds);
        a
    });
    let mut keys: Vec<&NSAttributedStringKey> = unsafe {
        vec![
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let mut values: Vec<&AnyObject> = vec![&font, &color, &paragraph];
    if let Some(a) = &attachment {
        keys.push(unsafe { NSAttachmentAttributeName });
        values.push(a);
        return (
            prism_core::OBJECT_CHAR.to_string(),
            NSDictionary::from_slices(&keys, &values),
        );
    }
    if let Some(bg) = &background {
        keys.push(unsafe { NSBackgroundColorAttributeName });
        values.push(bg);
    }
    if s.underline {
        keys.push(unsafe { NSUnderlineStyleAttributeName });
        values.push(&one);
    }
    if s.strikethrough {
        keys.push(unsafe { NSStrikethroughStyleAttributeName });
        values.push(&one);
    }
    (s.text.clone(), NSDictionary::from_slices(&keys, &values))
}

/// A chip as an image: a rounded box with its symbol and label, sitting on
/// the baseline like the text around it. Redrawn on appearance changes
/// (dark mode), since it is drawn by a handler.
fn chip_image(
    label: &str,
    icon: Option<&str>,
    font: &NSFont,
    color: &NSColor,
    bg: &NSColor,
) -> (Retained<NSImage>, NSRect) {
    use objc2_app_kit::{NSFontAttributeName, NSForegroundColorAttributeName, NSStringDrawing};
    let (f, c): (&AnyObject, &AnyObject) = (font, color);
    let attrs = unsafe {
        NSDictionary::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[f, c],
        )
    };
    let s = NSString::from_str(label);
    let text = unsafe { s.sizeWithAttributes(Some(&attrs)) };
    let em = font.pointSize();
    let pad = (em * 0.35).round();
    let symbol = icon.and_then(|name| crate::label::symbol_image(name, font, color));
    let icon_w = symbol.as_ref().map_or(0., |_| em);
    let gap = if symbol.is_some() {
        (em * 0.25).round()
    } else {
        0.
    };
    let height = (font.ascender() - font.descender()).ceil();
    let size = NSSize::new((pad * 2. + icon_w + gap + text.width).ceil(), height);
    let descender = font.descender();
    let image = {
        let (s, attrs, bg, symbol) = (
            s.clone(),
            attrs.clone(),
            objc2::Message::retain(bg),
            symbol.clone(),
        );
        let handler = block2::RcBlock::new(move |rect: NSRect| -> Bool {
            bg.setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 4., 4.).fill();
            let mut x = pad;
            if let Some(img) = &symbol {
                let side = em * 0.9;
                let at = NSRect::new(
                    NSPoint::new(x, (rect.size.height - side) / 2.),
                    NSSize::new(side, side),
                );
                img.drawInRect(at);
                x += icon_w + gap;
            }
            let y = (rect.size.height - text.height) / 2.;
            unsafe { s.drawAtPoint_withAttributes(NSPoint::new(x, y), Some(&attrs)) };
            Bool::YES
        });
        NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler)
    };
    image.setAccessibilityDescription(Some(&NSString::from_str(label)));
    (image, NSRect::new(NSPoint::new(0., descender), size))
}

/// Whether `tv` is a rich input.
pub(crate) fn is_rich(tv: &NSTextView) -> bool {
    tv.downcast_ref::<PrismTextView>()
        .is_some_and(|p| p.ivars().rich.get())
}

/// The link id of the chip at UTF-16 offset `index` of rich input `tv`.
pub(crate) fn link_at(tv: &NSTextView, index: usize) -> Option<u64> {
    let tv = tv.downcast_ref::<PrismTextView>()?;
    chip_link(&decode_runs(&tv.ivars().runs.borrow()), index)
}

/// The link id of the clickable chip at UTF-16 offset `index` of `runs`
/// (a chip is one unit, its attachment character).
fn chip_link(runs: &[TextRun], index: usize) -> Option<u64> {
    let mut at = 0;
    for r in runs {
        let len = r.text.encode_utf16().count();
        if index >= at && index < at + len {
            return r.link.filter(|_| r.chip.is_some());
        }
        at += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chip_is_one_attachment_character_and_reports_its_link() {
        let runs = decode_runs(
            "a😀\u{1e}\u{fffc}\u{1f}k=page:1\u{1f}n=Road\u{1f}l=9\u{1e}b\u{1e}\u{fffc}\u{1f}k=p2\u{1f}n=X",
        );
        // "a😀" is three UTF-16 units: the chip is the fourth.
        assert_eq!(chip_link(&runs, 3), Some(9));
        assert_eq!(chip_link(&runs, 2), None, "inside the emoji");
        assert_eq!(chip_link(&runs, 4), None, "plain text");
        assert_eq!(chip_link(&runs, 5), None, "a chip without on_click");
        // The styled runs (what the view lays out) agree: one unit per chip.
        let styled = StyledText::from_runs("", &runs, &Style::default());
        assert_eq!(
            styled
                .runs
                .iter()
                .map(|r| r.utf16_start)
                .collect::<Vec<_>>(),
            [0, 3, 4, 5]
        );
        assert_eq!(runs_text(&runs).encode_utf16().count(), 6);
    }
}
