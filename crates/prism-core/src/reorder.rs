//! Lists whose rows the user reorders (#168): drag a row by its handle, or
//! focus the handle and press ↑ / ↓. Rows have a fixed height, as in
//! [`VirtualList`](crate::VirtualList), or follow their content and are
//! measured (#201); the handle is three bars at the trailing edge, or the
//! app's own at either edge ([`ReorderOptions`]).
//!
//! ```ignore
//! let tasks = ReorderList::new(cx, keys, 52., render_row, move |from, to, app| {
//!     store.update(app, |s, cx| { let t = s.tasks.remove(from); s.tasks.insert(to, t); cx.notify(); });
//! });
//! div().child(tasks)                                  // in render
//! tasks.update(cx, |l, cx| l.set_keys(new_keys, cx)); // when the rows change
//! ```

use std::cell::Cell;
use std::rc::Rc;

use crate::a11y::{Accessible, Role};
use crate::app::{App, Context, Entity, Render};
use crate::decoration::{Cursor, Shadow};
use crate::element::{Element, FluentBuilder, IntoElement, ParentElement, div};
use crate::events::{DragEvent, DragPhase, KeyEvent, LayoutEvent};
use crate::style::{Length, Styled, rgba};

type RowFn = Rc<dyn Fn(usize, &mut App) -> Element>;
type MoveFn = Rc<dyn Fn(usize, usize, &mut App)>;
type HandleFn = Rc<dyn Fn(usize, &mut App) -> Element>;

/// How a [`ReorderList`] lays its rows and handles out (#201).
#[derive(Clone, Default)]
pub struct ReorderOptions {
    /// Every row's height in points; `None`: each row is as tall as its
    /// content, measured after layout ([`on_layout`](crate::Div::on_layout)).
    pub row_height: Option<f32>,
    /// The handle at the leading edge (`false`: trailing, the default).
    pub handle_leading: bool,
    /// The handle's content for row `i` (`None`: three grey bars, 44 points
    /// wide). Prism keeps its role, label, keys and drag around it.
    pub handle: Option<HandleFn>,
}

impl ReorderOptions {
    /// Rows of `height` points each.
    pub fn row_height(mut self, height: f32) -> Self {
        self.row_height = Some(height.max(1.));
        self
    }

    /// Rows as tall as their content (the default), measured after layout.
    pub fn content_height(mut self) -> Self {
        self.row_height = None;
        self
    }

    /// The handle at the leading edge of each row.
    pub fn handle_leading(mut self) -> Self {
        self.handle_leading = true;
        self
    }

    /// The app's own handle for row `i` (a grip icon in its style).
    pub fn handle<E: IntoElement>(
        mut self,
        handle: impl Fn(usize, &mut App) -> E + 'static,
    ) -> Self {
        self.handle = Some(Rc::new(move |i, app| handle(i, app).into_element()));
        self
    }
}

thread_local! {
    /// Numbers the lists, so their handles' ids are unique.
    static LISTS: Cell<u64> = const { Cell::new(0) };
}

/// A column of rows of a fixed height that the user puts in another order
/// (#168). Each row gets a handle at its trailing edge: dragging it moves
/// the row (the others make room), and with the focus on it ↑ and ↓ move
/// the row by one. The rest of the row is the app's (clicks, scrolling).
/// On release the list takes its new order and tells the app `(from, to)`,
/// so it reorders its data the same way.
///
/// Built on Prism's drag and key events, it works wherever they do: the web,
/// macOS, iOS, iPadOS, visionOS, Android, Linux, and Windows by drag only
/// (its views with keys take no focus yet). tvOS and watchOS have no drag;
/// the keys still move rows on tvOS. Screen readers hear the handle
/// as a button labelled "Reorder", with the row's position and a hint, all
/// in the app's language.
pub struct ReorderList {
    /// The rows' keys, in order: a row keeps its native view (and focus)
    /// when it moves.
    keys: Vec<String>,
    options: ReorderOptions,
    /// Measured heights by key, for rows that follow their content (#201).
    heights: std::collections::HashMap<String, f32>,
    /// The row being dragged and how far it moved.
    drag: Option<(usize, f32)>,
    render_row: RowFn,
    on_move: MoveFn,
    list: u64,
}

