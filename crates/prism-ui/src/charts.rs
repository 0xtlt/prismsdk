//! Charts: line, bar, pie/donut, sparkline, ring and stat tiles, drawn with
//! native vector shapes ([`prism_core::shape`]) and themed by [`Theme`].
//!
//! ```ignore
//! ui::line_chart()
//!     .series("Revenue", [12., 18., 15., 24., 31.])
//!     .labels(["Mon", "Tue", "Wed", "Thu", "Fri"])
//!     .smooth()
//!     .area();
//! ui::bar_chart().bar("Mon", 3.).bar("Tue", 5.).on_select(cx.listener(|this, e, _| this.day = e.index()));
//! ui::pie_chart().slice("Rent", 900.).slice("Food", 400.).donut("1 300 €");
//! ui::ring(0.72).label("72%").caption("Goal");
//! ui::stat("Visitors", "12 400").delta("+12%", true).trend([3., 5., 4., 8.]);
//! ```
//!
//! Every chart reads as one element to screen readers, with its values
//! (bars are separate, tappable elements). Bars animate when values change.

use std::rc::Rc;

use prism_core::shape::nice_ticks;
use prism_core::{
    Accessible, App, ChangeEvent, Color, Drawing, Element, FluentBuilder, Handler, IntoElement,
    ParentElement, Path, Role, Styled, adaptive, div, px, relative, rgb, shape, text,
};

use crate::{Component, Refine, Theme};

/// Height of an axis label line (labels sit on the grid lines).
const LABEL_H: f32 = 14.;
const LABEL_SIZE: f32 = 11.;

const LIGHT: [u32; 6] = [0x5B5BD6, 0x2E9E5B, 0xE5732E, 0xD6409F, 0x1FA2B8, 0xC9A227];
const DARK: [u32; 6] = [0x8B8BFF, 0x4CD983, 0xFF9B5C, 0xFF6FC4, 0x4FD3E8, 0xF2CB4E];

impl Theme {
    /// The `i`-th chart color: the accent first, then distinct hues that
    /// follow the theme's light/dark mode.
    pub fn chart_color(&self, i: usize) -> Color {
        if i == 0 {
            return self.accent;
        }
        let (l, d) = (rgb(LIGHT[i % 6]), rgb(DARK[i % 6]));
        if self.follows_system {
            adaptive(l, d)
        } else if self.dark {
            d
        } else {
            l
        }
    }
}

/// `c` with its opacity (both variants) multiplied by `a`.
fn faded(c: Color, a: f32) -> Color {
    Color {
        a: c.a * a,
        dark: c.dark.map(|[r, g, b, da]| [r, g, b, da * a]),
        ..c
    }
}

/// A dot of `width` (a zero-length round-capped line: no fixed radius in
/// fractions needed).
fn dot(x: f32, y: f32, color: Color, width: f32) -> Path {
    Path::line([(x, y), (x, y)]).stroke(color, width).round()
}

/// A value axis: round ticks covering the data.
struct Axis {
    lo: f64,
    hi: f64,
    ticks: Vec<f64>,
    decimals: usize,
}

impl Axis {
    fn new(min: f64, max: f64) -> Axis {
        let ticks = nice_ticks(min, max, 4);
        let step = (ticks[1] - ticks[0]).abs();
        let mut decimals = 0;
        while decimals < 6 {
            let scaled = step * 10f64.powi(decimals as i32);
            if (scaled.round() - scaled).abs() < 1e-6 {
                break;
            }
            decimals += 1;
        }
        Axis {
            lo: ticks[0],
            hi: *ticks.last().expect("two ticks"),
            ticks,
            decimals,
        }
    }

    /// From the top: 0 at `hi`, 1 at `lo`.
    fn y(&self, v: f64) -> f32 {
        if self.hi == self.lo {
            return 0.5;
        }
        (1. - (v - self.lo) / (self.hi - self.lo)) as f32
    }

    fn grid(&self, t: &Theme) -> Vec<Path> {
        self.ticks
            .iter()
            .map(|v| {
                let y = self.y(*v);
                Path::line([(0., y), (1., y)]).stroke(t.border, 1.)
            })
            .collect()
    }

