//! State model: GPUI-style entities owned by an [`App`].
//!
//! Updates are entity-granular: `cx.notify()` marks only that entity dirty, and
//! the reconciler re-renders just the dirty entities, in place, reusing the
//! previous output of every other entity untouched.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use crate::element::{Element, Handler, IntoElement};
use crate::router::{NavOp, Router};

/// A stateful view. `render` runs on first mount and whenever the entity is
/// notified; other entities' output is reused as-is.
pub trait Render: 'static + Sized {
    /// Describes this entity's UI.
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Identifies an entity within its [`App`].
pub struct EntityId(u64);

/// A typed handle to state owned by the [`App`].
pub struct Entity<T> {
    id: EntityId,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Entity<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Entity<T> {}

impl<T: 'static> Entity<T> {
    #[cfg(test)]
    pub(crate) fn from_id(id: EntityId) -> Self {
        Entity {
            id,
            _marker: PhantomData,
        }
    }

    /// This entity's id.
    pub fn id(&self) -> EntityId {
        self.id
    }

    /// Borrows the entity's state.
    pub fn read<'a>(&self, app: &'a App) -> &'a T {
        app.read(self)
    }

    /// Mutably borrows the entity's state, with a [`Context`] for it.
    pub fn update<R>(&self, app: &mut App, f: impl FnOnce(&mut T, &mut Context<T>) -> R) -> R {
        app.update(self, f)
    }

    /// Marks this entity for re-render (and wakes its observers).
    pub fn notify(&self, app: &mut App) {
        app.notify_entity(self.id);
    }
}

impl<T: Render> Entity<T> {
    /// Erases the type, to use the entity as a child or the root.
    pub fn into_any(self) -> AnyEntity {
        AnyEntity {
            id: self.id,
            render: render_entity::<T>,
        }
    }
}

/// A type-erased renderable entity.
#[derive(Clone, Copy)]
pub struct AnyEntity {
    id: EntityId,
    render: fn(&mut App, EntityId) -> Element,
}

impl AnyEntity {
    /// The entity's id.
    pub fn id(&self) -> EntityId {
        self.id
    }

    pub(crate) fn render(&self, app: &mut App) -> Element {
        (self.render)(app, self.id)
    }
}

fn render_entity<T: Render>(app: &mut App, id: EntityId) -> Element {
    let entity = Entity::<T> {
        id,
        _marker: PhantomData,
    };
    app.update(&entity, |this, cx| this.render(cx).into_element())
}

type Observer = Rc<dyn Fn(&mut App)>;

/// Light/dark appearance of native chrome (bars, native controls, scrollbars).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Appearance {
    /// Follow the system setting.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// What the platform tells the app about the user's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    /// System language and region.
    pub locale: crate::i18n::Locale,
    /// Text size multiplier (iOS Dynamic Type; 1.0 = default size).
    pub text_scale: f32,
    /// The user asked for less motion: animations are skipped.
    pub reduce_motion: bool,
    /// Seconds east of UTC (for formatting dates without a platform formatter).
    pub utc_offset: i32,
}

impl Default for Environment {
    fn default() -> Self {
        Environment {
            locale: Default::default(),
            text_scale: 1.,
            reduce_motion: false,
            utc_offset: 0,
        }
    }
}

