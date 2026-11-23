//! Pointer, drag, keys, focus, layers, overlays and choice controls
//! (#17, #18, #19, #25).

use std::cell::RefCell;
use std::rc::Rc;

use prism_core::prelude::*;
use prism_core::{NodeKind, Patch, UiEvent, unix_time};
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
            button: prism_core::PointerButton::Primary,
            modifiers: Default::default(),
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

#[test]
fn dismiss_keyboard_goes_out_after_the_render() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Form { show_email: true });
        mount(cx, root);
    });
    assert_eq!(h.host.keyboard_dismissals, 0);
    // With a root swap in the same update (#29): the request still goes out.
    h.update::<Form>(|f| f.show_email = false);
    h.rt.app.dismiss_keyboard();
    let p = h.rt.poll();
    assert!(p.iter().any(|p| matches!(p, Patch::DismissKeyboard)));
    h.apply(p);
    assert_eq!(h.host.keyboard_dismissals, 1);
    // Once.
    let p = h.rt.poll();
    assert!(!p.iter().any(|p| matches!(p, Patch::DismissKeyboard)));
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

struct Slash {
    text: String,
}

impl Render for Slash {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(text_input().value(self.text.clone())).children(
            self.text.starts_with('/').then(|| {
                div().child(
                    popover(text("/heading"))
                        .open(true)
                        .light_dismiss(false)
                        .take_focus(false),
                )
            }),
        )
    }
}

#[test]
fn a_popover_can_be_inserted_open_and_leave_the_focus_to_its_anchor() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Slash {
            text: String::new(),
        });
        mount(cx, root);
    });
    assert!(h.host.find(|n| n.kind == NodeKind::Overlay).is_none());
    // The menu appears with its parent, already open, in one patch (#70).
    h.update::<Slash>(|s| s.text = "/".into());
    let ov = h.host.find(|n| n.kind == NodeKind::Overlay).unwrap();
    assert_eq!(h.host.get(ov).config, "popover manual keepfocus");
    assert_eq!(h.host.get(ov).value, Some(1.));
    assert_eq!(h.host.child_texts(ov), ["/heading"]);
    assert!(
        h.last
            .iter()
            .any(|p| matches!(p, Patch::Create { id, .. } if *id == ov))
    );
    assert!(
        h.last
            .iter()
            .any(|p| matches!(p, Patch::SetValue { id, value } if *id == ov && *value == 1.))
    );
    // The defaults: light dismiss, focus taken.
    assert_eq!(
        prism_core::controls::OverlayConfig::decode("popover"),
        prism_core::controls::OverlayConfig {
            kind: prism_core::OverlayKind::Popover,
            ..Default::default()
        }
    );
}

/// A menu with a submenu beside its item and a panel lined up with the
/// trailing edge of its button (#113).
struct Menus;

impl Render for Menus {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                div().child(text("Turn into")).child(
                    popover(text("Heading"))
                        .open(true)
                        .side(PopoverSide::Trailing),
                ),
            )
            .child(
                div().child(button("More")).child(
                    popover(text("Settings"))
                        .open(true)
                        .align(PopoverAlign::End)
                        .light_dismiss(false),
                ),
            )
            .child(div().child(popover(text("Plain")).open(true)))
    }
}

#[test]
fn popovers_say_their_side_and_alignment() {
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Menus);
        mount(cx, root);
    });
    let configs: Vec<String> = (h.host.mounted().into_iter())
        .map(|id| h.host.get(id))
        .filter(|n| n.kind == NodeKind::Overlay)
        .map(|n| n.config.clone())
        .collect();
    assert_eq!(
        configs,
        [
            "popover side=trailing",
            "popover manual align=end",
            "popover"
        ]
    );
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
                    .suggestions(["Paris", "Lyon"])
                    // Later input settings keep the suggestions.
                    .max_length(12),
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
    assert_eq!(h.host.env.as_ref().unwrap().clear, "Clear");
    let e = ChangeEvent { value: 27000. };
    assert_eq!(e.minutes(), 450);
    let input = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    assert_eq!(h.host.get(input).icon, "magnifyingglass");
    assert_eq!(
        prism_core::controls::input_suggestions(&h.host.get(input).config),
        ["Paris", "Lyon"]
    );
}

/// Option cards drawn by the app, reported as radios (#84).
struct Plans {
    chosen: usize,
}

impl Render for Plans {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let card = |i: usize, title: &'static str, note: &'static str| {
            div()
                .child(title)
                .child(note)
                .accessibility_role(Role::Radio)
                .accessibility_checked(self.chosen == i)
                .on_click(cx.listener(move |this: &mut Plans, _, _| this.chosen = i))
        };
        div()
            .accessibility_role(Role::RadioGroup)
            .accessibility_label("Plan")
            .child(card(0, "Monthly", "Billed every month"))
            .child(card(1, "Yearly", "Two months free"))
    }
}

