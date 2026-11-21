//! Rich inputs (#115) on iOS, iPadOS and visionOS: a text area's
//! `UITextView` drawing the runs of a [`rich_input`](prism_core::rich_input)
//! as attributes, each chip and inline icon an `NSTextAttachment` (one
//! character, U+FFFC, that the caret, the selection and Backspace treat as
//! one). tvOS has no text area (`UITextView.editable` is unavailable there).
//!
//! Edits reach the app as the plain text (`UiEvent::Input`, chips
//! included): prism-core gives the typed text the style of the run around
//! it and keeps the other runs. The app's runs come back synchronously,
//! inside the edit: when they hold the text the view shows (the echo of the
//! typing, or the app restyling it), their attributes are applied in place,
//! caret and undo untouched; other runs replace the text, the caret keeping
//! its distance from the end (#73). Nothing changes during a composition
//! (marked text). Without the app's hooks, a copy is plain text with each
//! chip's label and a paste is plain text without chips. A tap on a chip
//! with `on_click` reports it; the caret still moves.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    AnyThread, MainThreadMarker, MainThreadOnly, Message, class, define_class, msg_send, sel,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{
    NSAttributedString, NSAttributedStringKey, NSDictionary, NSMutableAttributedString, NSNumber,
    NSObject, NSObjectProtocol, NSRange, NSString,
};
use objc2_ui_kit::{
    NSMutableParagraphStyle, NSStringDrawing, NSTextAlignment, NSTextAttachment, UIBezierPath,
    UIColor, UIFont, UIGestureRecognizer, UIGestureRecognizerDelegate, UIImage, UIPasteboard,
    UITapGestureRecognizer, UITextInput, UITextView, UITraitCollection,
};
use prism_core::decode_runs;
use prism_core::{NodeId, OBJECT_CHAR, Style, TextAlign, TextRun, TextTransform, UiEvent};
use prism_native::text::{FontSpec, StyledRun, StyledText};

use crate::imp::ui_color;

/// A rich input's view, the app's runs and its style.
struct Rich {
    view: Retained<UITextView>,
    runs: String,
    style: Option<Style>,
}

thread_local! {
    static RICH: RefCell<HashMap<NodeId, Rich>> = RefCell::new(HashMap::new());
    /// The target and delegate of the chips' tap recognizers.
    static TAPS: RefCell<Option<Retained<ChipTaps>>> = const { RefCell::new(None) };
}

/// Makes text area `tv` a rich input: attributes and attachments, no
/// editing of them (styles are the app's), taps on chips reported.
pub fn setup(tv: &UITextView) {
    let id = tv.tag() as NodeId;
    tv.setAllowsEditingTextAttributes(false);
    RICH.with(|r| {
        r.borrow_mut().insert(
            id,
            Rich {
                view: tv.retain(),
                runs: String::new(),
                style: None,
            },
        )
    });
    let taps = TAPS.with(|t| {
        t.borrow_mut()
            .get_or_insert_with(|| ChipTaps::new(tv.mtm()))
            .clone()
    });
    let target: &AnyObject = &taps;
    let tap = unsafe {
        UITapGestureRecognizer::initWithTarget_action(
            UITapGestureRecognizer::alloc(tv.mtm()),
            Some(target),
            Some(sel!(prismChipTap:)),
        )
    };
    tap.setCancelsTouchesInView(false);
    tap.setDelaysTouchesEnded(false);
    tap.setDelegate(Some(ProtocolObject::from_ref(&*taps)));
    tv.addGestureRecognizer(&tap);
}

pub fn forget(id: NodeId) {
    RICH.with(|r| r.borrow_mut().remove(&id));
}

pub fn is_rich(id: NodeId) -> bool {
    RICH.with(|r| r.borrow().contains_key(&id))
}

