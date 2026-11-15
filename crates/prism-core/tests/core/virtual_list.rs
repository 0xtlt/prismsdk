//! VirtualList windows: which rows exist, spacer heights, row reuse.

use prism_core::prelude::*;
use prism_core::{Length, NodeKind, UiEvent};

use prism_testing::{Harness, creates, destroys, mount};

struct Screen {
    list: Entity<VirtualList>,
}

impl Render for Screen {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(self.list)
    }
}

const ROW: f32 = 40.;

fn app(count: usize) -> Harness {
    Harness::new(move |cx| {
        let list = VirtualList::new(cx, count, ROW, |i, _| text(format!("Row {i}")));
        cx.set_global(list);
        let root = cx.new_entity(|_| Screen { list });
        mount(cx, root);
    })
}

fn list(h: &Harness) -> Entity<VirtualList> {
    *h.rt.app.global::<Entity<VirtualList>>()
}

fn scroll_id(h: &Harness) -> u64 {
    h.host.find(|n| n.kind == NodeKind::Scroll).unwrap()
}

/// Rendered row numbers.
fn rows(h: &Harness) -> Vec<usize> {
    h.host
        .mounted()
        .into_iter()
        .filter_map(|id| h.host.get(id).text.strip_prefix("Row ")?.parse().ok())
        .collect()
}

/// Heights of the top and bottom spacers.
fn spacers(h: &Harness) -> (f32, f32) {
    let scroll = h.host.get(scroll_id(h));
    let height = |id: &u64| match h.host.get(*id).style.as_ref().unwrap().height {
        Length::Px(v) => v,
        other => panic!("spacer height {other:?}"),
    };
    (
        height(scroll.children.first().unwrap()),
        height(scroll.children.last().unwrap()),
    )
}

fn scroll_to(h: &mut Harness, offset: f32, viewport: f32) -> Vec<prism_core::Patch> {
    let id = scroll_id(h);
    h.dispatch(UiEvent::Scroll(id, offset, viewport)).to_vec()
}

#[test]
fn the_window_covers_the_viewport_plus_overscan() {
    let mut h = app(1000);
    scroll_to(&mut h, 0., 400.);
    // 400 / 40 = 10 rows, +1 partial, +6 overscan below.
    assert_eq!(rows(&h), (0..17).collect::<Vec<_>>());
    assert_eq!(list(&h).read(&h.rt.app).visible_range(), 0..17);
    // The spacers keep the content height: 1000 rows in total.
    let (top, bottom) = spacers(&h);
    assert_eq!((top, bottom), (0., (1000 - 17) as f32 * ROW));
    // Scrolled to row 100: 6 rows of overscan above too.
    scroll_to(&mut h, 100. * ROW, 400.);
    assert_eq!(rows(&h), (94..117).collect::<Vec<_>>());
    let (top, bottom) = spacers(&h);
    assert_eq!(top + bottom + 23. * ROW, 1000. * ROW);
    assert_eq!(top, 94. * ROW);
}

#[test]
fn overlapping_windows_reuse_rows() {
    let mut h = app(1000);
    scroll_to(&mut h, 100. * ROW, 400.);
    let kept = h.host.by_text("Row 110");
    let p = scroll_to(&mut h, 105. * ROW, 400.);
    // 5 rows scrolled: 5 new rows (and their text), 5 old ones destroyed.
    assert_eq!(creates(&p), 10);
    assert_eq!(destroys(&p).len(), 10);
    assert_eq!(h.host.by_text("Row 110"), kept, "keyed rows are kept");
    assert_eq!(rows(&h), (99..122).collect::<Vec<_>>());
}

#[test]
fn the_end_of_the_list_and_overscrolling() {
    let mut h = app(50);
    scroll_to(&mut h, 48. * ROW, 400.);
    assert_eq!(*rows(&h).last().unwrap(), 49);
    assert_eq!(spacers(&h).1, 0.);
    // Rubber-banding past both ends.
    scroll_to(&mut h, 10_000. * ROW, 400.);
    assert!(rows(&h).is_empty() || *rows(&h).last().unwrap() == 49);
    scroll_to(&mut h, -300., 400.);
    assert_eq!(rows(&h)[0], 0);
    assert_eq!(spacers(&h).0, 0.);
}

#[test]
fn changing_the_count_trims_or_extends_the_window() {
    let mut h = app(5);
    assert_eq!(rows(&h), [0, 1, 2, 3, 4]);
    let l = list(&h);
    l.update(&mut h.rt.app, |l, cx| l.set_count(2, cx));
    h.render();
    assert_eq!(rows(&h), [0, 1]);
    assert_eq!(spacers(&h), (0., 0.));
    l.update(&mut h.rt.app, |l, cx| l.set_count(0, cx));
    h.render();
    assert!(rows(&h).is_empty());
    l.update(&mut h.rt.app, |l, cx| l.set_count(3000, cx));
    h.render();
    assert_eq!(l.read(&h.rt.app).count(), 3000);
    assert_eq!(rows(&h).len(), l.read(&h.rt.app).visible_range().len());
}

#[test]
fn a_zero_viewport_still_shows_a_row() {
    let mut h = app(100);
    scroll_to(&mut h, 0., 0.);
    assert!(!rows(&h).is_empty());
}