    /// Tick labels, top to bottom, spaced like the grid lines.
    fn labels(&self, cx: &App, t: &Theme, unit: &str, height: f32) -> Element {
        div()
            .h(px(height))
            .justify_between()
            .items_end()
            .flex_shrink_0()
            .accessibility_hidden()
            .children(self.ticks.iter().rev().map(|v| {
                text(format!("{}{unit}", cx.format_number(*v, self.decimals)))
                    .text_size(LABEL_SIZE)
                    .text_color(t.muted)
            }))
            .into_element()
    }
}

/// Category labels under a plot, one cell per point (some skipped when
/// they would not fit).
fn x_labels(labels: &[String], t: &Theme, gap: f32) -> Element {
    let stride = (labels.len() as f32 / 7.).ceil().max(1.) as usize;
    div()
        .flex_row()
        .gap(gap)
        .accessibility_hidden()
        .children(labels.iter().enumerate().map(|(i, l)| {
            div().flex_1().items_center().child(
                text(if i % stride == 0 { l.as_str() } else { "" })
                    .text_size(LABEL_SIZE)
                    .text_color(t.muted),
            )
        }))
        .into_element()
}

/// Color dots and names, for charts with several series or slices.
fn legend(items: &[(String, Color)], t: &Theme) -> Element {
    div()
        .flex_row()
        .flex_wrap()
        .gap(12.)
        .accessibility_hidden()
        .children(items.iter().map(|(name, c)| {
            div()
                .flex_row()
                .items_center()
                .gap(6.)
                .child(div().size(px(8.)).rounded(4.).bg(*c))
                .child(text(name.clone()).text_size(12.).text_color(t.muted))
        }))
        .into_element()
}

/// What a screen reader says: the title and the values.
fn summary(cx: &App, title: &str, values: &[f64], unit: &str) -> String {
    let decimals = if values.iter().all(|v| v.fract() == 0.) {
        0
    } else {
        1
    };
    let f = |v: f64| format!("{}{unit}", cx.format_number(v, decimals));
    let body = if values.len() <= 12 {
        values.iter().map(|v| f(*v)).collect::<Vec<_>>().join(", ")
    } else {
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        format!("{} – {}", f(min), f(max))
    };
    if title.is_empty() {
        body
    } else {
        format!("{title}: {body}")
    }
}

// --- Line chart ----------------------------------------------------------------

/// One line of a [`LineChart`].
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
    /// Default: the theme's chart colors, in order.
    pub color: Option<Color>,
}

/// Lines over a value axis and category labels.
pub struct LineChart {
    pub series: Vec<Series>,
    pub labels: Vec<String>,
    pub smooth: bool,
    pub area: bool,
    pub points: bool,
    pub y_axis: bool,
    pub height: f32,
    /// Forced value range (default: the data, rounded to nice ticks).
    pub range: Option<(f64, f64)>,
    /// Appended to axis labels and spoken values (`" €"`, `"%"`).
    pub unit: String,
    pub title: String,
    refine: Refine,
}

/// A line chart. Add data with [`LineChart::series`].
pub fn line_chart() -> LineChart {
    LineChart {
        series: Vec::new(),
        labels: Vec::new(),
        smooth: false,
        area: false,
        points: false,
        y_axis: true,
        height: 180.,
        range: None,
        unit: String::new(),
        title: String::new(),
        refine: Refine::default(),
    }
}

impl LineChart {
    pub fn series(
        mut self,
        name: impl Into<String>,
        values: impl IntoIterator<Item = f64>,
    ) -> Self {
        self.series.push(Series {
            name: name.into(),
            values: values.into_iter().collect(),
            color: None,
        });
        self
    }
    /// Colors the last added series.
    pub fn color(mut self, c: Color) -> Self {
        if let Some(s) = self.series.last_mut() {
            s.color = Some(c);
        }
        self
    }
    pub fn labels<S: Into<String>>(mut self, labels: impl IntoIterator<Item = S>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }
    /// Curves instead of straight segments (never overshooting the data).
    pub fn smooth(mut self) -> Self {
        self.smooth = true;
        self
    }
    /// Fills under each line.
    pub fn area(mut self) -> Self {
        self.area = true;
        self
    }
    /// Marks each value with a dot.
    pub fn points(mut self) -> Self {
        self.points = true;
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some((min, max));
        self
    }
    pub fn hide_y_axis(mut self) -> Self {
        self.y_axis = false;
        self
    }
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }
    /// Spoken before the values.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
}