#[test]
fn drawn_choices_are_radios_with_their_state() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Plans { chosen: 0 });
        mount(cx, root);
    });
    let group = h
        .host
        .find(|n| n.a11y.role == Some(Role::RadioGroup))
        .unwrap();
    assert_eq!(h.host.get(group).a11y.label.as_deref(), Some("Plan"));
    // The app's role wins over the clickable view's button, its text is the label.
    let card = |h: &Harness, label: &str| {
        h.host
            .find(|n| n.a11y.role == Some(Role::Radio) && n.a11y.label.as_deref() == Some(label))
            .unwrap()
    };
    let monthly = card(&h, "Monthly, Billed every month");
    let yearly = card(&h, "Yearly, Two months free");
    assert_eq!(h.host.get(monthly).a11y.checked, Some(true));
    assert_eq!(h.host.get(yearly).a11y.checked, Some(false));
    assert!(Role::Radio.is_interactive() && !Role::RadioGroup.is_interactive());
    h.dispatch(UiEvent::Click(yearly));
    assert_eq!(h.host.get(yearly).a11y.checked, Some(true));
    assert_eq!(h.host.get(monthly).a11y.checked, Some(false));
}

struct Optional {
    published: Option<std::time::SystemTime>,
    opens: Option<u32>,
    due: std::time::SystemTime,
}

impl Render for Optional {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                date_picker_opt(self.published)
                    .placeholder("Not published")
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| {
                        this.published = e.date_opt();
                    })),
            )
            .child(
                time_picker_opt(self.opens)
                    .placeholder("Opens at")
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| {
                        this.opens = e.minutes_opt();
                    })),
            )
            .child(
                date_picker(self.due)
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.due = e.date())),
            )
    }
}

/// Optional date and time pickers: empty is `NaN`, cleared is `None` (#89).
#[test]
fn optional_date_pickers_can_be_empty_and_cleared() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Optional {
            published: None,
            opens: Some(9 * 60),
            due: unix_time(1000.),
        });
        mount(cx, root);
    });
    let pickers: Vec<_> = {
        let mut ids: Vec<_> = h
            .host
            .nodes
            .iter()
            .filter(|(_, n)| n.kind == NodeKind::DatePicker)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    };
    let [published, opens, due] = pickers[..] else {
        panic!("three pickers")
    };
    let p = h.host.get(published);
    assert!(p.value.unwrap().is_nan());
    assert_eq!(p.config, "date - - optional");
    assert_eq!(p.placeholder, "Not published");
    // The placeholder names the picker.
    assert_eq!(p.a11y.label.as_deref(), Some("Not published"));
    assert_eq!(h.host.get(opens).config, "timeofday - - optional");
    assert_eq!(h.host.get(opens).value, Some(9. * 3600.));
    assert_eq!(h.host.get(due).config, "date - -");
    // An unchanged empty value is not sent again.
    let patches = h.render().to_vec();
    assert!(!patches.iter().any(|p| matches!(p, Patch::SetValue { .. })));
    // The user picks a day, then clears it; then clears the time.
    h.dispatch(UiEvent::Change(published, 86_400.));
    assert_eq!(h.host.get(published).value, Some(86_400.));
    let cleared = h.dispatch(UiEvent::Change(published, f64::NAN)).to_vec();
    assert!(
        cleared.iter().any(
            |p| matches!(p, Patch::SetValue { id, value } if *id == published && value.is_nan())
        )
    );
    h.dispatch(UiEvent::Change(opens, f64::NAN));
    assert!(h.host.get(opens).value.unwrap().is_nan());
    // A picker that is not optional is never emptied (its handler would
    // have read the epoch).
    h.dispatch(UiEvent::Change(due, f64::NAN));
    assert_eq!(h.host.get(due).value, Some(1000.));
    // Reading a cleared value with the plain accessors never panics.
    let empty = ChangeEvent { value: f64::NAN };
    assert_eq!((empty.date(), empty.minutes()), (std::time::UNIX_EPOCH, 0));
    assert_eq!((empty.date_opt(), empty.minutes_opt()), (None, None));
    assert_eq!(ChangeEvent { value: 60. }.date_opt(), Some(unix_time(60.)));
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
    let dir = crate::temp_dir::TempDir::new("pick");
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
}

struct Editor {
    picking: bool,
    closed: u32,
}

impl Render for Editor {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(text("Colour")).child(
            dialog(text("picker"))
                .open(self.picking)
                .side(PopoverSide::Trailing)
                .backdrop(Backdrop::Clear)
                .size(320., 0.)
                .on_dismiss(cx.listener(|this: &mut Self, _, _| {
                    this.picking = false;
                    this.closed += 1;
                })),
        )
    }
}

