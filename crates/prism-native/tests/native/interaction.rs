//! Overlays, layers, pointer / key routing and their remote ops (#17, #19, #25).

use std::rc::Rc;

use prism_core::{
    App, Backend, ClickEvent, Context, DeviceCall, DragPhase, FluentBuilder, IntoElement, KeyEvent,
    NodeKind, ParentElement, PickFiles, Point, PointerPhase, Render, Styled, UiEvent, dialog, div,
    popover, sheet, text,
};
use prism_native::Widgets;
use prism_native::remote::{RemoteWidgets, device, parse_input_event};
use serde_json::json;

use crate::common::Harness;

struct Screen {
    kind: u8,
    open: bool,
}

impl Render for Screen {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .p(10.)
            .gap(4.)
            .child(text("Filters"))
            .child(text("Sort"));
        let overlay = match self.kind {
            0 => popover(content),
            // A dialog the app sizes (#79): wider and taller than its
            // content, its width kept inside a 400 wide window.
            2 => dialog(content).size(520., 200.),
            3 => sheet(content).size(120., 100.),
            _ => sheet(content),
        };
        div()
            .child(text("top"))
            .child(overlay.open(self.open))
            .child(text("below"))
    }
}

#[test]
fn overlays_take_no_room_and_lay_their_content_out_alone() {
    let mut h = Harness::root(|| Screen {
        kind: 0,
        open: true,
    });
    let f = h.layout(400., 600.);
    // The overlay does not push "below" down.
    assert_eq!(f.frame(f.by_text("below")).1, 20.);
    let ov = f.of_kind(NodeKind::Overlay)[0];
    let content = f.children[&ov][0];
    // A popover fits its content: 300 wide (the area), 10 + 20 + 4 + 20 + 10 high.
    assert_eq!(f.frame(content), (10., 20., 300., 64.));
    assert_eq!(f.content[&ov], (300., 64.));
    assert_eq!(f.frame(f.by_text("Sort")), (10., 34., 280., 20.));
}

#[test]
fn sheets_fill_their_area() {
    let mut h = Harness::root(|| Screen {
        kind: 1,
        open: true,
    });
    let f = h.layout(400., 600.);
    let ov = f.of_kind(NodeKind::Overlay)[0];
    let content = f.children[&ov][0];
    assert_eq!(f.frame(content), (10., 20., 300., 400.));
    assert_eq!(f.content[&ov], (300., 400.));
}

#[test]
fn an_overlay_takes_the_size_the_app_asks_for() {
    let mut h = Harness::root(|| Screen {
        kind: 2,
        open: true,
    });
    let f = h.layout(400., 600.);
    let ov = f.of_kind(NodeKind::Overlay)[0];
    let content = f.children[&ov][0];
    // 520 kept inside the window (less 16), 200 high, filled.
    assert_eq!(f.frame(content), (10., 20., 384., 200.));
    assert_eq!(f.content[&ov], (384., 200.));
    // Sheets keep the platform's area.
    let mut h = Harness::root(|| Screen {
        kind: 3,
        open: true,
    });
    let f = h.layout(400., 600.);
    let ov = f.of_kind(NodeKind::Overlay)[0];
    assert_eq!(f.content[&ov], (300., 400.));
}

struct Bar;

impl Render for Bar {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .justify_end()
            .child(text("toast").id("toast").autofocus())
    }
}

struct Root;

impl Render for Root {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(text("app")).child(
            div()
                .id("pad")
                .on_click(Rc::new(|_: &ClickEvent, _: &mut App| {}))
                .on_drag(Rc::new(|_, _: &mut App| {}))
                .on_key_down(Rc::new(|_: &KeyEvent, _: &mut App| {}))
                .when(true, |d| d.child(text("pad"))),
        )
    }
}

#[test]
fn layers_cover_the_window_and_events_reach_the_widgets() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Root);
        cx.set_root(root);
        let bar = cx.new_entity(|_| Bar);
        cx.add_layer(bar);
    });
    h.r.layout(
        400.,
        600.,
        prism_native::Insets {
            top: 0.,
            right: 0.,
            bottom: 30.,
            left: 0.,
        },
    );
    let f = h.w();
    assert_eq!(f.layers.len(), 1);
    let layer = f.layers[0];
    assert_eq!(f.frame(layer), (0., 0., 400., 600.));
    // Inside the safe area, at the bottom.
    assert_eq!(f.frame(f.by_text("toast")), (0., 550., 400., 20.));
    assert_eq!(f.focused, [f.by_text("toast")]);
    let pad = f.by_text("pad");
    let pad_view = *f.children.iter().find(|(_, c)| c.contains(&pad)).unwrap().0;
    assert_eq!(f.pointer[&pad_view], 4);
    assert_eq!(f.keys[&pad_view], (true, false));
    h.rt.app.scroll_into_view("pad");
    let p = h.rt.poll();
    h.r.apply(&p);
    assert_eq!(h.w().revealed, [pad_view]);
}

