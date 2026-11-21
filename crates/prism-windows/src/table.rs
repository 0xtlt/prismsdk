//! Tables for UI Automation (#284), as browsers expose an HTML table and
//! WinUI its data grid: the table's peer has the Grid and Table patterns
//! (its row and column counts, the cell at a row and column, its column
//! headers), each cell's peer the GridItem and TableItem patterns (its row
//! and column, its table, its column's header), so that Narrator reads a
//! cell's column header with it. `set_table` keeps each table's grid here,
//! by node (no Win32 types: tested on every host), and `a11y::apply` the
//! element standing for the table and each cell, whenever it is made;
//! `peers` answers the patterns from both.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::cell::RefCell;
use std::collections::HashMap;

use prism_core::NodeId;

/// A table's cells, row by row, header rows first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub rows: Vec<Vec<NodeId>>,
    /// How many of the rows are header rows.
    pub headers: usize,
}

impl Grid {
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn column_count(&self) -> usize {
        self.rows.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub fn at(&self, row: usize, column: usize) -> Option<NodeId> {
        self.rows.get(row)?.get(column).copied()
    }

    /// Every column header: the header rows' cells.
    pub fn column_headers(&self) -> Vec<NodeId> {
        self.rows
            .iter()
            .take(self.headers)
            .flatten()
            .copied()
            .collect()
    }

    /// The headers of the cell at `row` and `column`: its column's header
    /// cells (none for a header cell).
    pub fn headers_of(&self, row: usize, column: usize) -> Vec<NodeId> {
        if row < self.headers {
            return Vec::new();
        }
        self.rows
            .iter()
            .take(self.headers)
            .filter_map(|r| r.get(column).copied())
            .collect()
    }
}

/// Where a cell is: its table, its row and its column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub table: NodeId,
    pub row: usize,
    pub column: usize,
}

thread_local! {
    static GRIDS: RefCell<HashMap<NodeId, Grid>> = RefCell::default();
    static CELLS: RefCell<HashMap<NodeId, Place>> = RefCell::default();
}

/// Table `table`'s cells are now `grid`'s.
pub fn set(table: NodeId, grid: Grid) {
    CELLS.with(|c| {
        let mut c = c.borrow_mut();
        // Its cells of before: moved or gone.
        c.retain(|_, p| p.table != table);
        for (row, cells) in grid.rows.iter().enumerate() {
            for (column, cell) in cells.iter().enumerate() {
                c.insert(*cell, Place { table, row, column });
            }
        }
    });
    GRIDS.with(|g| g.borrow_mut().insert(table, grid));
}

/// What `f` makes of table `table`'s grid, when it is a table.
pub fn with_grid<R>(table: NodeId, f: impl FnOnce(&Grid) -> R) -> Option<R> {
    GRIDS.with(|g| g.borrow().get(&table).map(f))
}

/// Where cell `cell` is, when it is a table's.
pub fn place(cell: NodeId) -> Option<Place> {
    CELLS.with(|c| c.borrow().get(&cell).copied())
}

/// Node `id` is gone (a table, a cell or anything else).
pub fn forget(id: NodeId) {
    if GRIDS.with(|g| g.borrow_mut().remove(&id)).is_some() {
        CELLS.with(|c| c.borrow_mut().retain(|_, p| p.table != id));
    }
    CELLS.with(|c| c.borrow_mut().remove(&id));
    #[cfg(target_os = "windows")]
    elements::forget(id);
}

#[cfg(target_os = "windows")]
pub use elements::{element, set_element};

/// The element standing for a table or a cell (its canvas, or its button
/// when it is clickable), whose peer `peers` hands to a client.
#[cfg(target_os = "windows")]
mod elements {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use prism_core::NodeId;
    use windows_core::{Interface, Weak};

    use crate::winui::xaml::UIElement;

    thread_local! {
        static ELEMENTS: RefCell<HashMap<NodeId, Weak<UIElement>>> = RefCell::default();
    }

    pub fn set_element(id: NodeId, e: &UIElement) {
        if let Ok(weak) = e.downgrade() {
            ELEMENTS.with(|m| m.borrow_mut().insert(id, weak));
        }
    }

    pub fn element(id: NodeId) -> Option<UIElement> {
        ELEMENTS.with(|m| m.borrow().get(&id).and_then(Weak::upgrade))
    }

    pub fn forget(id: NodeId) {
        ELEMENTS.with(|m| m.borrow_mut().remove(&id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header row (1, 2, 3) over two rows (4, 5, 6) and (7, 8).
    fn board() -> Grid {
        Grid {
            rows: vec![vec![1, 2, 3], vec![4, 5, 6], vec![7, 8]],
            headers: 1,
        }
    }

    #[test]
    fn a_grid_counts_its_rows_and_widest_row() {
        let g = board();
        assert_eq!((g.row_count(), g.column_count()), (3, 3));
        assert_eq!(g.at(1, 2), Some(6));
        assert_eq!(g.at(2, 2), None, "a short row");
        assert_eq!(g.at(3, 0), None);
        assert_eq!(Grid::default().column_count(), 0);
    }

    #[test]
    fn a_cell_has_its_columns_header_and_a_header_none() {
        let g = board();
        assert_eq!(g.column_headers(), [1, 2, 3]);
        assert_eq!(g.headers_of(1, 0), [1]);
        assert_eq!(g.headers_of(2, 1), [2]);
        assert_eq!(g.headers_of(0, 1), Vec::<NodeId>::new());
        // Two header rows: both name the column.
        let two = Grid {
            rows: vec![vec![1, 2], vec![3, 4], vec![5, 6]],
            headers: 2,
        };
        assert_eq!(two.headers_of(2, 1), [2, 4]);
        // No header row: no headers.
        let none = Grid {
            rows: vec![vec![1, 2]],
            headers: 0,
        };
        assert!(none.column_headers().is_empty() && none.headers_of(0, 0).is_empty());
    }

    #[test]
    fn cells_know_their_table_and_follow_its_changes() {
        set(100, board());
        assert_eq!(
            place(5),
            Some(Place {
                table: 100,
                row: 1,
                column: 1
            })
        );
        assert_eq!(with_grid(100, Grid::row_count), Some(3));
        assert_eq!(with_grid(5, Grid::row_count), None, "a cell is no table");
        // A row removed: its cells are no longer the table's.
        set(
            100,
            Grid {
                rows: vec![vec![1, 2, 3], vec![7, 8]],
                headers: 1,
            },
        );
        assert_eq!(place(5), None);
        assert_eq!(place(8).map(|p| (p.row, p.column)), Some((1, 1)));
        // A cell gone, then the table.
        forget(7);
        assert_eq!(place(7), None);
        forget(100);
        assert_eq!((place(8), with_grid(100, |_| ())), (None, None));
    }
}
