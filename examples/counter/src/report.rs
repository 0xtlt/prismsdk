//! The Report screen (Components ▸ Open the report): filters kept in the
//! URL, a "Saved" notice handed over by the previous screen, its own header
//! instead of the stack's, fitted scroll views, and a refresh that runs on
//! the background pool and stops when the screen is closed.
//!
//! - `/components/report?period=30d&room=2&room=5`: `Params::query`.
//! - Period buttons: `cx.replace_query(..)` (the URL changes, no new screen).
//! - "Compare with 90 days": `cx.navigate(..)` to another query (Back undoes).
//! - The Navigation card's "Save and open": `cx.navigate_with(.., Saved)`.

use std::time::Duration;

use prism::prelude::*;
use prism::ui::{self, Theme};
use prism::{Div, encode_query};

/// Handed to the report by the card's "Save and open" (`navigate_with`).
pub struct Saved(pub String);

/// How many times reports were refreshed: plain bookkeeping that no view
/// shows, so it is updated without re-rendering (`update_global_quiet`).
#[derive(Default)]
struct RefreshCount(u64);

const PERIODS: [&str; 3] = ["7d", "30d", "90d"];
const ROOMS: [&str; 8] = [
    "Lobby", "Studio", "Atelier", "Library", "Kitchen", "Garden", "Loft", "Cellar",
];

pub struct Report {
    period: String,
    rooms: Vec<usize>,
    notice: Option<String>,
    /// Visits per room for the period (loaded in the background).
    visits: Option<Vec<u32>>,
}

impl Report {
    pub fn new(p: &Params, cx: &mut Context<Self>) -> Self {
        let period = p
            .query("period")
            .filter(|v| PERIODS.contains(v))
            .unwrap_or("7d")
            .to_string();
        let rooms = p
            .query_all("room")
            .iter()
            .filter_map(|r| r.parse().ok())
            .filter(|r| *r < ROOMS.len())
            .collect();
        let mut report = Report {
            period,
            rooms,
            notice: p.state::<Saved>().map(|s| s.0.clone()),
            visits: None,
        };
        report.refresh(cx);
        report
    }

    /// Loads the numbers on the background pool. Tied to this screen: Back
    /// cancels it (and skips the work if it has not started).
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.visits = None;
        let days: u32 = self.period.trim_end_matches('d').parse().unwrap_or(7);
        cx.spawn(async move |this, cx| {
            let visits = cx
                .background(move || {
                    // Stands for a request to the server.
                    std::thread::sleep(Duration::from_millis(300));
                    (0..ROOMS.len() as u32)
                        .map(|i| days * (3 + i * 7 % 11))
                        .collect::<Vec<_>>()
                })
                .await;
            cx.update_entity(&this, |this, cx| {
                this.visits = Some(visits);
                cx.update_global_quiet(|n: &mut RefreshCount| n.0 += 1);
            });
        })
        .cancel_on_release();
    }

    /// The query this screen's state stands for.
    fn query(&self) -> Vec<(&'static str, String)> {
        let mut q = vec![("period", self.period.clone())];
        q.extend(self.rooms.iter().map(|r| ("room", r.to_string())));
        q
    }

    fn header(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        div()
            .flex_row()
            .items_center()
            .gap(12.)
            .px(16.)
            .py(12.)
            .bg(t.surface)
            .border(1.)
            .border_color(t.border)
            .child(ui::button("‹ Back").ghost().on_click(go_back()))
            .child(
                text("Report")
                    .text_size(20.)
                    .bold()
                    .text_color(t.text)
                    .flex_1()
                    .accessibility_role(Role::Heading(1)),
            )
            .child(ui::badge(self.period.clone()))
    }

    fn periods(&self, cx: &mut Context<Self>) -> Div {
        div().flex_row().gap(8.).children(PERIODS.iter().map(|p| {
            let b = ui::button(*p).on_click(cx.listener(move |this, _, cx| {
                if this.period != *p {
                    this.period = p.to_string();
                    // Same screen, new URL: a reload or a shared link opens it.
                    cx.replace_query(this.query());
                    this.refresh(cx);
                }
            }));
            if self.period == *p { b } else { b.secondary() }
        }))
    }

    /// Rooms to compare: a fitted list (as tall as its rows, up to 150 pt).
    fn rooms(&self, cx: &mut Context<Self>) -> ui::Card {
        let t = Theme::of(cx);
        ui::card()
            .child(ui::heading("Rooms"))
            .child(
                scroll()
                    .fit()
                    .max_h(px(150.))
                    .rounded(8.)
                    .bg(t.surface_alt)
                    .children(ROOMS.iter().enumerate().map(|(i, name)| {
                        let on = self.rooms.contains(&i);
                        ui::list_row(*name)
                            .detail(if on { "✓" } else { "" })
                            .on_click(cx.listener(move |this, _, cx| {
                                if let Some(at) = this.rooms.iter().position(|r| *r == i) {
                                    this.rooms.remove(at);
                                } else {
                                    this.rooms.push(i);
                                }
                                cx.replace_query(this.query());
                            }))
                    })),
            )
            .child(
                text("Selected rooms (fits its two lines):")
                    .text_size(13.)
                    .text_color(t.muted),
            )
            .child(
                scroll()
                    .fit()
                    .max_h(px(150.))
                    .child(text(if self.rooms.is_empty() {
                        "None yet: tap rooms above.".to_string()
                    } else {
                        self.rooms
                            .iter()
                            .map(|r| ROOMS[*r])
                            .collect::<Vec<_>>()
                            .join(", ")
                    }))
                    .child(
                        text(format!(
                            "URL: /components/report?{}",
                            encode_query(self.query())
                        ))
                        .text_size(12.)
                        .text_color(t.muted),
                    ),
            )
    }
}

