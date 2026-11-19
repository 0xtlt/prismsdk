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
fn shrinking_text_goes_below_its_longest_word_like_the_web() {
    // Every node has `min-width: 0` on the web: a squeezed text takes the
    // room left (and breaks its words) instead of overflowing its row.
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
    assert_eq!((t.2, t.3), (20., 40.));
}

#[test]
fn a_label_beside_an_icon_wraps_in_a_narrow_cell() {
    // #37: a 110 pt cell, a two-word label and a 16 pt icon spread apart.
    let mut h = tree(|| {
        div().flex_row().child(
            div().w(110.).child(
                div()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(6.)
                    .child(text("HEURES CREUSES").text_size(12.).semibold())
                    .child(div().w(16.).h(16.).flex_shrink_0()),
            ),
        )
    });
    let f = h.layout(400., 300.);
    let t = f.frame(f.by_text("HEURES CREUSES"));
    // 112 pt on one line; the row leaves it 110 - 6 - 16 = 88: two lines,
    // inside the cell.
    assert_eq!((t.0, t.2, t.3), (0., 88., 40.));
    let icon = f.of_kind(NodeKind::View)[3];
    assert_eq!(f.frame(icon), (94., 12., 16., 16.));
}

#[test]
fn a_fixed_width_shrinks_too_unless_told_not_to() {
    let mut h = tree(|| {
        div()
            .child(
                div()
                    .flex_row()
                    .w(100.)
                    .child(div().w(80.).h(10.))
                    .child(div().w(80.).h(10.)),
            )
            .child(
                div()
                    .flex_row()
                    .w(100.)
                    .child(div().w(80.).h(10.).min_w(prism_core::px(80.)))
                    .child(div().w(80.).h(10.)),
            )
    });
    let f = h.layout(400., 300.);
    let v = f.of_kind(NodeKind::View);
    assert_eq!(f.frame(v[2]).2, 50., "shrinks like the web's min-width:0");
    assert_eq!(f.frame(v[3]).2, 50.);
    assert_eq!(f.frame(v[5]).2, 80., "an explicit minimum holds");
    assert_eq!(f.frame(v[6]).2, 20.);
}

#[test]
fn fit_content_text_wraps_at_its_longest_word() {
    // Not stretched: its width is its content, capped by the room, never
    // narrower than its longest word (min-content).
    let mut h = tree(|| div().w(50.).items_start().child(text("aaaa bbbbbbbb cc")));
    let f = h.layout(400., 300.);
    let t = f.frame(f.by_text("aaaa bbbbbbbb cc"));
    assert_eq!((t.2, t.3), (64., 60.));
}

#[test]
fn switches_keep_their_width_in_a_crowded_row() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .w(100.)
                .child(text("aaaa bbbb cccc dddd"))
                .child(prism_core::toggle(false)),
        )
    });
    let f = h.layout(400., 300.);
    let toggle = f.of_kind(NodeKind::Toggle)[0];
    assert_eq!(f.frame(toggle).2, 50.);
    assert_eq!(f.frame(f.by_text("aaaa bbbb cccc dddd")).2, 50.);
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

/// A card centred in a full-height column of a scroll view (#76), with a
/// sticky title and, optionally, a footer below the scroll view.
struct CenteredScreen {
    footer: bool,
}
impl Render for CenteredScreen {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Centered")
            .child(
                scroll().child(
                    div()
                        .min_h(relative(1.))
                        .justify_center()
                        .child(text("title").sticky())
                        .child(div().h(100.).child(text("card"))),
                ),
            )
            .when(self.footer, |s| s.child(text("footer")))
    }
}

fn centered(footer: bool) -> Harness {
    let mut h = Harness::new(move |cx| {
        let r = Router::new()
            .header(false)
            .route("/", move |cx, _| {
                cx.new_entity(|_| CenteredScreen { footer })
            })
            .build(cx);
        cx.set_root(r);
    });
    h.r.widgets.screen_insets = Insets {
        top: 46.,
        right: 0.,
        bottom: 34.,
        left: 0.,
    };
    h
}

