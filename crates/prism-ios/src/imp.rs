use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, NSTextAlignment, UIAccessibilityTraitButton,
    UIAccessibilityTraitHeader, UIAccessibilityTraitImage, UIAccessibilityTraitLink,
    UIAccessibilityTraitNotEnabled, UIAccessibilityTraitSelected, UIAccessibilityTraitStaticText,
    UIAccessibilityTraitTabBar, UIAccessibilityTraitToggleButton, UIApplication,
    UIApplicationDelegate, UIButton, UIButtonType, UIColor, UIControlEvents, UIControlState,
    UIFont, UIFontWeightBold, UIFontWeightMedium, UIFontWeightRegular, UIFontWeightSemibold,
    UIImage, UILabel, UINavigationController, UINavigationControllerDelegate, UIResponder, UIScene,
    UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIScrollView,
    UIScrollViewContentInsetAdjustmentBehavior, UIScrollViewKeyboardDismissMode, UISwitch,
    UITabBarController, UITabBarControllerDelegate, UITapGestureRecognizer, UITextBorderStyle,
    UITextField, UIView, UIViewController, UIWindow, UIWindowScene, UIWindowSceneDelegate,
};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, FontWeight, NodeId, NodeKind,
    Role, Runtime, Style, TextAlign, UiEvent,
};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

static APP: OnceLock<(AppConfig, AppInit)> = OnceLock::new();

// ---------------------------------------------------------------------------
// Main-thread state
// ---------------------------------------------------------------------------

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<UiKit>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|cell| {
        // The platform can call back while we mutate views (layout passes,
        // navigation callbacks). We always re-layout after applying, so a
        // nested callback is safely skipped.
        if let Ok(mut guard) = cell.try_borrow_mut()
            && let Some(state) = guard.as_mut()
        {
            f(state)
        }
    });
}

fn dispatch(event: UiEvent) {
    with_state(|s| {
        let patches = s.runtime.dispatch(event);
        if !patches.is_empty() {
            s.renderer.apply(&patches);
            s.relayout();
        }
    });
}

impl State {
    fn relayout(&mut self) {
        let root = &self.renderer.widgets.root;
        let size = root.bounds().size;
        let i = root.safeAreaInsets();
        let insets = Insets {
            top: i.top as f32,
            right: i.right as f32,
            bottom: i.bottom as f32,
            left: i.left as f32,
        };
        self.renderer
            .layout(size.width as f32, size.height as f32, insets);
    }
}

// ---------------------------------------------------------------------------
// Objective-C classes
// ---------------------------------------------------------------------------

define_class!(
    /// Receives control events and taps; routes them by view tag (= node id).
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTarget"]
    pub struct Target;

    impl Target {
        #[unsafe(method(onAction:))]
        fn on_action(&self, sender: &UIView) {
            dispatch(UiEvent::Click(sender.tag() as NodeId));
        }

        #[unsafe(method(onEditing:))]
        fn on_editing(&self, sender: &UITextField) {
            let text = sender.text().map(|t| t.to_string()).unwrap_or_default();
            dispatch(UiEvent::Input(sender.tag() as NodeId, text));
        }

        #[unsafe(method(onToggle:))]
        fn on_toggle(&self, sender: &UISwitch) {
            let value = if sender.isOn() { 1. } else { 0. };
            dispatch(UiEvent::Change(sender.tag() as NodeId, value));
        }

        #[unsafe(method(onTap:))]
        fn on_tap(&self, sender: &UITapGestureRecognizer) {
            if let Some(view) = sender.view() {
                dispatch(UiEvent::Click(view.tag() as NodeId));
            }
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl UITabBarControllerDelegate for Target {
        #[unsafe(method(tabBarController:didSelectViewController:))]
        fn did_select(&self, tabs: &UITabBarController, _controller: &UIViewController) {
            if let Some(view) = tabs.viewIfLoaded() {
                dispatch(UiEvent::Change(view.tag() as NodeId, tabs.selectedIndex() as f64));
            }
        }
    }

    unsafe impl UINavigationControllerDelegate for Target {
        /// Fires after pushes, pops and swipe-back. Reports the native depth;
        /// the router ignores it when it already matches.
        #[unsafe(method(navigationController:didShowViewController:animated:))]
        fn did_show(
            &self,
            nav: &UINavigationController,
            _controller: &UIViewController,
            _animated: bool,
        ) {
            let depth = nav.viewControllers().count();
            if let Some(view) = nav.viewIfLoaded() {
                dispatch(UiEvent::Back(view.tag() as NodeId, depth));
            }
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// Re-runs layout whenever UIKit lays out the root (rotation, safe area, ...).
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismViewController"]
    pub struct PrismViewController;

    impl PrismViewController {
        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            with_state(State::relayout);
        }
    }
);

impl PrismViewController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    /// Hosts one Prism screen inside a `UINavigationController`.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismScreenController"]
    pub struct ScreenController;

    impl ScreenController {
        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            with_state(State::relayout);
        }
    }
);

impl ScreenController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismAppDelegate"]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}
    unsafe impl UIApplicationDelegate for AppDelegate {}
);