#[test]
fn remote_ops_and_input_events() {
    let mut w = RemoteWidgets::new(|_| (0., 0.));
    w.set_pointer_events(&3, 5);
    w.set_key_events(&3, true, false);
    w.focus(&3);
    w.scroll_into_view(&3);
    w.set_layers(&[9, 10]);
    assert_eq!(
        w.ops,
        [
            json!({"op": "pointer", "id": 3, "mask": 5}),
            json!({"op": "keys", "id": 3, "local": true, "global": false}),
            json!({"op": "focus", "id": 3}),
            json!({"op": "reveal", "id": 3}),
            json!({"op": "layers", "ids": [9, 10]}),
        ]
    );
    // Overlays get the area the host reported, else the root's size.
    w.set_root(&1);
    w.set_frame(&1, 0., 0., 320., 480.);
    assert_eq!(w.overlay_viewport(&5, &6).width, 320.);
    w.viewports.insert(5, (200., 300.));
    assert_eq!(w.overlay_viewport(&5, &6).height, 300.);

    let ev = |v| parse_input_event(&v).unwrap();
    assert_eq!(ev(json!({"t": "click", "id": 4})), UiEvent::Click(4));
    // Wheel and pinch from hosts (#133).
    assert!(matches!(
        ev(json!({"t": "wheel", "id": 4, "dx": 1, "dy": 2, "phase": "start", "precise": true})),
        UiEvent::Wheel(4, w) if w.delta == Point::new(1., 2.) && w.precise
    ));
    assert!(matches!(
        ev(json!({"t": "magnify", "id": 4, "scale": 0.5})),
        UiEvent::Magnify(4, m) if m.scale == 0.5
    ));
    // Drop targets (#111): the op, then the host's copies of dropped files.
    let mut d = RemoteWidgets::new(|_| (0., 0.));
    d.set_drop_target(&3, Some(&["image/*".to_string()]));
    d.set_drop_target(&3, None);
    assert_eq!(
        d.ops,
        [
            json!({"op": "drop", "id": 3, "types": ["image/*"]}),
            json!({"op": "drop", "id": 3, "types": null}),
        ]
    );
    assert_eq!(
        ev(json!({"t": "drop", "id": 3, "files": [
            {"path": "/cache/a.png", "name": "cat.png", "mime": "image/png", "size": 3}
        ]})),
        UiEvent::DropFiles(
            3,
            vec![prism_core::PickedFile {
                path: "/cache/a.png".into(),
                name: "cat.png".into(),
                mime: "image/png".into(),
                size: 3,
            }]
        )
    );
    assert_eq!(
        prism_core::Interaction::from_name("drop_hover"),
        Some(prism_core::Interaction::DropHover)
    );
    assert_eq!(
        ev(json!({"t": "click", "id": 4, "x": 1.5, "y": 2})),
        UiEvent::ClickAt(4, Point::new(1.5, 2.))
    );
    let UiEvent::Pointer(4, p) =
        ev(json!({"t": "pointer", "id": 4, "phase": "leave", "x": 1, "y": 2}))
    else {
        panic!()
    };
    assert_eq!(p.phase, PointerPhase::Leave);
    let UiEvent::Drag(4, d) =
        ev(json!({"t": "drag", "id": 4, "phase": "end", "x": 1, "y": 2, "dx": 3, "dy": -4}))
    else {
        panic!()
    };
    assert_eq!(d.phase, DragPhase::End);
    assert_eq!(d.position(), Point::new(4., -2.));
    let UiEvent::Key(None, k) = ev(json!({"t": "key", "id": 0, "key": "ArrowUp", "mods": 1}))
    else {
        panic!()
    };
    assert!(k.is("ArrowUp") && k.modifiers.shift && !k.repeat);
    assert!(parse_input_event(&json!({"t": "drag", "id": 4, "phase": "fly"})).is_none());
    assert_eq!(
        device::call(
            1,
            DeviceCall::PickFiles(PickFiles::new().types(["image/*"]).multiple())
        ),
        json!({"op": "device", "id": 1, "call": "pick_files", "types": ["image/*"], "multiple": true})
    );
}

// --- Web views (#62) ----------------------------------------------------------------------

struct Preview;

impl Render for Preview {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().flex_row().child(text("form").w(100.)).child(
            prism_core::web_view("https://example.com")
                .id("preview")
                .flex_1(),
        )
    }
}

#[test]
fn web_views_are_sized_by_their_style_and_take_messages() {
    let mut h = Harness::root(|| Preview);
    let f = h.layout(400., 300.);
    let web = f.of_kind(NodeKind::WebView)[0];
    assert!(!f.measured.contains_key(&web), "no natural size");
    assert_eq!(f.frame(web), (100., 0., 300., 300.));
    assert_eq!(f.texts[&web], "https://example.com");
    h.rt.app.post_web_message("preview", &"hi");
    h.rt.app.reload_web_view("preview");
    h.rt.app.refresh();
    h.rerender();
    assert_eq!(h.w().web_messages, [(web, "\"hi\"".to_string())]);
    assert_eq!(h.w().web_reloads, [web]);

    // Hosts in another language get them as ops, and report back as events.
    let mut w = RemoteWidgets::new(|_| (0., 0.));
    w.post_web_message(&7, "[1]");
    w.reload_web_view(&7);
    assert_eq!(
        w.ops,
        [
            json!({"op": "webpost", "id": 7, "data": "[1]"}),
            json!({"op": "webreload", "id": 7})
        ]
    );
    assert_eq!(
        parse_input_event(
            &json!({"t": "webmsg", "id": 7, "data": "{\"a\":1}", "origin": "https://example.com"})
        ),
        Some(UiEvent::WebMessage(
            7,
            "{\"a\":1}".into(),
            "https://example.com".into()
        ))
    );
    assert_eq!(
        parse_input_event(&json!({"t": "webload", "id": 7, "url": "https://example.com/"})),
        Some(UiEvent::WebLoad(7, "https://example.com/".into()))
    );
}
