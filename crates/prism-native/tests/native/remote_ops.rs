//! Golden JSON for every op `RemoteWidgets` sends to the watchOS (SwiftUI)
//! and Android (Java) hosts: those hosts parse exactly these shapes.

use std::time::Duration;

use prism_core::{
    Accessibility, Accuracy, Appearance, Backend, Color, DeviceCall, DeviceError, DeviceEvent,
    DeviceValue, Direction, FontWeight, Haptic, Locale, NodeKind, Path, Permission,
    PermissionState, PickImages, Pose, RenderEnvironment, Share, Style, TextAlign, Transform,
    Transition, menu::MenuSpec, rgb,
};
use prism_native::remote::{MeasureRequest, RemoteWidgets, device};
use prism_native::{NativeRenderer, Widgets};
use serde_json::{Value, json};

/// Echoes the request in the size: width = chars × 10 (+ font, weight
/// checks below), height = font size.
fn measure(r: &MeasureRequest) -> (f32, f32) {
    let w = r.text.chars().count() as f32 * 10.;
    match r.max_width {
        Some(m) if w > m => (m, r.font_size * 2.),
        _ => (w, r.font_size),
    }
}

fn last(w: &RemoteWidgets) -> Value {
    w.ops.last().cloned().expect("an op")
}

fn linear() -> Transition {
    Transition::linear(250).delay(500)
}

const ANIM: fn() -> Value = || json!({ "d": 0.25, "delay": 0.5, "ease": [0., 0., 1., 1.] });

#[test]
fn tree_ops() {
    let mut w = RemoteWidgets::new(measure);
    w.create(7, NodeKind::TextInput);
    assert_eq!(
        last(&w),
        json!({"op": "create", "id": 7, "kind": "text_input"})
    );
    w.set_text(&7, "héllo \"x\"");
    assert_eq!(
        last(&w),
        json!({"op": "text", "id": 7, "text": "héllo \"x\""})
    );
    w.set_placeholder(&7, "Name");
    assert_eq!(
        last(&w),
        json!({"op": "placeholder", "id": 7, "text": "Name"})
    );
    w.set_clickable(&7, true);
    assert_eq!(last(&w), json!({"op": "clickable", "id": 7, "on": true}));
    w.set_children(&1, &[3, 2]);
    assert_eq!(
        last(&w),
        json!({"op": "children", "id": 1, "children": [3, 2]})
    );
    w.set_children(&1, &[]);
    assert_eq!(last(&w), json!({"op": "children", "id": 1, "children": []}));
    w.set_root(&1);
    assert_eq!(last(&w), json!({"op": "root", "id": 1}));
    w.set_value(&7, 2.);
    assert_eq!(last(&w), json!({"op": "value", "id": 7, "value": 2.0}));
    w.set_hidden(&7, true);
    assert_eq!(last(&w), json!({"op": "display", "id": 7, "hidden": true}));
    w.set_content_size(&7, 100., 2000.5);
    assert_eq!(
        last(&w),
        json!({"op": "content", "id": 7, "size": [100.0, 2000.5]})
    );
    w.set_swipe(&7, false, true);
    assert_eq!(
        last(&w),
        json!({"op": "swipe", "id": 7, "left": false, "right": true})
    );
    w.set_scroll_events(&7, true);
    assert_eq!(last(&w), json!({"op": "scrollev", "id": 7, "on": true}));
    w.destroy(&7);
    assert_eq!(last(&w), json!({"op": "destroy", "id": 7}));
}

#[test]
fn frame_ops() {
    let mut w = RemoteWidgets::new(measure);
    w.set_frame(&3, 1., 2.5, 30., 40.);
    assert_eq!(
        last(&w),
        json!({"op": "frame", "id": 3, "f": [1.0, 2.5, 30.0, 40.0]})
    );
    w.set_frame_animated(&3, 0., 0., 10., 20., linear());
    assert_eq!(
        last(&w),
        json!({"op": "frame", "id": 3, "f": [0., 0., 10., 20.], "anim": ANIM()})
    );
}

