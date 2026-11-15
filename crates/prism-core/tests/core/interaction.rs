//! Pointer, drag, keys, focus, layers, overlays and choice controls
//! (#17, #18, #19, #25).

use std::cell::RefCell;
use std::rc::Rc;

use prism_core::prelude::*;
use prism_core::{NodeKind, Patch, UiEvent};
use prism_testing::{Harness, mount};

type Log = Rc<RefCell<Vec<String>>>;

struct Pad {
    log: Log,
    with_key: bool,
}

impl Render for Pad {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let log = self.log.clone();
        let l = |log: &Log, tag: &'static str| {
            let log = log.clone();
            move |s: String| log.borrow_mut().push(format!("{tag} {s}"))
        };
        let (click, hover, mv, drag, key, global, outer) = (
            l(&log, "click"),
            l(&log, "hover"),
            l(&log, "move"),
            l(&log, "drag"),
            l(&log, "key"),
            l(&log, "global"),
            l(&log, "outer"),
        );
        div()
            .on_key_down(Rc::new(move |e: &KeyEvent, _: &mut App| {
                outer(e.key.clone())
            }))
            .child(
                div()
                    .id("pad")
                    .child(text("pad"))
                    .on_click(Rc::new(move |e: &ClickEvent, _: &mut App| {
                        click(format!("{:?}", e.position.map(|p| (p.x, p.y))))
                    }))
                    .on_hover(Rc::new(move |e: &HoverEvent, _: &mut App| {
                        hover(e.hovered.to_string())
                    }))
                    .on_pointer_move(Rc::new(move |e: &PointerEvent, _: &mut App| {
                        mv(format!("{} {}", e.position.x, e.position.y))
                    }))
                    .on_drag(Rc::new(move |e: &DragEvent, _: &mut App| {
                        drag(format!("{} {}", e.phase.name(), e.position().x))
                    }))
                    .when(self.with_key, |d| {
                        d.on_key_down(Rc::new(move |e: &KeyEvent, app: &mut App| {
                            key(e.key.clone());
                            if e.is("Escape") {
                                app.stop_propagation();
                            }
                        }))
                    }),
            )
            .child(
                div().on_global_key_down(Rc::new(move |e: &KeyEvent, _: &mut App| {
                    global(e.key.clone())
                })),
            )
    }
}

fn pad() -> (Harness, Log) {
    let h = Harness::new(move |cx| {
        let log: Log = Rc::default();
        cx.set_global(log.clone());
        let root = cx.new_entity(|_| Pad {
            log,
            with_key: true,
        });
        mount(cx, root);
    });
    let log = h.rt.app.global::<Log>().clone();
    (h, log)
}

#[test]
fn pointer_and_drag_events_reach_their_handlers() {
    let (mut h, log) = pad();
    let id = h
        .host
        .find(|n| n.pointer_mask != 0)
        .expect("the pad reports pointer events");
    assert_eq!(h.host.get(id).pointer_mask, 1 | 2 | 4);
    h.dispatch(UiEvent::ClickAt(id, Point::new(3., 4.)));
    h.dispatch(UiEvent::Click(id));
    for phase in [PointerPhase::Enter, PointerPhase::Move, PointerPhase::Leave] {
        h.dispatch(UiEvent::Pointer(
            id,
            PointerEvent {
                phase,
                position: Point::new(1., 2.),
            },
        ));
    }
    h.dispatch(UiEvent::Drag(
        id,
        DragEvent {
            phase: DragPhase::Move,
            start: Point::new(10., 0.),
            translation: Point::new(5., 0.),
        },
    ));
    assert_eq!(
        *log.borrow(),
        [
            "click Some((3.0, 4.0))",
            "click None",
            "hover true",
            "move 1 2",
            "move 1 2",
            "hover false",
            "drag move 15",
        ]
    );
}

