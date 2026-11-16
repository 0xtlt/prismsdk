//! Text nodes: `PrismLabel`, a `UILabel` that draws the node's typography
//! (family, weight, italic, line height, letter spacing, case, decoration,
//! tabular digits, truncation) and rich-text runs as one attributed string,
//! so `sizeThatFits:` measures exactly what is drawn.
//!
//! Inline links (and a clickable text) react to taps (hit-tested with
//! TextKit); VoiceOver reads the paragraph as one element and lists its links
//! as actions (also reachable with Full Keyboard Access). On tvOS a clickable
//! text or a text with links takes the remote's focus; select activates the
//! text, or its first link.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    sel,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSNumber, NSObject,
    NSObjectProtocol, NSRange, NSString,
};
use objc2_ui_kit::{
    NSLayoutManager, NSLineBreakMode, NSLineBreakStrategy, NSMutableParagraphStyle,
    NSTextAlignment, NSTextAttachment, NSTextContainer, NSTextStorage, UIAccessibilityCustomAction,
    UIColor, UIFont, UIFontDescriptor, UIFontDescriptorSymbolicTraits, UIImage,
    UIImageSymbolConfiguration, UILabel, UIResponder, UITapGestureRecognizer, UIView,
};
use prism_core::{FontFamily, NodeId, Style, TextAlign, TextWrap, UiEvent};
use prism_native::text::{FontSpec, StyledText, interpolate_weight};

use crate::imp::{dispatch, ui_color};

pub struct LabelIvars {
    id: Cell<NodeId>,
    clickable: Cell<bool>,
    text: RefCell<String>,
    /// Encoded runs (`prism_core::spans`), empty for plain text.
    runs: RefCell<String>,
    style: RefCell<Style>,
    /// What is drawn (links, for taps).
    styled: RefCell<Option<StyledText>>,
    tap: RefCell<Option<Retained<UITapGestureRecognizer>>>,
    /// Padding (#14): the text is drawn this far inside the bounds.
    insets: Cell<prism_core::Edges>,
}

