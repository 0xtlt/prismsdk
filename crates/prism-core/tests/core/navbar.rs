//! Navigation bar options: large title, bar buttons, tint (#30).

use prism_core::navbar::{BarPlacement, ScreenConfig};
use prism_core::prelude::*;
use prism_core::{NodeKind, UiEvent};

use prism_testing::Harness;

// --- Bar options (#30) -------------------------------------------------------

#[derive(Default)]
struct Todos {
    editing: bool,
}

impl Render for Todos {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        screen("Todos")
            .large_title(false)
            .leading(
                bar_button(if self.editing { "Done" } else { "Edit" })
                    .on_click(cx.listener(|this: &mut Todos, _, _| this.editing = !this.editing)),
            )
            .trailing(
                bar_button("Add")
                    .icon("plus")
                    .accessibility_label("Add a todo")
                    .on_click(navigate_to("/new")),
            )
            .child(text(if self.editing { "editing" } else { "list" }))
    }
}

struct Plain(&'static str);

impl Render for Plain {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen(self.0)
    }
}

fn todos_app() -> Harness {
    Harness::at("/", |cx| {
        let router = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Todos::default()))
            .route("/new", |cx, _| cx.new_entity(|_| Plain("New")))
            .build(cx);
        cx.set_root(router);
    })
}

fn screen_titled(h: &Harness, title: &str) -> u64 {
    h.host
        .find(|n| n.kind == NodeKind::Screen && n.text == title)
        .unwrap_or_else(|| panic!("no screen {title}"))
}

#[test]
fn screens_send_their_bar_options_as_config() {
    let h = todos_app();
    let todos = screen_titled(&h, "Todos");
    let c = ScreenConfig::decode(&h.host.get(todos).config);
    assert_eq!(c.large_title, Some(false));
    assert_eq!(c.items.len(), 2);
    assert_eq!(c.items[0].placement, BarPlacement::Leading);
    assert_eq!(c.items[0].title, "Edit");
    assert_eq!(c.items[1].placement, BarPlacement::Trailing);
    assert_eq!(
        (c.items[1].icon.as_str(), c.items[1].spoken()),
        ("plus", "Add a todo")
    );
    assert_eq!(c.leading().map(|(i, _)| i).collect::<Vec<_>>(), [0]);
    assert_eq!(c.trailing().map(|(i, _)| i).collect::<Vec<_>>(), [1]);
}

#[test]
fn a_screen_without_options_has_no_config() {
    let mut h = todos_app();
    h.rt.app.navigate("/new");
    h.render();
    assert_eq!(h.host.get(screen_titled(&h, "New")).config, "");
}

#[test]
fn a_tap_on_a_bar_button_runs_its_handler() {
    let mut h = todos_app();
    let todos = screen_titled(&h, "Todos");
    // Button 0 (leading): the screen's own state changes, its title follows.
    h.dispatch(UiEvent::Change(todos, 0.));
    h.host.by_text("editing");
    let c = ScreenConfig::decode(&h.host.get(todos).config);
    assert_eq!(c.items[0].title, "Done");
    // Button 1 (trailing): navigates.
    h.dispatch(UiEvent::Change(todos, 1.));
    assert_eq!(h.rt.app.current_path(), Some("/new"));
    // An index without a button does nothing.
    h.dispatch(UiEvent::Change(todos, 7.));
    assert_eq!(h.rt.app.current_path(), Some("/new"));
}

#[test]
fn bar_configs_round_trip_and_keep_to_one_line_per_button() {
    let c = ScreenConfig {
        large_title: Some(true),
        items: vec![prism_core::navbar::BarItem {
            placement: BarPlacement::Leading,
            title: "Two\nlines\tand a tab".into(),
            icon: String::new(),
            label: String::new(),
        }],
    };
    let encoded = c.encode();
    assert_eq!(encoded, "large\nL\t\tTwo lines and a tab\t");
    let back = ScreenConfig::decode(&encoded);
    assert_eq!(back.items[0].title, "Two lines and a tab");
    assert_eq!(back.large_title, Some(true));
    assert_eq!(ScreenConfig::decode(""), ScreenConfig::default());
    assert_eq!(ScreenConfig::default().encode(), "");
}

fn tints(h: &Harness) -> Vec<(NodeKind, Option<Color>)> {
    h.host
        .mounted()
        .into_iter()
        .map(|id| h.host.get(id))
        .filter(|n| matches!(n.kind, NodeKind::Tabs | NodeKind::Stack))
        .map(|n| (n.kind, n.style.as_ref().and_then(|s| s.tint)))
        .collect()
}

#[test]
fn the_tab_bar_and_header_bars_take_the_router_tint_or_the_accent_color() {
    let routes = |b: prism_core::RouterBuilder| {
        b.tab("One", "1.circle", "/")
            .tab("Two", "2.circle", "/two")
            .route("/", |cx, _| cx.new_entity(|_| Plain("One")))
            .route("/two", |cx, _| cx.new_entity(|_| Plain("Two")))
    };
    // No tint: the platform's.
    let h = Harness::at("/", move |cx| {
        let router = routes(Router::new()).build(cx);
        cx.set_root(router);
    });
    assert!(
        tints(&h).iter().all(|(_, t)| t.is_none()),
        "{:?}",
        tints(&h)
    );

    // The app's accent color, and its changes.
    let mut h = Harness::at("/", move |cx| {
        cx.set_accent_color(Some(rgb(0x5B5BD6)));
        let router = routes(Router::new()).build(cx);
        cx.set_root(router);
    });
    let all = tints(&h);
    assert_eq!(all.len(), 3, "the tab bar and both stacks");
    assert!(all.iter().all(|(_, t)| *t == Some(rgb(0x5B5BD6))));
    h.rt.app.set_accent_color(Some(rgb(0xFF0000)));
    h.render();
    assert!(tints(&h).iter().all(|(_, t)| *t == Some(rgb(0xFF0000))));

    // The router's own tint wins.
    let h = Harness::at("/", move |cx| {
        cx.set_accent_color(Some(rgb(0x5B5BD6)));
        let router = routes(Router::new()).tint(rgb(0x00AA00)).build(cx);
        cx.set_root(router);
    });
    assert!(tints(&h).iter().all(|(_, t)| *t == Some(rgb(0x00AA00))));
}