/// Owns every entity and tracks which ones must be re-rendered.
pub struct App {
    entities: HashMap<EntityId, Option<Box<dyn Any>>>,
    next_entity: u64,
    dirty: HashSet<EntityId>,
    /// Notifications not yet delivered to observers.
    pending: Vec<EntityId>,
    observers: HashMap<EntityId, Vec<Observer>>,
    root: Option<AnyEntity>,
    initial_path: String,
    navigator: Option<Entity<Router>>,
    nav_queue: Vec<NavOp>,
    globals: HashMap<TypeId, Box<dyn Any>>,
    /// The page request's status, set by the app (`cx.set_page_status`).
    pub(crate) page_status: Option<u16>,
    appearance: Appearance,
    environment: Environment,
    pub(crate) locale_override: Option<crate::i18n::Locale>,
    formatter: Option<Box<dyn crate::i18n::Formatter>>,
    pub(crate) surfaces: crate::surfaces::Surfaces,
    pub(crate) fonts: crate::fonts::Fonts,
    pub(crate) car: crate::car::CarState,
    /// Set by `with_animation` for the next render.
    pub(crate) animation: Option<crate::style::Transition>,
    pub(crate) executor: Rc<crate::task::Executor>,
    pub(crate) menus: crate::menu::Menus,
    pub(crate) background: crate::background::Background,
    pub(crate) device: crate::device::Device,
    pub(crate) push: crate::push::Push,
    pub(crate) device_info: crate::device_info::DeviceInfoState,
    storage: Box<dyn crate::storage::Storage>,
    pub(crate) cloud: crate::storage::Cloud,
    pub(crate) window: crate::window::Window,
    // Focus, scrolling, layers, key propagation (#17, #25).
    pub(crate) ui_commands: Vec<UiCommand>,
    pub(crate) layers: Vec<AnyEntity>,
    pub(crate) stop_propagation: bool,
    /// The title the app set (`cx.set_title`, #154).
    title: Option<String>,
    /// The app has unsaved changes (`cx.set_unsaved_changes`, #158).
    unsaved: bool,
    // --- Releasing what an entity made (#31) ---
    /// Entities created with `cx.new` in an entity's context, by creator:
    /// released with it.
    children: HashMap<EntityId, Vec<EntityId>>,
    /// Observers an entity registered (`cx.observe`), by that entity:
    /// `(observed, observer)`, dropped when it is released.
    observing: HashMap<EntityId, Vec<(EntityId, Observer)>>,
}

/// A request for the UI, applied after the next render.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UiCommand {
    Focus(String),
    ScrollIntoView(String),
    /// `dismiss_keyboard` (#29).
    DismissKeyboard,
    /// `post_web_message` (#62): the web view's name, the message as JSON.
    PostWebMessage(String, String),
    /// `reload_web_view` (#62).
    ReloadWebView(String),
}

impl Default for App {
    fn default() -> Self {
        App {
            entities: HashMap::new(),
            next_entity: 0,
            dirty: HashSet::new(),
            pending: Vec::new(),
            observers: HashMap::new(),
            root: None,
            initial_path: "/".into(),
            navigator: None,
            nav_queue: Vec::new(),
            globals: HashMap::new(),
            page_status: None,
            appearance: Appearance::System,
            environment: Environment::default(),
            locale_override: None,
            formatter: None,
            surfaces: Default::default(),
            fonts: Default::default(),
            car: Default::default(),
            animation: None,
            executor: Default::default(),
            menus: Default::default(),
            background: Default::default(),
            device: Default::default(),
            push: Default::default(),
            storage: Box::new(crate::storage::MemoryStorage::default()),
            cloud: Default::default(),
            window: Default::default(),
            device_info: Default::default(),
            ui_commands: Vec::new(),
            layers: Vec::new(),
            stop_propagation: false,
            title: None,
            unsaved: false,
            children: HashMap::new(),
            observing: HashMap::new(),
        }
    }
}

impl App {
    /// An empty app.
    pub fn new() -> Self {
        Self::default()
    }

    /// Persistent key-value storage (see [`Storage`](crate::storage::Storage)).
    pub fn storage(&mut self) -> &mut dyn crate::storage::Storage {
        &mut *self.storage
    }

    /// Replaces the storage (backends install the platform's).
    pub fn set_storage(&mut self, storage: Box<dyn crate::storage::Storage>) {
        self.storage = storage;
    }

