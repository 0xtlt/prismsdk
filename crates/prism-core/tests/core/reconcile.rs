//! Patch generation: keyed moves, inserts and removals, in-place updates.

use std::collections::HashMap;

use prism_core::prelude::*;
use prism_core::{Accessibility, Element, NodeId, NodeKind, Patch, UiEvent};

use prism_testing::{Harness, Lcg, count, creates, destroys, mount};

/// A keyed list of texts.
struct List {
    items: Vec<String>,
    keyed: bool,
}

impl Render for List {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().children(self.items.iter().map(|i| {
            let t = text(i.clone());
            if self.keyed { t.key(i.clone()) } else { t }
        }))
    }
}

fn list(items: &[&str], keyed: bool) -> Harness {
    let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    Harness::new(move |cx| {
        let items = items.clone();
        let root = cx.new_entity(|_| List { items, keyed });
        mount(cx, root);
    })
}

fn set_items(h: &mut Harness, items: &[&str]) -> Vec<Patch> {
    let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    h.update::<List>(|l| l.items = items).to_vec()
}

/// Node id of each key.
fn ids(h: &Harness) -> HashMap<String, NodeId> {
    let root = h.host.root.unwrap();
    h.host
        .get(root)
        .children
        .iter()
        .map(|c| (h.host.get(*c).text.clone(), *c))
        .collect()
}

