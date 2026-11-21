//! # prism-core
//!
//! The platform-agnostic heart of Prism. An app is a tree of [`Render`]
//! entities; rendering produces [`Element`]s; the [`Reconciler`] diffs them into
//! [`Patch`]es; a [`Backend`] applies the patches to a real host (DOM, AppKit,
//! UIKit, ...) and feeds [`UiEvent`]s back through a [`Runtime`].

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]

mod a11y;
pub mod animation;
mod app;
mod apple_models;
mod assets;
pub mod attributes;
pub mod background;
pub mod car;
pub mod channel;
pub mod clipboard;
pub mod controls;
pub mod decoration;
pub mod device;
pub mod device_info;
mod element;
pub mod events;
pub mod fonts;
pub mod glyphs;
pub mod i18n;
pub mod interaction;
pub mod island;
pub mod layout;
pub mod list;
pub mod menu;
pub mod navbar;
pub mod platform;
pub mod push;
mod reconcile;
pub mod reorder;
pub mod request;
mod router;
pub mod scene3d;
pub mod security;
pub mod selection_across;
pub mod shape;
pub mod spans;
pub mod storage;
mod style;
pub mod surfaces;
pub mod svg;
pub mod table;
mod task;
pub mod text_edit;
pub mod video;
pub mod web_view;
mod widget_links;
mod window;
pub use table::{CELL_PADDING, Column, Table, TableCell, TableRow, cell, column, row, table};
pub use window::{WindowChrome, WindowControls};

pub use a11y::{Accessibility, Accessible, Role, Tooltip};
pub use animation::{Animations, Pose, Transform};
pub use app::{AnyEntity, App, Appearance, Context, Entity, EntityId, Environment, Render};
pub use assets::EmbeddedAssets;
pub use background::{BackgroundKind, BackgroundTask, Lifecycle};
pub use car::{CarButton, CarButtonStyle, CarItem, CarSection, CarTab, CarTemplate};
pub use channel::{Receiver, Sender, channel};
pub use clipboard::{ClipboardImage, ClipboardItem};
pub use controls::{
    Backdrop, Capitalize, DateMode, Keyboard, OverlayKind, PopoverAlign, PopoverSide, ReturnKey,
};
pub use decoration::{
    BorderSide, BorderSides, BorderStyle, Cursor, Gradient, GradientKind, GradientStop, Shadow,
};
pub use device::{
    Accuracy, DeviceCall, DeviceError, DeviceEvent, DeviceValue, Haptic, Location, Motion,
    Permission, PermissionState, PickImages, PickedFile, SaveFile, SaveSource, SavedFile, Share,
    Upload, UploadProgress, Watch,
};
pub use device::{FOLDER_MIME, PickFiles, PickFolder, PickedFolder};
pub use device_info::{
    Browser, DeviceInfo, DeviceKind, Engine, Fold, Orientation, Os, OsName, Posture, Rotation,
    ScreenInfo, Version, WindowMode,
};
pub use element::scroll_x;
pub use element::{
    BackEvent, Button, ChangeEvent, ClickEvent, DatePicker, DismissReason, Div, Element,
    FluentBuilder, FocusEvent, Handler, Image, InputEvent, IntoElement, Modal, NavigateEvent,
    NodeKind, ParentElement, Picker, Picture, Progress, Screen, Scroll, ScrollEvent, Shape, Slider,
    Spinner, Stack, SwipeDirection, Tab, Tabs, Text, TextArea, TextInput, Timer, Toggle, VNode,
    button, date_picker, date_picker_opt, div, format_remaining, image, lazy, modal, picker,
    picture, progress, rich_text, screen, scroll, shape, slider, spinner, stack, tab, tabs, text,
    text_area, text_input, timer, timer_text, toggle, unix_seconds, unix_time,
};
pub use element::{
    Checkbox, Overlay, RadioGroup, checkbox, dialog, keyboard_toolbar, popover, radio_group, sheet,
    time_picker, time_picker_opt,
};
pub use element::{RemoteCaret, SelectionEvent, byte_to_char, char_to_byte};
pub use element::{RichInput, RichInputEvent, rich_input};
pub use element::{STACK_CLOSABLE, STACK_NO_HEADER};
pub use element::{cover_crop, picture_position, picture_source};
pub use element::{picture_data, svg};
pub use events::{
    CaretEdge, CaretEdges, DragEvent, DragOverEvent, DragOverPhase, DragPayload, DragPhase,
    DragPreview, DropEvent, DropPoint, DropProgress, FileDropEvent, GesturePhase, HoverEvent,
    KeyEvent, LayoutEvent, MagnifyEvent, Modifiers, Point, PointerButton, PointerEvent,
    PointerPhase, Rect, WheelEvent,
};
pub use fonts::{FontFace, FontSource, Stylesheet};
pub use i18n::{BasicFormatter, DateStyle, Direction, Formatter, Locale, Translations};
pub use interaction::{Interaction, Interactions, States};
pub use island::{Island, IslandEvent, IslandSource, island, web_component};
pub use layout::{
    GridTrack, Repetition, ScrollAxis, Scrollbar, auto_fill, auto_fit, fr, minmax, repeat,
};
pub use list::VirtualList;
pub use menu::{ActionRole, Alert, AlertStyle, Menu, MenuItem, SystemAction, Tray};
pub use navbar::{BarButton, BarPlacement, bar_button};
pub use platform::{Platform, Platforms};
pub use push::{
    ActivityPushToken, PushEvent, PushFuture, PushMessage, PushService, PushSubscription,
};
pub use reconcile::{
    IdHasher, IdMap, IdSet, NodeId, Patch, Reconciler, RenderEnvironment, RichEdit, UiEvent,
};
pub use reorder::{ReorderList, ReorderOptions};
pub use request::{Cookie, RequestKind, SameSite, UrlTarget, WebRequest};
pub use router::{Params, Router, RouterBuilder, go_back, navigate_to, normalize_path};
pub use router::{encode_query, parse_query};
pub use scene3d::{Scene3d, SceneTapEvent, scene3d};
pub use security::{
    AppleCredential, AppleSignIn, AuthRequest, Biometry, BiometryKind, SecureOptions,
    SecureStorage, WebAuthRequest, WebAuthnRequest,
};
pub use selection_across::{
    EditAcross, EditCause, FieldSelection, SelectionAcross, SelectionPoint, text_flow,
};
pub use shape::{Drawing, Path, Segment};
pub use spans::{
    OBJECT_CHAR, Span, TextRun, UnderlineStyle, chip, decode_runs, encode_runs, icon, link,
    normalize_spans, restyle_spans, span, spans_text, splice_spans, split_spans,
};
pub use storage::{FileStorage, MemoryStorage, Storage};
pub use style::*;
pub use surfaces::WidgetAction;
pub use surfaces::{
    Activity, ActivityId, Capabilities, Notification, NotificationAction, NotificationResponse,
    PlatformRequest, PlatformServices, SNode, Widget, WidgetConfig, WidgetEntry, WidgetFamily,
    WidgetOption, WidgetParameter,
};
pub use task::{AsyncApp, Background, Sleep, TaskHandle, WakeUi};
pub use task::{DEFAULT_BACKGROUND_THREADS, background_threads, set_background_threads};
pub use text_edit::{
    ClipboardContent, CopyEvent, PasteEvent, SelectionRectEvent, SpanClickEvent, edit_events,
};
pub use video::{EndedEvent, Video, VideoConfig, video};
pub use web_view::{WebLoadEvent, WebMessageEvent, WebSandbox, WebView, web_view};

