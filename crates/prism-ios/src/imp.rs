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
    UIAccessibilityTraitTabBar, UIAccessibilityTraitToggleButton,
    UIAdaptivePresentationControllerDelegate, UIApplication, UIApplicationDelegate, UIButton,
    UIButtonType, UIColor, UIContextMenuInteractionDelegate, UIControlEvents, UIControlState,
    UIFont, UIFontWeightBold, UIFontWeightMedium, UIFontWeightRegular, UIFontWeightSemibold,
    UIImage, UIImageSymbolConfiguration, UIImageView, UILabel, UINavigationController,
    UINavigationControllerDelegate, UIProgressView, UIResponder, UIScene, UISceneConnectionOptions,
    UISceneDelegate, UISceneSession, UIScrollView, UIScrollViewContentInsetAdjustmentBehavior,
    UIScrollViewDelegate, UIScrollViewKeyboardDismissMode, UISwitch, UITabBarController,
    UITabBarControllerDelegate, UITapGestureRecognizer, UITextBorderStyle, UITextField,
    UITraitCollection, UIUserInterfaceStyle, UIView, UIViewController, UIWindow, UIWindowScene,
    UIWindowSceneDelegate,
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
    services: crate::services::IosServices,
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

pub(crate) fn dispatch(event: UiEvent) {
    with_state(|s| {
        let patches = s.runtime.dispatch(event);
        s.commit(patches);
    });
}

/// Opens a deep link (widget, Live Activity or notification tap).
pub(crate) fn open_link(path: &str) {
    with_state(|s| {
        let patches = s.runtime.open_link(path);
        s.commit(patches);
    });
}

/// `prism-counter:/todos/1` → `/todos/1`.
fn link_from_url(url: &objc2_foundation::NSURL) -> Option<String> {
    let s = url.absoluteString()?.to_string();
    let path = s.split_once(':')?.1.trim_start_matches("//");
    Some(if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    })
}

impl State {
    /// Applies patches, lays out, and hands platform requests to the OS.
    fn commit(&mut self, patches: Vec<prism_core::Patch>) {
        if !patches.is_empty() {
            self.renderer.apply(&patches);
            self.relayout();
        }
        for request in self.runtime.take_requests() {
            prism_core::PlatformServices::handle(&mut self.services, request);
        }
        schedule_timer(self.runtime.next_timer());
    }

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

        /// Text size, reduce motion or language changed in Settings.
        #[unsafe(method(onEnvironment:))]
        fn on_environment(&self, _n: &objc2_foundation::NSNotification) {
            with_state(|s| {
                s.runtime.app.set_environment(ios_environment());
                let patches = s.runtime.render();
                s.commit(patches);
            });
        }

        /// The keyboard moves: shrink the safe area so content stays visible.
        #[unsafe(method(onKeyboard:))]
        fn on_keyboard(&self, n: &objc2_foundation::NSNotification) {
            keyboard_changed(n);
        }

        #[unsafe(method(onBeginEditing:))]
        fn on_begin_editing(&self, sender: &UITextField) {
            scroll_into_view(sender);
        }

        #[unsafe(method(onRefresh:))]
        fn on_refresh(&self, sender: &objc2_ui_kit::UIRefreshControl) {
            dispatch(UiEvent::Click(sender.tag() as NodeId));
        }

        #[unsafe(method(onSwipe:))]
        fn on_swipe(&self, sender: &objc2_ui_kit::UISwipeGestureRecognizer) {
            let dir = if sender.direction().contains(objc2_ui_kit::UISwipeGestureRecognizerDirection::Left) {
                prism_core::SwipeDirection::Left
            } else {
                prism_core::SwipeDirection::Right
            };
            if let Some(view) = sender.view() {
                dispatch(UiEvent::Swipe(view.tag() as NodeId, dir));
            }
        }