#[test]
fn style_ops() {
    let mut w = RemoteWidgets::new(measure);
    let style = Style {
        background: Some(Color::adaptive(rgb(0xff0000), rgb(0x0000ff))),
        corner_radius: 8.,
        border_width: 1.,
        border_color: Some(rgb(0x00ff00)),
        opacity: 0.5,
        text_color: Some(Color::WHITE),
        font_size: 20.,
        font_weight: FontWeight::Semibold,
        text_align: TextAlign::Center,
        transform: Transform {
            x: 4.,
            y: -2.,
            scale: 1.5,
            rotate: 90.,
        },
        ..Style::default()
    };
    w.set_style(&5, &style);
    let golden = json!({"op": "style", "id": 5, "s": {
        "bg": [1., 0., 0., 1., 0., 0., 1., 1.],
        "radius": 8.,
        "bw": 1.,
        "bc": [0., 1., 0., 1., 0., 1., 0., 1.],
        "opacity": 0.5,
        "tf": [4., -2., 1.5, 90.],
        "color": [1., 1., 1., 1., 1., 1., 1., 1.],
        "font": 20.,
        "weight": 600,
        "talign": "center",
    }});
    assert_eq!(last(&w), golden);

    // Defaults: no colors, identity transform, no border color without a border.
    w.set_style(
        &5,
        &Style {
            border_color: Some(rgb(0x00ff00)),
            ..Style::default()
        },
    );
    let s = &last(&w)["s"];
    assert_eq!(s["bg"], Value::Null);
    assert_eq!(s["bc"], Value::Null, "border color only with a border");
    assert_eq!(s["tf"], Value::Null);
    assert_eq!(s["weight"], 400);
    assert_eq!(s["talign"], "left");

    w.set_style_animated(&5, &Style::default(), linear());
    assert_eq!(last(&w)["op"], "style");
    assert_eq!(last(&w)["anim"], ANIM());
}

#[test]
fn motion_ops() {
    let mut w = RemoteWidgets::new(measure);
    let to = Style {
        opacity: 0.,
        transform: Transform {
            x: 10.,
            ..Transform::IDENTITY
        },
        ..Style::default()
    };
    w.animate_exit(&1, &9, &to, linear());
    assert_eq!(
        last(&w),
        json!({"op": "exit", "id": 9, "parent": 1,
               "to": {"o": 0., "tf": [10., 0., 1., 0.]}, "anim": ANIM()})
    );
    let rest = Style {
        opacity: 1.,
        ..Style::default()
    };
    w.set_repeat(
        &9,
        &rest,
        Some((Pose::new().scale(2.).opacity(0.5), linear())),
    );
    assert_eq!(
        last(&w),
        json!({"op": "repeat", "id": 9,
               "from": {"o": 1., "tf": null},
               "to": {"o": 0.5, "tf": [0., 0., 2., 0.]}, "anim": ANIM()})
    );
    w.set_repeat(&9, &rest, None);
    assert_eq!(last(&w), json!({"op": "repeat", "id": 9, "to": null}));
}

#[test]
fn drawing_ops() {
    let mut w = RemoteWidgets::new(measure);
    let mut p = Path::new()
        .move_to(0., 0.)
        .line_to(10., 0.)
        .curve_to((1., 2.), (3., 4.), (5., 6.))
        .close();
    p.fill = Some(rgb(0xff0000));
    p.stroke = None;
    p.width = 2.;
    p.dash = Some((4., 2.));
    w.set_drawing(&4, &[p]);
    assert_eq!(
        last(&w),
        json!({"op": "draw", "id": 4, "paths": [{
            "d": [0., 0., 0., 1., 10., 0., 2., 1., 2., 3., 4., 5., 6., 3.],
            "fill": [1., 0., 0., 1., 1., 0., 0., 1.],
            "stroke": null,
            "w": 2.,
            "round": false,
            "dash": [4., 2.],
        }]})
    );
}

