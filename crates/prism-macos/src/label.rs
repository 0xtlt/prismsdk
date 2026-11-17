//! Text nodes: `PrismLabel`, an `NSTextField` label that draws the node's
//! typography (family, weight, italic, line height, letter spacing, case,
//! decoration, tabular digits, truncation) and rich-text runs as one
//! attributed string, so `cellSizeForBounds` measures exactly what is drawn.
//!
//! Inline links (and a clickable text) react to clicks, take keyboard focus
//! with Full Keyboard Access (Tab moves from link to link, Space or Return
//! activates), and are VoiceOver actions of the paragraph, which reads as one
//! element.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_app_kit::{
    NSAccessibilityCustomAction, NSApplication, NSBezierPath, NSColor, NSControl, NSEvent, NSFont,
    NSFontDescriptor, NSFontDescriptorSymbolicTraits, NSFontManager, NSFontTraitMask, NSImage,
    NSImageSymbolConfiguration, NSLayoutManager, NSLineBreakMode, NSLineBreakStrategy,
    NSMutableParagraphStyle, NSResponder, NSTextAlignment, NSTextAttachment, NSTextContainer,
    NSTextField, NSTextStorage, NSView,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSNumber, NSObject,
    NSPoint, NSRange, NSRect, NSSize, NSString,
};
use prism_core::{FontFamily, NodeId, Style, TextAlign, TextWrap, UiEvent};
use prism_native::text::{FontSpec, StyledText, interpolate_weight};

use crate::imp::{dispatch, ns_color};

pub struct LabelIvars {
    id: Cell<NodeId>,
    clickable: Cell<bool>,
    text: RefCell<String>,
    /// Encoded runs (`prism_core::spans`), empty for plain text.
    runs: RefCell<String>,
    style: RefCell<Style>,
    /// What is drawn (links, for clicks and focus).
    styled: RefCell<Option<StyledText>>,
    /// The link that has keyboard focus (index into the links).
    focused: Cell<Option<usize>>,
}

define_class!(
    #[unsafe(super(NSTextField, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = LabelIvars]
    pub struct PrismLabel;

    impl PrismLabel {

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            self.interactive()
        }

        /// Like native buttons and links: reachable with Tab when the user
        /// enabled Keyboard Navigation.
        #[unsafe(method(canBecomeKeyView))]
        fn can_become_key_view(&self) -> bool {
            self.interactive()
                && NSApplication::sharedApplication(self.mtm()).isFullKeyboardAccessEnabled()
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // Reached with Tab: the first link (Shift-Tab: the last) shows
            // focus. A click focuses the text without highlighting a link.
            let links = self.links().len();
            let key = NSApplication::sharedApplication(self.mtm())
                .currentEvent()
                // `keyCode` raises on anything but key events.
                .filter(|e| e.r#type() == objc2_app_kit::NSEventType::KeyDown);
            if let Some(e) = key.filter(|_| links > 0) {
                let back = e.keyCode() == 48
                    && e.modifierFlags()
                        .contains(objc2_app_kit::NSEventModifierFlags::Shift);
                self.focus_link(Some(if back { links - 1 } else { 0 }));
            }
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.focus_link(None);
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let code = event.keyCode();
            let links = self.links();
            let shift = event
                .modifierFlags()
                .contains(objc2_app_kit::NSEventModifierFlags::Shift);
            match code {
                // Tab: the next (previous) link, then the next key view.
                48 if !links.is_empty() => {
                    let at = self.ivars().focused.get().unwrap_or(0);
                    let next = if shift { at.checked_sub(1) } else { Some(at + 1) };
                    match next.filter(|n| *n < links.len()) {
                        Some(n) => self.focus_link(Some(n)),
                        None => {
                            if let Some(w) = self.window() {
                                if shift {
                                    w.selectPreviousKeyView(None);
                                } else {
                                    w.selectNextKeyView(None);
                                }
                            }
                        }
                    }
                }
                // Space, Return, Enter.
                49 | 36 | 76 => {
                    match self.ivars().focused.get().and_then(|i| links.get(i)) {
                        Some((_, link)) => dispatch(UiEvent::MenuSelect(*link)),
                        None if self.ivars().clickable.get() => {
                            dispatch(UiEvent::Click(self.ivars().id.get()))
                        }
                        None => unsafe { msg_send![super(self), keyDown: event] },
                    }
                }
                _ => unsafe { msg_send![super(self), keyDown: event] },
            }
        }

        /// Tracks the mouse until it is released (like a control): a click
        /// activates the link under it, or the clickable text.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.interactive() {
                return unsafe { msg_send![super(self), mouseDown: event] };
            }
            let Some(window) = self.window() else { return };
            let mask = objc2_app_kit::NSEventMask::LeftMouseUp
                | objc2_app_kit::NSEventMask::LeftMouseDragged;
            while let Some(e) = window.nextEventMatchingMask(mask) {
                if e.r#type() == objc2_app_kit::NSEventType::LeftMouseUp {
                    self.click_at(&e);
                    break;
                }
            }
        }

        /// VoiceOver "press": a clickable text, or its only link.
        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_perform_press(&self) -> Bool {
            if self.ivars().clickable.get() {
                dispatch(UiEvent::Click(self.ivars().id.get()));
                return Bool::YES;
            }
            match &self.links()[..] {
                [(_, link)] => {
                    dispatch(UiEvent::MenuSelect(*link));
                    Bool::YES
                }
                _ => Bool::NO,
            }
        }

        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_mask_bounds(&self) -> NSRect {
            self.bounds()
        }

        /// The full text as a tooltip, only while it is cut short
        /// (`tooltip_when_truncated`, #216): asked as the pointer rests.
        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        fn string_for_tool_tip(
            &self,
            _view: &NSView,
            _tag: isize,
            _point: NSPoint,
            _data: *mut std::ffi::c_void,
        ) -> Retained<NSString> {
            if self.truncated() {
                NSString::from_str(&self.ivars().text.borrow())
            } else {
                NSString::new()
            }
        }

        /// The text's box (background, border, rounding) under the text,
        /// which is drawn inside the padding (#14).
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            let style = self.ivars().style.borrow().clone();
            crate::padding::draw_box(self, &style);
            let inner = crate::padding::inset_rect(self, self.bounds(), style.padding);
            match self.cell() {
                Some(cell) if !style.padding.is_zero() => cell.drawWithFrame_inView(inner, self),
                _ => unsafe { msg_send![super(self), drawRect: dirty] },
            }
        }

        #[unsafe(method(drawFocusRingMask))]
        fn draw_focus_ring_mask(&self) {
            NSBezierPath::fillRect(self.bounds());
        }
    }
);

