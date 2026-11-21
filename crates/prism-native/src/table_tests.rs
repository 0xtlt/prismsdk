//! Tables laid out by the renderer (#284): columns sized from their cells,
//! the same in every row, and handed to the platform's accessibility.

use std::collections::HashMap;
use std::sync::Arc;

use prism_core::{
    Accessibility, AppInit, Backend, FluentBuilder, IntoElement, NodeId, NodeKind, ParentElement,
    Render, Runtime, Style, Styled, cell, column, div, row, table, text,
};

use crate::{Insets, NativeRenderer, Viewport, Widgets, table::TableRow};

/// A row handed to `set_table`: the row, whether it is a header, its cells.
type Row = (NodeId, bool, Vec<NodeId>);

/// Text is 10 points per character on 20-point lines, wrapped between
/// words at the width it is given.
#[derive(Default)]
struct Fake {
    kinds: HashMap<NodeId, NodeKind>,
    texts: HashMap<NodeId, String>,
    children: HashMap<NodeId, Vec<NodeId>>,
    frames: HashMap<NodeId, (f32, f32, f32, f32)>,
    /// Each `set_table`: the table, then its rows (header or not, cells).
    tables: Vec<(NodeId, Vec<Row>)>,
}

fn lines(t: &str, width: f32) -> usize {
    let mut lines = 1;
    let mut line = 0.;
    for word in t.split_whitespace() {
        let w = word.len() as f32 * 10.;
        let with_space = if line > 0. { line + 10. + w } else { w };
        if line > 0. && with_space > width {
            lines += 1;
            line = w;
        } else {
            line = with_space;
        }
    }
    lines
}

impl Widgets for Fake {
    type View = NodeId;
    fn create(&mut self, id: NodeId, kind: NodeKind) -> NodeId {
        self.kinds.insert(id, kind);
        id
    }
    fn set_style(&mut self, _: &NodeId, _: &Style) {}
    fn set_text(&mut self, v: &NodeId, t: &str) {
        self.texts.insert(*v, t.into());
    }
    fn set_placeholder(&mut self, _: &NodeId, _: &str) {}
    fn set_clickable(&mut self, _: &NodeId, _: bool) {}
    fn set_children(&mut self, v: &NodeId, children: &[NodeId]) {
        self.children.insert(*v, children.to_vec());
    }
    fn set_root(&mut self, _: &NodeId) {}
    fn destroy(&mut self, _: &NodeId) {}
    fn measure(&mut self, v: &NodeId, max_width: Option<f32>) -> (f32, f32) {
        let t = self.texts.get(v).cloned().unwrap_or_default();
        let natural = t.len() as f32 * 10.;
        match max_width {
            Some(w) if w < natural => {
                let n = lines(&t, w);
                (if n == 1 { natural } else { w }, 20. * n as f32)
            }
            _ => (natural, 20.),
        }
    }
    fn measure_min_content(&mut self, v: &NodeId) -> f32 {
        let t = self.texts.get(v).cloned().unwrap_or_default();
        t.split_whitespace().map(|w| w.len()).max().unwrap_or(0) as f32 * 10.
    }
    fn set_frame(&mut self, v: &NodeId, x: f32, y: f32, w: f32, h: f32) {
        self.frames.insert(*v, (x, y, w, h));
    }
    fn set_accessibility(&mut self, _: &NodeId, _: &Accessibility) {}
    fn set_value(&mut self, _: &NodeId, _: f64) {}
    fn screen_viewport(&mut self, _: &NodeId, _: &NodeId) -> Viewport {
        Viewport::default()
    }
    fn set_table(&mut self, t: &NodeId, rows: &[TableRow<'_, NodeId>]) {
        let rows = rows
            .iter()
            .map(|r| (*r.view, r.header, r.cells.iter().map(|c| **c).collect()))
            .collect();
        self.tables.push((*t, rows));
    }
}

impl Fake {
    fn of_kind(&self, kind: NodeKind) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .kinds
            .iter()
            .filter(|(_, k)| **k == kind)
            .map(|(id, _)| *id)
            .collect();
        ids.sort();
        ids
    }