#[test]
fn chrome_ops() {
    let mut w = RemoteWidgets::new(measure);
    w.set_accessibility(
        &2,
        &Accessibility {
            label: Some("Close".into()),
            hidden: false,
            ..Default::default()
        },
    );
    assert_eq!(
        last(&w),
        json!({"op": "a11y", "id": 2, "label": "Close", "hidden": false, "role": null, "value": null})
    );
    w.set_icon(&2, "heart.fill");
    assert_eq!(
        last(&w),
        json!({"op": "icon", "id": 2, "icon": "heart.fill", "glyph": "♥"})
    );
    let menu = MenuSpec {
        title: "M".into(),
        items: vec![prism_core::menu::MenuItemSpec::Separator],
    };
    w.set_menu(&2, Some(&menu));
    assert_eq!(
        last(&w),
        json!({"op": "menu", "id": 2, "menu": {"title": "M", "items": [{"t": "sep"}]}})
    );
    w.set_menu(&2, None);
    assert_eq!(last(&w), json!({"op": "menu", "id": 2, "menu": null}));
    w.set_environment(&RenderEnvironment {
        lang: "ar".into(),
        direction: Direction::Rtl,
        text_scale: 1.,
        reduce_motion: false,
        back: "رجوع".into(),
        close: "إغلاق".into(),
    });
    assert_eq!(
        last(&w),
        json!({"op": "env", "lang": "ar", "rtl": true, "back": "رجوع", "close": "إغلاق"})
    );
    for (a, mode) in [
        (Appearance::System, "system"),
        (Appearance::Light, "light"),
        (Appearance::Dark, "dark"),
    ] {
        w.set_appearance(a);
        assert_eq!(last(&w), json!({"op": "appearance", "mode": mode}));
    }
}

/// (kind, text, font size, weight, max width) of each measure request.
type Seen = Vec<(String, String, f32, i32, Option<f32>)>;

thread_local! {
    static SEEN: std::cell::RefCell<Seen> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn recording(r: &MeasureRequest) -> (f32, f32) {
    SEEN.with(|s| {
        s.borrow_mut().push((
            r.kind.into(),
            r.text.into(),
            r.font_size,
            r.weight,
            r.max_width,
        ))
    });
    measure(r)
}

#[test]
fn measure_requests_carry_kind_text_font_and_width() {
    SEEN.with(|s| s.borrow_mut().clear());
    let mut w = RemoteWidgets::new(recording);
    w.create(1, NodeKind::Text);
    w.set_style(
        &1,
        &Style {
            font_size: 12.,
            font_weight: FontWeight::Bold,
            ..Style::default()
        },
    );
    w.set_text(&1, "a bb cccc");
    assert_eq!(w.measure(&1, Some(50.)), (50., 24.));
    // min-content: the longest word.
    assert_eq!(w.measure_min_content(&1), 40.);
    w.create(2, NodeKind::Timer);
    w.measure(&2, None);
    w.create(3, NodeKind::Image);
    w.set_icon(&3, "star");
    w.measure(&3, None);
    let seen = SEEN.with(|s| s.borrow().clone());
    assert_eq!(
        seen[0],
        ("text".into(), "a bb cccc".into(), 12., 700, Some(50.))
    );
    let words: Vec<&str> = seen[1..4].iter().map(|s| s.1.as_str()).collect();
    assert_eq!(words, ["a", "bb", "cccc"]);
    assert_eq!(seen[4].0, "timer");
    assert_eq!(seen[4].1, "00:00", "timers measure their widest text");
    assert_eq!((seen[5].0.as_str(), seen[5].1.as_str()), ("image", "star"));
    // The full text is back after measuring words.
    assert_eq!(w.measure(&1, None), (90., 12.));
}

#[test]
fn screen_viewports_prefer_the_host_report() {
    let mut w = RemoteWidgets::new(measure);
    w.set_frame(&1, 0., 0., 300., 600.);
    assert_eq!(
        w.screen_viewport(&1, &2).height,
        600.,
        "the container's frame"
    );
    w.viewports.insert(1, (300., 500.));
    assert_eq!(
        w.screen_viewport(&1, &2).height,
        500.,
        "what the host reported"
    );
    w.viewports.insert(2, (300., 400.));
    assert_eq!(w.screen_viewport(&1, &2).height, 400., "a sheet's own size");
    w.destroy(&1);
    w.viewports.remove(&2);
    assert_eq!(w.screen_viewport(&1, &2).width, 0.);
}

#[test]
fn a_render_becomes_an_ordered_op_stream() {
    use prism_core::{
        AppInit, Context, IntoElement, ParentElement, Render, Runtime, Styled, button, div,
    };
    struct App;
    impl Render for App {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div().p(4.).child(button("+1"))
        }
    }
    let init: AppInit = std::sync::Arc::new(|cx: &mut prism_core::App| {
        let e = cx.new_entity(|_| App);
        cx.set_root(e);
    });
    let mut rt = Runtime::new(&init);
    let mut r = NativeRenderer::new(RemoteWidgets::new(measure));
    r.apply(&rt.render());
    r.layout(200., 100., Default::default());
    let ops: Vec<&str> = r
        .widgets
        .ops
        .iter()
        .map(|o| o["op"].as_str().unwrap())
        .collect();
    assert_eq!(
        ops[0], "env",
        "environment first: text is created at the right scale"
    );
    let pos = |name: &str| ops.iter().position(|o| *o == name).unwrap();
    assert!(pos("create") < pos("style"));
    assert!(pos("children") < pos("root"));
    assert!(pos("root") < pos("frame"));
    let frames: Vec<&Value> = r
        .widgets
        .ops
        .iter()
        .filter(|o| o["op"] == "frame")
        .collect();
    assert_eq!(frames[0]["f"], json!([0., 0., 200., 100.]));
    // "+1": 2 chars × 10, 16 pt default font, inside the 4 pt padding.
    assert_eq!(frames[1]["f"], json!([4., 4., 192., 16.]));
    let _ = Locale::new("en");
}