#[test]
fn a_dialog_can_open_beside_its_anchor_without_a_dim() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Editor {
            picking: false,
            closed: 0,
        });
        mount(cx, root);
    });
    let ov = h.host.find(|n| n.kind == NodeKind::Overlay).unwrap();
    assert_eq!(
        h.host.get(ov).config,
        "dialog side=trailing placed backdrop=clear size=320x0"
    );
    h.update::<Editor>(|e| e.picking = true);
    assert_eq!(h.host.get(ov).value, Some(1.));
    assert_eq!(h.host.child_texts(ov), ["picker"]);
    // A click outside or Escape: the app hears it, as for any dialog (#79).
    h.dispatch(UiEvent::Back(ov, 0));
    assert_eq!(h.rt.app.read(&h.root::<Editor>()).closed, 1);
    assert!(!h.rt.app.read(&h.root::<Editor>()).picking);
}

struct Bare;

impl Render for Bare {
    fn render(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text("Colour"))
            .child(
                dialog(text("picker"))
                    .side(PopoverSide::Trailing)
                    .backdrop(Backdrop::Clear)
                    .bare(),
            )
            .child(dialog(text("centered")).bare())
            .child(sheet(text("sheet")).bare())
            .child(popover(text("popover")).bare())
    }
}

#[test]
fn a_bare_dialog_drops_prisms_box_and_stays_a_dialog() {
    use prism_core::controls::OverlayConfig;
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Bare);
        mount(cx, root);
    });
    let config = |kind: &str| {
        let id = h
            .host
            .find(|n| n.kind == NodeKind::Overlay && n.config.split(' ').next() == Some(kind))
            .unwrap();
        OverlayConfig::decode(&h.host.get(id).config)
    };
    let placed = config("dialog");
    assert!(placed.bare && placed.placed);
    assert_eq!(placed.anchored_side(), Some(PopoverSide::Trailing));
    // Centered, without its box.
    assert!(h.host.find(|n| n.config == "dialog bare").is_some());
    // Sheets keep their platform's box (#166); popovers drop it too (#287).
    assert!(!config("sheet").bare);
    assert!(config("popover").bare);
}

struct Tasks {
    names: Vec<&'static str>,
    list: Option<Entity<ReorderList>>,
}

impl Render for Tasks {
    fn render(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(self.list.unwrap())
    }
}

/// A list of three tasks, the app's data reordered by `on_move`.
fn tasks() -> Harness {
    Harness::new(|cx| {
        let root = cx.new_entity(|_| Tasks {
            names: vec!["Write", "Test", "Ship"],
            list: None,
        });
        let list = ReorderList::new(
            cx,
            ["w", "t", "s"],
            40.,
            move |i, app| text(app.read(&root).names[i]),
            move |from, to, app| {
                root.update(app, |t, _| {
                    let name = t.names.remove(from);
                    t.names.insert(to, name);
                })
            },
        );
        root.update(cx, |t, _| t.list = Some(list));
        mount(cx, root);
    })
}

fn names(h: &Harness) -> Vec<&'static str> {
    h.rt.app.read(&h.root::<Tasks>()).names.clone()
}

fn row_texts(h: &Harness) -> Vec<String> {
    h.host
        .mounted()
        .into_iter()
        .filter(|id| h.host.get(*id).kind == NodeKind::Text && !h.host.get(*id).a11y.hidden)
        .map(|id| h.host.get(id).text.clone())
        .collect()
}

fn handle(h: &Harness, n: usize) -> prism_core::NodeId {
    h.host
        .find(|x| x.a11y.value.as_deref() == Some(&format!("{n} of 3")))
        .expect("a handle")
}

fn drag(phase: DragPhase, dy: f32) -> DragEvent {
    DragEvent {
        phase,
        start: Point::new(20., 20.),
        translation: Point::new(0., dy),
        button: prism_core::PointerButton::Primary,
        modifiers: Default::default(),
    }
}

