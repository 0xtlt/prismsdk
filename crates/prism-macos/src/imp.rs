use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSAccessibilityCheckBoxRole,
    NSAccessibilityGroupRole, NSAccessibilityHeadingRole, NSAccessibilityImageRole,
    NSAccessibilityLinkRole, NSAccessibilityListRole, NSAccessibilityRadioButtonRole,
    NSAccessibilityStaticTextRole, NSAccessibilityTabGroupRole, NSAnimatablePropertyContainer,
    NSAppearance, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath,
    NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate,
    NSEvent, NSFont, NSFontWeightBold, NSFontWeightMedium, NSFontWeightRegular,
    NSFontWeightSemibold, NSImage, NSImageSymbolConfiguration, NSImageView, NSMenu, NSMenuItem,
    NSProgressIndicator, NSProgressIndicatorStyle, NSResponder, NSScrollView, NSSegmentedControl,
    NSSwitch, NSTextAlignment, NSTextField, NSTextFieldDelegate, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use prism_core::{
    Accessibility, AppConfig, AppInit, Appearance, Backend, Color, FontWeight, NodeId, NodeKind,
    Role, Runtime, Style, TextAlign, UiEvent,
};
use prism_native::{Insets, NativeRenderer, Viewport, Widgets};

// ---------------------------------------------------------------------------
// Global (main-thread) state. AppKit callbacks re-enter Rust through here.
// ---------------------------------------------------------------------------

struct State {
    runtime: Runtime,
    renderer: NativeRenderer<AppKit>,
    services: crate::services::MacServices,
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
    with_state(|s| s.dispatch(event));
}

/// Opens a deep link (notification or menu bar activity).
pub(crate) fn open_link(path: &str) {
    with_state(|s| {
        let patches = s.runtime.open_link(path);
        s.commit(patches);
    });
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
        self.commit(patches);
    }

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
    /// Context menu (right-click, ⌃-click, VoiceOver "show menu").
    menu: RefCell<Option<prism_core::menu::MenuSpec>>,
    /// Root of a sheet: Escape dismisses it.
    sheet_root: Cell<bool>,
    /// Picture content, drawn aspect-fill (or aspect-fit).
    image: RefCell<Option<Retained<NSImage>>>,
    fit: Cell<bool>,
    /// Trackpad swipes (left, right) reported to Prism.
    swipe: Cell<(bool, bool)>,
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
            if let Some(img) = iv.image.borrow().as_ref() {
                draw_picture(img, bounds, r, iv.fit.get());
            }
        }

        /// Three-finger (or two-finger, per System Settings) swipes.
        #[unsafe(method(swipeWithEvent:))]
        fn swipe_with_event(&self, event: &NSEvent) {
            let (left, right) = self.ivars().swipe.get();
            let dx = event.deltaX();
            let dir = if dx > 0. && left {
                Some(prism_core::SwipeDirection::Left)
            } else if dx < 0. && right {
                Some(prism_core::SwipeDirection::Right)
            } else {
                None
            };
            match dir {
                Some(dir) => dispatch(UiEvent::Swipe(self.ivars().id.get(), dir)),
                None => unsafe { msg_send![super(self), swipeWithEvent: event] },
            }
        }

        /// VoiceOver "press" on a clickable view.
        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_perform_press(&self) -> objc2::runtime::Bool {
            if !self.ivars().clickable.get() {
                return objc2::runtime::Bool::NO;
            }
            dispatch(UiEvent::Click(self.ivars().id.get()));
            objc2::runtime::Bool::YES
        }

        // Full keyboard access: clickable views take focus, show a focus
        // ring, and activate with Space or Return.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            self.ivars().clickable.get() || self.ivars().sheet_root.get()
        }

        /// Right-click / ⌃-click.
        #[unsafe(method(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> *mut NSMenu {
            match self.context_menu() {
                Some(menu) => Retained::autorelease_return(menu),
                None => std::ptr::null_mut(),
            }
        }

        /// VoiceOver / Full Keyboard Access "show menu".
        #[unsafe(method(accessibilityPerformShowMenu))]
        fn accessibility_perform_show_menu(&self) -> objc2::runtime::Bool {
            let Some(menu) = self.context_menu() else { return objc2::runtime::Bool::NO };
            let at = NSPoint::new(8., self.bounds().size.height.min(24.));
            menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(self));
            objc2::runtime::Bool::YES
        }

        /// Escape in a sheet.
        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, sender: Option<&AnyObject>) {
            if self.ivars().sheet_root.get() {
                dispatch(UiEvent::Back(self.ivars().id.get(), 0));
            } else {
                unsafe { msg_send![super(self), cancelOperation: sender] }
            }
        }

        /// Like native buttons: reachable with Tab only when the user enabled
        /// Keyboard Navigation (System Settings › Keyboard).
        #[unsafe(method(canBecomeKeyView))]
        fn can_become_key_view(&self) -> bool {
            self.ivars().clickable.get()
                && NSApplication::sharedApplication(self.mtm()).isFullKeyboardAccessEnabled()
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let code = event.keyCode();
            if self.ivars().clickable.get() && (code == 49 || code == 36 || code == 76) {
                dispatch(UiEvent::Click(self.ivars().id.get()));
            } else {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_mask_bounds(&self) -> NSRect {
            self.bounds()
        }

        #[unsafe(method(drawFocusRingMask))]
        fn draw_focus_ring_mask(&self) {
            let r = self.ivars().radius.get();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), r, r).fill();
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

/// Draws `img` in `bounds`, cropped (fill) or letterboxed (fit), clipped to the corner radius.
fn draw_picture(img: &NSImage, bounds: NSRect, radius: f64, fit: bool) {
    let size = img.size();
    if size.width <= 0. || size.height <= 0. {
        return;
    }
    let (bw, bh) = (bounds.size.width, bounds.size.height);
    let scale = if fit {
        (bw / size.width).min(bh / size.height)
    } else {
        (bw / size.width).max(bh / size.height)
    };
    let (w, h) = (size.width * scale, size.height * scale);
    let rect = NSRect::new(
        NSPoint::new((bw - w) / 2., (bh - h) / 2.),
        NSSize::new(w, h),
    );
    objc2_app_kit::NSGraphicsContext::saveGraphicsState_class();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, radius, radius).addClip();
    unsafe {
        img.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
            rect,
            NSRect::ZERO,
            objc2_app_kit::NSCompositingOperation::SourceOver,
            1.,
            true,
            None,
        )
    };
    objc2_app_kit::NSGraphicsContext::restoreGraphicsState_class();
}

