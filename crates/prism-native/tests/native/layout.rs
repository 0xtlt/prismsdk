//! Flexbox results, text measuring, safe areas, scroll content, RTL, zoom.

use prism_core::{
    Backend, Context, Environment, FluentBuilder, IntoElement, Locale, NodeKind, ParentElement,
    Render, Router, Styled, TextAlign, UiEvent, button, div, relative, screen, scroll, text,
};
use prism_native::Insets;

use crate::common::{HEADER, Harness};

struct Tree(fn() -> prism_core::Div);
impl Render for Tree {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        (self.0)()
    }
}

fn tree(f: fn() -> prism_core::Div) -> Harness {
    Harness::root(move || Tree(f))
}

#[test]
fn columns_apply_padding_and_gap_and_stretch_children() {
    let mut h = tree(|| div().p(10.).gap(5.).child(text("ab")).child(text("abcd")));
    let f = h.layout(400., 300.);
    assert_eq!(f.frame(f.by_text("ab")), (10., 10., 380., 20.));
    assert_eq!(f.frame(f.by_text("abcd")), (10., 35., 380., 20.));
}

#[test]
fn rows_center_cross_axis_and_leave_gaps() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .gap(10.)
                .items_center()
                .child(text("aa"))
                .child(div().w(30.).h(40.)),
        )
    });
    let f = h.layout(400., 300.);
    let row = f.of_kind(NodeKind::View)[1];
    assert_eq!(
        f.frame(row),
        (0., 0., 400., 40.),
        "row is as tall as its tallest child"
    );
    assert_eq!(f.frame(f.by_text("aa")), (0., 10., 16., 20.), "centered");
    let boxed = f.of_kind(NodeKind::View)[2];
    assert_eq!(f.frame(boxed), (26., 0., 30., 40.), "after the 10 pt gap");
}

#[test]
fn justify_between_spreads_children() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .justify_between()
                .child(text("a"))
                .child(text("b")),
        )
    });
    let f = h.layout(200., 100.);
    assert_eq!(f.frame(f.by_text("a")).0, 0.);
    assert_eq!(f.frame(f.by_text("b")).0, 192.);
}

#[test]
fn text_wraps_to_its_container_width() {
    let mut h = tree(|| div().child(div().w(100.).child(text("hello world foo"))));
    let f = h.layout(400., 300.);
    let t = f.by_text("hello world foo");
    // 120 pt on one line > 100: two lines ("hello world" / "foo").
    assert_eq!(f.frame(t), (0., 0., 100., 40.));
    assert!(
        f.measured[&t].contains(&Some(100.)),
        "measured with the width limit: {:?}",
        f.measured[&t]
    );
}

#[test]
fn shrinking_text_stops_at_its_longest_word() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .w(100.)
                .child(text("aaaa bbbb"))
                .child(div().w(80.).h(10.).flex_shrink_0()),
        )
    });
    let f = h.layout(400., 300.);
    let t = f.frame(f.by_text("aaaa bbbb"));
    // min-content (one 4-letter word = 32 pt), so the text wraps to 2 lines.
    assert_eq!((t.2, t.3), (32., 40.));
}

#[test]
fn fractions_hidden_nodes_and_margins() {
    let mut h = tree(|| {
        div()
            .child(div().w(relative(0.5)).h(10.))
            .child(text("gone").when(true, |t| t.with_style(|s| s.display_none = true)))
            .child(div().mt(7.).ml(3.).h(10.))
    });
    let f = h.layout(400., 300.);
    let views = f.of_kind(NodeKind::View);
    assert_eq!(f.frame(views[1]), (0., 0., 200., 10.));
    // The hidden text takes no room and is hidden natively.
    assert_eq!(f.frame(views[2]), (3., 17., 397., 10.));
    assert_eq!(f.hidden.get(&f.by_text("gone")), Some(&true));
}