    /// The table's rows, each its cells' frames.
    fn grid(&self) -> Vec<Vec<(f32, f32, f32, f32)>> {
        let t = self.of_kind(NodeKind::Table)[0];
        self.children[&t]
            .iter()
            .map(|r| self.children[r].iter().map(|c| self.frames[c]).collect())
            .collect()
    }
}

/// A departures board: the destination is the only column that wraps.
struct Board {
    width: Option<f32>,
}

impl Render for Board {
    fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
        let t = table()
            .column(column("Time").width_maybe(self.width))
            .column("Line")
            .column("Destination")
            .row(
                row()
                    .child("09:35")
                    .child("12")
                    .child("Gare de Lyon par Bastille"),
            )
            .row(row().child("09:41").child("4").child("Porte d'Orléans"));
        div().items_start().child(t)
    }
}

trait WidthMaybe {
    fn width_maybe(self, w: Option<f32>) -> Self;
}

impl WidthMaybe for prism_core::Column {
    fn width_maybe(self, w: Option<f32>) -> Self {
        match w {
            Some(w) => self.width(w),
            None => self,
        }
    }
}

fn board(width: f32, fixed: Option<f32>) -> NativeRenderer<Fake> {
    let init: AppInit = Arc::new(move |cx: &mut prism_core::App| {
        let r = cx.new_entity(|_| Board { width: fixed });
        cx.set_root(r);
    });
    let mut rt = Runtime::new(&init);
    let mut r = NativeRenderer::new(Fake::default());
    r.apply(&rt.render());
    r.layout(width, 600., Insets::default());
    r
}

/// Cells have 10 points of padding on each side (`CELL_PADDING`).
const PAD: f32 = 20.;

#[track_caller]
fn assert_columns_line_up(grid: &[Vec<(f32, f32, f32, f32)>]) {
    for row in grid {
        assert_eq!(row.len(), 3);
        for (c, cell) in row.iter().enumerate() {
            assert_eq!((cell.0, cell.2), (grid[0][c].0, grid[0][c].2), "{grid:?}");
        }
    }
}

#[test]
fn a_table_sizes_its_columns_from_its_cells() {
    // Room for everything: the table is as wide as its content (it sits at
    // the start of a column), each column as wide as its widest cell.
    let r = board(800., None);
    let grid = r.widgets.grid();
    assert_columns_line_up(&grid);
    let widths: Vec<f32> = grid[0].iter().map(|c| c.2).collect();
    // "09:35", "Line", "Gare de Lyon par Bastille".
    assert_eq!(widths, [50. + PAD, 40. + PAD, 250. + PAD]);
    assert_eq!(
        grid[1][2].3,
        20. + 12.,
        "one line, 6 points above and below"
    );
}

#[test]
fn a_narrow_table_wraps_the_column_that_can_wrap() {
    // 300 points: Time and Line can't get narrower, Destination takes the
    // rest (min 130 for its header, max 270).
    let r = board(300., None);
    let grid = r.widgets.grid();
    assert_columns_line_up(&grid);
    let widths: Vec<f32> = grid[0].iter().map(|c| c.2).collect();
    assert_eq!(widths, [70., 60., 170.]);
    // "Gare de Lyon par Bastille" in 150 points: two lines, and the row's
    // other cells as tall.
    assert_eq!(grid[1][2].3, 2. * 20. + 12.);
    assert_eq!(grid[1][0].3, grid[1][2].3);
}

#[test]
fn a_table_never_gets_narrower_than_its_columns_minimum() {
    // Like a browser's: it overflows its 200 points.
    let r = board(200., None);
    let grid = r.widgets.grid();
    assert_columns_line_up(&grid);
    let widths: Vec<f32> = grid[0].iter().map(|c| c.2).collect();
    assert_eq!(widths, [70., 60., 130.]);
    let t = r.widgets.of_kind(NodeKind::Table)[0];
    assert_eq!(r.widgets.frames[&t].2, 260.);
}