use std::sync::Arc;

/// Builds the initial app state. Called once per window (native) or once per
/// browser session (web), so it must be shareable across threads.
pub type AppInit = Arc<dyn Fn(&mut App) + Send + Sync>;

/// What a backend provides to the app at startup.
#[derive(Default)]
pub struct RuntimeOptions {
    /// Initial path (deep link, request URL).
    pub path: String,
    /// What the platform supports (`cx.capabilities()`).
    pub capabilities: Capabilities,
    /// Where `cx.storage()` persists data (in memory if `None`).
    pub storage: Option<Box<dyn Storage>>,
    /// Called from any thread when async work is ready; the backend must then
    /// call [`Runtime::poll`] on the UI thread.
    pub wake_ui: Option<WakeUi>,
    /// System locale, text size, reduce motion.
    pub environment: Environment,
    /// Platform number/date formatting (portable fallback if `None`).
    pub formatter: Option<Box<dyn Formatter>>,
    /// Storage synced between devices (local storage semantics if `None`).
    pub cloud_storage: Option<Box<dyn Storage>>,
    /// Per-user data directory (web: per browser).
    pub data_dir: Option<std::path::PathBuf>,
    /// Data shared by every user (web: the server's directory).
    pub shared_data_dir: Option<std::path::PathBuf>,
    /// Window or viewport size at startup (a phone if unknown).
    pub viewport: Option<(f32, f32)>,
    /// Where the app runs (the compilation target if `None`; the web backend
    /// says `Web`).
    pub platform: Option<Platform>,
    /// What the device is (guessed from the platform and viewport if `None`).
    pub device_info: Option<DeviceInfo>,
}

/// Window/page settings shared by every backend.
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Window title (the page title on the web).
    pub title: String,
    /// Initial window width, in points.
    pub width: f32,
    /// Initial window height, in points.
    pub height: f32,
    /// TCP port used by the web backend (`PRISM_PORT` overrides it).
    pub port: u16,
    /// Directory of app assets (`picture("logo.png")`), used in development;
    /// bundles copy it into their resources.
    pub assets: Option<String>,
    /// App assets compiled into the binary (`Application::embedded_assets`):
    /// found before the files of [`assets`](Self::assets).
    pub embedded_assets: EmbeddedAssets,
    /// The desktop window's frame: the system's title bar, or none
    /// (`WindowChrome::Custom`, macOS and Windows). Ignored on the web,
    /// mobile and Linux.
    pub window_chrome: WindowChrome,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            title: "Prism".into(),
            width: 480.,
            height: 640.,
            port: 3000,
            assets: None,
            embedded_assets: EmbeddedAssets::new(),
            window_chrome: WindowChrome::Standard,
        }
    }
}

/// A host that can display a tree described by patches.
pub trait Backend {
    /// Applies `patches`, in order, to the host's views.
    fn apply(&mut self, patches: &[Patch]);
}

/// The app plus its reconciler: everything a backend needs to drive a UI.
pub struct Runtime {
    /// The app state: entities, globals and platform services.
    pub app: App,
    reconciler: Reconciler,
    /// `prism dev`: where the current route is saved, so a restart reopens it.
    dev_state: Option<(std::path::PathBuf, String)>,
    /// The key (name, modifier bits) a selection group took last
    /// (`consumes_key_event`, #231): its press acts on the group when
    /// dispatched. A key the backend let the field have does not.
    across_key: Option<(String, u8)>,
    /// The payload being dragged (#251).
    drag_session: Option<DragSession>,
}

/// A [`DragPayload`] being dragged (#251): from which node, and the target
/// it is over with the last position there.
struct DragSession {
    source: NodeId,
    payload: DragPayload,
    over: Option<(NodeId, Point)>,
}

impl Runtime {
    /// Starts the app on `/`.
    pub fn new(init: &AppInit) -> Self {
        Self::with_path(init, "/")
    }

    /// Starts the app on `path` (a deep link / the requested URL).
    pub fn with_path(init: &AppInit, path: &str) -> Self {
        Self::with_capabilities(init, path, Capabilities::default())
    }

    /// Like [`Runtime::with_path`], telling the app what the platform supports
    /// before it initializes.
    pub fn with_capabilities(init: &AppInit, path: &str, caps: Capabilities) -> Self {
        Self::with_options(
            init,
            RuntimeOptions {
                path: path.into(),
                capabilities: caps,
                ..Default::default()
            },
        )
    }

