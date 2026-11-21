//! Islands: client-side Web Components (WASM or JS) in the tree.

use prism_core::prelude::*;
use prism_core::{IslandSource, NodeKind, Patch, Platform, Runtime, RuntimeOptions, UiEvent};
use prism_testing::{Harness, mount};

struct Dash {
    points: Vec<f64>,
    label: Option<String>,
    selected: Option<(usize, f64)>,
    tag: &'static str,
}

impl Render for Dash {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let chart = island(self.tag)
            .prop("points", &self.points)
            .when_some(self.label.clone(), |i, l| i.prop("label", &l))
            .on(
                "select",
                cx.listener(|this, e: &IslandEvent, _| this.selected = e.detail::<(usize, f64)>()),
            )
            .h(200.)
            .child(text("fallback"));
        div().child(chart).child(
            web_component("time-ago")
                .module("time-ago.js")
                .prop("datetime", &1.5),
        )
    }
}

fn dash() -> Harness {
    Harness::new(|cx| {
        let e = cx.new_entity(|_| Dash {
            points: vec![1., 2.],
            label: Some("a".into()),
            selected: None,
            tag: "chart",
        });
        mount(cx, e);
    })
}

fn chart(h: &Harness) -> u64 {
    h.host
        .find(|n| n.island.as_deref() == Some("prism-island-chart"))
        .expect("the chart is mounted")
}

#[test]
fn an_island_is_a_view_holding_its_fallback() {
    let h = dash();
    let id = chart(&h);
    let n = h.host.get(id);
    assert_eq!(n.kind, NodeKind::View);
    assert_eq!(h.host.child_texts(id), ["fallback"]);
    assert_eq!(n.island_props["points"], "[1.0,2.0]");
    assert_eq!(n.island_props["label"], "\"a\"");
    // The element right after its creation, before anything else about it.
    let at = |f: &dyn Fn(&Patch) -> bool| h.last.iter().position(f).unwrap();
    let created = at(&|p| matches!(p, Patch::Create { id: i, .. } if *i == id));
    let island = at(&|p| matches!(p, Patch::SetIsland { id: i, .. } if *i == id));
    assert_eq!(island, created + 1);
    match &h.last[island] {
        Patch::SetIsland {
            tag,
            source,
            events,
            ..
        } => {
            assert_eq!(tag, "prism-island-chart");
            assert_eq!(*source, IslandSource::Wasm("chart".into()));
            assert_eq!(events, &["select"]);
        }
        _ => unreachable!(),
    }
    let tc = h
        .host
        .find(|n| n.island.as_deref() == Some("time-ago"))
        .unwrap();
    assert!(h.last.iter().any(|p| matches!(p,
        Patch::SetIsland { id, source: IslandSource::Module(m), events, .. }
            if *id == tc && m == "time-ago.js" && events.is_empty())));
    assert_eq!(h.host.get(tc).island_props["datetime"], "1.5");
}

#[test]
fn changed_props_patch_the_live_island() {
    let mut h = dash();
    let id = chart(&h);
    // Same props: nothing for the island.
    h.update::<Dash>(|d| d.selected = Some((0, 0.)));
    assert!(
        !h.last
            .iter()
            .any(|p| matches!(p, Patch::SetIslandProps { .. })),
        "{:?}",
        h.last
    );
    h.update::<Dash>(|d| {
        d.points.push(3.);
        d.label = None;
    });
    assert_eq!(chart(&h), id, "updated in place, not re-created");
    assert!(!h.last.iter().any(|p| matches!(p, Patch::Create { .. })));
    let props: Vec<&Vec<(String, String)>> = h
        .last
        .iter()
        .filter_map(|p| match p {
            Patch::SetIslandProps { id: i, props } if *i == id => Some(props),
            _ => None,
        })
        .collect();
    // Only what changed; a removed prop is `null`.
    assert_eq!(
        props,
        [&vec![
            ("points".to_string(), "[1.0,2.0,3.0]".to_string()),
            ("label".to_string(), "null".to_string())
        ]]
    );
    assert!(!h.last.iter().any(|p| matches!(p, Patch::SetIsland { .. })));
    assert!(!h.host.get(id).island_props.contains_key("label"));
}

#[test]
fn another_island_gets_a_new_element() {
    let mut h = dash();
    let id = chart(&h);
    h.update::<Dash>(|d| d.tag = "map");
    let map = h
        .host
        .find(|n| n.island.as_deref() == Some("prism-island-map"))
        .unwrap();
    assert_ne!(map, id);
    assert!(h.host.destroyed.contains(&id));
}

#[test]
fn events_reach_the_handler_with_their_detail() {
    let mut h = dash();
    let id = chart(&h);
    h.dispatch(UiEvent::Island(id, "select".into(), "[1,2.5]".into()));
    assert_eq!(h.root::<Dash>().read(&h.rt.app).selected, Some((1, 2.5)));
    // Unknown events and other nodes are ignored.
    h.dispatch(UiEvent::Island(id, "other".into(), "1".into()));
    h.dispatch(UiEvent::Island(id + 100, "select".into(), "[0,0]".into()));
    assert_eq!(h.root::<Dash>().read(&h.rt.app).selected, Some((1, 2.5)));
}

#[test]
fn event_details_and_tags() {
    let e = IslandEvent {
        name: "select".into(),
        detail: r#"{"x":1}"#.into(),
    };
    assert_eq!(e.detail::<u32>(), None);
    assert_eq!(e.detail_value()["x"], 1);
    assert_eq!(
        prism_core::island::island_tag("Big chart"),
        "prism-island-big-chart"
    );
}

struct Native;

impl Render for Native {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        island("chart")
            .child(text("web fallback"))
            .native(text("native chart"))
    }
}

fn on(platform: Platform) -> Harness {
    Harness::with_runtime(
        |init| {
            Runtime::with_options(
                init,
                RuntimeOptions {
                    platform: Some(platform),
                    ..Default::default()
                },
            )
        },
        |cx| {
            let e = cx.new_entity(|_| Native);
            mount(cx, e);
        },
    )
}

#[test]
fn native_apps_can_show_something_else() {
    let web = on(Platform::Web);
    assert_eq!(web.host.child_texts(chart(&web)), ["web fallback"]);
    let mac = on(Platform::Macos);
    assert_eq!(mac.host.child_texts(chart(&mac)), ["native chart"]);
}

/// S20: only valid custom element names become elements (a tag can come
/// from data, and the server writes it into the page).
#[test]
fn custom_element_names_follow_the_html_grammar() {
    use prism_core::island::is_custom_element_name;
    for ok in ["time-ago", "x-1.2_b", "my-élément", "emoji-\u{1F600}", "a-"] {
        assert!(is_custom_element_name(ok), "{ok}");
    }
    for bad in [
        "",
        "timeago",
        "Time-ago",
        "-x",
        "1-x",
        "x-a b",
        "x-a>",
        "x-\"a",
        "x-a/",
        "x-a=b",
        "x-a\u{0}",
        "font-face",
        "annotation-xml",
        "missing-glyph",
    ] {
        assert!(!is_custom_element_name(bad), "{bad:?}");
    }
}