#[test]
fn safe_area_insets_pad_the_root_but_not_its_background() {
    let mut h = tree(|| div().p(10.).child(text("x")));
    h.r.layout(
        400.,
        800.,
        Insets {
            top: 47.,
            right: 0.,
            bottom: 34.,
            left: 5.,
        },
    );
    let f = h.w();
    let root = f.root.expect("root");
    assert_eq!(f.frame(root), (0., 0., 400., 800.), "edge to edge");
    assert_eq!(f.frame(f.by_text("x")), (15., 57., 375., 20.));
    // Insets going away (rotation) re-pad the root.
    h.r.layout(400., 800., Insets::default());
    assert_eq!(h.w().frame(h.w().by_text("x")), (10., 10., 380., 20.));
}

struct Screens;
impl Render for Screens {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Home").child(text("top"))
    }
}
struct ScrollScreen;
impl Render for ScrollScreen {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("List").child(scroll().child(text("row")))
    }
}

#[test]
fn stack_screens_get_the_platform_safe_area_unless_they_scroll() {
    let mut h = Harness::new(|cx| {
        let r = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Screens))
            .route("/list", |cx, _| cx.new_entity(|_| ScrollScreen))
            .build(cx);
        cx.set_root(r);
    });
    h.r.widgets.screen_insets = Insets {
        top: 10.,
        right: 0.,
        bottom: 10.,
        left: 5.,
    };
    let f = h.layout(400., 800.);
    assert_eq!(f.frame(f.by_text("top")), (5., 10., 395., 20.));
    let screen = f.of_kind(NodeKind::Screen)[0];
    assert_eq!(f.frame(screen), (0., HEADER, 400., 800. - HEADER));

    h.rt.app.navigate("/list");
    h.rerender();
    let f = h.layout(400., 800.);
    let s = f.of_kind(NodeKind::Scroll)[0];
    // The scroll view runs under the bars (top/bottom insets dropped).
    assert_eq!(f.frame(s), (5., 0., 395., 800. - HEADER));
}

struct Rows {
    n: usize,
}
impl Render for Rows {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(button("more").on_click(cx.listener(|this, _, _| this.n += 10)))
            .child(
                scroll()
                    .p(4.)
                    .children((0..self.n).map(|i| text(format!("r{i}")))),
            )
    }
}

#[test]
fn scroll_content_size_follows_rows_and_never_drops_below_the_frame() {
    let mut h = Harness::root(|| Rows { n: 2 });
    let f = h.layout(300., 220.);
    let s = f.of_kind(NodeKind::Scroll)[0];
    // 200 pt viewport (220 - the 20 pt button); 2 rows need 48.
    assert_eq!(f.frame(s).3, 200.);
    assert_eq!(f.content[&s], (300., 200.));
    let more = f.by_text("more");
    let patches = h.rt.dispatch(UiEvent::Click(more));
    h.r.apply(&patches);
    let f = h.layout(300., 220.);
    // 12 rows × 20 + 4 + 4.
    assert_eq!(f.content[&s], (300., 248.));
}

#[test]
fn right_to_left_mirrors_padding_offsets_and_text_alignment() {
    let mut h = tree(|| {
        div()
            .pl(20.)
            .child(div().w(50.).h(10.).offset(10., 0.))
            .child(text("t"))
    });
    let f = h.layout(400., 300.);
    let boxed = f.of_kind(NodeKind::View)[1];
    assert_eq!(f.frame(boxed).0, 20.);
    assert_eq!(f.styles[&boxed].transform.x, 10.);
    assert_eq!(f.styles[&f.by_text("t")].text_align, TextAlign::Left);

    h.rt.app.set_locale(Some(Locale::new("ar")));
    h.rerender();
    let f = h.layout(400., 300.);
    assert_eq!(f.frame(boxed).0, 330., "400 - 20 (leading padding) - 50");
    assert_eq!(f.styles[&boxed].transform.x, -10., "offsets are leading");
    assert_eq!(f.styles[&f.by_text("t")].text_align, TextAlign::Right);
    assert_eq!(f.env.as_ref().map(|e| e.lang.as_str()), Some("ar"));
}