#[test]
fn a_scroll_view_under_the_bars_lays_its_content_out_between_them() {
    let mut h = centered(false);
    let f = h.layout(400., 800.);
    let s = f.of_kind(NodeKind::Scroll)[0];
    let visible = 800. - HEADER - 46. - 34.;
    // The scroll view still runs under the bars; the platform insets its
    // content by them, so `relative(1.)` is what is visible between them.
    assert_eq!(f.frame(s), (0., 0., 400., 800. - HEADER));
    let column = f.children[&s][0];
    assert_eq!(f.frame(column), (0., 0., 400., visible));
    // Centred there: the title and the 100 pt card, 120 pt together.
    let card = f.children[&column][1];
    assert_eq!(f.frame(f.by_text("title")).1, (visible - 120.) / 2.);
    assert_eq!(f.frame(card).1, (visible - 120.) / 2. + 20.);
    // Nothing to scroll: the content fits what is visible.
    assert_eq!(f.content[&s], (400., visible));
    // A sticky title counts from the top of the content, as the platform
    // reports its scrolling; it sticks to the end of the column.
    let (_, st) = f.sticky[&f.by_text("title")].expect("sticky");
    let top = (visible - 120.) / 2.;
    assert_eq!(st.top, top);
    assert_eq!(st.max_offset, visible - top - 20.);
}

#[test]
fn a_scroll_view_above_a_footer_only_takes_the_bar_it_runs_under() {
    let mut h = centered(true);
    let f = h.layout(400., 800.);
    let s = f.of_kind(NodeKind::Scroll)[0];
    // The 20 pt footer covers 20 of the 34 pt bottom bar.
    let height = 800. - HEADER - 20.;
    assert_eq!(f.frame(s), (0., 0., 400., height));
    let column = f.children[&s][0];
    assert_eq!(f.frame(column).3, height - 46. - 14.);
    assert_eq!(f.content[&s], (400., height - 46. - 14.));
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
fn translate_percent_shifts_by_the_nodes_own_size_after_layout() {
    // A 40 × 10 label centred on 30 % of a 200 pt wide parent, above its top.
    let mut h = tree(|| {
        div().w(200.).h(50.).child(
            div()
                .absolute()
                .left(relative(0.3))
                .top(20.)
                .w(40.)
                .h(10.)
                .translate_percent(-50., -100.),
        )
    });
    let f = h.layout(400., 300.);
    let label = f.of_kind(NodeKind::View)[1];
    assert_eq!(f.frame(label), (40., 10., 40., 10.), "60 - 20, 20 - 10");

    // Right to left: leading is the right edge, the shift mirrors too.
    h.rt.app.set_locale(Some(Locale::new("ar")));
    h.rerender();
    let f = h.layout(400., 300.);
    assert_eq!(f.frame(label).0, 120., "200 - 60 - 40, then + 20");
}

#[test]
fn absolute_overlays_leave_their_sibling_bars_alone() {
    // A bar chart's structure: the grid, the bars pulled up over it by a
    // negative margin, then a zero-height absolute layer of labels and a
    // read-out (what the watch, Android and every native backend lay out).
    fn chart(overlays: bool) -> prism_core::Div {
        let bar = |f: f32| {
            div().flex_1().h_full().justify_end().items_center().child(
                div()
                    .w(relative(0.72))
                    .h(relative(f))
                    .bg(prism_core::rgb(0xff0000)),
            )
        };
        div().w(300.).child(
            div()
                .child(prism_core::shape().h(100.).w_full())
                .child(
                    div()
                        .flex_row()
                        .h(100.)
                        .with_style(|s| s.margin.top = -100.)
                        .py(7.)
                        .items_end()
                        .child(bar(0.5))
                        .child(bar(1.))
                        .child(bar(0.25)),
                )
                .when(overlays, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(7.)
                            .left(0.)
                            .right(0.)
                            .h(0.)
                            .child(
                                text("Target")
                                    .absolute()
                                    .right(0.)
                                    .top(20.)
                                    .translate_percent(0., -100.),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(relative(0.5))
                                    .top(0.)
                                    .w(60.)
                                    .h(30.)
                                    .bg(prism_core::rgb(0x00ff00)),
                            ),
                    )
                }),
        )
    }
    let bars = |overlays: bool| {
        let mut h = if overlays {
            tree(|| chart(true))
        } else {
            tree(|| chart(false))
        };
        let f = h.layout(400., 400.);
        let red = Some(prism_core::rgb(0xff0000));
        let mut out: Vec<(f32, f32, f32, f32)> = f
            .styles
            .iter()
            .filter(|(_, s)| s.background == red)
            .map(|(id, _)| f.frame(*id))
            .collect();
        // (Frames are relative to each bar's cell: sorted by height.)
        out.sort_by(|a, b| a.3.total_cmp(&b.3));
        out
    };
    let plain = bars(false);
    assert_eq!(
        plain.iter().map(|b| b.3).collect::<Vec<_>>(),
        [21., 43., 86.],
        "fractions of the 86 pt between the paddings (rounded to points)"
    );
    assert_eq!(bars(true), plain, "the overlay changes no bar");
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

