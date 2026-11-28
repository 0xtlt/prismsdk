//! Golden JSON for every op `RemoteWidgets` sends to the watchOS (SwiftUI)
//! and Android (Java) hosts: those hosts parse exactly these shapes.

use std::time::Duration;

use prism_core::{
    Accessibility, Accuracy, Appearance, Backend, Color, DeviceCall, DeviceError, DeviceEvent,
    DeviceValue, Direction, FontWeight, Haptic, Locale, NodeKind, Path, Permission,
    PermissionState, PickImages, Pose, RenderEnvironment, Role, Share, Style, TextAlign, Transform,
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

/// An empty optional date picker (#89): its value is `null`, and it
/// measures its placeholder too (second line of the measured text).
#[test]
fn optional_date_picker_ops() {
    let mut w = RemoteWidgets::new(measure);
    w.create(3, NodeKind::DatePicker);
    w.set_config(&3, NodeKind::DatePicker, "date - - optional");
    w.set_placeholder(&3, "Not yet");
    w.set_value(&3, f64::NAN);
    assert_eq!(last(&w), json!({"op": "value", "id": 3, "value": null}));
    // "date - - optional\nNot yet": 25 characters.
    assert_eq!(w.measure(&3, None).0, 250.);
    // A picker that is not optional measures its config only.
    w.set_config(&3, NodeKind::DatePicker, "date - -");
    assert_eq!(w.measure(&3, None).0, 80.);
    assert_eq!(
        prism_native::remote::change_value(&Value::Null).map(f64::is_nan),
        Some(true)
    );
    assert_eq!(prism_native::remote::change_value(&json!(2)), Some(2.));
    assert_eq!(prism_native::remote::change_value(&json!("x")), None);
}

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
            ..Transform::IDENTITY
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
fn loops_that_start_over_and_non_uniform_scale() {
    let mut w = RemoteWidgets::new(measure);
    let mut spinner = Style::default();
    spinner.animations.restart = true;
    w.set_repeat(&3, &spinner, Some((Pose::new().rotate(360.), linear())));
    assert_eq!(
        last(&w),
        json!({"op": "repeat", "id": 3, "restart": true,
               "from": {"o": 1., "tf": null},
               "to": {"o": 1., "tf": [0., 0., 1., 360.]}, "anim": ANIM()})
    );
    // A blink: only the vertical factor changes, sent after the first four.
    w.set_repeat(
        &3,
        &Style::default(),
        Some((Pose::new().scale_y(0.1), linear())),
    );
    assert_eq!(
        last(&w)["to"]["tf"],
        json!([0., 0., 1., 0., 1., 0.10000000149011612])
    );
    assert!(last(&w).get("restart").is_none());
    let squashed = Style {
        transform: Transform {
            scale_x: 2.,
            ..Transform::IDENTITY
        },
        ..Style::default()
    };
    w.set_style(&3, &squashed);
    assert_eq!(last(&w)["s"]["tf"], json!([0., 0., 1., 0., 2., 1.]));
}

#[test]
fn even_odd_paths_and_data_url_pictures() {
    let mut w = RemoteWidgets::new(measure);
    let hole = Path::rect(0., 0., 10., 10.).fill(rgb(0)).even_odd();
    w.set_drawing(&4, &[hole, Path::rect(0., 0., 1., 1.).fill(rgb(0))]);
    let paths = &last(&w)["paths"];
    assert_eq!(paths[0]["eo"], json!(true));
    assert!(paths[1].get("eo").is_none(), "only when set");

    use prism_native::images::{ImageSources, data_url, load};
    assert_eq!(
        data_url("data:image/png;base64,TWFu").as_deref(),
        Some(&b"Man"[..])
    );
    assert_eq!(data_url("data:;base64,TWE=").as_deref(), Some(&b"Ma"[..]));
    assert_eq!(
        data_url("data:image/svg+xml,%3Csvg%2F%3E").as_deref(),
        Some(&b"<svg/>"[..])
    );
    assert_eq!(data_url("data:image/png;base64,***"), None);
    assert_eq!(data_url("https://x"), None);
    // `load` decodes them without touching the disk or the network.
    assert_eq!(
        load("data:image/gif;base64,//79", &ImageSources::default()),
        Some(vec![0xff, 0xfe, 0xfd])
    );
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

/// A drawing with more than paths (an SVG document, #342) reaches a host
/// that draws them as `items`, and any other host as its paths.
#[test]
fn a_document_s_items_reach_the_hosts_that_draw_them() {
    use prism_core::shape::{Group, Item, Text, TextRun};
    let items = [Item::Group(Group {
        opacity: 0.5,
        items: vec![
            Item::Path(Path::rect(0., 0., 10., 10.).fill(rgb(0xff0000))),
            Item::Text(Text {
                x: 2.,
                y: 8.,
                runs: vec![TextRun {
                    text: "Hi".into(),
                    size: 6.,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        ],
        ..Default::default()
    })];

    let mut plain = RemoteWidgets::new(measure);
    plain.set_drawing_items(&4, &items);
    let op = last(&plain);
    assert!(op.get("items").is_none());
    assert_eq!(op["paths"].as_array().map(Vec::len), Some(1));
    assert_eq!(op["paths"][0]["fill"][3], json!(0.5), "the group's opacity");

    let mut whole = RemoteWidgets::new(measure).with_drawing_items();
    whole.set_drawing_items(&4, &items);
    let op = last(&whole);
    assert!(op.get("paths").is_none());
    let group = &op["items"][0];
    assert_eq!((&group["t"], &group["o"]), (&json!("g"), &json!(0.5)));
    assert_eq!(group["clip"], Value::Null);
    assert_eq!(group["i"][0]["t"], json!("p"));
    let text = &group["i"][1];
    assert_eq!(
        (&text["t"], &text["a"], &text["b"]),
        (&json!("x"), &json!(0), &json!(0))
    );
    assert_eq!(text["m"], json!([1., 0., 0., 1., 0., 0.]));
    assert_eq!(text["r"][0]["s"], json!("Hi"));
    assert_eq!(text["r"][0]["z"], json!(6.));
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
        json!({"op": "a11y", "id": 2, "label": "Close", "hidden": false, "role": null, "value": null, "checked": null, "selected": false})
    );
    // A tooltip (#216): its text, or `true` while truncated.
    w.set_accessibility(
        &2,
        &Accessibility {
            tooltip: Some(prism_core::Tooltip::WhenTruncated),
            ..Default::default()
        },
    );
    assert_eq!(last(&w)["tip"], json!(true));
    // A drawn radio (#84) says whether it is the one chosen.
    w.set_accessibility(
        &3,
        &Accessibility {
            role: Some(Role::Radio),
            checked: Some(true),
            ..Default::default()
        },
    );
    assert_eq!(
        last(&w),
        json!({"op": "a11y", "id": 3, "label": null, "hidden": false, "role": "radio", "value": null, "checked": true, "selected": true})
    );
    // A disclosure (#291): its state, only when it is one.
    w.set_accessibility(
        &3,
        &Accessibility {
            role: Some(Role::Button),
            expanded: Some(false),
            ..Default::default()
        },
    );
    assert_eq!(last(&w)["expanded"], json!(false));
    // The current page of a navigation is sent as selected (#64).
    w.set_accessibility(
        &3,
        &Accessibility {
            role: Some(prism_core::Role::Link),
            current: true,
            ..Default::default()
        },
    );
    assert_eq!(last(&w)["selected"], json!(true));
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
        clear: "مسح".into(),
        expanded: "موسّع".into(),
        collapsed: "مطوي".into(),
    });
    assert_eq!(
        last(&w),
        json!({"op": "env", "lang": "ar", "rtl": true, "back": "رجوع", "close": "إغلاق", "clear": "مسح", "expanded": "موسّع", "collapsed": "مطوي"})
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
    // A timer with a longer `after` text measures it (#281), whatever
    // text it shows now.
    w.create(4, NodeKind::Timer);
    w.set_placeholder(&4, "Departed");
    w.set_text(&4, "1:05");
    w.measure(&4, None);
    let seen = SEEN.with(|s| s.borrow().clone());
    assert_eq!(
        (seen[6].0.as_str(), seen[6].1.as_str()),
        ("timer", "Departed")
    );
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
        // #246: text types as text, other bytes in base64.
        (DeviceCall::ReadClipboardItem, json!({"call": "paste_item"})),
        (
            DeviceCall::WriteClipboard(
                prism_core::ClipboardItem::new()
                    .text("hi")
                    .image(vec![0x89, b'P', b'N', b'G'], "image/png"),
            ),
            json!({"call": "copy_item", "entries": [
                {"type": "text/plain", "text": "hi"},
                {"type": "image/png", "data": "iVBORw=="},
            ]}),
        ),
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
        // Passkeys of the app's relying party (#94): the options as text.
        (
            DeviceCall::WebAuthn(prism_core::WebAuthnRequest {
                create: false,
                options: r#"{"challenge":"AQID"}"#.into(),
                conditional: false,
            }),
            json!({"call": "webauthn", "create": false, "options": r#"{"challenge":"AQID"}"#, "conditional": false}),
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
fn style_sends_hit_test_and_inert_only_when_set() {
    let mut w = RemoteWidgets::new(measure);
    w.set_style(&3, &Style::default());
    assert!(last(&w)["s"].get("hit").is_none());
    assert!(last(&w)["s"].get("inert").is_none());
    w.set_style(
        &3,
        &Style {
            hit_test: false,
            inert: true,
            ..Default::default()
        },
    );
    assert_eq!(last(&w)["s"]["hit"], json!(false));
    assert_eq!(last(&w)["s"]["inert"], json!(true));
}

#[test]
fn radio_groups_measure_their_config() {
    fn echo(r: &MeasureRequest) -> (f32, f32) {
        assert_eq!(r.kind, "radio_group");
        assert_eq!(r.text, "row\nA\nB");
        (40., 20.)
    }
    let mut w = RemoteWidgets::new(echo);
    w.create(4, NodeKind::RadioGroup);
    w.set_config(&4, NodeKind::RadioGroup, "row\nA\nB");
    assert_eq!(w.measure(&4, None), (40., 20.));
}

#[test]
fn style_ops_carry_decorations_only_when_set() {
    use prism_core::{Cursor, Gradient, Interaction, Interactions, Shadow, Styled};
    let mut w = RemoteWidgets::new(measure);
    let style = Style::default()
        .shadow(Shadow::new(0., 2., 4., rgb(0x000000)).spread(1.))
        .shadow(Shadow::new(1., 1., 0., rgb(0xff0000)).inset())
        .border(1.)
        .border_color(rgb(0x00ff00))
        .border_bottom(2., rgb(0x0000ff))
        .border_dashed()
        .clip()
        .bg_gradient(Gradient::linear(90., [rgb(0xff0000), rgb(0x0000ff)]))
        .backdrop_blur(12.)
        .cursor(Cursor::Pointer);
    w.set_style(&4, &style);
    let s = &last(&w)["s"];
    assert_eq!(
        s["shadows"],
        json!([
            {"x": 0., "y": 2., "blur": 4., "spread": 1., "color": [0., 0., 0., 1., 0., 0., 0., 1.], "inset": false},
            {"x": 1., "y": 1., "blur": 0., "spread": 0., "color": [1., 0., 0., 1., 1., 0., 0., 1.], "inset": true},
        ])
    );
    let green = json!([0., 1., 0., 1., 0., 1., 0., 1.]);
    assert_eq!(
        s["sides"],
        json!([
            [1., green],
            [1., green],
            [2., [0., 0., 1., 1., 0., 0., 1., 1.]],
            [1., green]
        ])
    );
    assert_eq!(s["bstyle"], json!("dashed"));
    assert_eq!(s["clip"], json!(true));
    assert_eq!(
        s["grad"],
        json!({"kind": "linear", "angle": 90., "stops": [
            [[1., 0., 0., 1., 1., 0., 0., 1.], 0.],
            [[0., 0., 1., 1., 0., 0., 1., 1.], 1.],
        ]})
    );
    assert_eq!(s["blur"], json!(12.));
    assert_eq!(s["cursor"], json!("pointer"));

    // A conic gradient: its start angle in `angle`.
    let conic = Style::default().bg_gradient(Gradient::conic(30., [rgb(0xff0000), rgb(0x0000ff)]));
    w.set_style(&4, &conic);
    let grad = &last(&w)["s"]["grad"];
    assert_eq!(grad["kind"], json!("conic"));
    assert_eq!(grad["angle"], json!(30.));

    // None of them by default: older hosts see the same op.
    w.set_style(&4, &Style::default());
    let s = last(&w)["s"].clone();
    for key in [
        "shadows", "sides", "bstyle", "clip", "grad", "blur", "cursor",
    ] {
        assert!(s.get(key).is_none(), "{key} sent by default");
    }

    // Which states to report; derived ones are not asked for.
    let states = Interactions::from(Interaction::Hover)
        | Interaction::FocusWithin.into()
        | Interaction::GroupHover.into();
    w.set_interactions(&4, states);
    assert_eq!(last(&w), json!({"op": "states", "id": 4, "on": ["hover"]}));

    // What hosts send back.
    let event = json!({"t": "state", "id": 4, "s": "active", "on": true});
    assert_eq!(
        prism_native::remote::state_event(&event),
        Some((4, Interaction::Active, true))
    );
    assert_eq!(
        prism_native::remote::state_event(&json!({"t": "click", "id": 4})),
        None
    );
    assert_eq!(
        prism_native::remote::state_event(
            &json!({"t": "state", "id": 4, "s": "wiggle", "on": true})
        ),
        None
    );
}

#[test]
fn hosts_learn_which_stacks_and_screens_hide_the_header() {
    use prism_core::{
        AppInit, Context, IntoElement, Render, Router, Runtime, STACK_NO_HEADER, screen,
    };
    struct Bare;
    impl Render for Bare {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            screen("Photo").bare()
        }
    }
    let init: AppInit = std::sync::Arc::new(|cx: &mut prism_core::App| {
        let r = Router::new()
            .header(false)
            .route("/", |cx, _| cx.new_entity(|_| Bare))
            .build(cx);
        cx.set_root(r);
    });
    let mut rt = Runtime::new(&init);
    let mut r = NativeRenderer::new(RemoteWidgets::new(measure));
    r.apply(&rt.render());
    let value_of = |kind: &str| {
        let id = r
            .widgets
            .ops
            .iter()
            .find(|o| o["op"] == "create" && o["kind"] == kind)
            .map(|o| o["id"].clone())
            .unwrap();
        r.widgets
            .ops
            .iter()
            .find(|o| o["op"] == "value" && o["id"] == id)
            .map(|o| o["value"].as_f64().unwrap())
    };
    // The host hides its header for flag 2 on the stack, or value 1 on the
    // top screen (PrismHost.java refreshStack, PrismWatch.swift StackView).
    assert_eq!(value_of("stack"), Some(STACK_NO_HEADER as f64));
    assert_eq!(value_of("screen"), Some(1.));
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

#[test]
fn typography_and_runs_reach_the_host_and_its_measure() {
    use prism_core::{FontFamily, Length, Styled, TextRun, TextTransform, encode_runs};
    let mut w = RemoteWidgets::new(measure);
    w.create(1, NodeKind::Text);
    let style = Style {
        font_size: 10.,
        font_weight: FontWeight::Custom(650),
        font_family: FontFamily::Monospace,
        italic: true,
        line_height: Length::Fraction(1.5),
        letter_spacing: 0.5,
        text_transform: TextTransform::Uppercase,
        underline: true,
        tabular_numbers: true,
        max_lines: Some(2),
        ..Style::default()
    };
    w.set_style(&1, &style);
    let s = last(&w)["s"].clone();
    assert_eq!(s["weight"], json!(650));
    for (k, v) in [
        ("ff", json!("mono")),
        ("it", json!(true)),
        ("lh", json!(15.0)),
        ("ls", json!(0.5)),
        ("tt", json!("upper")),
        ("ul", json!(true)),
        ("tn", json!(true)),
        ("ml", json!(2)),
    ] {
        assert_eq!(s[k], v, "{k}");
    }
    assert_eq!(s.get("st"), None, "only what is set");
    assert_eq!(s.get("tw"), None, "the usual line breaking");
    // Balanced and pretty wrapping (#95).
    w.set_style(&1, &Style::default().text_balance());
    assert_eq!(last(&w)["s"]["tw"], json!("balance"));
    w.set_style(&1, &Style::default().text_pretty());
    assert_eq!(last(&w)["s"]["tw"], json!("pretty"));
    // Plain styles carry no typography keys.
    w.set_style(&1, &Style::default());
    assert_eq!(last(&w)["s"].get("ff"), None);
    assert_eq!(last(&w)["s"].get("tw"), None);

    let runs = encode_runs(&[
        TextRun {
            text: "Hi ".into(),
            ..TextRun::default()
        },
        TextRun {
            text: "you".into(),
            weight: Some(FontWeight::Bold),
            color: Some(rgb(0xff0000)),
            link: Some(9),
            href: Some("/me".into()),
            ..TextRun::default()
        },
        TextRun {
            text: "★".into(),
            icon: Some("star.fill".into()),
            ..TextRun::default()
        },
        TextRun {
            text: "note".into(),
            underline: true,
            underline_color: Some(rgb(0x0000ff)),
            underline_style: prism_core::UnderlineStyle::Dotted,
            ..TextRun::default()
        },
    ]);
    w.set_config(&1, NodeKind::Text, &runs);
    assert_eq!(
        last(&w),
        json!({ "op": "runs", "id": 1, "runs": [
            { "t": "Hi " },
            { "t": "you", "w": 700, "c": [1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0], "link": 9, "href": "/me" },
            { "t": "★", "icon": "star.fill", "glyph": prism_core::glyphs::glyph("star.fill") },
            // Its own underline (#221).
            { "t": "note", "u": true, "uc": [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0], "us": "dotted" },
        ]})
    );
    // Controls still get their raw config.
    w.create(2, NodeKind::Slider);
    w.set_config(&2, NodeKind::Slider, "0 10 1");
    assert_eq!(
        last(&w),
        json!({ "op": "config", "id": 2, "config": "0 10 1" })
    );

    // The measure gets the same typography and runs.
    thread_local! {
        static SPEC: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn spec(r: &MeasureRequest) -> (f32, f32) {
        SPEC.with(|s| s.borrow_mut().push(r.spec.to_string()));
        measure(r)
    }
    let mut w = RemoteWidgets::new(spec);
    w.create(1, NodeKind::Text);
    w.set_style(&1, &style);
    w.set_text(&1, "Hi you★");
    w.set_config(&1, NodeKind::Text, &runs);
    w.measure(&1, Some(100.));
    w.measure_min_content(&1);
    let seen = SPEC.with(|s| s.borrow().clone());
    let full: Value = serde_json::from_str(&seen[0]).unwrap();
    assert_eq!(full["ff"], json!("mono"));
    assert_eq!(full["runs"][1]["w"], json!(700));
    // Words of the min-content pass: the style, without the runs.
    let word: Value = serde_json::from_str(&seen[1]).unwrap();
    assert_eq!(word.get("runs"), None);
    assert_eq!(word["ml"], json!(2));
    // A plain text has no spec.
    w.create(3, NodeKind::Text);
    w.set_text(&3, "plain");
    w.measure(&3, None);
    assert_eq!(
        SPEC.with(|s| s.borrow().last().cloned()),
        Some(String::new())
    );
}

#[test]
fn padded_leaves_carry_pad_and_measure_their_content_alone() {
    use prism_core::Edges;
    let padded = Style {
        padding: Edges {
            top: 4.,
            right: 12.,
            bottom: 4.,
            left: 20.,
        },
        ..Style::default()
    };
    let mut w = RemoteWidgets::new(measure);
    w.create(1, NodeKind::Button);
    w.set_style(&1, &Style::default());
    assert!(last(&w)["s"].get("pad").is_none(), "only when set");
    w.set_style(&1, &padded);
    assert_eq!(last(&w)["s"]["pad"], json!([4., 12., 4., 20.]));
    assert!(last(&w)["s"].get("bnone").is_none());
    // `.border(0.)`: no frame of the field's own.
    w.set_style(
        &1,
        &Style {
            border_none: true,
            ..Style::default()
        },
    );
    assert_eq!(last(&w)["s"]["bnone"], json!(true));
    // A padded button is its title, measured as text (no built-in insets).
    SEEN.with(|s| s.borrow_mut().clear());
    let mut w = RemoteWidgets::new(recording);
    w.create(1, NodeKind::Button);
    w.set_style(&1, &padded);
    w.set_text(&1, "Save");
    assert_eq!(w.measure(&1, None), (40., 16.));
    // A padded input: one line, 10 em wide.
    w.create(2, NodeKind::TextInput);
    w.set_style(&2, &padded);
    assert_eq!(w.measure(&2, None), (160., 16.));
    let seen = SEEN.with(|s| s.borrow().clone());
    assert_eq!((seen[0].0.as_str(), seen[0].1.as_str()), ("text", "Save"));
    assert_eq!((seen[1].0.as_str(), seen[1].1.as_str()), ("text", "Ag"));
}

#[test]
fn a_zero_padded_field_carries_pad_and_measures_its_line_alone() {
    use prism_core::{Patch, Styled, text_input};
    let mut el = text_input().border(0.).p(0.);
    let zero = el.style_mut().clone();
    // `.p(0.)` is sent (a real zero inset, #69); no padding is not.
    let mut w = RemoteWidgets::new(measure);
    w.create(1, NodeKind::TextInput);
    w.set_style(&1, &Style::default());
    assert!(last(&w)["s"].get("pad").is_none());
    w.set_style(&1, &zero);
    assert_eq!(last(&w)["s"]["pad"], json!([0., 0., 0., 0.]));
    // Measured as its line of text, without the control's own insets.
    SEEN.with(|s| s.borrow_mut().clear());
    let mut w = RemoteWidgets::new(recording);
    w.create(1, NodeKind::TextInput);
    w.set_style(&1, &zero);
    assert_eq!(w.measure(&1, None), (160., 16.));
    let seen = SEEN.with(|s| s.borrow().clone());
    assert_eq!((seen[0].0.as_str(), seen[0].1.as_str()), ("text", "Ag"));
    let mut r = NativeRenderer::new(RemoteWidgets::new(measure));
    r.apply(&[
        Patch::Create {
            id: 1,
            kind: NodeKind::View,
        },
        Patch::Create {
            id: 2,
            kind: NodeKind::TextInput,
        },
        Patch::SetStyle {
            id: 1,
            style: Style {
                align_items: prism_core::Align::Start,
                ..Style::default()
            },
        },
        Patch::SetStyle { id: 2, style: zero },
        Patch::SetChildren {
            id: 1,
            children: vec![2],
        },
        Patch::SetRoot { id: 1 },
    ]);
    r.layout(400., 800., prism_native::Insets::default());
    let frame = r
        .widgets
        .ops
        .iter()
        .rev()
        .find(|o| o["op"] == "frame" && o["id"] == 2)
        .cloned()
        .expect("a frame");
    // As wide as offered (an input fills), as tall as its line: no inset.
    assert_eq!(frame["f"], json!([0., 0., 400., 16.]));
}

#[test]
fn a_padded_text_frame_includes_its_padding() {
    use prism_core::{Edges, Patch};
    let mut r = NativeRenderer::new(RemoteWidgets::new(measure));
    let leaf = Style {
        padding: Edges {
            top: 4.,
            right: 12.,
            bottom: 4.,
            left: 12.,
        },
        ..Style::default()
    };
    r.apply(&[
        Patch::Create {
            id: 1,
            kind: NodeKind::View,
        },
        Patch::Create {
            id: 2,
            kind: NodeKind::Text,
        },
        Patch::SetText {
            id: 2,
            text: "Chip".into(),
        },
        Patch::SetStyle {
            id: 1,
            style: Style {
                align_items: prism_core::Align::Start,
                ..Style::default()
            },
        },
        Patch::SetStyle { id: 2, style: leaf },
        Patch::SetChildren {
            id: 1,
            children: vec![2],
        },
        Patch::SetRoot { id: 1 },
    ]);
    r.layout(400., 800., prism_native::Insets::default());
    let frame = r
        .widgets
        .ops
        .iter()
        .rev()
        .find(|o| o["op"] == "frame" && o["id"] == 2)
        .cloned()
        .expect("a frame");
    // 40 × 16 of text, 12 and 4 around it.
    assert_eq!(frame["f"], json!([0., 0., 64., 24.]));
    let style = r
        .widgets
        .ops
        .iter()
        .find(|o| o["op"] == "style" && o["id"] == 2)
        .cloned()
        .expect("a style");
    assert_eq!(style["s"]["pad"], json!([4., 12., 4., 12.]));
}

#[test]
fn screens_send_their_bar_buttons_with_glyphs_and_bars_their_tint() {
    use prism_core::navbar::{BarItem, BarPlacement, ScreenConfig};
    let mut w = RemoteWidgets::new(measure);
    w.create(3, NodeKind::Screen);
    let config = ScreenConfig {
        large_title: None,
        items: vec![
            BarItem {
                placement: BarPlacement::Trailing,
                icon: "plus".into(),
                label: "Add".into(),
                ..Default::default()
            },
            BarItem {
                placement: BarPlacement::Leading,
                title: "Edit".into(),
                ..Default::default()
            },
        ],
    }
    .encode();
    w.set_config(&3, NodeKind::Screen, &config);
    assert_eq!(
        last(&w),
        json!({"op": "config", "id": 3, "config": config, "glyphs": ["+", ""]})
    );
    // Other configs carry no glyphs.
    w.create(4, NodeKind::Picker);
    w.set_config(&4, NodeKind::Picker, "a\nb");
    assert_eq!(last(&w).get("glyphs"), None);

    w.create(5, NodeKind::Tabs);
    w.set_style(&5, &Style::default());
    assert_eq!(last(&w)["s"].get("tint"), None, "no tint: no key");
    let tinted = Style {
        tint: Some(prism_core::rgb(0xFF0000)),
        ..Style::default()
    };
    w.set_style(&5, &tinted);
    assert_eq!(
        last(&w)["s"]["tint"],
        json!([1., 0., 0., 1., 1., 0., 0., 1.])
    );
}

/// Keyboard bars (#242): each text field learns its bar (`kbar`), its own
/// before its container's, and the bar's content is laid out at the
/// overlay's width.
#[test]
fn text_fields_learn_their_keyboard_bar() {
    use prism_core::{
        AppInit, Context, IntoElement, ParentElement, Platform, Render, Runtime, RuntimeOptions,
        Styled, button, div, text_area, text_input,
    };
    struct App;
    impl Render for App {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(text_input().keyboard_toolbar(button("B").h(40.)))
                .child(div().child(text_area()))
                .keyboard_toolbar(div().h(44.).child(button("Done")))
        }
    }
    let init: AppInit = std::sync::Arc::new(|cx: &mut prism_core::App| {
        let e = cx.new_entity(|_| App);
        cx.set_root(e);
    });
    let mut rt = Runtime::with_options(
        &init,
        RuntimeOptions {
            path: "/".into(),
            platform: Some(Platform::Android),
            ..Default::default()
        },
    );
    let mut r = NativeRenderer::new(RemoteWidgets::new(measure));
    r.apply(&rt.render());
    r.layout(320., 600., Default::default());
    let ops = &r.widgets.ops;
    let created = |kind: &str| -> Vec<Value> {
        ops.iter()
            .filter(|o| o["op"] == "create" && o["kind"] == kind)
            .map(|o| o["id"].clone())
            .collect()
    };
    let (input, area) = (
        created("text_input")[0].clone(),
        created("text_area")[0].clone(),
    );
    let overlays = created("overlay");
    let configs: Vec<Value> = overlays
        .iter()
        .map(|id| {
            ops.iter()
                .find(|o| o["op"] == "config" && o["id"] == *id)
                .map(|o| o["config"].clone())
                .unwrap()
        })
        .collect();
    let own = overlays[configs.iter().position(|c| c == "keyboard field").unwrap()].clone();
    let shared = overlays[configs.iter().position(|c| c == "keyboard").unwrap()].clone();
    let bar_of = |field: &Value| {
        ops.iter()
            .rev()
            .find(|o| o["op"] == "kbar" && o["id"] == *field)
            .map(|o| o["bar"].clone())
    };
    assert_eq!(bar_of(&input), Some(own));
    assert_eq!(
        bar_of(&area),
        Some(shared.clone()),
        "a nested field gets its container's bar"
    );
    // The shared bar's content: 44 high, at the root's width.
    assert!(
        ops.iter()
            .any(|o| o["op"] == "content" && o["id"] == shared && o["size"] == json!([320., 44.]))
    );
}

/// Other people's carets (#240): offsets in characters, the color in both
/// appearances, the name and whether it fades; an empty list removes them.
#[test]
fn remote_caret_ops() {
    let mut w = RemoteWidgets::new(measure);
    w.create(4, NodeKind::TextArea);
    w.set_remote_carets(
        &4,
        &[
            prism_core::RemoteCaret::caret(12, rgb(0xFF0000)).label("Camille"),
            prism_core::RemoteCaret::new(3, 9, rgb(0x0000FF))
                .idle(true)
                .label_text_color(rgb(0x000000)),
        ],
    );
    assert_eq!(
        last(&w),
        json!({"op": "carets", "id": 4, "carets": [
            {"anchor": 12, "head": 12, "color": [1., 0., 0., 1., 1., 0., 0., 1.], "label": "Camille", "idle": false, "text": null},
            {"anchor": 3, "head": 9, "color": [0., 0., 1., 1., 0., 0., 1., 1.], "label": null, "idle": true, "text": [0., 0., 0., 1., 0., 0., 0., 1.]},
        ]})
    );
    w.set_remote_carets(&4, &[]);
    assert_eq!(last(&w), json!({"op": "carets", "id": 4, "carets": []}));
}

/// A rich input's editing hooks (#217): the ops a host gets, the events it
/// sends back.
#[test]
fn rich_input_editing_hook_ops_and_events() {
    use prism_core::{ClipboardContent, UiEvent};
    use prism_native::remote::parse_input_event;
    let mut w = RemoteWidgets::new(measure);
    w.create(4, NodeKind::TextArea);
    w.set_edit_events(&4, 10);
    assert_eq!(last(&w), json!({"op": "editev", "id": 4, "mask": 10}));
    w.write_clipboard(&4, &ClipboardContent::text("a").html("<b>a</b>"));
    assert_eq!(
        last(&w),
        json!({"op": "clip", "id": 4, "items": [["text/plain", "a"], ["text/html", "<b>a</b>"]]})
    );
    w.insert_text(&4, "pasted");
    assert_eq!(
        last(&w),
        json!({"op": "instext", "id": 4, "text": "pasted"})
    );

    assert_eq!(
        parse_input_event(
            &json!({"t": "copy", "id": 4, "cut": true, "start": 1, "end": 3, "text": "bc"})
        ),
        Some(UiEvent::Copy(4, true, 1, 3, "bc".into()))
    );
    let Some(UiEvent::Paste(4, p)) = parse_input_event(&json!({"t": "paste", "id": 4, "start": 2,
        "end": 2, "text": "x", "html": "<i>x</i>", "types": ["text/plain", "text/html"]}))
    else {
        panic!("paste")
    };
    assert_eq!(
        (p.text.as_str(), p.html.as_deref()),
        ("x", Some("<i>x</i>"))
    );
    assert_eq!(
        parse_input_event(
            &json!({"t": "selrect", "id": 4, "start": 0, "end": 2, "x": 1, "y": 2, "w": 3, "h": 4})
        ),
        Some(UiEvent::SelectionRect(
            4,
            0,
            2,
            prism_core::Rect::new(1., 2., 3., 4.)
        ))
    );
}

/// Wraps at `max_width` (10 per character), 20 per line.
fn lines(r: &MeasureRequest) -> (f32, f32) {
    let max = r.max_width.unwrap_or(f32::MAX);
    let rows: f32 = r
        .text
        .split('\n')
        .map(|l| (l.chars().count() as f32 * 10. / max).ceil().max(1.))
        .sum();
    (max.min(1000.), rows * 20.)
}

/// A text area is at least its minimum lines tall (#235), three unless
/// set; a rich input one: a paragraph of one line is one line tall.
#[test]
fn a_text_area_measures_its_minimum_lines() {
    let mut w = RemoteWidgets::new(lines).with_rich_inputs();
    w.create(1, NodeKind::TextArea);
    w.set_text(&1, "One line");
    assert_eq!(w.measure(&1, Some(300.)).1, 60.);
    // A long line wraps past them.
    w.set_text(&1, &"x".repeat(50));
    assert_eq!(w.measure(&1, Some(100.)).1, 100.);
    w.set_config(&1, NodeKind::TextArea, "rows=1");
    w.set_text(&1, "One line");
    assert_eq!(w.measure(&1, Some(300.)).1, 20.);
    // A rich input: a text area of `rows=1`, as `rich_input` sends it
    // (`prism-core`'s `a_rich_input_is_one_line_at_least`).
    w.create(2, NodeKind::RichInput);
    w.set_config(&2, NodeKind::RichInput, "rows=1");
    w.set_text(&2, "Select from the middle of this paragraph,");
    assert_eq!(w.measure(&2, Some(1000.)).1, 20.);
    w.set_text(&2, "Two\nlines");
    assert_eq!(w.measure(&2, Some(1000.)).1, 40.);
    // With padding, the content alone: the same lines.
    w.set_style(
        &2,
        &Style {
            padding: prism_core::Edges::all(8.),
            ..Style::default()
        },
    );
    assert_eq!(w.measure(&2, Some(1000.)).1, 40.);
    w.set_text(&2, "One");
    assert_eq!(w.measure(&2, Some(1000.)).1, 20.);
}

/// A chip click with its box (#327): `RunClick`; without one it is left to
/// the backend (a plain `MenuSelect`).
#[test]
fn chip_click_with_its_box() {
    use prism_core::{Rect, UiEvent};
    use prism_native::remote::parse_input_event;
    assert_eq!(
        parse_input_event(&json!({"t": "menu", "id": 9, "x": 4, "y": 20, "w": 60, "h": 18})),
        Some(UiEvent::RunClick(9, Rect::new(4., 20., 60., 18.)))
    );
    assert_eq!(parse_input_event(&json!({"t": "menu", "id": 9})), None);
}

/// #343: a web view of an app scheme goes to the host (Android) as the
/// `https:` URL standing for it, with its allowed origins; its messages
/// and loads come back as the app wrote them.
#[test]
fn app_scheme_web_view_ops() {
    use prism_native::remote::parse_input_event;
    let mut w = RemoteWidgets::new(measure);
    w.create(4, NodeKind::WebView);
    w.set_text(&4, "app://main/index.html?v=1");
    assert_eq!(
        last(&w),
        json!({"op": "text", "id": 4, "text": "https://main.app.prism.invalid/index.html?v=1"})
    );
    w.set_config(
        &4,
        NodeKind::WebView,
        "messages loads\napp://main\nhttps://example.com",
    );
    assert_eq!(
        last(&w),
        json!({"op": "config", "id": 4,
               "config": "messages loads\nhttps://main.app.prism.invalid\nhttps://example.com"})
    );
    // Web pages are sent as they are.
    w.set_text(&4, "https://example.com/a");
    assert_eq!(last(&w)["text"], "https://example.com/a");
    // Other nodes' texts are never mapped.
    w.create(5, NodeKind::Text);
    w.set_text(&5, "app://main/");
    assert_eq!(last(&w)["text"], "app://main/");

    let msg = parse_input_event(&json!({"t": "webmsg", "id": 4, "data": "1",
        "origin": "https://main.app.prism.invalid"}));
    assert_eq!(
        msg,
        Some(prism_core::UiEvent::WebMessage(
            4,
            "1".into(),
            "app://main".into()
        ))
    );
    let load = parse_input_event(&json!({"t": "webload", "id": 4,
        "url": "https://main.app.prism.invalid/index.html#x"}));
    assert_eq!(
        load,
        Some(prism_core::UiEvent::WebLoad(
            4,
            "app://main/index.html#x".into()
        ))
    );
    let other = parse_input_event(&json!({"t": "webmsg", "id": 4, "data": "1",
        "origin": "https://example.com"}));
    assert_eq!(
        other,
        Some(prism_core::UiEvent::WebMessage(
            4,
            "1".into(),
            "https://example.com".into()
        ))
    );
}

/// #343: what Android's `shouldInterceptRequest` hands the app, and the
/// answer it reads back.
#[test]
fn app_scheme_requests_from_the_host() {
    use prism_native::remote::{encode_scheme_response, scheme_request};
    let r = scheme_request(
        "GET",
        "https://main.app.prism.invalid/api/data.json?x=1",
        "Accept: */*\nOrigin: https://main.app.prism.invalid\nReferer: https://main.app.prism.invalid/index.html\n",
    )
    .unwrap();
    assert_eq!(r.url, "app://main/api/data.json?x=1");
    assert_eq!(r.header("referer"), Some("app://main/index.html"));
    assert_eq!(r.origin().as_deref(), Some("app://main"));
    // A third-party frame keeps its own origin.
    let third = scheme_request(
        "GET",
        "https://main.app.prism.invalid/api/data.json",
        "Origin: https://ads.example\n",
    )
    .unwrap();
    assert_eq!(third.origin().as_deref(), Some("https://ads.example"));
    assert!(scheme_request("GET", "https://example.com/", "").is_none());
    let bytes = encode_scheme_response(
        &prism_core::SchemeResponse::text("hi").with_header("Cache-Control", "no-store"),
    );
    assert_eq!(
        bytes,
        b"200 OK\nContent-Type: text/plain; charset=utf-8\nCache-Control: no-store\n\nhi"
    );
}