#[derive(Default)]
pub struct SceneIvars {
    window: RefCell<Option<Retained<UIWindow>>>,
}

define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismSceneDelegate"]
    #[ivars = SceneIvars]
    pub struct SceneDelegate;

    impl SceneDelegate {
        #[unsafe(method_id(init))]
        fn init(this: objc2::rc::Allocated<Self>) -> Option<Retained<Self>> {
            unsafe { msg_send![super(this.set_ivars(SceneIvars::default())), init] }
        }
    }

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn will_connect(
            &self,
            scene: &UIScene,
            _session: &UISceneSession,
            _options: &UISceneConnectionOptions,
        ) {
            let Ok(scene) = scene.retain().downcast::<UIWindowScene>() else { return };
            let window = connect(self.mtm(), &scene);
            *self.ivars().window.borrow_mut() = Some(window);
        }
    }

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}
);

fn connect(mtm: MainThreadMarker, scene: &UIWindowScene) -> Retained<UIWindow> {
    let (_config, init) = APP.get().expect("prism-ios: run() was not called");
    let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
    let controller = PrismViewController::new(mtm);
    let root = controller.view().expect("view controller has a view");
    root.setBackgroundColor(Some(&UIColor::systemBackgroundColor()));

    let path = std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into());
    let mut runtime = Runtime::with_path(init, &path);
    let mut renderer = NativeRenderer::new(UiKit {
        mtm,
        target: Target::new(mtm),
        root,
        controller: controller.clone(),
        screens: HashMap::new(),
        titles: HashMap::new(),
        icons: HashMap::new(),
        selected: HashMap::new(),
    });
    renderer.apply(&runtime.render());
    STATE.with(|cell| *cell.borrow_mut() = Some(State { runtime, renderer }));

    window.setRootViewController(Some(&controller));
    window.makeKeyAndVisible();
    window
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

pub struct NavStack {
    nav: Retained<UINavigationController>,
    view: Retained<UIView>,
}

pub struct TabBar {
    controller: Retained<UITabBarController>,
    view: Retained<UIView>,
}

/// Makes `child` a child view controller of the nearest controller above its
/// view (stacks nested in tabs or screens), so it gets safe-area, rotation and
/// appearance callbacks.
fn ensure_contained(child: &UIViewController, view: &UIView) {
    if child.parentViewController().is_some() {
        return;
    }
    let mut responder = view.superview().map(Retained::into_super);
    while let Some(r) = responder {
        if let Ok(parent) = r.clone().downcast::<UIViewController>() {
            parent.addChildViewController(child);
            child.didMoveToParentViewController(Some(&parent));
            return;
        }
        responder = r.nextResponder();
    }
}

#[derive(Clone)]
pub enum View {
    Container(Retained<UIView>),
    Screen(Retained<UIView>),
    Stack(Rc<NavStack>),
    Tabs(Rc<TabBar>),
    Tab(Retained<UIView>),
    Scroll(Retained<UIScrollView>),
    Toggle(Retained<UISwitch>),
    Label(Retained<UILabel>),
    Button(Retained<UIButton>),
    Input(Retained<UITextField>),
}

impl View {
    fn ui(&self) -> &UIView {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) => v,
            View::Stack(s) => &s.view,
            View::Tabs(t) => &t.view,
            View::Scroll(v) => v,
            View::Toggle(v) => v,
            View::Label(v) => v,
            View::Button(v) => v,
            View::Input(v) => v,
        }
    }

    fn id(&self) -> NodeId {
        self.ui().tag() as NodeId
    }
}

