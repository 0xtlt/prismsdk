# Tables

`table()` shows data in columns with headers, then rows of cells (#284). A
cell holds any element: text, a badge, an icon, a button. Columns are as wide
as their content asks, like a browser's table, and screen readers get a real
table: they read a cell with its column's header and move by row and column.

```rust
use prism::prelude::*;

table()
    .column("Time")
    .column(column("Line").align_center())
    .column(column("Destination").min_width(96.))
    .column(column("Min").align_end())
    .rows(departures.iter().map(|d| {
        row()
            .key(d.id.to_string())
            .child(text(d.time.clone()).semibold())
            .child(cell(line_badge(&d.line)))
            .child(d.destination.clone())
            .child(d.minutes.to_string())
    }))
    .separators()
    .accessibility_label("Departures")
```

The counter example shows this as a timetable in its Components tab, and on a
screen of its own at `/components/table`.

## Columns

- `column(header)`: a column headed by a string or any element (an icon and a
  label). A string alone is a column too: `.column("Time")`,
  `.columns(["Time", "Line"])`.
- `.align_end()`, `.align_center()`, `.align(TextAlign)`: the alignment of
  the column's cells and header. Align numbers with `align_end()`: the
  trailing edge, so left in right-to-left languages.
- `.width(w)`: a column `w` points wide, cell padding included. It still
  grows to fit content that cannot get narrower (a long word, a picture).
- `.min_width(w)`: at least `w` points, wider when its content asks and
  there is room.

## Rows and cells

- `row()` makes a row; each `.child(..)` is a cell. An absent child
  (`.child(None::<Text>)`) is an empty cell, so the next ones stay in their
  columns. `.key(..)` keeps a row's views when rows are reordered.
- `cell(content)` is a cell with its own style: a background, its own
  padding, an accessibility label (`cell(text("Cancelled")).bg(..)`).
- A row can take `.on_click(..)` and `.context_menu(..)`. Keyboard and
  screen reader users reach a row through its cells: put a link or a button
  in a cell as well when the action matters. On Apple platforms, VoiceOver's
  activation of a cell without a control clicks its row, and on tvOS the
  remote focuses such a cell (Select clicks the row).

## The table

- `.separators()`: a thin line between rows and under the headers, in a
  gray that suits light and dark. `.separator_color(c)` picks the color.
- `.cell_padding(vertical, horizontal)`: every cell's padding (6 and 10
  points by default). A cell given its own padding keeps it.
- Digits are tabular by default (`tabular_nums`), so times and amounts line
  up from row to row. `.proportional_digits()` turns that off.
- Headers are semibold, like a browser's `<th>`, unless they set a weight.
- A table is styled like any element: `.bg(..)`, `.rounded(..)`,
  `.border(..)`, `.w_full()`.

## Column widths

On the web the table is a real `<table>` with `<thead>`, `<th scope="col">`
and `<tbody>`, sized by the browser. Native backends size the columns with
the same algorithm, the CSS automatic table layout (`table-layout: auto`),
written once in `prism_native::table`:

1. Each column's **min-content** width (its widest unbreakable content:
   `09:35`, a long word) and **max-content** width (its content on one line)
   are measured from all its cells, padding included.
2. With more room than the max-content widths, the extra goes to the columns
   without a width, in proportion to their max-content width.
3. With less room, each column gets its min-content width plus a share of
   the rest in proportion to how much it can shrink. Text that can wrap
   wraps; `09:35` never does.
4. Below the min-content widths, the table overflows (put it in a horizontal
   `scroll()` if it can get that narrow).

The algorithm is a pure function, `prism_native::table::column_widths`, with
unit tests for equal content, one long column, a narrow table, fixed and
minimum widths and empty cells.

## Screen readers

| Platform | What the reader gets |
|---|---|
| Web | `<table>`, `<th scope="col">`, `<td>`: the browser's own table semantics |
| macOS | `NSAccessibilityTableRole`, rows, columns and cells; each cell's row, column and column header (`AXColumnHeaderUIElements`), read by VoiceOver. A cell's controls (buttons, switches, links, text fields, pickers) are its children, reached by interacting with the cell; a cell holding one reads what it holds (`Track, button`), and pressing it presses its control (a picker opens its menu). An icon is no control: a cell of an icon and a text reads as text, and pressing it clicks a clickable row |
| iOS, iPadOS, visionOS | `UIAccessibilityContainerDataTable`: VoiceOver reads the column header when the focus moves to another column, and the row and column counts. A cell of text is one element (`Time, 09:35`); each control in a cell (a button, a switch, a link, a text field, a picker) is an element of its own, in the cell's row and column, and activating a picker opens its menu. The table answers an element for every cell, never a container, so VoiceOver's table navigation always lands on something it reads |
| tvOS | The same data table. The remote focuses each control in a cell (a picker too), and the cells of a clickable row (a row whose cells hold a control hands the focus to them) |
| watchOS | No data table (see below). The table is a container of its cells, row by row: header cells have the header trait, a cell of text is one element, each control in a cell is an element of its own, and each element gives its column header in VoiceOver's More Content rotor |
| Android | `CollectionInfo` (rows, columns) on the table, `CollectionItemInfo` on each cell (row, column, heading for headers, and the column's title from API 33) |
| Linux (GTK 4) | Roles `TABLE`, `ROW`, `COLUMN_HEADER` and `CELL` with their row and column indexes; each cell is described by its column's header |
| Windows | UI Automation `Table` with the Grid and Table patterns; cells are `DataItem`s (headers `HeaderItem`s) with the GridItem and TableItem patterns, which give their row, column and column header |

What is not possible everywhere:

- Android: a cell is one element, read from its text (`Time, 09:35`).
  A control inside a cell is not reached on its own: put actions on the row
  or next to the table.
- Linux: GTK 4's AT-SPI bridge doesn't implement the `Table` and `TableCell`
  interfaces for app widgets, which Orca asks for a cell's column header. The
  closest thing: each cell is described by its column's header
  (`DESCRIBED_BY`), which Orca reads after the cell. GTK 4.8 also shows
  `COLUMN_HEADER` as a filler (later versions as a column header).
- watchOS: `UIAccessibilityContainerDataTable`, `UIAccessibilityContainerDataTableCell`
  and `accessibilityContainerType` are `API_UNAVAILABLE(watchos)`
  (`UIAccessibilityContainer.h`), and the watch app's host is SwiftUI, which
  has no row or column API (its `Table` is unavailable on watchOS). VoiceOver
  reads no row or column number there; the column header is in the More
  Content rotor (`accessibilityCustomContent`), not read on its own.

## Backends

| | Web | macOS | iOS · visionOS | tvOS | watchOS | Linux | Android | Windows |
|---|---|---|---|---|---|---|---|---|
| Table layout | CSS | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Table semantics | ✓ | ✓ | ✓ | ✓ | Headers, column header as custom content | ✓ | ✓ | ✓ |