/// A row's height before it was measured.
const UNMEASURED: f32 = 44.;

impl ReorderList {
    /// A list of rows named `keys` (one per row, unique), each
    /// `row_height` points high, built by `render_row` (row `i` in the
    /// current order). `on_move(from, to, app)`: the user moved row `from`
    /// to position `to` (the list already shows the new order).
    pub fn new<K: Into<String>, E: IntoElement>(
        cx: &mut App,
        keys: impl IntoIterator<Item = K>,
        row_height: f32,
        render_row: impl Fn(usize, &mut App) -> E + 'static,
        on_move: impl Fn(usize, usize, &mut App) + 'static,
    ) -> Entity<ReorderList> {
        Self::with_options(
            cx,
            keys,
            ReorderOptions::default().row_height(row_height),
            render_row,
            on_move,
        )
    }

    /// Like [`new`](Self::new), laid out as `options` say (#201): rows of
    /// their content's height, the app's own handle, at either edge.
    ///
    /// ```ignore
    /// ReorderList::with_options(cx, keys,
    ///     ReorderOptions::default()          // rows as tall as their content
    ///         .handle_leading()
    ///         .handle(|_, _| icon("line.3.horizontal").text_color(rgb(0x8e8e93))),
    ///     render_row, on_move)
    /// ```
    pub fn with_options<K: Into<String>, E: IntoElement>(
        cx: &mut App,
        keys: impl IntoIterator<Item = K>,
        options: ReorderOptions,
        render_row: impl Fn(usize, &mut App) -> E + 'static,
        on_move: impl Fn(usize, usize, &mut App) + 'static,
    ) -> Entity<ReorderList> {
        let list = LISTS.with(|n| {
            n.set(n.get() + 1);
            n.get()
        });
        let keys = keys.into_iter().map(Into::into).collect();
        cx.new_entity(|_| ReorderList {
            keys,
            options,
            heights: Default::default(),
            drag: None,
            render_row: Rc::new(move |i, app| render_row(i, app).into_element()),
            on_move: Rc::new(on_move),
            list,
        })
    }