#[test]
fn a_column_given_a_width_keeps_it() {
    let r = board(800., Some(120.));
    let grid = r.widgets.grid();
    assert_columns_line_up(&grid);
    assert_eq!(grid[0][0].2, 120.);
    assert_eq!(grid[0][1].0, 120.);
}

#[test]
fn the_platform_gets_the_rows_and_cells() {
    let r = board(800., None);
    let w = &r.widgets;
    let (t, rows) = w.tables.last().expect("set_table");
    assert_eq!(*t, w.of_kind(NodeKind::Table)[0]);
    assert_eq!(rows.len(), 3);
    assert!(rows[0].1 && !rows[1].1 && !rows[2].1);
    assert_eq!(rows[0].2, w.of_kind(NodeKind::ColumnHeader));
    let cells = w.of_kind(NodeKind::TableCell);
    assert_eq!(rows[1].2, cells[..3]);
    assert_eq!(rows[2].2, cells[3..]);
}

/// A table that fills its container: extra room by max-content.
struct Wide;

impl Render for Wide {
    fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
        div().child(
            table()
                .columns(["A", "B"])
                .row(row().child(cell(text("xx"))).child("yyyyyy")),
        )
    }
}

#[test]
fn a_stretched_table_shares_the_extra_room_by_content() {
    let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
        let r = cx.new_entity(|_| Wide);
        cx.set_root(r);
    });
    let mut rt = Runtime::new(&init);
    let mut r = NativeRenderer::new(Fake::default());
    r.apply(&rt.render());
    r.layout(400., 600., Insets::default());
    let grid = r.widgets.grid();
    // Max-content 40 and 80; 280 more, shared 40:80.
    assert_eq!(grid[0][0].2.round(), (40. + 280. / 3.0_f32).round());
    assert_eq!(grid[1][1].2.round(), (80. + 280. * 2. / 3.0_f32).round());
}

/// A stretched table at a fractional offset (as density scaling makes).
struct Offset;

impl Render for Offset {
    fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
        div().pl(10.4).child(
            table()
                .columns(["A", "B", "C"])
                .row(row().child("x").child("yyy").child("zz"))
                .row(row().child("xxxx").child("y").child("z")),
        )
    }
}

#[test]
fn neighbouring_cells_meet_at_a_fractional_offset() {
    let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
        let r = cx.new_entity(|_| Offset);
        cx.set_root(r);
    });
    let mut rt = Runtime::new(&init);
    let mut r = NativeRenderer::new(Fake::default());
    r.apply(&rt.render());
    r.layout(401.3, 600., Insets::default());
    // No 1-point seam in the row separators.
    for row in r.widgets.grid() {
        for pair in row.windows(2) {
            assert_eq!(pair[0].0 + pair[0].2, pair[1].0, "{row:?}");
        }
    }
}

/// A grid of two `auto` columns, stretched or not.
struct Tracks {
    stretch: bool,
}

impl Render for Tracks {
    fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
        use prism_core::layout::GridTrack;
        div()
            .grid()
            .grid_cols([GridTrack::Auto, GridTrack::Auto])
            .when(self.stretch, |d| d.justify_stretch())
            .child(text("ab"))
            .child(text("abcd"))
    }
}

#[test]
fn justify_stretch_makes_a_grids_auto_columns_fill_it() {
    let frames = |stretch: bool| {
        let init: AppInit = Arc::new(move |cx: &mut prism_core::App| {
            let r = cx.new_entity(move |_| Tracks { stretch });
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut r = NativeRenderer::new(Fake::default());
        r.apply(&rt.render());
        r.layout(400., 600., Insets::default());
        let texts = r.widgets.of_kind(NodeKind::Text);
        texts
            .iter()
            .map(|t| r.widgets.frames[t].2)
            .collect::<Vec<_>>()
    };
    // Packed at the start, as wide as their content.
    assert_eq!(frames(false), [20., 40.]);
    // The 340 points left go to both columns equally, like CSS.
    assert_eq!(frames(true), [190., 210.]);
}
