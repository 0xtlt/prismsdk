//! `/components/islands`: parts of the page that run in the browser.
//!
//! - `island("chart")`: the WASM island in `islands/chart` (a canvas chart
//!   whose crosshair follows the pointer with no round trip). A click emits
//!   `select`; "Add a point" pushes new points as a prop, live.
//! - `web_component("time-ago")`: `assets/time-ago.js`, a plain Web
//!   Component ticking client-side from a time the server gives it.
//!
//! Native apps (and pages without JavaScript) show the fallbacks: the chart
//! drawn by Prism, the time as text.

use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

pub(crate) struct IslandsDemo {
    points: Vec<f64>,
    /// The point clicked in the chart: (index, value).
    selected: Option<(usize, f64)>,
    /// When the last point was added (ms since 1970).
    added: f64,
    seed: u64,
}

fn now_ms() -> f64 {
    prism::unix_seconds(std::time::SystemTime::now()) * 1000.
}

impl IslandsDemo {
    pub fn new(_: &mut Context<Self>) -> Self {
        IslandsDemo {
            points: vec![12., 18., 15., 24., 21., 30., 26., 34., 31., 38.],
            selected: None,
            added: now_ms(),
            seed: 0x2545_f491,
        }
    }

    /// The next value: a small random step from the last one.
    fn add_point(&mut self) {
        self.seed = self
            .seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let step = ((self.seed >> 33) % 17) as f64 - 7.;
        let last = self.points.last().copied().unwrap_or(20.);
        self.points.push((last + step).clamp(2., 60.));
        if self.points.len() > 20 {
            self.points.remove(0);
            self.selected = self
                .selected
                .and_then(|(i, v)| i.checked_sub(1).map(|i| (i, v)));
        }
        self.added = now_ms();
    }
}

impl Render for IslandsDemo {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let (screen, body) = crate::page(strings::islands_title(cx), cx);
        let selected = match self.selected {
            Some((i, v)) => strings::islands_selected(cx, i + 1, cx.format_number(v, 1)),
            None => strings::islands_none(cx),
        };
        let chart = island("chart")
            .prop("points", &self.points)
            .prop("color", &t.accent.hex())
            .prop("selected", &self.selected.map(|(i, _)| i))
            .on(
                "select",
                cx.listener(|this, e: &IslandEvent, _| {
                    if let Some((i, v)) = e.detail::<(usize, f64)>() {
                        this.selected = Some((i, v));
                    }
                }),
            )
            .w_full()
            .h(px(220.))
            .gap(4.)
            .accessibility_label(strings::islands_chart_label(cx, self.points.len()))
            // The fallback: the first paint, no JavaScript, native apps.
            .child(
                ui::line_chart()
                    .series(strings::islands_title(cx), self.points.iter().copied())
                    .area()
                    .points()
                    .height(180.),
            )
            .child(
                text(strings::islands_summary(
                    cx,
                    self.points.len(),
                    cx.format_number(self.points.last().copied().unwrap_or(0.), 1),
                ))
                .text_size(13.)
                .text_color(t.muted),
            );
        screen.child(
            body.child(
                ui::card()
                    .child(ui::heading(strings::islands_chart_heading(cx)))
                    .child(text(strings::islands_chart_intro(cx)).text_color(t.muted))
                    .child(chart)
                    .child(
                        text(selected)
                            .id("island-selection")
                            .accessibility_role(Role::Status),
                    )
                    .child(
                        div()
                            .flex_row()
                            .flex_wrap()
                            .gap(8.)
                            .child(
                                ui::button(strings::islands_add(cx))
                                    .on_click(cx.listener(|this, _, _| this.add_point())),
                            )
                            .child(
                                ui::button(strings::islands_clear(cx))
                                    .secondary()
                                    .disabled(self.selected.is_none())
                                    .on_click(cx.listener(|this, _, _| this.selected = None)),
                            ),
                    )
                    .child(
                        text(strings::islands_native(cx))
                            .text_size(13.)
                            .text_color(t.muted),
                    ),
            )
            .child(
                ui::card()
                    .child(ui::heading(strings::islands_component_heading(cx)))
                    .child(text(strings::islands_component_intro(cx)).text_color(t.muted))
                    .child(
                        div()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(6.)
                            .child(text(strings::islands_added(cx)))
                            .child(
                                web_component("time-ago")
                                    .module("time-ago.js")
                                    .prop("datetime", &self.added)
                                    .text_color(t.accent)
                                    .child(text(strings::islands_just_now(cx))),
                            ),
                    ),
            ),
        )
    }
}
