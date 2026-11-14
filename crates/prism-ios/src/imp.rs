use std::cell::RefCell;
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    NSTextAlignment, UIApplication, UIApplicationDelegate, UIButton, UIButtonType, UIColor,
    UIControlEvents, UIControlState, UIFont, UIFontWeightBold, UIFontWeightMedium,
    UIFontWeightRegular, UIFontWeightSemibold, UILabel, UIResponder, UIScene,
    UISceneConnectionOptions, UISceneDelegate, UISceneSession, UITapGestureRecognizer,
    UITextBorderStyle, UITextField, UIView, UIViewController, UIWindow, UIWindowScene,
    UIWindowSceneDelegate,
};
use prism_core::{
    AppConfig, AppInit, Backend, Color, FontWeight, NodeId, NodeKind, Runtime, Style, TextAlign,
    UiEvent,
};
use prism_native::{Insets, NativeRenderer, Widgets};

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
    STATE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => {
            if let Some(state) = guard.as_mut() {
                f(state)
            }
        }
        Err(_) => eprintln!("prism-ios: re-entrant UI callback ignored"),
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

        #[unsafe(method(onTap:))]
        fn on_tap(&self, sender: &UITapGestureRecognizer) {
            if let Some(view) = sender.view() {
                dispatch(UiEvent::Click(view.tag() as NodeId));
            }
        }
    }

    unsafe impl NSObjectProtocol for Target {}
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

    let mut runtime = Runtime::new(init);
    let mut renderer = NativeRenderer::new(UiKit {
        mtm,
        target: Target::new(mtm),
        root,
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

#[derive(Clone)]
pub enum View {
    Container(Retained<UIView>),
    Label(Retained<UILabel>),
    Button(Retained<UIButton>),
    Input(Retained<UITextField>),
}

impl View {
    fn ui(&self) -> &UIView {
        match self {
            View::Container(v) => v,
            View::Label(v) => v,
            View::Button(v) => v,
            View::Input(v) => v,
        }
    }
}

pub struct UiKit {
    mtm: MainThreadMarker,
    target: Retained<Target>,
    root: Retained<UIView>,
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
            View::Container(_) => {}
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
            View::Container(_) => {}
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
        let parent = parent.ui();
        for (i, child) in children.iter().enumerate() {
            parent.insertSubview_atIndex(child.ui(), i as isize);
        }
        let subviews = parent.subviews();
        for sub in subviews.iter().skip(children.len()) {
            sub.removeFromSuperview();
        }
    }

    fn set_root(&mut self, view: &View) {
        for sub in self.root.subviews().iter() {
            sub.removeFromSuperview();
        }
        self.root.addSubview(view.ui());
    }

    fn destroy(&mut self, view: &View) {
        view.ui().removeFromSuperview();
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        let limit = CGSize::new(max_width.map_or(f64::MAX / 4., |w| w as f64), f64::MAX / 4.);
        let size = match view {
            View::Container(_) => CGSize::ZERO,
            View::Input(f) => {
                let s = f.sizeThatFits(limit);
                CGSize::new(s.width.max(120.), s.height.max(36.))
            }
            other => other.ui().sizeThatFits(limit),
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        view.ui().setFrame(CGRect::new(
            CGPoint::new(x as f64, y as f64),
            CGSize::new(w as f64, h as f64),
        ));
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