#[test]
fn keys_bubble_then_reach_global_handlers_unless_stopped() {
    let (mut h, log) = pad();
    let id = h.host.find(|n| n.keys.0 && n.pointer_mask != 0).unwrap();
    assert!(h.host.find(|n| n.keys == (false, true)).is_some());
    h.dispatch(UiEvent::Key(Some(id), KeyEvent::new("ArrowUp")));
    h.dispatch(UiEvent::Key(Some(id), KeyEvent::new("Escape")));
    h.dispatch(UiEvent::Key(None, KeyEvent::new("a")));
    assert_eq!(
        *log.borrow(),
        [
            "key ArrowUp",
            "outer ArrowUp",
            "global ArrowUp",
            "key Escape",
            "global a",
        ]
    );
    // Removing the handler unsubscribes the node.
    h.update::<Pad>(|p| p.with_key = false);
    assert!(h.last.iter().any(|p| matches!(
        p,
        Patch::SetKeyEvents {
            local: false,
            global: false,
            ..
        }
    )));
}

struct Form {
    show_email: bool,
}

impl Render for Form {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(button("Jump").id("jump").on_click(cx.listener(|_, _, cx| {
                cx.scroll_into_view("email");
                cx.focus("email");
                cx.focus("nobody");
            })))
            .when(self.show_email, |d| {
                d.child(text_input().id("email").autofocus().max_length(40))
            })
    }
}

#[test]
fn focus_scroll_and_autofocus_patches_name_mounted_nodes() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Form { show_email: false });
        mount(cx, root);
    });
    assert!(h.host.focused.is_empty());
    h.update::<Form>(|f| f.show_email = true);
    let email = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    assert_eq!(h.host.focused, [email], "autofocus on mount");
    assert_eq!(h.host.get(email).config, "max=40");
    // Re-rendering does not focus it again.
    h.update::<Form>(|_| {});
    assert_eq!(h.host.focused, [email]);
    h.click("Jump");
    assert_eq!(h.host.scrolled_to, [email]);
    assert_eq!(h.host.focused, [email, email], "unknown names are ignored");
    // Requests without any re-render still go out.
    h.rt.app.focus("jump");
    let p = h.rt.poll();
    h.apply(p);
    assert_eq!(h.host.focused.last(), Some(&h.host.by_text("Jump")));
}

struct Banner {
    text: String,
}

impl Render for Banner {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(text(self.text.clone()))
    }
}

struct Plain;

impl Render for Plain {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(text("screen"))
    }
}

#[test]
fn layers_mount_above_the_root_and_update_in_place() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Plain);
        mount(cx, root);
        let banner = cx.new_entity(|_| Banner { text: "hi".into() });
        cx.set_global(banner);
    });
    assert!(h.host.layers.is_empty());
    let banner = *h.rt.app.global::<Entity<Banner>>();
    h.rt.app.add_layer(banner);
    h.rt.app.add_layer(banner);
    h.render();
    assert_eq!(h.host.layers.len(), 1);
    let layer = h.host.layers[0];
    assert_eq!(h.host.child_texts(layer), ["hi"]);
    assert!(h.rt.app.has_layer(&banner));
    banner.update(&mut h.rt.app, |b, cx| {
        b.text = "saved".into();
        cx.notify();
    });
    h.render();
    assert_eq!(h.host.layers, [layer], "re-rendered in place");
    assert_eq!(h.host.child_texts(layer), ["saved"]);
    assert_eq!(h.host.get(h.host.root.unwrap()).children.len(), 1);
    h.rt.app.remove_layer(&banner);
    h.render();
    assert!(h.host.layers.is_empty());
    assert!(h.host.destroyed.contains(&layer));
}

struct Sheet {
    open: bool,
    dismissed: usize,
}

impl Render for Sheet {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(
            sheet(text("content"))
                .open(self.open)
                .half()
                .on_dismiss(cx.listener(|this, _, _| this.dismissed += 1)),
        )
    }
}

#[test]
fn overlays_build_their_content_only_while_open() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Sheet {
            open: false,
            dismissed: 0,
        });
        mount(cx, root);
    });
    let ov = h.host.find(|n| n.kind == NodeKind::Overlay).unwrap();
    assert_eq!(h.host.get(ov).value, Some(0.));
    assert_eq!(h.host.get(ov).config, "sheet medium");
    assert!(h.host.get(ov).children.is_empty());
    h.update::<Sheet>(|s| s.open = true);
    assert_eq!(h.host.get(ov).value, Some(1.));
    assert_eq!(h.host.child_texts(ov), ["content"]);
    h.dispatch(UiEvent::Back(ov, 0));
    assert_eq!(h.rt.app.read(&h.root::<Sheet>()).dismissed, 1);
    // The app kept it open (a controlled overlay): it is shown again.
    assert_eq!(h.host.get(ov).value, Some(1.));
    assert!(
        h.last
            .iter()
            .any(|p| matches!(p, Patch::SetValue { id, value } if *id == ov && *value == 1.))
    );
    h.update::<Sheet>(|s| s.open = false);
    assert!(h.host.get(ov).children.is_empty());
}