impl PrismLabel {
    /// Whether the text is cut short (#216): wider than its box on one
    /// line, or taller than its line clamp lets it be.
    pub(crate) fn truncated(&self) -> bool {
        let Some(cell) = self.cell() else {
            return false;
        };
        let padding = self.ivars().style.borrow().padding;
        let inner = crate::padding::inset_rect(self, self.bounds(), padding);
        let (max_lines, no_wrap) = self
            .ivars()
            .styled
            .borrow()
            .as_ref()
            .map_or((None, false), |t| (t.max_lines, t.no_wrap));
        if max_lines == Some(1) || no_wrap {
            return cell.cellSize().width > inner.size.width + 0.5;
        }
        if max_lines.is_none() {
            return false;
        }
        // The height of every line, without the clamp.
        let clamp = self.maximumNumberOfLines();
        self.setMaximumNumberOfLines(0);
        let all = cell.cellSizeForBounds(NSRect::new(
            NSPoint::ZERO,
            objc2_foundation::NSSize::new(inner.size.width, 1e7),
        ));
        self.setMaximumNumberOfLines(clamp);
        all.height > inner.size.height + 0.5
    }

    /// A click released at `event`: the link under it, or the text.
    fn click_at(&self, event: &NSEvent) {
        let p = self.convertPoint_fromView(event.locationInWindow(), None);
        let b = self.bounds();
        if p.x < 0. || p.y < 0. || p.x > b.size.width || p.y > b.size.height {
            return;
        }
        if let Some(link) = self.link_at(p) {
            dispatch(UiEvent::MenuSelect(link));
        } else if self.ivars().clickable.get() {
            dispatch(UiEvent::Click(self.ivars().id.get()));
        }
    }

    fn interactive(&self) -> bool {
        self.ivars().clickable.get() || !self.links().is_empty()
    }

    fn links(&self) -> Vec<(String, u64)> {
        self.ivars()
            .styled
            .borrow()
            .as_ref()
            .map(StyledText::links)
            .unwrap_or_default()
    }

    fn focus_link(&self, index: Option<usize>) {
        if self.ivars().focused.get() != index {
            self.ivars().focused.set(index);
            self.refresh();
        }
    }

