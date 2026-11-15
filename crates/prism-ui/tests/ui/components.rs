//! Components: roles, labels, states, theming and overrides.

use prism_core::prelude::*;
use prism_core::{Color, NodeKind, UiEvent};
use prism_ui::{self as ui, Theme};

use crate::{a11y_tree, labelled, log, node, show};

#[test]
fn buttons_are_labelled_buttons_and_disabled_ones_do_nothing() {
    let mut h = show(Theme::light, |_, cx| {
        div()
            .child(
                ui::button("Save")
                    .hint("Saves the draft")
                    .on_click(cx.listener(|p, _, _| p.log.push("save".into()))),
            )
            .child(
                ui::button("Delete")
                    .danger()
                    .disabled(true)
                    .on_click(cx.listener(|p, _, _| p.log.push("delete".into()))),
            )
            .into_element()
    });
    let save = labelled(&h, "Save");
    let a = &node(&h, save).a11y;
    assert_eq!(a.role, Some(Role::Button));
    assert_eq!(a.hint.as_deref(), Some("Saves the draft"));
    assert!(!a.disabled && node(&h, save).clickable);
    // The visible text is not read twice.
    assert!(node(&h, h.host.by_text("Save")).a11y.hidden);

    let delete = labelled(&h, "Delete");
    assert!(node(&h, delete).a11y.disabled);
    assert!(!node(&h, delete).clickable);
    let style = node(&h, delete).style.clone().unwrap();
    assert_eq!(style.opacity, 0.45);
    assert_eq!(style.background, Some(Theme::light().danger));

    h.dispatch(UiEvent::Click(delete));
    h.dispatch(UiEvent::Click(save));
    assert_eq!(log(&h), ["save"]);
}

#[test]
fn button_variants_and_sizes_follow_the_theme() {
    let h = show(Theme::light, |_, _| {
        div()
            .child(ui::button("p").primary())
            .child(ui::button("s").secondary())
            .child(ui::button("g").ghost())
            .child(ui::button("small").small())
            .child(ui::button("large").large())
            .into_element()
    });
    let t = Theme::light();
    let bg = |l: &str| node(&h, labelled(&h, l)).style.clone().unwrap().background;
    assert_eq!(bg("p"), Some(t.accent));
    assert_eq!(bg("s"), Some(t.surface_alt));
    assert_eq!(bg("g"), None);
    let font = |l: &str| node(&h, h.host.by_text(l)).style.clone().unwrap().font_size;
    assert_eq!(font("small"), t.font_size - 2.);
    assert_eq!(font("large"), t.font_size + 2.);
    // Custom buttons are views, not native buttons.
    assert!(h.host.find(|n| n.kind == NodeKind::Button).is_none());
}

#[test]
fn custom_switches_toggle_through_the_whole_row() {
    let mut h = show(Theme::light, |p, cx| {
        ui::switch("Wi-Fi", p.on)
            .on_change(cx.listener(|p, e: &ChangeEvent, _| p.on = e.checked()))
            .into_element()
    });
    let row = labelled(&h, "Wi-Fi");
    assert_eq!(node(&h, row).a11y.role, Some(Role::Switch));
    assert_eq!(node(&h, row).a11y.checked, Some(false));
    h.dispatch(UiEvent::Click(row));
    assert_eq!(node(&h, row).a11y.checked, Some(true));
    // The track turned accent-colored, the thumb moved to the end.
    let track = node(&h, row).children[1];
    let style = node(&h, track).style.clone().unwrap();
    assert_eq!(style.background, Some(Theme::light().accent));
    assert_eq!(style.justify_content, prism_core::Justify::End);
    h.dispatch(UiEvent::Click(row));
    assert_eq!(node(&h, row).a11y.checked, Some(false));
}

#[test]
fn disabled_and_native_switches() {
    let mut h = show(Theme::light, |p, cx| {
        div()
            .child(
                ui::switch("Locked", false)
                    .disabled(true)
                    .on_change(cx.listener(|p, _, _| p.log.push("locked".into()))),
            )
            .child(
                ui::switch("Native", p.on)
                    .native()
                    .on_change(cx.listener(|p, e: &ChangeEvent, _| p.on = e.checked())),
            )
            .into_element()
    });
    let locked = labelled(&h, "Locked");
    assert!(node(&h, locked).a11y.disabled && !node(&h, locked).clickable);
    h.dispatch(UiEvent::Click(locked));
    assert!(log(&h).is_empty());
    let native = labelled(&h, "Native");
    assert_eq!(node(&h, native).kind, NodeKind::Toggle);
    h.dispatch(UiEvent::Change(native, 1.));
    assert_eq!(node(&h, native).value, Some(1.));
    assert_eq!(node(&h, native).a11y.checked, Some(true));
}