// --- Grid, weights, per-axis gaps, baselines, aspect ratio (#13) -----------

fn views(h: &Harness) -> Vec<prism_core::NodeId> {
    h.w().of_kind(NodeKind::View)
}

#[test]
fn auto_fill_grids_fit_as_many_columns_as_the_minimum_allows() {
    use prism_core::{auto_fill, fr, minmax};
    let mut h = tree(|| {
        div().child(
            div()
                .grid()
                .grid_cols([auto_fill(minmax(172., fr(1.)))])
                .gap(12.)
                .children((0..5).map(|i| text(format!("c{i}")))),
        )
    });
    let f = h.layout(400., 600.);
    // (400 + 12) / (172 + 12) = 2 columns of (400 - 12) / 2 = 194.
    assert_eq!(f.frame(f.by_text("c0")), (0., 0., 194., 20.));
    assert_eq!(f.frame(f.by_text("c1")), (206., 0., 194., 20.));
    assert_eq!(f.frame(f.by_text("c2")), (0., 32., 194., 20.));
    assert_eq!(f.frame(f.by_text("c4")), (0., 64., 194., 20.));
    let f = h.layout(800., 600.);
    // 4 columns now: (800 - 36) / 4 = 191.
    assert_eq!(f.frame(f.by_text("c3")), (609., 0., 191., 20.));
    assert_eq!(f.frame(f.by_text("c4")).1, 32.);
}

#[test]
fn grid_cells_of_a_row_share_its_height_and_spans_cover_columns() {
    let mut h = tree(|| {
        div().child(
            div()
                .grid()
                .cols(3)
                .gap_x(10.)
                .gap_y(4.)
                .child(div().child(text("short")))
                .child(div().child(text("a much longer text that wraps")))
                .child(div().child(text("x")))
                .child(div().col_span(2).child(text("wide")))
                .child(div().child(text("y"))),
        )
    });
    h.layout(320., 600.);
    let cells = views(&h)[2..].to_vec();
    let f = h.w();
    // 3 columns of (320 - 20) / 3 = 100.
    let (_, _, w0, h0) = f.frame(cells[0]);
    let (x1, _, _, h1) = f.frame(cells[1]);
    assert_eq!((w0, x1), (100., 110.));
    assert!(h1 > 20., "the long text wraps: {h1}");
    assert_eq!(h0, h1, "equal-height cells");
    assert_eq!(f.frame(cells[2]).3, h1);
    // Second row, after the 4 pt row gap: the wide cell spans 2 columns.
    assert_eq!(f.frame(cells[3]), (0., h1 + 4., 210., 20.));
    assert_eq!(f.frame(cells[4]).0, 220.);
}

#[test]
fn fixed_tracks_and_explicit_placement() {
    use prism_core::{fr, px};
    let mut h = tree(|| {
        div().child(
            div()
                .grid()
                .grid_cols([px(100.).into(), fr(1.), fr(2.)])
                .child(text("a"))
                .child(text("b").col_start(3))
                .child(text("c").row_start(2).col_start(1).col_span(3)),
        )
    });
    let f = h.layout(400., 300.);
    assert_eq!(f.frame(f.by_text("a")), (0., 0., 100., 20.));
    // 300 left: 1fr = 100, 2fr = 200.
    assert_eq!(f.frame(f.by_text("b")), (200., 0., 200., 20.));
    assert_eq!(f.frame(f.by_text("c")), (0., 20., 400., 20.));
}

