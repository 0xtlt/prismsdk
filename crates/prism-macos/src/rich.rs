//! Rich inputs (#115): a text area's `NSTextView` drawing the runs of a
//! [`rich_input`](prism_core::rich_input) as attributes, each chip as an
//! `NSTextAttachment` (one character, U+FFFC, that the caret, the selection
//! and Backspace treat as one), and each inline icon as its symbol, an
//! attachment too (#221).
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
    NSBezierPath, NSColor, NSFont, NSImage, NSLayoutManager, NSMutableParagraphStyle,
    NSTextAttachment, NSTextContainer, NSTextView,
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

/// The field's font, color, alignment and line height, for text typed
/// before the app restyles it, and for a plain text area's text (#179).
pub(crate) fn base_attributes(style: &Style) -> Attributes {
    use objc2_app_kit::{
        NSBaselineOffsetAttributeName, NSFontAttributeName, NSForegroundColorAttributeName,
        NSParagraphStyleAttributeName,
    };
    let font = crate::label::ns_font(&prism_native::text::FontSpec {
        family: style.font_family.clone(),
        size: style.font_size,
        weight: style.font_weight,
        italic: style.italic,
        tabular: style.tabular_numbers,
    });
    let color = style.text_color.map_or_else(NSColor::labelColor, ns_color);
    let paragraph = paragraph(style.text_align, style.line_height_points());
    let offset =
        crate::label::baseline_offset(&font, style.line_height_points()).map(NSNumber::new_f64);
    let mut keys: Vec<&NSAttributedStringKey> = unsafe {
        vec![
            NSFontAttributeName,
            NSForegroundColorAttributeName,
            NSParagraphStyleAttributeName,
        ]
    };
    let mut values: Vec<&AnyObject> = vec![&font, &color, &paragraph];
    if let Some(o) = &offset {
        keys.push(unsafe { NSBaselineOffsetAttributeName });
        values.push(o);
    }
    NSDictionary::from_slices(&keys, &values)
}

/// The paragraph style of a field's text: its alignment, and the line
/// height `text()` uses (#179).
fn paragraph(
    align: prism_core::TextAlign,
    line_height: Option<f32>,
) -> Retained<NSMutableParagraphStyle> {
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(match align {
        prism_core::TextAlign::Left => objc2_app_kit::NSTextAlignment::Left,
        prism_core::TextAlign::Center => objc2_app_kit::NSTextAlignment::Center,
        prism_core::TextAlign::Right => objc2_app_kit::NSTextAlignment::Right,
    });
    crate::label::set_line_height(&paragraph, line_height);
    paragraph
}

/// A plain text area's text takes its style's line height and baseline,
/// like `text()` (#179): the whole text and what is typed next.
pub(crate) fn set_plain_style(tv: &NSTextView, style: &Style) {
    let attrs = base_attributes(style);
    if let Some(storage) = unsafe { tv.textStorage() } {
        let all = NSRange::new(0, storage.length());
        storage.beginEditing();
        unsafe { storage.addAttributes_range(&attrs, all) };
        storage.endEditing();
    }
    unsafe { tv.setTypingAttributes(&attrs) };
}