refinable!(LineChart);

impl Component for LineChart {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let all: Vec<f64> = self.series.iter().flat_map(|s| s.values.clone()).collect();
        let (min, max) = self.range.unwrap_or_else(|| {
            let min = all.iter().copied().fold(f64::INFINITY, f64::min);
            let max = all.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if all.is_empty() { (0., 1.) } else { (min, max) }
        });
        let axis = Axis::new(min, max);
        let n = self
            .series
            .iter()
            .map(|s| s.values.len())
            .max()
            .unwrap_or(0);
        let labelled = !self.labels.is_empty();
        let x = |i: usize| {
            if labelled {
                (i as f32 + 0.5) / n.max(1) as f32
            } else if n <= 1 {
                0.5
            } else {
                i as f32 / (n - 1) as f32
            }
        };
        let mut paths = if self.y_axis {
            axis.grid(&t)
        } else {
            Vec::new()
        };
        for (i, s) in self.series.iter().enumerate() {
            let color = s.color.unwrap_or_else(|| t.chart_color(i));
            let pts: Vec<(f32, f32)> = s
                .values
                .iter()
                .enumerate()
                .map(|(j, v)| (x(j), axis.y(*v)))
                .collect();
            let line = if self.smooth {
                Path::smooth(pts.clone())
            } else {
                Path::line(pts.clone())
            };
            if let (true, Some(first), Some(last)) = (self.area, pts.first(), pts.last()) {
                paths.push(
                    line.clone()
                        .line_to(last.0, 1.)
                        .line_to(first.0, 1.)
                        .close()
                        .fill(faded(color, 0.16)),
                );
            }
            paths.push(line.stroke(color, 2.5).round());
            if self.points {
                for (px_, py_) in &pts {
                    paths.push(dot(*px_, *py_, color, 8.));
                    paths.push(dot(*px_, *py_, t.surface, 3.5));
                }
            }
        }
        let spoken = self
            .series
            .iter()
            .map(|s| summary(cx, &s.name, &s.values, &self.unit))
            .collect::<Vec<_>>()
            .join(". ");
        let plot = shape()
            .inset(if labelled { 0. } else { 5. }, LABEL_H / 2.)
            .paths(paths)
            .h(px(self.height))
            .w_full()
            .label(if self.title.is_empty() {
                spoken
            } else {
                format!("{}. {spoken}", self.title)
            });
        let legend_items: Vec<(String, Color)> = self
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.clone(), s.color.unwrap_or_else(|| t.chart_color(i))))
            .collect();
        let el = div()
            .gap(8.)
            .when(self.series.len() > 1, |d| {
                d.child(legend(&legend_items, &t))
            })
            .child(
                div()
                    .flex_row()
                    .items_start()
                    .gap(8.)
                    .when(self.y_axis, |d| {
                        d.child(axis.labels(cx, &t, &self.unit, self.height))
                    })
                    .child(
                        div()
                            .flex_1()
                            .gap(4.)
                            .child(plot)
                            .when(labelled, |d| d.child(x_labels(&self.labels, &t, 0.))),
                    ),
            );
        self.refine.apply(el).into_element()
    }
}

component!(LineChart);

// --- Bar chart -----------------------------------------------------------------

/// Vertical bars, each a native element: tappable, spoken on its own, and
/// animated when its value changes.
pub struct BarChart {
    /// Label, value, color (default: the chart's color).
    pub bars: Vec<(String, f64, Option<Color>)>,
    pub color: Option<Color>,
    pub height: f32,
    pub y_axis: bool,
    pub range: Option<(f64, f64)>,
    /// The other bars fade.
    pub highlight: Option<usize>,
    pub unit: String,
    pub on_select: Option<Handler<ChangeEvent>>,
    refine: Refine,
}

/// A bar chart. Add bars with [`BarChart::bar`]. Values below zero draw as
/// zero.
pub fn bar_chart() -> BarChart {
    BarChart {
        bars: Vec::new(),
        color: None,
        height: 180.,
        y_axis: true,
        range: None,
        highlight: None,
        unit: String::new(),
        on_select: None,
        refine: Refine::default(),
    }
}