    /// Replaces the rows (added, removed or reordered by the app) and
    /// re-renders.
    pub fn set_keys<K: Into<String>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
        cx: &mut Context<Self>,
    ) {
        self.keys = keys.into_iter().map(Into::into).collect();
        self.drag = None;
        cx.notify();
    }

    /// The rows' keys, in the order shown.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// While a row is dragged: `(from, to)`, the row and where it would go
    /// if released now.
    pub fn dragging(&self) -> Option<(usize, usize)> {
        self.drag.map(|(from, dy)| (from, self.target(from, dy)))
    }

    /// The `id` of row `i`'s handle (for [`App::focus`]).
    pub fn handle_id(&self, i: usize) -> Option<String> {
        self.keys
            .get(i)
            .map(|k| format!("prism-reorder-{}-{k}", self.list))
    }

    /// Row `i`'s height: fixed, measured, or a guess until measured.
    fn height(&self, i: usize) -> f32 {
        match self.options.row_height {
            Some(h) => h,
            None => self
                .keys
                .get(i)
                .and_then(|k| self.heights.get(k))
                .copied()
                .unwrap_or(UNMEASURED),
        }
    }

    /// The top of row `i` in the list.
    fn top(&self, i: usize) -> f32 {
        (0..i).map(|j| self.height(j)).sum()
    }

    /// Where row `from` goes when it moved `dy` points: past every row
    /// whose middle its leading edge passed (its bottom going down, its top
    /// going up), whatever their heights.
    fn target(&self, from: usize, dy: f32) -> usize {
        if self.keys.is_empty() {
            return 0;
        }
        let top = self.top(from) + dy;
        let bottom = top + self.height(from);
        let middle = |j: usize| self.top(j) + self.height(j) / 2.;
        if dy >= 0. {
            from + (from + 1..self.keys.len())
                .take_while(|&j| bottom > middle(j))
                .count()
        } else {
            from - (0..from).rev().take_while(|&j| top < middle(j)).count()
        }
    }

    /// How far row `i` is shifted while `from` is dragged to `to`: by the
    /// dragged row's height, to make room.
    fn shift(&self, i: usize, from: usize, to: usize) -> f32 {
        let h = self.height(from);
        if from < to && i > from && i <= to {
            -h
        } else if to < from && i >= to && i < from {
            h
        } else {
            0.
        }
    }

    /// Moves row `from` to `to` in the list's own order: `Some` when it
    /// moved, for the app to hear.
    fn reorder(&mut self, from: usize, to: usize) -> Option<(usize, usize)> {
        if from == to || from >= self.keys.len() || to >= self.keys.len() {
            return None;
        }
        let key = self.keys.remove(from);
        self.keys.insert(to, key);
        Some((from, to))
    }

    /// A drag of row `i`'s handle.
    fn dragged(&mut self, i: usize, e: &DragEvent) -> Option<(usize, usize)> {
        match e.phase {
            DragPhase::Start | DragPhase::Move => {
                self.drag = Some((i, e.translation.y));
                None
            }
            DragPhase::End => {
                let (from, dy) = self.drag.take().unwrap_or((i, e.translation.y));
                let to = self.target(from, dy);
                self.reorder(from, to)
            }
            DragPhase::Cancel => {
                self.drag = None;
                None
            }
        }
    }

    /// ↑ / ↓ on row `i`'s handle.
    fn key(&mut self, i: usize, e: &KeyEvent) -> Option<(usize, usize)> {
        if self.drag.is_some() {
            return None;
        }
        match e.key.as_str() {
            "ArrowUp" if i > 0 => self.reorder(i, i - 1),
            "ArrowDown" => self.reorder(i, i + 1),
            "Home" => self.reorder(i, 0),
            "End" => self.reorder(i, self.keys.len().saturating_sub(1)),
            _ => None,
        }
    }
}

/// Three short bars: the handle's grip.
fn grip() -> Element {
    let bar = || {
        div()
            .w(Length::Px(16.))
            .h(Length::Px(2.))
            .rounded(1.)
            .bg(rgba(0x8e8e93, 1.))
            .into_element()
    };
    div()
        .flex_col()
        .gap(3.)
        .items_center()
        .justify_center()
        .children([bar(), bar(), bar()])
        .into_element()
}

