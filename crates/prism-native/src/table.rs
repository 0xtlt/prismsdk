//! Tables (#284): the CSS automatic table layout, for every native backend.
//!
//! A [`table`](prism_core::table()) is a column of rows, each a row of
//! cells. Its columns are sized like a browser sizes a `<table>`
//! (`table-layout: auto`, the web backend's), from what their cells ask for:
//!
//! - each column's **min-content** width (its widest unbreakable content: a
//!   long word, `09:35`) and **max-content** width (its content on one
//!   line), cell padding included, measured from every cell of the column;
//! - a column given a width (`column(..).width(w)`) is that wide, or as wide
//!   as its min-content; one given a minimum (`min_width(w)`) is at least
//!   that wide;
//! - with more room than the max-content widths, the extra goes to the
//!   columns without a width, in proportion to their max-content width;
//! - with less, each column gets its min-content width plus a share of
//!   the rest in proportion to how much it can shrink (max − min): text
//!   that can wrap wraps, a column that can't shrink keeps its width;
//! - below the min-content widths, the table overflows.
//!
//! [`column_widths`] is that algorithm as a pure function. The layout runs
//! it inside taffy's flexbox, in one pass, without knowing the table's
//! width in advance: [`flex_columns`] turns the measured columns into each
//! cell's flex basis, grow and shrink factors and minimum width, which
//! give the cells exactly those widths in every row (the tests check both
//! against each other).
//!
//! The renderer measures each cell's min- and max-content widths with
//! taffy (the cell laid out alone), before a layout, for the tables whose
//! cells changed; rows keep no horizontal padding or gap of their own, so
//! their cells line up.

use crate::{NativeRenderer, Widgets};
use prism_core::{IdMap, IdSet, Length, NodeId, NodeKind, Patch};
use taffy::prelude::{AvailableSpace, Dimension, LengthPercentage, LengthPercentageAuto, Size};

/// What one column asks for, measured from its cells (points, cell
/// padding included).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColumnSizes {
    /// Its widest min-content cell: the narrowest it can be.
    pub min: f32,
    /// Its widest max-content cell: its content on one line.
    pub max: f32,
    /// A width set on the column (`column(..).width(w)`).
    pub fixed: Option<f32>,
    /// A minimum width set on the column (`column(..).min_width(w)`).
    pub min_width: Option<f32>,
}

impl ColumnSizes {
    /// The column's min and max widths with its own width settings
    /// applied, and whether it takes extra room (it has no fixed width).
    fn resolved(&self) -> (f32, f32, bool) {
        let min = self.min.max(self.min_width.unwrap_or(0.)).max(0.);
        match self.fixed {
            // A fixed width, unless the content can't get that narrow.
            Some(w) => {
                let w = w.max(min);
                (w, w, false)
            }
            None => (min, self.max.max(min), true),
        }
    }
}

/// Each column's width in a table whose rows are `available` points wide,
/// by the CSS automatic table layout (see the [module docs](self)).
pub fn column_widths(columns: &[ColumnSizes], available: f32) -> Vec<f32> {
    let cols: Vec<(f32, f32, bool)> = columns.iter().map(ColumnSizes::resolved).collect();
    let min: f32 = cols.iter().map(|c| c.0).sum();
    let max: f32 = cols.iter().map(|c| c.1).sum();
    if available <= min {
        return cols.iter().map(|c| c.0).collect();
    }
    if available <= max {
        // Between min and max, in proportion to how much each can shrink.
        let t = (available - min) / (max - min);
        return cols.iter().map(|c| c.0 + (c.1 - c.0) * t).collect();
    }
    let extra = available - max;
    let weights = grow_weights(&cols);
    let total: f32 = weights.iter().sum();
    cols.iter()
        .zip(&weights)
        .map(|(c, w)| c.1 + extra * w / total)
        .collect()
}

/// How the extra room is shared: columns without a width, in proportion
/// to their max-content width (equally if all are empty); with none,
/// the fixed columns the same way.
fn grow_weights(cols: &[(f32, f32, bool)]) -> Vec<f32> {
    let pick = |auto: bool| -> Option<Vec<f32>> {
        if !cols.iter().any(|c| c.2 == auto) {
            return None;
        }
        let by_max: Vec<f32> = cols
            .iter()
            .map(|c| if c.2 == auto { c.1 } else { 0. })
            .collect();
        if by_max.iter().sum::<f32>() > 0. {
            Some(by_max)
        } else {
            Some(
                cols.iter()
                    .map(|c| if c.2 == auto { 1. } else { 0. })
                    .collect(),
            )
        }
    };
    pick(true)
        .or_else(|| pick(false))
        .unwrap_or_else(|| vec![0.; cols.len()])
}