define_class!(
    #[unsafe(super(UILabel, UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismLabel"]
    #[ivars = LabelIvars]
    pub struct PrismLabel;

    unsafe impl NSObjectProtocol for PrismLabel {}

    impl PrismLabel {
        /// The text inside the padding; the box (background, border) fills
        /// the bounds.
        #[unsafe(method(drawTextInRect:))]
        fn draw_text_in_rect(&self, rect: CGRect) {
            let rect = inset(rect, self.ivars().insets.get());
            unsafe { msg_send![super(self), drawTextInRect: rect] }
        }

        #[unsafe(method(prismTap:))]
        fn prism_tap(&self, tap: &UITapGestureRecognizer) {
            let p = tap.locationInView(Some(self));
            // tvOS: the remote's select press has no location.
            let link = if cfg!(target_os = "tvos") {
                None
            } else {
                self.link_at(p)
            };
            if let Some(link) = link {
                dispatch(UiEvent::MenuSelect(link));
            } else if self.ivars().clickable.get() {
                dispatch(UiEvent::Click(self.ivars().id.get()));
            } else if let Some((_, link)) = self.links().first()
                && cfg!(target_os = "tvos")
            {
                // The remote has no pointer: select activates the first link.
                dispatch(UiEvent::MenuSelect(*link));
            }
        }

        /// VoiceOver "activate": a clickable text, or its only link.
        #[unsafe(method(accessibilityActivate))]
        fn accessibility_activate(&self) -> Bool {
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

        #[cfg(target_os = "tvos")]
        #[unsafe(method(canBecomeFocused))]
        fn can_become_focused(&self) -> bool {
            self.interactive()
        }

        #[cfg(target_os = "tvos")]
        #[unsafe(method(didUpdateFocusInContext:withAnimationCoordinator:))]
        fn did_update_focus(
            &self,
            context: &objc2_ui_kit::UIFocusUpdateContext,
            coordinator: &objc2_ui_kit::UIFocusAnimationCoordinator,
        ) {
            let _: () = unsafe {
                msg_send![super(self), didUpdateFocusInContext: context, withAnimationCoordinator: coordinator]
            };
            crate::tv::raise(self, self.isFocused());
        }
    }
);

impl PrismLabel {
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

    /// The link under `p`: the label's text laid out again with TextKit in
    /// the rectangle the label draws it in.
    fn link_at(&self, p: CGPoint) -> Option<u64> {
        let styled = self.ivars().styled.borrow();
        let styled = styled.as_ref().filter(|s| s.has_links())?;
        let attributed = self.attributedText()?;
        let lines = self.numberOfLines();
        let bounds = inset(self.bounds(), self.ivars().insets.get());
        let rect = self.textRectForBounds_limitedToNumberOfLines(bounds, lines);
        let storage = NSTextStorage::new();
        storage.setAttributedString(&attributed);
        let layout = NSLayoutManager::new();
        let container = NSTextContainer::initWithSize(
            NSTextContainer::alloc(),
            CGSize::new(rect.size.width, 1e7),
        );
        container.setLineFragmentPadding(0.);
        container.setMaximumNumberOfLines(lines.max(0) as usize);
        container.setLineBreakMode(self.lineBreakMode());
        layout.addTextContainer(&container);
        storage.addLayoutManager(&layout);
        layout.ensureLayoutForTextContainer(&container);
        let (x, y) = (p.x - rect.origin.x, p.y - rect.origin.y);
        let used = layout.usedRectForTextContainer(&container);
        if x < 0. || y < 0. || x > used.size.width + 1. || y > used.size.height + 1. {
            return None;
        }
        let glyph = layout.glyphIndexForPoint_inTextContainer(CGPoint::new(x, y), &container);
        let index = layout.characterIndexForGlyphAtIndex(glyph);
        styled.link_at_utf16(index)
    }

    fn refresh(&self) {
        let iv = self.ivars();
        let styled = StyledText::new(&iv.text.borrow(), &iv.runs.borrow(), &iv.style.borrow());
        apply(self, &styled);
        let actions: Vec<Retained<UIAccessibilityCustomAction>> = styled
            .links()
            .into_iter()
            .map(|(name, link)| {
                let handler = block2::RcBlock::new(
                    move |_: std::ptr::NonNull<UIAccessibilityCustomAction>| {
                        dispatch(UiEvent::MenuSelect(link));
                        Bool::YES
                    },
                );
                unsafe {
                    UIAccessibilityCustomAction::initWithName_actionHandler(
                        UIAccessibilityCustomAction::alloc(self.mtm()),
                        &NSString::from_str(name.trim()),
                        block2::RcBlock::as_ptr(&handler),
                    )
                }
            })
            .collect();
        let actions = (!actions.is_empty()).then(|| NSArray::from_retained_slice(&actions));
        unsafe {
            let _: () = msg_send![self, setAccessibilityCustomActions: actions.as_deref()];
        }
        *iv.styled.borrow_mut() = Some(styled);
        self.update_tap();
    }

    /// Taps (the remote's select press on tvOS) only when there is
    /// something to activate: plain labels let touches through.
    fn update_tap(&self) {
        let on = self.interactive();
        self.setUserInteractionEnabled(on);
        let mut tap = self.ivars().tap.borrow_mut();
        match (on, tap.is_some()) {
            (true, false) => {
                let target: &AnyObject = self;
                let t = unsafe {
                    UITapGestureRecognizer::initWithTarget_action(
                        UITapGestureRecognizer::alloc(self.mtm()),
                        Some(target),
                        Some(sel!(prismTap:)),
                    )
                };
                #[cfg(target_os = "tvos")]
                crate::tv::select_press_only(&t);
                self.addGestureRecognizer(&t);
                *tap = Some(t);
            }
            (false, true) => {
                if let Some(t) = tap.take() {
                    self.removeGestureRecognizer(&t);
                }
            }
            _ => {}
        }
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
        tap: RefCell::new(None),
        insets: Cell::new(Default::default()),
    });
    let l: Retained<PrismLabel> = unsafe { msg_send![super(this), init] };
    l.setNumberOfLines(0);
    l.setTag(id as isize);
    l.refresh();
    l
}

