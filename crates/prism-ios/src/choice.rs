//! Checkboxes and radio groups (#18). UIKit has neither control, so they
//! are drawn: buttons showing an SF Symbol box or circle and the label.
//! VoiceOver reads a checkbox as a toggle button (`checked` value, the
//! iOS 17 toggle-button trait), each radio option as a button that is
//! selected or not, inside a semantic group named by the group's label.
//! On tvOS they are ordinary focusable buttons.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, Message, sel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSString;
use objc2_ui_kit::{
    NSObjectUIAccessibility, NSObjectUIAccessibilityContainer, UIAccessibilityTraitButton,
    UIAccessibilityTraitSelected, UIButton, UIButtonConfiguration, UIColor,
    UIControlContentHorizontalAlignment, UIControlEvents, UIFont, UIImage, UIView,
};
use prism_core::controls::RadioConfig;

fn event() -> UIControlEvents {
    if cfg!(target_os = "tvos") {
        UIControlEvents::PrimaryActionTriggered
    } else {
        UIControlEvents::TouchUpInside
    }
}

/// A button showing `symbol` then `title`, aligned to the leading edge.
fn choice_button(
    mtm: MainThreadMarker,
    target: &AnyObject,
    action: objc2::runtime::Sel,
) -> Retained<UIButton> {
    let config = UIButtonConfiguration::plainButtonConfiguration(mtm);
    config.setImagePadding(8.);
    config.setContentInsets(objc2_ui_kit::NSDirectionalEdgeInsets {
        top: 6.,
        leading: 0.,
        bottom: 6.,
        trailing: 0.,
    });
    let b = UIButton::buttonWithConfiguration_primaryAction(&config, None);
    b.setContentHorizontalAlignment(UIControlContentHorizontalAlignment::Leading);
    unsafe { b.addTarget_action_forControlEvents(Some(target), action, event()) };
    b
}

fn symbol(name: &str) -> Option<Retained<UIImage>> {
    UIImage::systemImageNamed(&NSString::from_str(name))
}

/// Shows `title` in `font` / `color` with the box or circle for `on`.
fn refresh(
    b: &UIButton,
    title: &str,
    on: bool,
    radio: bool,
    font: Option<&UIFont>,
    color: Option<&UIColor>,
) {
    let image = match (radio, on) {
        (false, true) => "checkmark.square.fill",
        (false, false) => "square",
        (true, true) => "largecircle.fill.circle",
        (true, false) => "circle",
    };
    crate::forms::update_button(b, |c| {
        c.setImage(symbol(image).as_deref());
        c.setBaseForegroundColor(Some(
            &color.map_or_else(UIColor::labelColor, |c| c.retain()),
        ));
        let title = NSString::from_str(title);
        match font {
            Some(f) => {
                let key: &NSString = unsafe { objc2_ui_kit::NSFontAttributeName };
                let value: &AnyObject = f;
                let attrs = objc2_foundation::NSDictionary::from_slices(&[key], &[value]);
                let s = unsafe {
                    objc2_foundation::NSAttributedString::initWithString_attributes(
                        objc2_foundation::NSAttributedString::alloc(),
                        &title,
                        Some(&attrs),
                    )
                };
                c.setAttributedTitle(Some(&s));
            }
            None => c.setTitle(Some(&title)),
        }
    });
    b.setSelected(false);
}

// ---------------------------------------------------------------------------
// Checkbox
// ---------------------------------------------------------------------------

pub struct Checkbox {
    pub button: Retained<UIButton>,
    on: Cell<bool>,
    title: RefCell<String>,
    font: RefCell<Option<Retained<UIFont>>>,
    color: RefCell<Option<Retained<UIColor>>>,
}

impl Checkbox {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject) -> Checkbox {
        let c = Checkbox {
            button: choice_button(mtm, target, sel!(onCheckbox:)),
            on: Cell::new(false),
            title: RefCell::default(),
            font: RefCell::default(),
            color: RefCell::default(),
        };
        c.refresh();
        c
    }

    fn refresh(&self) {
        refresh(
            &self.button,
            &self.title.borrow(),
            self.on.get(),
            false,
            self.font.borrow().as_deref(),
            self.color.borrow().as_deref(),
        );
    }

    pub fn set_on(&self, on: bool) {
        if self.on.get() != on {
            self.on.set(on);
            self.refresh();
        }
    }

    pub fn is_on(&self) -> bool {
        self.on.get()
    }

    pub fn set_title(&self, title: &str) {
        *self.title.borrow_mut() = title.to_string();
        self.refresh();
    }

    pub fn set_style(&self, font: Retained<UIFont>, color: Option<Retained<UIColor>>) {
        *self.font.borrow_mut() = Some(font);
        *self.color.borrow_mut() = color;
        self.refresh();
    }
}

// ---------------------------------------------------------------------------
// Radio group
// ---------------------------------------------------------------------------

/// Space between options, in points.
const GAP_ROW: f64 = 16.;
const GAP_COLUMN: f64 = 2.;