pub struct UiKit {
    mtm: MainThreadMarker,
    target: Retained<Target>,
    root: Retained<UIView>,
    controller: Retained<PrismViewController>,
    /// One view controller per child of a navigation stack.
    screens: HashMap<NodeId, Retained<ScreenController>>,
    titles: HashMap<NodeId, String>,
    icons: HashMap<NodeId, String>,
    /// Selected index of each tab bar, applied once its tabs exist.
    selected: HashMap<NodeId, usize>,
}

impl UiKit {
    /// The view controller hosting `view` in a stack (created on first use).
    fn screen_controller(&mut self, view: &View) -> Retained<ScreenController> {
        let id = view.id();
        let mtm = self.mtm;
        let vc = self
            .screens
            .entry(id)
            .or_insert_with(|| ScreenController::new(mtm))
            .clone();
        let host = vc.view().expect("view controller has a view");
        host.setBackgroundColor(Some(&UIColor::systemBackgroundColor()));
        if view.ui().superview().as_deref() != Some(&*host) {
            host.addSubview(view.ui());
        }
        let title = self.titles.get(&id).map(|t| NSString::from_str(t));
        vc.setTitle(title.as_deref());
        if let (Some(item), Some(icon)) = (vc.tabBarItem(), self.icons.get(&id)) {
            item.setImage(UIImage::systemImageNamed(&NSString::from_str(icon)).as_deref());
        }
        vc
    }
}