#[test]
fn a_row_dragged_by_its_handle_moves_and_the_others_make_room() {
    let mut h = tasks();
    assert_eq!(row_texts(&h), ["Write", "Test", "Ship"]);
    let first = handle(&h, 1);
    let grip = h.host.get(first);
    assert_eq!(grip.a11y.label.as_deref(), Some("Reorder"));
    assert!(grip.a11y.hint.as_deref().unwrap().contains("arrow keys"));
    assert_ne!(grip.pointer_mask, 0, "the handle reports drags");
    assert!(grip.keys.0, "the handle takes the focus and keys");
    // Dragged down past the next two rows' middles: it would go last.
    h.dispatch(UiEvent::Drag(first, drag(DragPhase::Start, 4.)));
    h.dispatch(UiEvent::Drag(first, drag(DragPhase::Move, 70.)));
    let list = h.rt.app.read(&h.root::<Tasks>()).list.unwrap();
    assert_eq!(h.rt.app.read(&list).dragging(), Some((0, 2)));
    // The lifted copy is drawn too, hidden from screen readers; the app's
    // data has not moved yet.
    assert_eq!(names(&h), ["Write", "Test", "Ship"]);
    h.dispatch(UiEvent::Drag(first, drag(DragPhase::End, 70.)));
    assert_eq!(names(&h), ["Test", "Ship", "Write"]);
    assert_eq!(h.rt.app.read(&list).keys(), ["t", "s", "w"]);
    assert_eq!(row_texts(&h), ["Test", "Ship", "Write"]);
    assert_eq!(h.rt.app.read(&list).dragging(), None);
    // A cancelled drag (the platform took the gesture) moves nothing; a
    // short one puts the row back.
    let last = handle(&h, 3);
    h.dispatch(UiEvent::Drag(last, drag(DragPhase::Move, -90.)));
    h.dispatch(UiEvent::Drag(last, drag(DragPhase::Cancel, -90.)));
    h.dispatch(UiEvent::Drag(last, drag(DragPhase::Move, -12.)));
    h.dispatch(UiEvent::Drag(last, drag(DragPhase::End, -12.)));
    assert_eq!(names(&h), ["Test", "Ship", "Write"]);
}

#[test]
fn arrow_keys_on_a_handle_move_its_row_by_one() {
    let mut h = tasks();
    let second = handle(&h, 2);
    h.dispatch(UiEvent::Key(Some(second), KeyEvent::new("ArrowUp")));
    assert_eq!(names(&h), ["Test", "Write", "Ship"]);
    // Now first: up does nothing; End sends it last.
    let first = handle(&h, 1);
    h.dispatch(UiEvent::Key(Some(first), KeyEvent::new("ArrowUp")));
    assert_eq!(names(&h), ["Test", "Write", "Ship"]);
    h.dispatch(UiEvent::Key(Some(first), KeyEvent::new("End")));
    assert_eq!(names(&h), ["Write", "Ship", "Test"]);
    h.dispatch(UiEvent::Key(
        Some(handle(&h, 2)),
        KeyEvent::new("ArrowDown"),
    ));
    assert_eq!(names(&h), ["Write", "Test", "Ship"]);
    assert_eq!(row_texts(&h), ["Write", "Test", "Ship"]);
}

/// A field under a suggestion menu: the menu takes ↑ ↓ ↵ while open.
struct Suggest {
    open: bool,
}

impl Render for Suggest {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let keys: &[&str] = if self.open {
            &["ArrowUp", "ArrowDown", "Enter"]
        } else {
            &[]
        };
        div()
            .child(text_input().id("query"))
            .on_key_down(Rc::new(|_: &KeyEvent, _: &mut App| {}))
            .consume_keys(keys)
    }
}

#[test]
fn consumed_keys_are_taken_from_the_focused_field_while_declared() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Suggest { open: true });
        mount(cx, root);
    });
    let root = h.host.root.unwrap();
    let field = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    assert_eq!(
        h.host.get(root).consumed_keys,
        ["ArrowUp", "ArrowDown", "Enter"]
    );
    // The field's keys go up to its ancestors' declarations (#181).
    assert!(h.rt.consumes_key(Some(field), "ArrowDown", None));
    assert!(h.rt.consumes_key(Some(root), "Enter", None));
    assert!(!h.rt.consumes_key(Some(field), "a", None));
    assert!(!h.rt.consumes_key(None, "ArrowDown", None));
    // The menu closed: the field gets its keys back, with one patch.
    let p = h.update::<Suggest>(|s| s.open = false).to_vec();
    assert!(p.contains(&Patch::SetConsumedKeys {
        id: root,
        keys: vec![]
    }));
    assert!(!h.rt.consumes_key(Some(field), "ArrowDown", None));
    // An unrelated render sends nothing.
    h.rt.app.refresh();
    let again = h.render().to_vec();
    assert!(
        !again
            .iter()
            .any(|p| matches!(p, Patch::SetConsumedKeys { .. }))
    );
}

/// A row that reports its frame (#192), mounted while `shown`.
struct Framed {
    shown: bool,
    seen: Vec<prism_core::Rect>,
}

impl Render for Framed {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().when(self.shown, |d| {
            d.child(
                div()
                    .id("row")
                    .on_layout(cx.listener(|this, e: &LayoutEvent, _| this.seen.push(e.frame))),
            )
        })
    }
}