#[test]
fn device_calls_golden() {
    let cases: Vec<(DeviceCall, Value)> = vec![
        (
            DeviceCall::PermissionStatus(Permission::Camera),
            json!({"call": "permission_status", "permission": "camera"}),
        ),
        (
            DeviceCall::RequestPermission(Permission::LocationAlways),
            json!({"call": "request_permission", "permission": "location_always"}),
        ),
        (
            DeviceCall::CurrentLocation(Accuracy::Best),
            json!({"call": "location", "accuracy": "best"}),
        ),
        (
            DeviceCall::WatchLocation(Accuracy::Coarse),
            json!({"call": "watch_location", "accuracy": "coarse"}),
        ),
        (
            DeviceCall::WatchMotion(Duration::from_millis(250)),
            json!({"call": "watch_motion", "interval": 250}),
        ),
        (DeviceCall::StopWatch, json!({"call": "stop"})),
        (
            DeviceCall::PickImages(PickImages {
                limit: 3,
                camera: true,
            }),
            json!({"call": "pick_images", "limit": 3, "camera": true}),
        ),
        (DeviceCall::ReadClipboard, json!({"call": "paste"})),
        (
            DeviceCall::Haptic(Haptic::Success),
            json!({"call": "haptic", "kind": "success"}),
        ),
        (
            DeviceCall::CopyText("hi".into()),
            json!({"call": "copy", "text": "hi"}),
        ),
        (
            DeviceCall::Share(Share {
                text: Some("t".into()),
                url: None,
                files: vec!["/a.png".into()],
            }),
            json!({"call": "share", "text": "t", "url": null, "files": ["/a.png"]}),
        ),
        (
            DeviceCall::OpenUrl("https://x.y".into()),
            json!({"call": "open_url", "url": "https://x.y"}),
        ),
    ];
    for (call, mut golden) in cases {
        golden["op"] = json!("device");
        golden["id"] = json!(42);
        assert_eq!(device::call(42, call), golden);
    }
}

