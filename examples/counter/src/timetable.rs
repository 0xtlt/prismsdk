//! The Timetable card of the Components tab (#284): `table()` with column
//! headers and rows of cells. The browser's table layout on the web, the
//! same algorithm on native backends: each column as wide as its cells
//! ask, the extra room shared by content, and when the window is narrow
//! the destinations wrap while the times never do. The line is a badge
//! (any element goes in a cell), the minutes are right-aligned, digits are
//! tabular, and thin separators run between the rows.

use prism::Table;
use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

/// Departures: time, line and its color, destination (a key), minutes.
const DEPARTURES: [(&str, &str, u32, usize, u32); 5] = [
    ("09:35", "12", 0x0a84ff, 0, 3),
    ("09:41", "4", 0x30a46c, 1, 9),
    ("09:48", "N7", 0xe5484d, 2, 16),
    ("10:02", "12", 0x0a84ff, 3, 30),
    ("10:15", "31", 0x8e4ec6, 4, 43),
];

fn destination(cx: &App, i: usize) -> String {
    match i {
        0 => strings::table_harbour(cx),
        1 => strings::table_old_town(cx),
        2 => strings::table_airport(cx),
        3 => strings::table_riverside(cx),
        _ => strings::table_north(cx),
    }
}

/// The table alone.
pub fn timetable(cx: &App) -> Table {
    let t = Theme::of(cx);
    let badge = |line: &str, color: u32| {
        div().bg(rgb(color)).rounded(6.).px(6.).py(2.).child(
            text(line)
                .text_size(13.)
                .semibold()
                .text_color(rgb(0xffffff)),
        )
    };
    table()
        .column(strings::table_time(cx))
        .column(column(strings::table_line(cx)).align_center())
        .column(column(strings::table_destination(cx)).min_width(96.))
        .column(column(strings::table_minutes(cx)).align_end())
        .rows(DEPARTURES.iter().map(|&(time, line, color, to, minutes)| {
            row()
                .child(text(time).semibold())
                .child(cell(badge(line, color)))
                .child(text(destination(cx, to)))
                .child(text(minutes.to_string()).text_color(t.muted))
        }))
        .separators()
        .accessibility_label(strings::table_title(cx))
}

pub fn card(cx: &App) -> ui::Card {
    let t = Theme::of(cx);
    ui::card()
        .child(ui::heading(strings::table_title(cx)))
        .child(
            text(strings::table_note(cx))
                .text_size(13.)
                .text_color(t.muted),
        )
        .child(timetable(cx))
        .child(
            ui::list_row(strings::table_open(cx))
                .leading("▦")
                .on_click(navigate_to("/components/table")),
        )
}

/// `/components/table`: the timetable on a screen of its own.
pub struct TimetableScreen;

impl Render for TimetableScreen {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (screen, body) = crate::page(strings::table_title(cx), cx);
        screen.child(body.child(ui::card().child(timetable(cx))))
    }
}