#[test]
fn laid_out_frames_reach_on_layout_and_frame_of() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Framed {
            shown: true,
            seen: vec![],
        });
        mount(cx, root);
    });
    let row = h.host.find(|n| n.reports_layout).expect("the row reports");
    assert_eq!(h.rt.app.frame_of("row"), None, "not laid out yet");
    let frame = prism_core::Rect::new(0., 40., 300., 24.);
    h.dispatch(UiEvent::Layout(row, frame));
    assert_eq!(h.root::<Framed>().read(&h.rt.app).seen, vec![frame]);
    assert_eq!(h.rt.app.frame_of("row"), Some(frame));
    assert!(frame.contains(prism_core::Point::new(10., 50.)));
    assert!(!frame.contains(prism_core::Point::new(10., 64.)));
    // Unmounted: no frame.
    h.update::<Framed>(|f| f.shown = false);
    assert_eq!(h.rt.app.frame_of("row"), None);
}

/// A list the app scrolls (#193), and a plain view with an id.
struct Scrolled;

impl Render for Scrolled {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(
                scroll()
                    .id("list")
                    .on_scroll(cx.listener(|_, _: &ScrollEvent, _| {}))
                    .child(text("rows")),
            )
            .child(div().id("plain"))
    }
}

#[test]
fn the_app_scrolls_a_scroll_view_by_or_to_an_offset() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Scrolled);
        mount(cx, root);
    });
    let list = h.host.find(|n| n.kind == NodeKind::Scroll).unwrap();
    h.rt.app.scroll_by("list", 0., 24.);
    h.rt.app.scroll_to("list", 0., f32::MAX, true);
    // Only scroll views scroll.
    h.rt.app.scroll_by("plain", 0., 24.);
    h.rt.app.scroll_by("missing", 0., 24.);
    h.rt.app.refresh();
    let p = h.render().to_vec();
    let scrolls: Vec<_> = p
        .iter()
        .filter(|p| matches!(p, Patch::ScrollTo { .. }))
        .cloned()
        .collect();
    assert_eq!(
        scrolls,
        [
            Patch::ScrollTo {
                id: list,
                x: 0.,
                y: 24.,
                relative: true,
                animated: false
            },
            Patch::ScrollTo {
                id: list,
                x: 0.,
                y: f32::MAX,
                relative: false,
                animated: true
            },
        ]
    );
    // Its offset, once it reported one.
    assert_eq!(h.rt.app.scroll_offset("list"), None);
    h.dispatch(UiEvent::Scroll(list, 120., 300.));
    assert_eq!(h.rt.app.scroll_offset("list"), Some(120.));
}

/// #201: rows as tall as their content, the app's handle at the leading edge.
#[test]
fn rows_of_their_own_height_take_the_slot_their_edge_reaches() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Tasks {
            names: vec!["Write", "Test", "Ship"],
            list: None,
        });
        let list = ReorderList::with_options(
            cx,
            ["w", "t", "s"],
            prism_core::ReorderOptions::default()
                .handle_leading()
                .handle(|_, _| text("≡")),
            move |i, app| text(app.read(&root).names[i]),
            move |from, to, app| {
                root.update(app, |t, _| {
                    let name = t.names.remove(from);
                    t.names.insert(to, name);
                })
            },
        );
        root.update(cx, |t, _| t.list = Some(list));
        mount(cx, root);
    });
    // The app's grip, before each row's content.
    let first = handle(&h, 1);
    assert_eq!(h.host.child_texts(first), ["≡"]);
    let row = h.host.find(|n| n.children.contains(&first)).expect("a row");
    assert_eq!(h.host.get(row).children.first(), Some(&first), "leading");
    // Measured: a tall first row (100), two short ones (20).
    let rows: Vec<_> = h
        .host
        .mounted()
        .into_iter()
        .filter(|id| h.host.get(*id).reports_layout)
        .collect();
    assert_eq!(rows.len(), 3);
    for (id, height) in rows.iter().zip([100., 20., 20.]) {
        h.dispatch(UiEvent::Layout(
            *id,
            prism_core::Rect::new(0., 0., 300., height),
        ));
    }
    // 30 points down: its bottom (130) passed the next row's middle (110),
    // not the last one's (130). A 40-point row would not have moved.
    h.dispatch(UiEvent::Drag(first, drag(DragPhase::Start, 4.)));
    h.dispatch(UiEvent::Drag(first, drag(DragPhase::Move, 30.)));
    let list = h.rt.app.read(&h.root::<Tasks>()).list.unwrap();
    assert_eq!(h.rt.app.read(&list).dragging(), Some((0, 1)));
    h.dispatch(UiEvent::Drag(first, drag(DragPhase::End, 30.)));
    assert_eq!(names(&h), ["Test", "Write", "Ship"]);
}