impl BarChart {
    pub fn bar(mut self, label: impl Into<String>, value: f64) -> Self {
        self.bars.push((label.into(), value, None));
        self
    }
    pub fn bars<S: Into<String>>(self, bars: impl IntoIterator<Item = (S, f64)>) -> Self {
        bars.into_iter().fold(self, |c, (l, v)| c.bar(l, v))
    }
    /// One bar in its own color.
    pub fn bar_colored(mut self, label: impl Into<String>, value: f64, c: Color) -> Self {
        self.bars.push((label.into(), value, Some(c)));
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some((min, max));
        self
    }
    pub fn hide_y_axis(mut self) -> Self {
        self.y_axis = false;
        self
    }
    pub fn highlight(mut self, index: Option<usize>) -> Self {
        self.highlight = index;
        self
    }
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }
    /// Called with the tapped bar's index (`e.index()`).
    pub fn on_select(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_select = Some(handler);
        self
    }
}

refinable!(BarChart);

impl Component for BarChart {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let max = self.bars.iter().map(|b| b.1).fold(0., f64::max);
        let (lo, hi) = self.range.unwrap_or((0., if max > 0. { max } else { 1. }));
        let axis = Axis::new(lo.min(0.), hi);
        let (h, gap) = (self.height, 6.);
        let color = self.color.unwrap_or(t.accent);
        let decimals = if self.bars.iter().all(|b| b.1.fract() == 0.) {
            0
        } else {
            1
        };
        if cx.in_snapshot() {
            return self.render_static(cx, &t, &axis, color, decimals);
        }
        let bars = self.bars.iter().enumerate().map(|(i, (label, v, c))| {
            let fraction = ((v.max(0.) - axis.lo) / (axis.hi - axis.lo)).clamp(0., 1.) as f32;
            let faded = self.highlight.is_some_and(|hl| hl != i);
            let spoken = format!("{label}: {}{}", cx.format_number(*v, decimals), self.unit);
            div()
                .flex_1()
                .h_full()
                .justify_end()
                .items_center()
                .accessibility_label(spoken)
                .accessibility_role(if self.on_select.is_some() {
                    Role::Button
                } else {
                    Role::Text
                })
                .when_some(self.on_select.clone(), |d, h| {
                    d.on_click(Rc::new(move |_, cx: &mut App| {
                        h(&ChangeEvent { value: i as f64 }, cx)
                    }))
                })
                .child(
                    div()
                        .w(relative(0.72))
                        .max_w(px(44.))
                        .h(relative(fraction))
                        .min_h(px(if *v > 0. { 2. } else { 0. }))
                        .rounded(4.)
                        .bg(c.unwrap_or(color))
                        .opacity(if faded { 0.35 } else { 1. })
                        .spring(450),
                )
        });
        let labels: Vec<String> = self.bars.iter().map(|b| b.0.clone()).collect();
        let plot = div()
            .flex_1()
            .gap(4.)
            .child(
                // The grid, with the bars over it (pulled up by a negative margin).
                div()
                    .child(
                        shape()
                            .inset(0., LABEL_H / 2.)
                            .paths(if self.y_axis {
                                axis.grid(&t)
                            } else {
                                Vec::new()
                            })
                            .h(px(h))
                            .w_full(),
                    )
                    .child(
                        div()
                            .flex_row()
                            .h(px(h))
                            .with_style(move |s| s.margin.top = -h)
                            .py(LABEL_H / 2.)
                            .gap(gap)
                            .items_end()
                            .children(bars),
                    ),
            )
            .child(x_labels(&labels, &t, gap));
        let el = div()
            .flex_row()
            .items_start()
            .gap(8.)
            .when(self.y_axis, |d| d.child(axis.labels(cx, &t, &self.unit, h)))
            .child(plot);
        self.refine.apply(el).into_element()
    }
}