        #[unsafe(method(onTick:))]
        fn on_tick(&self, _timer: &AnyObject) {
            with_state(|s| s.renderer.widgets.tick());
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

    impl Target {
        /// Close button of a stack inside a sheet (tag = stack id).
        #[unsafe(method(onCloseStack:))]
        fn on_close_stack(&self, sender: &objc2_ui_kit::UIBarButtonItem) {
            dispatch(UiEvent::Back(sender.tag() as NodeId, 0));
        }
    }

    unsafe impl UIScrollViewDelegate for Target {
        /// Virtualized lists follow the scroll position.
        #[unsafe(method(scrollViewDidScroll:))]
        fn scroll_view_did_scroll(&self, s: &UIScrollView) {
            let y = s.contentOffset().y + s.adjustedContentInset().top;
            dispatch(UiEvent::Scroll(s.tag() as NodeId, y as f32, s.bounds().size.height as f32));
        }
    }

    unsafe impl UIContextMenuInteractionDelegate for Target {
        #[unsafe(method_id(contextMenuInteraction:configurationForMenuAtLocation:))]
        fn context_menu_configuration(
            &self,
            interaction: &objc2_ui_kit::UIContextMenuInteraction,
            _location: objc2_core_foundation::CGPoint,
        ) -> Option<Retained<objc2_ui_kit::UIContextMenuConfiguration>> {
            menu_configuration(self.mtm(), interaction)
        }
    }

    unsafe impl UIAdaptivePresentationControllerDelegate for Target {
        /// A sheet was swiped down: tell its Modal node.
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn presentation_did_dismiss(&self, pc: &objc2_ui_kit::UIPresentationController) {
            let host = pc.presentedViewController();
            if let Some(content) = host.view().and_then(|v| v.subviews().firstObject()) {
                let id = content.tag() as NodeId;
                with_state(|s| s.renderer.widgets.presented.retain(|p| *p != id));
                dispatch(UiEvent::Back(id, 0));
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
    unsafe impl UIApplicationDelegate for AppDelegate {
        /// The notification delegate must be set before launch finishes, or
        /// a tap that launched the app is lost.
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        fn did_finish_launching(&self, _app: &UIApplication, _options: Option<&AnyObject>) -> bool {
            crate::services::init_bridge();
            true
        }
    }
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
            // Cold launch from a widget or Live Activity.
            // `URLContexts` is nil on a normal launch (objc2 declares it non-null).
            let contexts: Option<Retained<objc2_foundation::NSSet<objc2_ui_kit::UIOpenURLContext>>> =
                unsafe { msg_send![_options, URLContexts] };
            for ctx in contexts.iter().flat_map(|c| c.iter()) {
                if let Some(path) = link_from_url(&ctx.URL()) {
                    open_link(&path);
                }
            }
        }

        #[unsafe(method(scene:openURLContexts:))]
        fn open_urls(&self, _scene: &UIScene, contexts: &objc2_foundation::NSSet<objc2_ui_kit::UIOpenURLContext>) {
            for ctx in contexts.iter() {
                if let Some(path) = link_from_url(&ctx.URL()) {
                    open_link(&path);
                }
            }
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
    // Dynamic Island iPhones have a taller status bar (≥ 54 pt).
    let island = scene
        .statusBarManager()
        .is_some_and(|m| m.statusBarFrame().size.height >= 54.);
    let services =
        crate::services::IosServices::new(island, Retained::into_super(controller.clone()));
    let mut runtime = Runtime::with_options(
        init,
        prism_core::RuntimeOptions {
            path,
            capabilities: prism_core::PlatformServices::capabilities(&services),
            storage: Some(Box::new(prism_core::FileStorage::new(data_dir()))),
            wake_ui: Some(wake_ui()),
            environment: ios_environment(),
            formatter: Some(Box::new(prism_native::apple::FoundationFormatter)),
        },
    );
    let renderer = NativeRenderer::new(UiKit {
        mtm,
        target: Target::new(mtm),
        root,
        controller: controller.clone(),
        screens: HashMap::new(),
        titles: HashMap::new(),
        icons: HashMap::new(),
        selected: HashMap::new(),
        timers: HashMap::new(),
        menus: HashMap::new(),
        presented: Vec::new(),
        pictures: HashMap::new(),
        image_cache: HashMap::new(),
        image_sources: image_sources(_config.assets.as_deref()),
        semantic: None,
    });
    let first = runtime.render();
    let mut state = State {
        runtime,
        renderer,
        services,
    };
    state.commit(first);
    STATE.with(|cell| *cell.borrow_mut() = Some(state));
    flush_responses();
    start_ticker(mtm);
    observe_system(mtm);

    window.setRootViewController(Some(&controller));
    window.makeKeyAndVisible();
    // Bars attached by UIKit after creation follow the app's direction too.
    with_state(|s| {
        if let Some(attr) = s.renderer.widgets.semantic {
            set_semantic(&window, attr);
        }
    });
    window
}

// ---------------------------------------------------------------------------
// Widgets
// ---------------------------------------------------------------------------

pub struct NavStack {
    nav: Retained<UINavigationController>,
    view: Retained<UIView>,
    /// Shows a Close button on the first screen (stacks inside sheets).
    closable: std::cell::Cell<bool>,
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
    /// The content of a sheet (hosted by a presented view controller).
    Sheet(Retained<UIView>),
    Tabs(Rc<TabBar>),
    Tab(Retained<UIView>),
    Scroll(Retained<UIScrollView>),
    Toggle(Retained<UISwitch>),
    Image(Retained<UIImageView>),
    Progress(Retained<UIProgressView>),
    Timer(Retained<UILabel>),
    Picture(Retained<UIImageView>),
    Label(Retained<UILabel>),
    Button(Retained<UIButton>),
    Input(Retained<UITextField>),
}

impl View {
    fn ui(&self) -> &UIView {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) => v,
            View::Stack(s) => &s.view,
            View::Sheet(v) => v,
            View::Tabs(t) => &t.view,
            View::Scroll(v) => v,
            View::Toggle(v) => v,
            View::Image(v) => v,
            View::Progress(v) => v,
            View::Timer(v) => v,
            View::Picture(v) => v,
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
    /// Countdown labels and their end time (Unix seconds).
    timers: HashMap<NodeId, (Retained<UILabel>, f64)>,
    /// Context menus by view id (read by the interaction delegate).
    menus: HashMap<NodeId, prism_core::menu::MenuSpec>,
    /// Pictures waiting for their image, by source.
    pictures: HashMap<String, Vec<(NodeId, Retained<UIImageView>)>>,
    /// Decoded images by source.
    image_cache: HashMap<String, Retained<UIImage>>,
    image_sources: prism_native::images::ImageSources,
    /// Layout direction of native chrome (set once the locale is known).
    semantic: Option<objc2_ui_kit::UISemanticContentAttribute>,
    /// Sheets currently presented, by modal id.
    presented: Vec<NodeId>,
}

impl UiKit {
    /// Shows `src` in `view`: from memory, or loaded on a background thread.
    fn load_picture(&mut self, id: NodeId, view: &Retained<UIImageView>, src: &str) {
        if let Some(img) = self.image_cache.get(src) {
            view.setImage(Some(img));
            return;
        }
        view.setImage(None);
        let waiting = self.pictures.entry(src.to_string()).or_default();
        waiting.retain(|(i, _)| *i != id);
        waiting.push((id, view.clone()));
        if waiting.len() > 1 {
            return; // already loading
        }
        let (src, sources) = (src.to_string(), self.image_sources.clone());
        std::thread::spawn(move || {
            let bytes = prism_native::images::load(&src, &sources);
            dispatch2::DispatchQueue::main().exec_async(move || {
                with_state(|s| {
                    s.renderer.widgets.picture_loaded(&src, bytes);
                    s.renderer.mark_all_dirty();
                    s.relayout();
                })
            });
        });
    }

    fn picture_loaded(&mut self, src: &str, bytes: Option<Vec<u8>>) {
        let waiting = self.pictures.remove(src).unwrap_or_default();
        let Some(img) =
            bytes.and_then(|b| UIImage::imageWithData(&objc2_foundation::NSData::from_vec(b)))
        else {
            eprintln!("prism-ios: could not load image {src:?}");
            return;
        };
        for (_, view) in waiting {
            view.setImage(Some(&img));
        }
        self.image_cache.insert(src.to_string(), img);
    }

    fn present_sheets(&mut self, sheets: &[View]) {
        for sheet in sheets {
            let id = sheet.id();
            if self.presented.contains(&id) {
                continue;
            }
            let vc = self.screen_controller(sheet);
            if let Some(pc) = vc.presentationController() {
                unsafe { pc.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
            }
            crate::menus::top_controller(&self.controller)
                .presentViewController_animated_completion(&vc, true, None);
            self.presented.push(id);
        }
        let keep: Vec<NodeId> = sheets.iter().map(View::id).collect();
        let gone: Vec<NodeId> = self
            .presented
            .iter()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        for id in gone {
            self.presented.retain(|p| *p != id);
            if let Some(vc) = self.screens.get(&id)
                && vc.presentingViewController().is_some()
            {
                vc.dismissViewControllerAnimated_completion(true, None);
            }
        }
    }

    /// The Close button of a stack inside a sheet.
    fn apply_close_button(&self, stack: &NavStack, stack_id: NodeId) {
        let Some(first) = stack.nav.viewControllers().firstObject() else {
            return;
        };
        if stack.closable.get() {
            let target: &AnyObject = &self.target;
            let item = unsafe {
                objc2_ui_kit::UIBarButtonItem::initWithBarButtonSystemItem_target_action(
                    objc2_ui_kit::UIBarButtonItem::alloc(self.mtm),
                    objc2_ui_kit::UIBarButtonSystemItem::Close,
                    Some(target),
                    Some(sel!(onCloseStack:)),
                )
            };
            item.setTag(stack_id as isize);
            first.navigationItem().setLeftBarButtonItem(Some(&item));
        } else {
            first.navigationItem().setLeftBarButtonItem(None);
        }
    }
}

impl UiKit {
    fn tick(&self) {
        for (label, end) in self.timers.values() {
            let text = NSString::from_str(&prism_core::format_remaining(*end));
            if label.text().as_deref() != Some(&*text) {
                label.setText(Some(&text));
            }
        }
    }
}

/// Ticks countdowns every second.
fn start_ticker(mtm: MainThreadMarker) {
    let target = Target::new(mtm);
    let _timer = unsafe {
        objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            &target,
            sel!(onTick:),
            None,
            true,
        )
    };
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

fn plain_color(c: Color) -> Retained<UIColor> {
    UIColor::colorWithRed_green_blue_alpha(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

/// Adaptive colors become dynamic `UIColor`s, resolved live by UIKit.
fn ui_color(c: Color) -> Retained<UIColor> {
    if c.dark.is_none() {
        return plain_color(c);
    }
    let (light, dark) = (c.light_variant(), c.dark_variant());
    let provider = block2::RcBlock::new(move |traits: std::ptr::NonNull<UITraitCollection>| {
        let traits = unsafe { traits.as_ref() };
        let style = unsafe { traits.userInterfaceStyle() };
        let pick = if style == UIUserInterfaceStyle::Dark {
            dark
        } else {
            light
        };
        let color = plain_color(pick);
        std::ptr::NonNull::new(Retained::autorelease_return(color)).expect("color")
    });
    unsafe { UIColor::colorWithDynamicProvider(&provider) }
}

fn font_weight(style: &Style) -> objc2_ui_kit::UIFontWeight {
    unsafe {
        match style.font_weight {
            FontWeight::Regular => UIFontWeightRegular,
            FontWeight::Medium => UIFontWeightMedium,
            FontWeight::Semibold => UIFontWeightSemibold,
            FontWeight::Bold => UIFontWeightBold,
        }
    }
}

fn symbol_weight(style: &Style) -> objc2_ui_kit::UIImageSymbolWeight {
    use objc2_ui_kit::UIImageSymbolWeight as W;
    match style.font_weight {
        FontWeight::Regular => W::Regular,
        FontWeight::Medium => W::Medium,
        FontWeight::Semibold => W::Semibold,
        FontWeight::Bold => W::Bold,
    }
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
            NodeKind::Modal => View::Sheet(UIView::new(mtm)),
            NodeKind::Stack => {
                let nav = UINavigationController::new(mtm);
                nav.navigationBar().setPrefersLargeTitles(true);
                unsafe { nav.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
                let view = nav.view().expect("navigation controller has a view");
                View::Stack(Rc::new(NavStack {
                    nav,
                    view,
                    closable: std::cell::Cell::new(false),
                }))
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
            NodeKind::Image => {
                let v = UIImageView::new(mtm);
                v.setContentMode(objc2_ui_kit::UIViewContentMode::Center);
                View::Image(v)
            }
            NodeKind::Progress => View::Progress(UIProgressView::new(mtm)),
            NodeKind::Picture => {
                let v = UIImageView::new(mtm);
                v.setClipsToBounds(true);
                v.setContentMode(objc2_ui_kit::UIViewContentMode::ScaleAspectFill);
                View::Picture(v)
            }
            NodeKind::Timer => {
                let l = UILabel::new(mtm);
                View::Timer(l)
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
                    );
                    f.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onBeginEditing:),
                        UIControlEvents::EditingDidBegin,
                    );
                };
                View::Input(f)
            }
        };
        view.ui().setTag(id as isize);
        if let Some(attr) = self.semantic {
            set_semantic(view.ui(), attr);
        }
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
            | View::Sheet(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Tab(_)
            | View::Scroll(_)
            | View::Picture(_)
            | View::Toggle(_) => {}
            View::Image(v) => {
                let config = UIImageSymbolConfiguration::configurationWithPointSize_weight(
                    style.font_size as f64,
                    symbol_weight(style),
                );
                v.setPreferredSymbolConfiguration(Some(&config));
                unsafe {
                    v.setTintColor(Some(
                        &text_color.clone().unwrap_or_else(UIColor::labelColor),
                    ))
                };
            }
            View::Progress(p) => p.setProgressTintColor(text_color.as_deref()),
            View::Label(l) | View::Timer(l) => {
                let f = if matches!(view, View::Timer(_)) {
                    // Digits keep a fixed width while the countdown ticks.
                    UIFont::monospacedDigitSystemFontOfSize_weight(
                        style.font_size as f64,
                        font_weight(style),
                    )
                } else {
                    font(style)
                };
                unsafe { l.setFont(Some(&f)) };
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
            View::Label(l) | View::Timer(l) => l.setText(Some(&s)),
            View::Picture(v) => self.load_picture(view.id(), v, text),
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
                    // A tab's item keeps its first title otherwise (language changes).
                    if matches!(view, View::Tab(_))
                        && let Some(item) = vc.tabBarItem()
                    {
                        item.setTitle(Some(&s));
                    }
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
        // A "clickable" scroll view is refreshable: pull to refresh.
        if let View::Scroll(s) = view {
            if clickable {
                let rc = objc2_ui_kit::UIRefreshControl::new(self.mtm);
                rc.setTag(s.tag());
                let target: &AnyObject = &self.target;
                unsafe {
                    rc.addTarget_action_forControlEvents(
                        Some(target),
                        sel!(onRefresh:),
                        UIControlEvents::ValueChanged,
                    )
                };
                s.setRefreshControl(Some(&rc));
            } else {
                s.setRefreshControl(None);
            }
            return;
        }
        let View::Container(v) = view else { return };
        if let Some(existing) = v.gestureRecognizers() {
            for g in existing.iter() {
                if g.isKindOfClass(UITapGestureRecognizer::class()) {
                    v.removeGestureRecognizer(&g);
                }
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
        // Sheets attached to a root container are presented modally.
        let (sheets, children): (Vec<View>, Vec<View>) = children
            .iter()
            .cloned()
            .partition(|c| matches!(c, View::Sheet(_)));
        if matches!(parent, View::Stack(_) | View::Tabs(_)) {
            self.present_sheets(&sheets);
        }
        let children = &children[..];
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
            self.apply_close_button(stack, parent.id());
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
        self.timers.remove(&id);
        self.pictures.retain(|_, ids| {
            ids.retain(|(i, _)| *i != id);
            !ids.is_empty()
        });
        view.ui().removeFromSuperview();
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        let limit = CGSize::new(max_width.map_or(f64::MAX / 4., |w| w as f64), f64::MAX / 4.);
        let size = match view {
            View::Toggle(t) => t.sizeThatFits(limit),
            View::Image(v) => v.sizeThatFits(limit),
            View::Timer(l) => l.sizeThatFits(limit),
            // A progress bar takes the width it is given.
            View::Progress(_) => CGSize::new(max_width.unwrap_or(120.) as f64, 4.),
            // Natural size of the loaded image, scaled to the available width.
            View::Picture(v) => match v.image() {
                Some(img) => {
                    let size = unsafe { img.size() };
                    let w = max_width.map_or(size.width, |m| (m as f64).min(size.width));
                    CGSize::new(
                        w,
                        if size.width > 0. {
                            w * size.height / size.width
                        } else {
                            0.
                        },
                    )
                }
                None => CGSize::ZERO,
            },
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

    fn set_frame_animated(
        &mut self,
        view: &View,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        t: prism_core::Transition,
    ) {
        let v: Retained<UIView> = view.ui().retain();
        let old = v.frame();
        self.set_frame(view, x, y, w, h);
        let new = v.frame();
        v.setFrame(old);
        animate(self.mtm, t, move || v.setFrame(new));
    }

    fn set_style_animated(&mut self, view: &View, style: &Style, t: prism_core::Transition) {
        let v: Retained<UIView> = view.ui().retain();
        let (alpha, bg) = (v.alpha(), v.backgroundColor());
        self.set_style(view, style);
        let (new_alpha, new_bg) = (v.alpha(), v.backgroundColor());
        v.setAlpha(alpha);
        v.setBackgroundColor(bg.as_deref());
        animate(self.mtm, t, move || {
            v.setAlpha(new_alpha);
            v.setBackgroundColor(new_bg.as_deref());
        });
    }

    fn set_environment(&mut self, env: &prism_core::RenderEnvironment) {
        use objc2_ui_kit::UISemanticContentAttribute as A;
        let attr = if env.direction == prism_core::Direction::Rtl {
            A::ForceRightToLeft
        } else {
            A::ForceLeftToRight
        };
        // Navigation bars, tab bars, back buttons and swipe-back follow it;
        // views created later get it in `create`.
        self.semantic = Some(attr);
        match self.controller.view().and_then(|v| v.window()) {
            Some(window) => set_semantic(&window, attr),
            None => set_semantic(&self.root, attr),
        }
    }

    fn set_value(&mut self, view: &View, value: f64) {
        match view {
            View::Stack(st) => {
                st.closable.set(value != 0.);
                self.apply_close_button(st, view.id());
            }
            View::Progress(p) => p.setProgress_animated(value as f32, true),
            View::Picture(v) => v.setContentMode(if value == 1. {
                objc2_ui_kit::UIViewContentMode::ScaleAspectFit
            } else {
                objc2_ui_kit::UIViewContentMode::ScaleAspectFill
            }),
            View::Scroll(s) => {
                if let Some(rc) = s.refreshControl() {
                    if value != 0. && !rc.isRefreshing() {
                        rc.beginRefreshing();
                    } else if value == 0. && rc.isRefreshing() {
                        rc.endRefreshing();
                    }
                }
            }
            View::Timer(l) => {
                self.timers.insert(view.id(), (l.clone(), value));
                l.setText(Some(&NSString::from_str(&prism_core::format_remaining(
                    value,
                ))));
            }
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
        if let View::Image(v) = view {
            v.setImage(UIImage::systemImageNamed(&NSString::from_str(icon)).as_deref());
            return;
        }
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

    fn set_scroll_events(&mut self, view: &View, on: bool) {
        if let View::Scroll(s) = view {
            let delegate = on.then(|| ProtocolObject::from_ref(&*self.target));
            unsafe { s.setDelegate(delegate) };
        }
    }

    fn set_swipe(&mut self, view: &View, left: bool, right: bool) {
        let v = view.ui();
        if let Some(existing) = v.gestureRecognizers() {
            for g in existing.iter() {
                if g.isKindOfClass(objc2_ui_kit::UISwipeGestureRecognizer::class()) {
                    v.removeGestureRecognizer(&g);
                }
            }
        }
        use objc2_ui_kit::UISwipeGestureRecognizerDirection as D;
        for (on, dir) in [(left, D::Left), (right, D::Right)] {
            if !on {
                continue;
            }
            let target: &AnyObject = &self.target;
            let swipe = unsafe {
                objc2_ui_kit::UISwipeGestureRecognizer::initWithTarget_action(
                    objc2_ui_kit::UISwipeGestureRecognizer::alloc(self.mtm),
                    Some(target),
                    Some(sel!(onSwipe:)),
                )
            };
            swipe.setDirection(dir);
            v.addGestureRecognizer(&swipe);
        }
    }

    fn set_menu(&mut self, view: &View, menu: Option<&prism_core::menu::MenuSpec>) {
        let id = view.id();
        let v = view.ui();
        let has = self.menus.contains_key(&id);
        match menu {
            Some(spec) => {
                self.menus.insert(id, spec.clone());
                if !has {
                    let interaction = objc2_ui_kit::UIContextMenuInteraction::initWithDelegate(
                        objc2_ui_kit::UIContextMenuInteraction::alloc(self.mtm),
                        ProtocolObject::from_ref(&*self.target),
                    );
                    v.addInteraction(ProtocolObject::from_ref(&*interaction));
                }
            }
            None => {
                self.menus.remove(&id);
                for i in v.interactions().iter() {
                    v.removeInteraction(&i);
                }
            }
        }
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
        // Tabs and sheets host a navigation stack, which handles the safe area.
        let own_insets = !matches!(screen, View::Tab(_) | View::Sheet(_));
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

// ---------------------------------------------------------------------------
// Async: wake the UI thread through GCD, and schedule the next timer.
// ---------------------------------------------------------------------------

fn wake_ui() -> prism_core::WakeUi {
    std::sync::Arc::new(|| dispatch2::DispatchQueue::main().exec_async(poll_now))
}

/// Runs ready tasks and applies what they changed.
fn poll_now() {
    with_state(|s| {
        let patches = s.runtime.poll();
        s.commit(patches);
    });
    flush_responses();
}

thread_local! {
    static NEXT_WAKE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// Makes sure the UI thread wakes up when the next timer is due.
fn schedule_timer(at: Option<std::time::Instant>) {
    let Some(at) = at else { return };
    if NEXT_WAKE.with(|w| w.get().is_some_and(|w| w <= at)) {
        return;
    }
    NEXT_WAKE.with(|w| w.set(Some(at)));
    let delay = at.saturating_duration_since(std::time::Instant::now());
    let when = dispatch2::DispatchTime::try_from(delay).unwrap_or(dispatch2::DispatchTime::NOW);
    let _ = dispatch2::DispatchQueue::main().after(when, || {
        NEXT_WAKE.with(|w| w.set(None));
        poll_now();
    });
}

/// App data: the App Group container when the app has one (widgets can read
/// it), else Application Support.
fn data_dir() -> std::path::PathBuf {
    let bundle = objc2_foundation::NSBundle::mainBundle();
    let group = bundle
        .objectForInfoDictionaryKey(&NSString::from_str("PrismAppGroup"))
        .and_then(|g| g.downcast::<NSString>().ok());
    if let Some(group) = group
        && let Some(url) = objc2_foundation::NSFileManager::defaultManager()
            .containerURLForSecurityApplicationGroupIdentifier(&group)
        && let Some(path) = url.path()
    {
        return std::path::PathBuf::from(path.to_string()).join("prism-data");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join("Library/Application Support/prism-data")
}

/// The context menu of the view hosting `interaction`, if it has one.
fn menu_configuration(
    mtm: MainThreadMarker,
    interaction: &objc2_ui_kit::UIContextMenuInteraction,
) -> Option<Retained<objc2_ui_kit::UIContextMenuConfiguration>> {
    let id = objc2_ui_kit::UIInteraction::view(interaction)?.tag() as NodeId;
    let mut spec = None;
    with_state(|s| spec = s.renderer.widgets.menus.get(&id).cloned());
    let spec = spec?;
    let provider = block2::RcBlock::new(
        move |_: std::ptr::NonNull<NSArray<objc2_ui_kit::UIMenuElement>>| {
            Retained::autorelease_return(crate::menus::build_menu(mtm, &spec, dispatch))
        },
    );
    Some(unsafe {
        objc2_ui_kit::UIContextMenuConfiguration::configurationWithIdentifier_previewProvider_actionProvider(
            None,
            std::ptr::null_mut(),
            &*provider as *const _ as *mut _,
            mtm,
        )
    })
}

/// Assets: the bundle's `assets/` folder, then the dev directory; downloads
/// are cached in Library/Caches.
fn image_sources(dev_assets: Option<&str>) -> prism_native::images::ImageSources {
    let mut asset_dirs = Vec::new();
    if let Some(res) = objc2_foundation::NSBundle::mainBundle().resourcePath() {
        asset_dirs.push(std::path::PathBuf::from(res.to_string()).join("assets"));
    }
    if let Some(dev) = dev_assets {
        asset_dirs.push(dev.into());
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    prism_native::images::ImageSources {
        asset_dirs,
        cache_dir: Some(std::path::PathBuf::from(home).join("Library/Caches/prism-images")),
    }
}

/// Locale, Dynamic Type and Reduce Motion (`PRISM_TEXT_SCALE` overrides the
/// text size, e.g. for screenshots).
fn ios_environment() -> prism_core::Environment {
    let scale = objc2_ui_kit::UIFontMetrics::defaultMetrics().scaledValueForValue(17.) / 17.;
    prism_core::Environment {
        locale: prism_native::apple::system_locale(),
        text_scale: std::env::var("PRISM_TEXT_SCALE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(scale as f32),
        reduce_motion: objc2_ui_kit::UIAccessibilityIsReduceMotionEnabled(),
        utc_offset: prism_native::apple::utc_offset(),
    }
}

/// Settings changes and keyboard moves.
fn observe_system(mtm: MainThreadMarker) {
    let target = Target::new(mtm);
    let center = objc2_foundation::NSNotificationCenter::defaultCenter();
    unsafe {
        for name in [
            objc2_ui_kit::UIContentSizeCategoryDidChangeNotification,
            objc2_ui_kit::UIAccessibilityReduceMotionStatusDidChangeNotification,
            objc2_foundation::NSCurrentLocaleDidChangeNotification,
        ] {
            center.addObserver_selector_name_object(
                &target,
                sel!(onEnvironment:),
                Some(name),
                None,
            );
        }
        for name in [
            objc2_ui_kit::UIKeyboardWillChangeFrameNotification,
            objc2_ui_kit::UIKeyboardWillHideNotification,
        ] {
            center.addObserver_selector_name_object(&target, sel!(onKeyboard:), Some(name), None);
        }
    }
    // Observers are weak: keep the target alive for the app's lifetime.
    std::mem::forget(target);
}

/// Runs `f` in a UIKit animation shaped like `t`.
fn animate(mtm: MainThreadMarker, t: prism_core::Transition, f: impl Fn() + 'static) {
    use objc2_ui_kit::UIViewAnimationOptions as O;
    let block = block2::RcBlock::new(f);
    let duration = t.duration as f64;
    let options = O::AllowUserInteraction | O::BeginFromCurrentState;
    match t.curve {
        prism_core::Curve::Spring => UIView::animateWithSpringDuration_bounce_initialSpringVelocity_delay_options_animations_completion(
            duration, 0.25, 0., 0., options, &block, None, mtm,
        ),
        prism_core::Curve::Linear => UIView::animateWithDuration_delay_options_animations_completion(
            duration, 0., options | O::CurveLinear, &block, None, mtm,
        ),
        prism_core::Curve::EaseInOut => UIView::animateWithDuration_delay_options_animations_completion(
            duration, 0., options | O::CurveEaseInOut, &block, None, mtm,
        ),
    }
}

/// Raises the bottom of the safe area of each visible screen above the
/// keyboard, animated like it. Tab bars stay put under the keyboard.
fn keyboard_changed(n: &objc2_foundation::NSNotification) {
    let hiding = &*n.name() == unsafe { objc2_ui_kit::UIKeyboardWillHideNotification };
    let info = n.userInfo();
    let get = |key: &NSString| info.as_ref().and_then(|i| i.objectForKey(key));
    let duration: f64 = get(unsafe { objc2_ui_kit::UIKeyboardAnimationDurationUserInfoKey })
        .map_or(0.25, |d| unsafe { msg_send![&*d, doubleValue] });
    let keyboard: Option<CGRect> = get(unsafe { objc2_ui_kit::UIKeyboardFrameEndUserInfoKey })
        .map(|v| unsafe { msg_send![&*v, CGRectValue] });
    // Screens in a stack, or the root controller when there is none.
    let mut controllers: Vec<Retained<UIViewController>> = Vec::new();
    with_state(|s| {
        let w = &s.renderer.widgets;
        controllers.extend(
            w.screens
                .values()
                .map(|c| Retained::into_super(c.clone()))
                .filter(|c: &Retained<UIViewController>| {
                    c.isViewLoaded() && c.view().is_some_and(|v| v.window().is_some())
                }),
        );
        if controllers.is_empty() {
            controllers.push(Retained::into_super(w.controller.clone()));
        }
    });
    let Some(mtm) = controllers.first().map(|c| c.mtm()) else {
        return;
    };
    let mut changes = Vec::new();
    let mut window = None;
    for vc in controllers {
        let Some(view) = vc.view() else { continue };
        let frame = view.convertRect_toView(view.bounds(), None);
        let overlap = match (hiding, keyboard) {
            (false, Some(kb)) => {
                (frame.origin.y + frame.size.height - kb.origin.y).clamp(0., frame.size.height)
            }
            _ => 0.,
        };
        let current = vc.additionalSafeAreaInsets().bottom;
        // Tab bar and home indicator already count in the safe area.
        let base = view.safeAreaInsets().bottom - current;
        let extra = (overlap - base).max(0.);
        if (extra - current).abs() >= 0.5 {
            changes.push((vc, extra));
        }
        window = window.or(view.window());
    }
    if !changes.is_empty() {
        let block = block2::RcBlock::new(move || {
            for (vc, extra) in &changes {
                vc.setAdditionalSafeAreaInsets(objc2_ui_kit::UIEdgeInsets {
                    top: 0.,
                    left: 0.,
                    bottom: *extra,
                    right: 0.,
                });
                if let Some(v) = vc.view() {
                    v.layoutIfNeeded();
                }
            }
        });
        UIView::animateWithDuration_delay_options_animations_completion(
            duration,
            0.,
            objc2_ui_kit::UIViewAnimationOptions::BeginFromCurrentState,
            &block,
            None,
            mtm,
        );
    }
    if !hiding && let Some(field) = window.as_deref().and_then(|w| first_responder(w)) {
        scroll_into_view(&field);
    }
}

fn first_responder(v: &UIView) -> Option<Retained<UIView>> {
    if v.isFirstResponder() {
        return Some(v.retain());
    }
    v.subviews().iter().find_map(|c| first_responder(&c))
}

/// Scrolls the nearest scroll view so `view` sits above the keyboard.
fn scroll_into_view(view: &UIView) {
    let mut parent = view.superview();
    while let Some(p) = parent {
        if let Ok(scroll) = p.clone().downcast::<UIScrollView>() {
            let r = scroll.convertRect_fromView(view.bounds(), Some(view));
            let padded = CGRect::new(
                CGPoint::new(r.origin.x, r.origin.y - 16.),
                CGSize::new(r.size.width, r.size.height + 32.),
            );
            scroll.scrollRectToVisible_animated(padded, true);
            return;
        }
        parent = p.superview();
    }
}

/// Applies a layout direction to a view and everything inside it.
fn set_semantic(v: &UIView, attr: objc2_ui_kit::UISemanticContentAttribute) {
    if v.semanticContentAttribute() != attr {
        v.setSemanticContentAttribute(attr);
        // Bars cache their layout (large titles, back buttons).
        v.setNeedsLayout();
    }
    for child in v.subviews().iter() {
        set_semantic(&child, attr);
    }
}

thread_local! {
    /// Notification responses that arrived before the app was ready (launch
    /// from a notification).
    static PENDING_RESPONSES: RefCell<Vec<(prism_core::NotificationResponse, Option<String>)>> =
        const { RefCell::new(Vec::new()) };
}

pub(crate) fn notification_response(
    response: prism_core::NotificationResponse,
    link: Option<String>,
) {
    PENDING_RESPONSES.with(|p| p.borrow_mut().push((response, link)));
    flush_responses();
}

fn flush_responses() {
    let pending = PENDING_RESPONSES.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if pending.is_empty() {
        return;
    }
    let mut handled = false;
    with_state(|s| {
        for (response, link) in pending.iter().cloned() {
            let patches = s.runtime.notification_response(response, link);
            s.commit(patches);
        }
        handled = true;
    });
    if !handled {
        PENDING_RESPONSES.with(|p| {
            p.borrow_mut().splice(0..0, pending);
        });
    }
}