/// #207: a video's node carries its source and headers, its config, and
/// hears the end of the playback.
struct Clip {
    ended: u32,
}

impl Render for Clip {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(
            prism_core::video("https://example.com/talk.mp4")
                .header("Authorization", "Bearer s3cret")
                .poster("talk.jpg")
                .muted(true)
                .on_ended(cx.listener(|this, _: &prism_core::EndedEvent, _| this.ended += 1)),
        )
    }
}

#[test]
fn a_video_carries_its_source_config_and_hears_its_end() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Clip { ended: 0 });
        mount(cx, root);
    });
    let v = h.host.find(|n| n.kind == NodeKind::Video).expect("a video");
    let node = h.host.get(v);
    assert_eq!(
        node.text,
        "https://example.com/talk.mp4\nAuthorization: Bearer s3cret"
    );
    let c = prism_core::VideoConfig::decode(&node.config);
    assert!(c.controls && c.muted && !c.autoplay);
    assert_eq!(c.poster.as_deref(), Some("talk.jpg"));
    h.dispatch(UiEvent::Ended(v));
    assert_eq!(h.root::<Clip>().read(&h.rt.app).ended, 1);
}

/// #211: blocks of a document: ↑ leaves a field from its first visual line,
/// ↓ from its last, ← from its start; elsewhere the field keeps them.
struct Blocks;

impl Render for Blocks {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text_area().value("one\ntwo"))
            .on_key_down(Rc::new(|_: &KeyEvent, _: &mut App| {}))
            .consume_keys(&["Escape"])
            .consume_key_at("ArrowUp", CaretEdge::FirstLine)
            .consume_key_at("ArrowDown", CaretEdge::LastLine)
            .consume_key_at("ArrowLeft", CaretEdge::Start)
    }
}

#[test]
fn keys_are_taken_from_a_field_at_the_caret_edges_declared() {
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Blocks);
        mount(cx, root);
    });
    let root = h.host.root.unwrap();
    let field = h.host.find(|n| n.kind == NodeKind::TextArea).unwrap();
    assert_eq!(
        h.host.get(root).consumed_keys,
        [
            "Escape",
            "ArrowUp@first-line",
            "ArrowDown@last-line",
            "ArrowLeft@start"
        ]
    );
    let first = CaretEdges::caret(1, 7, 0, 2);
    let last = CaretEdges::caret(5, 7, 1, 2);
    let start = CaretEdges::caret(0, 7, 0, 2);
    let up = |e| h.rt.consumes_key(Some(field), "ArrowUp", Some(e));
    assert!(up(first) && up(start) && !up(last));
    assert!(h.rt.consumes_key(Some(field), "ArrowDown", Some(last)));
    assert!(!h.rt.consumes_key(Some(field), "ArrowDown", Some(first)));
    assert!(h.rt.consumes_key(Some(field), "ArrowLeft", Some(start)));
    assert!(!h.rt.consumes_key(Some(field), "ArrowLeft", Some(first)));
    // A selection: the key acts on it.
    assert!(
        !h.rt
            .consumes_key(Some(field), "ArrowUp", Some(CaretEdges::default()))
    );
    // No field focused: nothing to keep the key from.
    assert!(h.rt.consumes_key(Some(root), "ArrowUp", None));
    assert!(h.rt.consumes_key(Some(field), "Escape", Some(last)));
    assert!(!h.rt.consumes_key(Some(field), "ArrowRight", Some(last)));

    // The handler hears the same.
    let e = KeyEvent::new("ArrowUp").with_edges(first);
    assert!(e.at(CaretEdge::FirstLine) && !e.at(CaretEdge::LastLine));
    assert!(KeyEvent::new("ArrowUp").at(CaretEdge::FirstLine));
    // The wire format.
    assert_eq!(start.encode(), "sf");
    assert_eq!(CaretEdges::decode("sfl"), CaretEdges::caret(0, 3, 0, 1));
    assert_eq!(CaretEdges::caret(3, 3, 0, 1).encode(), "efl");
}

/// #215: tabs closed with the middle button; `on_click` stays the primary's.
struct Tabs {
    closed: Vec<String>,
    opened: u32,
}

impl Render for Tabs {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(
            div()
                .on_click(cx.listener(|this, _: &ClickEvent, _| this.opened += 1))
                .on_aux_click(cx.listener(|this, e: &ClickEvent, _| {
                    this.closed.push(format!("{:?} {}", e.button, e.count))
                })),
        )
    }
}