/// One column as flex parameters for its cells, in each row (a flex row):
/// they give the cells [`column_widths`]'s widths whatever the row's width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlexColumn {
    /// The cell's flex basis: the column's max-content width.
    pub basis: f32,
    /// Its share of the extra room.
    pub grow: f32,
    /// Its shrink factor: flexbox scales it by the basis, so the room is
    /// taken in proportion to `max − min`.
    pub shrink: f32,
    /// Its minimum width: the column's min-content width.
    pub min: f32,
}

/// The flex parameters giving each column its [`column_widths`] width.
pub fn flex_columns(columns: &[ColumnSizes]) -> Vec<FlexColumn> {
    let cols: Vec<(f32, f32, bool)> = columns.iter().map(ColumnSizes::resolved).collect();
    let grow = at_least_one(grow_weights(&cols));
    let shrink = at_least_one(
        cols.iter()
            .map(|&(min, max, _)| if max > 0. { (max - min) / max } else { 0. })
            .collect(),
    );
    cols.iter()
        .zip(grow.into_iter().zip(shrink))
        .map(|(&(min, max, _), (grow, shrink))| FlexColumn {
            basis: max,
            grow,
            shrink,
            min,
        })
        .collect()
}

/// Flex factors scaled so they add up to at least 1: below that, flexbox
/// hands out only that fraction of the free space (CSS Flexbox §9.7).
fn at_least_one(factors: Vec<f32>) -> Vec<f32> {
    let sum: f32 = factors.iter().sum();
    if sum > 0. && sum < 1. {
        factors.iter().map(|f| f * 2. / sum).collect()
    } else {
        factors
    }
}

impl FlexColumn {
    /// Gives a cell's flex style its column's width.
    pub(crate) fn apply(&self, style: &mut taffy::Style) {
        style.flex_basis = Dimension::length(self.basis);
        style.flex_grow = self.grow;
        style.flex_shrink = self.shrink;
        style.size.width = Dimension::auto();
        style.min_size.width = LengthPercentageAuto::length(self.min);
    }
}

/// A table's parts as flex boxes: the table a column of rows, each row a
/// row of cells with no horizontal padding or gap (they would shift its
/// cells off their columns), each cell a column of its content.
pub(crate) fn apply(style: &mut taffy::Style, kind: NodeKind) {
    match kind {
        NodeKind::Table => {
            style.flex_direction = taffy::FlexDirection::Column;
            style.flex_wrap = taffy::FlexWrap::NoWrap;
            style.align_items = Some(taffy::AlignItems::STRETCH);
            // Like a browser's table: no wider than its container (rows
            // would size to their content, not to the room they have),
            // and no narrower than its columns' minimum (`Tables::mins`).
            if style.max_size.width == LengthPercentageAuto::auto() {
                style.max_size.width = LengthPercentageAuto::percent(1.);
            }
        }
        NodeKind::TableRow => {
            style.flex_direction = taffy::FlexDirection::Row;
            style.flex_wrap = taffy::FlexWrap::NoWrap;
            style.align_items = Some(taffy::AlignItems::STRETCH);
            style.gap.width = LengthPercentage::length(0.);
            style.padding.left = LengthPercentage::length(0.);
            style.padding.right = LengthPercentage::length(0.);
        }
        _ => {}
    }
}

/// The tables of a renderer and their columns.
#[derive(Default)]
pub(crate) struct Tables {
    /// Every table.
    pub ids: IdSet<NodeId>,
    /// Tables to size again before the next layout: a cell changed.
    stale: IdSet<NodeId>,
    /// Each cell's column, as flex parameters.
    pub cells: IdMap<NodeId, FlexColumn>,
    /// Each table's minimum width: its columns' (and its padding).
    pub mins: IdMap<NodeId, f32>,
}

impl Tables {
    /// Every table sizes its columns again (zoom, text size, fonts).
    pub fn all_stale(&mut self) {
        self.stale.clone_from(&self.ids);
    }

    pub fn forget(&mut self, id: NodeId) {
        self.ids.remove(&id);
        self.stale.remove(&id);
        self.cells.remove(&id);
        self.mins.remove(&id);
    }

