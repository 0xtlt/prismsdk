//! Tables (#284): columns with headers, then rows of cells.
//!
//! ```ignore
//! table()
//!     .column("Time")
//!     .column("Line")
//!     .column(column("Destination").min_width(120.))
//!     .column(column("Platform").align_end())
//!     .separators()
//!     .rows(departures.iter().map(|d| {
//!         row()
//!             .key(d.id.to_string())
//!             .child(d.time.clone())
//!             .child(line_badge(&d.line))
//!             .child(d.destination.clone())
//!             .child(d.platform.to_string())
//!     }))
//! ```
//!
//! A cell holds any element. Columns are as wide as their content asks,
//! like a browser's table: on the web the table is a real `<table>` (with
//! `<thead>` and `<th scope="col">`), and native backends size the columns
//! with the same algorithm, the CSS automatic table layout
//! (`prism_native::table`): each column's narrowest and widest content,
//! extra room shared in proportion to the widest, and columns that shrink
//! toward their narrowest when the table is narrow, so text that can wrap
//! wraps and `09:35` never does.
//!
//! Screen readers get a table, rows, column headers and cells: they read a
//! cell with its column's header ("Time, 09:35") and move by row and
//! column, where the platform allows it (see the accessibility guide).

use crate::a11y::{Accessibility, Accessible};
use crate::element::{ClickEvent, Element, FluentBuilder, Handler, IntoElement, NodeKind, VNode};
use crate::style::{Align, Color, FontWeight, Justify, Length, Styled, TextAlign, rgba};
use crate::{ParentElement, Style};

/// The padding of a cell unless the table or the cell sets one: points,
/// top and bottom, then leading and trailing.
pub const CELL_PADDING: (f32, f32) = (6., 10.);

/// How wide a column is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ColumnWidth {
    /// From its content (the default).
    Auto,
    /// This many points, padding included (more if a cell's content
    /// cannot get narrower).
    Fixed(f32),
    /// At least this many points, padding included.
    Min(f32),
}

/// One column of a [`table`]: its header and how its cells are laid out.
/// A string is a column with that header: `.column("Time")`.
pub struct Column {
    header: Element,
    align: TextAlign,
    width: ColumnWidth,
}

/// A column headed by `header`: a string, or any element (an icon and a
/// label). The header is read with each cell of the column.
pub fn column(header: impl IntoElement) -> Column {
    Column {
        header: header.into_element(),
        align: TextAlign::Left,
        width: ColumnWidth::Auto,
    }
}

impl Column {
    /// Aligns the column's cells (and its header): leading by default.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    /// Aligns the column's cells on their trailing edge: numbers, prices,
    /// durations (with the table's tabular digits, they line up).
    pub fn align_end(self) -> Self {
        self.align(TextAlign::Right)
    }

    /// Centers the column's cells.
    pub fn align_center(self) -> Self {
        self.align(TextAlign::Center)
    }

    /// A column `width` points wide, cell padding included: the other
    /// columns share the rest. It still grows to fit a cell's content that
    /// cannot get narrower (a long word, a picture).
    pub fn width(mut self, width: f32) -> Self {
        self.width = ColumnWidth::Fixed(width.max(0.));
        self
    }

    /// A column at least `width` points wide, cell padding included; wider
    /// when its content asks for it and there is room.
    pub fn min_width(mut self, width: f32) -> Self {
        self.width = ColumnWidth::Min(width.max(0.));
        self
    }
}

impl From<&str> for Column {
    fn from(header: &str) -> Self {
        column(header)
    }
}

impl From<String> for Column {
    fn from(header: String) -> Self {
        column(header)
    }
}

/// A table: [`column`](fn@column)s with headers, then [`row`]s of cells. See the
/// [module docs](self).
pub struct Table {
    node: VNode,
    columns: Vec<Column>,
    rows: Vec<Element>,
    separators: Option<Color>,
    tabular: bool,
    padding: (f32, f32),
}