#[test]
fn checkboxes_report_and_flip_their_state() {
    let mut h = show(Theme::light, |p, cx| {
        ui::checkbox("Terms", p.on)
            .on_change(cx.listener(|p, e: &ChangeEvent, _| p.on = e.checked()))
            .into_element()
    });
    let c = labelled(&h, "Terms");
    assert_eq!(node(&h, c).a11y.role, Some(Role::Checkbox));
    assert_eq!(node(&h, c).a11y.checked, Some(false));
    assert!(h.host.find(|n| n.text == "✓").is_none());
    h.dispatch(UiEvent::Click(c));
    assert_eq!(node(&h, c).a11y.checked, Some(true));
    let tick = h.host.by_text("✓");
    assert_eq!(node(&h, tick).text, "✓");
    // Only the checkbox itself is exposed.
    assert_eq!(a11y_tree(&h), [(Some(Role::Checkbox), "Terms".into())]);
}

#[test]
fn text_fields_are_labelled_and_errors_win_over_helpers() {
    let mut h = show(Theme::light, |p, cx| {
        let error = p.on.then_some("Required");
        ui::text_field("Email", "")
            .placeholder("you@example.com")
            .helper("We never share it")
            .error(error)
            .on_input(cx.listener(|p, e: &InputEvent, _| {
                p.log.push(e.value.clone());
                p.on = e.value.is_empty();
            }))
            .into_element()
    });
    let input = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    let a = &node(&h, input).a11y;
    assert_eq!(a.label.as_deref(), Some("Email"));
    assert_eq!(a.hint.as_deref(), Some("We never share it"));
    assert_eq!(node(&h, input).placeholder, "you@example.com");
    h.dispatch(UiEvent::Input(input, "".into()));
    assert_eq!(node(&h, input).a11y.hint.as_deref(), Some("Required"));
    let style = node(&h, input).style.clone().unwrap();
    assert_eq!(style.border_color, Some(Theme::light().danger));
    h.host.by_text("Required");
    assert!(h.host.find(|n| n.text == "We never share it").is_none());
    h.dispatch(UiEvent::Input(input, "a@b.c".into()));
    assert_eq!(log(&h), ["", "a@b.c"]);
    assert_eq!(
        node(&h, input).a11y.hint.as_deref(),
        Some("We never share it")
    );
}

#[test]
fn headings_clamp_their_level() {
    let h = show(Theme::light, |_, _| {
        div()
            .child(ui::heading("Title").level(1))
            .child(ui::heading("Section"))
            .child(ui::heading("Deep").level(9))
            .child(ui::heading("Zero").level(0))
            .into_element()
    });
    let role = |t: &str| node(&h, h.host.by_text(t)).a11y.role;
    assert_eq!(role("Title"), Some(Role::Heading(1)));
    assert_eq!(role("Section"), Some(Role::Heading(2)));
    assert_eq!(role("Deep"), Some(Role::Heading(6)));
    assert_eq!(role("Zero"), Some(Role::Heading(1)));
    let size = |t: &str| node(&h, h.host.by_text(t)).style.clone().unwrap().font_size;
    assert!(size("Title") > size("Section") && size("Section") > size("Deep"));
}

#[test]
fn list_rows_read_as_one_element() {
    let mut h = show(Theme::light, |_, cx| {
        div()
            .child(
                ui::list_row("Milk")
                    .subtitle("2 bottles")
                    .detail("€3")
                    .leading("•")
                    .on_click(cx.listener(|p, _, _| p.log.push("milk".into()))),
            )
            .child(ui::list_row("Static"))
            .into_element()
    });
    let row = labelled(&h, "Milk, 2 bottles, €3");
    assert_eq!(node(&h, row).a11y.role, Some(Role::Button));
    h.host.by_text("›");
    assert_eq!(
        node(&h, labelled(&h, "Static")).a11y.role,
        Some(Role::Group)
    );
    h.dispatch(UiEvent::Click(row));
    assert_eq!(log(&h), ["milk"]);
    // Leading glyphs and chevrons are decorative.
    assert!(node(&h, h.host.by_text("•")).a11y.hidden);
    assert!(node(&h, h.host.by_text("›")).a11y.hidden);
}

#[test]
fn list_row_chevrons_point_the_other_way_right_to_left() {
    let mut h = show(Theme::light, |_, cx| {
        ui::list_row("Row")
            .on_click(cx.listener(|_, _, _| {}))
            .into_element()
    });
    h.rt.app.set_locale(Some(Locale::new("he")));
    h.render();
    h.host.by_text("‹");
}

