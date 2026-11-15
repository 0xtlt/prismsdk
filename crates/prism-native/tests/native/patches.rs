//! Every patch reaches the matching `Widgets` call: menus, gestures, tabs,
//! sheets, controls, appearance.

use prism_core::{
    Accessible, Appearance, Backend, Context, IntoElement, Menu, NodeKind, ParentElement, Patch,
    Render, Role, Router, Styled, SwipeDirection, UiEvent, button, div, menu::MenuItemSpec, screen,
    scroll, text, text_input, toggle,
};

use crate::common::{HEADER, Harness};

struct Controls {
    on: bool,
    menu: bool,
}

impl Render for Controls {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut card = div()
            .key("card")
            .on_click(cx.listener(|_, _, _| {}))
            .on_swipe(SwipeDirection::Left, cx.listener(|_, _, _| {}))
            .child(text("Card title"));
        if self.menu {
            card = card.context_menu(
                Menu::new()
                    .action("Rename", cx.listener(|_, _, _| {}))
                    .separator()
                    .action("Delete", cx.listener(|this, _, _| this.menu = false)),
            );
        }
        div()
            .child(card)
            .child(
                toggle(self.on)
                    .on_change(
                        cx.listener(|this, e: &prism_core::ChangeEvent, _| this.on = e.value != 0.),
                    )
                    .accessibility_label("Wi-Fi"),
            )
            .child(text_input().placeholder("Name"))
            .child(scroll().on_scroll(cx.listener(|_, _, _| {})))
    }
}

#[test]
fn controls_get_values_placeholders_clicks_and_accessibility() {
    let h = Harness::root(|| Controls {
        on: true,
        menu: false,
    });
    let w = h.w();
    let t = w.of_kind(NodeKind::Toggle)[0];
    assert_eq!(w.values[&t], 1.);
    assert_eq!(w.a11y[&t].checked, Some(true));
    assert_eq!(w.a11y[&t].label.as_deref(), Some("Wi-Fi"));
    let input = w.of_kind(NodeKind::TextInput)[0];
    assert_eq!(w.placeholders[&input], "Name");
    assert_eq!(w.a11y[&input].label.as_deref(), Some("Name"));
    let card = w.of_kind(NodeKind::View)[1];
    assert_eq!(w.clickable.get(&card), Some(&true));
    assert_eq!(w.a11y[&card].role, Some(Role::Button));
    assert_eq!(w.a11y[&card].label.as_deref(), Some("Card title"));
    assert_eq!(w.swipes[&card], (true, false));
    let s = w.of_kind(NodeKind::Scroll)[0];
    assert!(w.scroll_events[&s]);
}

#[test]
fn toggles_report_changes_and_get_the_new_value() {
    let mut h = Harness::root(|| Controls {
        on: true,
        menu: false,
    });
    let t = h.w().of_kind(NodeKind::Toggle)[0];
    let patches = h.rt.dispatch(UiEvent::Change(t, 0.));
    h.r.apply(&patches);
    assert_eq!(h.w().values[&t], 0.);
    assert_eq!(h.w().a11y[&t].checked, Some(false));
}

#[test]
fn context_menus_are_set_chosen_and_removed() {
    let mut h = Harness::root(|| Controls {
        on: false,
        menu: true,
    });
    let card = h.w().of_kind(NodeKind::View)[1];
    let menu = h.w().menus[&card].clone().expect("a menu");
    let titles: Vec<(&str, u64)> = menu
        .items
        .iter()
        .filter_map(|i| match i {
            MenuItemSpec::Action { title, id, .. } => Some((title.as_str(), *id)),
            _ => None,
        })
        .collect();
    assert_eq!(titles.len(), 2);
    assert_eq!(titles[0].0, "Rename");
    assert!(matches!(menu.items[1], MenuItemSpec::Separator));
    // "Delete" removes the menu: the view is told so.
    let patches = h.rt.dispatch(UiEvent::MenuSelect(titles[1].1));
    h.r.apply(&patches);
    assert_eq!(h.w().menus[&card], None);
}

struct Page(&'static str);
impl Render for Page {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen(self.0).child(text(format!("{} body", self.0)))
    }
}

fn tabbed() -> Harness {
    Harness::new(|cx| {
        let r = Router::new()
            .tab("Home", "house", "/")
            .tab("Settings", "gear", "/settings")
            .route("/", |cx, _| cx.new_entity(|_| Page("Home")))
            .route("/settings", |cx, _| cx.new_entity(|_| Page("Settings")))
            .route("/about", |cx, _| cx.new_entity(|_| Page("About")))
            .build(cx);
        cx.set_root(r);
    })
}

#[test]
fn tabs_get_titles_icons_selection_and_their_stacks() {
    let mut h = tabbed();
    let w = h.w();
    let bar = w.of_kind(NodeKind::Tabs)[0];
    let tabs = w.of_kind(NodeKind::Tab);
    assert_eq!(w.root, Some(bar));
    assert_eq!(w.values[&bar], 0.);
    assert_eq!(w.children[&bar], tabs);
    assert_eq!(w.texts[&tabs[0]], "Home");
    assert_eq!(w.icons[&tabs[1]], "gear");
    assert_eq!(w.a11y[&tabs[1]].label.as_deref(), Some("Settings"));
    let stack = w.of_kind(NodeKind::Stack)[0];
    assert_eq!(w.children[&tabs[0]], vec![stack]);

    let patches = h.rt.dispatch(UiEvent::Change(bar, 1.));
    h.r.apply(&patches);
    assert_eq!(h.w().values[&bar], 1.);
    let f = h.layout(400., 800.);
    let body = f.by_text("Settings body");
    // Tab (no bar in the fake) → stack → screen below the header.
    assert_eq!(f.frame(body), (0., 0., 400., 20.));
    let screen = f
        .of_kind(NodeKind::Screen)
        .into_iter()
        .find(|s| f.texts[s] == "Settings")
        .unwrap();
    assert_eq!(f.frame(screen), (0., HEADER, 400., 800. - HEADER));
}