#[test]
fn grow_and_shrink_take_any_weight() {
    let mut h = tree(|| {
        div()
            .child(
                div()
                    .flex_row()
                    .child(div().h(10.).flex_1())
                    .child(div().h(10.).flex_1().flex_grow(2.)),
            )
            .child(
                div()
                    .flex_row()
                    .w(100.)
                    .child(div().h(10.).w(100.))
                    .child(div().h(10.).w(100.).flex_shrink(3.)),
            )
    });
    h.layout(300., 300.);
    let v = views(&h);
    let f = h.w();
    assert_eq!(f.frame(v[2]).2, 100.);
    assert_eq!(f.frame(v[3]), (100., 0., 200., 10.));
    // 100 pt too wide, shrunk 1 : 3 (weighted by width): 25 and 75.
    assert_eq!(f.frame(v[5]).2, 75.);
    assert_eq!(f.frame(v[6]).2, 25.);
}

#[test]
fn per_axis_gaps_override_gap() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .flex_wrap()
                .w(100.)
                .gap(50.)
                .gap_x(4.)
                .gap_y(6.)
                .children((0..3).map(|_| div().w(40.).h(10.))),
        )
    });
    h.layout(300., 300.);
    let v = views(&h);
    let f = h.w();
    assert_eq!(f.frame(v[2]).0, 0.);
    assert_eq!(f.frame(v[3]).0, 44.);
    assert_eq!(f.frame(v[4]), (0., 16., 40., 10.), "next line after gap_y");
}

#[test]
fn items_baseline_lines_up_first_lines_of_text() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .items_baseline()
                .child(text("42").text_size(40.))
                .child(text("kg").text_size(12.))
                .child(div().w(10.).h(8.)),
        )
    });
    let f = h.layout(300., 300.);
    let big = f.frame(f.by_text("42"));
    let small = f.frame(f.by_text("kg"));
    // Estimated ascent 0.95 em: 38 and 11.4 (frames are rounded).
    assert_eq!(big.1, 0.);
    assert_eq!(small.1, (38f32 - 11.4).round(), "{small:?}");
    // A box without text sits on the baseline with its bottom edge.
    let boxed = f.frame(f.of_kind(NodeKind::View)[2]);
    assert_eq!(boxed.1 + boxed.3, 38., "{boxed:?}");
}

#[test]
fn aspect_ratio_derives_the_missing_side() {
    let mut h = tree(|| {
        div()
            .items_start()
            .child(div().w(200.).aspect_ratio(2.))
            .child(div().h(30.).aspect_ratio(0.5))
    });
    h.layout(400., 400.);
    let v = views(&h);
    assert_eq!(h.w().frame(v[1]), (0., 0., 200., 100.));
    assert_eq!(h.w().frame(v[2]), (0., 100., 15., 30.));
}

// --- Sticky, fixed, horizontal scroll ---------------------------------------

#[test]
fn fixed_nodes_are_placed_against_the_root_not_their_parent() {
    let mut h = tree(|| {
        div().child(
            scroll().p(20.).child(text("row")).child(
                div()
                    .p(30.)
                    .child(button("+").fixed().bottom(16.).right(16.).w(56.).h(56.)),
            ),
        )
    });
    h.r.layout(
        400.,
        800.,
        Insets {
            bottom: 34.,
            ..Default::default()
        },
    );
    let f = h.w();
    let root = f.root.unwrap();
    let fab = f.of_kind(NodeKind::Button)[0];
    assert_eq!(f.children[&root].last(), Some(&fab), "shown by the root");
    let card = f.of_kind(NodeKind::View)[1];
    assert!(f.children[&card].is_empty(), "not by its parent");
    // Inside the safe area: 16 + 34 from the bottom.
    assert_eq!(f.frame(fab), (400. - 16. - 56., 800. - 50. - 56., 56., 56.));
}

thread_local!(static FIXED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) });

struct FixedScreen;
impl Render for FixedScreen {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let fixed = FIXED.with(std::cell::Cell::get);
        screen("Home").child(
            scroll().child(
                div()
                    .child(text("row"))
                    .child(text("fab").when(fixed, |t| t.fixed().top(10.).left(10.))),
            ),
        )
    }
}