    /// The link under `p` (view coordinates): the text is laid out again
    /// with TextKit in the cell's text rectangle.
    fn link_at(&self, p: NSPoint) -> Option<u64> {
        let styled = self.ivars().styled.borrow();
        let styled = styled.as_ref().filter(|s| s.has_links())?;
        let cell = self.cell()?;
        let padding = self.ivars().style.borrow().padding;
        let inner = crate::padding::inset_rect(self, self.bounds(), padding);
        let rect = cell.titleRectForBounds(inner);
        let attributed = self.attributedStringValue();
        let storage = NSTextStorage::new();
        storage.setAttributedString(&attributed);
        let layout = NSLayoutManager::new();
        let container = NSTextContainer::initWithSize(
            NSTextContainer::alloc(),
            NSSize::new(rect.size.width, 1e7),
        );
        container.setLineFragmentPadding(0.);
        if let Some(n) = styled.max_lines {
            container.setMaximumNumberOfLines(n as usize);
        }
        layout.addTextContainer(&container);
        storage.addLayoutManager(&layout);
        layout.ensureLayoutForTextContainer(&container);
        let x = p.x - rect.origin.x;
        let y = if self.isFlipped() {
            p.y - rect.origin.y
        } else {
            rect.origin.y + rect.size.height - p.y
        };
        let used = layout.usedRectForTextContainer(&container);
        if x < 0. || y < 0. || x > used.size.width + 1. || y > used.size.height + 1. {
            return None;
        }
        let glyph = layout.glyphIndexForPoint_inTextContainer(NSPoint::new(x, y), &container);
        let index = layout.characterIndexForGlyphAtIndex(glyph);
        styled.link_at_utf16(index)
    }

    /// Draws the text again from its text, runs and style.
    fn refresh(&self) {
        let iv = self.ivars();
        let styled = StyledText::new(&iv.text.borrow(), &iv.runs.borrow(), &iv.style.borrow());
        apply(self, &styled, iv.focused.get());
        // VoiceOver reads one paragraph; its links are actions of it.
        let actions: Vec<Retained<NSAccessibilityCustomAction>> = styled
            .links()
            .into_iter()
            .map(|(name, link)| {
                let handler = block2::RcBlock::new(move || {
                    dispatch(UiEvent::MenuSelect(link));
                    Bool::YES
                });
                NSAccessibilityCustomAction::initWithName_handler(
                    NSAccessibilityCustomAction::alloc(),
                    &NSString::from_str(name.trim()),
                    Some(&handler),
                )
            })
            .collect();
        let actions = (!actions.is_empty()).then(|| NSArray::from_retained_slice(&actions));
        unsafe {
            let _: () = msg_send![self, setAccessibilityCustomActions: actions.as_deref()];
        }
        *iv.styled.borrow_mut() = Some(styled);
    }
}

/// A text node's label.
pub fn new_label(mtm: MainThreadMarker, id: NodeId) -> Retained<PrismLabel> {
    let this = PrismLabel::alloc(mtm).set_ivars(LabelIvars {
        id: Cell::new(id),
        clickable: Cell::new(false),
        text: RefCell::new(String::new()),
        runs: RefCell::new(String::new()),
        style: RefCell::new(Style::default()),
        styled: RefCell::new(None),
        focused: Cell::new(None),
    });
    let l: Retained<PrismLabel> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
    // What `wrappingLabelWithString:` sets up.
    l.setEditable(false);
    l.setSelectable(false);
    l.setBezeled(false);
    l.setBordered(false);
    l.setDrawsBackground(false);
    l.setTag(id as isize);
    if let Some(cell) = l.cell() {
        cell.setWraps(true);
        cell.setScrollable(false);
    }
    l.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    l.refresh();
    l
}

pub fn set_text(l: &PrismLabel, text: &str) {
    *l.ivars().text.borrow_mut() = text.to_string();
    l.refresh();
}

pub fn set_runs(l: &PrismLabel, runs: &str) {
    *l.ivars().runs.borrow_mut() = runs.to_string();
    l.ivars().focused.set(None);
    l.refresh();
}

pub fn set_style(l: &PrismLabel, style: &Style) {
    *l.ivars().style.borrow_mut() = style.clone();
    l.refresh();
    // Its box and padding are drawn by `drawRect:`.
    NSView::setNeedsDisplay(l, true);
}

pub fn set_clickable(l: &PrismLabel, clickable: bool) {
    l.ivars().clickable.set(clickable);
}