fn ui_color(c: Color) -> Retained<UIColor> {
    UIColor::colorWithRed_green_blue_alpha(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

fn font(style: &Style) -> Retained<UIFont> {
    let weight = unsafe {
        match style.font_weight {
            FontWeight::Regular => UIFontWeightRegular,
            FontWeight::Medium => UIFontWeightMedium,
            FontWeight::Semibold => UIFontWeightSemibold,
            FontWeight::Bold => UIFontWeightBold,
        }
    };
    UIFont::systemFontOfSize_weight(style.font_size as f64, weight)
}

fn alignment(a: TextAlign) -> NSTextAlignment {
    match a {
        TextAlign::Left => NSTextAlignment::Left,
        TextAlign::Center => NSTextAlignment::Center,
        TextAlign::Right => NSTextAlignment::Right,
    }
}

impl Widgets for UiKit {
    type View = View;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> View {
        let mtm = self.mtm;
        let target: &AnyObject = &self.target;
        let view = match kind {
            NodeKind::View => View::Container(UIView::new(mtm)),
            NodeKind::Screen => View::Screen(UIView::new(mtm)),
            NodeKind::Stack => {
                let nav = UINavigationController::new(mtm);
                nav.navigationBar().setPrefersLargeTitles(true);
                unsafe { nav.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
                let view = nav.view().expect("navigation controller has a view");
                View::Stack(Rc::new(NavStack { nav, view }))
            }
            NodeKind::Tabs => {
                let controller = UITabBarController::new(mtm);
                controller.setDelegate(Some(ProtocolObject::from_ref(&*self.target)));
                let view = controller.view().expect("tab bar controller has a view");
                View::Tabs(Rc::new(TabBar { controller, view }))
            }
            NodeKind::Tab => View::Tab(UIView::new(mtm)),
            NodeKind::Scroll => {
                let s = UIScrollView::new(mtm);
                s.setAlwaysBounceVertical(true);
                s.setKeyboardDismissMode(UIScrollViewKeyboardDismissMode::OnDrag);
                // UIKit insets content under bars (screens let a leading scroll
                // view run under them, see prism-native).
                s.setContentInsetAdjustmentBehavior(
                    UIScrollViewContentInsetAdjustmentBehavior::Automatic,
                );
                View::Scroll(s)
            }
            NodeKind::Toggle => {
                let t = UISwitch::new(mtm);
                unsafe {
                    t.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onToggle:),
                        UIControlEvents::ValueChanged,
                    )
                };
                View::Toggle(t)
            }
            NodeKind::Text => {
                let l = UILabel::new(mtm);
                l.setNumberOfLines(0);
                View::Label(l)
            }
            NodeKind::Button => {
                let b = UIButton::buttonWithType(UIButtonType::System, mtm);
                unsafe {
                    b.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onAction:),
                        UIControlEvents::TouchUpInside,
                    )
                };
                View::Button(b)
            }
            NodeKind::TextInput => {
                let f = UITextField::new(mtm);
                f.setBorderStyle(UITextBorderStyle::RoundedRect);
                unsafe {
                    f.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onEditing:),
                        UIControlEvents::EditingChanged,
                    )
                };
                View::Input(f)
            }
        };
        view.ui().setTag(id as isize);
        view
    }

    fn set_style(&mut self, view: &View, style: &Style) {
        let v = view.ui();
        v.setAlpha(style.opacity as f64);
        if !matches!(view, View::Input(_)) {
            // `nil` renders black for some UIKit views; be explicit.
            let bg = style.background.map_or_else(UIColor::clearColor, ui_color);
            v.setBackgroundColor(Some(&bg));
        }
        let layer = v.layer();
        layer.setCornerRadius(style.corner_radius as f64);
        layer.setBorderWidth(style.border_width as f64);
        let border = style.border_color.map(|c| unsafe { ui_color(c).CGColor() });
        layer.setBorderColor(border.as_deref());

        let text_color = style.text_color.map(ui_color);
        match view {
            View::Container(_)
            | View::Screen(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Tab(_)
            | View::Scroll(_)
            | View::Toggle(_) => {}
            View::Label(l) => {
                unsafe { l.setFont(Some(&font(style))) };
                unsafe { l.setTextColor(Some(&text_color.unwrap_or_else(UIColor::labelColor))) };
                l.setTextAlignment(alignment(style.text_align));
            }
            View::Input(f) => {
                f.setFont(Some(&font(style)));
                f.setTextColor(Some(&text_color.unwrap_or_else(UIColor::labelColor)));
                f.setTextAlignment(alignment(style.text_align));
            }
            View::Button(b) => {
                if let Some(label) = b.titleLabel() {
                    unsafe { label.setFont(Some(&font(style))) };
                }
                b.setTitleColor_forState(text_color.as_deref(), UIControlState::Normal);
            }
        }
    }

    fn set_text(&mut self, view: &View, text: &str) {
        let s = NSString::from_str(text);
        match view {
            View::Label(l) => l.setText(Some(&s)),
            View::Input(f) => {
                if f.text().map(|t| t.to_string()).unwrap_or_default() != text {
                    f.setText(Some(&s));
                }
            }
            View::Button(b) => b.setTitle_forState(Some(&s), UIControlState::Normal),
            View::Screen(_) | View::Tab(_) => {
                let id = view.id();
                self.titles.insert(id, text.to_string());
                if let Some(vc) = self.screens.get(&id) {
                    vc.setTitle(Some(&s));
                }
            }
            _ => {}
        }
    }

    fn set_placeholder(&mut self, view: &View, text: &str) {
        if let View::Input(f) = view {
            f.setPlaceholder(Some(&NSString::from_str(text)));
        }
    }

    fn set_clickable(&mut self, view: &View, clickable: bool) {
        let View::Container(v) = view else { return };
        if let Some(existing) = v.gestureRecognizers() {
            for g in existing.iter() {
                v.removeGestureRecognizer(&g);
            }
        }
        if clickable {
            let target: &AnyObject = &self.target;
            let tap = unsafe {
                UITapGestureRecognizer::initWithTarget_action(
                    UITapGestureRecognizer::alloc(self.mtm),
                    Some(target),
                    Some(sel!(onTap:)),
                )
            };
            v.addGestureRecognizer(&tap);
        }
    }

    fn set_children(&mut self, parent: &View, children: &[View]) {
        if let View::Stack(stack) = parent {
            let controllers: Vec<Retained<UIViewController>> = children
                .iter()
                .map(|c| Retained::into_super(self.screen_controller(c)))
                .collect();
            let current = stack.nav.viewControllers();
            let unchanged = current.count() == controllers.len()
                && current.iter().zip(&controllers).all(|(a, b)| *a == **b);
            if !unchanged {
                // Animate pushes/pops, but not the very first screen(s).
                let animated = current.count() > 0;
                stack.nav.setViewControllers_animated(
                    &NSArray::from_retained_slice(&controllers),
                    animated,
                );
            }
            return;
        }
        if let View::Tabs(bar) = parent {
            let controllers: Vec<Retained<UIViewController>> = children
                .iter()
                .map(|c| Retained::into_super(self.screen_controller(c)))
                .collect();
            bar.controller.setViewControllers_animated(
                Some(&NSArray::from_retained_slice(&controllers)),
                false,
            );
            if let Some(i) = self.selected.get(&parent.id()) {
                bar.controller.setSelectedIndex(*i);
            }
            return;
        }
        let parent = parent.ui();
        for (i, child) in children.iter().enumerate() {
            parent.insertSubview_atIndex(child.ui(), i as isize);
        }
        // Remove former Prism children only (tag > 0); UIKit's own subviews,
        // like scroll indicators, have tag 0.
        let keep: Vec<isize> = children.iter().map(|c| c.ui().tag()).collect();
        for sub in parent.subviews().iter() {
            if sub.tag() > 0 && !keep.contains(&sub.tag()) {
                sub.removeFromSuperview();
            }
        }
    }

    fn set_root(&mut self, view: &View) {
        for sub in self.root.subviews().iter() {
            sub.removeFromSuperview();
        }
        for child in self.controller.childViewControllers().iter() {
            child.removeFromParentViewController();
        }
        // Proper containment, so container controllers get the safe area,
        // rotation and appearance callbacks.
        let child: Option<Retained<UIViewController>> = match view {
            View::Stack(stack) => Some(Retained::into_super(stack.nav.clone())),
            View::Tabs(bar) => Some(Retained::into_super(bar.controller.clone())),
            _ => None,
        };
        if let Some(child) = &child {
            self.controller.addChildViewController(child);
        }
        self.root.addSubview(view.ui());
        if let Some(child) = &child {
            child.didMoveToParentViewController(Some(&self.controller));
        }
    }

    fn destroy(&mut self, view: &View) {
        let id = view.id();
        self.screens.remove(&id);
        self.titles.remove(&id);
        self.icons.remove(&id);
        self.selected.remove(&id);
        view.ui().removeFromSuperview();
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        let limit = CGSize::new(max_width.map_or(f64::MAX / 4., |w| w as f64), f64::MAX / 4.);
        let size = match view {
            View::Toggle(t) => t.sizeThatFits(limit),
            View::Container(_)
            | View::Screen(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Tab(_)
            | View::Scroll(_) => CGSize::ZERO,
            View::Input(f) => {
                let s = f.sizeThatFits(limit);
                CGSize::new(s.width.max(120.), s.height.max(36.))
            }
            other => other.ui().sizeThatFits(limit),
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        match view {
            View::Stack(st) => ensure_contained(&st.nav, &st.view),
            View::Tabs(bar) => ensure_contained(&bar.controller, &bar.view),
            _ => {}
        }
        view.ui().setFrame(CGRect::new(
            CGPoint::new(x as f64, y as f64),
            CGSize::new(w as f64, h as f64),
        ));
    }

    fn set_value(&mut self, view: &View, value: f64) {
        match view {
            View::Toggle(t) => {
                if t.isOn() != (value != 0.) {
                    t.setOn_animated(value != 0., true);
                }
            }
            View::Tabs(bar) => {
                let i = value.max(0.) as usize;
                self.selected.insert(view.id(), i);
                if bar.controller.selectedIndex() != i {
                    bar.controller.setSelectedIndex(i);
                }
            }
            _ => {}
        }
    }

    fn set_icon(&mut self, view: &View, icon: &str) {
        let id = view.id();
        self.icons.insert(id, icon.to_string());
        if let Some(item) = self.screens.get(&id).and_then(|vc| vc.tabBarItem()) {
            item.setImage(UIImage::systemImageNamed(&NSString::from_str(icon)).as_deref());
        }
    }

    fn set_appearance(&mut self, appearance: Appearance) {
        let style = match appearance {
            Appearance::System => objc2_ui_kit::UIUserInterfaceStyle::Unspecified,
            Appearance::Light => objc2_ui_kit::UIUserInterfaceStyle::Light,
            Appearance::Dark => objc2_ui_kit::UIUserInterfaceStyle::Dark,
        };
        self.controller.setOverrideUserInterfaceStyle(style);
    }

    fn set_content_size(&mut self, view: &View, width: f32, height: f32) {
        if let View::Scroll(s) = view {
            s.setContentSize(CGSize::new(width as f64, height as f64));
        }
    }

    fn set_accessibility(&mut self, view: &View, a: &Accessibility) {
        let mtm = self.mtm;
        let v = view.ui();
        let ns = |s: &Option<String>| s.as_deref().map(NSString::from_str);
        v.setAccessibilityElementsHidden(a.hidden, mtm);
        // Platform containers announce themselves through their titles.
        if matches!(
            view,
            View::Stack(_) | View::Tabs(_) | View::Tab(_) | View::Screen(_) | View::Scroll(_)
        ) {
            return;
        }
        let native_control = matches!(view, View::Button(_) | View::Input(_) | View::Toggle(_));
        if !native_control {
            // Operable containers and images are single elements; plain text
            // labels already are; groups gather their children.
            let element = a
                .role
                .is_some_and(|r| r.is_interactive() || r == Role::Image)
                || matches!(view, View::Label(_));
            v.setIsAccessibilityElement(element && !a.hidden, mtm);
            v.setShouldGroupAccessibilityChildren(a.role == Some(Role::Group), mtm);
            let mut traits = 0;
            unsafe {
                traits |= match a.role {
                    Some(Role::Button) => UIAccessibilityTraitButton,
                    Some(Role::Link) => UIAccessibilityTraitLink,
                    Some(Role::Heading(_)) => UIAccessibilityTraitHeader,
                    Some(Role::Image) => UIAccessibilityTraitImage,
                    Some(Role::Text) => UIAccessibilityTraitStaticText,
                    Some(Role::Switch | Role::Checkbox) => {
                        UIAccessibilityTraitToggleButton | UIAccessibilityTraitButton
                    }
                    Some(Role::Tab) => UIAccessibilityTraitButton,
                    Some(Role::TabList) => UIAccessibilityTraitTabBar,
                    _ => 0,
                };
                if a.disabled {
                    traits |= UIAccessibilityTraitNotEnabled;
                }
                if a.checked == Some(true) && matches!(a.role, Some(Role::Tab | Role::Button)) {
                    traits |= UIAccessibilityTraitSelected;
                }
            }
            v.setAccessibilityTraits(traits, mtm);
        }
        // Labels override the visible text only when given.
        if a.label.is_some() || !native_control {
            v.setAccessibilityLabel(ns(&a.label).as_deref(), mtm);
        }
        v.setAccessibilityHint(ns(&a.hint).as_deref(), mtm);
        let value = a.value.clone().or_else(|| match (a.role, a.checked) {
            (Some(Role::Switch | Role::Checkbox), Some(on)) if !native_control => {
                Some(if on { "1".into() } else { "0".into() })
            }
            _ => None,
        });
        if value.is_some() || !native_control {
            v.setAccessibilityValue(ns(&value).as_deref(), mtm);
        }
    }

    fn screen_viewport(&mut self, stack: &View, screen: &View) -> Viewport {
        // A tab hosts a navigation stack, which handles the safe area itself.
        let own_insets = !matches!(screen, View::Tab(_));
        let host = self
            .screens
            .get(&screen.id())
            .and_then(|vc| vc.view())
            .unwrap_or_else(|| stack.ui().retain());
        let b = host.bounds();
        let i = if own_insets {
            host.safeAreaInsets()
        } else {
            objc2_ui_kit::UIEdgeInsets {
                top: 0.,
                left: 0.,
                bottom: 0.,
                right: 0.,
            }
        };
        Viewport {
            x: 0.,
            y: 0.,
            width: b.size.width as f32,
            height: b.size.height as f32,
            insets: Insets {
                top: i.top as f32,
                right: i.right as f32,
                bottom: i.bottom as f32,
                left: i.left as f32,
            },
        }
    }
}

/// Starts UIKit. Never returns.
pub fn run(config: AppConfig, init: AppInit) {
    let mtm = MainThreadMarker::new().expect("prism-ios: run() must be called on the main thread");
    let _ = APP.set((config, init));
    // Register the classes UIKit instantiates by name (Info.plist, UIApplicationMain).
    let _ = (AppDelegate::class(), SceneDelegate::class());
    let delegate = NSString::from_str("PrismAppDelegate");
    UIApplication::main(None, Some(&delegate), mtm);
}