    /// Runs `f` whenever `entity` is notified (app-level observer: trays,
    /// app menus, anything outside a view).
    pub fn on_notify<T: 'static>(&mut self, entity: &Entity<T>, f: impl Fn(&mut App) + 'static) {
        self.observers
            .entry(entity.id)
            .or_default()
            .push(Rc::new(f));
    }

    /// Re-renders widgets whenever `entity` is notified.
    pub fn reload_widgets_on<T: 'static>(&mut self, entity: &Entity<T>) {
        let observer: Observer = Rc::new(|app: &mut App| app.reload_widgets());
        self.observers.entry(entity.id).or_default().push(observer);
    }

    /// Forces light or dark native chrome (or follows the system).
    pub fn set_appearance(&mut self, appearance: Appearance) {
        if self.appearance != appearance {
            self.appearance = appearance;
            self.refresh();
        }
    }

    /// The appearance set with [`App::set_appearance`].
    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    /// Locale, text size and motion settings of the platform.
    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    /// Called by backends at startup and when a system setting changes.
    pub fn set_environment(&mut self, environment: Environment) {
        if self.environment != environment {
            self.environment = environment;
            self.refresh();
        }
    }

    /// Dynamic Type multiplier (native text is scaled automatically).
    pub fn text_scale(&self) -> f32 {
        self.environment.text_scale
    }

    /// The user asked for less motion: animations are skipped.
    pub fn reduce_motion(&self) -> bool {
        self.environment.reduce_motion
    }

    /// Platform number/date formatting (Foundation on Apple platforms).
    pub fn set_formatter(&mut self, formatter: Box<dyn crate::i18n::Formatter>) {
        self.formatter = Some(formatter);
    }

    pub(crate) fn formatter(&self) -> Box<dyn crate::i18n::Formatter + '_> {
        match &self.formatter {
            Some(f) => Box::new(FormatterRef(&**f)),
            None => Box::new(crate::i18n::BasicFormatter {
                utc_offset: self.environment.utc_offset,
            }),
        }
    }

    // --- Globals: app-wide singletons such as a theme ---

    /// Stores an app-wide value (one per type) and re-renders everything.
    pub fn set_global<T: 'static>(&mut self, value: T) {
        self.globals.insert(TypeId::of::<T>(), Box::new(value));
        self.refresh();
    }

    /// Reads a global, or `None` if it was never set.
    pub fn try_global<T: 'static>(&self) -> Option<&T> {
        self.globals.get(&TypeId::of::<T>())?.downcast_ref()
    }

    /// Reads a global. Panics if it was never set; see [`App::try_global`].
    pub fn global<T: 'static>(&self) -> &T {
        self.try_global()
            .unwrap_or_else(|| panic!("no global of type {}", std::any::type_name::<T>()))
    }

    /// Mutates a global (inserting its default first) and re-renders everything.
    /// For bookkeeping that no view shows, use [`App::update_global_quiet`].
    pub fn update_global<T: Default + 'static, R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        let slot = self
            .globals
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()));
        let r = f(slot.downcast_mut().expect("global type"));
        self.refresh();
        r
    }

    /// Mutates a global (inserting its default first) **without**
    /// re-rendering anything: for bookkeeping no view reads while
    /// rendering (a debounce table, a cache, request counters). Call
    /// [`App::refresh`] or notify an entity yourself if a view must change.
    pub fn update_global_quiet<T: Default + 'static, R>(
        &mut self,
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        f(self.global_mut())
    }

    /// The global of type `T` (inserting its default first), mutably,
    /// without re-rendering anything; see [`App::update_global_quiet`].
    pub fn global_mut<T: Default + 'static>(&mut self) -> &mut T {
        self.globals
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut()
            .expect("global type")
    }

    // --- Navigation (applied after the current event, by the active Router) ---

    /// Pushes the screen for `path` (no-op if it is already on top).
    pub fn navigate(&mut self, path: impl Into<String>) {
        self.nav_queue.push(NavOp::Push(path.into()));
    }

    /// Pops the top screen.
    pub fn back(&mut self) {
        self.nav_queue.push(NavOp::Back);
    }

    /// Presents `path` as a sheet (modal) over the current screen.
    pub fn present(&mut self, path: impl Into<String>) {
        self.nav_queue.push(NavOp::Present(path.into()));
    }

    /// Closes the presented sheet.
    pub fn dismiss(&mut self) {
        self.nav_queue.push(NavOp::Dismiss);
    }

    /// Replaces the top screen.
    pub fn replace(&mut self, path: impl Into<String>) {
        self.nav_queue.push(NavOp::Replace(path.into()));
    }

    /// Replaces the whole stack (e.g. after login).
    pub fn reset(&mut self, path: impl Into<String>) {
        self.nav_queue.push(NavOp::Reset(path.into()));
    }

    /// Pushes the screen for `path`, handing it `state`: its route reads it
    /// with [`Params::state`](crate::Params::state) (a "Saved" notice, a
    /// draft, an object already loaded). Only that screen gets it; a reload
    /// or a deep link opens the path without it. Dropped if no screen is
    /// built (`path` is already on top, or a tab's root).
    pub fn navigate_with(&mut self, path: impl Into<String>, state: impl std::any::Any) {
        self.nav_queue.push(NavOp::State(Rc::new(state)));
        self.nav_queue.push(NavOp::Push(path.into()));
    }

    /// Replaces the top screen, handing `state` to the new one (a form
    /// that saved, then shows the saved item with a notice); see
    /// [`App::navigate_with`].
    pub fn replace_with(&mut self, path: impl Into<String>, state: impl std::any::Any) {
        self.nav_queue.push(NavOp::State(Rc::new(state)));
        self.nav_queue.push(NavOp::Replace(path.into()));
    }

    /// Rewrites the query string of the visible screen without pushing
    /// one (the web replaces the history entry; native apps keep it for
    /// `prism dev` restarts): `cx.replace_query([("period", "30d")])`. The
    /// screen keeps its state; an empty list clears the query. To make Back
    /// undo a change, [`navigate`](App::navigate) to the new path instead.
    pub fn replace_query<K: AsRef<str>, V: AsRef<str>>(
        &mut self,
        pairs: impl IntoIterator<Item = (K, V)>,
    ) {
        let query = crate::router::encode_query(pairs);
        self.nav_queue.push(NavOp::ReplaceQuery(query));
    }

    /// The query parameters of the visible screen's path, decoded.
    pub fn current_query(&self) -> Vec<(String, String)> {
        let path = self.current_path().unwrap_or("/");
        crate::router::parse_query(crate::router::split_query(path).1)
    }

    /// Path of the visible screen, if a router is active.
    /// `None` once the router is released (`cx.release(router)`).
    pub fn current_path(&self) -> Option<&str> {
        self.navigator
            .and_then(|r| self.try_read(&r))
            .map(Router::current_path)
    }

    /// Whether the app's router shows its not-found screen (no route matches
    /// the visible path): the web answers the page with `404`. `false`
    /// without a router.
    pub fn shows_not_found(&self) -> bool {
        self.navigator
            .and_then(|r| self.try_read(&r))
            .is_some_and(Router::shows_not_found)
    }

    /// The path the app was opened with (request URL on the web).
    pub fn initial_path(&self) -> &str {
        &self.initial_path
    }

    /// Sets the path the app opens on (normalized).
    pub fn set_initial_path(&mut self, path: impl AsRef<str>) {
        self.initial_path = crate::router::normalize_path(path.as_ref());
    }

    pub(crate) fn set_navigator(&mut self, router: Entity<Router>) {
        self.navigator = Some(router);
    }

    /// Creates a new entity. `build` receives a context for the entity being built.
    pub fn new_entity<T: 'static>(
        &mut self,
        build: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T> {
        self.next_entity += 1;
        let entity = Entity {
            id: EntityId(self.next_entity),
            _marker: PhantomData,
        };
        self.entities.insert(entity.id, None);
        let value = build(&mut Context { app: self, entity });
        self.entities.insert(entity.id, Some(Box::new(value)));
        entity
    }

    /// Borrows an entity's state. Panics if it is released or being updated.
    pub fn read<T: 'static>(&self, entity: &Entity<T>) -> &T {
        self.entities
            .get(&entity.id)
            .and_then(|slot| slot.as_ref())
            .and_then(|b| b.downcast_ref::<T>())
            .expect("entity is released or currently being updated")
    }

    /// Mutably borrows an entity. Re-entrant updates of the same entity panic.
    pub fn update<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        f: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        let mut boxed = self
            .entities
            .get_mut(&entity.id)
            .and_then(Option::take)
            .expect("entity is released or already being updated");
        let value = boxed.downcast_mut::<T>().expect("entity type mismatch");
        let result = f(
            value,
            &mut Context {
                app: self,
                entity: *entity,
            },
        );
        // Released while it was being updated (a Sign out button that
        // releases its own screen): it stays released.
        if let Some(slot) = self.entities.get_mut(&entity.id) {
            *slot = Some(boxed);
        }
        result
    }

    /// Whether the entity `id` exists.
    pub fn contains(&self, id: EntityId) -> bool {
        self.entities.contains_key(&id)
    }

    /// Drops an entity's state and its observers, and cancels the tasks
    /// tied to it ([`TaskHandle::cancel_on_release`](crate::TaskHandle::cancel_on_release)).
    /// The entities it created with [`cx.new`](Context::new) and the
    /// observers it registered ([`cx.observe`](Context::observe)) go with
    /// it. Releasing a [`Router`] releases every screen it holds (see
    /// [`Router::close`]); releasing the app's router also clears the
    /// navigator: [`current_path`](App::current_path) is then `None`.
    pub fn release<T>(&mut self, entity: Entity<T>) {
        self.release_id(entity.id);
    }

    pub(crate) fn release_any(&mut self, entity: AnyEntity) {
        self.release_id(entity.id());
    }

    /// Releases `id`, what it created and, for a router, its screens.
    fn release_id(&mut self, id: EntityId) {
        let mut queue = vec![id];
        while let Some(id) = queue.pop() {
            if let Some(router) = self
                .entities
                .get_mut(&id)
                .and_then(Option::as_mut)
                .and_then(|b| b.downcast_mut::<Router>())
            {
                queue.extend(router.take_screens().into_iter().map(|e| e.id()));
            }
            if self.navigator.is_some_and(|n| n.id == id) {
                self.navigator = None;
            }
            let existed = self.entities.remove(&id).is_some();
            self.observers.remove(&id);
            self.dirty.remove(&id);
            self.executor.release_owner(id);
            for (observed, observer) in self.observing.remove(&id).unwrap_or_default() {
                if let Some(list) = self.observers.get_mut(&observed) {
                    list.retain(|o| !Rc::ptr_eq(o, &observer));
                }
            }
            self.layers.retain(|l| l.id() != id);
            if existed {
                queue.extend(self.children.remove(&id).unwrap_or_default());
            }
        }
    }

    /// The number of live entities: a test checks that closing a screen,
    /// a router or a session leaves none behind.
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// An entity's state, or `None` if it is released or being updated.
    pub(crate) fn try_read<T: 'static>(&self, entity: &Entity<T>) -> Option<&T> {
        self.entities
            .get(&entity.id)
            .and_then(|slot| slot.as_ref())
            .and_then(|b| b.downcast_ref::<T>())
    }

    /// The id the next entity will get is above this one.
    pub(crate) fn last_entity_id(&self) -> EntityId {
        EntityId(self.next_entity)
    }

    /// Marks one entity for re-render and queues its observers.
    pub fn notify_entity(&mut self, id: EntityId) {
        self.dirty.insert(id);
        self.pending.push(id);
    }

    /// Re-renders every entity on the next frame.
    pub fn refresh(&mut self) {
        let ids: Vec<EntityId> = self.entities.keys().copied().collect();
        self.dirty.extend(ids);
    }

    /// Whether some entity waits to be re-rendered (or a focus / scroll
    /// request waits for the next render).
    pub fn has_dirty(&self) -> bool {
        !self.dirty.is_empty() || !self.ui_commands.is_empty()
    }

    // --- Focus, scrolling, layers (#17, #25) ---

    /// Gives the keyboard focus to the element named `id` (`.id(id)`)
    /// after the next render: a text field, a view with `on_key_down`, a
    /// button. Does nothing if no such element is mounted.
    pub fn focus(&mut self, id: impl Into<String>) {
        self.ui_commands.push(UiCommand::Focus(id.into()));
    }

    /// Takes the keyboard focus away from the focused text field (or other
    /// focused element) and hides the on-screen keyboard, after the next
    /// render: before swapping the root (`set_root`) away from a form, after
    /// a submit. Does nothing if nothing has the focus.
    pub fn dismiss_keyboard(&mut self) {
        self.ui_commands.push(UiCommand::DismissKeyboard);
    }

    /// Scrolls the scroll views around the element named `id` (`.id(id)`)
    /// so it is visible, after the next render (a link to a form, the row
    /// just added).
    pub fn scroll_into_view(&mut self, id: impl Into<String>) {
        self.ui_commands.push(UiCommand::ScrollIntoView(id.into()));
    }

    // --- Web views (#62) ---

    /// Posts `message` (serialized as JSON) to the page of the web view
    /// named `id` (`.id(id)`, see [`web_view`](crate::web_view())), after
    /// the next render: a `message` event on the page's `window`. Delivered
    /// once the page has loaded, and only while it shows an allowed origin.
    /// Does nothing if no such web view is mounted or the value cannot be
    /// serialized.
    pub fn post_web_message<T: serde::Serialize + ?Sized>(
        &mut self,
        id: impl Into<String>,
        message: &T,
    ) {
        if let Ok(json) = serde_json::to_string(message) {
            self.ui_commands
                .push(UiCommand::PostWebMessage(id.into(), json));
        }
    }

    /// Like [`post_web_message`](App::post_web_message), with a message
    /// already encoded as JSON. Ignored if `json` is not valid JSON.
    pub fn post_web_message_json(&mut self, id: impl Into<String>, json: &str) {
        // Re-encoded: backends embed it in a script as it is.
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
            self.ui_commands
                .push(UiCommand::PostWebMessage(id.into(), value.to_string()));
        }
    }

    /// Reloads the page of the web view named `id` after the next render
    /// (on the web, a frame showing another origin than the app's goes back
    /// to the URL the app gave: it cannot be reloaded in place).
    pub fn reload_web_view(&mut self, id: impl Into<String>) {
        self.ui_commands.push(UiCommand::ReloadWebView(id.into()));
    }

    /// Mounts `entity` above the whole UI (router, tabs, sheets and
    /// dialogs), covering the window: it lets pointer input through
    /// wherever it has no child. For app-wide feedback such as
    /// `prism_ui::show_toast`. Adding it again does nothing.
    pub fn add_layer<T: Render>(&mut self, entity: Entity<T>) {
        if !self.layers.iter().any(|l| l.id() == entity.id) {
            self.layers.push(entity.into_any());
            self.dirty.insert(entity.id);
        }
    }

    /// Unmounts a layer added with [`add_layer`](App::add_layer).
    pub fn remove_layer<T>(&mut self, entity: &Entity<T>) {
        let before = self.layers.len();
        self.layers.retain(|l| l.id() != entity.id);
        if self.layers.len() != before
            && let Some(root) = self.root
        {
            self.dirty.insert(root.id());
        }
    }

    /// Whether `entity` is mounted as a layer.
    pub fn has_layer<T>(&self, entity: &Entity<T>) -> bool {
        self.layers.iter().any(|l| l.id() == entity.id)
    }

    /// Called from a key handler: the key press reaches no further handler
    /// (an overlay's Escape does not also close the screen under it).
    pub fn stop_propagation(&mut self) {
        self.stop_propagation = true;
    }

    /// Makes `entity` the root of the UI and marks it for rendering.
    pub fn set_root<T: Render>(&mut self, entity: Entity<T>) {
        self.root = Some(entity.into_any());
        self.dirty.insert(entity.id);
    }

    /// Delivers queued notifications to observers. Each entity's observers run
    /// at most once per flush, so observer cycles terminate.
    pub fn flush_effects(&mut self) {
        if let Some(router) = self.navigator.filter(|r| self.contains(r.id())) {
            let ops = std::mem::take(&mut self.nav_queue);
            let navigated = !ops.is_empty();
            for op in ops {
                router.update(self, |r, cx| r.apply(op, cx));
            }
            // Screens just built may have spawned tasks (a first load):
            // start them now rather than at the next event.
            if navigated && self.executor.has_ready() {
                crate::task::poll_tasks(self);
            }
        } else {
            self.nav_queue.clear();
        }
        let mut delivered = HashSet::new();
        while let Some(id) = self.pending.pop() {
            if !delivered.insert(id) {
                continue;
            }
            if let Some(observers) = self.observers.get(&id).cloned() {
                for observer in observers {
                    observer(self);
                }
            }
        }
    }

    pub(crate) fn take_dirty(&mut self) -> HashSet<EntityId> {
        std::mem::take(&mut self.dirty)
    }

    pub(crate) fn root(&self) -> Option<AnyEntity> {
        self.root
    }
}