impl BarChart {
    /// Widgets and Live Activities: the grid and the bars in one shape
    /// (nothing to tap or animate there, and bar heights relative to the
    /// plot would not survive the snapshot).
    fn render_static(
        self,
        cx: &App,
        t: &Theme,
        axis: &Axis,
        color: Color,
        decimals: usize,
    ) -> Element {
        let n = self.bars.len().max(1) as f32;
        let mut paths = if self.y_axis {
            axis.grid(t)
        } else {
            Vec::new()
        };
        let base = axis.y(axis.lo.max(0.));
        for (i, (_, v, c)) in self.bars.iter().enumerate() {
            let top = axis.y(v.max(0.).min(axis.hi));
            let w = 0.72 / n;
            let x = (i as f32 + 0.5) / n - w / 2.;
            let faded_bar = self.highlight.is_some_and(|hl| hl != i);
            let c = c.unwrap_or(color);
            paths.push(
                Path::rect(x, top, w, (base - top).max(0.)).fill(if faded_bar {
                    faded(c, 0.35)
                } else {
                    c
                }),
            );
        }
        let spoken = self
            .bars
            .iter()
            .map(|(l, v, _)| format!("{l}: {}{}", cx.format_number(*v, decimals), self.unit))
            .collect::<Vec<_>>()
            .join(", ");
        let labels: Vec<String> = self.bars.iter().map(|b| b.0.clone()).collect();
        let h = self.height;
        let el = div()
            .flex_row()
            .items_start()
            .gap(8.)
            .when(self.y_axis, |d| d.child(axis.labels(cx, t, &self.unit, h)))
            .child(
                div()
                    .flex_1()
                    .gap(4.)
                    .child(
                        shape()
                            .inset(0., LABEL_H / 2.)
                            .paths(paths)
                            .h(px(h))
                            .w_full()
                            .label(spoken),
                    )
                    .when(!labels.iter().all(String::is_empty), |d| {
                        d.child(x_labels(&labels, t, 0.))
                    }),
            );
        self.refine.apply(el).into_element()
    }
}

component!(BarChart);

// --- Pie / donut ----------------------------------------------------------------

/// Slices of a whole, with a legend.
pub struct PieChart {
    pub slices: Vec<(String, f64, Option<Color>)>,
    /// Inner radius (fraction of the radius): a donut.
    pub hole: f32,
    /// Shown in the hole.
    pub center: Option<String>,
    pub size: f32,
    pub legend: bool,
    pub unit: String,
    refine: Refine,
}

/// A pie chart. Add slices with [`PieChart::slice`]; [`PieChart::donut`]
/// for a ring with a label in the middle.
pub fn pie_chart() -> PieChart {
    PieChart {
        slices: Vec::new(),
        hole: 0.,
        center: None,
        size: 150.,
        legend: true,
        unit: String::new(),
        refine: Refine::default(),
    }
}

impl PieChart {
    pub fn slice(mut self, label: impl Into<String>, value: f64) -> Self {
        self.slices.push((label.into(), value, None));
        self
    }
    pub fn slices<S: Into<String>>(self, slices: impl IntoIterator<Item = (S, f64)>) -> Self {
        slices.into_iter().fold(self, |c, (l, v)| c.slice(l, v))
    }
    pub fn slice_colored(mut self, label: impl Into<String>, value: f64, c: Color) -> Self {
        self.slices.push((label.into(), value, Some(c)));
        self
    }
    /// A ring, with `center` in the middle (a total, a percentage).
    pub fn donut(mut self, center: impl Into<String>) -> Self {
        self.hole = 0.62;
        self.center = Some(center.into()).filter(|c| !c.is_empty());
        self
    }
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    pub fn hide_legend(mut self) -> Self {
        self.legend = false;
        self
    }
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }
}

refinable!(PieChart);

