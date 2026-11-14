use std::cell::{Cell, RefCell};

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
use prism_native::{Insets, NativeRenderer, Widgets};

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
    STATE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut guard) => {
            if let Some(state) = guard.as_mut() {
                f(state)
            }
        }
        Err(_) => eprintln!("prism-macos: re-entrant UI callback ignored"),
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

#[derive(Clone)]
pub enum View {
    Container(Retained<PrismView>),
    Label(Retained<NSTextField>),
    Button(Retained<NSButton>),
    Input(Retained<NSTextField>),
}

impl View {
    fn ns(&self) -> &NSView {
        match self {
            View::Container(v) => v,
            View::Label(v) | View::Input(v) => v,
            View::Button(v) => v,
        }
    }
}

pub struct AppKit {
    mtm: MainThreadMarker,
    target: Retained<Target>,
    root: Retained<PrismView>,
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
            View::Container(v) => {
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
            View::Container(_) => {}
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
        let views: Vec<&NSView> = children.iter().map(View::ns).collect();
        parent.ns().setSubviews(&NSArray::from_slice(&views));
    }

    fn set_root(&mut self, view: &View) {
        self.root.setSubviews(&NSArray::from_slice(&[view.ns()]));
    }

    fn destroy(&mut self, view: &View) {
        if let View::Input(f) = view {
            unsafe { f.setDelegate(None) };
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
            View::Container(_) => NSSize::ZERO,
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        view.ns().setFrame(NSRect::new(
            NSPoint::new(x as f64, y as f64),
            NSSize::new(w as f64, h as f64),
        ));
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

fn install_menu(app: &NSApplication, mtm: MainThreadMarker, title: &str) {
    let bar = NSMenu::new(mtm);
    let submenu = |items: Vec<Retained<NSMenuItem>>| {
        let item = NSMenuItem::new(mtm);
        let menu = NSMenu::new(mtm);
        for i in items {
            menu.addItem(&i);
        }
        item.setSubmenu(Some(&menu));
        bar.addItem(&item);
    };
    submenu(vec![menu_item(
        mtm,
        &format!("Quit {title}"),
        sel!(terminate:),
        "q",
    )]);
    submenu(vec![
        menu_item(mtm, "Cut", sel!(cut:), "x"),
        menu_item(mtm, "Copy", sel!(copy:), "c"),
        menu_item(mtm, "Paste", sel!(paste:), "v"),
        menu_item(mtm, "Select All", sel!(selectAll:), "a"),
    ]);
    app.setMainMenu(Some(&bar));
}

/// Opens a window and runs the AppKit event loop. Must be called on the main thread.
pub fn run(config: AppConfig, init: AppInit) {
    let mtm =
        MainThreadMarker::new().expect("prism-macos: run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    install_menu(&app, mtm, &config.title);

    let target = Target::new(mtm);
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

    let mut runtime = Runtime::new(&init);
    let mut renderer = NativeRenderer::new(AppKit {
        mtm,
        target: target.clone(),
        root,
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
