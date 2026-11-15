//! Padding on leaves (#14). `prism-native` lays a padded leaf out with its
//! padding around the measured content and hands the padding over in the
//! style (leading = left): texts and timers draw their text inside it
//! (`label.rs`), buttons get it as their content insets, text inputs and
//! text areas inset their text by it in place of their own inset.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSObject;
use objc2_ui_kit::{
    UIButton, UIColor, UIControl, UIEdgeInsets, UIResponder, UITextBorderStyle, UITextField,
    UIUserInterfaceLayoutDirection, UIView,
};
use prism_core::{Edges, NodeId, Style};

use crate::label::inset;

thread_local! {
    /// Each button, input and text area's style (what `measure` needs).
    static STYLES: RefCell<HashMap<NodeId, Style>> = RefCell::new(HashMap::new());
}

/// Remembers the style of leaf `id`.
pub fn remember(id: NodeId, style: &Style) {
    STYLES.with(|p| p.borrow_mut().insert(id, style.clone()));
}

/// Forgets leaf `id`.
pub fn forget(id: NodeId) {
    STYLES.with(|p| p.borrow_mut().remove(&id));
}

/// The style of leaf `id` (the default one when unknown).
pub fn style_of(id: NodeId) -> Style {
    STYLES.with(|p| p.borrow().get(&id).cloned().unwrap_or_default())
}

fn edge_insets(e: Edges) -> UIEdgeInsets {
    UIEdgeInsets {
        top: e.top as f64,
        left: e.left as f64,
        bottom: e.bottom as f64,
        right: e.right as f64,
    }
}

/// A button's padding replaces its content insets.
#[allow(deprecated)] // `contentEdgeInsets`: buttons here have no configuration.
pub fn style_button(b: &UIButton, style: &Style) {
    b.setContentEdgeInsets(edge_insets(style.padding));
}

/// A padded button's content: its fitting size without the padding (the
/// layout adds it).
pub fn button_content(fit: CGSize, style: &Style) -> CGSize {
    let p = style.padding;
    CGSize::new(
        (fit.width - (p.left + p.right) as f64).max(0.),
        (fit.height - (p.top + p.bottom) as f64).max(0.),
    )
}

/// Whether a text input draws Prism's box instead of the rounded-rect
/// field: it has padding, a background, a border, or `.border(0.)`.
pub fn custom_field(style: &Style) -> bool {
    !style.padding.is_zero()
        || style.background.is_some()
        || style.border_width > 0.
        || style.border_none
}

/// A custom field's border: its own, or the default one (1 point, gray,
/// rounded 6, like the web) unless `.border(0.)`.
fn field_border(style: &Style) -> Option<(f32, Option<prism_core::Color>)> {
    if style.border_width > 0. {
        Some((style.border_width, style.border_color))
    } else if style.border_none {
        None
    } else {
        Some((1., Some(prism_core::rgba(0x808080, 0.45))))
    }
}

/// Where a custom field's text sits inside its box: its padding (or the
/// default 6 × 8 inset, like the web) inside its border.
pub fn field_inset(style: &Style) -> Edges {
    if !custom_field(style) {
        return Edges::default();
    }
    let p = if style.padding.is_zero() {
        Edges {
            top: 6.,
            right: 8.,
            bottom: 6.,
            left: 8.,
        }
    } else {
        style.padding
    };
    let b = field_border(style).map_or(0., |(w, _)| w);
    Edges {
        top: p.top + b,
        right: p.right + b,
        bottom: p.bottom + b,
        left: p.left + b,
    }
}

/// The field's look: the rounded rect when unstyled; else Prism's box (its
/// background, its border or the default one) around the text, inset by
/// [`field_inset`].
pub fn style_field(f: &UITextField, style: &Style) {
    let custom = custom_field(style);
    f.setBorderStyle(if custom {
        UITextBorderStyle::None
    } else {
        UITextBorderStyle::RoundedRect
    });
    if custom {
        let bg = style
            .background
            .map_or_else(UIColor::clearColor, crate::imp::ui_color);
        f.setBackgroundColor(Some(&bg));
        if style.border_width <= 0.
            && let Some((w, c)) = field_border(style)
        {
            let layer = f.layer();
            layer.setBorderWidth(w as f64);
            crate::decor::set_layer_border(f, c);
            if style.corner_radius <= 0. {
                layer.setCornerRadius(6.);
            }
        }
    }
    if let Some(field) = f.downcast_ref::<PrismField>() {
        field.ivars().insets.set(field_inset(style));
        f.setNeedsLayout();
    }
}