#[test]
fn text_scale_grows_text_but_not_containers() {
    let mut h = tree(|| div().text_size(10.).child(text("t").text_size(17.)));
    let root = h.w().root.unwrap();
    let t = h.w().by_text("t");
    assert_eq!(h.w().styles[&t].font_size, 17.);
    h.rt.app.set_environment(Environment {
        text_scale: 1.5,
        ..Default::default()
    });
    h.rerender();
    assert_eq!(h.w().styles[&t].font_size, 26., "17 × 1.5, rounded");
    assert_eq!(h.w().styles[&root].font_size, 10., "views keep their size");
}

#[test]
fn zoom_scales_lengths_fonts_and_ignores_nonsense() {
    let mut h = tree(|| {
        div()
            .p(10.)
            .child(div().w(50.).h(10.).rounded(4.))
            .child(text("z"))
    });
    h.r.set_zoom(2.);
    assert_eq!(h.r.zoom(), 2.);
    let f = h.layout(400., 300.);
    let boxed = f.of_kind(NodeKind::View)[1];
    assert_eq!(f.frame(boxed), (20., 20., 100., 20.));
    assert_eq!(f.styles[&boxed].corner_radius, 8.);
    let t = f.by_text("z");
    assert_eq!(f.styles[&t].font_size, prism_core::DEFAULT_FONT_SIZE * 2.);
    h.r.set_zoom(0.);
    h.r.set_zoom(-1.);
    assert_eq!(h.r.zoom(), 2., "non-positive zooms are ignored");
    h.r.set_zoom(1.);
    let f = h.layout(400., 300.);
    assert_eq!(f.frame(boxed), (10., 10., 50., 10.));
}

#[test]
fn unchanged_frames_are_not_sent_again() {
    let mut h = tree(|| div().child(text("a")));
    h.layout(400., 300.);
    h.r.widgets.take_calls();
    h.layout(400., 300.);
    assert!(
        h.r.widgets.take_calls().is_empty(),
        "a relayout at the same size touches no view"
    );
}

#[test]
fn right_to_left_swaps_leading_and_trailing_corners() {
    use prism_core::Corners;
    let mut h = tree(|| {
        div()
            .child(
                div()
                    .w(50.)
                    .h(10.)
                    .rounded_corners(4., Corners::TOP_LEADING),
            )
            .child(div().w(50.).h(10.).rounded_top(4.))
    });
    let f = h.layout(400., 300.);
    let (lead, top) = (f.of_kind(NodeKind::View)[1], f.of_kind(NodeKind::View)[2]);
    assert_eq!(f.styles[&lead].rounded_corners, Corners::TOP_LEADING);
    assert_eq!(f.styles[&top].rounded_corners, Corners::TOP);

    h.rt.app.set_locale(Some(Locale::new("ar")));
    h.rerender();
    let f = h.layout(400., 300.);
    assert_eq!(
        f.styles[&lead].rounded_corners,
        Corners::TOP_TRAILING,
        "backends get leading = left"
    );
    assert_eq!(f.styles[&top].rounded_corners, Corners::TOP, "symmetric");
}

#[test]
fn truncated_and_unwrapped_texts_shrink_below_their_words_on_one_line() {
    let mut h = tree(|| {
        div()
            .child(
                div()
                    .flex_row()
                    .w(100.)
                    .child(text("aaaa bbbb").truncate())
                    .child(div().w(80.).h(10.).flex_shrink_0()),
            )
            .child(
                div()
                    .flex_row()
                    .w(100.)
                    .child(text("cccc dddd").nowrap())
                    .child(div().w(80.).h(10.).flex_shrink_0()),
            )
            .child(div().w(40.).child(text("eeee ffff gggg").line_clamp(2)))
    });
    let f = h.layout(400., 300.);
    // One line (measured without a width limit), cut to the 20 pt left.
    let t = f.frame(f.by_text("aaaa bbbb"));
    assert_eq!((t.2, t.3), (20., 20.));
    let id = f.by_text("cccc dddd");
    let t = f.frame(id);
    assert_eq!((t.2, t.3), (20., 20.));
    assert!(
        f.measured[&id].iter().all(Option::is_none),
        "{:?}",
        f.measured[&id]
    );
    // A clamp wraps at the width; the platform caps the lines it draws.
    let t = f.frame(f.by_text("eeee ffff gggg"));
    assert_eq!(t.2, 40.);
}
