//! prism-web unit tests: SSR markup for every node kind, styles → CSS, the
//! patch wire format, the socket protocol (messages in → patches out), and
//! static files / mount points.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use http::{HeaderMap, Request, StatusCode, Uri};
use prism_core::prelude::*;
use prism_core::{
    Accessibility, Align, AppConfig, AppInit, Appearance, Backend, Direction, Edges, FlexDirection,
    FontWeight, Justify, Length, NodeKind, Patch, RenderEnvironment, Role, Runtime, Style,
    TextAlign, UiEvent,
};
use serde_json::{Value, json};

use crate::html::{HtmlBackend, a11y_attrs, escape, picture_url};
use crate::session::Encoder;
use crate::{WebApp, css};

// --- SSR markup, one node kind at a time ----------------------------------------------

/// The HTML of a tree described by `patches` (no styles: `style=""`).
fn html(patches: &[Patch]) -> String {
    let mut b = HtmlBackend::default();
    b.apply(patches);
    b.to_html()
}

fn create(id: u64, kind: NodeKind) -> Patch {
    Patch::Create { id, kind }
}

fn set_text(id: u64, text: &str) -> Patch {
    Patch::SetText {
        id,
        text: text.into(),
    }
}

fn children(id: u64, children: &[u64]) -> Patch {
    Patch::SetChildren {
        id,
        children: children.to_vec(),
    }
}

fn a11y(id: u64, a11y: Accessibility) -> Patch {
    Patch::SetAccessibility { id, a11y }
}

fn role(role: Role) -> Accessibility {
    Accessibility {
        role: Some(role),
        ..Default::default()
    }
}

#[test]
fn ssr_view_text_and_button() {
    assert_eq!(
        html(&[
            create(1, NodeKind::View),
            create(2, NodeKind::Text),
            set_text(2, "a < b & \"c\""),
            create(3, NodeKind::Button),
            set_text(3, "+1"),
            a11y(3, role(Role::Button)),
            children(1, &[2, 3]),
            Patch::SetRoot { id: 1 },
        ]),
        concat!(
            r#"<div data-pid="1" data-kind="view" style="">"#,
            r#"<div data-pid="2" data-kind="text" style="">a &lt; b &amp; &quot;c&quot;</div>"#,
            // A button's own role is implicit: no redundant role attribute.
            r#"<button data-pid="3" data-kind="button" style="">+1</button>"#,
            "</div>"
        )
    );
}

#[test]
fn ssr_text_input_and_toggle() {
    assert_eq!(
        html(&[
            create(1, NodeKind::TextInput),
            set_text(1, "milk"),
            Patch::SetPlaceholder {
                id: 1,
                text: "New todo".into(),
            },
            Patch::SetRoot { id: 1 },
        ]),
        r#"<input type="text" data-pid="1" data-kind="text_input" style="" value="milk" placeholder="New todo">"#
    );
    let toggle = |value: f64| {
        html(&[
            create(1, NodeKind::Toggle),
            Patch::SetValue { id: 1, value },
            a11y(
                1,
                Accessibility {
                    role: Some(Role::Switch),
                    label: Some("Dark mode".into()),
                    checked: Some(value != 0.),
                    ..Default::default()
                },
            ),
            Patch::SetRoot { id: 1 },
        ])
    };
    // The native checkbox carries the state: no aria-checked on top of it.
    assert_eq!(
        toggle(1.),
        r#"<input type="checkbox" role="switch" class="prism-switch" data-pid="1" data-kind="toggle" style="" aria-label="Dark mode" checked>"#
    );
    assert!(!toggle(0.).contains("checked"));
}

#[test]
fn ssr_image_progress_timer_and_shape() {
    assert_eq!(
        html(&[
            create(1, NodeKind::Image),
            Patch::SetIcon {
                id: 1,
                icon: "plus".into(),
            },
            Patch::SetRoot { id: 1 },
        ]),
        r#"<span data-pid="1" data-kind="image" style="" class="prism-symbol">+</span>"#
    );
    assert_eq!(
        html(&[
            create(1, NodeKind::Progress),
            Patch::SetValue { id: 1, value: 0.25 },
            Patch::SetRoot { id: 1 },
        ]),
        r#"<progress data-pid="1" data-kind="progress" style="" max="1" value="0.25"></progress>"#
    );
    assert_eq!(
        html(&[
            create(1, NodeKind::Shape),
            set_text(1, "i0,0\n|M 0 0 L 1 1\n"),
            Patch::SetRoot { id: 1 },
        ]),
        // Drawn by the client once it knows the box's size.
        r#"<div data-pid="1" data-kind="shape" style="" class="prism-shape"></div>"#
    );
    assert_eq!(
        html(&[create(1, NodeKind::Timer), Patch::SetRoot { id: 1 }]),
        r#"<div data-pid="1" data-kind="timer" style=""></div>"#
    );
}

