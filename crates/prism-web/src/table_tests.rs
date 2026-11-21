//! Tables on the web (#284): a real `<table>`, its header row in `<thead>`
//! with `<th scope="col">` cells, laid out by the browser's table layout.

use std::sync::Arc;

use prism_core::prelude::*;
use prism_core::{AppInit, Backend, NodeKind, Runtime, Style};

use crate::css;
use crate::html::HtmlBackend;

struct Board;

impl Render for Board {
    fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
        div().child(
            table()
                .column("Time")
                .column("Line")
                .column(column("Platform").align_end())
                .row(row().child("09:35").child("12").child("4"))
                .row(row().child("09:41").child("4").child("11"))
                .separators(),
        )
    }
}

fn page() -> String {
    let init: AppInit = Arc::new(|cx: &mut App| {
        let r = cx.new_entity(|_| Board);
        cx.set_root(r);
    });
    let mut rt = Runtime::new(&init);
    let mut b = HtmlBackend::default();
    b.apply(&rt.render());
    b.to_html()
}

/// The opening tags in `page`, without their attributes.
fn tags(page: &str) -> Vec<String> {
    page.split('<')
        .skip(1)
        .filter_map(|t| {
            let name: String = t
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '/')
                .collect();
            ["table", "thead", "tbody", "tr", "th", "td"]
                .contains(&name.as_str())
                .then_some(name)
        })
        .collect()
}

#[test]
fn a_table_is_a_real_table_with_its_header_in_thead() {
    let page = page();
    assert_eq!(
        tags(&page).join(" "),
        "table thead tr th th th tbody tr td td td tr td td td"
    );
    assert_eq!(page.matches(r#"scope="col""#).count(), 3, "{page}");
    assert!(page.contains(r#"class="prism-table""#));
    // Each cell's content in its box.
    assert_eq!(page.matches(r#"<div class="prism-cell">"#).count(), 9);
    assert!(page.contains(">09:35</div>"));
}

#[test]
fn table_parts_keep_their_implicit_roles_and_cells_their_content() {
    let page = page();
    // No `role` or `aria-label` on a table's parts: the tags say it, and
    // a cell is read from its content.
    for kind in ["table", "table_row", "column_header", "table_cell"] {
        let at = page
            .find(&format!(r#"data-kind="{kind}""#))
            .unwrap_or_else(|| panic!("{kind} in {page}"));
        let tag_end = at + page[at..].find('>').unwrap();
        let tag = &page[at..tag_end];
        assert!(
            !tag.contains("role=") && !tag.contains("aria-label"),
            "{tag}"
        );
    }
}

#[test]
fn table_css_uses_the_browsers_table_layout() {
    let table = css::css(NodeKind::Table, &Style::default());
    assert!(table.contains("display:table;border-collapse:separate;border-spacing:0;"));
    assert!(!table.contains("display:flex"));
    assert!(css::css(NodeKind::TableRow, &Style::default()).contains("display:table-row;"));
    // A cell is vertically centered by default (CSS's `middle`), its box a
    // column; no font of its own (its texts have theirs).
    let cell_style = Style {
        justify_content: prism_core::Justify::Center,
        ..Default::default()
    };
    let cell = css::css(NodeKind::TableCell, &cell_style);
    assert!(cell.contains("display:table-cell;vertical-align:middle;flex-direction:column;"));
    assert!(!cell.contains("font-size"), "{cell}");
    let top = css::css(NodeKind::ColumnHeader, &Style::default());
    assert!(top.contains("vertical-align:top;"));
}

#[test]
fn a_grid_can_stretch_its_auto_columns() {
    let s = Style {
        grid: true,
        justify_content: prism_core::Justify::Stretch,
        ..Default::default()
    };
    assert!(css::css(NodeKind::View, &s).contains("justify-content:stretch;"));
}
