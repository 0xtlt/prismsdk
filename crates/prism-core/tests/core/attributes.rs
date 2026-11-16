//! App-defined attributes (`.attr(…)`, #85): `data-*` and a safe list,
//! sent once and again only when they change.

use prism_core::attributes::{RESERVED_DATA_ATTRIBUTES, is_app_attribute};
use prism_core::prelude::*;
use prism_core::{Element, NodeKind, Patch};
use prism_testing::{Harness, count, mount};

struct Form {
    code: bool,
    tag: &'static str,
}

impl Render for Form {
    fn render(&mut self, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(
            text_input()
                .placeholder("Code")
                .attr("data-1p-ignore", "")
                .attr("Data-TestId", self.tag)
                .when(self.code, |t| t.attr("autocomplete", "one-time-code")),
        )
    }
}

fn form() -> Harness {
    Harness::new(|cx| {
        let e = cx.new_entity(|_| Form {
            code: true,
            tag: "code",
        });
        mount(cx, e);
    })
}

fn field(h: &Harness) -> u64 {
    h.host.find(|n| n.kind == NodeKind::TextInput).unwrap()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect()
}

#[test]
fn attributes_are_sent_in_order_lower_cased() {
    let h = form();
    assert_eq!(
        h.host.get(field(&h)).attributes,
        pairs(&[
            ("data-1p-ignore", ""),
            ("data-testid", "code"),
            ("autocomplete", "one-time-code"),
        ])
    );
}

#[test]
fn attributes_are_patched_only_when_they_change() {
    let mut h = form();
    let id = field(&h);
    h.update::<Form>(|f| f.tag = "code");
    assert_eq!(
        count(&h.last, |p| matches!(p, Patch::SetAttributes { .. })),
        0,
        "{:?}",
        h.last
    );
    h.update::<Form>(|f| {
        f.code = false;
        f.tag = "otp";
    });
    assert_eq!(field(&h), id, "updated in place");
    assert_eq!(
        h.host.get(id).attributes,
        pairs(&[("data-1p-ignore", ""), ("data-testid", "otp")]),
        "the full list: autocomplete is gone"
    );
}

#[test]
fn setting_a_name_again_replaces_its_value() {
    let b = button("Save")
        .attr("data-testid", "a")
        .attr("DATA-TESTID", "b");
    let Element::Node(node) = b.into_element() else {
        unreachable!()
    };
    assert_eq!(node.attributes, pairs(&[("data-testid", "b")]));
}

#[test]
fn only_data_and_safe_names_are_app_attributes() {
    for ok in [
        "data-testid",
        "data-1p-ignore",
        "data-lpignore",
        "data-form-type",
        "data-a.b_c",
        "autocomplete",
        "translate",
    ] {
        assert!(is_app_attribute(ok), "{ok}");
    }
    for refused in [
        "onclick",
        "style",
        "href",
        "src",
        "class",
        "id",
        "role",
        "aria-label",
        "inputmode",
        "data-",
        "data-Test",
        "data-x y",
        "data-x\"",
        "data-prism",
        "data-prism-x",
        "data-pid",
        "data-kind",
        "autocomplete ",
    ] {
        assert!(!is_app_attribute(refused), "{refused}");
    }
    for name in RESERVED_DATA_ATTRIBUTES {
        assert!(!is_app_attribute(&format!("data-{name}")), "{name}");
    }
}

#[test]
#[cfg(not(debug_assertions))]
fn other_names_are_ignored() {
    let Element::Node(node) = div().attr("onclick", "alert(1)").into_element() else {
        unreachable!()
    };
    assert!(node.attributes.is_empty());
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "not an attribute an app may set")]
fn other_names_fail_a_debug_assertion() {
    let _ = div().attr("onclick", "alert(1)");
}