/// The context handed to `render`, `new_entity` builders and listeners.
/// Dereferences to [`App`].
pub struct Context<'a, T> {
    pub(crate) app: &'a mut App,
    entity: Entity<T>,
}

impl<T: 'static> Context<'_, T> {
    /// The entity this context belongs to.
    pub fn entity(&self) -> Entity<T> {
        self.entity
    }

    /// Creates a child entity (GPUI's `cx.new`): it is released with this
    /// one (and its own children with it). For an entity that outlives
    /// this one, use [`App::new_entity`].
    #[allow(clippy::new_ret_no_self)]
    pub fn new<U: 'static>(&mut self, build: impl FnOnce(&mut Context<U>) -> U) -> Entity<U> {
        let child = self.app.new_entity(build);
        let app = &mut *self.app;
        let list = app.children.entry(self.entity.id).or_default();
        list.push(child.id);
        // Children released on their own: forgotten now and then.
        if list.len() >= 64 && list.len().is_power_of_two() {
            list.retain(|c| app.entities.contains_key(c));
        }
        child
    }

    /// Re-renders this entity on the next frame. Listeners call it for you.
    pub fn notify(&mut self) {
        self.app.notify_entity(self.entity.id);
    }

    /// Wraps a closure into an event handler bound to this entity.
    /// The entity is re-rendered after the handler runs.
    ///
    /// ```ignore
    /// button("+").on_click(cx.listener(|this, _, _| this.count += 1))
    /// ```
    pub fn listener<E: 'static>(
        &self,
        f: impl Fn(&mut T, &E, &mut Context<T>) + 'static,
    ) -> Handler<E> {
        let entity = self.entity;
        Rc::new(move |event: &E, app: &mut App| {
            if app.contains(entity.id) {
                app.update(&entity, |this, cx| {
                    f(this, event, cx);
                    cx.notify();
                });
            }
        })
    }

    /// Runs `f` whenever `other` is notified, then re-renders this entity.
    /// Use it for derived state (totals, filters) that depends on another entity.
    pub fn observe<U: 'static>(
        &mut self,
        other: &Entity<U>,
        f: impl Fn(&mut T, Entity<U>, &mut Context<T>) + 'static,
    ) {
        let this = self.entity;
        let other = *other;
        let observer: Observer = Rc::new(move |app: &mut App| {
            if app.contains(this.id) {
                app.update(&this, |t, cx| {
                    f(t, other, cx);
                    cx.notify();
                });
            }
        });
        self.app
            .observers
            .entry(other.id)
            .or_default()
            .push(observer.clone());
        self.app
            .observing
            .entry(this.id)
            .or_default()
            .push((other.id, observer));
    }
}