fn alignment(a: TextAlign) -> NSTextAlignment {
    match a {
        TextAlign::Left => NSTextAlignment::Left,
        TextAlign::Center => NSTextAlignment::Center,
        TextAlign::Right => NSTextAlignment::Right,
    }
}

/// `NSFontWeight` for any CSS weight: the AppKit constants at 100 … 900,
/// interpolated in between (the system font is variable).
pub fn ns_weight(w: prism_core::FontWeight) -> f64 {
    use objc2_app_kit::*;
    let steps = unsafe {
        [
            NSFontWeightUltraLight,
            NSFontWeightThin,
            NSFontWeightLight,
            NSFontWeightRegular,
            NSFontWeightMedium,
            NSFontWeightSemibold,
            NSFontWeightBold,
            NSFontWeightHeavy,
            NSFontWeightBlack,
        ]
    };
    interpolate_weight(w, steps)
}

/// The font of a run: the system font in its design (monospaced, serif,
/// rounded) or a family by name (installed or registered from the app's
/// `assets/fonts`), italic, with tabular digits.
pub fn ns_font(f: &FontSpec) -> Retained<NSFont> {
    let size = f.size as f64;
    let system = || NSFont::systemFontOfSize_weight(size, ns_weight(f.weight));
    let font = match &f.family {
        FontFamily::Named(name) => {
            // A run-time font's alias (`cx.load_font`) → its file's family.
            let family = prism_native::fonts::resolve_name(name);
            let Some(mtm) = MainThreadMarker::new() else {
                return system();
            };
            let manager = NSFontManager::sharedFontManager(mtm);
            // NSFontManager weights: 5 regular, 9 bold (0 … 15).
            let weight = match f.weight.nearest_hundred() {
                100 => 2,
                200 => 3,
                300 => 4,
                400 => 5,
                500 => 6,
                600 => 8,
                700 => 9,
                800 => 10,
                _ => 11,
            };
            let traits = if f.italic {
                NSFontTraitMask::ItalicFontMask
            } else {
                NSFontTraitMask::empty()
            };
            manager
                .fontWithFamily_traits_weight_size(
                    &NSString::from_str(&family),
                    traits,
                    weight,
                    size,
                )
                .unwrap_or_else(system)
        }
        family => {
            let base = system();
            let design = unsafe {
                match family {
                    FontFamily::Monospace => {
                        Some(objc2_app_kit::NSFontDescriptorSystemDesignMonospaced)
                    }
                    FontFamily::Serif => Some(objc2_app_kit::NSFontDescriptorSystemDesignSerif),
                    FontFamily::Rounded => Some(objc2_app_kit::NSFontDescriptorSystemDesignRounded),
                    _ => None,
                }
            };
            match design.and_then(|d| base.fontDescriptor().fontDescriptorWithDesign(d)) {
                Some(d) => NSFont::fontWithDescriptor_size(&d, size).unwrap_or(base),
                None => base,
            }
        }
    };
    let mut descriptor: Retained<NSFontDescriptor> = font.fontDescriptor();
    let mut changed = false;
    if f.italic && !matches!(f.family, FontFamily::Named(_)) {
        let traits = descriptor.symbolicTraits() | NSFontDescriptorSymbolicTraits::TraitItalic;
        descriptor = descriptor.fontDescriptorWithSymbolicTraits(traits);
        changed = true;
    }
    if f.tabular {
        descriptor = with_tabular_digits(&descriptor);
        changed = true;
    }
    if changed {
        NSFont::fontWithDescriptor_size(&descriptor, size).unwrap_or(font)
    } else {
        font
    }
}

/// The descriptor with the `tnum` feature (Number Spacing: monospaced).
pub fn with_tabular_digits(d: &NSFontDescriptor) -> Retained<NSFontDescriptor> {
    use objc2_app_kit::{
        NSFontFeatureSelectorIdentifierKey, NSFontFeatureSettingsAttribute,
        NSFontFeatureTypeIdentifierKey,
    };
    // kNumberSpacingType = 6, kMonospacedNumbersSelector = 0 (SFNTLayoutTypes.h).
    let (ty, sel) = (NSNumber::new_i32(6), NSNumber::new_i32(0));
    unsafe {
        let feature =
            NSDictionary::<objc2_app_kit::NSFontDescriptorFeatureKey, AnyObject>::from_slices(
                &[
                    NSFontFeatureTypeIdentifierKey,
                    NSFontFeatureSelectorIdentifierKey,
                ],
                &[&*ty as &AnyObject, &*sel as &AnyObject],
            );
        let features = NSArray::from_retained_slice(&[feature]);
        let attrs =
            NSDictionary::<objc2_app_kit::NSFontDescriptorAttributeName, AnyObject>::from_slices(
                &[NSFontFeatureSettingsAttribute],
                &[&*features as &AnyObject],
            );
        d.fontDescriptorByAddingAttributes(&attrs)
    }
}