pub fn set_text(l: &PrismLabel, text: &str) {
    *l.ivars().text.borrow_mut() = text.to_string();
    l.refresh();
}

pub fn set_runs(l: &PrismLabel, runs: &str) {
    *l.ivars().runs.borrow_mut() = runs.to_string();
    l.refresh();
}

pub fn set_style(l: &PrismLabel, style: &Style) {
    *l.ivars().style.borrow_mut() = style.clone();
    set_insets(l, style.padding);
    l.refresh();
}

/// Draws the text `e` inside the bounds (a text's or a timer's padding).
pub fn set_insets(l: &PrismLabel, e: prism_core::Edges) {
    if l.ivars().insets.get() != e {
        l.ivars().insets.set(e);
        l.setNeedsDisplay();
    }
}

/// A timer's label: a plain one-line label that can draw inside a padding.
pub fn new_timer(mtm: MainThreadMarker, id: NodeId) -> Retained<PrismLabel> {
    let l = new_label(mtm, id);
    l.setNumberOfLines(1);
    l
}

/// `rect` minus `e` (left/right already mirrored right to left).
pub fn inset(rect: CGRect, e: prism_core::Edges) -> CGRect {
    let (l, r, t, b) = (e.left as f64, e.right as f64, e.top as f64, e.bottom as f64);
    CGRect::new(
        CGPoint::new(rect.origin.x + l, rect.origin.y + t),
        CGSize::new(
            (rect.size.width - l - r).max(0.),
            (rect.size.height - t - b).max(0.),
        ),
    )
}

pub fn set_clickable(l: &PrismLabel, clickable: bool) {
    l.ivars().clickable.set(clickable);
    l.update_tap();
}

fn alignment(a: TextAlign) -> NSTextAlignment {
    match a {
        TextAlign::Left => NSTextAlignment::Left,
        TextAlign::Center => NSTextAlignment::Center,
        TextAlign::Right => NSTextAlignment::Right,
    }
}

/// `UIFont.Weight` for any CSS weight: the UIKit constants at 100 … 900,
/// interpolated in between (the system font is variable).
pub fn ui_weight(w: prism_core::FontWeight) -> f64 {
    use objc2_ui_kit::*;
    let steps = unsafe {
        [
            UIFontWeightUltraLight,
            UIFontWeightThin,
            UIFontWeightLight,
            UIFontWeightRegular,
            UIFontWeightMedium,
            UIFontWeightSemibold,
            UIFontWeightBold,
            UIFontWeightHeavy,
            UIFontWeightBlack,
        ]
    };
    interpolate_weight(w, steps)
}

