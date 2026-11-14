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
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn read<'a>(&self, app: &'a App) -> &'a T {
        app.read(self)
    }

    pub fn update<R>(&self, app: &mut App, f: impl FnOnce(&mut T, &mut Context<T>) -> R) -> R {
        app.update(self, f)
    }

    /// Marks this entity for re-render (and wakes its observers).
    pub fn notify(&self, app: &mut App) {
        app.notify_entity(self.id);
    }
}

impl<T: Render> Entity<T> {
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
    Light,
    Dark,
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
    appearance: Appearance,
    pub(crate) surfaces: crate::surfaces::Surfaces,
    pub(crate) executor: Rc<crate::task::Executor>,
    storage: Box<dyn crate::storage::Storage>,
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
            appearance: Appearance::System,
            surfaces: Default::default(),
            executor: Default::default(),
            storage: Box::new(crate::storage::MemoryStorage::default()),
        }
    }
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// Persistent key-value storage (see [`Storage`](crate::storage::Storage)).
    pub fn storage(&mut self) -> &mut dyn crate::storage::Storage {
        &mut *self.storage
    }

    pub fn set_storage(&mut self, storage: Box<dyn crate::storage::Storage>) {
        self.storage = storage;
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

    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    // --- Globals: app-wide singletons such as a theme ---

    /// Stores an app-wide value (one per type) and re-renders everything.
    pub fn set_global<T: 'static>(&mut self, value: T) {
        self.globals.insert(TypeId::of::<T>(), Box::new(value));
        self.refresh();
    }

    pub fn try_global<T: 'static>(&self) -> Option<&T> {
        self.globals.get(&TypeId::of::<T>())?.downcast_ref()
    }

    /// Reads a global. Panics if it was never set; see [`App::try_global`].
    pub fn global<T: 'static>(&self) -> &T {
        self.try_global()
            .unwrap_or_else(|| panic!("no global of type {}", std::any::type_name::<T>()))
    }

    /// Mutates a global (inserting its default first) and re-renders everything.
    pub fn update_global<T: Default + 'static, R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        let slot = self
            .globals
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()));
        let r = f(slot.downcast_mut().expect("global type"));
        self.refresh();
        r
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

    /// Replaces the top screen.
    pub fn replace(&mut self, path: impl Into<String>) {
        self.nav_queue.push(NavOp::Replace(path.into()));
    }

    /// Replaces the whole stack (e.g. after login).
    pub fn reset(&mut self, path: impl Into<String>) {
        self.nav_queue.push(NavOp::Reset(path.into()));
    }

    /// Path of the visible screen, if a router is active.
    pub fn current_path(&self) -> Option<&str> {
        self.navigator.map(|r| self.read(&r).current_path())
    }

    /// The path the app was opened with (request URL on the web).
    pub fn initial_path(&self) -> &str {
        &self.initial_path
    }

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
        self.entities.insert(entity.id, Some(boxed));
        result
    }

    pub fn contains(&self, id: EntityId) -> bool {
        self.entities.contains_key(&id)
    }

    /// Drops an entity's state and its observers.
    pub fn release<T>(&mut self, entity: Entity<T>) {
        self.entities.remove(&entity.id);
        self.observers.remove(&entity.id);
        self.dirty.remove(&entity.id);
    }

    pub(crate) fn release_any(&mut self, entity: AnyEntity) {
        self.entities.remove(&entity.id());
        self.observers.remove(&entity.id());
        self.dirty.remove(&entity.id());
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

    pub fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    pub fn set_root<T: Render>(&mut self, entity: Entity<T>) {
        self.root = Some(entity.into_any());
        self.dirty.insert(entity.id);
    }

    /// Delivers queued notifications to observers. Each entity's observers run
    /// at most once per flush, so observer cycles terminate.
    pub fn flush_effects(&mut self) {
        if let Some(router) = self.navigator.filter(|r| self.contains(r.id())) {
            for op in std::mem::take(&mut self.nav_queue) {
                router.update(self, |r, cx| r.apply(op, cx));
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

    /// Creates a child entity (GPUI's `cx.new`).
    #[allow(clippy::new_ret_no_self)]
    pub fn new<U: 'static>(&mut self, build: impl FnOnce(&mut Context<U>) -> U) -> Entity<U> {
        self.app.new_entity(build)
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
            .push(observer);
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