impl Component for PieChart {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let total: f64 = self.slices.iter().map(|s| s.1.max(0.)).sum();
        let colors: Vec<Color> = self
            .slices
            .iter()
            .enumerate()
            .map(|(i, s)| s.2.unwrap_or_else(|| t.chart_color(i)))
            .collect();
        let mut start = 0.;
        let mut paths = Vec::new();
        for (s, c) in self.slices.iter().zip(&colors) {
            if total <= 0. {
                break;
            }
            let sweep = (s.1.max(0.) / total * 360.) as f32;
            paths.push(
                Path::sector(0.5, 0.5, 0.5, 0.5 * self.hole, start, start + sweep)
                    .fill(*c)
                    // Separates the slices (cards are `surface`).
                    .stroke(t.surface, 2.),
            );
            start += sweep;
        }
        if total <= 0. {
            paths.push(Path::circle(0.5, 0.5, 0.5).fill(t.surface_alt));
        }
        let pct = |v: f64| {
            if total > 0. {
                format!("{}%", cx.format_number(v.max(0.) / total * 100., 0))
            } else {
                "0%".into()
            }
        };
        let spoken = self
            .slices
            .iter()
            .map(|s| format!("{}: {}", s.0, pct(s.1)))
            .collect::<Vec<_>>()
            .join(", ");
        let size = self.size;
        let chart =
            div()
                .size(px(size))
                .flex_shrink_0()
                .child(shape().inset(1., 1.).paths(paths).size(px(size)).label(
                    match &self.center {
                        Some(c) => format!("{c}. {spoken}"),
                        None => spoken,
                    },
                ))
                .when_some(self.center.clone(), |d, center| {
                    d.child(
                        div()
                            .size(px(size))
                            .with_style(move |s| s.margin.top = -size)
                            .items_center()
                            .justify_center()
                            .accessibility_hidden()
                            .child(
                                text(center)
                                    .text_size((size * 0.14).round())
                                    .bold()
                                    .text_color(t.text),
                            ),
                    )
                });
        let rows = self.slices.iter().zip(&colors).map(|(s, c)| {
            div()
                .flex_row()
                .items_center()
                .gap(8.)
                .child(div().size(px(10.)).rounded(5.).bg(*c))
                .child(text(s.0.clone()).text_size(14.).text_color(t.text).flex_1())
                .child(text(pct(s.1)).text_size(14.).text_color(t.muted))
        });
        let el = div()
            .flex_row()
            .items_center()
            .gap(20.)
            .child(chart)
            .when(self.legend, |d| {
                d.child(div().flex_1().gap(8.).accessibility_hidden().children(rows))
            });
        self.refine.apply(el).into_element()
    }
}

component!(PieChart);

// --- Sparkline ------------------------------------------------------------------

/// A small trend line without axes (in a stat tile, a list row).
pub struct Sparkline {
    pub values: Vec<f64>,
    pub color: Option<Color>,
    pub height: f32,
    pub area: bool,
    refine: Refine,
}

pub fn sparkline(values: impl IntoIterator<Item = f64>) -> Sparkline {
    Sparkline {
        values: values.into_iter().collect(),
        color: None,
        height: 32.,
        area: true,
        refine: Refine::default(),
    }
}

impl Sparkline {
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    /// Only the line.
    pub fn line_only(mut self) -> Self {
        self.area = false;
        self
    }

    /// The trend alone, for image slots that take no views (car screens:
    /// [`CarItem::drawing`](prism_core::CarItem::drawing)).
    pub fn drawing(&self, cx: &App) -> Drawing {
        Drawing {
            inset: (3., 3.),
            paths: self.paths(&Theme::of(cx)),
        }
    }

    fn paths(&self, t: &Theme) -> Vec<Path> {
        let color = self.color.unwrap_or(t.accent);
        let min = self.values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = self
            .values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let n = self.values.len();
        let pts: Vec<(f32, f32)> = self
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = if n <= 1 {
                    1.
                } else {
                    i as f32 / (n - 1) as f32
                };
                let y = if max > min {
                    (1. - (v - min) / (max - min)) as f32
                } else {
                    0.5
                };
                (x, y)
            })
            .collect();
        let line = Path::smooth(pts.clone());
        let mut paths = Vec::new();
        if let (true, Some(first), Some(last)) = (self.area, pts.first(), pts.last()) {
            paths.push(
                line.clone()
                    .line_to(last.0, 1.)
                    .line_to(first.0, 1.)
                    .close()
                    .fill(faded(color, 0.14)),
            );
        }
        paths.push(line.stroke(color, 2.).round());
        if let Some((x, y)) = pts.last() {
            paths.push(dot(*x, *y, color, 6.));
        }
        paths
    }
}

refinable!(Sparkline);

impl Component for Sparkline {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let paths = self.paths(&t);
        let el = shape()
            .inset(3., 3.)
            .paths(paths)
            .h(px(self.height))
            .w_full()
            .label(summary(cx, "", &self.values, ""));
        self.refine.apply(el).into_element()
    }
}

component!(Sparkline);

// --- Ring -----------------------------------------------------------------------