    /// Gives a table's flex style its minimum width.
    pub fn apply_min(&self, id: NodeId, style: &mut taffy::Style) {
        if let Some(&min) = self.mins.get(&id) {
            // Its own minimum, in points; a percentage gives way.
            let raw = style.min_size.width.into_raw();
            let own = if raw.tag() == taffy::CompactLength::LENGTH_TAG {
                raw.value()
            } else {
                0.
            };
            style.min_size.width = LengthPercentageAuto::length(min.max(own));
        }
    }
}

/// One row of a table, as [`Widgets::set_table`] gets it.
pub struct TableRow<'a, V> {
    /// The row's view.
    pub view: &'a V,
    /// A row of column headers (the table's first).
    pub header: bool,
    /// Its cells' views, in column order.
    pub cells: Vec<&'a V>,
}

/// The node a patch changes something of that can change its size.
fn sized_by(patch: &Patch) -> Option<NodeId> {
    match patch {
        Patch::SetStyle { id, .. }
        | Patch::SetText { id, .. }
        | Patch::SetRichValue { id, .. }
        | Patch::SetChildren { id, .. }
        | Patch::SetConfig { id, .. }
        | Patch::SetIcon { id, .. }
        | Patch::SetValue { id, .. }
        | Patch::SetPlaceholder { id, .. } => Some(*id),
        _ => None,
    }
}

impl<W: Widgets> NativeRenderer<W> {
    /// After a batch of patches: the tables around a changed node size
    /// their columns again, and the ones whose rows or cells changed tell
    /// the platform.
    pub(crate) fn note_table_patches(&mut self, patches: &[Patch]) {
        let mut structure = IdSet::default();
        for patch in patches {
            if let Patch::SetEnvironment { .. } = patch {
                self.tables.all_stale();
            }
            let Some(id) = sized_by(patch) else { continue };
            let reshaped = matches!(patch, Patch::SetChildren { .. })
                && self
                    .nodes
                    .get(&id)
                    .is_some_and(|e| matches!(e.kind, NodeKind::Table | NodeKind::TableRow));
            // Every table around it: a table in a cell sizes its column.
            let mut node = Some(id);
            let mut nearest = true;
            while let Some(n) = node {
                if self.tables.ids.contains(&n) {
                    self.tables.stale.insert(n);
                    if reshaped && std::mem::take(&mut nearest) {
                        structure.insert(n);
                    }
                }
                node = self.extras.parents.get(&n).copied();
            }
        }
        for table in structure {
            self.announce_table(table);
        }
    }

    /// The rows of `table`, each with its cells.
    fn table_grid(&self, table: NodeId) -> Vec<(NodeId, Vec<NodeId>)> {
        let children = |id: &NodeId| self.extras.children.get(id).map_or(&[][..], Vec::as_slice);
        let kind = |id: &NodeId| self.nodes.get(id).map(|e| e.kind);
        children(&table)
            .iter()
            .filter(|r| kind(r) == Some(NodeKind::TableRow))
            .map(|r| {
                let cells = children(r)
                    .iter()
                    .copied()
                    .filter(|c| kind(c).is_some_and(NodeKind::is_table_cell))
                    .collect();
                (*r, cells)
            })
            .collect()
    }

    /// Hands the platform a table's rows and cells ([`Widgets::set_table`]).
    fn announce_table(&mut self, table: NodeId) {
        let grid = self.table_grid(table);
        let nodes = &self.nodes;
        let Some(t) = nodes.get(&table) else { return };
        let rows: Vec<TableRow<'_, W::View>> = grid
            .iter()
            .filter_map(|(row, cells)| {
                Some(TableRow {
                    view: &nodes.get(row)?.view,
                    header: cells
                        .first()
                        .and_then(|c| nodes.get(c))
                        .is_some_and(|e| e.kind == NodeKind::ColumnHeader),
                    cells: cells
                        .iter()
                        .filter_map(|c| nodes.get(c).map(|e| &e.view))
                        .collect(),
                })
            })
            .collect();
        self.widgets.set_table(&t.view, &rows);
    }

    /// Sizes the columns of the tables whose cells changed (see the
    /// [module docs](self)).
    pub(crate) fn measure_tables(&mut self) {
        if self.tables.stale.is_empty() {
            return;
        }
        let stale: Vec<NodeId> = self.tables.stale.drain().collect();
        for table in stale {
            self.measure_table(table);
        }
    }

