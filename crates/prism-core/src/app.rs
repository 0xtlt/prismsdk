//! State model: GPUI-style entities owned by an [`App`].

use std::any::Any;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use crate::element::{Element, Handler, IntoElement};

/// A stateful view. `render` is called whenever the app is notified.
pub trait Render: 'static + Sized {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn read<'a>(&self, app: &'a App) -> &'a T {
        app.read(self)
    }

    pub fn update<R>(&self, app: &mut App, f: impl FnOnce(&mut T, &mut Context<T>) -> R) -> R {
        app.update(self, f)
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

/// Owns every entity and tracks whether the UI must be re-rendered.
#[derive(Default)]
pub struct App {
    entities: HashMap<EntityId, Option<Box<dyn Any>>>,
    next_entity: u64,
    dirty: bool,
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

    /// Drops an entity's state.
    pub fn release<T>(&mut self, entity: Entity<T>) {
        self.entities.remove(&entity.id);
    }

    /// Schedules a re-render after the current event.
    pub fn notify(&mut self) {
        self.dirty = true;
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_root<T: Render>(&mut self, entity: Entity<T>) {
        self.root = Some(entity.into_any());
        self.dirty = true;
    }

    pub(crate) fn take_dirty(&mut self) -> bool {
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

    /// Wraps a closure into an event handler bound to this entity.
    ///
    /// ```ignore
    /// button("+").on_click(cx.listener(|this, _, cx| { this.count += 1; cx.notify(); }))
    /// ```
    pub fn listener<E: 'static>(
        &self,
        f: impl Fn(&mut T, &E, &mut Context<T>) + 'static,
    ) -> Handler<E> {
        let entity = self.entity;
        Rc::new(move |event: &E, app: &mut App| {
            app.update(&entity, |this, cx| f(this, event, cx));
        })
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