#[test]
fn first_frame_is_well_formed() {
    let h = list(&["a", "b", "c"], true);
    assert_eq!(h.describe(), r#"view[text"a" text"b" text"c"]"#);
    // Environment first (text is created at the right scale and direction).
    assert!(matches!(h.last[0], Patch::SetEnvironment { .. }));
    assert_eq!(creates(&h.last), 4);
    assert_eq!(count(&h.last, |p| matches!(p, Patch::SetRoot { .. })), 1);
}

#[test]
fn keyed_insert_in_the_middle_creates_one_node() {
    let mut h = list(&["a", "b", "c"], true);
    let before = ids(&h);
    let p = set_items(&mut h, &["a", "x", "b", "c"]);
    assert_eq!(creates(&p), 1);
    assert!(destroys(&p).is_empty());
    assert_eq!(h.describe(), r#"view[text"a" text"x" text"b" text"c"]"#);
    let after = ids(&h);
    for k in ["a", "b", "c"] {
        assert_eq!(before[k], after[k], "{k} kept its node");
    }
    // The existing nodes are not touched: only the new one and the list.
    assert!(p.iter().all(|p| match p {
        Patch::SetText { id, .. } | Patch::SetStyle { id, .. } => *id == after["x"],
        _ => true,
    }));
}

#[test]
fn keyed_removal_in_the_middle_destroys_exactly_that_node() {
    let mut h = list(&["a", "b", "c", "d"], true);
    let before = ids(&h);
    let p = set_items(&mut h, &["a", "c", "d"]);
    assert_eq!(destroys(&p), vec![before["b"]]);
    assert_eq!(creates(&p), 0);
    assert_eq!(h.describe(), r#"view[text"a" text"c" text"d"]"#);
    // A destroyed node's handlers are gone: clicking it is a no-op.
    assert!(h.dispatch(UiEvent::Click(before["b"])).is_empty());
}

#[test]
fn keyed_swap_moves_nodes_with_a_single_children_patch() {
    let mut h = list(&["a", "b", "c"], true);
    let before = ids(&h);
    let p = set_items(&mut h, &["c", "b", "a"]);
    assert_eq!(creates(&p), 0);
    assert!(destroys(&p).is_empty());
    assert_eq!(
        p,
        vec![Patch::SetChildren {
            id: h.host.root.unwrap(),
            children: vec![before["c"], before["b"], before["a"]],
        }]
    );
}

#[test]
fn unkeyed_children_are_patched_by_position() {
    let mut h = list(&["a", "b", "c"], false);
    let before = ids(&h);
    let p = set_items(&mut h, &["a", "B", "c"]);
    assert_eq!(
        p,
        vec![Patch::SetText {
            id: before["b"],
            text: "B".into()
        }]
    );
    // Removing the first unkeyed child shifts texts and drops the last node.
    let p = set_items(&mut h, &["B", "c"]);
    assert_eq!(destroys(&p), vec![before["c"]]);
    assert_eq!(h.describe(), r#"view[text"B" text"c"]"#);
}

#[test]
fn duplicate_keys_still_render_every_child() {
    let mut h = list(&["a", "a", "b"], true);
    assert_eq!(h.describe(), r#"view[text"a" text"a" text"b"]"#);
    set_items(&mut h, &["b", "a", "a", "a"]);
    assert_eq!(h.describe(), r#"view[text"b" text"a" text"a" text"a"]"#);
}

/// Random edits of a keyed list: the tree always matches, and every key that
/// survives an edit keeps its native node.
#[test]
fn random_keyed_edits_preserve_identity() {
    let mut rng = Lcg(7);
    let mut next_key = 0;
    let mut items: Vec<String> = (0..8)
        .map(|_| {
            next_key += 1;
            format!("k{next_key}")
        })
        .collect();
    let init: Vec<&str> = items.iter().map(String::as_str).collect();
    let mut h = list(&init, true);
    for _ in 0..300 {
        let before = ids(&h);
        match rng.below(4) {
            0 => {
                next_key += 1;
                let at = rng.below(items.len() + 1);
                items.insert(at, format!("k{next_key}"));
            }
            1 if !items.is_empty() => {
                let at = rng.below(items.len());
                items.remove(at);
            }
            2 if items.len() > 1 => {
                let (a, b) = (rng.below(items.len()), rng.below(items.len()));
                items.swap(a, b);
            }
            _ => {
                // Rotate.
                if !items.is_empty() {
                    let n = rng.below(items.len());
                    items.rotate_left(n);
                }
            }
        }
        let now: Vec<&str> = items.iter().map(String::as_str).collect();
        let p = set_items(&mut h, &now);
        let root = h.host.root.unwrap();
        assert_eq!(h.host.child_texts(root), items);
        let after = ids(&h);
        for (k, id) in &after {
            if let Some(old) = before.get(k) {
                assert_eq!(old, id, "{k} was recreated");
            }
        }
        let added = after.keys().filter(|k| !before.contains_key(*k)).count();
        let removed = before.keys().filter(|k| !after.contains_key(*k)).count();
        assert_eq!(creates(&p), added);
        assert_eq!(destroys(&p).len(), removed);
    }
}

// --- In-place updates -------------------------------------------------------

#[derive(Default)]
struct Props {
    color: bool,
    label: Option<&'static str>,
    on: bool,
    disabled: bool,
    kind_button: bool,
    placeholder: &'static str,
    hidden_hint: bool,
}

impl Render for Props {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let mut t = text("styled");
        if self.color {
            t = t.text_color(rgb(0xff0000));
        }
        let mut b = button("act").accessibility_disabled(self.disabled);
        if let Some(l) = self.label {
            b = b.accessibility_label(l);
        }
        if self.hidden_hint {
            b = b.accessibility_hint("Does things");
        }
        let swap = if self.kind_button {
            button("swap").into_element()
        } else {
            text("swap").into_element()
        };
        div()
            .child(t)
            .child(b)
            .child(toggle(self.on))
            .child(swap)
            .child(text_input().placeholder(self.placeholder))
    }
}

fn props() -> Harness {
    Harness::new(|cx| {
        let root = cx.new_entity(|_| Props {
            placeholder: "Name",
            ..Default::default()
        });
        mount(cx, root);
    })
}

#[test]
fn style_change_is_one_set_style() {
    let mut h = props();
    let id = h.host.by_text("styled");
    let p = h.update::<Props>(|s| s.color = true).to_vec();
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(
        matches!(&p[0], Patch::SetStyle { id: i, style } if *i == id && style.text_color.is_some())
    );
}

#[test]
fn accessibility_updates_only_send_the_new_description() {
    let mut h = props();
    let act = h.host.by_text("act");
    assert_eq!(h.host.get(act).a11y.role, Some(Role::Button));
    let p = h.update::<Props>(|s| s.label = Some("Do it")).to_vec();
    assert_eq!(
        p,
        vec![Patch::SetAccessibility {
            id: act,
            a11y: Accessibility {
                role: Some(Role::Button),
                label: Some("Do it".into()),
                ..Default::default()
            }
        }]
    );
    h.update::<Props>(|s| {
        s.disabled = true;
        s.hidden_hint = true;
    });
    let a = &h.host.get(act).a11y;
    assert!(a.disabled);
    assert_eq!(a.hint.as_deref(), Some("Does things"));
    // Back to defaults.
    h.update::<Props>(|s| {
        s.disabled = false;
        s.hidden_hint = false;
        s.label = None;
    });
    assert_eq!(h.host.get(act).a11y.label, None);
    assert!(!h.host.get(act).a11y.disabled);
}

#[test]
fn toggles_send_value_and_checked_state() {
    let mut h = props();
    let t = h.host.find(|n| n.kind == NodeKind::Toggle).unwrap();
    assert_eq!(h.host.get(t).value, Some(0.));
    assert_eq!(h.host.get(t).a11y.checked, Some(false));
    let p = h.update::<Props>(|s| s.on = true).to_vec();
    assert!(p.contains(&Patch::SetValue { id: t, value: 1. }));
    assert_eq!(h.host.get(t).a11y.role, Some(Role::Switch));
    assert_eq!(h.host.get(t).a11y.checked, Some(true));
    // Same value: nothing.
    assert!(h.update::<Props>(|s| s.on = true).is_empty());
}

#[test]
fn a_kind_change_replaces_the_node() {
    let mut h = props();
    let old = h.host.by_text("swap");
    let p = h.update::<Props>(|s| s.kind_button = true).to_vec();
    assert_eq!(destroys(&p), vec![old]);
    assert_eq!(creates(&p), 1);
    let new = h.host.by_text("swap");
    assert_eq!(h.host.get(new).kind, NodeKind::Button);
}

#[test]
fn placeholders_label_text_inputs() {
    let mut h = props();
    let input = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    assert_eq!(h.host.get(input).placeholder, "Name");
    assert_eq!(h.host.get(input).a11y.label.as_deref(), Some("Name"));
    let p = h.update::<Props>(|s| s.placeholder = "Email").to_vec();
    assert!(p.contains(&Patch::SetPlaceholder {
        id: input,
        text: "Email".into()
    }));
    assert_eq!(h.host.get(input).a11y.label.as_deref(), Some("Email"));
}

/// #281: a timer's `after` text reaches backends as its placeholder, and a
/// change of it as a patch.
#[test]
fn a_timers_after_text_is_its_placeholder() {
    struct Countdown {
        after: &'static str,
    }
    impl Render for Countdown {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            let past = std::time::SystemTime::now() - std::time::Duration::from_secs(5);
            div().child(timer(past).after(self.after))
        }
    }
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Countdown { after: "Departed" });
        mount(cx, root);
    });
    let t = h.host.find(|n| n.kind == NodeKind::Timer).unwrap();
    assert_eq!(h.host.get(t).placeholder, "Departed");
    assert_eq!(h.host.get(t).text, "Departed");
    let p = h.update::<Countdown>(|s| s.after = "Gone").to_vec();
    assert!(p.contains(&Patch::SetPlaceholder {
        id: t,
        text: "Gone".into()
    }));
}

// --- Clickable containers and nested entities ------------------------------

struct Card {
    badge: Entity<Badge>,
    title: String,
}

struct Badge {
    count: i32,
}

impl Render for Badge {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        text(format!("{} new", self.count))
    }
}