/// The font of a run: the system font in its design (monospaced, serif,
/// rounded) or a family by name (installed or registered from the app's
/// `assets/fonts`), italic, with tabular digits.
pub fn ui_font(f: &FontSpec) -> Retained<UIFont> {
    use objc2_ui_kit::{
        UIFontDescriptorAttributeName, UIFontDescriptorFamilyAttribute,
        UIFontDescriptorTraitsAttribute, UIFontWeightTrait,
    };
    let size = f.size as f64;
    let weight = ui_weight(f.weight);
    let system = UIFont::systemFontOfSize_weight(size, weight);
    let mut descriptor: Retained<UIFontDescriptor> = match &f.family {
        FontFamily::Named(name) => {
            // A run-time font's alias (`cx.load_font`) → its file's family.
            let family = NSString::from_str(&prism_native::fonts::resolve_name(name));
            if UIFont::fontNamesForFamilyName(&family).count() == 0 {
                unsafe { system.fontDescriptor() }
            } else {
                let w = NSNumber::new_f64(weight);
                unsafe {
                    let traits = NSDictionary::<objc2_ui_kit::UIFontDescriptorTraitKey, AnyObject>::from_slices(
                        &[UIFontWeightTrait],
                        &[&*w as &AnyObject],
                    );
                    UIFontDescriptor::fontDescriptorWithFontAttributes(&NSDictionary::<
                        UIFontDescriptorAttributeName,
                        AnyObject,
                    >::from_slices(
                        &[
                            UIFontDescriptorFamilyAttribute,
                            UIFontDescriptorTraitsAttribute,
                        ],
                        &[&*family as &AnyObject, &*traits as &AnyObject],
                    ))
                }
            }
        }
        family => {
            let design = unsafe {
                match family {
                    FontFamily::Monospace => {
                        Some(objc2_ui_kit::UIFontDescriptorSystemDesignMonospaced)
                    }
                    FontFamily::Serif => Some(objc2_ui_kit::UIFontDescriptorSystemDesignSerif),
                    FontFamily::Rounded => Some(objc2_ui_kit::UIFontDescriptorSystemDesignRounded),
                    _ => None,
                }
            };
            let base = unsafe { system.fontDescriptor() };
            design
                .and_then(|d| base.fontDescriptorWithDesign(d))
                .unwrap_or(base)
        }
    };
    if f.italic {
        let traits =
            unsafe { descriptor.symbolicTraits() } | UIFontDescriptorSymbolicTraits::TraitItalic;
        if let Some(d) = descriptor.fontDescriptorWithSymbolicTraits(traits) {
            descriptor = d;
        }
    }
    if f.tabular {
        descriptor = with_tabular_digits(&descriptor);
    }
    UIFont::fontWithDescriptor_size(&descriptor, size)
}

/// The descriptor with the `tnum` feature (Number Spacing: monospaced).
fn with_tabular_digits(d: &UIFontDescriptor) -> Retained<UIFontDescriptor> {
    use objc2_ui_kit::{
        UIFontDescriptorFeatureSettingsAttribute, UIFontFeatureSelectorIdentifierKey,
        UIFontFeatureTypeIdentifierKey,
    };
    // kNumberSpacingType = 6, kMonospacedNumbersSelector = 0 (SFNTLayoutTypes.h).
    let (ty, sel) = (NSNumber::new_i32(6), NSNumber::new_i32(0));
    unsafe {
        let feature =
            NSDictionary::<objc2_ui_kit::UIFontDescriptorFeatureKey, AnyObject>::from_slices(
                &[
                    UIFontFeatureTypeIdentifierKey,
                    UIFontFeatureSelectorIdentifierKey,
                ],
                &[&*ty as &AnyObject, &*sel as &AnyObject],
            );
        let features = NSArray::from_retained_slice(&[feature]);
        d.fontDescriptorByAddingAttributes(&NSDictionary::from_slices(
            &[UIFontDescriptorFeatureSettingsAttribute],
            &[&*features as &AnyObject],
        ))
    }
}

/// Line breaking for pretty and balanced wrapping (#95): UIKit avoids a lone
/// word on the last line (`pushOut`) but has no balanced breaking.
fn line_strategy(t: &StyledText) -> Option<NSLineBreakStrategy> {
    (t.wrap != TextWrap::Auto).then_some(NSLineBreakStrategy::PushOut)
}

fn line_break(t: &StyledText) -> NSLineBreakMode {
    match (t.max_lines, t.no_wrap) {
        (Some(_), _) => NSLineBreakMode::ByTruncatingTail,
        (_, true) => NSLineBreakMode::ByClipping,
        _ => NSLineBreakMode::ByWordWrapping,
    }
}