#[test]
fn device_replies_and_events_parse() {
    assert_eq!(
        device::reply(&json!({"err": "denied"})),
        Err(DeviceError::Denied)
    );
    assert_eq!(
        device::reply(&json!({"err": "boom"})),
        Err(DeviceError::Failed("boom".into()))
    );
    assert_eq!(
        device::reply(&json!({"permission": "granted"})),
        Ok(DeviceValue::Permission(PermissionState::Granted))
    );
    let Ok(DeviceValue::Location(l)) = device::reply(&json!({
        "location": {"latitude": 48.5, "longitude": 2.25, "accuracy": 5.0, "speed": 1.0}
    })) else {
        panic!("a location")
    };
    assert_eq!((l.latitude, l.longitude, l.accuracy), (48.5, 2.25, 5.));
    assert_eq!((l.altitude, l.speed, l.heading), (None, Some(1.), None));
    // A location without coordinates is not one.
    assert_eq!(
        device::reply(&json!({"location": {"latitude": 1.0}})),
        Ok(DeviceValue::Done)
    );
    let Ok(DeviceValue::Files(files)) = device::reply(&json!({
        "files": [{"path": "/p", "name": "n.jpg", "mime": "image/jpeg", "size": 12}, {}]
    })) else {
        panic!("files")
    };
    assert_eq!(files[0].name, "n.jpg");
    assert_eq!(files[0].size, 12);
    assert_eq!(files[1].path, "", "missing fields are empty");
    assert_eq!(
        device::reply(&json!({"text": null})),
        Ok(DeviceValue::Text(None))
    );
    assert_eq!(
        device::reply(&json!({"text": "clip"})),
        Ok(DeviceValue::Text(Some("clip".into())))
    );
    assert_eq!(device::reply(&json!({})), Ok(DeviceValue::Done));

    assert_eq!(
        device::event(&json!({"err": "unavailable"})),
        DeviceEvent::Error(DeviceError::Unavailable)
    );
    assert!(matches!(
        device::event(&json!({"location": {"latitude": 1.0, "longitude": 2.0}})),
        DeviceEvent::Location(_)
    ));
    let DeviceEvent::Motion(m) = device::event(&json!({
        "acceleration": [1.0, 2.0, 3.0], "gravity": [0.0, -1.0]
    })) else {
        panic!("motion")
    };
    assert_eq!(m.acceleration, [1., 2., 3.]);
    assert_eq!(m.gravity, [0., -1., 0.], "short arrays are padded");
    assert_eq!(m.attitude, [0., 0., 0.]);
}

#[test]
fn style_sends_rounded_corners_only_when_some_are_square() {
    let mut w = RemoteWidgets::new(measure);
    w.set_style(
        &3,
        &Style {
            corner_radius: 6.,
            rounded_corners: prism_core::Corners::TOP,
            ..Default::default()
        },
    );
    assert_eq!(last(&w)["s"]["corners"], json!(3));
    w.set_style(
        &3,
        &Style {
            corner_radius: 6.,
            ..Default::default()
        },
    );
    assert!(last(&w)["s"].get("corners").is_none());
}

#[test]
fn sticky_horizontal_scroll_and_baseline_ops() {
    let mut w = RemoteWidgets::new(|r| match r.kind {
        // A host that knows baselines: 80% of the font size from the top.
        "baseline" => (0., r.font_size * 0.8),
        _ => measure(r),
    });
    w.create(4, NodeKind::Scroll);
    let s = Style {
        scroll_axis: prism_core::ScrollAxis::Horizontal,
        ..Style::default()
    };
    w.set_style(&4, &s);
    assert_eq!(last(&w)["s"]["hscroll"], json!(true));
    w.set_style(&4, &Style::default());
    assert_eq!(last(&w)["s"].get("hscroll"), None, "vertical: no key");

    let sticky = prism_native::Sticky {
        top: 20.,
        inset: 8.,
        max_offset: 300.,
    };
    w.set_sticky(&9, Some((&4, sticky)));
    assert_eq!(
        last(&w),
        json!({"op": "sticky", "id": 9, "scroll": 4, "top": 20., "inset": 8., "max": 300.})
    );
    w.set_sticky(&9, None);
    assert_eq!(last(&w), json!({"op": "sticky", "id": 9, "scroll": null}));

    w.create(5, NodeKind::Text);
    w.set_style(
        &5,
        &Style {
            font_size: 20.,
            ..Style::default()
        },
    );
    assert_eq!(w.baseline(&5, 100., 24.), Some(16.));
    w.create(6, NodeKind::View);
    assert_eq!(w.baseline(&6, 100., 24.), None, "no text, no baseline");
    let mut old = RemoteWidgets::new(|r| match r.kind {
        "baseline" => (0., 0.),
        _ => measure(r),
    });
    old.create(5, NodeKind::Text);
    assert_eq!(old.baseline(&5, 100., 24.), None, "unknown: estimated");
}