impl Render for Card {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .on_click(cx.listener(|_, _, _| {}))
            .child(text(self.title.clone()))
            .child(text("decoration").accessibility_hidden())
            .child(self.badge)
    }
}

fn card() -> (Harness, Entity<Badge>) {
    let h = Harness::new(|cx| {
        let badge = cx.new_entity(|_| Badge { count: 1 });
        cx.set_global(badge);
        let root = cx.new_entity(|_| Card {
            badge,
            title: "Inbox".into(),
        });
        mount(cx, root);
    });
    let badge = *h.rt.app.global::<Entity<Badge>>();
    (h, badge)
}

#[test]
fn clickable_views_are_buttons_named_by_their_visible_text() {
    let (h, _) = card();
    let root = h.host.root.unwrap();
    let a = &h.host.get(root).a11y;
    assert_eq!(a.role, Some(Role::Button));
    assert_eq!(a.label.as_deref(), Some("Inbox, 1 new"));
    assert!(h.host.get(root).clickable);
}

#[test]
fn a_clickable_views_label_follows_its_own_rerender() {
    let (mut h, _) = card();
    h.update::<Card>(|c| c.title = "Archive".into());
    let root = h.host.root.unwrap();
    assert_eq!(
        h.host.get(root).a11y.label.as_deref(),
        Some("Archive, 1 new")
    );
}