impl PrismView {
    fn context_menu(&self) -> Option<Retained<NSMenu>> {
        let spec = self.ivars().menu.borrow().clone()?;
        let mut target = None;
        with_state(|s| target = Some(s.renderer.widgets.target.clone()));
        let target: Retained<AnyObject> = Retained::into_super(Retained::into_super(target?));
        Some(crate::menus::build_menu(self.mtm(), &spec, &target))
    }

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

        #[unsafe(method(onToggle:))]
        fn on_toggle(&self, sender: &NSSwitch) {
            let on = sender.state() == NSControlStateValueOn;
            dispatch(UiEvent::Change(sender.tag() as NodeId, if on { 1. } else { 0. }));
        }

        #[unsafe(method(onSegment:))]
        fn on_segment(&self, sender: &NSSegmentedControl) {
            let index = sender.selectedSegment().max(0) as f64;
            dispatch(UiEvent::Change(sender.tag() as NodeId, index));
        }

        /// Any Prism menu item (context menus, tray, app menus).
        #[unsafe(method(onMenuItem:))]
        fn on_menu_item(&self, sender: &NSMenuItem) {
            dispatch(UiEvent::MenuSelect(sender.tag() as u64));
        }

        /// Every second: countdowns in the window and the menu bar.
        #[unsafe(method(onTick:))]
        fn on_tick(&self, _timer: &AnyObject) {
            with_state(|s| {
                s.renderer.widgets.tick();
                s.services.tick();
            });
        }

        /// "Open" in a menu bar activity (tag = activity id).
        #[unsafe(method(onStatusItem:))]
        fn on_status_item(&self, sender: &NSMenuItem) {
            let mut link = None;
            with_state(|s| link = s.services.link_for(sender.tag()));
            if let Some(link) = link {
                NSApplication::sharedApplication(self.mtm()).activate();
                open_link(&link);
            }
        }

        /// Reduce Motion or the language changed in System Settings.
        #[unsafe(method(onEnvironment:))]
        fn on_environment(&self, _n: &NSNotification) {
            with_state(|s| {
                s.runtime.app.set_environment(mac_environment());
                let patches = s.runtime.render();
                s.commit(patches);
            });
        }