/// A table of data: columns with headers, then rows of cells, with real
/// table semantics on every platform. See the [module docs](mod@crate::table).
pub fn table() -> Table {
    Table {
        node: VNode::new(NodeKind::Table),
        columns: Vec::new(),
        rows: Vec::new(),
        separators: None,
        tabular: true,
        padding: CELL_PADDING,
    }
}

impl Table {
    /// Adds a column: a header string, or a [`column`](fn@column) with options.
    pub fn column(mut self, column: impl Into<Column>) -> Self {
        self.columns.push(column.into());
        self
    }

    /// Adds several columns.
    pub fn columns<C: Into<Column>>(mut self, columns: impl IntoIterator<Item = C>) -> Self {
        self.columns.extend(columns.into_iter().map(Into::into));
        self
    }

    /// Adds a row: a [`row`] of cells (or an element that renders one).
    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_element());
        self
    }

    /// Adds several rows.
    pub fn rows<R: IntoElement>(mut self, rows: impl IntoIterator<Item = R>) -> Self {
        self.rows
            .extend(rows.into_iter().map(IntoElement::into_element));
        self
    }

    /// A thin line between rows, and under the headers, in a gray that
    /// suits light and dark appearances.
    pub fn separators(self) -> Self {
        self.separator_color(rgba(0x808080, 0.3))
    }

    /// Lines between rows, and under the headers, in `color`.
    pub fn separator_color(mut self, color: Color) -> Self {
        self.separators = Some(color);
        self
    }

    /// The padding of every cell: `vertical` points above and below,
    /// `horizontal` before and after ([`CELL_PADDING`] by default). A cell
    /// given its own padding (`cell(..).p(..)`) keeps it.
    pub fn cell_padding(mut self, vertical: f32, horizontal: f32) -> Self {
        self.padding = (vertical.max(0.), horizontal.max(0.));
        self
    }

    /// Digits keep their proportional widths. By default every text in
    /// the table has tabular digits (`tabular_nums`), so figures line up
    /// from row to row.
    pub fn proportional_digits(mut self) -> Self {
        self.tabular = false;
        self
    }

    /// Stable identity among siblings.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.node.key = Some(key.into());
        self
    }

    /// A name to address the table from code ([`App::scroll_into_view`](crate::App::scroll_into_view)).
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.node.id = Some(id.into());
        self
    }
}

impl Styled for Table {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.node.style
    }
}

impl Accessible for Table {
    fn a11y_mut(&mut self) -> &mut Accessibility {
        &mut self.node.a11y
    }
}

impl FluentBuilder for Table {}

impl IntoElement for Table {
    fn into_element(self) -> Element {
        let Table {
            mut node,
            columns,
            rows,
            separators,
            tabular,
            padding,
        } = self;
        let layout = Layout {
            columns: columns.iter().map(|c| c.align).collect(),
            separators,
            tabular,
            padding,
        };
        // The header row, from the columns.
        let headers: Vec<Element> = columns
            .into_iter()
            .map(|c| {
                let mut cell = VNode::new(NodeKind::ColumnHeader);
                cell.style = cell_style();
                match c.width {
                    ColumnWidth::Auto => {}
                    ColumnWidth::Fixed(w) => cell.style.width = Length::Px(w),
                    ColumnWidth::Min(w) => cell.style.min_width = Length::Px(w),
                }
                cell.children = vec![header_text(c.header)];
                Element::Node(Box::new(cell))
            })
            .collect();
        let mut header = VNode::new(NodeKind::TableRow);
        header.key = Some("prism-table-header".into());
        header.children = headers;
        // Body rows: fragments flattened (`rows` of a `Vec`).
        let mut body = Vec::with_capacity(rows.len());
        for row in rows {
            flatten(row, &mut body);
        }
        let last = body.len();
        node.children = std::iter::once(Element::Node(Box::new(header)))
            .chain(body)
            .enumerate()
            .map(|(i, row)| layout.row(row, i < last))
            .collect();
        Element::Node(Box::new(node))
    }
}

