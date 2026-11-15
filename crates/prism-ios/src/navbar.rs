//! A screen's navigation bar options (#30): `UIBarButtonItem`s for its bar
//! buttons, `largeTitleDisplayMode` for its title (not on tvOS, which has
//! no large titles), and the tint of the navigation and tab bars.
//!
//! A screen's config arrives before its view controller exists (it is made
//! when the screen joins a stack), so configs are kept by screen id and
//! applied when the controller is made, and on every change.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIBarButtonItem, UIBarButtonItemStyle, UIColor, UIImage,
    UINavigationController, UITabBarController, UIViewController,
};
use prism_core::navbar::{BarItem, ScreenConfig};
use prism_core::{NodeId, UiEvent};

/// Bar buttons per screen: the tag of an item is `screen * 256 + index`.
const PER_SCREEN: isize = 256;

thread_local! {
    static CONFIGS: RefCell<HashMap<NodeId, ScreenConfig>> = RefCell::default();
    static TARGET: Retained<BarTarget> = {
        let mtm = MainThreadMarker::new().expect("main thread");
        let this = BarTarget::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    };
}

define_class!(
    /// The target of every bar button: reports the tap to the screen.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismBarTarget"]
    struct BarTarget;

    unsafe impl NSObjectProtocol for BarTarget {}

    impl BarTarget {
        #[unsafe(method(onBarButton:))]
        fn on_bar_button(&self, sender: &UIBarButtonItem) {
            let tag = sender.tag();
            let (screen, index) = (tag / PER_SCREEN, tag % PER_SCREEN);
            crate::imp::dispatch(UiEvent::Change(screen as NodeId, index as f64));
        }
    }
);

/// Records screen `id`'s config and applies it to its controller, if any.
pub(crate) fn set_config(id: NodeId, config: &str, vc: Option<&UIViewController>) {
    CONFIGS.with(|c| c.borrow_mut().insert(id, ScreenConfig::decode(config)));
    if let Some(vc) = vc {
        apply(vc, id);
    }
}

/// Forgets a destroyed screen.
pub(crate) fn forget(id: NodeId) {
    CONFIGS.with(|c| c.borrow_mut().remove(&id));
}

/// The screen a controller hosts (its view's first subview).
fn screen_of(vc: &UIViewController) -> Option<NodeId> {
    vc.viewIfLoaded()
        .and_then(|v| v.subviews().firstObject())
        .map(|v| v.tag() as NodeId)
}

/// Applies the bar options of the screen `vc` hosts (after the stack's
/// Close button changed, for instance).
pub(crate) fn refresh(vc: &UIViewController) {
    if let Some(id) = screen_of(vc) {
        apply(vc, id);
    }
}

fn item(
    mtm: MainThreadMarker,
    screen: NodeId,
    index: usize,
    it: &BarItem,
) -> Retained<UIBarButtonItem> {
    let image = (!it.icon.is_empty())
        .then(|| UIImage::systemImageNamed(&NSString::from_str(&it.icon)))
        .flatten();
    let title = NSString::from_str(if it.title.is_empty() {
        &it.icon
    } else {
        &it.title
    });
    let button = TARGET.with(|t| {
        let target: &AnyObject = t;
        unsafe {
            match &image {
                Some(image) => UIBarButtonItem::initWithImage_style_target_action(
                    UIBarButtonItem::alloc(mtm),
                    Some(image),
                    UIBarButtonItemStyle::Plain,
                    Some(target),
                    Some(sel!(onBarButton:)),
                ),
                None => UIBarButtonItem::initWithTitle_style_target_action(
                    UIBarButtonItem::alloc(mtm),
                    Some(&title),
                    UIBarButtonItemStyle::Plain,
                    Some(target),
                    Some(sel!(onBarButton:)),
                ),
            }
        }
    });
    button.setTag(screen as isize * PER_SCREEN + index as isize);
    button.setAccessibilityLabel(Some(&NSString::from_str(it.spoken())), mtm);
    button
}

/// Applies screen `screen`'s bar options to the controller hosting it.
pub(crate) fn apply(vc: &UIViewController, screen: NodeId) {
    let mtm = vc.mtm();
    let config = CONFIGS
        .with(|c| c.borrow().get(&screen).cloned())
        .unwrap_or_default();
    let nav = vc.navigationItem();
    // A sheet's Close button (the stack's) stays first.
    let close = nav
        .leftBarButtonItems()
        .and_then(|items| items.firstObject())
        .filter(|i| unsafe { i.action() } == Some(sel!(onCloseStack:)));
    let mut left: Vec<Retained<UIBarButtonItem>> = close.into_iter().collect();
    left.extend(config.leading().map(|(i, it)| item(mtm, screen, i, it)));
    // Right items go from the trailing edge inwards: the first button added
    // is the innermost, so the bar reads in the order they were added.
    let right: Vec<Retained<UIBarButtonItem>> = config
        .trailing()
        .map(|(i, it)| item(mtm, screen, i, it))
        .collect();
    let right: Vec<Retained<UIBarButtonItem>> = right.into_iter().rev().collect();
    nav.setLeftBarButtonItems(Some(&NSArray::from_retained_slice(&left)));
    nav.setRightBarButtonItems(Some(&NSArray::from_retained_slice(&right)));
    #[cfg(not(target_os = "tvos"))]
    {
        use objc2_ui_kit::UINavigationItemLargeTitleDisplayMode as Mode;
        // Back stays next to the leading buttons.
        nav.setLeftItemsSupplementBackButton(true);
        nav.setLargeTitleDisplayMode(match config.large_title {
            Some(true) => Mode::Always,
            Some(false) => Mode::Never,
            None => Mode::Automatic,
        });
    }
}

/// `UITabBar.tintColor`: the selected tab.
pub(crate) fn tint_tabs(tabs: &UITabBarController, tint: Option<&UIColor>) {
    unsafe { tabs.tabBar().setTintColor(tint) };
}

/// `UINavigationBar.tintColor`: Back and the bar buttons.
pub(crate) fn tint_stack(nav: &UINavigationController, tint: Option<&UIColor>) {
    unsafe { nav.navigationBar().setTintColor(tint) };
}