#[test]
fn middle_clicks_reach_on_aux_click() {
    use prism_core::events::pointer_mask::AUX_CLICK;
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Tabs {
            closed: Vec::new(),
            opened: 0,
        });
        mount(cx, root);
    });
    let root = h.host.root.unwrap();
    let tab = h.host.get(root).children[0];
    assert_eq!(h.host.get(tab).pointer_mask & AUX_CLICK, AUX_CLICK);
    h.dispatch(UiEvent::AuxClick(
        tab,
        ClickEvent {
            button: PointerButton::Middle,
            ..ClickEvent
        },
    ));
    h.dispatch(UiEvent::AuxClick(
        tab,
        ClickEvent {
            button: PointerButton::BACK,
            count: 2,
            ..ClickEvent
        },
    ));
    h.dispatch(UiEvent::Click(tab));
    let tabs = h.root::<Tabs>().read(&h.rt.app);
    assert_eq!(tabs.closed, ["Middle 1", "Other(3) 2"]);
    assert_eq!(tabs.opened, 1);
    assert_eq!(ClickEvent.button, PointerButton::Primary);
}

/// #216: tooltips travel with the accessibility description.
struct Tipped;

impl Render for Tipped {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(div().tooltip("Close tab")).child(
            text("A very long title")
                .truncate()
                .tooltip_when_truncated(),
        )
    }
}

#[test]
fn tooltips_reach_the_backend() {
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Tipped);
        mount(cx, root);
    });
    let root = h.host.root.unwrap();
    let close = h.host.get(root).children[0];
    let title = h.host.find(|n| n.kind == NodeKind::Text).unwrap();
    assert_eq!(
        h.host.get(close).a11y.tooltip,
        Some(prism_core::Tooltip::Text("Close tab".into()))
    );
    assert_eq!(
        h.host.get(title).a11y.tooltip,
        Some(prism_core::Tooltip::WhenTruncated)
    );
}

/// #218: a filled picture's focal point, and the crop it gives.
struct Cover;

impl Render for Cover {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(picture("cover.jpg").object_position(0.2, 1.))
            .child(picture("plain.jpg").object_position(0.5, 0.5))
    }
}

#[test]
fn pictures_carry_their_focal_point() {
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Cover);
        mount(cx, root);
    });
    let pics: Vec<_> = h.host.root.map(|r| h.host.get(r).children.clone()).unwrap();
    assert_eq!(h.host.get(pics[0]).config, "0.2 1");
    assert_eq!(h.host.get(pics[1]).config, "", "the center is the default");
    assert_eq!(prism_core::picture_position("0.2 1"), (0.2, 1.));
    assert_eq!(prism_core::picture_position(""), (0.5, 0.5));
    assert_eq!(prism_core::picture_position("x 3"), (0.5, 0.5));
    // A 400 × 100 image in a 100 × 100 frame shows a quarter of its width.
    assert_eq!(
        prism_core::cover_crop((400., 100.), (100., 100.), (0., 0.5)),
        (0., 0., 0.25, 1.)
    );
    assert_eq!(
        prism_core::cover_crop((400., 100.), (100., 100.), (1., 0.5)),
        (0.75, 0., 0.25, 1.)
    );
    // A tall image: the height is cropped.
    assert_eq!(
        prism_core::cover_crop((100., 400.), (100., 100.), (0.5, 1.)),
        (0., 0.75, 1., 0.25)
    );
}

/// #230: a button that only navigates is a link.
struct Cart;

impl Render for Cart {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(button("Checkout").link("/cart"))
    }
}

#[test]
fn buttons_can_be_links() {
    let h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Cart);
        mount(cx, root);
    });
    let b = h.host.find(|n| n.kind == NodeKind::Button).unwrap();
    assert_eq!(h.host.get(b).link.as_deref(), Some("/cart"));
    assert_eq!(h.host.get(b).a11y.role, Some(prism_core::Role::Link));
}

/// #224: a field commits its value when it loses the focus.
struct Rename {
    log: Vec<bool>,
}

impl Render for Rename {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(
            text_input()
                .on_focus_change(cx.listener(|this, e: &FocusEvent, _| this.log.push(e.focused))),
        )
    }
}

#[test]
fn fields_hear_focus_changes() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Rename { log: Vec::new() });
        mount(cx, root);
    });
    let field = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    h.dispatch(UiEvent::FocusChange(field, true));
    h.dispatch(UiEvent::FocusChange(field, false));
    assert_eq!(h.root::<Rename>().read(&h.rt.app).log, [true, false]);
}

// --- Drag and drop between elements (#251) -------------------------------------

struct Board {
    log: Log,
    cards: Vec<&'static str>,
}