/// The runs to show (`Widgets::set_rich_value`).
pub fn set_runs(tv: &UITextView, runs: &str) {
    let id = tv.tag() as NodeId;
    let found = RICH.with(|r| {
        r.borrow_mut()
            .get_mut(&id)
            .map(|rich| rich.runs = runs.to_string())
            .is_some()
    });
    if found {
        show(id);
    }
}

/// The field's style (font, color, alignment, line height): the base of
/// the runs, and what is typed until the app restyles it.
pub fn set_style(tv: &UITextView, style: &Style) {
    let id = tv.tag() as NodeId;
    let found = RICH.with(|r| {
        r.borrow_mut()
            .get_mut(&id)
            .map(|rich| rich.style = Some(style.clone()))
            .is_some()
    });
    if found {
        show(id);
    }
}

/// The appearance changed: chips are images, drawn again in its colors.
pub fn appearance_changed() {
    let ids: Vec<NodeId> = RICH.with(|r| r.borrow().keys().copied().collect());
    for id in ids {
        show(id);
    }
}

fn runs_text(runs: &[TextRun]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

/// Draws the runs of rich input `id`: in place when its view shows their
/// text already.
fn show(id: NodeId) {
    let Some((tv, runs, style)) = RICH.with(|r| {
        let r = r.borrow();
        let rich = r.get(&id)?;
        Some((rich.view.clone(), rich.runs.clone(), rich.style.clone()?))
    }) else {
        return;
    };
    // The text the user edits is the value: no case transform.
    let style = Style {
        text_transform: TextTransform::None,
        ..style
    };
    let runs = decode_runs(&runs);
    let text = runs_text(&runs);
    if tv.markedTextRange().is_some() {
        return;
    }
    let styled = StyledText::from_runs(&text, &runs, &style);
    let traits = crate::decor::traits_of(&tv);
    let pieces: Vec<_> = styled
        .runs
        .iter()
        .zip(&runs)
        .map(|(s, r)| piece(s, r, &styled, &traits))
        .collect();
    let storage = tv.textStorage();
    if tv.text().to_string() == text {
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
        crate::forms::set_field_text(&*tv, || tv.setAttributedText(Some(&out)));
    }
    // What the user types next: the field's own font and color (the app's
    // runs come back with the edit and restyle it).
    unsafe { tv.setTypingAttributes(&base_attributes(&style)) };
}

type Attributes = Retained<NSDictionary<NSAttributedStringKey, AnyObject>>;

fn paragraph(align: TextAlign, line_height: Option<f32>) -> Retained<NSMutableParagraphStyle> {
    let p = NSMutableParagraphStyle::new();
    p.setAlignment(match align {
        TextAlign::Left => NSTextAlignment::Left,
        TextAlign::Center => NSTextAlignment::Center,
        TextAlign::Right => NSTextAlignment::Right,
    });
    if let Some(lh) = line_height {
        p.setMinimumLineHeight(lh as f64);
        p.setMaximumLineHeight(lh as f64);
    }
    p
}

/// The field's font, color, alignment and line height, for the text typed
/// before the app restyles it.
fn base_attributes(style: &Style) -> Attributes {
    use objc2_ui_kit::{
        NSBaselineOffsetAttributeName, NSFontAttributeName, NSForegroundColorAttributeName,
        NSParagraphStyleAttributeName,
    };
    let font = crate::label::ui_font(&FontSpec {
        family: style.font_family.clone(),
        size: style.font_size,
        weight: style.font_weight,
        italic: style.italic,
        tabular: style.tabular_numbers,
    });
    let color = style.text_color.map_or_else(UIColor::labelColor, ui_color);
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

/// One run's characters and attributes: its text, or for a chip or an
/// inline icon the attachment character with its image.
fn piece(
    s: &StyledRun,
    run: &TextRun,
    t: &StyledText,
    traits: &UITraitCollection,
) -> (String, Attributes) {
    use objc2_ui_kit::{
        NSAttachmentAttributeName, NSBackgroundColorAttributeName, NSBaselineOffsetAttributeName,
        NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName,
        NSStrikethroughStyleAttributeName, NSUnderlineColorAttributeName,
        NSUnderlineStyleAttributeName,
    };
    let font = crate::label::ui_font(&s.font);
    let color = s.color.map_or_else(UIColor::labelColor, ui_color);
    let paragraph = paragraph(t.align, t.line_height);
    let background = s.background.map(ui_color);
    let attachment = (run.is_object() && run.text == OBJECT_CHAR.to_string()).then(|| {
        let (label, icon, bg) = match &run.chip {
            Some(_) => (
                run.chip_label.as_deref().unwrap_or(""),
                run.chip_icon.as_deref(),
                background
                    .clone()
                    .unwrap_or_else(|| UIColor::systemGrayColor().colorWithAlphaComponent(0.18)),
            ),
            // An inline icon: its symbol alone, or its glyph.
            None => {
                match crate::label::symbol(run.icon.as_deref().unwrap_or(""), &s.font, &color) {
                    Some(_) => ("", run.icon.as_deref(), UIColor::clearColor()),
                    None => (
                        prism_core::glyphs::glyph(run.icon.as_deref().unwrap_or("")),
                        None,
                        UIColor::clearColor(),
                    ),
                }
            }
        };
        let boxed = run.chip.is_some();
        let (image, bounds) = chip_image(label, icon, &s.font, &font, &color, &bg, boxed, traits);
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
            OBJECT_CHAR.to_string(),
            NSDictionary::from_slices(&keys, &values),
        );
    }
    let offset = crate::label::baseline_offset(&font, t.line_height).map(NSNumber::new_f64);
    if let Some(o) = &offset {
        keys.push(unsafe { NSBaselineOffsetAttributeName });
        values.push(o);
    }
    if let Some(bg) = &background {
        keys.push(unsafe { NSBackgroundColorAttributeName });
        values.push(bg);
    }
    let underline = NSNumber::new_isize(prism_native::text::ns_underline_style(s.underline_style));
    let underline_color = s.underline_color.map(ui_color);
    if s.underline {
        keys.push(unsafe { NSUnderlineStyleAttributeName });
        values.push(&underline);
        if let Some(c) = &underline_color {
            keys.push(unsafe { NSUnderlineColorAttributeName });
            values.push(c);
        }
    }
    let one = NSNumber::new_isize(1);
    if s.strikethrough {
        keys.push(unsafe { NSStrikethroughStyleAttributeName });
        values.push(&one);
    }
    (s.text.clone(), NSDictionary::from_slices(&keys, &values))
}

/// A chip as an image: a rounded box (when `boxed`) with its symbol and
/// label, sitting on the baseline like the text around it, in the colors of
/// appearance `traits` (drawn again when it changes).
#[allow(clippy::too_many_arguments)]
fn chip_image(
    label: &str,
    icon: Option<&str>,
    spec: &FontSpec,
    font: &UIFont,
    color: &UIColor,
    bg: &UIColor,
    boxed: bool,
    traits: &UITraitCollection,
) -> (Retained<UIImage>, CGRect) {
    use objc2_ui_kit::{NSFontAttributeName, NSForegroundColorAttributeName};
    let color = color.resolvedColorWithTraitCollection(traits);
    let bg = bg.resolvedColorWithTraitCollection(traits);
    let (f, c): (&AnyObject, &AnyObject) = (font, &color);
    let attrs = unsafe {
        NSDictionary::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[f, c],
        )
    };
    let s = NSString::from_str(label);
    let text = unsafe { s.sizeWithAttributes(Some(&attrs)) };
    let em = unsafe { font.pointSize() };
    let pad = if boxed { (em * 0.35).round() } else { 0. };
    let symbol = icon.and_then(|name| crate::label::symbol(name, spec, &color));
    let icon_w = symbol.as_ref().map_or(0., |img| {
        let size = unsafe { img.size() };
        if size.height > 0. {
            size.width * (em * 0.9 / size.height)
        } else {
            em
        }
    });
    let gap = if symbol.is_some() && !label.is_empty() {
        (em * 0.25).round()
    } else {
        0.
    };
    let descender = unsafe { font.descender() };
    let height = (unsafe { font.ascender() } - descender).ceil();
    let size = CGSize::new(
        (pad * 2. + icon_w + gap + text.width).ceil().max(1.),
        height,
    );
    let draw = block2::RcBlock::new(move |_context: *mut AnyObject| {
        if boxed {
            bg.setFill();
            UIBezierPath::bezierPathWithRoundedRect_cornerRadius(
                CGRect::new(CGPoint::ZERO, size),
                4.,
            )
            .fill();
        }
        let mut x = pad;
        if let Some(img) = &symbol {
            let side = em * 0.9;
            img.drawInRect(CGRect::new(
                CGPoint::new(x, (size.height - side) / 2.),
                CGSize::new(icon_w, side),
            ));
            x += icon_w + gap;
        }
        let y = (size.height - text.height) / 2.;
        unsafe { s.drawAtPoint_withAttributes(CGPoint::new(x, y), Some(&attrs)) };
    });
    let image: Retained<UIImage> = unsafe {
        let renderer: Retained<AnyObject> = msg_send![
            msg_send![class!(UIGraphicsImageRenderer), alloc],
            initWithSize: size
        ];
        msg_send![&*renderer, imageWithActions: &*draw]
    };
    (image, CGRect::new(CGPoint::new(0., descender), size))
}

// --- Copy, paste and taps --------------------------------------------------

/// The runs of rich input `id`, when its view shows their text.
fn shown_runs(id: NodeId, tv: &UITextView) -> Option<Vec<TextRun>> {
    let runs = RICH.with(|r| r.borrow().get(&id).map(|rich| rich.runs.clone()))?;
    let runs = decode_runs(&runs);
    (runs_text(&runs) == tv.text().to_string()).then_some(runs)
}

/// `text` (UTF-16 units `from..to` of the view's text) as copied: each
/// chip as its label, icons and other objects dropped.
fn labelled(text: &str, runs: Option<&[TextRun]>, from: usize, to: usize) -> String {
    let mut out = String::new();
    let mut at = 0;
    for c in text.chars() {
        let len = c.len_utf16();
        if at >= from && at + len <= to {
            if c != OBJECT_CHAR {
                out.push(c);
            } else if let Some(label) = runs.and_then(|r| chip_at(r, at)).and_then(|r| {
                r.chip.as_ref()?;
                r.chip_label.as_deref()
            }) {
                out.push_str(label);
            }
        }
        at += len;
    }
    out
}

/// The run holding UTF-16 unit `index`.
fn chip_at(runs: &[TextRun], index: usize) -> Option<&TextRun> {
    let mut at = 0;
    for r in runs {
        let len = r.text.encode_utf16().count();
        if index >= at && index < at + len {
            return Some(r);
        }
        at += len;
    }
    None
}

/// Copy or cut in a rich input the app does not take: plain text, each
/// chip as its label (`false`: not a rich input, or nothing selected, so
/// UIKit's own copy runs and a cut deletes nothing).
pub fn copy(tv: &UITextView) -> bool {
    let id = tv.tag() as NodeId;
    if !is_rich(id) {
        return false;
    }
    let Some(range) = tv.selectedTextRange() else {
        return false;
    };
    let begin = tv.beginningOfDocument();
    let from = tv
        .offsetFromPosition_toPosition(&begin, &range.start())
        .max(0) as usize;
    let to = tv
        .offsetFromPosition_toPosition(&begin, &range.end())
        .max(0) as usize;
    if from >= to {
        return false;
    }
    let text = tv.text().to_string();
    let runs = shown_runs(id, tv);
    let copied = labelled(&text, runs.as_deref(), from, to);
    unsafe { UIPasteboard::generalPasteboard().setString(Some(&NSString::from_str(&copied))) };
    true
}

/// Paste in a rich input the app does not take: the pasteboard's plain
/// text, without object characters (`false`: not a rich input).
pub fn paste(tv: &UITextView) -> bool {
    if !is_rich(tv.tag() as NodeId) {
        return false;
    }
    let text = unsafe { UIPasteboard::generalPasteboard().string() }
        .map(|s| s.to_string())
        .unwrap_or_default()
        .replace(OBJECT_CHAR, "");
    if !text.is_empty() {
        objc2_ui_kit::UIKeyInput::insertText(tv, &NSString::from_str(&text));
    }
    true
}

/// The link of the clickable chip (or icon) under `point` of `tv`.
fn tapped_link(tv: &UITextView, point: CGPoint) -> Option<u64> {
    let runs = shown_runs(tv.tag() as NodeId, tv)?;
    let inset = tv.textContainerInset();
    let p = CGPoint::new(point.x - inset.left, point.y - inset.top);
    let layout = tv.layoutManager();
    let container = tv.textContainer();
    let glyph = unsafe {
        layout.glyphIndexForPoint_inTextContainer_fractionOfDistanceThroughGlyph(
            p,
            &container,
            std::ptr::null_mut(),
        )
    };
    let rect = layout.boundingRectForGlyphRange_inTextContainer(NSRange::new(glyph, 1), &container);
    let inside = p.x >= rect.origin.x
        && p.x <= rect.origin.x + rect.size.width
        && p.y >= rect.origin.y
        && p.y <= rect.origin.y + rect.size.height;
    if !inside {
        return None;
    }
    let index = layout.characterIndexForGlyphAtIndex(glyph);
    chip_at(&runs, index)
        .filter(|r| r.is_object())
        .and_then(|r| r.link)
}

define_class!(
    /// Taps on chips: the recognizers' target, recognizing with the text
    /// view's own (the caret still moves).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismChipTaps"]
    struct ChipTaps;

    unsafe impl NSObjectProtocol for ChipTaps {}

    unsafe impl UIGestureRecognizerDelegate for ChipTaps {
        #[unsafe(method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:))]
        fn simultaneous(&self, _a: &UIGestureRecognizer, _b: &UIGestureRecognizer) -> bool {
            true
        }
    }

    impl ChipTaps {
        #[unsafe(method(prismChipTap:))]
        fn tap(&self, tap: &UITapGestureRecognizer) {
            let Some(view) = tap.view() else { return };
            let Some(tv) = view.downcast_ref::<UITextView>() else {
                return;
            };
            let point = tap.locationInView(Some(tv));
            if let Some(link) = tapped_link(tv, point) {
                crate::imp::dispatch(UiEvent::MenuSelect(link));
            }
        }
    }
);

impl ChipTaps {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_writes_each_chip_as_its_label() {
        let runs = vec![
            TextRun {
                text: "See ".into(),
                ..TextRun::default()
            },
            TextRun {
                text: OBJECT_CHAR.to_string(),
                chip: Some("page:1".into()),
                chip_label: Some("Roadmap".into()),
                link: Some(3),
                ..TextRun::default()
            },
            TextRun {
                text: OBJECT_CHAR.to_string(),
                icon: Some("star".into()),
                ..TextRun::default()
            },
            TextRun {
                text: " 😀".into(),
                ..TextRun::default()
            },
        ];
        let text = runs_text(&runs);
        let all = text.encode_utf16().count();
        assert_eq!(labelled(&text, Some(&runs), 0, all), "See Roadmap 😀");
        // Part of it, in UTF-16 units: the chip alone.
        assert_eq!(labelled(&text, Some(&runs), 4, 5), "Roadmap");
        // Without runs (stale), objects are dropped.
        assert_eq!(labelled(&text, None, 0, all), "See  😀");
        assert_eq!(chip_at(&runs, 4).and_then(|r| r.link), Some(3));
        assert_eq!(chip_at(&runs, 5).and_then(|r| r.link), None);
    }
}