#[test]
fn pushed_screens_join_the_stack_and_back_pops_them() {
    let mut h = tabbed();
    h.rt.app.navigate("/about");
    h.rerender();
    let stack = h.w().of_kind(NodeKind::Stack)[0];
    let screens = h.w().children[&stack].clone();
    assert_eq!(screens.len(), 2);
    assert_eq!(h.w().texts[&screens[1]], "About");
    // A native back gesture popped to the root.
    let patches = h.rt.dispatch(UiEvent::Back(stack, 1));
    h.r.apply(&patches);
    assert_eq!(h.w().children[&stack], vec![screens[0]]);
    assert!(!h.w().kinds.contains_key(&screens[1]), "destroyed");
}

#[test]
fn sheets_cover_their_container_and_close() {
    let mut h = tabbed();
    h.rt.app.present("/about");
    h.rerender();
    let modal = h.w().of_kind(NodeKind::Modal)[0];
    let bar = h.w().of_kind(NodeKind::Tabs)[0];
    assert!(h.w().children[&bar].contains(&modal));
    let f = h.layout(400., 800.);
    assert_eq!(f.frame(modal), (0., 0., 400., 800.));
    let about = f.by_text("About body");
    // The sheet's own stack shows its screen below a header.
    assert_eq!(f.frame(about).1, 0.);
    let sheet_screen = f
        .of_kind(NodeKind::Screen)
        .into_iter()
        .find(|s| f.texts[s] == "About")
        .unwrap();
    assert_eq!(f.frame(sheet_screen).1, HEADER);
    // Swipe down: the platform reports the sheet's stack at depth 0.
    let sheet_stack = *f
        .of_kind(NodeKind::Stack)
        .iter()
        .find(|s| f.children[&modal].contains(s))
        .expect("the sheet's stack");
    let patches = h.rt.dispatch(UiEvent::Back(sheet_stack, 0));
    h.r.apply(&patches);
    assert!(h.w().of_kind(NodeKind::Modal).is_empty());
}

#[test]
fn appearance_environment_and_unknown_ids() {
    let mut h = Harness::root(|| Page("Solo"));
    let lang = h
        .w()
        .env
        .as_ref()
        .map(|e| e.lang.clone())
        .unwrap_or_default();
    assert!(lang.starts_with("en"), "{lang}");
    h.rt.app.set_appearance(Appearance::Dark);
    h.rerender();
    assert_eq!(h.w().appearance, Some(Appearance::Dark));
    // Patches for views that do not exist (destroyed meanwhile) are ignored.
    h.r.apply(&[
        Patch::SetText {
            id: 999,
            text: "x".into(),
        },
        Patch::SetChildren {
            id: 999,
            children: vec![1],
        },
        Patch::Destroy { id: 999 },
        Patch::SetRoot { id: 999 },
    ]);
    h.layout(100., 100.);
}

#[test]
fn replacing_the_root_makes_the_new_one_fill_the_window() {
    let mut h = Harness::root(|| Page("Solo"));
    let old = h.w().root.unwrap();
    h.r.apply(&[
        Patch::Create {
            id: 500,
            kind: NodeKind::View,
        },
        Patch::SetStyle {
            id: 500,
            style: Default::default(),
        },
        Patch::SetRoot { id: 500 },
    ]);
    let f = h.layout(300., 200.);
    assert_eq!(f.root, Some(500));
    assert_eq!(f.frame(500), (0., 0., 300., 200.));
    let _ = old;
}

struct Labeled;
impl Render for Labeled {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(button("+1").accessibility_hint("Adds one"))
            .child(text("decor").accessibility_hidden())
    }
}

#[test]
fn accessibility_reaches_the_platform() {
    let h = Harness::root(|| Labeled);
    let w = h.w();
    let b = w.by_text("+1");
    assert_eq!(w.a11y[&b].role, Some(Role::Button));
    assert_eq!(w.a11y[&b].hint.as_deref(), Some("Adds one"));
    assert!(w.a11y[&w.by_text("decor")].hidden);
}

#[test]
fn style_only_updates_reach_the_view() {
    struct Bg(u32);
    impl Render for Bg {
        fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .bg(prism_core::rgb(self.0))
                .child(button("next").on_click(cx.listener(|this, _, _| this.0 += 1)))
        }
    }
    let mut h = Harness::root(|| Bg(0x100000));
    let root = h.w().root.unwrap();
    let next = h.w().by_text("next");
    let patches = h.rt.dispatch(UiEvent::Click(next));
    assert!(
        patches
            .iter()
            .all(|p| matches!(p, Patch::SetStyle { id, .. } if *id == root)),
        "{patches:?}"
    );
    h.r.apply(&patches);
    assert_eq!(
        h.w().styles[&root].background,
        Some(prism_core::rgb(0x100001))
    );
}