impl Render for Board {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let column = |name: &'static str, log: &Log| {
            let (over, dropped) = (log.clone(), log.clone());
            div()
                .id(name)
                .on_drag_over(
                    "card",
                    Rc::new(move |e: &DragOverEvent, _: &mut App| {
                        over.borrow_mut().push(format!(
                            "{name} {:?} {} {}",
                            e.phase, e.payload.data, e.position.y
                        ))
                    }),
                )
                .on_drop_payload(
                    "card",
                    Rc::new(move |e: &DropEvent, _: &mut App| {
                        dropped.borrow_mut().push(format!(
                            "drop {name} {} {} {}",
                            e.payload.data, e.position.y, e.modifiers.alt
                        ))
                    }),
                )
        };
        let entity = cx.entity();
        div()
            .child(
                column("todo", &self.log).children(self.cards.iter().map(|c| {
                    div()
                        .id(*c)
                        .child(text(*c))
                        .draggable(DragPayload::new("card", *c))
                })),
            )
            .child(column("done", &self.log))
            // Takes other kinds only.
            .child(div().id("trash").on_drop_payload(
                "file",
                Rc::new(move |_: &DropEvent, app: &mut App| {
                    entity.update(app, |b, _| b.cards.clear())
                }),
            ))
    }
}

fn board() -> (Harness, Log) {
    let h = Harness::new(move |cx| {
        let log: Log = Rc::default();
        cx.set_global(log.clone());
        let root = cx.new_entity(|_| Board {
            log,
            cards: vec!["a", "b"],
        });
        mount(cx, root);
    });
    let log = h.rt.app.global::<Log>().clone();
    (h, log)
}

fn payload_drag(phase: DragPhase, alt: bool) -> DragEvent {
    DragEvent {
        phase,
        start: Point::new(5., 5.),
        translation: Point::new(0., 30.),
        button: prism_core::PointerButton::Primary,
        modifiers: Modifiers {
            alt,
            ..Modifiers::NONE
        },
    }
}

#[test]
fn a_draggable_payload_enters_moves_over_leaves_and_drops_on_targets_of_its_kind() {
    let (mut h, log) = board();
    let card = h.by_name("a");
    let todo = h.by_name("todo");
    let done = h.by_name("done");
    let trash = h.by_name("trash");
    assert_eq!(h.host.get(card).dnd, (Some("card".into()), vec![]));
    assert_ne!(h.host.get(card).pointer_mask & 4, 0, "it reports its drags");
    assert_eq!(h.host.get(todo).dnd, (None, vec!["card".into()]));
    assert_eq!(h.host.get(trash).dnd, (None, vec!["file".into()]));
    let at = |target, y| {
        Some(prism_core::DropPoint {
            target,
            position: Point::new(0., y),
        })
    };
    let ev = |phase, over| UiEvent::PayloadDrag(card, payload_drag(phase, false), over);
    h.dispatch(ev(DragPhase::Start, at(todo, 10.)));
    h.dispatch(ev(DragPhase::Move, at(todo, 20.)));
    // Over its own element, or a target of other kinds: over nothing.
    h.dispatch(ev(DragPhase::Move, at(card, 2.)));
    h.dispatch(ev(DragPhase::Move, at(trash, 2.)));
    h.dispatch(ev(DragPhase::Move, at(done, 40.)));
    h.dispatch(UiEvent::PayloadDrag(
        card,
        payload_drag(DragPhase::End, true),
        at(done, 44.),
    ));
    assert_eq!(
        *log.borrow(),
        [
            "todo Enter a 10",
            "todo Move a 20",
            "todo Leave a 20",
            "done Enter a 40",
            "drop done a 44 true",
            "done Leave a 44",
        ]
    );
    // A cancelled drag drops nothing; released over nothing, nothing either.
    log.borrow_mut().clear();
    h.dispatch(ev(DragPhase::Start, at(done, 1.)));
    h.dispatch(ev(DragPhase::Cancel, at(done, 1.)));
    h.dispatch(ev(DragPhase::Start, at(done, 1.)));
    h.dispatch(ev(DragPhase::End, None));
    assert_eq!(
        *log.borrow(),
        [
            "done Enter a 1",
            "done Leave a 1",
            "done Enter a 1",
            "done Leave a 1",
        ]
    );
    // A plain drag report (a backend that finds no target) is over nothing;
    // the test harness drags like a backend that does.
    log.borrow_mut().clear();
    h.dispatch(UiEvent::Drag(card, payload_drag(DragPhase::Start, false)));
    h.dispatch(UiEvent::Drag(card, payload_drag(DragPhase::End, false)));
    assert!(log.borrow().is_empty());
    let b = h.by_name("b");
    h.drag_payload_to(b, todo, Point::new(3., 7.));
    assert_eq!(
        log.borrow().last().map(String::as_str),
        Some("todo Leave b 7")
    );
    assert!(log.borrow().contains(&"drop todo b 7 false".to_string()));
}