#[test]
fn a_clickable_views_label_follows_a_child_entity_rerender() {
    let (mut h, badge) = card();
    badge.update(&mut h.rt.app, |b, cx| {
        b.count = 5;
        cx.notify();
    });
    h.render();
    let root = h.host.root.unwrap();
    // Only the child entity re-rendered, but the button's name includes it.
    assert_eq!(h.rt.last_render_count(), 1);
    assert_eq!(h.host.get(root).a11y.label.as_deref(), Some("Inbox, 5 new"));
}

// --- Entities moving between containers ------------------------------------

struct Mover {
    child: Entity<Clicks>,
    left: bool,
}

struct Clicks {
    n: u32,
}

impl Render for Clicks {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        button(format!("clicked {}", self.n)).on_click(cx.listener(|this, _, _| this.n += 1))
    }
}

impl Render for Mover {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let (l, r) = if self.left {
            (Some(self.child), None)
        } else {
            (None, Some(self.child))
        };
        div()
            .child(div().key("left").child(text("L")).children(l))
            .child(div().key("right").child(text("R")).children(r))
    }
}

#[test]
fn a_child_entity_moved_to_another_container_keeps_working() {
    let mut h = Harness::new(|cx| {
        let child = cx.new_entity(|_| Clicks { n: 0 });
        let root = cx.new_entity(|_| Mover { child, left: true });
        mount(cx, root);
    });
    assert_eq!(
        h.describe(),
        r#"view[view[text"L" button"clicked 0"] view[text"R"]]"#
    );
    h.click("clicked 0");
    h.update::<Mover>(|m| m.left = false);
    assert_eq!(
        h.describe(),
        r#"view[view[text"L"] view[text"R" button"clicked 1"]]"#
    );
    // Its handler is wired to the new node, and its state survived.
    h.click("clicked 1");
    assert_eq!(
        h.describe(),
        r#"view[view[text"L"] view[text"R" button"clicked 2"]]"#
    );
    h.update::<Mover>(|m| m.left = true);
    h.click("clicked 2");
    assert_eq!(
        h.describe(),
        r#"view[view[text"L" button"clicked 3"] view[text"R"]]"#
    );
}

// --- Fragments, lazy elements, empty ----------------------------------------

struct Mixed {
    show: bool,
}

impl Render for Mixed {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        let show = self.show;
        div()
            .child(text("first"))
            .child(Element::Fragment(vec![
                text("f1").into_element(),
                Element::Empty,
                Element::Fragment(vec![text("f2").into_element()]),
            ]))
            .child(lazy(move |_| {
                if show {
                    text("lazy").into_element()
                } else {
                    Element::Empty
                }
            }))
            .child(text("last"))
    }
}

#[test]
fn fragments_flatten_and_lazy_elements_come_and_go() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Mixed { show: false });
        mount(cx, root);
    });
    assert_eq!(
        h.describe(),
        r#"view[text"first" text"f1" text"f2" text"last"]"#
    );
    let last = h.host.by_text("last");
    h.update::<Mixed>(|m| m.show = true);
    assert_eq!(
        h.describe(),
        r#"view[text"first" text"f1" text"f2" text"lazy" text"last"]"#
    );
    // Unkeyed: "last" moved a slot down, so its node now shows "lazy".
    assert_eq!(h.host.get(last).text, "lazy");
    h.update::<Mixed>(|m| m.show = false);
    assert_eq!(
        h.describe(),
        r#"view[text"first" text"f1" text"f2" text"last"]"#
    );
}

// --- Environment and appearance ---------------------------------------------

struct Hello;

impl Render for Hello {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        text(cx.t("hello"))
    }
}