    fn measure_table(&mut self, table: NodeId) {
        let rows = self.table_grid(table);
        let count = rows.iter().map(|(_, cells)| cells.len()).max().unwrap_or(0);
        let mut columns = vec![ColumnSizes::default(); count];
        for (_, cells) in &rows {
            for (column, cell) in columns.iter_mut().zip(cells) {
                let Some(e) = self.nodes.get(cell) else {
                    continue;
                };
                let zoomed = e.style.scaled(self.zoom);
                let layout = e.layout;
                // A width set on a cell (the column's, on its header) sizes
                // the column; its content is measured without it.
                if let Length::Px(w) = zoomed.width {
                    column.fixed = Some(column.fixed.map_or(w, |f| f.max(w)));
                }
                if let Length::Px(w) = zoomed.min_width {
                    column.min_width = Some(column.min_width.map_or(w, |m| m.max(w)));
                }
                let mut style = crate::to_taffy(&zoomed, e.kind);
                style.size.width = Dimension::auto();
                style.min_size.width = LengthPercentageAuto::length(0.);
                let _ = self.taffy.set_style(layout, style);
                column.min = column.min.max(self.cell_width(layout, true));
                column.max = column.max.max(self.cell_width(layout, false));
            }
        }
        let flex = flex_columns(&columns);
        if let Some(e) = self.nodes.get(&table) {
            let p = e.style.padding;
            let inset = (p.left + p.right) * self.zoom;
            let min = flex.iter().map(|f| f.min).sum::<f32>() + inset;
            self.tables.mins.insert(table, min);
            self.sync_layout_style(table);
        }
        for (_, cells) in &rows {
            for (column, cell) in flex.iter().zip(cells) {
                self.tables.cells.insert(*cell, *column);
                self.sync_layout_style(*cell);
            }
        }
    }

    /// The min- or max-content width of the subtree at `node`, laid out
    /// alone.
    fn cell_width(&mut self, node: taffy::NodeId, min: bool) -> f32 {
        let width = if min {
            AvailableSpace::MinContent
        } else {
            AvailableSpace::MaxContent
        };
        let available = Size {
            width,
            height: AvailableSpace::MaxContent,
        };
        if self.compute_in(node, available) {
            self.taffy.unrounded_layout(node).size.width
        } else {
            0.
        }
    }