    /// Full control over the environment a backend gives the app.
    pub fn with_options(init: &AppInit, options: RuntimeOptions) -> Self {
        let mut app = App::new();
        app.set_initial_path(&options.path);
        app.set_capabilities(options.capabilities);
        if let Some(storage) = options.storage {
            app.set_storage(storage);
        }
        if let Some(wake) = options.wake_ui {
            app.executor.set_wake_ui(wake);
        }
        app.set_environment(options.environment);
        if let Some(formatter) = options.formatter {
            app.set_formatter(formatter);
        }
        if let Some(cloud) = options.cloud_storage {
            app.set_cloud_storage(cloud);
        }
        app.set_data_dirs(options.data_dir, options.shared_data_dir);
        if let Some(platform) = options.platform {
            app.window.platform = platform;
        }
        if let Some(size) = options.viewport {
            app.set_viewport(size);
        }
        if let Some(info) = options.device_info {
            app.set_device_info(info);
        }
        init(&mut app);
        Runtime {
            app,
            reconciler: Reconciler::new(),
            dev_state: std::env::var_os("PRISM_DEV_STATE").map(|p| (p.into(), String::new())),
            across_key: None,
            drag_session: None,
        }
    }

    /// Renders what is dirty (everything on first call) and returns the patches.
    pub fn render(&mut self) -> Vec<Patch> {
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// The first render, then the work it started, for a first paint with
    /// its data (the web backend's server-rendered page): while
    /// [`background`](AsyncApp::background) work (HTTP, database, files)
    /// started by `init`, a constructor or a `render` is in flight, waits
    /// for it, runs the tasks it resumes and renders what they changed,
    /// until none is left or `timeout` passes. Returns every patch, in
    /// order. Timers and channels are not waited for, so a clock or a live
    /// feed does not hold the paint; past `timeout`, what is still loading
    /// shows its loading state and updates later through [`poll`](Self::poll).
    /// A zero `timeout` is [`render`](Self::render).
    pub fn render_settled(&mut self, timeout: std::time::Duration) -> Vec<Patch> {
        let deadline = std::time::Instant::now() + timeout;
        let mut patches = self.render();
        while self.app.executor.wait_for_work(deadline) {
            patches.extend(self.poll());
        }
        patches
    }

    /// `handler` of node `id`, unless the node is disabled or gone: its
    /// control takes no clicks or submits then (a modified page could send
    /// them).
    fn enabled<T>(&self, id: NodeId, handler: impl FnOnce(&Reconciler) -> Option<T>) -> Option<T> {
        self.reconciler
            .is_enabled(id)
            .then(|| handler(&self.reconciler))
            .flatten()
    }

    /// Runs the handler for `event`, then re-renders the entities it touched.
    pub fn dispatch(&mut self, event: UiEvent) -> Vec<Patch> {
        // Answers to the field that reported the event (#217), after the
        // render.
        let mut answers = Vec::new();
        match event {
            UiEvent::Click(id) => {
                if let Some(h) = self.enabled(id, |r| r.click_handler(id)) {
                    h(&ClickEvent, &mut self.app);
                }
            }
            UiEvent::Input(id, value) if self.reconciler.is_enabled(id) => {
                if let Some(edit) = self.reconciler.rich_text_edit(id, &value) {
                    // A rich input on a backend that edits plain text (#115).
                    match edit {
                        reconcile::RichEdit::Submit => {
                            if let Some(h) = self.reconciler.submit_handler(id) {
                                h(&ClickEvent, &mut self.app);
                            }
                        }
                        reconcile::RichEdit::Spans(e) => {
                            if let Some(h) = self.reconciler.rich_input_handler(id) {
                                h(&e, &mut self.app);
                            }
                        }
                    }
                } else if let Some(h) = self.reconciler.input_handler(id) {
                    h(&InputEvent { value }, &mut self.app);
                }
            }
            UiEvent::RichInput(id, runs) if self.reconciler.is_enabled(id) => {
                if let Some((h, e)) = self.reconciler.rich_runs_edit(id, &runs) {
                    h(&e, &mut self.app);
                }
            }
            // A disabled field is not typed into.
            UiEvent::Input(..) | UiEvent::RichInput(..) => {}
            UiEvent::Select(id, start, end) => {
                // A selection across fields (#231) ends where the user
                // selects anew.
                let out = self.reconciler.across_select(id, start, end);
                self.finish_across(out, &mut answers);
                let e = SelectionEvent::new(start, end);
                // What the field shows: rendered back, it is an echo.
                self.reconciler.selection_reported(id, e);
                if let Some(h) = self.reconciler.select_handler(id) {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::Back(id, depth) => {
                self.reconciler.overlay_dismissed(id);
                if let Some(h) = self.reconciler.back_handler(id) {
                    let reason = if self.reconciler.is_overlay(id) {
                        element::DismissReason::Unknown
                    } else {
                        element::DismissReason::Back
                    };
                    h(&BackEvent { depth, reason }, &mut self.app);
                }
            }
            UiEvent::Dismiss(id, reason) => {
                self.reconciler.overlay_dismissed(id);
                if let Some(h) = self.reconciler.back_handler(id) {
                    h(&BackEvent { depth: 0, reason }, &mut self.app);
                }
            }
            UiEvent::Change(id, value) => {
                // Only what the control could report: in range, on a step,
                // `NaN` for a cleared optional date picker only (#89).
                if let Some(value) = self.reconciler.change_value(id, value)
                    && let Some(h) = self.reconciler.change_handler(id)
                {
                    h(&ChangeEvent { value }, &mut self.app);
                }
            }
            UiEvent::MenuSelect(item) => {
                if item >= menu::APP_MENU_BASE {
                    self.app.select_menu(item);
                } else if let Some(h) = self.reconciler.menu_handler(item) {
                    h(&ClickEvent, &mut self.app);
                }
            }
            UiEvent::AlertAction(alert, index) => self.app.alert_action(alert, index),
            UiEvent::Scroll(id, offset, viewport) => {
                // Kept for `cx.scroll_offset` (#193), from the views that
                // report it.
                if !(self.reconciler.reports_scroll(id)
                    && offset.is_finite()
                    && viewport.is_finite())
                {
                    return Vec::new();
                }
                self.app.scroll_offsets.insert(id, offset);
                if let Some(name) = self.reconciler.name_of(id) {
                    self.app.scroll_names.insert(name.to_string(), id);
                }
                if let Some(h) = self.reconciler.scroll_handler(id) {
                    h(&ScrollEvent { offset, viewport }, &mut self.app);
                }
            }
            UiEvent::Layout(id, frame) => {
                // Kept for `cx.frame_of` (#192), then the handler, from the
                // nodes that report it.
                let finite = [frame.x, frame.y, frame.width, frame.height]
                    .iter()
                    .all(|v| v.is_finite());
                if !(self.reconciler.reports_layout(id) && finite) {
                    return Vec::new();
                }
                self.app.frames.insert(id, frame);
                if let Some(name) = self.reconciler.name_of(id) {
                    self.app.frame_names.insert(name.to_string(), id);
                }
                if let Some(h) = self.reconciler.layout_handler(id) {
                    h(&LayoutEvent { frame }, &mut self.app);
                }
            }
            UiEvent::Swipe(id, dir) => {
                if let Some(h) = self.reconciler.swipe_handler(id, dir) {
                    h(&ClickEvent, &mut self.app);
                }
            }
            UiEvent::Navigate(id, path) => {
                if let Some(h) = self.reconciler.navigate_handler(id) {
                    h(&NavigateEvent { path }, &mut self.app);
                }
            }
            UiEvent::Submit(id) => {
                if let Some(h) = self.enabled(id, |r| r.submit_handler(id)) {
                    h(&ClickEvent, &mut self.app);
                }
            }
            UiEvent::ClickAt(id, position) => {
                if let Some(h) = self.enabled(id, |r| r.click_handler(id)) {
                    let e = ClickEvent {
                        position: Some(position),
                        ..ClickEvent
                    };
                    h(&e, &mut self.app);
                }
            }
            UiEvent::ClickWith(id, e) => {
                if let Some(h) = self.enabled(id, |r| r.click_handler(id)) {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::KeyUp(focused, e) => {
                self.app.stop_propagation = false;
                for h in self.reconciler.key_up_handlers(focused) {
                    h(&e, &mut self.app);
                    if self.app.stop_propagation {
                        break;
                    }
                }
                self.app.stop_propagation = false;
            }
            UiEvent::Pointer(id, e) => self.dispatch_pointer(id, e),
            UiEvent::Drag(id, e) => {
                if self.reconciler.drag_payload(id).is_some() {
                    self.payload_drag(id, e, None);
                } else if let Some(h) = self.reconciler.drag_handler(id) {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::PayloadDrag(id, e, over) => self.payload_drag(id, e, over),
            UiEvent::Wheel(id, e) => {
                if let Some(h) = self.reconciler.wheel_handler(id) {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::FocusChange(id, focused) => {
                if let Some(h) = self.reconciler.focus_change_handler(id) {
                    h(&FocusEvent { focused }, &mut self.app);
                }
            }
            UiEvent::AuxClick(id, e) => {
                if let Some(h) = self.enabled(id, |r| r.aux_click_handler(id)) {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::Magnify(id, e) => {
                if let Some(h) = self.reconciler.magnify_handler(id) {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::Ended(id) => {
                if let Some(h) = self.reconciler.ended_handler(id) {
                    h(&EndedEvent, &mut self.app);
                }
            }
            UiEvent::DropProgress(id, sent, total) => {
                if let Some(h) = self.reconciler.drop_progress_handler(id) {
                    h(&DropProgress { sent, total }, &mut self.app);
                }
            }
            UiEvent::DropFiles(id, files) => {
                if let Some((accept, h)) = self.reconciler.drop_files_handler(id) {
                    let files: Vec<_> = files
                        .into_iter()
                        .filter(|f| accept.accepts(&f.name, &f.mime))
                        .collect();
                    if !files.is_empty() {
                        h(&FileDropEvent { files }, &mut self.app);
                    }
                }
            }
            UiEvent::Island(id, name, detail) => {
                if let Some(h) = self.reconciler.island_handler(id, &name) {
                    h(&island::IslandEvent { name, detail }, &mut self.app);
                }
            }
            UiEvent::SceneTap(id, e) => {
                if let Some(h) = self.reconciler.scene_tap_handler(id) {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::WebMessage(id, data, origin) => {
                let allowed =
                    self.reconciler
                        .web_message_allowed(id, &origin, self.app.page_origin());
                if allowed && let Some(h) = self.reconciler.web_message_handler(id) {
                    h(&web_view::WebMessageEvent { data, origin }, &mut self.app);
                }
            }
            UiEvent::WebLoad(id, url) => {
                if let Some(h) = self.reconciler.web_load_handler(id) {
                    h(&web_view::WebLoadEvent { url }, &mut self.app);
                }
            }
            UiEvent::Key(focused, e) => {
                // A selection group's keys (#231): ⇧-arrows across fields,
                // edits and copies of a selection spanning them.
                // (The web client handles them itself.)
                let taken = self.across_key.take() == Some((e.key.clone(), e.modifiers.bits()));
                if taken
                    && self.app.platform() != Platform::Web
                    && let (Some(f), Some((group, fields, action))) =
                        (focused, self.reconciler.across_key(focused, &e))
                {
                    let out = self.reconciler.apply_across(group, &fields, f, action);
                    self.finish_across(out, &mut answers);
                }
                self.app.stop_propagation = false;
                for h in self.reconciler.key_handlers(focused) {
                    h(&e, &mut self.app);
                    if self.app.stop_propagation {
                        break;
                    }
                }
                self.app.stop_propagation = false;
            }
            UiEvent::SelectionRect(id, start, end, rect) => {
                let mut e = SelectionRectEvent::new(SelectionEvent::new(start, end), rect);
                e.frame = self.app.frames.get(&id).map(|f| {
                    events::Rect::new(f.x + rect.x, f.y + rect.y, rect.width, rect.height)
                });
                let handler = self.reconciler.edit_handlers(id).and_then(|h| {
                    self.app.selection_rects.insert(id, e);
                    h.on_selection_rect.clone()
                });
                if let Some(name) = self.reconciler.name_of(id) {
                    self.app.selection_names.insert(name.to_string(), id);
                }
                if let Some(h) = handler {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::Copy(id, cut, start, end, text) => {
                let spans = self.reconciler.rich_spans(id);
                let e =
                    text_edit::CopyEvent::new(SelectionEvent::new(start, end), text, &spans, cut);
                let handler = self.reconciler.edit_handlers(id).and_then(|h| {
                    if cut {
                        h.on_cut.clone()
                    } else {
                        h.on_copy.clone()
                    }
                });
                if let Some(h) = handler {
                    h(&e, &mut self.app);
                }
                let content = e
                    .written()
                    .unwrap_or_else(|| ClipboardContent::text(e.text.clone()));
                answers.push(Patch::WriteClipboard { id, content });
            }
            UiEvent::Paste(id, e) => {
                let handler = self
                    .reconciler
                    .edit_handlers(id)
                    .and_then(|h| h.on_paste.clone());
                if let Some(h) = handler {
                    h(&e, &mut self.app);
                }
                if !e.is_default_prevented() && !e.text.is_empty() {
                    answers.push(Patch::InsertText { id, text: e.text });
                }
            }
            UiEvent::SelectionAcross(group, range) => {
                // Native backends report it from a mouse drag (#264): the
                // runtime draws it there.
                let draw = self.app.platform() != Platform::Web;
                let out = self.reconciler.across_reported(group, range, draw);
                self.finish_across(out, &mut answers);
            }
            UiEvent::EditAcross(group, cause, text) => {
                let out = self
                    .reconciler
                    .across_client_edit(group, Some((cause, text)), None);
                self.finish_across(out, &mut answers);
            }
            UiEvent::CopyAcross(group, cut) => {
                let out = self.reconciler.across_client_edit(group, None, Some(cut));
                self.finish_across(out, &mut answers);
            }
            UiEvent::TypeAcross(field, text) => {
                if let Some((group, fields, action)) = self.reconciler.across_text(field, &text) {
                    let out = self.reconciler.apply_across(group, &fields, field, action);
                    self.finish_across(out, &mut answers);
                }
            }
            UiEvent::FieldClick(id, e) => {
                let handler = self
                    .reconciler
                    .edit_handlers(id)
                    .and_then(|h| h.on_click.clone());
                if let Some(h) = handler {
                    h(&e, &mut self.app);
                }
            }
            UiEvent::SpanClick(id, offset, modifiers) => {
                let spans = self.reconciler.rich_spans(id);
                let handler = self
                    .reconciler
                    .edit_handlers(id)
                    .and_then(|h| h.on_span_click.clone());
                if let (Some(h), Some(span)) = (handler, text_edit::span_at(&spans, offset)) {
                    h(&SpanClickEvent::new(span, offset, modifiers), &mut self.app);
                }
            }
        }
        // Tasks spawned by the handler start right away.
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        let mut patches = if self.app.has_dirty() {
            self.render_dirty()
        } else {
            Vec::new()
        };
        // Answers aimed at a node the re-render destroyed (an edit that
        // merged fields, #265) reach no one.
        let destroyed: std::collections::HashSet<NodeId> = patches
            .iter()
            .filter_map(|p| match p {
                Patch::Destroy { id } => Some(*id),
                _ => None,
            })
            .collect();
        if !destroyed.is_empty() {
            answers.retain(|a| !answer_targets(a).any(|id| destroyed.contains(&id)));
        }
        patches.extend(answers);
        patches
    }

    /// Hover (enter / leave) and move handlers of a pointer event.
    fn dispatch_pointer(&mut self, id: NodeId, e: PointerEvent) {
        if e.phase != PointerPhase::Move
            && let Some(h) = self.reconciler.hover_handler(id)
        {
            let hovered = e.phase == PointerPhase::Enter;
            h(&HoverEvent { hovered }, &mut self.app);
        }
        if e.phase != PointerPhase::Leave
            && let Some(h) = self.reconciler.pointer_handler(id)
        {
            h(&e, &mut self.app);
        }
    }

    /// A drag of a draggable node (#251): its own `on_drag`, then the drop
    /// targets' `on_drag_over` and, at the release, `on_drop_payload`.
    fn payload_drag(&mut self, id: NodeId, e: DragEvent, over: Option<events::DropPoint>) {
        if let Some(h) = self.reconciler.drag_handler(id) {
            h(&e, &mut self.app);
        }
        // What is carried is the payload the drag started with.
        let session = match self.drag_session.take() {
            Some(s) if s.source == id && e.phase != DragPhase::Start => Some(s),
            previous => {
                if let Some(old) = previous {
                    self.leave_target(&old);
                }
                self.reconciler
                    .drag_payload(id)
                    .cloned()
                    .map(|payload| DragSession {
                        source: id,
                        payload,
                        over: None,
                    })
            }
        };
        let Some(mut session) = session else { return };
        let kind = session.payload.kind.clone();
        let over =
            over.filter(|p| p.target != id && self.reconciler.accepts_payload(p.target, &kind));
        let modifiers = e.modifiers;
        match e.phase {
            DragPhase::Start | DragPhase::Move => {
                let phase = match (session.over, over) {
                    (Some((old, _)), Some(p)) if old == p.target => DragOverPhase::Move,
                    _ => {
                        self.leave_target(&session);
                        DragOverPhase::Enter
                    }
                };
                session.over = over.map(|p| (p.target, p.position));
                if let Some(p) = over
                    && let Some(h) = self.reconciler.drag_over_handler(p.target, &kind)
                {
                    let event = DragOverEvent {
                        phase,
                        payload: session.payload.clone(),
                        position: p.position,
                        modifiers,
                    };
                    h(&event, &mut self.app);
                }
                self.drag_session = Some(session);
            }
            DragPhase::End => {
                if let Some(p) = over
                    && let Some(h) = self.reconciler.drop_payload_handler(p.target, &kind)
                {
                    let event = DropEvent {
                        payload: session.payload.clone(),
                        position: p.position,
                        modifiers,
                    };
                    h(&event, &mut self.app);
                }
                if let Some(p) = over {
                    session.over = Some((p.target, p.position));
                }
                self.leave_target(&session);
            }
            DragPhase::Cancel => self.leave_target(&session),
        }
    }

    /// The payload left the target it was over (#251), if any.
    fn leave_target(&mut self, session: &DragSession) {
        let Some((target, position)) = session.over else {
            return;
        };
        if let Some(h) = self
            .reconciler
            .drag_over_handler(target, &session.payload.kind)
        {
            let event = DragOverEvent {
                phase: DragOverPhase::Leave,
                payload: session.payload.clone(),
                position,
                modifiers: Modifiers::NONE,
            };
            h(&event, &mut self.app);
        }
    }

    /// Runs ready async tasks and due timers, then re-renders what changed.
    /// Backends call it when woken (see `RuntimeOptions::wake_ui`) and at
    /// [`next_timer`](Runtime::next_timer).
    pub fn poll(&mut self) -> Vec<Patch> {
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        if self.app.has_dirty() {
            self.render_dirty()
        } else {
            Vec::new()
        }
    }

    /// The mounted node named `name` (`.id(name)`), if any.
    pub fn node_named(&self, name: &str) -> Option<NodeId> {
        self.reconciler.node_named(name)
    }

    /// When the earliest pending timer is due, if any.
    pub fn next_timer(&self) -> Option<std::time::Instant> {
        self.app.executor.next_timer()
    }

    /// Whether async tasks are alive (e.g. waiting on the network).
    pub fn has_tasks(&self) -> bool {
        self.app.executor.has_tasks()
    }

    /// Whether `background` work (HTTP, database, files) is running on
    /// other threads: backends that cannot be woken directly poll meanwhile.
    pub fn has_background_work(&self) -> bool {
        self.app.executor.has_background_work()
    }

    /// The selection group (#231) text field `field` belongs to, if any:
    /// a backend extends a pointer drag from it into the other fields of
    /// that group (`UiEvent::SelectionAcross`).
    pub fn selection_group(&self, field: NodeId) -> Option<NodeId> {
        self.reconciler.group_of(field)
    }

    /// The text fields of selection group `group` (#231), in document
    /// order: where a backend looks for the field under a pointer drag.
    pub fn selection_group_fields(&self, group: NodeId) -> Vec<NodeId> {
        self.reconciler
            .group_fields(group)
            .into_iter()
            .map(|f| f.id)
            .collect()
    }

    /// Whether text typed into field `field` by an input method or a soft
    /// keyboard (#231) goes to its selection group instead of the field:
    /// the selection spans several of the group's fields and the group has
    /// `on_edit_across`. The backend then keeps the field from inserting
    /// it and dispatches `UiEvent::TypeAcross` with the text (`""` for a
    /// deletion), after the field's own callback returned.
    pub fn takes_text_across(&self, field: NodeId) -> bool {
        self.app.platform() != Platform::Web && self.reconciler.across_text(field, "x").is_some()
    }

    /// Whether `key`, pressed in node `focused`, is the app's alone: its
    /// [`consumes_key`](Self::consumes_key), or a key a selection group
    /// takes from its focused field (#231: ⇧-arrows at a field's edge, and
    /// arrows, edits and copies while the selection spans several fields).
    /// Backends call this with the key as they will dispatch it (its
    /// modifiers, the field's selection and caret edges).
    ///
    /// A key it takes for a selection group acts on the group when it is
    /// then dispatched (`UiEvent::Key` with the same key and modifiers);
    /// a key the backend did not ask about stays the field's.
    pub fn consumes_key_event(&mut self, focused: Option<NodeId>, key: &KeyEvent) -> bool {
        let across = self.app.platform() != Platform::Web
            && self.reconciler.across_key(focused, key).is_some();
        if across {
            self.across_key = Some((key.key.clone(), key.modifiers.bits()));
        }
        across || self.reconciler.consumes_key(focused, &key.key, key.edges)
    }

    /// Runs what a selection group's key, report or edit led to (#231).
    fn finish_across(&mut self, out: reconcile::AcrossOutcome, answers: &mut Vec<Patch>) {
        answers.extend(out.patches);
        if let Some((h, e)) = out.report {
            h(&e, &mut self.app);
        }
        if let Some((id, handler, e)) = out.copy {
            if let Some(h) = handler {
                h(&e, &mut self.app);
            }
            let content = e
                .written()
                .unwrap_or_else(|| ClipboardContent::text(e.text.clone()));
            answers.push(Patch::WriteClipboard { id, content });
        }
        if let Some((h, e)) = out.edit {
            h(&e, &mut self.app);
        }
        if let Some((h, selection)) = out.paste {
            self.app.spawn(move |cx| async move {
                let text = cx.read_clipboard().await.unwrap_or_default();
                cx.update(|app| h(&EditAcross::new(selection, text, EditCause::Paste), app));
            });
        }
    }

    /// Whether a key pressed in node `focused` is the app's alone
    /// (`consume_keys`, #181): the backend keeps the focused control from
    /// acting on it after dispatching it. `edges`: where the focused text
    /// field's caret was when the key was pressed (#211), for entries of
    /// [`consume_key_at`](Div::consume_key_at).
    pub fn consumes_key(
        &self,
        focused: Option<NodeId>,
        key: &str,
        edges: Option<CaretEdges>,
    ) -> bool {
        self.reconciler.consumes_key(focused, key, edges)
    }

    /// Platform work requested by the app (widgets, Live Activities,
    /// notifications). Backends call this after each render or dispatch.
    pub fn take_requests(&mut self) -> Vec<PlatformRequest> {
        self.app.take_requests()
    }

    /// Opens a deep link (from a widget, Live Activity or notification tap)
    /// and returns the resulting patches. A widget action's link
    /// ([`App::widget_action_link`]) runs the action instead of navigating,
    /// once. One without a token of the app (any app or web page can open
    /// an app's links) does nothing; the app's own link opened again, or
    /// from a drawing of the widgets replaced too long ago, runs nothing
    /// and redraws the widgets (they showed an old drawing).
    pub fn open_link(&mut self, path: &str) -> Vec<Patch> {
        if let Some(action) = WidgetAction::from_link(path) {
            let token = WidgetAction::link_token(path);
            match self.app.check_widget_link(&action, token.as_deref()) {
                surfaces::LinkCheck::Run => {
                    self.run_widget_action(action);
                }
                surfaces::LinkCheck::Used | surfaces::LinkCheck::Expired => {
                    self.app.reload_widgets();
                }
                surfaces::LinkCheck::Forged => {}
            }
            return self.poll();
        }
        self.app.open_route_link(path);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// A car screen (CarPlay, Android Auto) shows the app, or stopped.
    pub fn car_connected(&mut self, connected: bool) -> Vec<Patch> {
        self.app.set_car_connected(connected);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// An item or button of the car's templates was selected (its `id`).
    pub fn car_select(&mut self, id: u64) -> Vec<Patch> {
        self.app.car_select(id);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// The car went back to screen `depth` (0: the root).
    pub fn car_back(&mut self, depth: usize) -> Vec<Patch> {
        self.app.car_back(depth);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// A notification was tapped or one of its actions chosen (`link`: the
    /// action's or notification's deep link). Returns the resulting patches.
    pub fn notification_response(
        &mut self,
        response: surfaces::NotificationResponse,
        link: Option<String>,
    ) -> Vec<Patch> {
        self.app.notification_response(response, link);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// The platform's answer to a push registration, or a refreshed token.
    /// Returns the resulting patches.
    pub fn push_registered(
        &mut self,
        result: Result<push::PushSubscription, device::DeviceError>,
    ) -> Vec<Patch> {
        self.app.push_registered(result);
        self.poll()
    }

    /// A push arrived while the app runs, or was tapped (its link opens).
    /// `display`: the platform does not show pushes itself, so a visible
    /// one becomes a notification while the app is in the background.
    pub fn push_received(&mut self, push: push::PushMessage, display: bool) -> Vec<Patch> {
        self.app.push_received(push, display);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// A silent push: runs the push handlers and the app's
    /// `on_background_push` job. Returns the job's task id (watch it with
    /// [`is_task_running`](Self::is_task_running), then tell the platform),
    /// or `None` if the app has no such job.
    pub fn run_background_push(&mut self, push: push::PushMessage) -> Option<u64> {
        let handle = self.app.start_background_push(push);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        handle.map(|h| h.id())
    }

    /// A widget or Live Activity button was tapped: runs the app's
    /// [`on_widget_action`](App::on_widget_action) job, then reloads the
    /// widgets. Returns the task's id: watch it with
    /// [`is_task_running`](Self::is_task_running) and tell the platform
    /// when it is done.
    pub fn run_widget_action(&mut self, action: WidgetAction) -> u64 {
        let handle = self.app.start_widget_action(action);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        handle.id()
    }

    /// A Live Activity push token arrived or changed.
    pub fn activity_push_token(&mut self, token: push::ActivityPushToken) -> Vec<Patch> {
        self.app.activity_push_token(token);
        self.poll()
    }

    /// Renders dirty entities, then remembers the route for `prism dev`.
    fn render_dirty(&mut self) -> Vec<Patch> {
        let patches = self.reconciler.render(&mut self.app);
        // Offsets of scroll views no longer mounted are gone (#193).
        if !self.app.scroll_offsets.is_empty() {
            let r = &self.reconciler;
            self.app.scroll_offsets.retain(|id, _| r.is_mounted(*id));
            let offsets = &self.app.scroll_offsets;
            self.app
                .scroll_names
                .retain(|_, id| offsets.contains_key(id));
        }
        // Frames of elements no longer laid out are gone (#192).
        if !self.app.frames.is_empty() {
            let r = &self.reconciler;
            self.app.frames.retain(|id, _| r.reports_layout(*id));
            let frames = &self.app.frames;
            self.app.frame_names.retain(|_, id| frames.contains_key(id));
        }
        // Rectangles of rich inputs no longer reporting them (#217).
        if !self.app.selection_rects.is_empty() {
            let r = &self.reconciler;
            self.app.selection_rects.retain(|id, _| {
                r.edit_handlers(*id)
                    .is_some_and(|h| h.mask() & edit_events::SELECTION_RECT != 0)
            });
            let rects = &self.app.selection_rects;
            self.app
                .selection_names
                .retain(|_, id| rects.contains_key(id));
        }
        if let Some((file, saved)) = &mut self.dev_state
            && let Some(path) = self.app.current_path()
            && path != saved
        {
            *saved = path.to_string();
            let _ = std::fs::write(file, path);
        }
        patches
    }

    /// A backend's answer to device call `id`. Returns the resulting patches.
    pub fn device_reply(
        &mut self,
        id: u64,
        result: Result<device::DeviceValue, device::DeviceError>,
    ) -> Vec<Patch> {
        self.app.device_reply(id, result);
        self.poll()
    }

    /// A sensor event for stream `id`. Returns the resulting patches.
    pub fn device_event(&mut self, id: u64, event: device::DeviceEvent) -> Vec<Patch> {
        self.app.device_event(id, event);
        self.poll()
    }

    /// The window was resized (re-renders everything when it crosses a
    /// breakpoint). Returns the resulting patches.
    pub fn set_viewport(&mut self, width: f32, height: f32) -> Vec<Patch> {
        self.app.set_viewport((width, height));
        self.poll()
    }

    /// The window is `width` × `height` and the system's bars, cutouts and
    /// side bars cover `safe_area` of it (not the on-screen keyboard):
    /// `cx.viewport()` becomes what they leave, and breakpoints follow its
    /// width. Re-renders when the insets or a breakpoint change. Returns
    /// the resulting patches.
    pub fn set_window(&mut self, width: f32, height: f32, safe_area: style::Edges) -> Vec<Patch> {
        self.app.set_window((width, height), safe_area);
        self.poll()
    }

    /// The window entered or left full screen, or its buttons moved
    /// (`cx.is_fullscreen()`, `cx.window_controls()`). Re-renders
    /// everything if anything differs. Returns the resulting patches.
    pub fn set_window_chrome(
        &mut self,
        fullscreen: bool,
        controls: Option<WindowControls>,
    ) -> Vec<Patch> {
        self.app.set_window_chrome_state(fullscreen, controls);
        self.poll()
    }

    /// The device changed (rotation, fold, split view, browser report).
    /// Re-renders everything if anything differs. Returns the resulting patches.
    pub fn set_device_info(&mut self, info: DeviceInfo) -> Vec<Patch> {
        self.app.set_device_info(info);
        self.poll()
    }

    /// What the platform supports changed after startup (`cx.capabilities()`:
    /// a check that answered late, a device plugged in). Re-renders
    /// everything if anything differs. Returns the resulting patches.
    pub fn set_capabilities(&mut self, caps: Capabilities) -> Vec<Patch> {
        self.app.update_capabilities(caps);
        self.poll()
    }

    /// Another device changed these synced keys. Returns the resulting patches.
    pub fn cloud_changed(&mut self, keys: &[String]) -> Vec<Patch> {
        self.app.cloud_changed(keys);
        self.poll()
    }

    /// Runs ready tasks without rendering: for apps launched in the
    /// background with no UI yet (the first `render` draws everything).
    pub fn poll_headless(&mut self) {
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
    }

    /// The app moved on or off screen. Returns the resulting patches.
    pub fn set_lifecycle(&mut self, phase: Lifecycle) -> Vec<Patch> {
        self.app.set_lifecycle(phase);
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        self.render_dirty()
    }

    /// The platform runs background task `id`. Returns the running task's id
    /// (watch it with [`is_task_running`](Self::is_task_running) and tell the
    /// platform when it is done), or `None` if no such task is registered.
    pub fn run_background_task(&mut self, id: &str) -> Option<u64> {
        let handle = self.app.start_background_task(id)?;
        task::poll_tasks(&mut self.app);
        self.app.flush_effects();
        Some(handle.id())
    }

    /// Whether the task returned by [`run_background_task`](Self::run_background_task) is still running.
    pub fn is_task_running(&self, id: u64) -> bool {
        self.app.executor.is_running(id)
    }

    /// Whether a task asked to finish in the background.
    pub fn wants_background_time(&self) -> bool {
        self.app.executor.wants_background_time()
    }

    /// Stops a task (e.g. the platform's background time expired).
    pub fn cancel_task(&mut self, id: u64) {
        self.app.executor.cancel(id);
    }

    /// Entity renders performed by the last render (for tests and profiling).
    pub fn last_render_count(&self) -> usize {
        self.reconciler.last_render_count()
    }
}

/// Everything an app usually needs.
pub mod prelude {
    pub use crate::WidgetAction;
    pub use crate::{
        Accessible, Accuracy, Activity, Alert, Animations, App, AppleSignIn, AsyncApp, AuthRequest,
        BackEvent, BackgroundTask, BiometryKind, Breakpoint, Capitalize, CarButton, CarButtonStyle,
        CarItem, CarSection, CarTab, CarTemplate, CaretEdge, CaretEdges, ChangeEvent, ClickEvent,
        ClipboardImage, ClipboardItem, Context, Corners, DateMode, DateStyle, DeviceInfo,
        DeviceKind, DismissReason, Entity, FluentBuilder, FocusEvent, Handler, Haptic, InputEvent,
        IntoElement, Keyboard, Lifecycle, Locale, Location, Menu, MenuItem, Motion, Notification,
        NotificationAction, NotificationResponse, Orientation, Params, ParentElement, Path,
        Permission, PermissionState, PickImages, PickedFile, Platform, Pose, Posture, PushEvent,
        PushMessage, PushService, PushSubscription, RemoteCaret, Render, ReorderList,
        ReorderOptions, ReturnKey, Role, Router, SaveFile, SavedFile, ScrollEvent, SecureOptions,
        SelectionEvent, Share, Styled, SwipeDirection, SystemAction, Transition, Translations,
        Tray, UploadProgress, VirtualList, Watch, Widget, WidgetConfig, WidgetFamily, WidgetOption,
        WidgetParameter, WindowChrome, WindowControls, adaptive, button, date_picker,
        date_picker_opt, div, go_back, image, lazy, navigate_to, picker, picture, progress, px,
        relative, rgb, rgba, screen, scroll, shape, slider, spinner, text, text_area, text_input,
        timer, toggle,
    };
    // One selection across several text fields (#231).
    pub use crate::{EditAcross, EditCause, SelectionAcross, SelectionPoint, text_flow};
    pub use crate::{
        GridTrack, ScrollAxis, Scrollbar, auto_fill, auto_fit, fr, minmax, repeat, scroll_x,
    };
    // Tables (#284).
    pub use crate::{cell, column, row, table};
    // App assets compiled into the binary (#61).
    pub use crate::EmbeddedAssets;
    // Typography and rich text.
    pub use crate::{
        FontFace, FontFamily, FontWeight, RichInputEvent, Span, Stylesheet, TextTransform,
        TextWrap, UnderlineStyle, chip, icon, link, rich_input, rich_text, span,
    };
    // Rich input editing hooks (#217).
    pub use crate::{ClipboardContent, CopyEvent, PasteEvent, SelectionRectEvent, SpanClickEvent};
    // Box decorations and interaction states.
    pub use crate::decoration::{shadow_lg, shadow_md, shadow_sm};
    pub use crate::{Color, Cursor, Gradient, Shadow};
    pub use crate::{picture_data, svg};
    // Navigation bar buttons (#30).
    pub use crate::bar_button;
    // Pointer, keys, choice controls, overlays (#17, #18, #19).
    pub use crate::{
        DragEvent, DragOverEvent, DragOverPhase, DragPayload, DragPhase, DragPreview, DropEvent,
        DropProgress, FileDropEvent, GesturePhase, HoverEvent, KeyEvent, LayoutEvent, MagnifyEvent,
        Modifiers, PickFiles, Point, PointerButton, PointerEvent, PointerPhase, PopoverAlign,
        PopoverSide, Rect, WheelEvent, checkbox, dialog, keyboard_toolbar, popover, radio_group,
        sheet, time_picker, time_picker_opt,
    };
    // Anchored overlays without a dim (#79).
    pub use crate::Backdrop;
    // Islands: client-side Web Components.
    pub use crate::{IslandEvent, island, web_component};
    // 3D scenes (the nodes, lights and camera: `use prism::scene3d::*`).
    pub use crate::{SceneTapEvent, scene3d};
    // Web views (#62).
    pub use crate::{WebLoadEvent, WebMessageEvent, WebSandbox, web_view};
    // Videos (#207).
    pub use crate::{EndedEvent, video};
}

#[cfg(test)]
mod temp_dir;
#[cfg(test)]
mod tests;

/// The nodes an answer of `dispatch` (a selection, a highlight, a clipboard
/// write) is aimed at (#265).
fn answer_targets(p: &Patch) -> impl Iterator<Item = NodeId> {
    let ids: Vec<NodeId> = match p {
        Patch::SetSelection { id, .. }
        | Patch::SetSelectionHighlight { id, .. }
        | Patch::Focus { id }
        | Patch::WriteClipboard { id, .. }
        | Patch::InsertText { id, .. } => vec![*id],
        Patch::SetSelectionAcross { group, range } => {
            let mut v = vec![*group];
            if let Some(((a, _), (b, _))) = range {
                v.extend([*a, *b]);
            }
            v
        }
        _ => Vec::new(),
    };
    ids.into_iter()
}
