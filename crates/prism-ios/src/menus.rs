//! UIKit menus (context menus) and alerts from Prism's specs.

use std::ptr::NonNull;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSString};
use objc2_ui_kit::{
    UIAction, UIAlertAction, UIAlertActionStyle, UIAlertController, UIAlertControllerStyle,
    UIImage, UIMenu, UIMenuElement, UIMenuElementAttributes, UIMenuElementState, UIMenuOptions,
    UIViewController,
};
use prism_core::UiEvent;
use prism_core::menu::{ActionRole, AlertSpec, MenuItemSpec, MenuSpec};

fn symbol(name: &Option<String>) -> Option<Retained<UIImage>> {
    name.as_deref()
        .and_then(|n| UIImage::systemImageNamed(&NSString::from_str(n)))
}

/// Builds a `UIMenu`; chosen actions are reported with `report`.
pub fn build_menu(mtm: MainThreadMarker, spec: &MenuSpec, report: fn(UiEvent)) -> Retained<UIMenu> {
    // Separators split the items into inline groups (UIKit draws dividers).
    let mut groups: Vec<Vec<Retained<UIMenuElement>>> = vec![Vec::new()];
    for item in &spec.items {
        match item {
            MenuItemSpec::Separator => groups.push(Vec::new()),
            // System actions (#161) are the Mac menu bar's.
            MenuItemSpec::System { .. } => {}
            MenuItemSpec::Submenu { title, icon, menu } => {
                let mut sub = build_menu(mtm, menu, report);
                if icon.is_some() {
                    sub = UIMenu::menuWithTitle_image_identifier_options_children(
                        &NSString::from_str(title),
                        symbol(icon).as_deref(),
                        None,
                        UIMenuOptions::empty(),
                        &sub.children(),
                        mtm,
                    );
                }
                groups.last_mut().unwrap().push(Retained::into_super(sub));
            }
            MenuItemSpec::Action {
                id,
                title,
                icon,
                role,
                checked,
                enabled,
                ..
            } => {
                let id = *id;
                let handler = block2::RcBlock::new(move |_: NonNull<UIAction>| {
                    report(UiEvent::MenuSelect(id))
                });
                let action = unsafe {
                    UIAction::actionWithTitle_image_identifier_handler(
                        &NSString::from_str(title),
                        symbol(icon).as_deref(),
                        None,
                        &*handler as *const _ as *mut _,
                        mtm,
                    )
                };
                let mut attrs = UIMenuElementAttributes::empty();
                if *role == ActionRole::Destructive {
                    attrs |= UIMenuElementAttributes::Destructive;
                }
                if !enabled {
                    attrs |= UIMenuElementAttributes::Disabled;
                }
                action.setAttributes(attrs);
                if *checked == Some(true) {
                    action.setState(UIMenuElementState::On);
                }
                groups
                    .last_mut()
                    .unwrap()
                    .push(Retained::into_super(action));
            }
        }
    }
    let children: Vec<Retained<UIMenuElement>> = if groups.len() == 1 {
        groups.pop().unwrap()
    } else {
        groups
            .into_iter()
            .filter(|g| !g.is_empty())
            .map(|g| {
                let inline = UIMenu::menuWithTitle_image_identifier_options_children(
                    &NSString::new(),
                    None,
                    None,
                    UIMenuOptions::DisplayInline,
                    &NSArray::from_retained_slice(&g),
                    mtm,
                );
                Retained::into_super(inline)
            })
            .collect()
    };
    UIMenu::menuWithTitle_children(
        &NSString::from_str(&spec.title),
        &NSArray::from_retained_slice(&children),
        mtm,
    )
}

thread_local! {
    /// The button presenting the last pop-up menu (#247).
    static POP_UP: std::cell::RefCell<Option<Retained<objc2_ui_kit::UIButton>>> =
        const { std::cell::RefCell::new(None) };
}

/// Opens `spec` as a pop-up menu over `parent` (#247): at `at` from its
/// top-left corner, or below it. UIKit presents menus from controls only: a
/// transparent one-point `UIButton` at that place shows the menu as its
/// primary action, pressed with `performPrimaryAction` (iOS 17.4 and
/// later; nothing before). Nothing on tvOS.
pub fn pop_up(
    mtm: MainThreadMarker,
    parent: &objc2_ui_kit::UIView,
    spec: &MenuSpec,
    at: Option<(f32, f32)>,
    report: fn(UiEvent),
) {
    if cfg!(target_os = "tvos") {
        return;
    }
    use objc2::sel;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_foundation::NSObjectProtocol;
    use objc2_ui_kit::{NSObjectUIAccessibility, UIButton, UIButtonType};
    let button = UIButton::buttonWithType(UIButtonType::Custom, mtm);
    if !button.respondsToSelector(sel!(performPrimaryAction)) {
        return;
    }
    if let Some(old) = POP_UP.with(|p| p.borrow_mut().take()) {
        old.removeFromSuperview();
    }
    let (x, y) = match at {
        Some((x, y)) => (x as f64, y as f64),
        None => (0., parent.bounds().size.height),
    };
    button.setFrame(CGRect::new(CGPoint::new(x, y), CGSize::new(1., 1.)));
    button.setIsAccessibilityElement(false, mtm);
    button.setMenu(Some(&build_menu(mtm, spec, report)));
    button.setShowsMenuAsPrimaryAction(true);
    parent.addSubview(&button);
    button.performPrimaryAction();
    POP_UP.with(|p| *p.borrow_mut() = Some(button));
}

/// The view controller currently on top (presented sheets included).
pub fn top_controller(root: &UIViewController) -> Retained<UIViewController> {
    let mut top = root.retain();
    while let Some(next) = top.presentedViewController() {
        top = next;
    }
    top
}

use objc2::Message;

/// Presents an alert or action sheet; reports the chosen action index.
pub fn show_alert(
    mtm: MainThreadMarker,
    root: &UIViewController,
    spec: &AlertSpec,
    report: fn(UiEvent),
) {
    let style = if spec.style == prism_core::menu::AlertStyle::ActionSheet {
        UIAlertControllerStyle::ActionSheet
    } else {
        UIAlertControllerStyle::Alert
    };
    let alert = UIAlertController::alertControllerWithTitle_message_preferredStyle(
        Some(&NSString::from_str(&spec.title)),
        spec.message.as_deref().map(NSString::from_str).as_deref(),
        style,
        mtm,
    );
    for (index, (title, role)) in spec.actions.iter().enumerate() {
        let id = spec.id;
        let handler = block2::RcBlock::new(move |_: NonNull<UIAlertAction>| {
            report(UiEvent::AlertAction(id, index))
        });
        let style = match role {
            ActionRole::Normal => UIAlertActionStyle::Default,
            ActionRole::Destructive => UIAlertActionStyle::Destructive,
            ActionRole::Cancel => UIAlertActionStyle::Cancel,
        };
        let action = UIAlertAction::actionWithTitle_style_handler(
            Some(&NSString::from_str(title)),
            style,
            Some(&handler),
            mtm,
        );
        alert.addAction(&action);
    }
    let top = top_controller(root);
    // iPad shows action sheets as popovers: anchor them to the center.
    if let Some(popover) = alert.popoverPresentationController()
        && let Some(view) = top.view()
    {
        popover.setSourceView(Some(&view));
        let b = view.bounds();
        popover.setSourceRect(objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(b.size.width / 2., b.size.height / 2.),
            objc2_core_foundation::CGSize::new(0., 0.),
        ));
    }
    top.presentViewController_animated_completion(&alert, true, None);
}
