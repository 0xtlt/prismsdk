//! The "Departures" widget (#331): its data comes over HTTP, and it stays
//! current without the app. On each of the system's timeline reloads (every
//! `COUNTER_DEPARTURES_RELOAD` seconds, 15 minutes by default) its refresh
//! hook fetches the board in the widget extension, then the widget is drawn
//! for now and ahead for each departure. A failed fetch leaves the last
//! board on screen. The board is kept in the app's storage, which the
//! extension shares: the app shows what the widget last fetched.
//!
//! Only built with a feed, so the default bundle keeps its Swift-only
//! widget extension:
//!
//! ```sh
//! COUNTER_DEPARTURES_URL=http://127.0.0.1:8765/board prism run ios
//! ```
//!
//! The feed is plain text, one departure per line: `<line> <unix time>
//! <destination>` (`12 1764151200 Harbour`).

use std::time::{Duration, SystemTime};

use prism::http::{Fetch, Request};
use prism::prelude::*;
use prism::ui::Theme;

/// The feed's URL, set when the app is built.
fn feed() -> Option<&'static str> {
    option_env!("COUNTER_DEPARTURES_URL").filter(|u| !u.is_empty())
}

/// The board, as last fetched.
#[derive(Default)]
struct Board {
    departures: Vec<(String, SystemTime, String)>,
    fetched: Option<SystemTime>,
}

/// Where the board is kept: the time it was fetched, then the feed.
const KEY: &str = "departures";

impl Board {
    fn load(saved: &str) -> Board {
        let (at, feed) = saved.split_once('\n').unwrap_or((saved, ""));
        Board {
            departures: parse(feed),
            fetched: at
                .parse::<u64>()
                .ok()
                .map(|at| SystemTime::UNIX_EPOCH + Duration::from_secs(at)),
        }
    }
}

pub fn register(cx: &mut App) {
    let Some(url) = feed() else { return };
    let saved = cx.storage().get(KEY).unwrap_or_default();
    let board = cx.new_entity(|_| Board::load(&saved));
    let reload = option_env!("COUNTER_DEPARTURES_RELOAD")
        .and_then(|s| s.parse().ok())
        .map_or(Widget::DEFAULT_RELOAD, Duration::from_secs);
    cx.register_widget(
        Widget::new("departures", "Departures")
            .description("The next departures, fetched without the app.")
            .families(&[WidgetFamily::Small, WidgetFamily::Medium])
            .link("/")
            .render(move |cx, family| {
                let t = Theme::of(cx);
                let b = board.read(cx);
                let now = cx.now();
                let rows = if family == WidgetFamily::Small { 2 } else { 4 };
                let next: Vec<_> = b
                    .departures
                    .iter()
                    .filter(|(_, at, _)| *at > now)
                    .take(rows)
                    .cloned()
                    .collect();
                let updated = b.fetched.map_or_else(
                    || "Not fetched yet".to_string(),
                    |at| format!("Fetched {}", cx.format_time(unix(at))),
                );
                div()
                    .gap(4.)
                    .flex_1()
                    .child(text("Departures").semibold().text_color(t.accent))
                    .children(next.into_iter().map(|(line, at, to)| {
                        div()
                            .flex_row()
                            .gap(6.)
                            .child(text(line).bold())
                            .child(text(to).flex_1())
                            .child(text(cx.format_time(unix(at))).text_color(t.muted))
                    }))
                    .child(text(updated).text_size(11.).text_color(t.muted))
            })
            .ahead(move |cx, _| {
                board
                    .read(cx)
                    .departures
                    .iter()
                    .map(|(_, at, _)| *at)
                    .collect::<Vec<_>>()
            })
            .refresh(move |cx| async move {
                let feed = fetch(&cx, url).await?;
                let saved = format!("{}\n{feed}", unix(SystemTime::now()) as u64);
                cx.update(|app| app.storage().set(KEY, &saved));
                cx.update_entity(&board, |b, _| *b = Board::load(&saved));
                Ok::<_, String>(())
            })
            .reload_after(reload),
    );
}

fn unix(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0., |d| d.as_secs_f64())
}

/// The feed.
async fn fetch(cx: &AsyncApp, url: &str) -> Result<String, String> {
    let res = cx
        .fetch(Request::get(url).timeout(Duration::from_secs(5)))
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("HTTP {}", res.status));
    }
    Ok(res.text())
}

/// The departures of a feed.
fn parse(feed: &str) -> Vec<(String, SystemTime, String)> {
    feed.lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, ' ');
            let line = parts.next()?.to_string();
            let at = parts.next()?.parse::<u64>().ok()?;
            let to = parts.next()?.trim().to_string();
            Some((line, SystemTime::UNIX_EPOCH + Duration::from_secs(at), to))
        })
        .collect()
}