#[test]
fn fixed_nodes_float_over_their_screen_and_come_back_when_not_fixed() {
    FIXED.with(|f| f.set(true));
    let mut h = Harness::new(|cx| {
        let r = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| FixedScreen))
            .build(cx);
        cx.set_root(r);
    });
    let f = h.layout(400., 800.);
    let screen_id = f.of_kind(NodeKind::Screen)[0];
    let fab = f.by_text("fab");
    assert_eq!(f.children[&screen_id].last(), Some(&fab));
    assert_eq!(f.frame(fab), (10., 10., 24., 20.));

    FIXED.with(|f| f.set(false));
    h.rt.app.refresh();
    h.rerender();
    let f = h.layout(400., 800.);
    let fab = f.by_text("fab");
    assert!(!f.children[&screen_id].contains(&fab));
    let card = f.of_kind(NodeKind::View)[0];
    assert_eq!(f.children[&card].last(), Some(&fab), "back in its parent");
    assert_eq!(f.frame(fab).1, 20., "in the flow, below the row");
}

#[test]
fn sticky_nodes_get_their_range_in_the_scroll_view() {
    let mut h = tree(|| {
        div().child(
            scroll()
                .child(text("intro"))
                .child(
                    div()
                        .child(text("header").sticky().top(8.))
                        .child(div().h(300.)),
                )
                .child(div().h(500.)),
        )
    });
    let f = h.layout(400., 600.);
    let header = f.by_text("header");
    let scroll_id = f.of_kind(NodeKind::Scroll)[0];
    let (s, st) = f.sticky[&header].expect("sticky");
    assert_eq!(s, scroll_id);
    // Below the 20 pt intro; its section ends at 20 + 20 + 300.
    assert_eq!(st.top, 20.);
    assert_eq!(st.inset, 8.);
    assert_eq!(st.max_offset, 300.);
    assert_eq!(st.offset(0.), 0.);
    assert_eq!(st.offset(100.), 88.);
    assert_eq!(st.offset(1000.), 300.);
    // In the flow, at its natural place.
    assert_eq!(f.frame(header).1, 0.);
}

#[test]
fn horizontal_scrolls_keep_children_at_their_size() {
    let mut h = tree(|| {
        div().child(
            prism_core::scroll_x()
                .gap(8.)
                .px(10.)
                .children((0..6).map(|i| text(format!("chip number {i}")))),
        )
    });
    let f = h.layout(200., 600.);
    let scroll_id = f.of_kind(NodeKind::Scroll)[0];
    let first = f.frame(f.by_text("chip number 0"));
    // 13 characters, never shrunk or wrapped.
    assert_eq!(first, (10., 0., 104., 20.));
    assert_eq!(f.frame(f.by_text("chip number 1")).0, 122.);
    // As tall as a chip, not grown to the window.
    assert_eq!(f.frame(scroll_id).3, 20.);
    let (w, _) = f.content[&scroll_id];
    assert_eq!(w, 10. + 6. * 104. + 5. * 8. + 10.);
}

#[test]
fn own_width_rules_follow_the_element_not_the_window() {
    let mut h = tree(|| {
        div().flex_row().child(
            div().w(relative(0.5)).child(
                div()
                    .when_width_at_least(300., |s| s.flex_row())
                    .child(text("a"))
                    .child(text("b")),
            ),
        )
    });
    let f = h.layout(400., 300.);
    // 200 wide: a column.
    assert_eq!(f.frame(f.by_text("b")).1, 20.);
    let f = h.layout(800., 300.);
    // 400 wide: a row.
    assert_eq!(f.frame(f.by_text("b")), (8., 0., 8., 20.));
    let f = h.layout(500., 300.);
    assert_eq!(f.frame(f.by_text("b")).1, 20.);
}

struct ChipsScreen;
impl Render for ChipsScreen {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Chips").child(prism_core::scroll_x().child(text("chip")))
    }
}

#[test]
fn a_screen_starting_with_a_horizontal_scroll_keeps_its_safe_area() {
    let mut h = Harness::new(|cx| {
        let r = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| ChipsScreen))
            .build(cx);
        cx.set_root(r);
    });
    h.r.widgets.screen_insets = Insets {
        top: 10.,
        right: 0.,
        bottom: 10.,
        left: 0.,
    };
    let f = h.layout(400., 800.);
    let s = f.of_kind(NodeKind::Scroll)[0];
    assert_eq!(
        f.frame(s).1,
        10.,
        "below the bar, unlike vertical page scrolls"
    );
}