/// A custom field's content size (without its padding: the layout adds
/// it): a line of text, plus the default inset and the border, at least
/// 120 points wide in all.
pub fn field_content(f: &UITextField, style: &Style) -> CGSize {
    let line = f.font().map_or(20., |font| unsafe { font.lineHeight() });
    let inset = field_inset(style);
    let p = style.padding;
    let extra_x = (inset.left + inset.right - p.left - p.right) as f64;
    let extra_y = (inset.top + inset.bottom - p.top - p.bottom) as f64;
    let min = (120. - (p.left + p.right) as f64).max(0.);
    CGSize::new(min.max(extra_x), (line + extra_y).ceil())
}

/// A text area's padding replaces its text container's inset (8 above and
/// below by default).
#[cfg(not(target_os = "tvos"))]
pub fn style_text_area(t: &crate::forms::TextArea, style: &Style) {
    let inset = if style.padding.is_zero() {
        UIEdgeInsets {
            top: 8.,
            left: 0.,
            bottom: 8.,
            right: 0.,
        }
    } else {
        edge_insets(style.padding)
    };
    t.view.setTextContainerInset(inset);
    t.layout_placeholder();
}

pub struct FieldIvars {
    /// Where the text sits inside the bounds (zero: UIKit's own inset).
    insets: Cell<Edges>,
}

define_class!(
    /// A text field whose text (and leading icon) sit inside its padding.
    #[unsafe(super(UITextField, UIControl, UIView, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismField"]
    #[ivars = FieldIvars]
    pub struct PrismField;

    impl PrismField {
        #[unsafe(method(textRectForBounds:))]
        fn text_rect(&self, bounds: CGRect) -> CGRect {
            match self.text_area(bounds) {
                Some(r) => r,
                None => unsafe { msg_send![super(self), textRectForBounds: bounds] },
            }
        }

        #[unsafe(method(editingRectForBounds:))]
        fn editing_rect(&self, bounds: CGRect) -> CGRect {
            match self.text_area(bounds) {
                Some(r) => r,
                None => unsafe { msg_send![super(self), editingRectForBounds: bounds] },
            }
        }

        #[unsafe(method(placeholderRectForBounds:))]
        fn placeholder_rect(&self, bounds: CGRect) -> CGRect {
            match self.text_area(bounds) {
                Some(r) => r,
                None => unsafe { msg_send![super(self), placeholderRectForBounds: bounds] },
            }
        }

        /// The leading icon (32 wide) just before the text.
        #[unsafe(method(leftViewRectForBounds:))]
        fn left_view_rect(&self, bounds: CGRect) -> CGRect {
            let e = self.ivars().insets.get();
            if e.is_zero() {
                return unsafe { msg_send![super(self), leftViewRectForBounds: bounds] };
            }
            let lead = self.leading(e);
            let w = bounds.size.width;
            let x = if self.rtl() { w - lead } else { lead - ICON };
            CGRect::new(
                CGPoint::new(x, ((bounds.size.height - 20.) / 2.).round()),
                CGSize::new(ICON, 20.),
            )
        }
    }
);

/// Width of the leading icon (see `interaction::set_leading_icon`).
const ICON: f64 = 32.;

impl PrismField {
    fn rtl(&self) -> bool {
        self.effectiveUserInterfaceLayoutDirection() == UIUserInterfaceLayoutDirection::RightToLeft
    }

    /// The text's leading inset: room for the icon too, when there is one.
    fn leading(&self, e: Edges) -> f64 {
        let lead = if self.rtl() { e.right } else { e.left } as f64;
        if self.leftView().is_some() {
            lead.max(ICON)
        } else {
            lead
        }
    }

    /// The text's rectangle inside the padding (`None`: UIKit's own).
    fn text_area(&self, bounds: CGRect) -> Option<CGRect> {
        let mut e = self.ivars().insets.get();
        if e.is_zero() {
            return None;
        }
        let lead = self.leading(e) as f32;
        if self.rtl() {
            e.right = lead;
        } else {
            e.left = lead;
        }
        Some(inset(bounds, e))
    }

    pub fn field(mtm: MainThreadMarker) -> Retained<UITextField> {
        let this = Self::alloc(mtm).set_ivars(FieldIvars {
            insets: Cell::new(Edges::default()),
        });
        let f: Retained<PrismField> = unsafe { msg_send![super(this), init] };
        Retained::into_super(f)
    }
}