        /// A scroll view moved (virtualized lists follow it).
        #[unsafe(method(onScrollBounds:))]
        fn on_scroll_bounds(&self, n: &NSNotification) {
            let Some(clip) = n.object().and_then(|o| o.downcast::<objc2_app_kit::NSClipView>().ok())
            else {
                return;
            };
            let Some(doc) = clip.documentView().and_then(|d| d.downcast::<PrismView>().ok()) else {
                return;
            };
            let b = clip.bounds();
            dispatch(UiEvent::Scroll(doc.ivars().id.get(), b.origin.y as f32, b.size.height as f32));
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
        /// Dev hook: `PRISM_CLICK="+1|Save"` presses controls by title or accessibility label, then
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
    /// Shows "Close" on the first screen (stacks inside sheets).
    closable: Cell<bool>,
}

/// A sheet window presenting a modal stack over the main window.
pub struct MacSheet {
    window: Retained<NSWindow>,
    content: Retained<PrismView>,
    presented: Cell<bool>,
}

/// A tab bar: a segmented control above the selected tab.
pub struct MacTabs {
    id: NodeId,
    container: Retained<PrismView>,
    bar: Retained<PrismView>,
    separator: Retained<PrismView>,
    segments: Retained<NSSegmentedControl>,
    tabs: RefCell<Vec<View>>,
    selected: Cell<usize>,
}

/// A scroll view; Prism children live in its flipped document view.
pub struct MacScroll {
    scroll: Retained<NSScrollView>,
    document: Retained<PrismView>,
}

#[derive(Clone)]
pub enum View {
    Container(Retained<PrismView>),
    Screen(Retained<PrismView>),
    Tab(Retained<PrismView>),
    Stack(Rc<MacStack>),
    Tabs(Rc<MacTabs>),
    Scroll(Rc<MacScroll>),
    Sheet(Rc<MacSheet>),
    Toggle(Retained<NSSwitch>),
    Image(Retained<NSImageView>),
    Progress(Retained<NSProgressIndicator>),
    Timer(Retained<NSTextField>),
    Picture(Retained<PrismView>),
    Label(Retained<NSTextField>),
    Button(Retained<NSButton>),
    Input(Retained<NSTextField>),
}

impl View {
    fn ns(&self) -> &NSView {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) => v,
            View::Stack(s) => &s.container,
            View::Tabs(t) => &t.container,
            View::Scroll(s) => &s.scroll,
            View::Sheet(s) => &s.content,
            View::Toggle(t) => t,
            View::Image(v) => v,
            View::Progress(v) => v,
            View::Timer(v) => v,
            View::Picture(v) => v,
            View::Label(v) | View::Input(v) => v,
            View::Button(v) => v,
        }
    }

    fn id(&self) -> NodeId {
        match self {
            View::Container(v) | View::Screen(v) | View::Tab(v) => v.ivars().id.get(),
            View::Stack(s) => s.id,
            View::Tabs(t) => t.id,
            View::Scroll(s) => s.document.ivars().id.get(),
            View::Sheet(s) => s.content.ivars().id.get(),
            View::Toggle(t) => t.tag() as NodeId,
            View::Image(v) => v.tag() as NodeId,
            // Progress bars need no id (no events, no per-node state).
            View::Progress(_) => 0,
            View::Timer(v) => v.tag() as NodeId,
            View::Picture(v) => v.ivars().id.get(),
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
    tab_bars: Vec<Rc<MacTabs>>,
    titles: HashMap<NodeId, String>,
    icons: HashMap<NodeId, String>,
    /// Countdown labels and their end time (Unix seconds).
    timers: HashMap<NodeId, (Retained<NSTextField>, f64)>,
    /// Presented sheets.
    sheets: Vec<Rc<MacSheet>>,
    /// Pictures waiting for their image, by source.
    pictures: HashMap<String, Vec<(NodeId, Retained<PrismView>)>>,
    /// Decoded images by source.
    image_cache: HashMap<String, Retained<NSImage>>,
    image_sources: prism_native::images::ImageSources,
    /// Right-to-left language: header buttons swap sides.
    rtl: bool,
    /// Header button texts: (back, close), in the app's language.
    chrome: (String, String),
}

impl AppKit {
    /// Shows `src` in `view`: from memory, or loaded on a background thread.
    fn load_picture(&mut self, id: NodeId, view: &Retained<PrismView>, src: &str) {
        if let Some(img) = self.image_cache.get(src) {
            *view.ivars().image.borrow_mut() = Some(img.clone());
            view.setNeedsDisplay(true);
            return;
        }
        *view.ivars().image.borrow_mut() = None;
        view.setNeedsDisplay(true);
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
        let data = bytes.map(objc2_foundation::NSData::from_vec);
        let Some(img) =
            data.and_then(|d| NSImage::initWithData(objc2::AllocAnyThread::alloc(), &d))
        else {
            eprintln!("prism-macos: could not load image {src:?}");
            return;
        };
        for (_, view) in waiting {
            *view.ivars().image.borrow_mut() = Some(img.clone());
            view.setNeedsDisplay(true);
        }
        self.image_cache.insert(src.to_string(), img);
    }

    fn tick(&self) {
        for (label, end) in self.timers.values() {
            let text = prism_core::format_remaining(*end);
            if label.stringValue().to_string() != text {
                label.setStringValue(&NSString::from_str(&text));
            }
        }
    }
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
            closable: Cell::new(false),
        }
    }

    fn new_tabs(&self, id: NodeId) -> MacTabs {
        let mtm = self.mtm;
        let bar = PrismView::new(mtm, 0);
        *bar.ivars().background.borrow_mut() = Some(NSColor::windowBackgroundColor());
        let separator = PrismView::new(mtm, 0);
        *separator.ivars().background.borrow_mut() = Some(NSColor::separatorColor());
        let segments = NSSegmentedControl::new(mtm);
        segments.setTag(id as isize);
        let target: &AnyObject = &self.target;
        unsafe {
            segments.setTarget(Some(target));
            segments.setAction(Some(sel!(onSegment:)));
        }
        bar.setSubviews(&NSArray::from_slice(&[&*segments as &NSView]));
        MacTabs {
            id,
            container: PrismView::new(mtm, id),
            bar,
            separator,
            segments,
            tabs: RefCell::new(Vec::new()),
            selected: Cell::new(0),
        }
    }

    /// Syncs segments (titles, SF Symbols) and shows the selected tab.
    fn refresh_tabs(&self, bar: &MacTabs) {
        let tabs = bar.tabs.borrow();
        bar.segments.setSegmentCount(tabs.len() as isize);
        for (i, t) in tabs.iter().enumerate() {
            let title = self.titles.get(&t.id()).map(String::as_str).unwrap_or("");
            bar.segments
                .setLabel_forSegment(&NSString::from_str(title), i as isize);
            let image = self.icons.get(&t.id()).and_then(|icon| {
                NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(icon),
                    None,
                )
            });
            bar.segments
                .setImage_forSegment(image.as_deref(), i as isize);
        }
        let selected = bar.selected.get().min(tabs.len().saturating_sub(1));
        bar.segments.setSelectedSegment(selected as isize);
        let mut subviews: Vec<&NSView> = vec![&bar.bar, &bar.separator];
        if let Some(t) = tabs.get(selected) {
            subviews.push(t.ns());
        }
        bar.container.setSubviews(&NSArray::from_slice(&subviews));
    }

    fn present_sheets(&mut self, sheets: &[View]) {
        for s in sheets {
            if let View::Sheet(sheet) = s
                && !sheet.presented.get()
                && let Some(main) = self.root.window()
            {
                sheet.presented.set(true);
                main.beginSheet_completionHandler(&sheet.window, None);
                sheet.window.makeFirstResponder(Some(&sheet.content));
                self.sheets.push(sheet.clone());
            }
        }
        // Sheets no longer listed are dismissed.
        let keep: Vec<NodeId> = sheets.iter().map(View::id).collect();
        let gone: Vec<Rc<MacSheet>> = self
            .sheets
            .iter()
            .filter(|s| !keep.contains(&s.content.ivars().id.get()))
            .cloned()
            .collect();
        for sheet in gone {
            self.dismiss_sheet(&sheet);
        }
    }

    fn dismiss_sheet(&mut self, sheet: &MacSheet) {
        if sheet.presented.replace(false)
            && let Some(main) = self.root.window()
        {
            main.endSheet(&sheet.window);
            sheet.window.orderOut(None);
        }
        let id = sheet.content.ivars().id.get();
        self.sheets.retain(|s| s.content.ivars().id.get() != id);
    }

    /// Shows the top screen, its title, and Back when there is somewhere to go.
    fn refresh_stack(&self, stack: &MacStack) {
        let screens = stack.screens.borrow();
        let mut subviews: Vec<&NSView> = vec![&stack.header, &stack.separator];
        if let Some(top) = screens.last() {
            subviews.push(top.ns());
        }
        stack.container.setSubviews(&NSArray::from_slice(&subviews));
        let close = stack.closable.get() && screens.len() <= 1;
        stack.back.setHidden(screens.len() <= 1 && !close);
        let label = if close {
            self.chrome.1.clone()
        } else if self.rtl {
            format!("{} ›", self.chrome.0)
        } else {
            format!("‹ {}", self.chrome.0)
        };
        stack.back.setTitle(&NSString::from_str(&label));
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

fn plain_color(c: Color) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
}

