//! State model: GPUI-style entities owned by an [`App`].
//!
//! Updates are entity-granular: `cx.notify()` marks only that entity dirty, and
//! the reconciler re-renders just the dirty entities, in place, reusing the
//! previous output of every other entity untouched.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use crate::element::{Element, Handler, IntoElement};

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

/// Owns every entity and tracks which ones must be re-rendered.
#[derive(Default)]
pub struct App {
    entities: HashMap<EntityId, Option<Box<dyn Any>>>,
    next_entity: u64,
    dirty: HashSet<EntityId>,
    /// Notifications not yet delivered to observers.
    pending: Vec<EntityId>,
    observers: HashMap<EntityId, Vec<Observer>>,
    root: Option<AnyEntity>,
}

impl App {
    pub fn new() -> Self {
        Self::default()
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
    pub(crate) fn flush_effects(&mut self) {
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
    app: &'a mut App,
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