/// One run's characters and attributes: its text, or for a chip the
/// attachment character with the chip's image.
fn piece(s: &StyledRun, run: &TextRun, t: &StyledText) -> (String, Attributes) {
    use objc2_app_kit::{
        NSAttachmentAttributeName, NSBackgroundColorAttributeName, NSFontAttributeName,
        NSForegroundColorAttributeName, NSParagraphStyleAttributeName,
        NSStrikethroughStyleAttributeName, NSUnderlineColorAttributeName,
        NSUnderlineStyleAttributeName,
    };
    let font = crate::label::ns_font(&s.font);
    let color = s.color.map_or_else(NSColor::labelColor, ns_color);
    // Same line height and baseline as `text()` (#179).
    let paragraph = paragraph(t.align, t.line_height);
    let offset = crate::label::baseline_offset(&font, t.line_height).map(NSNumber::new_f64);
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
    // An inline icon (#221): its symbol, or its glyph drawn as an image
    // (it stays one character either way).
    let attachment = attachment.or_else(|| {
        let name = run.icon.as_deref()?;
        let a = NSTextAttachment::new();
        match crate::label::symbol(name, &s.font, &color) {
            Some(image) => a.setImage(Some(&image)),
            None => {
                let clear = NSColor::clearColor();
                let glyph = prism_core::glyphs::glyph(name);
                let (image, bounds) = chip_image(glyph, None, &font, &color, &clear);
                a.setImage(Some(&image));
                a.setBounds(bounds);
            }
        }
        Some(a)
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
    if let Some(o) = &offset {
        keys.push(unsafe { objc2_app_kit::NSBaselineOffsetAttributeName });
        values.push(o);
    }
    if let Some(bg) = &background {
        keys.push(unsafe { NSBackgroundColorAttributeName });
        values.push(bg);
    }
    let underline = NSNumber::new_isize(prism_native::text::ns_underline_style(s.underline_style));
    let underline_color = s.underline_color.map(ns_color);
    if s.underline {
        keys.push(unsafe { NSUnderlineStyleAttributeName });
        values.push(&underline);
        if let Some(c) = &underline_color {
            keys.push(unsafe { NSUnderlineColorAttributeName });
            values.push(c);
        }
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

/// The link id of the clickable chip drawn under `p` (in `tv`'s
/// coordinates) in rich input `tv` (#115). The click is the text view's to
/// read: AppKit tells the delegate of clicks on an attachment's cell
/// (`textView:clickedOnCell:…`), and an image attachment has none.
pub(crate) fn chip_link_at(tv: &PrismTextView, p: NSPoint) -> Option<(u64, usize)> {
    if !tv.ivars().rich.get() {
        return None;
    }
    let (lm, tc) = unsafe { (tv.layoutManager()?, tv.textContainer()?) };
    let origin = tv.textContainerOrigin();
    let local = NSPoint::new(p.x - origin.x, p.y - origin.y);
    char_drawn_at(&lm, &tc, local).and_then(|index| Some((link_at(tv, index)?, index)))
}

/// The UTF-16 offset of the character drawn under `local` (in container
/// `tc`'s coordinates): none between lines or past their end.
fn char_drawn_at(lm: &NSLayoutManager, tc: &NSTextContainer, local: NSPoint) -> Option<usize> {
    let glyph = lm.glyphIndexForPoint_inTextContainer(local, tc);
    let r = lm.boundingRectForGlyphRange_inTextContainer(NSRange::new(glyph, 1), tc);
    let inside = local.x >= r.origin.x
        && local.x < r.origin.x + r.size.width
        && local.y >= r.origin.y
        && local.y < r.origin.y + r.size.height;
    inside.then(|| lm.characterIndexForGlyphAtIndex(glyph))
}

/// The link id of the clickable chip (or icon) at UTF-16 offset `index`
/// of `runs` (one unit each, its attachment character).
fn chip_link(runs: &[TextRun], index: usize) -> Option<u64> {
    let mut at = 0;
    for r in runs {
        let len = r.text.encode_utf16().count();
        if index >= at && index < at + len {
            return r.link.filter(|_| r.is_object());
        }
        at += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #179: a field's text takes the style's line height and the
    /// baseline `text()` uses, so it lines up with the same text shown.
    #[test]
    fn a_field_takes_the_line_height_and_baseline_of_text() {
        use objc2_app_kit::{
            NSBaselineOffsetAttributeName, NSFontAttributeName, NSParagraphStyle,
            NSParagraphStyleAttributeName,
        };
        let style = Style {
            font_size: 16.,
            line_height: prism_core::Length::Px(24.),
            ..Style::default()
        };
        let attrs = base_attributes(&style);
        let p = attrs
            .objectForKey(unsafe { NSParagraphStyleAttributeName })
            .and_then(|p| p.downcast::<NSParagraphStyle>().ok())
            .unwrap();
        assert_eq!((p.minimumLineHeight(), p.maximumLineHeight()), (24., 24.));
        let font = attrs
            .objectForKey(unsafe { NSFontAttributeName })
            .and_then(|f| f.downcast::<NSFont>().ok())
            .unwrap();
        let offset = attrs
            .objectForKey(unsafe { NSBaselineOffsetAttributeName })
            .and_then(|n| n.downcast::<NSNumber>().ok())
            .unwrap()
            .doubleValue();
        assert_eq!(
            Some(offset),
            crate::label::baseline_offset(&font, Some(24.)),
            "the label's offset"
        );
        // No line height: the font's own, nothing raised.
        let plain = base_attributes(&Style {
            font_size: 16.,
            ..Style::default()
        });
        assert!(
            plain
                .objectForKey(unsafe { NSBaselineOffsetAttributeName })
                .is_none()
        );
    }

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

    /// A click on a chip reaches its `on_click`: the text view finds the
    /// chip drawn under the point (an image attachment has no cell, so
    /// AppKit's `textView:clickedOnCell:…` never came), then its link
    /// (`chip_link`).
    #[test]
    fn a_click_finds_the_chip_drawn_under_it() {
        use objc2_app_kit::{NSAttachmentAttributeName, NSTextStorage};
        let storage = NSTextStorage::new();
        storage.setAttributedString(&NSAttributedString::from_nsstring(&NSString::from_str(
            "See \u{fffc}.",
        )));
        let chip = NSTextAttachment::new();
        let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(80., 18.));
        chip.setImage(Some(&image));
        chip.setBounds(NSRect::new(NSPoint::new(0., -4.), NSSize::new(80., 18.)));
        unsafe {
            storage.addAttribute_value_range(NSAttachmentAttributeName, &chip, NSRange::new(4, 1))
        };
        let lm = NSLayoutManager::new();
        storage.addLayoutManager(&lm);
        let tc = NSTextContainer::initWithSize(NSTextContainer::alloc(), NSSize::new(400., 1e6));
        lm.addTextContainer(&tc);
        lm.ensureLayoutForTextContainer(&tc);
        // One glyph a character here (the chip's attachment too).
        let rect = |i: usize| lm.boundingRectForGlyphRange_inTextContainer(NSRange::new(i, 1), &tc);
        let middle = |r: NSRect| {
            NSPoint::new(
                r.origin.x + r.size.width / 2.,
                r.origin.y + r.size.height / 2.,
            )
        };
        let chip_rect = rect(4);
        assert!(
            chip_rect.size.width >= 80.,
            "the chip's width: {chip_rect:?}"
        );
        assert_eq!(
            char_drawn_at(&lm, &tc, middle(chip_rect)),
            Some(4),
            "the chip"
        );
        assert_eq!(
            char_drawn_at(&lm, &tc, middle(rect(1))),
            Some(1),
            "a letter"
        );
        let past = NSPoint::new(300., middle(chip_rect).y);
        assert_eq!(
            char_drawn_at(&lm, &tc, past),
            None,
            "past the end of the line"
        );
        let below = NSPoint::new(middle(chip_rect).x, 200.);
        assert_eq!(char_drawn_at(&lm, &tc, below), None, "under the line");
    }
}