pub struct RadioGroup {
    pub view: Retained<UIView>,
    buttons: RefCell<Vec<Retained<UIButton>>>,
    options: RefCell<Vec<String>>,
    selected: Cell<Option<usize>>,
    horizontal: Cell<bool>,
    font: RefCell<Option<Retained<UIFont>>>,
    color: RefCell<Option<Retained<UIColor>>>,
    target: Retained<AnyObject>,
}

impl RadioGroup {
    pub fn new(mtm: MainThreadMarker, target: &AnyObject) -> RadioGroup {
        let view = UIView::new(mtm);
        view.setIsAccessibilityElement(false, mtm);
        // VoiceOver announces the group's label when entering it.
        view.setAccessibilityContainerType(
            objc2_ui_kit::UIAccessibilityContainerType::SemanticGroup,
            mtm,
        );
        RadioGroup {
            view,
            buttons: RefCell::default(),
            options: RefCell::default(),
            selected: Cell::new(None),
            horizontal: Cell::new(false),
            font: RefCell::default(),
            color: RefCell::default(),
            target: target.retain(),
        }
    }

    pub fn set_config(&self, config: &str) {
        let c = RadioConfig::decode(config);
        self.horizontal.set(c.horizontal);
        let mtm = self.view.mtm();
        {
            let mut buttons = self.buttons.borrow_mut();
            while buttons.len() > c.options.len() {
                if let Some(b) = buttons.pop() {
                    b.removeFromSuperview();
                }
            }
            while buttons.len() < c.options.len() {
                let b = choice_button(mtm, &self.target, sel!(onRadio:));
                // Negative tags: not Prism nodes; -1 - index.
                b.setTag(-(buttons.len() as isize) - 1);
                self.view.addSubview(&b);
                buttons.push(b);
            }
        }
        *self.options.borrow_mut() = c.options;
        self.refresh();
    }

    pub fn set_selected(&self, selected: Option<usize>) {
        self.selected.set(selected);
        self.refresh();
    }

    pub fn set_style(&self, font: Retained<UIFont>, color: Option<Retained<UIColor>>) {
        *self.font.borrow_mut() = Some(font);
        *self.color.borrow_mut() = color;
        self.refresh();
    }

    fn refresh(&self) {
        let options = self.options.borrow();
        let mtm = self.view.mtm();
        for (i, (b, title)) in self.buttons.borrow().iter().zip(options.iter()).enumerate() {
            let on = self.selected.get() == Some(i);
            refresh(
                b,
                title,
                on,
                true,
                self.font.borrow().as_deref(),
                self.color.borrow().as_deref(),
            );
            b.setAccessibilityLabel(Some(&NSString::from_str(title)), mtm);
            let mut traits = unsafe { UIAccessibilityTraitButton };
            if on {
                traits |= unsafe { UIAccessibilityTraitSelected };
            }
            b.setAccessibilityTraits(traits, mtm);
        }
    }

    /// Frames of the options in a `width`-wide group: rows wrap when
    /// horizontal, one per line otherwise.
    fn frames(&self, width: Option<f64>) -> (Vec<CGRect>, CGSize) {
        let limit = CGSize::new(f64::MAX / 4., f64::MAX / 4.);
        let mut out = Vec::new();
        let (mut x, mut y, mut line, mut widest) = (0f64, 0f64, 0f64, 0f64);
        for b in self.buttons.borrow().iter() {
            let s = b.sizeThatFits(limit);
            if self.horizontal.get() {
                if x > 0. && width.is_some_and(|w| x + s.width > w) {
                    y += line + GAP_COLUMN;
                    x = 0.;
                    line = 0.;
                }
                out.push(CGRect::new(CGPoint::new(x, y), s));
                x += s.width + GAP_ROW;
                widest = widest.max(x - GAP_ROW);
                line = line.max(s.height);
            } else {
                let w = width.map_or(s.width, |w| s.width.min(w));
                out.push(CGRect::new(CGPoint::new(0., y), CGSize::new(w, s.height)));
                y += s.height + GAP_COLUMN;
                widest = widest.max(w);
            }
        }
        let height = if self.horizontal.get() {
            y + line
        } else {
            (y - GAP_COLUMN).max(0.)
        };
        (out, CGSize::new(widest, height))
    }

    pub fn measure(&self, max_width: Option<f32>) -> CGSize {
        self.frames(max_width.map(f64::from)).1
    }

    /// Places the options inside the group's new width (mirrored right to left).
    pub fn layout(&self, width: f64) {
        let rtl = self.view.effectiveUserInterfaceLayoutDirection()
            == objc2_ui_kit::UIUserInterfaceLayoutDirection::RightToLeft;
        let (frames, _) = self.frames(Some(width));
        for (b, mut f) in self.buttons.borrow().iter().zip(frames) {
            if rtl {
                f.origin.x = width - f.origin.x - f.size.width;
            }
            b.setFrame(f);
        }
    }

    pub fn set_label(&self, label: Option<&str>) {
        let mtm = self.view.mtm();
        self.view
            .setAccessibilityLabel(label.map(NSString::from_str).as_deref(), mtm);
    }
}