impl Render for Report {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let notice = self.notice.clone().map(|n| {
            div()
                .p(12.)
                .rounded(10.)
                .bg(t.success)
                .child(text(n).text_color(rgb(0xFFFFFF)).bold())
                .accessibility_role(Role::Status)
        });
        let visits = match &self.visits {
            None => div().items_center().p(20.).child(spinner()),
            Some(v) => div().gap(4.).children(
                ROOMS
                    .iter()
                    .zip(v)
                    .map(|(name, n)| ui::list_row(*name).detail(cx.format_number(*n as f64, 0))),
            ),
        };
        // A bare screen: no stack header (this one draws its own). The
        // header stays put; only the body scrolls (`scroll().flex_1()`).
        screen(format!("Report · {}", self.period))
            .bare()
            .bg(t.background)
            .child(self.header(cx))
            .child(
                scroll()
                    .flex_1()
                    .p(20.)
                    .gap(16.)
                    .when_some(notice, |s, n| s.child(n))
                    .child(self.periods(cx))
                    .child(
                        ui::button("Compare with 90 days")
                            .secondary()
                            // Another query is another screen: Back returns here.
                            .on_click(navigate_to("/components/report?period=90d")),
                    )
                    .child(self.rooms(cx))
                    .child(
                        ui::card()
                            .child(ui::heading(format!("Visits, last {}", self.period)))
                            .child(visits),
                    ),
            )
    }
}

/// The Navigation card of the Components tab: opens the report.
pub fn card() -> ui::Card {
    ui::card()
        .child(ui::heading("Navigation"))
        .child(text("Filters live in the URL; the report has its own header.").text_size(13.))
        .child(
            div()
                .flex_row()
                .flex_wrap()
                .gap(8.)
                .child(
                    ui::button("Open the report")
                        .on_click(navigate_to("/components/report?period=30d&room=1")),
                )
                .child(
                    ui::button("Save and open")
                        .secondary()
                        .on_click(std::rc::Rc::new(|_: &ClickEvent, cx: &mut App| {
                            cx.navigate_with(
                                "/components/report?period=7d",
                                Saved("Saved ✓ — shown once, handed over by navigate_with".into()),
                            )
                        })),
                ),
        )
}