/// Rows of a fragment, in order; nothing for an absent row.
fn flatten(e: Element, out: &mut Vec<Element>) {
    match e {
        Element::Fragment(v) => v.into_iter().for_each(|e| flatten(e, out)),
        Element::Empty => {}
        other => out.push(other),
    }
}

/// What the table applies to the rows it sees.
#[derive(Clone)]
struct Layout {
    /// Each column's alignment.
    columns: Vec<TextAlign>,
    separators: Option<Color>,
    tabular: bool,
    padding: (f32, f32),
}

impl Layout {
    /// Applies the columns' alignment, the padding, the separator (`line`:
    /// a row follows) and tabular digits to `row`'s cells. A row rendered
    /// by its own entity is out of reach: its cells keep their defaults.
    fn row(&self, row: Element, line: bool) -> Element {
        match row {
            Element::Node(mut n) if n.kind == NodeKind::TableRow => {
                // Its cells are made when it is built (`TableRow`); a row
                // node made by hand gets them here.
                n.children = cells(std::mem::take(&mut n.children));
                for (i, cell) in n.children.iter_mut().enumerate() {
                    if let Element::Node(c) = cell {
                        self.cell(c, i, line);
                    }
                }
                Element::Node(n)
            }
            Element::Lazy(f) => {
                let layout = self.clone();
                Element::Lazy(Box::new(move |app| layout.row(f(app), line)))
            }
            // Anything else is a row of one cell.
            Element::Node(n) => {
                let mut r = VNode::new(NodeKind::TableRow);
                r.children = vec![Element::Node(n)];
                self.row(Element::Node(Box::new(r)), line)
            }
            other => other,
        }
    }

    fn cell(&self, c: &mut VNode, column: usize, line: bool) {
        let s = &mut c.style;
        if !s.padding_set {
            let (v, h) = self.padding;
            s.padding = crate::style::Edges {
                top: v,
                right: h,
                bottom: v,
                left: h,
            };
        }
        if let Some(color) = self.separators.filter(|_| line) {
            s.border_sides
                .bottom
                .get_or_insert(crate::decoration::BorderSide {
                    width: 1.,
                    color: Some(color),
                });
        }
        let align = self.columns.get(column).copied().unwrap_or_default();
        if align != TextAlign::Left {
            s.align_items = match align {
                TextAlign::Center => Align::Center,
                _ => Align::End,
            };
            for child in &mut c.children {
                if let Element::Node(t) = child
                    && t.kind == NodeKind::Text
                    && t.style.text_align == TextAlign::Left
                {
                    t.style.text_align = align;
                }
            }
        }
        if self.tabular {
            let children = std::mem::take(&mut c.children);
            c.children = children.into_iter().map(tabular).collect();
        }
    }
}

/// A new cell's style: content at the leading edge, centered vertically
/// (like a browser's `<td>`), with the default padding (not marked as set:
/// the table's padding replaces it).
fn cell_style() -> Style {
    let (v, h) = CELL_PADDING;
    Style {
        align_items: Align::Start,
        justify_content: Justify::Center,
        padding: crate::style::Edges {
            top: v,
            right: h,
            bottom: v,
            left: h,
        },
        ..Style::default()
    }
}

/// Tabular digits for every text in `e` (built lazily included).
fn tabular(e: Element) -> Element {
    match e {
        Element::Node(mut n) => {
            if n.kind == NodeKind::Text {
                n.style.tabular_numbers = true;
            }
            n.children = std::mem::take(&mut n.children)
                .into_iter()
                .map(tabular)
                .collect();
            Element::Node(n)
        }
        Element::Fragment(v) => Element::Fragment(v.into_iter().map(tabular).collect()),
        Element::Lazy(f) => Element::Lazy(Box::new(move |app| tabular(f(app)))),
        other => other,
    }
}

