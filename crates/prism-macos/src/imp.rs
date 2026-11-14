use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath, NSButton,
    NSColor, NSControlTextEditingDelegate, NSEvent, NSFont, NSFontWeightBold, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSMenu, NSMenuItem, NSResponder, NSTextAlignment,
    NSTextField, NSTextFieldDelegate, NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use prism_core::{
    AppConfig, AppInit, Backend, Color, FontWeight, NodeId, NodeKind, Runtime, Style, TextAlign,
    UiEvent,
};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

// ---------------------------------------------------------------------------
// Global (main-thread) state. AppKit callbacks re-enter Rust through here.
// ---------------------------------------------------------------------------

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<AppKit>,
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
    with_state(|s| s.dispatch(event));
}

/// Pops one screen: from a specific stack's Back button, or (⌘[) from the
/// innermost stack that can go back.
fn go_back(stack: Option<NodeId>) {
    with_state(|s| {
        let stacks = &s.renderer.widgets.stacks;
        let found = match stack {
            Some(id) => stacks.iter().find(|st| st.id == id),
            None => stacks.iter().rev().find(|st| st.screens.borrow().len() > 1),
        };
        if let Some(st) = found {
            let (id, depth) = (st.id, st.screens.borrow().len());
            s.dispatch(UiEvent::Back(id, depth.saturating_sub(1)));
        }
    });
}

impl State {
    fn dispatch(&mut self, event: UiEvent) {
        let patches = self.runtime.dispatch(event);
        if !patches.is_empty() {
            self.renderer.apply(&patches);
            self.relayout();
        }
    }

    fn relayout(&mut self) {
        let size = self.renderer.widgets.root.frame().size;
        self.renderer
            .layout(size.width as f32, size.height as f32, Insets::default());
    }
}

// ---------------------------------------------------------------------------
// PrismView: a flipped NSView that paints background/border and handles clicks.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct ViewIvars {
    id: Cell<NodeId>,
    clickable: Cell<bool>,
    background: RefCell<Option<Retained<NSColor>>>,
    border_color: RefCell<Option<Retained<NSColor>>>,
    radius: Cell<f64>,
    border: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismView"]
    #[ivars = ViewIvars]
    pub struct PrismView;

    impl PrismView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let iv = self.ivars();
            let bounds = self.bounds();
            let r = iv.radius.get();
            if let Some(bg) = iv.background.borrow().as_ref() {
                bg.setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, r, r).fill();
            }
            let bw = iv.border.get();
            if bw > 0.
                && let Some(c) = iv.border_color.borrow().as_ref()
            {
                let inset = NSRect::new(
                    NSPoint::new(bounds.origin.x + bw / 2., bounds.origin.y + bw / 2.),
                    NSSize::new(bounds.size.width - bw, bounds.size.height - bw),
                );
                let ri = (r - bw / 2.).max(0.);
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inset, ri, ri);
                path.setLineWidth(bw);
                c.setStroke();
                path.stroke();
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.ivars().clickable.get() {
                unsafe { msg_send![super(self), mouseDown: event] }
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if !self.ivars().clickable.get() {
                return unsafe { msg_send![super(self), mouseUp: event] };
            }
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let b = self.bounds();
            if p.x >= 0. && p.y >= 0. && p.x <= b.size.width && p.y <= b.size.height {
                dispatch(UiEvent::Click(self.ivars().id.get()));
            }
        }
    }
);

impl PrismView {
    fn new(mtm: MainThreadMarker, id: NodeId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            id: Cell::new(id),
            ..Default::default()
        });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