/// The attributed string of `t` (`focused`: the link drawn with focus).
pub fn attributed(t: &StyledText, focused: Option<usize>) -> Retained<NSAttributedString> {
    use objc2_app_kit::{
        NSBackgroundColorAttributeName, NSBaselineOffsetAttributeName, NSFontAttributeName,
        NSForegroundColorAttributeName, NSKernAttributeName, NSParagraphStyleAttributeName,
        NSStrikethroughStyleAttributeName, NSUnderlineStyleAttributeName,
    };
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(alignment(t.align));
    paragraph.setLineBreakMode(line_break(t));
    paragraph.setLineBreakStrategy(line_strategy(t));
    set_line_height(&paragraph, t.line_height);
    let out = NSMutableAttributedString::new();
    let mut link_index = 0;
    for run in &t.runs {
        let font = ns_font(&run.font);
        let color = run.color.map_or_else(NSColor::labelColor, ns_color);
        let start = out.length();
        match run
            .icon
            .as_deref()
            .and_then(|name| symbol(name, &run.font, &color))
        {
            Some(image) => {
                let attachment = NSTextAttachment::new();
                attachment.setImage(Some(&image));
                let s: Retained<NSAttributedString> = unsafe {
                    msg_send![NSAttributedString::class(), attributedStringWithAttachment: &*attachment]
                };
                out.appendAttributedString(&s);
            }
            None => out.appendAttributedString(&NSAttributedString::initWithString(
                NSAttributedString::alloc(),
                &NSString::from_str(&run.text),
            )),
        }
        let range = NSRange::new(start, out.length() - start);
        let add = |key: &objc2_foundation::NSAttributedStringKey, value: &AnyObject| unsafe {
            out.addAttribute_value_range(key, value, range)
        };
        let focus = run.link.is_some() && focused == Some(link_index);
        if run.link.is_some() {
            link_index += 1;
        }
        unsafe {
            add(NSFontAttributeName, &font);
            add(NSForegroundColorAttributeName, &color);
            add(NSParagraphStyleAttributeName, &paragraph);
            let background = if focus {
                Some(NSColor::selectedTextBackgroundColor())
            } else {
                run.background.map(ns_color)
            };
            if let Some(bg) = background {
                add(NSBackgroundColorAttributeName, &bg);
            }
            if t.letter_spacing != 0. {
                add(
                    NSKernAttributeName,
                    &NSNumber::new_f64(t.letter_spacing as f64),
                );
            }
            if run.underline || focus {
                add(NSUnderlineStyleAttributeName, &NSNumber::new_isize(1));
            }
            if run.strikethrough {
                add(NSStrikethroughStyleAttributeName, &NSNumber::new_isize(1));
            }
            if let Some(offset) = baseline_offset(&font, t.line_height) {
                add(NSBaselineOffsetAttributeName, &NSNumber::new_f64(offset));
            }
        }
    }
    Retained::into_super(out)
}

/// A fixed line height (`None`: the font's), shared by labels and text
/// fields so a field lines up with the same text shown by `text()` (#179).
pub(crate) fn set_line_height(paragraph: &NSMutableParagraphStyle, line_height: Option<f32>) {
    if let Some(lh) = line_height {
        paragraph.setMinimumLineHeight(lh as f64);
        paragraph.setMaximumLineHeight(lh as f64);
    }
}

/// The extra space of a fixed line height goes above the glyphs: raise
/// them by half of it, centered like CSS (`None`: nothing to move).
pub(crate) fn baseline_offset(font: &NSFont, line_height: Option<f32>) -> Option<f64> {
    let lh = line_height?;
    let natural = font.ascender() - font.descender() + font.leading();
    let offset = (lh as f64 - natural) / 2.;
    (offset.abs() > 0.01).then_some(offset)
}

/// Line breaking for pretty and balanced wrapping (#95): AppKit avoids a
/// lone word on the last line (`pushOut`) but has no balanced breaking.
fn line_strategy(t: &StyledText) -> NSLineBreakStrategy {
    match t.wrap {
        TextWrap::Auto => NSLineBreakStrategy::None,
        TextWrap::Pretty | TextWrap::Balance => NSLineBreakStrategy::PushOut,
    }
}