#[test]
fn aspect_ratio_with_a_capped_width_sizes_its_parent_by_the_cap() {
    let mut h = tree(|| {
        div().child(
            div()
                .p(10.)
                .child(div().w_full().max_w(360.).aspect_ratio(2.))
                .child(text("after")),
        )
    });
    h.layout(900., 900.);
    let v = views(&h);
    let f = h.w();
    assert_eq!(f.frame(v[2]), (10., 10., 360., 180.));
    assert_eq!(f.frame(f.by_text("after")).1, 190.);
    assert_eq!(f.frame(v[1]).3, 220.);
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

#[test]
fn reversed_rows_and_columns_start_from_the_end() {
    let mut h = tree(|| {
        div()
            .child(
                div()
                    .flex_row_reverse()
                    .gap(10.)
                    .child(text("aa"))
                    .child(text("b")),
            )
            .child(
                div()
                    .h(100.)
                    .flex_col_reverse()
                    .child(text("top"))
                    .child(text("up")),
            )
    });
    let f = h.layout(200., 300.);
    // Trailing to leading, packed against the trailing edge.
    assert_eq!(f.frame(f.by_text("aa")), (184., 0., 16., 20.));
    assert_eq!(f.frame(f.by_text("b")), (166., 0., 8., 20.));
    // Bottom to top, packed against the bottom.
    assert_eq!(f.frame(f.by_text("top")).1, 80.);
    assert_eq!(f.frame(f.by_text("up")).1, 60.);

    h.rt.app.set_locale(Some(Locale::new("ar")));
    h.rerender();
    let f = h.layout(200., 300.);
    assert_eq!(
        f.frame(f.by_text("aa")).0,
        0.,
        "RTL: the trailing edge is the left"
    );
    assert_eq!(f.frame(f.by_text("b")).0, 26.);
}

#[test]
fn order_moves_children_in_the_layout_but_not_in_the_view_tree() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .child(text("a"))
                .child(text("b").order(-1))
                .child(text("c")),
        )
    });
    let f = h.layout(200., 100.);
    let (a, b, c) = (f.by_text("a"), f.by_text("b"), f.by_text("c"));
    assert_eq!(
        (f.frame(b).0, f.frame(a).0, f.frame(c).0),
        (0., 8., 16.),
        "b first, then a and c in the app's order"
    );
    let row = f.of_kind(NodeKind::View)[1];
    assert_eq!(f.children[&row], [a, b, c], "views keep the app's order");
}

struct Checkbox;
impl Render for Checkbox {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        // The box after its label on narrow windows, before it from `md`.
        div().child(
            div()
                .flex_row()
                .child(text("label"))
                .child(div().w(20.).h(20.).order(1).md(|s| s.order(-1))),
        )
    }
}

#[test]
fn order_and_reversed_directions_follow_responsive_refinements() {
    let mut h = crate::common::Harness::root(|| Checkbox);
    h.r.apply(&h.rt.set_viewport(400., 300.));
    let f = h.layout(400., 300.);
    let label = f.by_text("label");
    let check = f.of_kind(NodeKind::View)[2];
    assert_eq!((f.frame(label).0, f.frame(check).0), (0., 40.));

    h.r.apply(&h.rt.set_viewport(800., 300.));
    let f = h.layout(800., 300.);
    assert_eq!(
        (f.frame(check).0, f.frame(label).0),
        (0., 20.),
        "md: box first"
    );

    h.r.apply(&h.rt.set_viewport(400., 300.));
    let f = h.layout(400., 300.);
    assert_eq!((f.frame(label).0, f.frame(check).0), (0., 40.), "back");
}

#[test]
fn reversed_directions_follow_responsive_refinements() {
    let mut h = tree(|| {
        div().child(
            div()
                .flex_row()
                .md(|s| s.flex_row_reverse())
                .child(text("a"))
                .child(text("b")),
        )
    });
    h.r.apply(&h.rt.set_viewport(400., 300.));
    let f = h.layout(400., 300.);
    assert_eq!(f.frame(f.by_text("a")).0, 0.);
    h.r.apply(&h.rt.set_viewport(800., 300.));
    let f = h.layout(800., 300.);
    assert_eq!(f.frame(f.by_text("a")).0, 792.);
    assert_eq!(f.frame(f.by_text("b")).0, 784.);
}