// ---------------------------------------------------------------------------
// Target: receives button actions and text-field edits, routes them by tag.
// ---------------------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PrismTarget"]
    pub struct Target;

    impl Target {
        #[unsafe(method(onAction:))]
        fn on_action(&self, sender: &NSButton) {
            dispatch(UiEvent::Click(sender.tag() as NodeId));
        }

        /// A stack's header Back button (tag = stack id).
        #[unsafe(method(onBack:))]
        fn on_back(&self, sender: &NSButton) {
            go_back(Some(sender.tag() as NodeId));
        }

        /// Go ▸ Back (⌘[).
        #[unsafe(method(onBackMenu:))]
        fn on_back_menu(&self, _sender: Option<&AnyObject>) {
            go_back(None);
        }
    }

    impl Target {
        /// Dev hook: `PRISM_CLICK="+1,+1"` presses buttons by title, then
        /// `PRISM_SNAPSHOT=out.png` writes the window to a PNG and quits.
        #[unsafe(method(prismAutomation:))]
        fn automation(&self, _sender: Option<&AnyObject>) {
            run_automation(self.mtm());
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSControlTextEditingDelegate for Target {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let Some(field) = notification.object().and_then(|o| o.downcast::<NSTextField>().ok())
            else {
                return;
            };
            dispatch(UiEvent::Input(field.tag() as NodeId, field.stringValue().to_string()));
        }
    }

    unsafe impl NSTextFieldDelegate for Target {}

    unsafe impl NSWindowDelegate for Target {
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _n: &NSNotification) {
            with_state(State::relayout);
        }

        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _n: &NSNotification) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// Widgets implementation
// ---------------------------------------------------------------------------

const HEADER_HEIGHT: f64 = 44.;

/// A navigation stack: a native header bar (Back button + title) above the
/// top screen. Other screens stay alive, detached, keeping their state.
pub struct MacStack {
    id: NodeId,
    container: Retained<PrismView>,
    header: Retained<PrismView>,
    separator: Retained<PrismView>,
    back: Retained<NSButton>,
    title: Retained<NSTextField>,
    screens: RefCell<Vec<View>>,
}

#[derive(Clone)]
pub enum View {
    Container(Retained<PrismView>),
    Screen(Retained<PrismView>),
    Stack(Rc<MacStack>),
    Label(Retained<NSTextField>),
    Button(Retained<NSButton>),
    Input(Retained<NSTextField>),
}

impl View {
    fn ns(&self) -> &NSView {
        match self {
            View::Container(v) | View::Screen(v) => v,
            View::Stack(s) => &s.container,
            View::Label(v) | View::Input(v) => v,
            View::Button(v) => v,
        }
    }

    fn id(&self) -> NodeId {
        match self {
            View::Container(v) | View::Screen(v) => v.ivars().id.get(),
            View::Stack(s) => s.id,
            View::Label(v) | View::Input(v) => v.tag() as NodeId,
            View::Button(v) => v.tag() as NodeId,
        }
    }
}

pub struct AppKit {
    mtm: MainThreadMarker,
    target: Retained<Target>,
    root: Retained<PrismView>,
    stacks: Vec<Rc<MacStack>>,
    titles: HashMap<NodeId, String>,
}

impl AppKit {
    fn new_stack(&self, id: NodeId) -> MacStack {
        let mtm = self.mtm;
        let header = PrismView::new(mtm, 0);
        *header.ivars().background.borrow_mut() = Some(NSColor::windowBackgroundColor());
        let separator = PrismView::new(mtm, 0);
        *separator.ivars().background.borrow_mut() = Some(NSColor::separatorColor());
        let target: &AnyObject = &self.target;
        let back = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("‹ Back"),
                Some(target),
                Some(sel!(onBack:)),
                mtm,
            )
        };
        back.setTag(id as isize);
        let title = NSTextField::labelWithString(&NSString::new(), mtm);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(13., unsafe {
            NSFontWeightSemibold
        })));
        title.setAlignment(NSTextAlignment::Center);
        header.setSubviews(&NSArray::from_slice(&[&*title as &NSView, &back]));
        MacStack {
            id,
            container: PrismView::new(mtm, id),
            header,
            separator,
            back,
            title,
            screens: RefCell::new(Vec::new()),
        }
    }

    /// Shows the top screen, its title, and Back when there is somewhere to go.
    fn refresh_stack(&self, stack: &MacStack) {
        let screens = stack.screens.borrow();
        let mut subviews: Vec<&NSView> = vec![&stack.header, &stack.separator];
        if let Some(top) = screens.last() {
            subviews.push(top.ns());
        }
        stack.container.setSubviews(&NSArray::from_slice(&subviews));
        stack.back.setHidden(screens.len() <= 1);
        let title = screens
            .last()
            .and_then(|t| self.titles.get(&t.id()))
            .map(String::as_str)
            .unwrap_or("");
        let title = NSString::from_str(title);
        stack.title.setStringValue(&title);
        if let Some(window) = stack.container.window()
            && title.length() > 0
        {
            window.setTitle(&title);
        }
    }
}