#[test]
fn badges_cards_and_dividers() {
    let h = show(Theme::dark, |_, _| {
        ui::card()
            .child(ui::badge("New").tone(ui::Variant::Primary))
            .child(ui::divider())
            .child(ui::badge("Old"))
            .into_element()
    });
    let t = Theme::dark();
    let root = node(&h, h.host.root.unwrap()).style.clone().unwrap();
    assert_eq!(root.background, Some(t.surface));
    assert_eq!(root.corner_radius, t.radius + 4.);
    let pill = |text: &str| {
        let label = h.host.by_text(text);
        let parent = h
            .host
            .find(|n| n.children.contains(&label))
            .expect("badge pill");
        node(&h, parent).style.clone().unwrap().background
    };
    assert_eq!(pill("New"), Some(t.accent));
    assert_eq!(pill("Old"), Some(t.surface_alt));
    let divider = node(&h, h.host.root.unwrap()).children[1];
    assert!(node(&h, divider).a11y.hidden);
    assert_eq!(h.host.appearance, Some(prism_core::Appearance::Dark));
}

#[test]
fn refinements_apply_after_the_theme() {
    let h = show(
        || Theme::light().radius(2.),
        |_, _| {
            div()
                .child(ui::button("Themed"))
                .child(ui::button("Square").style(|s| s.corner_radius = 0.))
                .into_element()
        },
    );
    let radius = |l: &str| {
        node(&h, labelled(&h, l))
            .style
            .clone()
            .unwrap()
            .corner_radius
    };
    assert_eq!(radius("Themed"), 2.);
    assert_eq!(radius("Square"), 0.);
}

#[test]
fn overrides_keep_props_and_behaviour() {
    let mut h = show(
        || {
            Theme::light().override_component(|s: ui::Switch, _: &mut App| {
                // A text "switch", still wired to the app's handler.
                let label = format!("{} is {}", s.label, if s.on { "on" } else { "off" });
                let next = if s.on { 0. } else { 1. };
                let h = s.on_change.clone();
                prism_core::button(label)
                    .on_click(std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                        if let Some(h) = &h {
                            h(&ChangeEvent { value: next }, app)
                        }
                    }))
                    .into_element()
            })
        },
        |p, cx| {
            div()
                .child(
                    ui::switch("Sound", p.on)
                        .on_change(cx.listener(|p, e: &ChangeEvent, _| p.on = e.checked())),
                )
                // Other components are untouched.
                .child(ui::button("Plain"))
                .into_element()
        },
    );
    h.click("Sound is off");
    h.host.by_text("Sound is on");
    assert_eq!(node(&h, labelled(&h, "Plain")).kind, NodeKind::View);
}

#[test]
fn prefer_native_switches_every_control_unless_told_otherwise() {
    let h = show(
        || Theme::light().prefer_native(true),
        |_, _| {
            div()
                .child(ui::button("Native"))
                .child(ui::button("Custom").custom())
                .child(ui::switch("Toggle", true))
                .child(ui::switch("Custom switch", true).custom())
                .into_element()
        },
    );
    assert_eq!(node(&h, h.host.by_text("Native")).kind, NodeKind::Button);
    assert_eq!(node(&h, labelled(&h, "Custom")).kind, NodeKind::View);
    assert_eq!(node(&h, labelled(&h, "Toggle")).kind, NodeKind::Toggle);
    assert_eq!(node(&h, labelled(&h, "Custom switch")).kind, NodeKind::View);
}

#[test]
fn theme_updates_restyle_everything() {
    let mut h = show(Theme::light, |_, _| ui::button("Go").into_element());
    let go = labelled(&h, "Go");
    ui::update_theme(&mut h.rt.app, |t| *t = Theme::dark().accent(rgb(0x123456)));
    h.render();
    assert_eq!(
        node(&h, go).style.clone().unwrap().background,
        Some(rgb(0x123456))
    );
    assert_eq!(h.host.appearance, Some(prism_core::Appearance::Dark));
    // A system theme leaves the appearance to the platform.
    ui::set_theme(&mut h.rt.app, Theme::system());
    h.render();
    assert_eq!(h.host.appearance, Some(prism_core::Appearance::System));
    let accent: Color = Theme::system().accent;
    assert!(accent.dark.is_some(), "system themes use adaptive colors");
}

#[test]
fn dual_panes_stack_on_phones() {
    let h = show(Theme::light, |_, _| {
        ui::dual_pane(text("list"), text("detail")).into_element()
    });
    let root = node(&h, h.host.root.unwrap()).style.clone().unwrap();
    assert_eq!(root.direction, prism_core::FlexDirection::Column);
    assert_eq!(h.describe(), r#"view[view[text"list"] view[text"detail"]]"#);
}