impl Render for ReorderList {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let fixed = self.options.row_height;
        let leading = self.options.handle_leading;
        let custom = self.options.handle.clone();
        let count = self.keys.len();
        let dragging = self.dragging();
        let label = cx.chrome_text("reorder");
        let hint = cx.chrome_text("reorder_hint");
        let position = cx.chrome_text("reorder_position");
        let entity = cx.entity();
        let render_row = self.render_row.clone();
        let on_move = self.on_move.clone();
        let mut rows: Vec<Element> = Vec::with_capacity(count + 1);
        let entity_for_rows = entity;
        let _ = entity_for_rows;
        for (i, key) in self.keys.clone().into_iter().enumerate() {
            let handle_id = self.handle_id(i).unwrap_or_default();
            let (e_drag, m_drag) = (entity, on_move.clone());
            let (e_key, m_key) = (entity, on_move.clone());
            let focus_id = format!("prism-reorder-{}-", self.list);
            let grip_or_own = match &custom {
                Some(f) => f(i, cx),
                None => grip(),
            };
            let handle = div()
                .id(handle_id)
                .when(custom.is_none(), |d| d.w(Length::Px(44.)))
                .min_w(Length::Px(44.))
                .h_full()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .cursor(if dragging.is_some() {
                    Cursor::Grabbing
                } else {
                    Cursor::Grab
                })
                .child(grip_or_own)
                .accessibility_role(Role::Button)
                .accessibility_label(label.clone())
                .accessibility_value(
                    position
                        .replace("{n}", &(i + 1).to_string())
                        .replace("{count}", &count.to_string()),
                )
                .accessibility_hint(hint.clone())
                .on_drag(Rc::new(move |e: &DragEvent, app: &mut App| {
                    let moved = e_drag.update(app, |l, cx| {
                        let moved = l.dragged(i, e);
                        cx.notify();
                        moved
                    });
                    if let Some((from, to)) = moved {
                        m_drag(from, to, app);
                    }
                }))
                .on_key_down(Rc::new(move |e: &KeyEvent, app: &mut App| {
                    let moved = e_key.update(app, |l, cx| {
                        let moved = l.key(i, e);
                        if moved.is_some() {
                            cx.notify();
                        }
                        moved.map(|m| (m, l.keys[m.1].clone()))
                    });
                    if let Some(((from, to), key)) = moved {
                        m_key(from, to, app);
                        // The moved row's handle keeps the focus (the web
                        // drops it from a moved element).
                        app.focus(format!("{focus_id}{key}"));
                    }
                }));
            let (shift, lifted) = match dragging {
                Some((from, _)) if from == i => (0., true),
                Some((from, to)) => (self.shift(i, from, to), false),
                None => (0., false),
            };
            let content = div()
                .flex_grow(1.)
                .min_w(Length::Px(0.))
                .when(fixed.is_some(), |d| d.h_full())
                .child(render_row(i, cx))
                .into_element();
            let (first, second) = if leading {
                (handle.into_element(), content)
            } else {
                (content, handle.into_element())
            };
            // Rows that follow their content are measured (#201).
            let e_layout = entity;
            let measured_key = key.clone();
            rows.push(
                div()
                    .key(key)
                    .when_some(fixed, |row, h| row.h(Length::Px(h)))
                    .when(fixed.is_none(), |row| {
                        row.on_layout(Rc::new(move |e: &LayoutEvent, app: &mut App| {
                            let k = measured_key.clone();
                            e_layout.update(app, |l, _| {
                                l.heights.insert(k, e.frame.height);
                            });
                        }))
                    })
                    .flex_shrink_0()
                    .flex_row()
                    .items_center()
                    .offset(0., shift)
                    .when(dragging.is_some(), |row| row.animate(150))
                    // The dragged row is drawn above the others, as a lifted
                    // copy; its place stays empty.
                    .opacity(if lifted { 0. } else { 1. })
                    .child(first)
                    .child(second)
                    .into_element(),
            );
        }
        if let Some((from, _)) = dragging
            && let Some((_, dy)) = self.drag
        {
            let h = self.height(from);
            let content = div()
                .flex_grow(1.)
                .min_w(Length::Px(0.))
                .h_full()
                .child(render_row(from, cx))
                .into_element();
            let handle = div()
                .when(custom.is_none(), |d| d.w(Length::Px(44.)))
                .min_w(Length::Px(44.))
                .h_full()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .child(match &custom {
                    Some(f) => f(from, cx),
                    None => grip(),
                })
                .into_element();
            let (first, second) = if leading {
                (handle, content)
            } else {
                (content, handle)
            };
            rows.push(
                div()
                    .key("prism-reorder-lifted")
                    .absolute()
                    .left(Length::Px(0.))
                    .right(Length::Px(0.))
                    .top(Length::Px(self.top(from) + dy))
                    .h(Length::Px(h))
                    .flex_row()
                    .items_center()
                    .hit_test(false)
                    .accessibility_hidden()
                    .shadow(Shadow::new(0., 6., 18., rgba(0x000000, 0.25)))
                    .child(first)
                    .child(second)
                    .into_element(),
            );
        }
        div()
            .flex_col()
            .accessibility_role(Role::List)
            .children(rows)
    }
}