fn ns_color(c: Color) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

fn font(style: &Style) -> Retained<NSFont> {
    let weight = unsafe {
        match style.font_weight {
            FontWeight::Regular => NSFontWeightRegular,
            FontWeight::Medium => NSFontWeightMedium,
            FontWeight::Semibold => NSFontWeightSemibold,
            FontWeight::Bold => NSFontWeightBold,
        }
    };
    NSFont::systemFontOfSize_weight(style.font_size as f64, weight)
}

fn alignment(a: TextAlign) -> NSTextAlignment {
    match a {
        TextAlign::Left => NSTextAlignment::Left,
        TextAlign::Center => NSTextAlignment::Center,
        TextAlign::Right => NSTextAlignment::Right,
    }
}

impl Widgets for AppKit {
    type View = View;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> View {
        let mtm = self.mtm;
        let empty = NSString::new();
        match kind {
            NodeKind::View => View::Container(PrismView::new(mtm, id)),
            NodeKind::Screen => View::Screen(PrismView::new(mtm, id)),
            NodeKind::Stack => {
                let stack = Rc::new(self.new_stack(id));
                self.stacks.push(stack.clone());
                View::Stack(stack)
            }
            NodeKind::Text => View::Label(NSTextField::wrappingLabelWithString(&empty, mtm)),
            NodeKind::Button => {
                let target: &AnyObject = &self.target;
                let b = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &empty,
                        Some(target),
                        Some(sel!(onAction:)),
                        mtm,
                    )
                };
                b.setTag(id as isize);
                View::Button(b)
            }
            NodeKind::TextInput => {
                let f = NSTextField::textFieldWithString(&empty, mtm);
                f.setTag(id as isize);
                // SAFETY: the delegate is weak; `self.target` outlives every view.
                unsafe { f.setDelegate(Some(ProtocolObject::from_ref(&*self.target))) };
                View::Input(f)
            }
        }
    }

    fn set_style(&mut self, view: &View, style: &Style) {
        view.ns().setAlphaValue(style.opacity as f64);
        match view {
            View::Stack(st) => {
                *st.container.ivars().background.borrow_mut() = style.background.map(ns_color);
                st.container.setNeedsDisplay(true);
            }
            View::Container(v) | View::Screen(v) => {
                let iv = v.ivars();
                *iv.background.borrow_mut() = style.background.map(ns_color);
                *iv.border_color.borrow_mut() = style.border_color.map(ns_color);
                iv.radius.set(style.corner_radius as f64);
                iv.border.set(style.border_width as f64);
                v.setNeedsDisplay(true);
            }
            View::Label(l) | View::Input(l) => {
                l.setFont(Some(&font(style)));
                l.setTextColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
                l.setAlignment(alignment(style.text_align));
            }
            View::Button(b) => {
                b.setFont(Some(&font(style)));
                b.setBezelColor(style.background.map(ns_color).as_deref());
            }
        }
    }

    fn set_text(&mut self, view: &View, text: &str) {
        let s = NSString::from_str(text);
        match view {
            View::Label(l) => l.setStringValue(&s),
            View::Input(f) => {
                // Don't reset the caret when the value already matches (controlled input echo).
                if f.stringValue().to_string() != text {
                    f.setStringValue(&s);
                }
            }
            View::Button(b) => b.setTitle(&s),
            View::Screen(_) => {
                self.titles.insert(view.id(), text.to_string());
                for stack in &self.stacks {
                    self.refresh_stack(stack);
                }
            }
            View::Container(_) | View::Stack(_) => {}
        }
    }

    fn set_placeholder(&mut self, view: &View, text: &str) {
        if let View::Input(f) = view {
            f.setPlaceholderString(Some(&NSString::from_str(text)));
        }
    }

    fn set_clickable(&mut self, view: &View, clickable: bool) {
        if let View::Container(v) = view {
            v.ivars().clickable.set(clickable);
        }
    }

    fn set_children(&mut self, parent: &View, children: &[View]) {
        if let View::Stack(stack) = parent {
            *stack.screens.borrow_mut() = children.to_vec();
            self.refresh_stack(stack);
            return;
        }
        let views: Vec<&NSView> = children.iter().map(View::ns).collect();
        parent.ns().setSubviews(&NSArray::from_slice(&views));
    }

    fn set_root(&mut self, view: &View) {
        self.root.setSubviews(&NSArray::from_slice(&[view.ns()]));
    }

    fn destroy(&mut self, view: &View) {
        match view {
            View::Input(f) => unsafe { f.setDelegate(None) },
            View::Stack(st) => self.stacks.retain(|s| s.id != st.id),
            View::Screen(_) => {
                self.titles.remove(&view.id());
            }
            _ => {}
        }
        view.ns().removeFromSuperview();
    }

    fn measure(&mut self, view: &View, max_width: Option<f32>) -> (f32, f32) {
        let size = match view {
            View::Label(l) => {
                let w = max_width.map_or(f64::MAX / 4., |w| w as f64);
                let bounds = NSRect::new(NSPoint::ZERO, NSSize::new(w, f64::MAX / 4.));
                l.cell()
                    .map_or(NSSize::ZERO, |c| c.cellSizeForBounds(bounds))
            }
            View::Input(f) => {
                let s = f.fittingSize();
                NSSize::new(s.width.max(120.), s.height)
            }
            View::Button(b) => b.fittingSize(),
            View::Container(_) | View::Screen(_) | View::Stack(_) => NSSize::ZERO,
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        let rect =
            |x: f64, y: f64, w: f64, h: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        view.ns()
            .setFrame(rect(x as f64, y as f64, w as f64, h as f64));
        if let View::Stack(st) = view {
            let w = w as f64;
            st.header.setFrame(rect(0., 0., w, HEADER_HEIGHT - 1.));
            st.separator.setFrame(rect(0., HEADER_HEIGHT - 1., w, 1.));
            let b = st.back.fittingSize();
            st.back.setFrame(rect(
                8.,
                ((HEADER_HEIGHT - 1. - b.height) / 2.).round(),
                b.width,
                b.height,
            ));
            let t = st.title.fittingSize();
            st.title.setFrame(rect(
                0.,
                ((HEADER_HEIGHT - 1. - t.height) / 2.).round(),
                w,
                t.height,
            ));
        }
    }

    fn screen_viewport(&mut self, stack: &View, _screen: &View) -> Viewport {
        let size = stack.ns().frame().size;
        Viewport {
            x: 0.,
            y: HEADER_HEIGHT as f32,
            width: size.width as f32,
            height: (size.height - HEADER_HEIGHT).max(0.) as f32,
            insets: Insets::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: objc2::runtime::Sel,
    key: &str,
) -> Retained<NSMenuItem> {
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    }
}

fn install_menu(app: &NSApplication, mtm: MainThreadMarker, title: &str, target: &Target) {
    let bar = NSMenu::new(mtm);
    let submenu = |name: &str, items: Vec<Retained<NSMenuItem>>| {
        let item = NSMenuItem::new(mtm);
        let menu = NSMenu::new(mtm);
        menu.setTitle(&NSString::from_str(name));
        for i in items {
            menu.addItem(&i);
        }
        item.setSubmenu(Some(&menu));
        bar.addItem(&item);
    };
    submenu(
        title,
        vec![menu_item(
            mtm,
            &format!("Quit {title}"),
            sel!(terminate:),
            "q",
        )],
    );
    submenu(
        "Edit",
        vec![
            menu_item(mtm, "Cut", sel!(cut:), "x"),
            menu_item(mtm, "Copy", sel!(copy:), "c"),
            menu_item(mtm, "Paste", sel!(paste:), "v"),
            menu_item(mtm, "Select All", sel!(selectAll:), "a"),
        ],
    );
    let back = menu_item(mtm, "Back", sel!(onBackMenu:), "[");
    unsafe { back.setTarget(Some(target)) };
    submenu("Go", vec![back]);
    app.setMainMenu(Some(&bar));
}

/// Opens a window and runs the AppKit event loop. Must be called on the main thread.
pub fn run(config: AppConfig, init: AppInit) {
    let mtm =
        MainThreadMarker::new().expect("prism-macos: run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let target = Target::new(mtm);
    install_menu(&app, mtm, &config.title, &target);
    let root = PrismView::new(mtm, 0);
    let frame = NSRect::new(
        NSPoint::ZERO,
        NSSize::new(config.width as f64, config.height as f64),
    );
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(&config.title));
    window.setContentView(Some(&root));
    window.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    window.center();

    let path = std::env::var("PRISM_PATH").unwrap_or_else(|_| "/".into());
    let mut runtime = Runtime::with_path(&init, &path);
    let mut renderer = NativeRenderer::new(AppKit {
        mtm,
        target: target.clone(),
        root,
        stacks: Vec::new(),
        titles: HashMap::new(),
    });
    renderer.apply(&runtime.render());
    let mut state = State { runtime, renderer };
    state.relayout();
    STATE.with(|cell| *cell.borrow_mut() = Some(state));

    window.makeKeyAndOrderFront(None);
    app.activate();
    if std::env::var_os("PRISM_SNAPSHOT").is_some() || std::env::var_os("PRISM_CLICK").is_some() {
        let none: Option<&AnyObject> = None;
        let _: () = unsafe {
            msg_send![&*target, performSelector: sel!(prismAutomation:), withObject: none, afterDelay: 0.5f64]
        };
    }
    app.run();
    drop(window);
}

// ---------------------------------------------------------------------------
// Dev automation (env-driven, used by `mise run macos-snapshot`)
// ---------------------------------------------------------------------------

fn find_buttons(view: &NSView, out: &mut Vec<Retained<NSButton>>) {
    for sub in view.subviews().iter() {
        if let Ok(b) = sub.clone().downcast::<NSButton>() {
            out.push(b);
        }
        find_buttons(&sub, out);
    }
}

fn run_automation(mtm: MainThreadMarker) {
    let mut root = None;
    with_state(|s| root = Some(s.renderer.widgets.root.clone()));
    let Some(root) = root else { return };

    if let Ok(clicks) = std::env::var("PRISM_CLICK") {
        for title in clicks.split(',').filter(|t| !t.is_empty()) {
            let mut buttons = Vec::new();
            find_buttons(&root, &mut buttons);
            match buttons.iter().find(|b| b.title().to_string() == title) {
                Some(b) => unsafe { b.performClick(None) },
                None => eprintln!("prism-macos: PRISM_CLICK: no button titled {title:?}"),
            }
        }
    }

    if let Ok(path) = std::env::var("PRISM_SNAPSHOT") {
        root.displayIfNeeded();
        let bounds = root.bounds();
        if let Some(rep) = root.bitmapImageRepForCachingDisplayInRect(bounds) {
            root.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
            let props = objc2_foundation::NSDictionary::new();
            let png = unsafe {
                rep.representationUsingType_properties(
                    objc2_app_kit::NSBitmapImageFileType::PNG,
                    &props,
                )
            };
            match png {
                Some(data) => match std::fs::write(&path, data.to_vec()) {
                    Ok(()) => println!("prism-macos: snapshot written to {path}"),
                    Err(e) => eprintln!("prism-macos: cannot write {path}: {e}"),
                },
                None => eprintln!("prism-macos: PNG encoding failed"),
            }
        }
        NSApplication::sharedApplication(mtm).terminate(None);
    }
}