/// #228: auto margins take the free space, like CSS.
#[test]
fn auto_margins_center_and_push() {
    let mut h = tree(|| {
        div()
            .h(300.)
            // A centered column, the parent left as it is.
            .child(div().w_full().max_w(200.).h(10.).mx_auto())
            // The last item of a row pushed to the end, with no spacer.
            .child(
                div()
                    .flex_row()
                    .child(div().w(20.).h(10.))
                    .child(div().w(30.).h(10.).ml_auto()),
            )
            // A footer pinned to the bottom of the column.
            .child(div().h(20.).mt_auto())
    });
    let f = h.layout(400., 300.);
    let v = f.of_kind(NodeKind::View);
    assert_eq!(f.frame(v[1]), (100., 0., 200., 10.), "centered column");
    assert_eq!(f.frame(v[4]).0, 370., "pushed to the end of the row");
    assert_eq!(f.frame(v[5]).1, 280., "footer at the bottom");
}

#[test]
fn a_margin_in_points_replaces_auto() {
    let mut h = tree(|| div().child(div().w(100.).h(10.).mx_auto().ml(8.)));
    let f = h.layout(400., 300.);
    let v = f.of_kind(NodeKind::View);
    assert_eq!(f.frame(v[1]).0, 8.);
}

/// #232: a `z_index` inside a cell paints the cell above its neighbours;
/// the layout keeps the tree order.
#[test]
fn z_index_sets_the_paint_order_not_the_layout() {
    let mut h = tree(|| {
        div()
            .flex_row()
            .child(div().w(50.).h(10.).child(div().w(10.).h(10.).z_index(1)))
            .child(div().w(50.).h(10.))
    });
    let f = h.layout(400., 300.);
    let v = f.of_kind(NodeKind::View);
    let (row, first, second) = (v[0], v[1], v[3]);
    assert_eq!(
        h.w().children[&row],
        [second, first],
        "the cell with the handle on top"
    );
    let f = h.layout(400., 300.);
    assert_eq!(
        (f.frame(first).0, f.frame(second).0),
        (0., 50.),
        "laid out in order"
    );
}

/// #232: a child shows its own `z_index`, else the highest (at least 0) of
/// the nodes inside it that set one, not looking inside those.
#[test]
fn z_index_shown_by_a_child_is_its_own_or_the_highest_inside() {
    let mut h = tree(|| {
        div()
            .flex_row()
            .child(div().child(div().z_index(-1)))
            .child(div().z_index(-2))
            .child(div().child(div().z_index(5).child(div().z_index(9))))
            .child(div().child(div().z_index(3)))
    });
    let f = h.layout(400., 300.);
    let v = f.of_kind(NodeKind::View);
    let (row, a, b, c, d) = (v[0], v[1], v[3], v[4], v[7]);
    assert_eq!(h.w().children[&row], [b, a, d, c]);
}

/// #232: a refinement's `z_index` (hover) paints its node above, then back.
#[test]
fn z_index_follows_interaction_states() {
    use prism_core::Interaction;
    let mut h = tree(|| {
        div()
            .flex_row()
            .child(div().w(50.).h(10.).hover(|s| s.z_index(2)))
            .child(div().w(50.).h(10.).z_index(1))
    });
    h.layout(400., 300.);
    let v = h.w().of_kind(NodeKind::View);
    let (row, first, second) = (v[0], v[1], v[2]);
    assert_eq!(h.w().children[&row], [first, second]);
    h.r.set_interaction(first, Interaction::Hover, true);
    assert_eq!(h.w().children[&row], [second, first], "hovered: on top");
    h.r.set_interaction(first, Interaction::Hover, false);
    assert_eq!(h.w().children[&row], [first, second]);
}

/// #232: tabs and stacks keep their order whatever their screens hold.
#[test]
fn z_index_leaves_tabs_in_their_order() {
    struct App;
    impl Render for App {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            let _ = cx;
            prism_core::tabs(0)
                .child(prism_core::tab("A", "a").child(div()))
                .child(prism_core::tab("B", "b").child(div().child(div().z_index(3))))
                .child(prism_core::tab("C", "c").child(div()))
        }
    }
    let mut h = Harness::root(|| App);
    h.layout(400., 300.);
    let bar = h.w().of_kind(NodeKind::Tabs)[0];
    let tabs = h.w().of_kind(NodeKind::Tab);
    assert_eq!(h.w().children[&bar], tabs);
}