    /// A cell's x and width in its row, its edges rounded from the row's
    /// start. Taffy rounds a location within its parent but a width within
    /// the window: at a fractional offset (density scaling), neighbouring
    /// cells would be 1 px apart, a seam in the separators.
    pub(crate) fn cell_span(&self, layout: taffy::NodeId) -> (f32, f32) {
        let u = self.taffy.unrounded_layout(layout);
        let x = u.location.x.round();
        (x, (u.location.x + u.size.width).round() - x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use taffy::prelude::{FlexDirection, Style, TaffyMaxContent, TaffyTree};

    fn col(min: f32, max: f32) -> ColumnSizes {
        ColumnSizes {
            min,
            max,
            ..Default::default()
        }
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
    }

    #[track_caller]
    fn check(columns: &[ColumnSizes], available: f32, want: &[f32]) {
        let got = column_widths(columns, available);
        assert!(
            close(&got, want),
            "{columns:?} in {available}: {got:?}, want {want:?}"
        );
        // The flex encoding the layout uses gives the same widths.
        let flex = flex_row(columns, available);
        assert!(close(&flex, want), "flex row: {flex:?}, want {want:?}");
    }

    /// Lays a row of cells out with taffy, as the layout does.
    fn flex_row(columns: &[ColumnSizes], available: f32) -> Vec<f32> {
        let mut t: TaffyTree<()> = TaffyTree::new();
        t.disable_rounding();
        let cells: Vec<taffy::NodeId> = flex_columns(columns)
            .iter()
            .map(|f| {
                t.new_leaf(Style {
                    flex_basis: Dimension::length(f.basis),
                    flex_grow: f.grow,
                    flex_shrink: f.shrink,
                    min_size: Size {
                        width: LengthPercentageAuto::length(f.min),
                        height: LengthPercentageAuto::auto(),
                    },
                    ..Default::default()
                })
                .unwrap()
            })
            .collect();
        let row = t
            .new_with_children(
                Style {
                    flex_direction: FlexDirection::Row,
                    size: Size {
                        width: Dimension::length(available),
                        height: Dimension::auto(),
                    },
                    ..Default::default()
                },
                &cells,
            )
            .unwrap();
        t.compute_layout(row, Size::MAX_CONTENT).unwrap();
        cells
            .iter()
            .map(|c| t.layout(*c).unwrap().size.width)
            .collect()
    }

    #[test]
    fn equal_content_shares_equally() {
        let three = [col(40., 80.); 3];
        check(&three, 240., &[80., 80., 80.]);
        check(&three, 360., &[120., 120., 120.]);
        check(&three, 180., &[60., 60., 60.]);
    }

    #[test]
    fn one_long_column_takes_the_extra_room_in_proportion() {
        // Time, Line, Destination: the extra room follows max-content.
        let cols = [col(52., 52.), col(28., 28.), col(90., 260.)];
        check(&cols, 340., &[52., 28., 260.]);
        // 60 more: 52/340, 28/340 and 260/340 of it.
        check(&cols, 400., &[61.18, 32.94, 305.88]);
    }

    #[test]
    fn too_narrow_shrinks_toward_min_content() {
        // Time can't wrap (min = max): it keeps its width; Destination
        // wraps.
        let cols = [col(52., 52.), col(28., 28.), col(90., 260.)];
        check(&cols, 250., &[52., 28., 170.]);
        // Two that can shrink share the shortfall by how much they can.
        let cols = [col(20., 120.), col(30., 80.), col(50., 50.)];
        // min 100, max 250: at 175, half of each one's range.
        check(&cols, 175., &[70., 55., 50.]);
        // Below min-content: every column at its minimum (overflow).
        check(&cols, 60., &[20., 30., 50.]);
    }

    #[test]
    fn a_fixed_column_keeps_its_width() {
        let fixed = ColumnSizes {
            fixed: Some(100.),
            ..col(30., 60.)
        };
        let cols = [fixed, col(40., 200.)];
        // The other column takes all the extra room.
        check(&cols, 400., &[100., 300.]);
        // And shrinks first.
        check(&cols, 200., &[100., 100.]);
        check(&cols, 120., &[100., 40.]);
        // A fixed width narrower than the content's minimum: the minimum.
        let narrow = ColumnSizes {
            fixed: Some(20.),
            ..col(52., 52.)
        };
        check(&[narrow, col(10., 10.)], 200., &[52., 148.]);
        // Only fixed columns: they share the extra room.
        let a = ColumnSizes {
            fixed: Some(50.),
            ..col(0., 0.)
        };
        let b = ColumnSizes {
            fixed: Some(150.),
            ..col(0., 0.)
        };
        check(&[a, b], 400., &[100., 300.]);
    }

    #[test]
    fn a_minimum_width_raises_both_widths() {
        let min = ColumnSizes {
            min_width: Some(120.),
            ..col(30., 60.)
        };
        let cols = [min, col(40., 200.)];
        check(&cols, 320., &[120., 200.]);
        // Extra room: in proportion to the max-content widths (120, 200).
        check(&cols, 480., &[180., 300.]);
        // Narrow: the minimum holds.
        check(&cols, 200., &[120., 80.]);
    }

    #[test]
    fn empty_cells_and_columns() {
        // An empty column (padding only) takes little extra: 120 more,
        // split 20:100.
        let cols = [col(20., 20.), col(40., 100.)];
        check(&cols, 240., &[40., 200.]);
        // A column with nothing at all takes nothing.
        let cols = [col(0., 0.), col(40., 100.)];
        check(&cols, 200., &[0., 200.]);
        // Only empty columns: they share the room equally.
        check(&[col(0., 0.); 2], 100., &[50., 50.]);
        // No column.
        assert!(column_widths(&[], 100.).is_empty());
        assert!(flex_columns(&[]).is_empty());
    }

    #[test]
    fn flex_encoding_matches_everywhere() {
        // A sweep over widths and mixes of columns, fixed and minimum.
        let sets: [&[ColumnSizes]; 4] = [
            &[col(10., 50.), col(30., 30.), col(5., 200.)],
            &[
                col(44., 44.),
                ColumnSizes {
                    min_width: Some(90.),
                    ..col(20., 70.)
                },
                col(60., 400.),
            ],
            &[
                ColumnSizes {
                    fixed: Some(64.),
                    ..col(12., 30.)
                },
                col(0., 0.),
                col(25., 150.),
            ],
            &[col(1., 1.), col(2., 300.), col(150., 160.), col(8., 9.)],
        ];
        for cols in sets {
            for available in (0..=60).map(|i| i as f32 * 12.5) {
                let want = column_widths(cols, available);
                let got = flex_row(cols, available);
                assert!(
                    close(&got, &want),
                    "{cols:?} in {available}: {got:?} vs {want:?}"
                );
            }
        }
    }
}