/// Adaptive colors become dynamic `NSColor`s, resolved per appearance.
fn ns_color(c: Color) -> Retained<NSColor> {
    if c.dark.is_none() {
        return plain_color(c);
    }
    let (light, dark) = (c.light_variant(), c.dark_variant());
    let provider = block2::RcBlock::new(move |appearance: std::ptr::NonNull<NSAppearance>| {
        let appearance = unsafe { appearance.as_ref() };
        let names = unsafe {
            NSArray::from_slice(&[
                objc2_app_kit::NSAppearanceNameAqua,
                objc2_app_kit::NSAppearanceNameDarkAqua,
            ])
        };
        let is_dark = appearance
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|n| &*n == unsafe { objc2_app_kit::NSAppearanceNameDarkAqua });
        let color = plain_color(if is_dark { dark } else { light });
        std::ptr::NonNull::new(Retained::autorelease_return(color)).expect("color")
    });
    unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
}

fn font_weight(style: &Style) -> objc2_app_kit::NSFontWeight {
    unsafe {
        match style.font_weight {
            FontWeight::Regular => NSFontWeightRegular,
            FontWeight::Medium => NSFontWeightMedium,
            FontWeight::Semibold => NSFontWeightSemibold,
            FontWeight::Bold => NSFontWeightBold,
        }
    }
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
            NodeKind::Tab => View::Tab(PrismView::new(mtm, id)),
            NodeKind::Tabs => {
                let tabs = Rc::new(self.new_tabs(id));
                self.tab_bars.push(tabs.clone());
                View::Tabs(tabs)
            }
            NodeKind::Modal => {
                let content = PrismView::new(mtm, id);
                content.ivars().sheet_root.set(true);
                let host = PrismView::new(mtm, 0);
                let window = unsafe {
                    NSWindow::initWithContentRect_styleMask_backing_defer(
                        NSWindow::alloc(mtm),
                        NSRect::new(NSPoint::ZERO, NSSize::new(520., 620.)),
                        NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable,
                        NSBackingStoreType::Buffered,
                        false,
                    )
                };
                unsafe { window.setReleasedWhenClosed(false) };
                window.setContentView(Some(&host));
                host.setSubviews(&NSArray::from_slice(&[&*content as &NSView]));
                View::Sheet(Rc::new(MacSheet {
                    window,
                    content,
                    presented: Cell::new(false),
                }))
            }
            NodeKind::Scroll => {
                let scroll = NSScrollView::new(mtm);
                scroll.setHasVerticalScroller(true);
                scroll.setDrawsBackground(false);
                let document = PrismView::new(mtm, id);
                scroll.setDocumentView(Some(&document));
                View::Scroll(Rc::new(MacScroll { scroll, document }))
            }
            NodeKind::Image => {
                let v = NSImageView::new(mtm);
                v.setTag(id as isize);
                View::Image(v)
            }
            NodeKind::Progress => {
                let p = NSProgressIndicator::new(mtm);
                p.setStyle(NSProgressIndicatorStyle::Bar);
                p.setIndeterminate(false);
                p.setMaxValue(1.);
                View::Progress(p)
            }
            NodeKind::Picture => View::Picture(PrismView::new(mtm, id)),
            NodeKind::Timer => {
                let l = NSTextField::labelWithString(&empty, mtm);
                l.setTag(id as isize);
                View::Timer(l)
            }
            NodeKind::Toggle => {
                let t = NSSwitch::new(mtm);
                t.setTag(id as isize);
                let target: &AnyObject = &self.target;
                unsafe {
                    t.setTarget(Some(target));
                    t.setAction(Some(sel!(onToggle:)));
                }
                View::Toggle(t)
            }
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
            View::Sheet(sheet) => {
                *sheet.content.ivars().background.borrow_mut() = style.background.map(ns_color);
                sheet.content.setNeedsDisplay(true);
            }
            View::Stack(st) => {
                *st.container.ivars().background.borrow_mut() = style.background.map(ns_color);
                st.container.setNeedsDisplay(true);
            }
            View::Tabs(t) => {
                *t.container.ivars().background.borrow_mut() = style.background.map(ns_color);
                t.container.setNeedsDisplay(true);
            }
            View::Scroll(sc) => {
                *sc.document.ivars().background.borrow_mut() = style.background.map(ns_color);
                sc.document.setNeedsDisplay(true);
            }
            View::Toggle(_) | View::Progress(_) => {}
            View::Image(v) => {
                let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    style.font_size as f64,
                    font_weight(style),
                );
                v.setSymbolConfiguration(Some(&config));
                v.setContentTintColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
            }
            View::Timer(l) => {
                l.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
                    style.font_size as f64,
                    font_weight(style),
                )));
                l.setTextColor(Some(
                    &style.text_color.map_or_else(NSColor::labelColor, ns_color),
                ));
                l.setAlignment(alignment(style.text_align));
            }
            View::Container(v) | View::Screen(v) | View::Tab(v) | View::Picture(v) => {
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
            View::Label(l) | View::Timer(l) => l.setStringValue(&s),
            View::Picture(v) => self.load_picture(view.id(), v, text),
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
            View::Tab(_) => {
                self.titles.insert(view.id(), text.to_string());
                for bar in &self.tab_bars {
                    self.refresh_tabs(bar);
                }
            }
            _ => {}
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
        // Sheets attached to a root container are presented, not laid out inline.
        let (sheets, children): (Vec<View>, Vec<View>) = children
            .iter()
            .cloned()
            .partition(|c| matches!(c, View::Sheet(_)));
        if matches!(parent, View::Stack(_) | View::Tabs(_)) {
            self.present_sheets(&sheets);
        }
        let children = &children[..];
        match parent {
            View::Stack(stack) => {
                *stack.screens.borrow_mut() = children.to_vec();
                self.refresh_stack(stack);
                return;
            }
            View::Tabs(bar) => {
                *bar.tabs.borrow_mut() = children.to_vec();
                self.refresh_tabs(bar);
                return;
            }
            View::Scroll(sc) => {
                let views: Vec<&NSView> = children.iter().map(View::ns).collect();
                sc.document.setSubviews(&NSArray::from_slice(&views));
                return;
            }
            _ => {}
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
            View::Tabs(t) => self.tab_bars.retain(|b| b.id != t.id),
            View::Sheet(sheet) => self.dismiss_sheet(sheet),
            View::Timer(_) => {
                self.timers.remove(&view.id());
            }
            View::Picture(_) => {
                let id = view.id();
                self.pictures.retain(|_, ids| {
                    ids.retain(|(i, _)| *i != id);
                    !ids.is_empty()
                });
            }
            View::Tab(_) => {
                self.titles.remove(&view.id());
                self.icons.remove(&view.id());
            }
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
            View::Toggle(t) => t.fittingSize(),
            View::Sheet(_) => NSSize::ZERO,
            View::Image(v) => v.fittingSize(),
            View::Timer(l) => l.fittingSize(),
            // Natural size of the loaded image, scaled to the available width.
            View::Picture(v) => match v.ivars().image.borrow().as_ref() {
                Some(img) => {
                    let size = img.size();
                    let w = max_width.map_or(size.width, |m| (m as f64).min(size.width));
                    let h = if size.width > 0. {
                        w * size.height / size.width
                    } else {
                        0.
                    };
                    NSSize::new(w, h)
                }
                None => NSSize::ZERO,
            },
            View::Progress(_) => NSSize::new(max_width.unwrap_or(120.) as f64, 6.),
            View::Container(_)
            | View::Screen(_)
            | View::Tab(_)
            | View::Stack(_)
            | View::Tabs(_)
            | View::Scroll(_) => NSSize::ZERO,
        };
        (size.width.ceil() as f32, size.height.ceil() as f32)
    }

    fn set_frame(&mut self, view: &View, x: f32, y: f32, w: f32, h: f32) {
        let rect =
            |x: f64, y: f64, w: f64, h: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        view.ns()
            .setFrame(rect(x as f64, y as f64, w as f64, h as f64));
        if let View::Tabs(t) = view {
            let w = w as f64;
            t.bar.setFrame(rect(0., 0., w, HEADER_HEIGHT - 1.));
            t.separator.setFrame(rect(0., HEADER_HEIGHT - 1., w, 1.));
            let sz = t.segments.fittingSize();
            t.segments.setFrame(rect(
                ((w - sz.width) / 2.).round(),
                ((HEADER_HEIGHT - 1. - sz.height) / 2.).round(),
                sz.width,
                sz.height,
            ));
        }
        if let View::Stack(st) = view {
            let w = w as f64;
            st.header.setFrame(rect(0., 0., w, HEADER_HEIGHT - 1.));
            st.separator.setFrame(rect(0., HEADER_HEIGHT - 1., w, 1.));
            let b = st.back.fittingSize();
            st.back.setFrame(rect(
                if self.rtl { w - 8. - b.width } else { 8. },
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

    fn set_value(&mut self, view: &View, value: f64) {
        match view {
            View::Stack(st) => {
                st.closable.set(value != 0.);
                self.refresh_stack(st);
            }
            View::Progress(p) => p.setDoubleValue(value),
            View::Picture(v) => {
                v.ivars().fit.set(value == 1.);
                v.setNeedsDisplay(true);
            }
            View::Timer(l) => {
                self.timers.insert(view.id(), (l.clone(), value));
                l.setStringValue(&NSString::from_str(&prism_core::format_remaining(value)));
            }
            View::Toggle(t) => t.setState(if value != 0. {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            }),
            View::Tabs(bar) => {
                bar.selected.set(value.max(0.) as usize);
                self.refresh_tabs(bar);
            }
            _ => {}
        }
    }

    fn set_icon(&mut self, view: &View, icon: &str) {
        if let View::Image(v) = view {
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(icon),
                None,
            );
            v.setImage(image.as_deref());
            return;
        }
        self.icons.insert(view.id(), icon.to_string());
        for bar in &self.tab_bars {
            self.refresh_tabs(bar);
        }
    }

    fn set_appearance(&mut self, appearance: Appearance) {
        let name = match appearance {
            Appearance::System => None,
            Appearance::Light => Some(unsafe { objc2_app_kit::NSAppearanceNameAqua }),
            Appearance::Dark => Some(unsafe { objc2_app_kit::NSAppearanceNameDarkAqua }),
        };
        let appearance = name.and_then(objc2_app_kit::NSAppearance::appearanceNamed);
        NSApplication::sharedApplication(self.mtm).setAppearance(appearance.as_deref());
    }

    fn set_scroll_events(&mut self, view: &View, on: bool) {
        let View::Scroll(sc) = view else { return };
        let clip = sc.scroll.contentView();
        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        let name = unsafe { objc2_app_kit::NSViewBoundsDidChangeNotification };
        unsafe { center.removeObserver_name_object(&self.target, Some(name), Some(&clip)) };
        if on {
            clip.setPostsBoundsChangedNotifications(true);
            unsafe {
                center.addObserver_selector_name_object(
                    &self.target,
                    sel!(onScrollBounds:),
                    Some(name),
                    Some(&clip),
                )
            };
        }
    }

    fn set_swipe(&mut self, view: &View, left: bool, right: bool) {
        if let View::Container(v) | View::Screen(v) | View::Tab(v) = view {
            v.ivars().swipe.set((left, right));
        }
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
        let v: Retained<NSView> = objc2::Message::retain(view.ns());
        let old = v.frame();
        self.set_frame(view, x, y, w, h);
        let new = v.frame();
        v.setFrame(old);
        animate(t, move || v.animator().setFrame(new));
    }

    fn set_style_animated(&mut self, view: &View, style: &Style, t: prism_core::Transition) {
        let v: Retained<NSView> = objc2::Message::retain(view.ns());
        let alpha = v.alphaValue();
        self.set_style(view, style);
        let new = v.alphaValue();
        v.setAlphaValue(alpha);
        animate(t, move || v.animator().setAlphaValue(new));
    }

    fn set_environment(&mut self, env: &prism_core::RenderEnvironment) {
        self.rtl = env.direction == prism_core::Direction::Rtl;
        self.chrome = (env.back.clone(), env.close.clone());
        let dir = if self.rtl {
            objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft
        } else {
            objc2_app_kit::NSUserInterfaceLayoutDirection::LeftToRight
        };
        self.root.setUserInterfaceLayoutDirection(dir);
        for stack in self.stacks.clone() {
            self.refresh_stack(&stack);
        }
    }

    fn set_menu(&mut self, view: &View, menu: Option<&prism_core::menu::MenuSpec>) {
        if let View::Container(v) | View::Screen(v) | View::Tab(v) = view {
            *v.ivars().menu.borrow_mut() = menu.cloned();
        }
    }

    fn set_content_size(&mut self, view: &View, width: f32, height: f32) {
        if let View::Scroll(sc) = view {
            sc.document
                .setFrameSize(NSSize::new(width as f64, height as f64));
        }
    }

    fn set_accessibility(&mut self, view: &View, a: &Accessibility) {
        let v = view.ns();
        if matches!(view, View::Stack(_) | View::Tabs(_) | View::Scroll(_)) {
            return;
        }
        v.setAccessibilityHidden(a.hidden);
        let ns = |s: &Option<String>| s.as_deref().map(NSString::from_str);
        let native = matches!(view, View::Button(_) | View::Input(_) | View::Toggle(_));
        if !native {
            let role = unsafe {
                match a.role {
                    Some(Role::Button) => Some(NSAccessibilityButtonRole),
                    Some(Role::Link) => Some(NSAccessibilityLinkRole),
                    Some(Role::Heading(_)) => Some(NSAccessibilityHeadingRole),
                    Some(Role::Image) => Some(NSAccessibilityImageRole),
                    Some(Role::Text) => Some(NSAccessibilityStaticTextRole),
                    Some(Role::Switch | Role::Checkbox) => Some(NSAccessibilityCheckBoxRole),
                    Some(Role::Tab) => Some(NSAccessibilityRadioButtonRole),
                    Some(Role::TabList) => Some(NSAccessibilityTabGroupRole),
                    Some(Role::List) => Some(NSAccessibilityListRole),
                    Some(Role::Group | Role::ListItem | Role::TabPanel) => {
                        Some(NSAccessibilityGroupRole)
                    }
                    _ => None,
                }
            };
            if let View::Container(_) | View::Screen(_) | View::Tab(_) = view {
                v.setAccessibilityElement(role.is_some() && !a.hidden);
                v.setAccessibilityRole(role);
                if a.role == Some(Role::Switch) {
                    v.setAccessibilityRoleDescription(Some(&NSString::from_str("switch")));
                }
            } else if let Some(role) = role
                && matches!(a.role, Some(Role::Heading(_)))
            {
                v.setAccessibilityRole(Some(role));
            }
            if let Some(checked) = a.checked {
                let value = NSString::from_str(if checked { "1" } else { "0" });
                unsafe { v.setAccessibilityValue(Some(&value)) };
            }
        }
        if a.label.is_some() || !native {
            v.setAccessibilityLabel(ns(&a.label).as_deref());
        }
        v.setAccessibilityHelp(ns(&a.hint).as_deref());
        if let Some(value) = &a.value {
            unsafe { v.setAccessibilityValue(Some(&NSString::from_str(value))) };
        }
        v.setAccessibilityEnabled(!a.disabled);
    }

    fn screen_viewport(&mut self, stack: &View, screen: &View) -> Viewport {
        if let View::Sheet(sheet) = screen {
            let size = sheet
                .window
                .contentView()
                .map_or(NSSize::new(520., 620.), |v| v.frame().size);
            return Viewport {
                x: 0.,
                y: 0.,
                width: size.width as f32,
                height: size.height as f32,
                insets: Insets::default(),
            };
        }
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

/// Opens a window and runs the AppKit event loop. Must be called on the main thread.
pub fn run(config: AppConfig, init: AppInit) {
    let mtm =
        MainThreadMarker::new().expect("prism-macos: run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let target = Target::new(mtm);
    crate::menus::install_main_menu(mtm, &config.title, &target, &[]);
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
    let services = crate::services::MacServices::new(
        mtm,
        Retained::into_super(Retained::into_super(target.clone())),
        window.clone(),
        config.title.clone(),
    );
    let mut runtime = Runtime::with_options(
        &init,
        prism_core::RuntimeOptions {
            path,
            capabilities: prism_core::PlatformServices::capabilities(&services),
            storage: Some(Box::new(prism_core::FileStorage::new(data_dir(
                &config.title,
            )))),
            wake_ui: Some(wake_ui()),
            environment: mac_environment(),
            formatter: Some(Box::new(prism_native::apple::FoundationFormatter)),
        },
    );
    let renderer = NativeRenderer::new(AppKit {
        mtm,
        target: target.clone(),
        root,
        stacks: Vec::new(),
        tab_bars: Vec::new(),
        titles: HashMap::new(),
        icons: HashMap::new(),
        timers: HashMap::new(),
        sheets: Vec::new(),
        pictures: HashMap::new(),
        image_cache: HashMap::new(),
        image_sources: image_sources(config.assets.as_deref()),
        rtl: false,
        chrome: ("Back".into(), "Close".into()),
    });
    let first = runtime.render();
    let mut state = State {
        runtime,
        renderer,
        services,
    };
    state.commit(first);
    state.relayout();
    let _ticker = unsafe {
        objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            &target,
            sel!(onTick:),
            None,
            true,
        )
    };
    STATE.with(|cell| *cell.borrow_mut() = Some(state));
    flush_responses();

    observe_system(&target);
    window.makeKeyAndOrderFront(None);
    app.activate();
    if std::env::var_os("PRISM_SNAPSHOT").is_some() || std::env::var_os("PRISM_CLICK").is_some() {
        let none: Option<&AnyObject> = None;
        // `PRISM_DELAY` (seconds) leaves time for images or network to load.
        let delay: f64 = std::env::var("PRISM_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.5);
        let _: () = unsafe {
            msg_send![&*target, performSelector: sel!(prismAutomation:), withObject: none, afterDelay: delay]
        };
    }
    app.run();
    drop(window);
}

// ---------------------------------------------------------------------------
// Dev automation (env-driven, used by `mise run macos-snapshot`)
// ---------------------------------------------------------------------------

/// Something `PRISM_CLICK` can press: a native button by title, or any
/// clickable Prism view by its accessibility label (what VoiceOver reads).
enum Pressable {
    Native(Retained<NSButton>),
    Custom(Retained<PrismView>),
}

fn find_pressable(view: &NSView, title: &str) -> Option<Pressable> {
    for sub in view.subviews().iter() {
        if sub.isHiddenOrHasHiddenAncestor() {
            continue;
        }
        if let Ok(b) = sub.clone().downcast::<NSButton>()
            && b.title().to_string() == title
        {
            return Some(Pressable::Native(b));
        }
        if let Ok(p) = sub.clone().downcast::<PrismView>()
            && p.ivars().clickable.get()
            && p.accessibilityLabel()
                .is_some_and(|l| l.to_string() == title)
        {
            return Some(Pressable::Custom(p));
        }
        if let Some(found) = find_pressable(&sub, title) {
            return Some(found);
        }
    }
    None
}

fn run_automation(mtm: MainThreadMarker) {
    let mut root = None;
    with_state(|s| root = Some(s.renderer.widgets.root.clone()));
    let Some(root) = root else { return };
    let sheet_roots = || {
        let mut roots: Vec<Retained<NSView>> = Vec::new();
        with_state(|s| {
            for sheet in &s.renderer.widgets.sheets {
                roots.push(Retained::into_super(sheet.content.clone()));
            }
        });
        roots
    };

    if let Ok(clicks) = std::env::var("PRISM_CLICK") {
        for title in clicks.split('|').filter(|t| !t.is_empty()) {
            // Sheets first: they are in front.
            let found = sheet_roots()
                .iter()
                .find_map(|r| find_pressable(r, title))
                .or_else(|| find_pressable(&root, title));
            match found {
                Some(Pressable::Native(b)) => unsafe { b.performClick(None) },
                Some(Pressable::Custom(p)) => {
                    let _: objc2::runtime::Bool =
                        unsafe { msg_send![&*p, accessibilityPerformPress] };
                }
                None => eprintln!("prism-macos: PRISM_CLICK: nothing labelled {title:?}"),
            }
        }
    }

    // `PRISM_SCROLL=<y>` scrolls the visible scroll views.
    if let Some(y) = std::env::var("PRISM_SCROLL")
        .ok()
        .and_then(|y| y.parse::<f64>().ok())
    {
        fn scroll_all(view: &NSView, y: f64) {
            if view.isHiddenOrHasHiddenAncestor() || view.window().is_none() {
                return;
            }
            if let Some(sv) = view.downcast_ref::<NSScrollView>() {
                let clip = sv.contentView();
                clip.scrollToPoint(NSPoint::new(0., y));
                sv.reflectScrolledClipView(&clip);
            }
            for child in view.subviews().iter() {
                scroll_all(&child, y);
            }
        }
        scroll_all(&root, y);
    }

    with_state(|s| {
        for title in s.services.status_titles() {
            println!("prism-macos: menu bar item: {title:?}");
        }
        if let Some(sheet) = s
            .renderer
            .widgets
            .root
            .window()
            .and_then(|w| w.attachedSheet())
        {
            println!(
                "prism-macos: attached sheet: {}",
                sheet.class().name().to_str().unwrap_or("?")
            );
        }
    });
    if let Ok(path) = std::env::var("PRISM_SNAPSHOT") {
        for (i, sheet) in sheet_roots().iter().enumerate() {
            write_png(sheet, &format!("{path}.sheet{i}.png"));
        }
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
        // `terminate:` is refused while a sheet is attached; this is a dev hook.
        let _ = mtm;
        std::process::exit(0);
    }
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

/// `~/Library/Application Support/<bundle id>` (or `prism.<title>` when run
/// unbundled with `cargo run`). `PRISM_DATA_DIR` overrides it.
fn data_dir(title: &str) -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("PRISM_DATA_DIR") {
        return dir.into();
    }
    let id = objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let slug: String = title
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            format!("prism.{slug}")
        });
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home)
        .join("Library/Application Support")
        .join(id)
}

fn write_png(view: &NSView, path: &str) {
    view.displayIfNeeded();
    let bounds = view.bounds();
    if let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(bounds) {
        view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
        let props = objc2_foundation::NSDictionary::new();
        if let Some(data) = unsafe {
            rep.representationUsingType_properties(
                objc2_app_kit::NSBitmapImageFileType::PNG,
                &props,
            )
        } {
            let _ = std::fs::write(path, data.to_vec());
        }
    }
}

/// Assets: the bundle's Resources/assets, then the dev directory; downloads
/// are cached in ~/Library/Caches.
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

/// Locale and Reduce Motion (`PRISM_TEXT_SCALE` scales text, macOS has no
/// Dynamic Type).
fn mac_environment() -> prism_core::Environment {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    prism_core::Environment {
        locale: prism_native::apple::system_locale(),
        text_scale: std::env::var("PRISM_TEXT_SCALE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.),
        reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        utc_offset: prism_native::apple::utc_offset(),
    }
}

fn observe_system(target: &Target) {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    unsafe {
        workspace
            .notificationCenter()
            .addObserver_selector_name_object(
                target,
                sel!(onEnvironment:),
                Some(objc2_app_kit::NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
            );
        objc2_foundation::NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            target,
            sel!(onEnvironment:),
            Some(objc2_foundation::NSCurrentLocaleDidChangeNotification),
            None,
        );
    }
}

/// Runs `f` (which changes `animator()` proxies) in an animation group.
fn animate(t: prism_core::Transition, f: impl Fn() + 'static) {
    let block = block2::RcBlock::new(
        move |ctx: std::ptr::NonNull<objc2_app_kit::NSAnimationContext>| {
            let ctx = unsafe { ctx.as_ref() };
            // Springs overshoot: approximate with a slightly longer ease.
            let duration = match t.curve {
                prism_core::Curve::Spring => t.duration * 1.2,
                _ => t.duration,
            };
            ctx.setDuration(duration as f64);
            ctx.setAllowsImplicitAnimation(true);
            f();
        },
    );
    objc2_app_kit::NSAnimationContext::runAnimationGroup(&block);
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