#[test]
fn ssr_pictures_resolve_assets_and_keep_one_style_attribute() {
    let page = html(&[
        create(1, NodeKind::Picture),
        Patch::SetStyle {
            id: 1,
            style: Style {
                width: Length::Px(40.),
                ..Default::default()
            },
        },
        set_text(1, "logo.png"),
        Patch::SetValue { id: 1, value: 1. },
        a11y(
            1,
            Accessibility {
                role: Some(Role::Image),
                label: Some("Logo".into()),
                ..Default::default()
            },
        ),
        Patch::SetRoot { id: 1 },
    ]);
    assert!(page.starts_with("<img "), "{page}");
    assert!(page.contains(r#"src="/assets/logo.png""#), "{page}");
    assert!(page.contains(r#"alt="Logo""#), "{page}");
    // Browsers ignore a second `style` attribute: the fit must be in the first.
    assert_eq!(page.matches("style=").count(), 1, "{page}");
    assert!(page.contains("width:40px;") && page.contains("object-fit:contain"));

    let cover = html(&[
        create(1, NodeKind::Picture),
        set_text(1, "https://example.com/a.jpg"),
        Patch::SetRoot { id: 1 },
    ]);
    assert!(cover.contains(r#"src="https://example.com/a.jpg""#));
    assert!(cover.contains("object-fit:cover"), "{cover}");
    // The description is the alt text only (no ARIA on top), empty when hidden.
    assert!(
        !page.contains("role=") && !page.contains("aria-label"),
        "{page}"
    );
    let hidden = html(&[
        create(1, NodeKind::Picture),
        a11y(
            1,
            Accessibility {
                label: Some("Logo".into()),
                hidden: true,
                ..Default::default()
            },
        ),
        Patch::SetRoot { id: 1 },
    ]);
    assert!(hidden.contains(r#"alt="""#), "{hidden}");
}

#[test]
fn ssr_stack_shows_only_the_top_screen() {
    let tree = [
        Patch::SetEnvironment {
            env: RenderEnvironment {
                back: "Retour".into(),
                ..Default::default()
            },
        },
        create(1, NodeKind::Stack),
        create(2, NodeKind::Screen),
        set_text(2, "Home"),
        create(3, NodeKind::Screen),
        set_text(3, "Détail <1>"),
        create(4, NodeKind::Text),
        set_text(4, "body"),
        children(3, &[4]),
        children(1, &[2, 3]),
        Patch::SetRoot { id: 1 },
    ];
    assert_eq!(
        html(&tree),
        concat!(
            r#"<div data-pid="1" data-kind="stack" style="" class="prism-stack">"#,
            r#"<div class="prism-header"><button class="prism-back" aria-label="Retour">‹ Retour</button>"#,
            r#"<h1 class="prism-title" tabindex="-1">Détail &lt;1&gt;</h1></div><div class="prism-body">"#,
            r#"<section data-pid="2" data-kind="screen" style="" hidden aria-label="Home"></section>"#,
            r#"<section data-pid="3" data-kind="screen" style="" aria-label="Détail &lt;1&gt;">"#,
            r#"<div data-pid="4" data-kind="text" style="">body</div></section>"#,
            "</div></div>"
        )
    );
    // One screen: no Back button (English default when no environment).
    let single = html(&[
        create(1, NodeKind::Stack),
        create(2, NodeKind::Screen),
        children(1, &[2]),
        Patch::SetRoot { id: 1 },
    ]);
    assert!(single.contains(r#"aria-label="Back" hidden>‹ Back</button>"#));

    let mut b = HtmlBackend::default();
    b.apply(&tree);
    assert_eq!(b.screen_title().as_deref(), Some("Détail <1>"));
}

#[test]
fn ssr_tabs_build_a_tablist_with_the_selected_panel() {
    let page = html(&[
        create(1, NodeKind::Tabs),
        Patch::SetValue { id: 1, value: 1. },
        create(2, NodeKind::Tab),
        set_text(2, "Todos"),
        Patch::SetIcon {
            id: 2,
            icon: "checkmark".into(),
        },
        create(3, NodeKind::Tab),
        set_text(3, "Settings"),
        children(1, &[2, 3]),
        Patch::SetRoot { id: 1 },
    ]);
    assert_eq!(
        page,
        concat!(
            r#"<div data-pid="1" data-kind="tabs" style="" class="prism-tabs"><div class="prism-panels">"#,
            r#"<div data-pid="2" data-kind="tab" style="" hidden id="prism-panel-2" role="tabpanel" aria-labelledby="prism-tab-2"></div>"#,
            r#"<div data-pid="3" data-kind="tab" style="" id="prism-panel-3" role="tabpanel" aria-labelledby="prism-tab-3"></div>"#,
            r#"</div><div class="prism-tabbar" role="tablist">"#,
            r#"<button type="button" role="tab" class="prism-tab" id="prism-tab-2" aria-controls="prism-panel-2" aria-selected="false" tabindex="-1"><span class="prism-tab-icon" aria-hidden="true">✓</span><span>Todos</span></button>"#,
            r#"<button type="button" role="tab" class="prism-tab" id="prism-tab-3" aria-controls="prism-panel-3" aria-selected="true" tabindex="0"><span class="prism-tab-icon" aria-hidden="true"></span><span>Settings</span></button>"#,
            "</div></div>"
        )
    );
}

#[test]
fn ssr_scroll_and_destroyed_nodes() {
    let page = html(&[
        create(1, NodeKind::Scroll),
        create(2, NodeKind::Text),
        set_text(2, "gone"),
        create(3, NodeKind::Text),
        set_text(3, "kept"),
        children(1, &[2, 3]),
        Patch::SetRoot { id: 1 },
        children(1, &[3]),
        Patch::Destroy { id: 2 },
    ]);
    assert_eq!(
        page,
        concat!(
            r#"<div data-pid="1" data-kind="scroll" style="" class="prism-scroll">"#,
            r#"<div data-pid="3" data-kind="text" style="">kept</div></div>"#
        )
    );
    assert_eq!(html(&[create(1, NodeKind::View)]), "", "no root, no markup");
}

#[test]
fn aria_attributes_mirror_the_client() {
    let a = |r: Role| role(r);
    assert_eq!(
        a11y_attrs(NodeKind::Text, &a(Role::Heading(2))),
        r#" role="heading" aria-level="2""#
    );
    // Text is the default: no role attribute.
    assert_eq!(a11y_attrs(NodeKind::Text, &a(Role::Text)), "");
    // A clickable view is a focusable button.
    assert_eq!(
        a11y_attrs(
            NodeKind::View,
            &Accessibility {
                role: Some(Role::Button),
                label: Some("Open".into()),
                hint: Some("Shows details".into()),
                ..Default::default()
            }
        ),
        r#" role="button" aria-label="Open" aria-description="Shows details" tabindex="0""#
    );
    // A disabled one is neither focusable nor enabled.
    let disabled = Accessibility {
        role: Some(Role::Button),
        disabled: true,
        ..Default::default()
    };
    assert_eq!(
        a11y_attrs(NodeKind::View, &disabled),
        r#" role="button" aria-disabled="true""#
    );
    assert_eq!(
        a11y_attrs(NodeKind::Button, &disabled),
        r#" aria-disabled="true" disabled"#
    );
    // Toggle buttons, custom checkboxes and tabs carry their state.
    assert_eq!(
        a11y_attrs(
            NodeKind::Button,
            &Accessibility {
                role: Some(Role::Button),
                checked: Some(true),
                ..Default::default()
            }
        ),
        r#" aria-pressed="true""#
    );
    assert_eq!(
        a11y_attrs(NodeKind::View, &a(Role::Checkbox)),
        r#" role="checkbox" aria-checked="false" tabindex="0""#
    );
    assert_eq!(
        a11y_attrs(
            NodeKind::View,
            &Accessibility {
                role: Some(Role::Tab),
                checked: Some(true),
                ..Default::default()
            }
        ),
        r#" role="tab" aria-selected="true" tabindex="0""#
    );
    // Hidden: out of the tree and of the tab order.
    assert_eq!(
        a11y_attrs(
            NodeKind::View,
            &Accessibility {
                role: Some(Role::Link),
                hidden: true,
                ..Default::default()
            }
        ),
        r#" aria-hidden="true" role="link""#
    );
    // Containers drawn by the client's own markup get nothing but hidden.
    for kind in [
        NodeKind::Stack,
        NodeKind::Tabs,
        NodeKind::Tab,
        NodeKind::Screen,
    ] {
        assert_eq!(a11y_attrs(kind, &a(Role::Main)), "");
    }
    assert_eq!(
        a11y_attrs(NodeKind::View, &a(Role::Main)),
        r#" role="main""#
    );
    assert_eq!(
        escape("<a href='x'>&</a>"),
        "&lt;a href=&#39;x&#39;&gt;&amp;&lt;/a&gt;"
    );
}

#[test]
fn picture_urls() {
    assert_eq!(picture_url("logo.png"), "/assets/logo.png");
    assert_eq!(picture_url("img/a b.png"), "/assets/img/a b.png");
    for remote in [
        "https://x.dev/a.png",
        "http://x/a",
        "data:image/png;base64,AA",
        "/files/a.png",
    ] {
        assert_eq!(picture_url(remote), remote);
    }
}

// --- A real app, rendered on the server -----------------------------------------------

struct Counter {
    count: i32,
    name: String,
}

impl Render for Counter {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .child(text(format!("Count: {}", self.count)).accessibility_role(Role::Heading(1)))
            .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
            .child(
                text_input()
                    .value(self.name.clone())
                    .placeholder("Name")
                    .on_input(cx.listener(|this, e: &InputEvent, _| this.name = e.value.clone())),
            )
            .child(text(format!("Hello {}", self.name)))
    }
}

fn counter_init() -> AppInit {
    Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Counter {
            count: 0,
            name: String::new(),
        });
        cx.set_root(root);
    })
}

#[test]
fn ssr_of_a_rendered_app_matches_its_patches() {
    let mut rt = Runtime::new(&counter_init());
    let mut b = HtmlBackend::default();
    b.apply(&rt.render());
    let page = b.to_html();
    assert!(
        page.starts_with(r#"<div data-pid="1" data-kind="view" style="box-sizing:border-box;"#)
    );
    assert!(
        page.contains(r#"role="heading" aria-level="1">Count: 0</div>"#),
        "{page}"
    );
    assert!(page.contains(r#"data-kind="button""#) && page.contains(">+1</button>"));
    assert!(page.contains(r#"placeholder="Name""#) && page.contains(r#"aria-label="Name""#));
    assert_eq!(b.color_scheme(), "light dark");
    // A click updates the server tree like the client's.
    let plus = find(&rt, &b, "+1");
    b.apply(&rt.dispatch(UiEvent::Click(plus)));
    assert!(b.to_html().contains(">Count: 1</div>"));
    b.apply(&[Patch::SetAppearance {
        appearance: Appearance::Dark,
    }]);
    assert_eq!(b.color_scheme(), "dark");
}

/// The `data-pid` of the button titled `title` in `b`'s markup.
fn find(_: &Runtime, b: &HtmlBackend, title: &str) -> u64 {
    let page = b.to_html();
    let end = page
        .find(&format!(">{title}</button>"))
        .expect("button exists");
    let start = page[..end].rfind("data-pid=\"").expect("pid") + 10;
    page[start..].split('"').next().unwrap().parse().unwrap()
}

struct Home;
impl Render for Home {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Home").child(text("home body"))
    }
}

struct Sheet;
impl Render for Sheet {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Welcome").child(text("sheet body"))
    }
}

fn routed(tabs: bool) -> AppInit {
    Arc::new(move |cx: &mut App| {
        let mut r = Router::new();
        if tabs {
            r = r.tab("Home", "house", "/").tab("More", "gear", "/more");
        }
        let router = r
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .route("/more", |cx, _| cx.new_entity(|_| Home))
            .route("/welcome", |cx, _| cx.new_entity(|_| Sheet))
            .build(cx);
        cx.set_root(router);
        cx.present("/welcome");
    })
}

#[test]
fn ssr_of_a_presented_sheet_keeps_the_screen_underneath_visible() {
    for tabs in [false, true] {
        let mut rt = Runtime::new(&routed(tabs));
        let mut b = HtmlBackend::default();
        b.apply(&rt.render());
        let page = b.to_html();
        // The page's screen is still the visible one, with its title…
        assert_eq!(b.screen_title().as_deref(), Some("Home"), "tabs: {tabs}");
        let home = page.find(r#"aria-label="Home">"#).expect("home screen");
        assert!(!page[..home].ends_with(" hidden "), "{page}");
        assert!(
            !page[page[..home].rfind("<section").unwrap()..home].contains(" hidden"),
            "the home screen is visible (tabs: {tabs}): {page}"
        );
        // …the sheet is an open modal dialog on top, not a screen or a tab.
        assert!(page.contains("<dialog "), "{page}");
        assert!(page.contains(r#"class="prism-sheet" aria-modal="true" open"#));
        assert!(page.contains("sheet body"));
        assert_eq!(
            page.matches(r#"role="tab" "#).count(),
            if tabs { 2 } else { 0 }
        );
    }
}

// --- Styles → CSS ------------------------------------------------------------------------

#[test]
fn default_styles_are_a_flex_column() {
    assert_eq!(
        css(NodeKind::View, &Style::default()),
        "box-sizing:border-box;min-width:0;display:flex;flex-direction:column;align-items:stretch;justify-content:flex-start;margin-block:0px 0px;margin-inline:0px 0px;flex:0 1 auto;"
    );
    let text = css(NodeKind::Text, &Style::default());
    assert!(text.contains("display:block;white-space:pre-wrap;overflow-wrap:anywhere;"));
    // Text sizes are rem (they follow the browser's text size).
    assert!(text.ends_with("font-size:1rem;font-weight:400;text-align:start;"));
    assert!(css(NodeKind::Timer, &Style::default()).contains("font-variant-numeric:tabular-nums"));
    assert!(css(NodeKind::Image, &Style::default()).contains("display:inline-flex;"));
}

#[test]
fn layout_properties() {
    let s = Style {
        direction: FlexDirection::Row,
        wrap: true,
        align_items: Align::Center,
        align_self: Some(Align::End),
        justify_content: Justify::SpaceBetween,
        gap: 8.,
        padding: Edges {
            top: 1.,
            right: 2.,
            bottom: 3.,
            left: 4.,
        },
        margin: Edges::all(5.),
        width: Length::Px(100.),
        height: Length::Fraction(0.5),
        min_height: Length::Px(10.),
        max_width: Length::Fraction(1.),
        flex_grow: 1.,
        flex_shrink: 0.,
        flex_basis: Length::Px(0.),
        display_none: true,
        ..Default::default()
    };
    let c = css(NodeKind::View, &s);
    for part in [
        "display:none !important;",
        "display:flex;flex-direction:row;",
        "flex-wrap:wrap;",
        "align-items:center;",
        "align-self:flex-end;",
        "justify-content:space-between;",
        "gap:8px;",
        // Logical sides: leading/trailing mirror under dir=rtl.
        "padding-block:1px 3px;padding-inline:4px 2px;",
        "margin-block:5px 5px;margin-inline:5px 5px;",
        "width:100px;",
        "height:50%;",
        "min-height:10px;",
        "max-width:100%;",
        "flex:1 0 0px;",
    ] {
        assert!(c.contains(part), "{part} missing in {c}");
    }
    assert!(!c.contains("min-width:") || c.matches("min-width:").count() == 1);
    // Padding is for containers: a text's padding is not applied by CSS.
    assert!(!css(NodeKind::Text, &s).contains("padding-block"));
    for (j, v) in [
        (Justify::Start, "flex-start"),
        (Justify::Center, "center"),
        (Justify::End, "flex-end"),
        (Justify::SpaceAround, "space-around"),
        (Justify::SpaceEvenly, "space-evenly"),
    ] {
        let c = css(
            NodeKind::View,
            &Style {
                justify_content: j,
                ..Default::default()
            },
        );
        assert!(c.contains(&format!("justify-content:{v};")));
    }
}

#[test]
fn paint_text_and_adaptive_colors() {
    let s = Style {
        background: Some(rgb(0xFF0000)),
        corner_radius: 12.,
        border_width: 1.,
        opacity: 0.5,
        text_color: Some(adaptive(rgb(0x000000), rgb(0xFFFFFF))),
        font_size: 24.,
        font_weight: FontWeight::Bold,
        text_align: TextAlign::Right,
        ..Default::default()
    };
    let c = css(NodeKind::Text, &s);
    for part in [
        "background:rgba(255,0,0,1);",
        "border-radius:12px;",
        "border:1px solid currentColor;",
        "opacity:0.5;",
        "font-size:1.5rem;font-weight:700;",
        "color:light-dark(rgba(0,0,0,1), rgba(255,255,255,1));",
        "text-align:end;",
    ] {
        assert!(c.contains(part), "{part} missing in {c}");
    }
    // Views have no text properties; progress bars tint with the text color.
    assert!(!css(NodeKind::View, &s).contains("font-size"));
    assert!(css(NodeKind::Progress, &s).contains("accent-color:light-dark("));
    let bordered = css(
        NodeKind::View,
        &Style {
            border_width: 2.,
            border_color: Some(rgba(0x00FF00, 0.5)),
            ..Default::default()
        },
    );
    assert!(bordered.contains("border:2px solid rgba(0,255,0,0.5);"));
}

#[test]
fn buttons_and_inputs_get_their_defaults() {
    let plain = css(NodeKind::Button, &Style::default());
    assert!(plain.ends_with("font-family:inherit;cursor:pointer;justify-content:center;"));
    let filled = css(
        NodeKind::Button,
        &Style {
            background: Some(rgb(0x0000FF)),
            padding: Edges::all(2.),
            ..Default::default()
        },
    );
    assert!(
        filled.ends_with("border:none;padding:8px 16px;"),
        "{filled}"
    );
    let input = css(NodeKind::TextInput, &Style::default());
    assert!(input.contains("padding:6px 8px;border:1px solid"));
    let bordered = css(
        NodeKind::TextInput,
        &Style {
            border_width: 1.,
            ..Default::default()
        },
    );
    assert_eq!(bordered.matches("border:").count(), 1);
}

#[test]
fn transforms_mirror_right_to_left_and_transitions_animate() {
    let s = Style {
        transform: prism_core::Transform {
            x: 10.,
            y: -4.,
            scale: 1.5,
            rotate: 45.,
        },
        transition: Some(Transition::ease_out(250).delay(100)),
        ..Default::default()
    };
    let c = css(NodeKind::View, &s);
    assert!(c.contains(
        "transform:translate(calc(var(--prism-dir,1)*10px),-4px) rotate(calc(var(--prism-dir,1)*45deg)) scale(1.5);"
    ));
    assert!(c.contains("transition:all 0.25s cubic-bezier(.2,.8,.2,1) 0.1s;"));
    assert!(!css(NodeKind::View, &Style::default()).contains("transform"));
}

#[test]
fn motion_describes_enter_exit_and_loops() {
    assert_eq!(crate::css::motion(&Style::default()), Value::Null);
    let mut s = Style {
        opacity: 0.8,
        ..Default::default()
    };
    s.animations.enter = Some(Pose::fade());
    s.animations.exit = Some(Pose::new().x(20.));
    s.animations.repeat = Some((Pose::new().scale(1.1), Transition::linear(1000)));
    let m = crate::css::motion(&s);
    assert_eq!(
        m["rest"],
        json!({ "o": 0.800000011920929, "tf": [0., 0., 1., 0.] })
    );
    assert_eq!(m["enter"]["o"], json!(0.));
    assert_eq!(m["exit"]["tf"], json!([20., 0., 1., 0.]));
    assert_eq!(
        m["repeat"]["to"]["tf"],
        json!([0., 0., 1.100000023841858, 0.])
    );
    assert_eq!(
        m["repeat"]["t"],
        json!({ "d": 1., "delay": 0., "ease": "linear" })
    );
    // Without a transition: the default enter and exit timings.
    assert!(m["t"]["d"].as_f64().unwrap() > 0. && m["xt"]["d"].as_f64().unwrap() > 0.);
    // With one: it times both, without the delay on exit.
    s.transition = Some(Transition::ease(300).delay(50));
    let m = crate::css::motion(&s);
    assert_eq!(m["t"]["delay"], json!(0.05000000074505806));
    assert_eq!(m["xt"]["delay"], json!(0.));
    assert_eq!(m["xt"]["d"], m["t"]["d"]);
}

// --- Patch wire format -------------------------------------------------------------------

#[test]
fn every_patch_has_a_stable_json_encoding() {
    let mut e = Encoder::default();
    let menu = prism_core::menu::MenuSpec {
        title: String::new(),
        items: vec![prism_core::menu::MenuItemSpec::Action {
            id: 7 << 16,
            title: "Delete".into(),
            icon: None,
            shortcut: None,
            role: prism_core::ActionRole::Destructive,
            checked: None,
            enabled: true,
        }],
    };
    let patches = vec![
        Patch::SetEnvironment {
            env: RenderEnvironment {
                lang: "ar".into(),
                direction: Direction::Rtl,
                back: "رجوع".into(),
                close: "إغلاق".into(),
                ..Default::default()
            },
        },
        create(7, NodeKind::Picture),
        set_text(7, "logo.png"),
        create(8, NodeKind::Text),
        set_text(8, "logo.png"),
        Patch::SetPlaceholder {
            id: 8,
            text: "p".into(),
        },
        Patch::SetClickable {
            id: 8,
            clickable: true,
        },
        children(7, &[8]),
        Patch::SetValue { id: 7, value: 1. },
        Patch::SetIcon {
            id: 8,
            icon: "plus".into(),
        },
        Patch::SetSwipe {
            id: 8,
            left: true,
            right: false,
        },
        Patch::SetScrollEvents { id: 8, on: true },
        Patch::SetMenu {
            id: 7,
            menu: Some(menu),
        },
        Patch::SetMenu { id: 8, menu: None },
        a11y(
            8,
            Accessibility {
                role: Some(Role::Heading(3)),
                label: Some("L".into()),
                checked: Some(true),
                ..Default::default()
            },
        ),
        Patch::SetAppearance {
            appearance: Appearance::Light,
        },
        Patch::SetRoot { id: 7 },
        Patch::Destroy { id: 8 },
    ];
    let encoded = e.encode(&patches);
    let expected = json!([
        { "op": "env", "lang": "ar", "dir": "rtl", "back": "رجوع", "close": "إغلاق" },
        { "op": "create", "id": 7, "kind": "picture" },
        // Picture sources become asset URLs; other texts stay as they are.
        { "op": "text", "id": 7, "text": "/assets/logo.png" },
        { "op": "create", "id": 8, "kind": "text" },
        { "op": "text", "id": 8, "text": "logo.png" },
        { "op": "placeholder", "id": 8, "text": "p" },
        { "op": "clickable", "id": 8, "on": true },
        { "op": "children", "id": 7, "children": [8] },
        { "op": "value", "id": 7, "value": 1.0 },
        { "op": "icon", "id": 8, "icon": "plus", "glyph": "+" },
        { "op": "swipe", "id": 8, "left": true, "right": false },
        { "op": "scrollev", "id": 8, "on": true },
        { "op": "menu", "id": 7, "menu": { "title": "", "items": [
            { "t": "act", "id": (7u64 << 16), "title": "Delete", "icon": null, "shortcut": null,
              "role": "destructive", "checked": null, "enabled": true }
        ]}},
        { "op": "menu", "id": 8, "menu": null },
        { "op": "a11y", "id": 8, "a11y": {
            "role": "heading", "level": 3, "label": "L", "hint": null, "value": null,
            "checked": true, "disabled": false, "hidden": false,
        }},
        { "op": "appearance", "value": "light" },
        { "op": "root", "id": 7 },
        { "op": "destroy", "id": 8 },
    ]);
    assert_eq!(Value::Array(encoded), expected);
}

#[test]
fn styles_are_encoded_as_css_for_the_node_kind() {
    let mut e = Encoder::default();
    let style = Style {
        font_size: 32.,
        ..Default::default()
    };
    let out = e.encode(&[
        create(1, NodeKind::Text),
        Patch::SetStyle {
            id: 1,
            style: style.clone(),
        },
    ]);
    assert_eq!(out[1]["css"], json!(css(NodeKind::Text, &style)));
    assert!(out[1].get("motion").is_none());
    // After a destroy the id's kind is forgotten (ids are never reused).
    let out = e.encode(&[
        Patch::Destroy { id: 1 },
        Patch::SetStyle {
            id: 1,
            style: style.clone(),
        },
    ]);
    assert_eq!(out[1]["css"], json!(css(NodeKind::View, &style)));
    let mut animated = style;
    animated.animations.enter = Some(Pose::fade());
    let out = e.encode(&[Patch::SetStyle {
        id: 2,
        style: animated,
    }]);
    assert_eq!(out[0]["motion"]["enter"]["o"], json!(0.));
}

#[test]
fn custom_asset_urls_resolve_pictures() {
    let mut e = Encoder::default();
    e.asset_urls = Some(Arc::new(|s: &str| format!("https://cdn/{s}")));
    let out = e.encode(&[create(1, NodeKind::Picture), set_text(1, "a.png")]);
    assert_eq!(out[1]["text"], json!("https://cdn/a.png"));
}

// --- The socket protocol: messages in, patches out ----------------------------------------

#[test]
fn page_events_become_ui_events() {
    use crate::session::parse_event;
    use prism_core::SwipeDirection;
    let ev = |v: Value| parse_event(&v);
    assert_eq!(ev(json!({"t": "click", "id": 3})), Some(UiEvent::Click(3)));
    assert_eq!(
        ev(json!({"t": "input", "id": 3, "v": "hi"})),
        Some(UiEvent::Input(3, "hi".into()))
    );
    assert_eq!(
        ev(json!({"t": "back", "id": 3, "depth": 1})),
        Some(UiEvent::Back(3, 1))
    );
    assert_eq!(
        ev(json!({"t": "change", "id": 3, "v": 1})),
        Some(UiEvent::Change(3, 1.))
    );
    assert_eq!(
        ev(json!({"t": "menu", "id": 65536})),
        Some(UiEvent::MenuSelect(65536))
    );
    assert_eq!(
        ev(json!({"t": "scroll", "id": 3, "o": 120.5, "v": 600})),
        Some(UiEvent::Scroll(3, 120.5, 600.))
    );
    assert_eq!(
        ev(json!({"t": "swipe", "id": 3, "dir": "left"})),
        Some(UiEvent::Swipe(3, SwipeDirection::Left))
    );
    assert_eq!(
        ev(json!({"t": "swipe", "id": 3, "dir": "right"})),
        Some(UiEvent::Swipe(3, SwipeDirection::Right))
    );
    assert_eq!(
        ev(json!({"t": "alert", "id": 9, "index": 2})),
        Some(UiEvent::AlertAction(9, 2))
    );
    assert_eq!(
        ev(json!({"t": "navigate", "id": 3, "path": "/a"})),
        Some(UiEvent::Navigate(3, "/a".into()))
    );
    // Malformed messages are ignored, not panics.
    for bad in [
        json!({"t": "click"}),
        json!({"t": "click", "id": "3"}),
        json!({"t": "click", "id": -1}),
        json!({"t": "input", "id": 3}),
        json!({"t": "back", "id": 3, "depth": "x"}),
        json!({"t": "nope", "id": 3}),
        json!({"id": 3}),
        json!([1, 2]),
    ] {
        assert_eq!(ev(bad.clone()), None, "{bad}");
    }
}

#[test]
fn uploads_decode_standard_and_url_safe_base64() {
    use crate::session::base64_decode;
    assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(base64_decode("aGVs\nbG8").unwrap(), b"hello");
    assert_eq!(
        base64_decode("+/8=").unwrap(),
        base64_decode("-_8=").unwrap()
    );
    assert_eq!(base64_decode("").unwrap(), b"");
    assert_eq!(base64_decode("a b"), None);
}

/// A live session over channels: what the page receives, as JSON.
struct Page {
    session: crate::Session,
    inbox: mpsc::Receiver<String>,
}

impl Page {
    fn open(app: &WebApp, uri: &str) -> Page {
        let (tx, inbox) = mpsc::channel();
        let session = app.connect(
            &uri.parse::<Uri>().unwrap(),
            &HeaderMap::new(),
            move |msg| tx.send(msg).is_ok(),
        );
        Page { session, inbox }
    }

    fn next(&self) -> Value {
        let msg = self
            .inbox
            .recv_timeout(Duration::from_secs(10))
            .expect("the session answers");
        serde_json::from_str(&msg).unwrap()
    }

    fn send(&self, msg: Value) {
        assert!(self.session.receive(msg.to_string()));
    }
}

fn web_app(init: AppInit, tag: &str) -> (WebApp, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("prism-web-{tag}-{}", std::process::id()));
    let config = AppConfig {
        title: "Test".into(),
        assets: Some(dir.join("assets").to_string_lossy().into()),
        ..Default::default()
    };
    (WebApp::new(config, init).data_dir(dir.join("data")), dir)
}

fn op_with_text(patches: &Value, text: &str) -> u64 {
    patches
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["op"] == "text" && p["text"] == text)
        .and_then(|p| p["id"].as_u64())
        .unwrap_or_else(|| panic!("no text {text:?} in {patches}"))
}

#[test]
fn a_session_resets_the_page_then_answers_events_with_patches() {
    let (app, dir) = web_app(counter_init(), "session");
    let page = Page::open(&app, "/ws?path=%2F&w=1280&h=800");
    let reset = page.next();
    assert_eq!(reset["t"], "reset");
    let first = &reset["p"];
    assert_eq!(first[0]["op"], "env", "the environment comes first");
    assert!(first.as_array().unwrap().iter().any(|p| p["op"] == "root"));
    let count = op_with_text(first, "Count: 0");
    let plus = op_with_text(first, "+1");
    let input = first
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["op"] == "create" && p["kind"] == "text_input")
        .and_then(|p| p["id"].as_u64())
        .unwrap();

    page.send(json!({ "t": "click", "id": plus }));
    assert_eq!(
        page.next(),
        json!({ "t": "patch", "p": [{ "op": "text", "id": count, "text": "Count: 1" }] })
    );
    page.send(json!({ "t": "input", "id": input, "v": "Ada" }));
    let typed = page.next();
    assert_eq!(typed["t"], "patch");
    // The field echoes its value and the greeting follows.
    let texts: Vec<&str> = typed["p"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["text"].as_str())
        .collect();
    assert!(texts.contains(&"Hello Ada"), "{typed}");

    // Garbage and no-op events send nothing; the next real event still works.
    page.send(json!("not an object"));
    assert!(page.session.receive("{oops"));
    page.send(json!({ "t": "click", "id": 999_999 }));
    page.send(json!({ "t": "click", "id": plus }));
    assert_eq!(
        page.next()["p"],
        json!([{ "op": "text", "id": count, "text": "Count: 2" }])
    );
    page.session.close();
    let _ = std::fs::remove_dir_all(dir);
}

struct Wide;
impl Render for Wide {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (w, _) = cx.viewport();
        text(if w >= 1024. { "wide" } else { "narrow" })
    }
}

#[test]
fn a_session_follows_the_viewport_and_lifecycle() {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let root = cx.new_entity(|_| Wide);
        cx.set_root(root);
    });
    let (app, dir) = web_app(init, "viewport");
    let page = Page::open(&app, "/ws?w=400&h=800");
    op_with_text(&page.next()["p"], "narrow");
    page.send(json!({ "t": "viewport", "w": 1280, "h": 800 }));
    op_with_text(&page.next()["p"], "wide");
    // Lifecycle changes and incomplete viewports produce no patches here.
    page.send(json!({ "t": "lifecycle", "state": "background" }));
    page.send(json!({ "t": "viewport", "w": 500 }));
    page.send(json!({ "t": "viewport", "w": 500, "h": 800 }));
    op_with_text(&page.next()["p"], "narrow");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn deep_links_from_the_page_navigate() {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let router = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .route("/welcome", |cx, _| cx.new_entity(|_| Sheet))
            .build(cx);
        cx.set_root(router);
    });
    let (app, dir) = web_app(init, "link");
    let page = Page::open(&app, "/ws?path=%2F");
    op_with_text(&page.next()["p"], "Home");
    page.send(json!({ "t": "link", "path": "/welcome" }));
    let pushed = page.next();
    op_with_text(&pushed["p"], "Welcome");
    op_with_text(&pushed["p"], "/welcome");
    let _ = std::fs::remove_dir_all(dir);
}

// --- Device calls and services ----------------------------------------------------------

#[test]
fn device_calls_are_encoded_for_the_client() {
    use crate::session::device_json;
    use prism_core::{Accuracy, DeviceCall, Haptic, Permission};
    assert_eq!(
        device_json(4, DeviceCall::CurrentLocation(Accuracy::Best)),
        json!({ "t": "device", "id": 4, "call": "location", "accuracy": "best" })
    );
    assert_eq!(
        device_json(5, DeviceCall::RequestPermission(Permission::Camera)),
        json!({ "t": "device", "id": 5, "call": "request_permission", "permission": "camera" })
    );
    assert_eq!(
        device_json(6, DeviceCall::WatchMotion(Duration::from_millis(250))),
        json!({ "t": "device", "id": 6, "call": "watch_motion", "interval": 250 })
    );
    assert_eq!(
        device_json(7, DeviceCall::Haptic(Haptic::Success))["kind"],
        "success"
    );
    assert_eq!(device_json(8, DeviceCall::StopWatch)["call"], "stop");
    assert_eq!(
        device_json(9, DeviceCall::CopyText("x".into())),
        json!({ "t": "device", "id": 9, "call": "copy", "text": "x" })
    );
}

#[test]
fn device_replies_become_values() {
    use crate::session::device_result;
    use prism_core::{DeviceError, DeviceValue, PermissionState};
    let (app, dir) = web_app(counter_init(), "reply");
    let r = |v: Value| device_result(&app, "0123456789abcdef0123456789abcdef", &v);
    assert!(matches!(
        r(json!({ "permission": "granted" })),
        Ok(DeviceValue::Permission(PermissionState::Granted))
    ));
    match r(json!({ "location": { "latitude": 48.8, "longitude": 2.3 } })) {
        Ok(DeviceValue::Location(l)) => {
            assert_eq!((l.latitude, l.longitude, l.altitude), (48.8, 2.3, None))
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        r(json!({ "text": "clip" })),
        Ok(DeviceValue::Text(Some(t))) if t == "clip"
    ));
    assert!(matches!(
        r(json!({ "text": null })),
        Ok(DeviceValue::Text(None))
    ));
    assert!(matches!(
        r(json!({ "err": "denied" })),
        Err(DeviceError::Denied)
    ));
    assert!(matches!(r(json!({})), Ok(DeviceValue::Done)));
    // Uploaded photos are stored for this browser and served under /files.
    match r(json!({ "files": [
        { "name": "cat.JPG", "mime": "image/jpeg", "data": "aGVsbG8=" },
        { "name": "bad", "data": "!!" },
    ] })) {
        Ok(DeviceValue::Files(files)) => {
            assert_eq!(files.len(), 1);
            let f = &files[0];
            assert_eq!(
                (f.name.as_str(), f.mime.as_str(), f.size),
                ("cat.JPG", "image/jpeg", 5)
            );
            assert!(
                f.path.starts_with("/files/") && f.path.ends_with(".jpg"),
                "{}",
                f.path
            );
        }
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn notifications_and_alerts_go_to_the_page() {
    use crate::session::WebServices;
    use prism_core::{PlatformRequest, PlatformServices};
    let mut s = WebServices::default();
    s.handle(PlatformRequest::Notify(
        Notification::new("Hi")
            .id("n1")
            .body("there")
            .link("/todos/1")
            .action(NotificationAction::new("done", "Done")),
    ));
    s.handle(PlatformRequest::CancelNotification("n1".into()));
    s.handle(PlatformRequest::RequestNotificationPermission);
    // Surfaces the web does not have are dropped.
    s.handle(PlatformRequest::RemoveTray("t".into()));
    let out = s.take_outbox();
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["t"], "notify");
    assert_eq!(
        (&out[0]["title"], &out[0]["body"], &out[0]["link"]),
        (&json!("Hi"), &json!("there"), &json!("/todos/1"))
    );
    assert_eq!(out[0]["actions"][0]["id"], "done");
    assert_eq!(out[1], json!({ "t": "notify_cancel", "id": "n1" }));
    assert_eq!(out[2], json!({ "t": "notify_permission" }));
    assert!(!s.capabilities().widgets && s.capabilities().notifications);
}

// --- Static files and mount points ------------------------------------------------------

#[test]
fn content_types_and_hashed_names() {
    use crate::assets::content_type;
    assert_eq!(content_type("a.PNG"), "image/png");
    assert_eq!(content_type("x/y.svg"), "image/svg+xml");
    assert_eq!(content_type("app.mjs"), "text/javascript; charset=utf-8");
    assert_eq!(content_type("README"), "application/octet-stream");
    // Same bytes, same hash; any change, another one.
    assert_eq!(crate::content_hash(b"abc"), crate::content_hash(b"abc"));
    assert_ne!(crate::content_hash(b"abc"), crate::content_hash(b"abd"));
    assert_eq!(crate::content_hash(b""), "e3b0c44298fc");
}

#[test]
fn base_paths_are_normalized() {
    let (app, dir) = web_app(counter_init(), "bases");
    for (given, base) in [
        ("", ""),
        ("/", ""),
        ("app", "/app"),
        ("/app/", "/app"),
        ("/a/b/", "/a/b"),
    ] {
        let a = web_app(counter_init(), "bases").0.base_path(given);
        assert_eq!(a.base(), base, "{given:?}");
    }
    let nested = app.base_path("/app");
    // Only the mount point itself and paths under it are stripped.
    assert!(nested.is_socket("/app/ws"));
    assert!(!nested.is_socket("/apple/ws"));
    assert!(!nested.is_socket("/app/ws/x"));
    assert_eq!(nested.picture_url("logo.png"), "/app/assets/logo.png");
    assert_eq!(nested.picture_url("/abs.png"), "/abs.png");
    let cdn = nested.asset_url("https://cdn.example.com///");
    assert_eq!(
        cdn.picture_url("logo.png"),
        "https://cdn.example.com/assets/logo.png"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pages_and_files_by_path() {
    let (app, dir) = web_app(counter_init(), "paths");
    let get = |path: &str| app.handle(&Request::get(path).body(()).unwrap());
    let page = get("/anything%20here");
    assert_eq!(page.status(), StatusCode::OK);
    let body = String::from_utf8(page.body().clone()).unwrap();
    assert!(body.starts_with("<!doctype html>"));
    assert!(body.contains(r#"<main id="prism-root"><div data-pid="1""#));
    // No `style` attribute anywhere: a strict CSP allows the page.
    assert!(body.contains(r#"<html lang="en-US" dir="ltr">"#), "{body}");
    assert!(body.contains(":root{color-scheme:light dark}"), "{body}");
    assert!(!body.contains("style="), "{body}");
    assert!(body.contains("<title>Test</title>"));
    assert_eq!(get("/favicon.ico").status(), StatusCode::NOT_FOUND);
    assert_eq!(get("/ws").status(), StatusCode::NOT_FOUND);
    assert_eq!(
        get("/_prism/client.0000.js").status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(get("/assets/missing.png").status(), StatusCode::NOT_FOUND);
    // A known session cookie is kept; a malformed one is replaced.
    let sid = "0123456789abcdef0123456789abcdef";
    let with =
        |cookie: &str| app.handle(&Request::get("/").header("cookie", cookie).body(()).unwrap());
    assert!(
        with(&format!("a=b; prism_sid={sid}"))
            .headers()
            .get("set-cookie")
            .is_none()
    );
    assert!(
        with("prism_sid=../../etc")
            .headers()
            .get("set-cookie")
            .is_some()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ssr_pages_are_titled_by_the_visible_screen() {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let router = Router::new()
            .route("/", |cx, _| cx.new_entity(|_| Home))
            .route("/welcome", |cx, _| cx.new_entity(|_| Sheet))
            .build(cx);
        cx.set_root(router);
    });
    let (app, dir) = web_app(init, "title");
    let page = |path: &str, lang: &str| {
        let r = app.handle(
            &Request::get(path)
                .header("accept-language", lang)
                .body(())
                .unwrap(),
        );
        String::from_utf8(r.body().clone()).unwrap()
    };
    let w = page("/welcome", "en");
    assert!(w.contains("<title>Welcome · Test</title>"), "{w}");
    // A deep link renders the screens underneath (hidden) and the target.
    let deep = page("/welcome", "en");
    assert!(deep.contains(r#"hidden aria-label="Home""#), "{deep}");
    // Right-to-left languages set the page direction before any script.
    assert!(page("/", "ar-EG,ar;q=0.9").contains(r#"lang="ar-EG" dir="rtl""#));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn cookies_and_urls_are_parsed_strictly() {
    use crate::app::{percent_decode, session_id, time_zone, viewport_cookie};
    assert_eq!(percent_decode("/a%20b%2Fc"), "/a b/c");
    assert_eq!(percent_decode("%E2%9C%93"), "✓");
    assert_eq!(percent_decode("100%"), "100%");
    assert_eq!(percent_decode("%zz%4"), "%zz%4");
    assert_eq!(
        session_id("x=1; prism_sid=ABCDEF0123456789abcdef0123456789"),
        Some("ABCDEF0123456789abcdef0123456789".into())
    );
    assert_eq!(session_id("prism_sid=short"), None);
    assert_eq!(
        session_id("prism_sid=0123456789abcdef0123456789abcdeg"),
        None
    );
    assert_eq!(viewport_cookie("prism_vw=1280x800"), Some((1280., 800.)));
    assert_eq!(viewport_cookie("prism_vw=1280"), None);
    assert_eq!(time_zone("prism_tz=-18000"), Some(-18000));
    assert_eq!(time_zone("prism_tz=999999"), None, "out of range");
    let a = crate::app::new_session_id();
    let b = crate::app::new_session_id();
    assert!(a.len() == 32 && a != b && session_id(&format!("prism_sid={a}")).is_some());
}

// --- Contract with client.js ---------------------------------------------------------------

/// The app behind `tests/js/fixtures/app.json`: every interactive node kind,
/// in tabs, so the JS tests can check the client builds what the server
/// rendered and sends the events the server parses.
struct Showcase {
    count: i32,
    dark: bool,
    name: String,
}

impl Render for Showcase {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        screen("Home").child(
            div()
                .gap(8.)
                .child(text(format!("Count: {}", self.count)).accessibility_role(Role::Heading(1)))
                .child(button("+1").on_click(cx.listener(|this, _, _| this.count += 1)))
                .child(
                    div()
                        .child(text("Card"))
                        .child(text("details"))
                        .on_click(cx.listener(|this, _, _| this.count = 0)),
                )
                .child(
                    text_input()
                        .value(self.name.clone())
                        .placeholder("Your name")
                        .on_input(
                            cx.listener(|this, e: &InputEvent, _| this.name = e.value.clone()),
                        ),
                )
                .child(
                    toggle(self.dark)
                        .accessibility_label("Dark mode")
                        .on_change(cx.listener(|this, e: &ChangeEvent, _| this.dark = e.checked())),
                )
                .child(progress(0.5))
                .child(image("plus"))
                .child(picture("logo.png").alt("Logo").fit())
                .child(
                    shape().w(px(100.)).h(px(50.)).path(
                        Path::new()
                            .move_to(0., 1.)
                            .line_to(1., 0.)
                            .stroke(rgb(0xFF0000), 2.),
                    ),
                ),
        )
    }
}

fn showcase_init() -> AppInit {
    Arc::new(|cx: &mut App| {
        let router = Router::new()
            .tab("Home", "house", "/")
            .tab("More", "gear", "/more")
            .route("/", |cx, _| {
                cx.new_entity(|_| Showcase {
                    count: 0,
                    dark: false,
                    name: String::new(),
                })
            })
            .route("/more", |cx, _| cx.new_entity(|_| Home))
            .build(cx);
        cx.set_root(router);
    })
}

#[test]
fn client_fixture_is_up_to_date() {
    let mut rt = Runtime::new(&showcase_init());
    let first = rt.render();
    let mut html = HtmlBackend::default();
    html.apply(&first);
    let mut enc = Encoder::default();
    let reset = Value::Array(enc.encode(&first));
    let ops = reset.as_array().unwrap().clone();
    let kind_id = |kind: &str| {
        ops.iter()
            .find(|p| p["op"] == "create" && p["kind"] == kind)
            .and_then(|p| p["id"].as_u64())
            .unwrap()
    };
    let plus = op_with_text(&reset, "+1");
    let card = op_with_text(&reset, "Card");
    let card_view = ops
        .iter()
        .find(|p| p["op"] == "children" && p["children"].as_array().unwrap().contains(&json!(card)))
        .and_then(|p| p["id"].as_u64())
        .unwrap();
    // What the page sends for each interaction, and what the server answers.
    let mut events = Vec::new();
    for msg in [
        json!({ "t": "click", "id": plus }),
        json!({ "t": "click", "id": plus }),
        json!({ "t": "click", "id": card_view }),
        json!({ "t": "input", "id": kind_id("text_input"), "v": "Ada" }),
        json!({ "t": "change", "id": kind_id("toggle"), "v": 1 }),
        json!({ "t": "change", "id": kind_id("tabs"), "v": 1 }),
    ] {
        let event = crate::session::parse_event(&msg).expect("a valid event");
        let patches = Value::Array(enc.encode(&rt.dispatch(event)));
        assert_ne!(patches, json!([]), "{msg} changes the page");
        events.push(json!({ "send": msg, "patches": patches }));
    }
    let fixture = json!({
        "ssr": html.to_html(),
        "reset": reset,
        "events": events,
    });
    let text = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/js/fixtures/app.json");
    if std::env::var_os("PRISM_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &text).unwrap();
    }
    let saved = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(
        saved == text,
        "{} is out of date: run `PRISM_UPDATE_FIXTURES=1 cargo test -p prism-web client_fixture` and commit it",
        file.display()
    );
}

#[test]
fn some_corners_rounded_use_logical_properties() {
    let s = Style {
        corner_radius: 4.,
        rounded_corners: prism_core::Corners::TOP,
        ..Default::default()
    };
    let c = css(NodeKind::View, &s);
    assert!(
        c.contains("border-radius:4px;border-end-end-radius:0;border-end-start-radius:0;"),
        "{c}"
    );
    let all = css(
        NodeKind::View,
        &Style {
            corner_radius: 4.,
            ..Default::default()
        },
    );
    assert!(!all.contains("-radius:0"), "{all}");
}

// --- Query strings, hidden headers, fitted scroll views (#7, #8, #16) -----------

struct Period(String);

impl Render for Period {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        screen("Analytics").child(text(format!("period {}", self.0)))
    }
}

#[test]
fn pages_open_with_their_query_string() {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let r = Router::new()
            .route("/", |cx, p| {
                let period = p.query("period").unwrap_or("all").to_string();
                cx.new_entity(|_| Period(period))
            })
            .build(cx);
        cx.set_root(r);
    });
    let (app, dir) = web_app(init, "query");
    let get = |path: &str| {
        let r = app.handle(&Request::get(path).body(()).unwrap());
        String::from_utf8(r.body().clone()).unwrap()
    };
    assert!(get("/?period=7%20d").contains("period 7 d"));
    assert!(get("/").contains("period all"));
    // The socket's `path` carries it too (the page's pathname + search).
    let uri: Uri = "/ws?path=%2F%3Fperiod%3D30d&w=400&h=800".parse().unwrap();
    let (_, path, _) = app.socket_visitor(&uri, &HeaderMap::new());
    assert_eq!(path, "/?period=30d");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ssr_hides_the_header_of_a_headerless_stack_or_a_bare_screen() {
    let stack = |flags: f64, bare: f64| {
        html(&[
            create(1, NodeKind::Stack),
            Patch::SetValue {
                id: 1,
                value: flags,
            },
            create(2, NodeKind::Screen),
            Patch::SetValue { id: 2, value: bare },
            children(1, &[2]),
            Patch::SetRoot { id: 1 },
        ])
    };
    let header = r#"<div class="prism-header"><button"#;
    let hidden = r#"<div class="prism-header" hidden><button"#;
    assert!(stack(0., 0.).contains(header));
    assert!(stack(prism_core::STACK_NO_HEADER as f64, 0.).contains(hidden));
    assert!(stack(0., 1.).contains(hidden));
    // Closable alone keeps it.
    assert!(stack(prism_core::STACK_CLOSABLE as f64, 0.).contains(header));
}

#[test]
fn fitted_scroll_views_are_as_tall_as_their_content() {
    let s = Style {
        flex_grow: 0.,
        flex_shrink: 1.,
        flex_basis: Length::Auto,
        max_height: Length::Px(240.),
        ..Default::default()
    };
    let c = css(NodeKind::Scroll, &s);
    assert!(c.contains("max-height:240px;"), "{c}");
    assert!(c.contains("flex:0 1 auto;"), "{c}");
}

#[test]
fn the_root_stack_and_its_screens_fill_the_window() {
    let sheet = include_str!("prism.css");
    for rule in [
        "#prism-root:has(> .prism-stack) { height: 100%; height: 100dvh; min-height: 0; }",
        ".prism-stack { min-height: 0; }",
        ".prism-body > [data-kind=screen] { min-height: 0; overflow-y: auto;",
    ] {
        assert!(sheet.contains(rule), "{rule}");
    }
}