struct Choices {
    every: Option<usize>,
    remember: bool,
}

impl Render for Choices {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                checkbox(self.remember)
                    .label("Remember me")
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.remember = e.checked())),
            )
            .child(
                radio_group(["Daily", "Weekly"], self.every)
                    .horizontal()
                    .accessibility_label("Frequency")
                    .on_change(
                        cx.listener(|this, e: &ChangeEvent, _| this.every = Some(e.index())),
                    ),
            )
            .child(time_picker(7 * 60 + 30))
            .child(
                text_input()
                    .leading_icon("magnifyingglass")
                    .suggestions(["Paris", "Lyon"]),
            )
    }
}

#[test]
fn choice_controls_carry_their_state_and_semantics() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Choices {
            every: None,
            remember: false,
        });
        mount(cx, root);
    });
    let cb = h.host.find(|n| n.kind == NodeKind::Checkbox).unwrap();
    let rg = h.host.find(|n| n.kind == NodeKind::RadioGroup).unwrap();
    let a = &h.host.get(cb).a11y;
    assert_eq!(a.role, Some(Role::Checkbox));
    assert_eq!(a.checked, Some(false));
    assert_eq!(a.label.as_deref(), Some("Remember me"));
    assert_eq!(h.host.get(rg).config, "row\nDaily\nWeekly");
    assert_eq!(h.host.get(rg).value, Some(-1.));
    assert_eq!(h.host.get(rg).a11y.role, Some(Role::RadioGroup));
    h.dispatch(UiEvent::Change(cb, 1.));
    h.dispatch(UiEvent::Change(rg, 1.));
    assert_eq!(h.host.get(cb).a11y.checked, Some(true));
    assert_eq!(h.host.get(rg).a11y.value.as_deref(), Some("Weekly"));
    let tp = h.host.find(|n| n.kind == NodeKind::DatePicker).unwrap();
    assert_eq!(h.host.get(tp).value, Some(27000.));
    assert_eq!(h.host.get(tp).config, "timeofday - -");
    let e = ChangeEvent { value: 27000. };
    assert_eq!(e.minutes(), 450);
    let input = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    assert_eq!(h.host.get(input).icon, "magnifyingglass");
    assert_eq!(
        prism_core::controls::input_suggestions(&h.host.get(input).config),
        ["Paris", "Lyon"]
    );
}

#[test]
fn hit_test_and_inert_are_styles() {
    let s = div().hit_test(false).inert(true);
    let n = match s.into_element() {
        prism_core::Element::Node(n) => n,
        _ => unreachable!(),
    };
    assert!(!n.style.hit_test && n.style.inert);
    assert!(prism_core::Style::default().hit_test);
}

#[test]
fn picked_files_read_their_bytes() {
    let dir = std::env::temp_dir().join(format!("prism-pick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.txt");
    std::fs::write(&file, b"hello").unwrap();
    let local = prism_core::PickedFile {
        path: file.to_string_lossy().into(),
        name: "a.txt".into(),
        mime: "text/plain".into(),
        size: 5,
    };
    assert_eq!(local.read().unwrap(), b"hello");
    let url = "https://example.test/files/abc.txt";
    prism_core::device::register_local_file(url, file.clone());
    let web = prism_core::PickedFile {
        path: url.into(),
        ..local
    };
    assert_eq!(web.read().unwrap(), b"hello");
    let any = PickFiles::new();
    assert!(any.accepts("x.bin", "application/octet-stream"));
    let images = PickFiles::new().types(["image/*", ".pdf"]).multiple();
    assert!(images.multiple);
    assert!(images.accepts("a.PNG", "image/png"));
    assert!(images.accepts("doc.pdf", "application/octet-stream"));
    assert!(!images.accepts("a.txt", "text/plain"));
    let _ = std::fs::remove_dir_all(dir);
}