#[test]
fn locale_changes_send_one_environment_patch_before_node_patches() {
    let mut h = Harness::new(|cx| {
        cx.set_translations(
            Translations::new()
                .add("en", &[("hello", "Hello")])
                .add("ar", &[("hello", "مرحبا")]),
        );
        let root = cx.new_entity(|_| Hello);
        mount(cx, root);
    });
    let env = h.host.env.clone().unwrap();
    assert_eq!(env.direction, prism_core::Direction::Ltr);
    assert_eq!(env.back, "Back");
    h.rt.app.set_locale(Some(Locale::new("ar")));
    let p = h.render().to_vec();
    let Patch::SetEnvironment { env } = &p[0] else {
        panic!("environment first: {p:?}")
    };
    assert_eq!(env.lang, "ar");
    assert_eq!(env.direction, prism_core::Direction::Rtl);
    assert_eq!(env.back, "رجوع");
    assert_eq!(count(&p, |p| matches!(p, Patch::SetEnvironment { .. })), 1);
    assert_eq!(h.describe(), r#"text"مرحبا""#);
    // Nothing changed: no environment patch again.
    h.rt.app.refresh();
    let p = h.render().to_vec();
    assert_eq!(count(&p, |p| matches!(p, Patch::SetEnvironment { .. })), 0);
}

#[test]
fn appearance_changes_are_sent_once() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Hello);
        mount(cx, root);
    });
    h.rt.app.set_appearance(prism_core::Appearance::Dark);
    let p = h.render().to_vec();
    assert_eq!(
        p.last(),
        Some(&Patch::SetAppearance {
            appearance: prism_core::Appearance::Dark
        })
    );
    h.rt.app.refresh();
    let p = h.render().to_vec();
    assert_eq!(count(&p, |p| matches!(p, Patch::SetAppearance { .. })), 0);
}

// --- Handlers follow keyed moves ---------------------------------------------

struct Rows {
    rows: Vec<u32>,
    hits: Vec<u32>,
}

impl Render for Rows {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div().children(self.rows.iter().map(|&r| {
            button(format!("row {r}"))
                .key(r.to_string())
                .on_click(cx.listener(move |this, _, _| this.hits.push(r)))
        }))
    }
}

#[test]
fn click_handlers_follow_their_keyed_row() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Rows {
            rows: vec![1, 2, 3],
            hits: vec![],
        });
        mount(cx, root);
    });
    let two = h.host.by_text("row 2");
    h.update::<Rows>(|r| r.rows = vec![3, 2, 1]);
    h.update::<Rows>(|r| r.rows.retain(|&x| x != 1));
    h.dispatch(UiEvent::Click(two));
    h.click("row 3");
    let root = h.root::<Rows>();
    assert_eq!(root.read(&h.rt.app).hits, vec![2, 3]);
}

// --- Context menus -----------------------------------------------------------

struct WithMenu {
    menu: bool,
    picked: Vec<&'static str>,
}

impl Render for WithMenu {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut d = div().child(text("target"));
        if self.menu {
            d = d.context_menu(
                Menu::new()
                    .item(
                        MenuItem::new("Copy")
                            .on_select(cx.listener(|t, _, _| t.picked.push("copy"))),
                    )
                    .item(
                        MenuItem::new("Delete")
                            .on_select(cx.listener(|t, _, _| t.picked.push("delete"))),
                    ),
            );
        }
        d
    }
}

#[test]
fn context_menus_are_added_kept_and_removed() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| WithMenu {
            menu: true,
            picked: vec![],
        });
        mount(cx, root);
    });
    let root = h.host.root.unwrap();
    assert!(h.host.get(root).has_menu);
    let spec = h
        .last
        .iter()
        .find_map(|p| match p {
            Patch::SetMenu { menu: Some(m), .. } => Some(m.clone()),
            _ => None,
        })
        .unwrap();
    // An unrelated re-render keeps the same menu: no patch.
    h.rt.app.refresh();
    assert_eq!(count(h.render(), |p| matches!(p, Patch::SetMenu { .. })), 0);
    let ids = menu_ids(&spec);
    assert_eq!(ids.len(), 2);
    h.dispatch(UiEvent::MenuSelect(ids[1]));
    h.dispatch(UiEvent::MenuSelect(ids[0]));
    let picked = h.root::<WithMenu>().read(&h.rt.app).picked.clone();
    assert_eq!(picked, vec!["delete", "copy"]);
    // Removed: one patch, and its items no longer dispatch.
    let p = h.update::<WithMenu>(|m| m.menu = false).to_vec();
    assert!(p.contains(&Patch::SetMenu {
        id: root,
        menu: None
    }));
    h.dispatch(UiEvent::MenuSelect(ids[0]));
    assert_eq!(h.root::<WithMenu>().read(&h.rt.app).picked.len(), 2);
}

fn menu_ids(spec: &prism_core::menu::MenuSpec) -> Vec<u64> {
    spec.items
        .iter()
        .filter_map(|i| match i {
            prism_core::menu::MenuItemSpec::Action { id, .. } => Some(*id),
            _ => None,
        })
        .collect()
}

/// A "+" button with a click menu, and a canvas (#247).
struct Popups {
    picked: Vec<&'static str>,
}

