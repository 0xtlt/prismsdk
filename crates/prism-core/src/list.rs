//! Virtualized lists: only the rows near the viewport exist, so 100 000 rows
//! scroll as smoothly as 10 (like React Native's `FlatList`).
//!
//! ```ignore
//! let list = VirtualList::new(cx, 10_000, 56., |i, _cx| text(format!("Row {i}")));
//! div().child(list)                        // in render
//! list.update(cx, |l, cx| l.set_count(n, cx));   // when data changes
//! ```

use std::rc::Rc;

use crate::app::{App, Context, Entity, Render};
use crate::element::{Element, IntoElement, ParentElement, ScrollEvent, div, scroll};
use crate::style::{Length, Styled};

type RowFn = Rc<dyn Fn(usize, &mut App) -> Element>;

/// A scrolling list of `count` rows of a fixed height.
pub struct VirtualList {
    count: usize,
    row_height: f32,
    offset: f32,
    viewport: f32,
    /// Extra rows rendered above and below the viewport.
    overscan: usize,
    render_row: RowFn,
}

impl VirtualList {
    /// A list of `count` rows of `row_height` points; `render_row` builds row `i` when it scrolls into view.
    pub fn new<E: IntoElement>(
        cx: &mut App,
        count: usize,
        row_height: f32,
        render_row: impl Fn(usize, &mut App) -> E + 'static,
    ) -> Entity<VirtualList> {
        cx.new_entity(|_| VirtualList {
            count,
            row_height,
            offset: 0.,
            // Until the platform reports it: enough for a tall screen.
            viewport: 1000.,
            overscan: 6,
            render_row: Rc::new(move |i, app| render_row(i, app).into_element()),
        })
    }

    /// Changes the number of rows and re-renders.
    pub fn set_count(&mut self, count: usize, cx: &mut Context<Self>) {
        self.count = count;
        cx.notify();
    }

    /// Number of rows.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The rows currently rendered.
    pub fn visible_range(&self) -> std::ops::Range<usize> {
        let first = (self.offset / self.row_height).floor().max(0.) as usize;
        let shown = (self.viewport / self.row_height).ceil() as usize + 1;
        let start = first.saturating_sub(self.overscan).min(self.count);
        let end = (first + shown + self.overscan).min(self.count);
        start..end
    }

    fn scrolled(&mut self, e: &ScrollEvent, cx: &mut Context<Self>) {
        let before = self.visible_range();
        self.offset = e.offset;
        self.viewport = e.viewport.max(1.);
        // Only re-render when the window of rows actually changes.
        if self.visible_range() != before {
            cx.notify();
        }
    }
}

impl Render for VirtualList {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let range = self.visible_range();
        let h = self.row_height;
        let render_row = self.render_row.clone();
        let rows: Vec<Element> = range
            .clone()
            .map(|i| {
                div()
                    .key(i.to_string())
                    .h(Length::Px(h))
                    .flex_shrink_0()
                    .child(render_row(i, cx))
                    .into_element()
            })
            .collect();
        let entity = cx.entity();
        scroll()
            .on_scroll(Rc::new(move |e: &ScrollEvent, app: &mut App| {
                // Not a listener: re-render only when the window changes.
                entity.update(app, |l, cx| l.scrolled(e, cx));
            }))
            .child(div().h(Length::Px(range.start as f32 * h)).flex_shrink_0())
            .children(rows)
            .child(
                div()
                    .h(Length::Px((self.count - range.end) as f32 * h))
                    .flex_shrink_0(),
            )
    }
}