impl<T> Deref for Context<'_, T> {
    type Target = App;
    fn deref(&self) -> &App {
        self.app
    }
}

impl<T> DerefMut for Context<'_, T> {
    fn deref_mut(&mut self) -> &mut App {
        self.app
    }
}

struct FormatterRef<'a>(&'a dyn crate::i18n::Formatter);

impl crate::i18n::Formatter for FormatterRef<'_> {
    fn number(&self, l: &crate::i18n::Locale, v: f64, d: usize) -> String {
        self.0.number(l, v, d)
    }
    fn date(&self, l: &crate::i18n::Locale, u: f64, s: crate::i18n::DateStyle) -> String {
        self.0.date(l, u, s)
    }
    fn time(&self, l: &crate::i18n::Locale, u: f64) -> String {
        self.0.time(l, u)
    }
}

// --- The app's title (#154) -------------------------------------------------------

impl App {
    /// Titles the page or the window `title` (#154), for an app that does
    /// its own navigation (a sidebar, a document's name): the web shows
    /// « title · App » in the tab, the first paint included; macOS and Linux
    /// title the window; iPadOS the scene (its multitasking switcher);
    /// Android the activity. While set it wins over the title of a
    /// [`Stack`](crate::Stack)'s top screen. Setting the same title again
    /// does nothing. Empty: like [`clear_title`](Self::clear_title).
    pub fn set_title(&mut self, title: impl Into<String>) {
        let title = Some(title.into()).filter(|t| !t.trim().is_empty());
        if self.title != title {
            self.title = title.clone();
            self.surfaces
                .requests
                .push(crate::PlatformRequest::SetTitle(title));
        }
    }

    /// Back to the default title: the top screen's, else the app's.
    pub fn clear_title(&mut self) {
        if self.title.take().is_some() {
            self.surfaces
                .requests
                .push(crate::PlatformRequest::SetTitle(None));
        }
    }

    /// The title set with [`set_title`](Self::set_title), if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
}

// --- Unsaved changes (#158) ---------------------------------------------------------

impl App {
    /// Whether the app has changes the user would lose by leaving (#158):
    /// while `true`, closing the page or the window asks first. Web: the
    /// browser's own « Leave site? » question (`beforeunload`); macOS: the
    /// close button's dot, and closing the window or quitting asks « Leave
    /// without saving? »; Linux: closing the window asks. Phones and
    /// tablets close apps themselves: save as you go there. Navigation
    /// inside the app is the app's to guard.
    pub fn set_unsaved_changes(&mut self, unsaved: bool) {
        if self.unsaved != unsaved {
            self.unsaved = unsaved;
            self.surfaces
                .requests
                .push(crate::PlatformRequest::SetUnsavedChanges(unsaved));
        }
    }

    /// See [`set_unsaved_changes`](Self::set_unsaved_changes).
    pub fn has_unsaved_changes(&self) -> bool {
        self.unsaved
    }
}