fn line_break(t: &StyledText) -> NSLineBreakMode {
    match (t.max_lines, t.no_wrap) {
        (Some(1), _) => NSLineBreakMode::ByTruncatingTail,
        (_, true) => NSLineBreakMode::ByClipping,
        _ => NSLineBreakMode::ByWordWrapping,
    }
}

/// An SF Symbol at the size of `font`, in `color` (a chip's icon, #115).
pub(crate) fn symbol_image(
    name: &str,
    font: &NSFont,
    color: &NSColor,
) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let size =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(font.pointSize() * 0.85, 0.);
    let palette =
        NSImageSymbolConfiguration::configurationWithPaletteColors(&NSArray::from_slice(&[color]));
    image.imageWithSymbolConfiguration(&size.configurationByApplyingConfiguration(&palette))
}

/// An SF Symbol at the run's size and weight, in its color.
fn symbol(name: &str, font: &FontSpec, color: &NSColor) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let size = NSImageSymbolConfiguration::configurationWithPointSize_weight(
        font.size as f64,
        ns_weight(font.weight),
    );
    let palette =
        NSImageSymbolConfiguration::configurationWithPaletteColors(&NSArray::from_slice(&[color]));
    image.imageWithSymbolConfiguration(&size.configurationByApplyingConfiguration(&palette))
}

/// Draws `t` in the label: its own font and color when plain (the fast
/// path), an attributed string otherwise; line limits on the cell.
fn apply(l: &NSTextField, t: &StyledText, focused: Option<usize>) {
    let clamp = t.max_lines.unwrap_or(0) as isize;
    l.setMaximumNumberOfLines(clamp);
    if let Some(cell) = l.cell() {
        // A clamp of several lines ends its last line with an ellipsis.
        cell.setTruncatesLastVisibleLine(t.max_lines.is_some());
    }
    l.setLineBreakMode(match (t.max_lines, t.no_wrap) {
        (Some(1), _) => NSLineBreakMode::ByTruncatingTail,
        (_, true) => NSLineBreakMode::ByClipping,
        _ => NSLineBreakMode::ByWordWrapping,
    });
    l.setAlignment(alignment(t.align));
    l.setLineBreakStrategy(line_strategy(t));
    if t.is_simple() && t.max_lines.is_none() && !t.no_wrap {
        let run = &t.runs[0];
        l.setFont(Some(&ns_font(&run.font)));
        l.setTextColor(Some(&run.color.map_or_else(NSColor::labelColor, ns_color)));
        l.setStringValue(&NSString::from_str(&run.text));
    } else {
        // The control's font too: it sizes empty lines and the ellipsis.
        if let Some(run) = t.runs.first() {
            l.setFont(Some(&ns_font(&run.font)));
        }
        l.setAttributedStringValue(&attributed(t, focused));
    }
}

/// Dev hook (`PRISM_CLICK`): presses the visible clickable text whose text
/// is `title`, or the inline link whose text is `title`.
pub fn press_in(view: &NSView, title: &str) -> bool {
    for sub in view.subviews().iter() {
        if sub.isHiddenOrHasHiddenAncestor() {
            continue;
        }
        if let Some(l) = sub.downcast_ref::<PrismLabel>() {
            if l.ivars().clickable.get() && *l.ivars().text.borrow() == title {
                dispatch(UiEvent::Click(l.ivars().id.get()));
                return true;
            }
            if let Some((_, link)) = l.links().into_iter().find(|(t, _)| t.trim() == title) {
                dispatch(UiEvent::MenuSelect(link));
                return true;
            }
        }
        if press_in(&sub, title) {
            return true;
        }
    }
    false
}

/// The label's longest word, as drawn (CSS min-content): a squeezed text
/// wraps down to it before breaking words.
pub fn min_content_width(l: &PrismLabel) -> f64 {
    use objc2_app_kit::NSAttributedStringNSStringDrawing;
    let shown = l.attributedStringValue();
    let text = shown.string().to_string();
    let mut widest = 0f64;
    for word in text.split_whitespace() {
        let start = word.as_ptr() as usize - text.as_ptr() as usize;
        // UTF-16 offsets, as `NSRange` counts them.
        let at = text[..start].encode_utf16().count();
        let range = NSRange::new(at, word.encode_utf16().count());
        widest = widest.max(shown.attributedSubstringFromRange(range).size().width);
    }
    widest.ceil()
}