impl Render for Popups {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let note = MenuItem::new("Note").on_select(cx.listener(|t, _, _| t.picked.push("note")));
        div()
            .child(button("+").on_click_menu(Menu::new().item(note)))
            .child(div().id("canvas").child(text("canvas")))
    }
}

#[test]
fn pop_up_menus_open_below_a_node_at_a_point_or_in_the_window() {
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Popups { picked: vec![] });
        mount(cx, root);
    });
    // The button's menu opens below it when it is pressed (#247).
    let p = h.click("+").to_vec();
    assert_eq!(count(&p, |p| matches!(p, Patch::ShowMenu { .. })), 1);
    let (node, spec, at) = h.host.popups[0].clone();
    let button = h.host.by_text("+");
    assert_eq!((node, at), (Some(button), None));
    let note = menu_ids(&spec);
    h.dispatch(UiEvent::MenuSelect(note[0]));
    assert_eq!(h.root::<Popups>().read(&h.rt.app).picked, vec!["note"]);
    // Over a named element, at a point in it, with a shortcut only shown.
    h.rt.app.show_menu_on(
        "canvas",
        Menu::new().item(
            MenuItem::new("Paste")
                .shortcut_hint("cmd+v")
                .on_select(std::rc::Rc::new(|_, _| {})),
        ),
        Some(Point::new(5., 6.)),
    );
    h.render();
    let (node, spec, at) = h.host.popups[1].clone();
    assert!(node.is_some() && node != Some(button));
    assert_eq!(at, Some(Point::new(5., 6.)));
    assert!(matches!(
        &spec.items[0],
        prism_core::menu::MenuItemSpec::Action { shortcut: Some(k), shortcut_hint: true, .. } if k == "cmd+v"
    ));
    // The previous pop-up's items are gone with it.
    h.dispatch(UiEvent::MenuSelect(note[0]));
    assert_eq!(h.root::<Popups>().read(&h.rt.app).picked.len(), 1);
    // In the window; nothing for an unknown element.
    h.rt.app.show_menu(Menu::new(), Point::new(1., 2.));
    h.rt.app.show_menu_on("missing", Menu::new(), None);
    h.render();
    assert_eq!(h.host.popups.len(), 3);
    assert_eq!(h.host.popups[2].0, None);
    assert_eq!(h.host.popups[2].2, Some(Point::new(1., 2.)));
}

thread_local! {
    static APP_MENU_PICKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[test]
fn app_menu_ids_fit_a_javascript_number_and_run_their_item() {
    let mut h = Harness::new(|cx| {
        cx.set_menu_bar(vec![
            prism_core::Menu::titled("App").item(
                prism_core::MenuItem::new("Search")
                    .shortcut("cmd+k")
                    .on_select(std::rc::Rc::new(|_, _| {
                        APP_MENU_PICKS.with(|n| n.set(n.get() + 1))
                    })),
            ),
        ]);
        let root = cx.new_entity(|_| WithMenu {
            menu: true,
            picked: vec![],
        });
        mount(cx, root);
    });
    let menus =
        h.rt.take_requests()
            .into_iter()
            .find_map(|r| match r {
                prism_core::PlatformRequest::SetMenuBar(m) => Some(m),
                _ => None,
            })
            .unwrap();
    let root = h.host.root.unwrap();
    let spec = h
        .last
        .iter()
        .find_map(|p| match p {
            Patch::SetMenu { menu: Some(m), .. } => Some(m.clone()),
            _ => None,
        })
        .unwrap();
    let context = menu_ids(&spec);
    let ids = menu_ids(&menus[0]);
    assert_eq!(ids.len(), 1);
    // The web page sends the id back as a JavaScript number (#170): exact
    // only up to 2^53.
    assert!(ids[0] < 1 << 53, "{} is rounded by JavaScript", ids[0]);
    h.dispatch(UiEvent::MenuSelect(ids[0]));
    assert_eq!(APP_MENU_PICKS.with(|n| n.get()), 1);
    // Context menus keep their own ids, below the app's.
    assert!(context.iter().all(|id| *id < ids[0] && id >> 16 == root));
    h.dispatch(UiEvent::MenuSelect(context[0]));
    assert_eq!(h.root::<WithMenu>().read(&h.rt.app).picked, vec!["copy"]);
    assert_eq!(APP_MENU_PICKS.with(|n| n.get()), 1);
}