/// The attributed string of `t`.
pub fn attributed(t: &StyledText) -> Retained<NSAttributedString> {
    use objc2_ui_kit::{
        NSBackgroundColorAttributeName, NSBaselineOffsetAttributeName, NSFontAttributeName,
        NSForegroundColorAttributeName, NSKernAttributeName, NSParagraphStyleAttributeName,
        NSStrikethroughStyleAttributeName, NSUnderlineStyleAttributeName,
    };
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(alignment(t.align));
    paragraph.setLineBreakMode(line_break(t));
    if let Some(strategy) = line_strategy(t) {
        paragraph.setLineBreakStrategy(strategy);
    }
    if let Some(lh) = t.line_height {
        paragraph.setMinimumLineHeight(lh as f64);
        paragraph.setMaximumLineHeight(lh as f64);
    }
    let out = NSMutableAttributedString::new();
    for run in &t.runs {
        let font = ui_font(&run.font);
        let color = run.color.map_or_else(UIColor::labelColor, ui_color);
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
        unsafe {
            add(NSFontAttributeName, &font);
            add(NSForegroundColorAttributeName, &color);
            add(NSParagraphStyleAttributeName, &paragraph);
            if let Some(bg) = run.background.map(ui_color) {
                add(NSBackgroundColorAttributeName, &bg);
            }
            if t.letter_spacing != 0. {
                add(
                    NSKernAttributeName,
                    &NSNumber::new_f64(t.letter_spacing as f64),
                );
            }
            if run.underline {
                add(NSUnderlineStyleAttributeName, &NSNumber::new_isize(1));
            }
            if run.strikethrough {
                add(NSStrikethroughStyleAttributeName, &NSNumber::new_isize(1));
            }
            if let Some(lh) = t.line_height {
                // The extra space of a fixed line height goes above the
                // glyphs: raise them to center them, like CSS.
                let offset = (lh as f64 - font.lineHeight()) / 4.;
                if offset.abs() > 0.01 {
                    add(NSBaselineOffsetAttributeName, &NSNumber::new_f64(offset));
                }
            }
        }
    }
    Retained::into_super(out)
}

/// An SF Symbol at the run's size and weight, in its color.
fn symbol(name: &str, font: &FontSpec, color: &UIColor) -> Option<Retained<UIImage>> {
    let config = UIImageSymbolConfiguration::configurationWithPointSize_weight(
        font.size as f64,
        crate::imp::symbol_weight_for(font.weight),
    );
    let image =
        UIImage::systemImageNamed_withConfiguration(&NSString::from_str(name), Some(&config))?;
    Some(image.imageWithTintColor_renderingMode(
        color,
        objc2_ui_kit::UIImageRenderingMode::AlwaysOriginal,
    ))
}

/// Draws `t` in the label: its own font and color when plain (the fast
/// path), an attributed string otherwise; the line limit.
fn apply(l: &UILabel, t: &StyledText) {
    l.setNumberOfLines(match (t.max_lines, t.no_wrap) {
        (Some(n), _) => n as isize,
        // One line per paragraph: newlines still break, nothing wraps.
        (None, true) => 0,
        (None, false) => 0,
    });
    l.setLineBreakMode(line_break(t));
    // A label's own default is `standard`.
    l.setLineBreakStrategy(line_strategy(t).unwrap_or(NSLineBreakStrategy::Standard));
    l.setTextAlignment(alignment(t.align));
    if t.is_simple() && t.max_lines.is_none() && !t.no_wrap {
        let run = &t.runs[0];
        unsafe {
            l.setFont(Some(&ui_font(&run.font)));
            l.setTextColor(Some(&run.color.map_or_else(UIColor::labelColor, ui_color)));
        }
        l.setText(Some(&NSString::from_str(&run.text)));
    } else {
        if let Some(run) = t.runs.first() {
            unsafe { l.setFont(Some(&ui_font(&run.font))) };
        }
        l.setAttributedText(Some(&attributed(t)));
    }
}