/// A circular progress (goal, quota), with a label in the middle.
pub struct Ring {
    pub value: f64,
    pub label: Option<String>,
    pub caption: Option<String>,
    pub color: Option<Color>,
    pub size: f32,
    pub thickness: f32,
    refine: Refine,
}

/// A ring filled to `fraction` (`0.0..=1.0`).
pub fn ring(fraction: f64) -> Ring {
    Ring {
        value: fraction.clamp(0., 1.),
        label: None,
        caption: None,
        color: None,
        size: 96.,
        thickness: 10.,
        refine: Refine::default(),
    }
}

impl Ring {
    /// In the middle, large ("72%", "6 / 8").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// Under the label, small ("Goal").
    pub fn caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    pub fn thickness(mut self, t: f32) -> Self {
        self.thickness = t;
        self
    }

    /// The track and the filled arc.
    fn paths(&self, t: &Theme) -> Vec<Path> {
        let color = self.color.unwrap_or(t.accent);
        let th = self.thickness;
        let mut paths = vec![Path::arc(0.5, 0.5, 0.5, 0., 360.).stroke(faded(color, 0.18), th)];
        if self.value > 0. {
            paths.push(
                Path::arc(0.5, 0.5, 0.5, 0., (self.value * 360.) as f32)
                    .stroke(color, th)
                    .round(),
            );
        }
        paths
    }

    /// The ring alone, for image slots that take no views (car screens:
    /// [`CarItem::drawing`](prism_core::CarItem::drawing)).
    pub fn drawing(&self, cx: &App) -> Drawing {
        let th = self.thickness;
        Drawing {
            inset: (th / 2., th / 2.),
            paths: self.paths(&Theme::of(cx)),
        }
    }
}

refinable!(Ring);

impl Component for Ring {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let (size, th) = (self.size, self.thickness);
        let paths = self.paths(&t);
        let pct = format!("{}%", cx.format_number(self.value * 100., 0));
        let el = div()
            .size(px(size))
            .flex_shrink_0()
            .accessibility_role(Role::ProgressBar)
            .accessibility_label(self.caption.clone().unwrap_or_default())
            .accessibility_value(self.label.clone().unwrap_or(pct))
            .child(shape().inset(th / 2., th / 2.).paths(paths).size(px(size)))
            .child(
                div()
                    .size(px(size))
                    .with_style(move |s| s.margin.top = -size)
                    .items_center()
                    .justify_center()
                    .accessibility_hidden()
                    .when_some(self.label, |d, l| {
                        d.child(
                            text(l)
                                .text_size((size * 0.2).round())
                                .bold()
                                .text_color(t.text),
                        )
                    })
                    .when_some(self.caption, |d, c| {
                        d.child(
                            text(c)
                                .text_size((size * 0.11).round().max(10.))
                                .text_color(t.muted),
                        )
                    }),
            );
        self.refine.apply(el).into_element()
    }
}

component!(Ring);

// --- Stat tile ------------------------------------------------------------------

/// A key figure: label, value, change, and an optional trend line.
pub struct Stat {
    pub label: String,
    pub value: String,
    /// Text and whether it is good news (green) or bad (red).
    pub delta: Option<(String, bool)>,
    pub trend: Vec<f64>,
    refine: Refine,
}

pub fn stat(label: impl Into<String>, value: impl Into<String>) -> Stat {
    Stat {
        label: label.into(),
        value: value.into(),
        delta: None,
        trend: Vec::new(),
        refine: Refine::default(),
    }
}

impl Stat {
    /// `"+12%"`, `true` when it is good news.
    pub fn delta(mut self, delta: impl Into<String>, good: bool) -> Self {
        self.delta = Some((delta.into(), good));
        self
    }
    /// Recent values, drawn as a sparkline.
    pub fn trend(mut self, values: impl IntoIterator<Item = f64>) -> Self {
        self.trend = values.into_iter().collect();
        self
    }
}

refinable!(Stat);