/// A header's text is semibold, like a browser's `<th>`, unless it sets a
/// weight.
fn header_text(e: Element) -> Element {
    match e {
        Element::Node(mut n) if n.kind == NodeKind::Text => {
            if n.style.font_weight == FontWeight::Regular {
                n.style.font_weight = FontWeight::Semibold;
            }
            Element::Node(n)
        }
        Element::Lazy(f) => Element::Lazy(Box::new(move |app| header_text(f(app)))),
        other => other,
    }
}

/// A row's children as cells: cells stay, anything else is put in one
/// (an absent child too, so the next ones stay in their columns).
fn cells(children: Vec<Element>) -> Vec<Element> {
    let mut out = Vec::with_capacity(children.len());
    for child in children {
        match child {
            Element::Fragment(v) => out.extend(cells(v)),
            Element::Node(n) if n.kind.is_table_cell() => out.push(Element::Node(n)),
            Element::Empty => out.push(Element::Node(Box::new(empty_cell()))),
            other => {
                let mut c = empty_cell();
                c.children = vec![other];
                out.push(Element::Node(Box::new(c)));
            }
        }
    }
    out
}

fn empty_cell() -> VNode {
    let mut c = VNode::new(NodeKind::TableCell);
    c.style = cell_style();
    c
}

/// A row of a [`table`]: each child is a cell (a string, any element, or a
/// [`cell`] with its own style). See [`row`].
pub struct TableRow(VNode);

/// A table row: `.child(..)` adds a cell. An absent child
/// (`.child(None::<Text>)`) is an empty cell, so the next ones stay in
/// their columns.
pub fn row() -> TableRow {
    TableRow(VNode::new(NodeKind::TableRow))
}

impl TableRow {
    /// Stable identity among the rows, so reordered rows keep their views.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.0.key = Some(key.into());
        self
    }

    /// A name to address the row from code.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.0.id = Some(id.into());
        self
    }

    /// Called when the row is clicked or tapped. Keyboard and screen
    /// reader users reach a row through its cells: give it a link or a
    /// button in a cell as well when the action matters.
    pub fn on_click(mut self, handler: Handler<ClickEvent>) -> Self {
        self.0.on_click = Some(handler);
        self
    }

    /// Long press (iOS, Android), right-click (macOS, web, Linux, Windows) menu.
    pub fn context_menu(mut self, menu: crate::menu::Menu) -> Self {
        self.0.context_menu = Some(menu);
        self
    }
}

impl ParentElement for TableRow {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Styled for TableRow {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.0.style
    }
}

impl Accessible for TableRow {
    fn a11y_mut(&mut self) -> &mut Accessibility {
        &mut self.0.a11y
    }
}

impl FluentBuilder for TableRow {}

impl IntoElement for TableRow {
    fn into_element(mut self) -> Element {
        self.0.children = cells(std::mem::take(&mut self.0.children));
        Element::Node(Box::new(self.0))
    }
}

/// One cell of a [`row`], styled on its own (a background, its own
/// padding, an accessibility label): `cell(text("Cancelled")).bg(..)`.
/// A row turns any other child into a cell by itself.
pub struct TableCell(VNode);

/// A cell holding `content`.
pub fn cell(content: impl IntoElement) -> TableCell {
    let mut c = empty_cell();
    c.children = vec![content.into_element()];
    TableCell(c)
}

impl ParentElement for TableCell {
    fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.0.children
    }
}

impl Styled for TableCell {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.0.style
    }
}

impl Accessible for TableCell {
    fn a11y_mut(&mut self) -> &mut Accessibility {
        &mut self.0.a11y
    }
}

impl FluentBuilder for TableCell {}

impl IntoElement for TableCell {
    fn into_element(self) -> Element {
        Element::Node(Box::new(self.0))
    }
}