impl Component for Stat {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let spoken = match &self.delta {
            Some((d, _)) => format!("{}: {}, {d}", self.label, self.value),
            None => format!("{}: {}", self.label, self.value),
        };
        let tone = |good: bool| if good { t.success } else { t.danger };
        let el = div()
            .bg(t.surface)
            .rounded(t.radius + 4.)
            .border(1.)
            .border_color(t.border)
            .p(14.)
            .gap(6.)
            .flex_1()
            .min_w(px(140.))
            .child(
                div()
                    .gap(4.)
                    .accessibility_label(spoken)
                    .accessibility_role(Role::Text)
                    .child(
                        div()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(8.)
                            .child(
                                text(self.label)
                                    .text_size(13.)
                                    .medium()
                                    .text_color(t.muted)
                                    .flex_1(),
                            )
                            .when_some(self.delta.clone(), |d, (delta, good)| {
                                d.child(
                                    div()
                                        .px(7.)
                                        .py(2.)
                                        .rounded(999.)
                                        .bg(faded(tone(good), 0.14))
                                        .child(
                                            text(delta)
                                                .text_size(12.)
                                                .semibold()
                                                .text_color(tone(good)),
                                        ),
                                )
                            }),
                    )
                    .child(text(self.value).text_size(24.).bold().text_color(t.text)),
            )
            .when(!self.trend.is_empty(), |d| {
                let color = self.delta.as_ref().map_or(t.accent, |(_, g)| tone(*g));
                d.child(sparkline(self.trend).color(color).height(28.))
            });
        self.refine.apply(el).into_element()
    }
}

component!(Stat);

#[cfg(test)]
mod tests {
    use prism_core::surfaces::{SNode, snapshot_one};
    use prism_core::{App, NodeKind};

    use super::*;

    fn shapes(n: &SNode, out: &mut Vec<SNode>) {
        if n.kind == NodeKind::Shape {
            out.push(n.clone());
        }
        n.children.iter().for_each(|c| shapes(c, out));
    }

    fn snap(e: impl IntoElement) -> (SNode, String) {
        let mut app = App::new();
        crate::set_theme(&mut app, Theme::system());
        let node = snapshot_one(&mut app, e);
        let json = node.to_json();
        (node, json)
    }

    #[test]
    fn charts_snapshot_as_shapes_within_surface_budgets() {
        let week = [3., 5., 4., 8., 6., 9., 7.];
        for (name, (node, json), budget) in [
            ("sparkline", snap(sparkline(week)), 900),
            ("ring", snap(ring(0.72).label("72%").size(60.)), 600),
            (
                "stat",
                snap(stat("Revenue", "12 480 €").delta("+12%", true).trend(week)),
                1500,
            ),
            (
                "line",
                snap(
                    line_chart()
                        .series("This week", week)
                        .labels(["M", "T", "W", "T", "F", "S", "S"])
                        .smooth()
                        .area(),
                ),
                2600,
            ),
            (
                "bars",
                snap(
                    bar_chart()
                        .bars([("M", 3.), ("T", 5.), ("W", 4.)])
                        .highlight(Some(1)),
                ),
                1600,
            ),
            (
                "pie",
                snap(pie_chart().slice("A", 2.).slice("B", 1.).donut("3")),
                1200,
            ),
        ] {
            let mut found = Vec::new();
            shapes(&node, &mut found);
            assert!(!found.is_empty(), "{name}: no shape");
            assert!(
                json.len() <= budget,
                "{name}: {} bytes > {budget}\n{json}",
                json.len()
            );
            // Adaptive colors (light/dark) survive the snapshot.
            assert!(json.contains("/#"), "{name}: {json}");
        }
    }

    #[test]
    fn bar_charts_draw_their_bars_in_snapshots() {
        let (node, json) = snap(
            bar_chart()
                .bars([("Mon", 3.), ("Tue", 5.)])
                .on_select(std::rc::Rc::new(|_: &ChangeEvent, _: &mut App| {})),
        );
        let mut found = Vec::new();
        shapes(&node, &mut found);
        assert_eq!(found.len(), 1, "{json}");
        // Grid lines plus one filled rectangle per bar.
        let d = prism_core::Drawing::decode(&found[0].text);
        assert_eq!(d.paths.iter().filter(|p| p.fill.is_some()).count(), 2);
        assert_eq!(found[0].label.as_deref(), Some("Mon: 3, Tue: 5"));
    }

    #[test]
    fn ring_label_overlays_its_ring_in_snapshots() {
        let (_, json) = snap(ring(0.5).label("50%").size(60.));
        // The label is pulled up over the ring by a negative margin.
        assert!(json.contains(r#""m":[-60,0,0,0]"#), "{json}");
    }
}
