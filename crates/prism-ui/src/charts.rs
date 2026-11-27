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
    Accessible, App, ChangeEvent, Color, Corners, Drawing, Element, FluentBuilder, Handler,
    IntoElement, ParentElement, Path, Role, Styled, adaptive, div, px, relative, rgb, shape, text,
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

/// A value axis: round ticks covering the data (or the ticks asked for).
struct Axis {
    lo: f64,
    hi: f64,
    ticks: Vec<f64>,
    decimals: usize,
}

impl Axis {
    /// `ticks.y` as they are (the range stretched to the data), or about
    /// `ticks.y_count` round ones.
    fn with(min: f64, max: f64, ticks: &Ticks) -> Axis {
        let values = match &ticks.y {
            Some(v) if !v.is_empty() => {
                let mut v: Vec<f64> = v.iter().copied().filter(|v| v.is_finite()).collect();
                v.sort_by(f64::total_cmp);
                v.dedup();
                v
            }
            _ => nice_ticks(min, max, ticks.y_count.unwrap_or(4).max(1)),
        };
        let values = if values.len() < 2 {
            nice_ticks(min, max, 4)
        } else {
            values
        };
        let step = values
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(f64::INFINITY, f64::min);
        let mut decimals = 0;
        while decimals < 6 {
            let scaled = step * 10f64.powi(decimals as i32);
            if (scaled.round() - scaled).abs() < 1e-6 {
                break;
            }
            decimals += 1;
        }
        let (first, last) = (values[0], *values.last().expect("two ticks"));
        Axis {
            lo: if min.is_finite() {
                first.min(min)
            } else {
                first
            },
            hi: if max.is_finite() { last.max(max) } else { last },
            ticks: values,
            decimals,
        }
    }

    /// From the top: 0 at `hi`, 1 at `lo` (clamped to the plot).
    fn y(&self, v: f64) -> f32 {
        if self.hi == self.lo {
            return 0.5;
        }
        ((1. - (v - self.lo) / (self.hi - self.lo)) as f32).clamp(0., 1.)
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

    /// Whether the ticks are evenly spaced over the whole range.
    fn even(&self) -> bool {
        let n = self.ticks.len();
        n >= 2
            && self.ticks[0] == self.lo
            && self.ticks[n - 1] == self.hi
            && self.ticks.windows(2).all(|w| {
                ((w[1] - w[0]) - (self.hi - self.lo) / (n - 1) as f64).abs()
                    < 1e-9 * (self.hi - self.lo).abs().max(1.)
            })
    }

    /// Tick labels, each centered on its grid line (a plot `height` tall,
    /// inset by half a label at the top and bottom).
    fn labels(&self, cx: &App, t: &Theme, fmt: &Fmt, height: f32) -> Element {
        let text_of = |v: f64| fmt.axis(cx, v, self.decimals);
        let label = |v: f64| text(text_of(v)).text_size(LABEL_SIZE).text_color(t.muted);
        if self.even() {
            // Evenly spaced: spread over the height.
            return div()
                .h(px(height))
                .justify_between()
                .items_end()
                .flex_shrink_0()
                .accessibility_hidden()
                .children(self.ticks.iter().rev().map(|v| label(*v)))
                .into_element();
        }
        // Uneven: each label a line tall, centered on its grid line by the
        // space above it (in the flow, so widgets keep it too).
        let inner = height - LABEL_H;
        let mut end = 0.;
        let labels: Vec<_> = self
            .ticks
            .iter()
            .rev()
            .map(|v| {
                let top = self.y(*v) * inner;
                let gap = top - end;
                end = top + LABEL_H;
                label(*v)
                    .h(px(LABEL_H))
                    .flex_shrink_0()
                    .with_style(move |s| s.margin.top = gap)
            })
            .collect();
        div()
            .h(px(height))
            .flex_shrink_0()
            .items_end()
            .accessibility_hidden()
            .children(labels)
            .into_element()
    }
}

/// How a chart writes values: [`Ticks::format_y`], or the app's number
/// format and the unit.
struct Fmt {
    custom: Option<Rc<dyn Fn(f64) -> String>>,
    unit: String,
}

impl Fmt {
    fn new(ticks: &Ticks, unit: &str) -> Fmt {
        Fmt {
            custom: ticks.format_y.clone(),
            unit: unit.to_string(),
        }
    }

    fn with(&self, cx: &App, v: f64, decimals: usize) -> String {
        if !v.is_finite() {
            return "–".into();
        }
        match &self.custom {
            Some(f) => f(v),
            None => format!("{}{}", cx.format_number(v, decimals), self.unit),
        }
    }

    /// An axis label (the axis' own decimals).
    fn axis(&self, cx: &App, v: f64, decimals: usize) -> String {
        self.with(cx, v, decimals)
    }

    /// A data value.
    /// A data value: whole numbers without decimals, others with one or two.
    fn value(&self, cx: &App, v: f64) -> String {
        let decimals = if v.fract() == 0. {
            0
        } else if ((v * 10.).fract()).abs() < 1e-9 {
            1
        } else {
            2
        };
        self.with(cx, v, decimals)
    }
}

/// Category labels under a plot, one cell per point (some skipped when
/// they would not fit, or every `every`-th).
fn x_labels(cx: &App, labels: &[String], t: &Theme, gap: f32, every: Option<usize>) -> Element {
    // Seven labels fit a phone; a watch (~170 pt) fits four.
    let room = if cx.device_info().kind == prism_core::DeviceKind::Watch {
        4.
    } else {
        7.
    };
    let stride = every
        .filter(|e| *e > 0)
        .unwrap_or_else(|| (labels.len() as f32 / room).ceil().max(1.) as usize);
    div()
        .flex_row()
        .gap(gap)
        .accessibility_hidden()
        .children(labels.iter().enumerate().map(|(i, l)| {
            // A label never widens its column (nor the chart): it is as wide
            // as the `stride` columns it stands for, centered on its own
            // (overflowing both sides), so it stays on one line.
            div()
                .flex_1()
                .min_w(px(0.))
                .items_center()
                .when(i % stride == 0, |d| {
                    d.child(
                        text(l.as_str())
                            .w(relative(stride as f32))
                            .text_size(LABEL_SIZE)
                            .text_color(t.muted)
                            .text_center(),
                    )
                })
                .when(i % stride != 0, |d| d.h(px(LABEL_H)))
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
    let fmt = Fmt::new(&Ticks::default(), unit);
    spoken(cx, title, values, &fmt)
}

fn spoken(cx: &App, title: &str, values: &[f64], fmt: &Fmt) -> String {
    // One number of decimals for the whole series.
    let finite = || values.iter().copied().filter(|v| v.is_finite());
    let decimals = if finite().all(|v| v.fract() == 0.) {
        0
    } else {
        1
    };
    let f = |v: f64| fmt.with(cx, v, decimals);
    let body = if values.len() <= 12 {
        values.iter().map(|v| f(*v)).collect::<Vec<_>>().join(", ")
    } else {
        let min = finite().fold(f64::INFINITY, f64::min);
        let max = finite().fold(f64::NEG_INFINITY, f64::max);
        format!("{} – {}", f(min), f(max))
    };
    if title.is_empty() {
        body
    } else {
        format!("{title}: {body}")
    }
}

// --- Reference lines, bands, ticks, read-outs ------------------------------------

/// A horizontal line across the plot at a value, with a label at its end
/// (an average, a target, a limit).
#[derive(Clone, Debug, PartialEq)]
pub struct RefLine {
    /// Where the line is, in data units.
    pub value: f64,
    /// Shown above the line's trailing end, and spoken.
    pub label: String,
    /// Default: the theme's muted text color.
    pub color: Option<Color>,
}

/// A colored band behind the plot between two values (good / fair / poor
/// zones).
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    /// One edge, in data units.
    pub from: f64,
    /// The other edge, in data units.
    pub to: f64,
    /// Drawn at 14 % opacity behind the grid.
    pub color: Color,
    /// Shown in the band's top leading corner.
    pub label: Option<String>,
}

/// Which axis labels a chart shows, and how it writes values.
#[derive(Clone, Default)]
pub struct Ticks {
    /// The values of the y axis labels and grid lines (default: round
    /// numbers covering the data). The axis still covers the data.
    pub y: Option<Vec<f64>>,
    /// About how many y ticks (default 4, rounded to nice steps).
    pub y_count: Option<usize>,
    /// Writes a value: axis labels, reference lines, the read-out and what
    /// screen readers say (default: the app's number format and the unit).
    pub format_y: Option<Rc<dyn Fn(f64) -> String>>,
    /// Shows every `n`-th x label (default: as many as fit).
    pub x_every: Option<usize>,
}

/// The overlay layer of a plot `height` tall: zero-height (it covers
/// nothing, so bars and tap targets under it stay tappable), its children
/// placed in points from the top of the inner area and in fractions of its
/// width.
fn overlay(inset_x: f32) -> prism_core::Div {
    div()
        .absolute()
        .top(px(LABEL_H / 2.))
        .left(px(inset_x))
        .right(px(inset_x))
        .h(px(0.))
        .accessibility_hidden()
}

/// Labels of reference lines (above each line's trailing end) and bands
/// (in their top leading corner), in an [`overlay`] of a plot whose inner
/// area is `inner` points tall.
fn plot_labels(
    cx: &App,
    t: &Theme,
    axis: &Axis,
    fmt: &Fmt,
    refs: &[RefLine],
    bands: &[Band],
    inner: f32,
) -> Vec<Element> {
    let mut out = Vec::new();
    for b in bands {
        if let Some(label) = &b.label {
            let top = axis.y(b.from.max(b.to));
            out.push(
                text(label.clone())
                    .text_size(LABEL_SIZE)
                    .medium()
                    .text_color(b.color)
                    .absolute()
                    .left(px(4.))
                    .top(px(top * inner + 2.))
                    .into_element(),
            );
        }
    }
    for r in refs {
        let color = r.color.unwrap_or(t.muted);
        let label = if r.label.is_empty() {
            fmt.value(cx, r.value)
        } else {
            r.label.clone()
        };
        out.push(
            div()
                .absolute()
                .right(px(0.))
                .top(px(axis.y(r.value) * inner - 1.))
                .translate_percent(0., -100.)
                .px(4.)
                .rounded(4.)
                .bg(faded(t.surface, 0.85))
                .child(
                    text(label)
                        .text_size(LABEL_SIZE)
                        .semibold()
                        .text_color(color),
                )
                .into_element(),
        );
    }
    out
}

/// Bands (behind) and reference lines (over the grid) as paths.
fn band_paths(axis: &Axis, bands: &[Band]) -> Vec<Path> {
    bands
        .iter()
        .map(|b| {
            let (a, c) = (axis.y(b.from), axis.y(b.to));
            Path::rect(0., a.min(c), 1., (a - c).abs()).fill(faded(b.color, 0.14))
        })
        .collect()
}

fn ref_paths(t: &Theme, axis: &Axis, refs: &[RefLine]) -> Vec<Path> {
    refs.iter()
        .map(|r| {
            let y = axis.y(r.value);
            Path::line([(0., y), (1., y)])
                .stroke(r.color.unwrap_or(t.muted), 1.5)
                .dashed(5., 4.)
        })
        .collect()
}

/// What screen readers hear about reference lines ("Average: 2.1 k€").
fn spoken_refs(cx: &App, fmt: &Fmt, refs: &[RefLine]) -> String {
    refs.iter()
        .map(|r| {
            if r.label.is_empty() {
                fmt.value(cx, r.value)
            } else {
                format!("{}: {}", r.label, fmt.value(cx, r.value))
            }
        })
        .collect::<Vec<_>>()
        .join(". ")
}

/// Where a read-out goes: beside what it reads (`from`..`to`, fractions of
/// the plot's width), on the side with more room, so the selected point or
/// bars stay visible; at the bottom of the plot when what it reads is high
/// (`top`: the highest point, a fraction from the top), else at the top.
struct Spot {
    from: f32,
    to: f32,
    top: f32,
}

/// The read-out bubble at `spot`: a title, then one row per series (color
/// dot, name when there are several, value).
fn readout(
    t: &Theme,
    spot: Spot,
    inner: f32,
    title: &str,
    rows: Vec<(Color, String, String)>,
) -> Element {
    let named = rows.len() > 1;
    let right = (spot.from + spot.to) / 2. <= 0.5;
    let low = spot.top < 0.45;
    // Points get a little air; bars are already a cell apart.
    let air = if spot.from == spot.to { 8. } else { 0. };
    div()
        .absolute()
        .left(relative(if right { spot.to } else { spot.from }))
        .top(px(if low { inner } else { 0. }))
        .translate_percent(if right { 0. } else { -100. }, if low { -100. } else { 0. })
        .offset(if right { air } else { -air }, 0.)
        .bg(t.surface)
        .border(1.)
        .border_color(t.border)
        .rounded(8.)
        .px(8.)
        .py(5.)
        .gap(2.)
        .when(!title.is_empty(), |d| {
            d.child(
                text(title.to_string())
                    .text_size(LABEL_SIZE)
                    .text_color(t.muted),
            )
        })
        .children(rows.into_iter().map(move |(c, name, value)| {
            div()
                .flex_row()
                .items_center()
                .gap(5.)
                .child(div().size(px(7.)).rounded(3.5).bg(c))
                .when(named, |d| {
                    d.child(text(name).text_size(12.).text_color(t.muted))
                })
                .child(text(value).text_size(12.).semibold().text_color(t.text))
        }))
        .into_element()
}

macro_rules! chart_options {
    ($ty:ty) => {
        impl $ty {
            /// A dashed horizontal line at `value`, labelled at its end
            /// ("Average"; an empty label shows the value).
            pub fn reference_line(mut self, value: f64, label: impl Into<String>) -> Self {
                self.refs.push(RefLine {
                    value,
                    label: label.into(),
                    color: None,
                });
                self
            }
            /// A reference line in its own color.
            pub fn reference_line_colored(
                mut self,
                value: f64,
                label: impl Into<String>,
                color: Color,
            ) -> Self {
                self.refs.push(RefLine {
                    value,
                    label: label.into(),
                    color: Some(color),
                });
                self
            }
            /// A band between two values behind the plot, in `color` at
            /// 14 % opacity (a "good" zone).
            pub fn band(mut self, from: f64, to: f64, color: Color) -> Self {
                self.bands.push(Band {
                    from,
                    to,
                    color,
                    label: None,
                });
                self
            }
            /// A band with a label in its top leading corner.
            pub fn band_labeled(
                mut self,
                from: f64,
                to: f64,
                color: Color,
                label: impl Into<String>,
            ) -> Self {
                self.bands.push(Band {
                    from,
                    to,
                    color,
                    label: Some(label.into()),
                });
                self
            }
            /// The y axis labels and grid lines at exactly these values.
            pub fn y_ticks(mut self, values: impl IntoIterator<Item = f64>) -> Self {
                self.ticks.y = Some(values.into_iter().collect());
                self
            }
            /// About `n` round y ticks (default 4).
            pub fn y_tick_count(mut self, n: usize) -> Self {
                self.ticks.y_count = Some(n);
                self
            }
            /// Writes values (axis labels, reference lines, the read-out,
            /// screen readers) with `f` instead of the number format and
            /// unit: `|v| format!("{v:.0} kWh")`.
            pub fn format_y(mut self, f: impl Fn(f64) -> String + 'static) -> Self {
                self.ticks.format_y = Some(Rc::new(f));
                self
            }
            /// Shows every `n`-th x label (`1`: all of them; default: as
            /// many as fit).
            pub fn x_label_every(mut self, n: usize) -> Self {
                self.ticks.x_every = Some(n);
                self
            }
            /// Shows the read-out of the `index`-th point or bar (a rule,
            /// dots and a bubble with the values); `None` hides it. Pair it
            /// with `on_select` to follow taps.
            pub fn selected(mut self, index: Option<usize>) -> Self {
                self.selected = index;
                self
            }
        }
    };
}

// --- Line chart ----------------------------------------------------------------

/// One line of a [`LineChart`] (or one color of a grouped [`BarChart`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// Spoken before the values; shown in the legend when there are several series.
    pub name: String,
    /// One value per label; `f64::NAN` for a missing one (a gap).
    pub values: Vec<f64>,
    /// Default: the theme's chart colors, in order.
    pub color: Option<Color>,
    /// Drawn dashed (a forecast, last year).
    pub dashed: bool,
    /// Lows and highs per label: a shaded band around the line.
    pub band: Option<(Vec<f64>, Vec<f64>)>,
}

impl Series {
    fn new(name: String, values: Vec<f64>, color: Option<Color>) -> Series {
        Series {
            name,
            values,
            color,
            dashed: false,
            band: None,
        }
    }
}

/// Lines over a value axis and category labels.
pub struct LineChart {
    /// The lines, drawn in order.
    pub series: Vec<Series>,
    /// Category labels under the plot.
    pub labels: Vec<String>,
    /// Curves instead of straight segments.
    pub smooth: bool,
    /// Fills under each line.
    pub area: bool,
    /// Marks each value with a dot.
    pub points: bool,
    /// Shows the value axis and its grid (default).
    pub y_axis: bool,
    /// Plot height in points.
    pub height: f32,
    /// Forced value range (default: the data, rounded to nice ticks).
    pub range: Option<(f64, f64)>,
    /// Appended to axis labels and spoken values (`" €"`, `"%"`).
    pub unit: String,
    /// Spoken before the values.
    pub title: String,
    /// Missing values (NaN) are bridged instead of leaving gaps.
    pub connect_gaps: bool,
    /// Horizontal reference lines.
    pub refs: Vec<RefLine>,
    /// Colored bands behind the plot.
    pub bands: Vec<Band>,
    /// Axis labels and value formatting.
    pub ticks: Ticks,
    /// The point whose values the read-out shows.
    pub selected: Option<usize>,
    /// Called with the index of the point nearest to a tap or click; the
    /// plot then has one tap target per point.
    pub on_select: Option<Handler<ChangeEvent>>,
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
        connect_gaps: false,
        refs: Vec::new(),
        bands: Vec::new(),
        ticks: Ticks::default(),
        selected: None,
        on_select: None,
        refine: Refine::default(),
    }
}

impl LineChart {
    /// Adds a line named `name`; `f64::NAN` values leave gaps.
    pub fn series(
        mut self,
        name: impl Into<String>,
        values: impl IntoIterator<Item = f64>,
    ) -> Self {
        self.series
            .push(Series::new(name.into(), values.into_iter().collect(), None));
        self
    }
    /// Colors the last added series.
    pub fn color(mut self, c: Color) -> Self {
        if let Some(s) = self.series.last_mut() {
            s.color = Some(c);
        }
        self
    }
    /// Draws the last added series dashed.
    pub fn dashed(mut self) -> Self {
        if let Some(s) = self.series.last_mut() {
            s.dashed = true;
        }
        self
    }
    /// A shaded band around the last added series, from `lows` to `highs`
    /// (one of each per label: a min–max or a confidence range).
    pub fn min_max(
        mut self,
        lows: impl IntoIterator<Item = f64>,
        highs: impl IntoIterator<Item = f64>,
    ) -> Self {
        if let Some(s) = self.series.last_mut() {
            s.band = Some((lows.into_iter().collect(), highs.into_iter().collect()));
        }
        self
    }
    /// Category labels under the plot, one per value.
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
    /// Plot height in points (default 180).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    /// Forces the value range instead of fitting the data.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some((min, max));
        self
    }
    /// Hides the value axis and its grid.
    pub fn hide_y_axis(mut self) -> Self {
        self.y_axis = false;
        self
    }
    /// Appended to axis labels and spoken values (`" €"`, `"%"`).
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }
    /// Spoken before the values.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
    /// Bridges missing values (NaN) instead of leaving gaps.
    pub fn connect_gaps(mut self) -> Self {
        self.connect_gaps = true;
        self
    }
    /// Called with the index of the point nearest to a tap or click
    /// (`e.index()`): show it with [`selected`](Self::selected).
    pub fn on_select(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_select = Some(handler);
        self
    }
}

chart_options!(LineChart);
refinable!(LineChart);

/// Runs of consecutive finite points (a gap ends a run), or every finite
/// point in one run when gaps are bridged.
/// Indices `0..n` where `ok`, in runs of consecutive ones (a missing value
/// ends a run), or all in one run when gaps are bridged.
fn runs(n: usize, ok: impl Fn(usize) -> bool, bridge: bool) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![Vec::new()];
    for i in 0..n {
        if ok(i) {
            out.last_mut().expect("a run").push(i);
        } else if !bridge && !out.last().expect("a run").is_empty() {
            out.push(Vec::new());
        }
    }
    out.retain(|r| !r.is_empty());
    out
}

impl Component for LineChart {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        // Missing values (NaN, infinities) leave gaps.
        let all: Vec<f64> = self
            .series
            .iter()
            .flat_map(|s| {
                let band = s.band.iter().flat_map(|(lo, hi)| lo.iter().chain(hi));
                s.values.iter().chain(band).copied()
            })
            .chain(self.refs.iter().map(|r| r.value))
            .filter(|v| v.is_finite())
            .collect();
        let (min, max) = self.range.unwrap_or_else(|| {
            let min = all.iter().copied().fold(f64::INFINITY, f64::min);
            let max = all.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if all.is_empty() { (0., 1.) } else { (min, max) }
        });
        let axis = Axis::with(min, max, &self.ticks);
        let fmt = Fmt::new(&self.ticks, &self.unit);
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
        let y = |v: f64| axis.y(v);
        let color_of = |i: usize, s: &Series| s.color.unwrap_or_else(|| t.chart_color(i));
        let mut paths = band_paths(&axis, &self.bands);
        if self.y_axis {
            paths.extend(axis.grid(&t));
        }
        // Min–max bands under every line.
        for (i, s) in self.series.iter().enumerate() {
            let Some((lows, highs)) = &s.band else {
                continue;
            };
            let color = color_of(i, s);
            let known = |j: usize| {
                lows.get(j).is_some_and(|v| v.is_finite())
                    && highs.get(j).is_some_and(|v| v.is_finite())
            };
            for idx in runs(n, known, self.connect_gaps) {
                let top: Vec<(f32, f32)> = idx.iter().map(|j| (x(*j), y(highs[*j]))).collect();
                let mut p = if self.smooth {
                    Path::smooth(top)
                } else {
                    Path::line(top)
                };
                let bottom: Vec<(f32, f32)> =
                    idx.iter().rev().map(|j| (x(*j), y(lows[*j]))).collect();
                // Back along the lows (reversed: monotone curves need x sorted,
                // so the low edge is straight segments).
                for (bx, by) in bottom {
                    p = p.line_to(bx, by);
                }
                paths.push(p.close().fill(faded(color, 0.16)));
            }
        }
        for (i, s) in self.series.iter().enumerate() {
            let color = color_of(i, s);
            let known = |j: usize| s.values.get(j).is_some_and(|v| v.is_finite());
            for idx in runs(n, known, self.connect_gaps) {
                let run: Vec<(f32, f32)> = idx.iter().map(|j| (x(*j), y(s.values[*j]))).collect();
                if run.len() == 1 {
                    // A point alone between gaps.
                    paths.push(dot(run[0].0, run[0].1, color, 5.));
                    continue;
                }
                let line = if self.smooth {
                    Path::smooth(run.clone())
                } else {
                    Path::line(run.clone())
                };
                if let (true, Some(first), Some(last)) = (self.area, run.first(), run.last()) {
                    paths.push(
                        line.clone()
                            .line_to(last.0, 1.)
                            .line_to(first.0, 1.)
                            .close()
                            .fill(faded(color, 0.16)),
                    );
                }
                let stroke = line.stroke(color, 2.5).round();
                paths.push(if s.dashed {
                    stroke.dashed(6., 5.)
                } else {
                    stroke
                });
            }
            if self.points {
                for (j, v) in s.values.iter().enumerate().filter(|(_, v)| v.is_finite()) {
                    paths.push(dot(x(j), y(*v), color, 8.));
                    paths.push(dot(x(j), y(*v), t.surface, 3.5));
                }
            }
        }
        paths.extend(ref_paths(&t, &axis, &self.refs));
        let selected = self.selected.filter(|i| *i < n);
        if let Some(i) = selected {
            // The read-out: a rule and a dot on every line.
            paths.push(Path::line([(x(i), 0.), (x(i), 1.)]).stroke(faded(t.muted, 0.6), 1.));
            for (k, s) in self.series.iter().enumerate() {
                if let Some(v) = s.values.get(i).filter(|v| v.is_finite()) {
                    paths.push(dot(x(i), y(*v), color_of(k, s), 10.));
                    paths.push(dot(x(i), y(*v), t.surface, 4.));
                }
            }
        }
        let mut spoken = self
            .series
            .iter()
            .map(|s| spoken(cx, &s.name, &s.values, &fmt))
            .collect::<Vec<_>>()
            .join(". ");
        if !self.refs.is_empty() {
            spoken = format!("{spoken}. {}", spoken_refs(cx, &fmt, &self.refs));
        }
        let inset_x = if labelled { 0. } else { 5. };
        let h = self.height;
        let inner = h - LABEL_H;
        let label_of = |i: usize| self.labels.get(i).cloned().unwrap_or_default();
        let values_at = |i: usize| {
            self.series
                .iter()
                .enumerate()
                .map(|(k, s)| {
                    let v = s.values.get(i).copied().unwrap_or(f64::NAN);
                    (color_of(k, s), s.name.clone(), fmt.value(cx, v))
                })
                .collect::<Vec<_>>()
        };
        let mut overlays = plot_labels(cx, &t, &axis, &fmt, &self.refs, &self.bands, inner);
        if let Some(i) = selected.filter(|_| !cx.in_snapshot()) {
            let top = self
                .series
                .iter()
                .filter_map(|s| s.values.get(i).filter(|v| v.is_finite()))
                .map(|v| y(*v))
                .fold(1., f32::min);
            // Series without a value there are left out.
            let rows: Vec<_> = values_at(i)
                .into_iter()
                .zip(&self.series)
                .filter(|(_, s)| s.values.get(i).is_some_and(|v| v.is_finite()))
                .map(|(r, _)| r)
                .collect();
            let spot = Spot {
                from: x(i),
                to: x(i),
                top,
            };
            overlays.push(readout(&t, spot, inner, &label_of(i), rows));
        }
        let snapshot = cx.in_snapshot();
        // One tap target per point, as wide as the region nearest to it.
        let targets = self
            .on_select
            .clone()
            .filter(|_| !snapshot && n > 0)
            .map(|handler| {
                div()
                    .absolute()
                    .top(px(LABEL_H / 2.))
                    .bottom(px(LABEL_H / 2.))
                    .left(px(inset_x))
                    .right(px(inset_x))
                    .flex_row()
                    .children((0..n).map(|i| {
                        let grow = if !labelled && n > 1 && (i == 0 || i == n - 1) {
                            0.5
                        } else {
                            1.
                        };
                        let spoken = values_at(i)
                            .into_iter()
                            .map(|(_, name, v)| {
                                if self.series.len() > 1 {
                                    format!("{name}: {v}")
                                } else {
                                    v
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        let label = label_of(i);
                        let handler = handler.clone();
                        div()
                            .h_full()
                            .min_w(px(0.))
                            .with_style(move |s| {
                                s.flex_grow = grow;
                                s.flex_basis = prism_core::Length::Px(0.);
                            })
                            .accessibility_role(Role::Button)
                            .accessibility_label(if label.is_empty() {
                                spoken
                            } else {
                                format!("{label}: {spoken}")
                            })
                            .on_click(Rc::new(move |_, cx: &mut App| {
                                handler(&ChangeEvent { value: i as f64 }, cx)
                            }))
                    }))
                    .into_element()
            });
        let plot = div()
            .h(px(h))
            .w_full()
            .child(
                shape()
                    .stroke_inset(inset_x, LABEL_H / 2.)
                    .paths(paths)
                    .h(px(h))
                    .w_full()
                    .label(if self.title.is_empty() {
                        spoken
                    } else {
                        format!("{}. {spoken}", self.title)
                    }),
            )
            .when_some(targets, |d, targets| d.child(targets))
            .when(!overlays.is_empty() && !snapshot, |d| {
                d.child(overlay(inset_x).children(overlays))
            });
        let legend_items: Vec<(String, Color)> = self
            .series
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.clone(), color_of(i, s)))
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
                    .when(self.y_axis, |d| d.child(axis.labels(cx, &t, &fmt, h)))
                    .child(div().flex_1().gap(4.).child(plot).when(labelled, |d| {
                        d.child(x_labels(cx, &self.labels, &t, 0., self.ticks.x_every))
                    })),
            );
        self.refine.apply(el).into_element()
    }
}

component!(LineChart);

// --- Bar chart -----------------------------------------------------------------

/// Vertical bars, each a native element: tappable, spoken on its own, and
/// animated when its value changes. Several series draw grouped bars.
pub struct BarChart {
    /// Label, value, color (default: the chart's color).
    pub bars: Vec<(String, f64, Option<Color>)>,
    /// Color of every bar without its own (default: the accent).
    pub color: Option<Color>,
    /// Plot height in points.
    pub height: f32,
    /// Shows the value axis and its grid (default).
    pub y_axis: bool,
    /// Forced value range (default: zero to the largest value).
    pub range: Option<(f64, f64)>,
    /// The other bars fade.
    pub highlight: Option<usize>,
    /// Appended to axis labels and spoken values.
    pub unit: String,
    /// Called with the tapped bar's index; bars are then buttons.
    pub on_select: Option<Handler<ChangeEvent>>,
    /// Corner radius of the bars, points (default 4; 0: square).
    pub bar_radius: f32,
    /// The corners of each bar that are rounded (default: the top ones, the
    /// bars stand on the baseline).
    pub bar_corners: Corners,
    /// Grouped bars: one bar per series in each category of `labels`
    /// (instead of `bars`).
    pub series: Vec<Series>,
    /// Category labels of grouped bars.
    pub labels: Vec<String>,
    /// Horizontal reference lines.
    pub refs: Vec<RefLine>,
    /// Colored bands behind the plot.
    pub bands: Vec<Band>,
    /// Axis labels and value formatting.
    pub ticks: Ticks,
    /// The bar (or group) whose values the read-out shows; the others fade.
    pub selected: Option<usize>,
    refine: Refine,
}

/// A bar chart. Add bars with [`BarChart::bar`], or groups with
/// [`BarChart::labels`] and [`BarChart::series`]. Values below zero draw
/// as zero.
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
        bar_radius: 4.,
        bar_corners: Corners::TOP,
        series: Vec::new(),
        labels: Vec::new(),
        refs: Vec::new(),
        bands: Vec::new(),
        ticks: Ticks::default(),
        selected: None,
        refine: Refine::default(),
    }
}

impl BarChart {
    /// Rounds the bars by `r` points (0: square bars).
    pub fn bar_radius(mut self, r: f32) -> Self {
        self.bar_radius = r;
        self
    }
    /// Which corners of each bar are rounded (default `Corners::TOP`;
    /// `Corners::ALL` for pills).
    pub fn bar_corners(mut self, corners: Corners) -> Self {
        self.bar_corners = corners;
        self
    }
    /// Adds a bar.
    pub fn bar(mut self, label: impl Into<String>, value: f64) -> Self {
        self.bars.push((label.into(), value, None));
        self
    }
    /// Adds bars from `(label, value)` pairs.
    pub fn bars<S: Into<String>>(self, bars: impl IntoIterator<Item = (S, f64)>) -> Self {
        bars.into_iter().fold(self, |c, (l, v)| c.bar(l, v))
    }
    /// One bar in its own color.
    pub fn bar_colored(mut self, label: impl Into<String>, value: f64, c: Color) -> Self {
        self.bars.push((label.into(), value, Some(c)));
        self
    }
    /// Category labels of grouped bars (with [`series`](Self::series)).
    pub fn labels<S: Into<String>>(mut self, labels: impl IntoIterator<Item = S>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }
    /// Adds a series: one bar per label, side by side with the other
    /// series' (grouped bars), in the theme's chart colors.
    pub fn series(
        mut self,
        name: impl Into<String>,
        values: impl IntoIterator<Item = f64>,
    ) -> Self {
        self.series
            .push(Series::new(name.into(), values.into_iter().collect(), None));
        self
    }
    /// Adds a series in its own color.
    pub fn series_colored(
        mut self,
        name: impl Into<String>,
        values: impl IntoIterator<Item = f64>,
        c: Color,
    ) -> Self {
        self.series.push(Series::new(
            name.into(),
            values.into_iter().collect(),
            Some(c),
        ));
        self
    }
    /// Color of every bar without its own.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// Plot height in points (default 180).
    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }
    /// Forces the value range.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some((min, max));
        self
    }
    /// Hides the value axis and its grid.
    pub fn hide_y_axis(mut self) -> Self {
        self.y_axis = false;
        self
    }
    /// Highlights one bar (the others fade); `None` for none.
    pub fn highlight(mut self, index: Option<usize>) -> Self {
        self.highlight = index;
        self
    }
    /// Appended to axis labels and spoken values.
    pub fn unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = unit.into();
        self
    }
    /// Called with the tapped bar's (or group's) index (`e.index()`).
    pub fn on_select(mut self, handler: Handler<ChangeEvent>) -> Self {
        self.on_select = Some(handler);
        self
    }

    /// Categories: a label and its bars' values and colors.
    fn groups(&self, t: &Theme) -> (Vec<String>, Vec<Vec<(f64, Color)>>) {
        if self.series.is_empty() {
            let color = self.color.unwrap_or(t.accent);
            return (
                self.bars.iter().map(|b| b.0.clone()).collect(),
                self.bars
                    .iter()
                    .map(|b| vec![(b.1, b.2.unwrap_or(color))])
                    .collect(),
            );
        }
        let n = self
            .series
            .iter()
            .map(|s| s.values.len())
            .max()
            .unwrap_or(0)
            .max(self.labels.len());
        let labels = (0..n)
            .map(|i| self.labels.get(i).cloned().unwrap_or_default())
            .collect();
        let groups = (0..n)
            .map(|i| {
                self.series
                    .iter()
                    .enumerate()
                    .map(|(k, s)| {
                        (
                            s.values.get(i).copied().unwrap_or(f64::NAN),
                            s.color.unwrap_or_else(|| t.chart_color(k)),
                        )
                    })
                    .collect()
            })
            .collect();
        (labels, groups)
    }

    /// What one category says: its label and value(s).
    fn spoken_group(&self, cx: &App, fmt: &Fmt, label: &str, values: &[(f64, Color)]) -> String {
        let body = if self.series.is_empty() {
            values
                .iter()
                .map(|(v, _)| fmt.value(cx, *v))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            self.series
                .iter()
                .zip(values)
                .map(|(s, (v, _))| format!("{}: {}", s.name, fmt.value(cx, *v)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if label.is_empty() {
            body
        } else {
            format!("{label}: {body}")
        }
    }
}

chart_options!(BarChart);
refinable!(BarChart);

impl Component for BarChart {
    fn render(self, cx: &mut App) -> Element {
        let t = Theme::of(cx);
        let (labels, groups) = self.groups(&t);
        let finite = || {
            groups
                .iter()
                .flatten()
                .map(|g| g.0)
                .filter(|v| v.is_finite())
        };
        let max = finite()
            .chain(self.refs.iter().map(|r| r.value))
            .fold(0., f64::max);
        let (lo, hi) = self.range.unwrap_or((0., if max > 0. { max } else { 1. }));
        let axis = Axis::with(lo.min(0.), hi, &self.ticks);
        let fmt = Fmt::new(&self.ticks, &self.unit);
        let (h, gap) = (self.height, if self.series.is_empty() { 6. } else { 10. });
        let highlight = self.selected.or(self.highlight);
        if cx.in_snapshot() {
            return self.render_static(cx, &t, &axis, &fmt, &labels, &groups, highlight);
        }
        let n = groups.len();
        let grouped = !self.series.is_empty();
        let fraction = |v: f64| {
            if v.is_finite() {
                ((v.max(0.) - axis.lo) / (axis.hi - axis.lo)).clamp(0., 1.) as f32
            } else {
                0.
            }
        };
        let cells = groups.iter().enumerate().map(|(i, values)| {
            let faded_cell = highlight.is_some_and(|hl| hl != i);
            let spoken = self.spoken_group(cx, &fmt, &labels[i], values);
            let bar = |v: f64, c: Color| {
                div()
                    .h(relative(fraction(v)))
                    .min_h(px(if v > 0. { 2. } else { 0. }))
                    .rounded_corners(self.bar_radius, self.bar_corners)
                    .bg(c)
                    .opacity(if faded_cell { 0.35 } else { 1. })
                    .spring(450)
            };
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
                .map(|d| {
                    if grouped {
                        // Side by side, sharing the cell (a gap between them).
                        d.child(
                            div()
                                .flex_row()
                                .items_end()
                                .gap(2.)
                                .w(relative(0.84))
                                .max_w(px(28. * values.len() as f32))
                                .h_full()
                                .children(
                                    values
                                        .iter()
                                        .map(|(v, c)| bar(*v, *c).flex_1().min_w(px(0.))),
                                ),
                        )
                    } else {
                        let (v, c) = values[0];
                        d.child(bar(v, c).w(relative(0.72)).max_w(px(44.)))
                    }
                })
        });
        let inner = h - LABEL_H;
        let mut overlays = plot_labels(cx, &t, &axis, &fmt, &self.refs, &self.bands, inner);
        if let Some(i) = self.selected.filter(|i| *i < n) {
            let rows = if grouped {
                self.series
                    .iter()
                    .zip(&groups[i])
                    .map(|(s, (v, c))| (*c, s.name.clone(), fmt.value(cx, *v)))
                    .collect()
            } else {
                vec![(groups[i][0].1, String::new(), fmt.value(cx, groups[i][0].0))]
            };
            // Beside the cell's bars (they take 72–84 % of it).
            let (cell, half) = (1. / n as f32, if grouped { 0.42 } else { 0.36 });
            let center = (i as f32 + 0.5) * cell;
            let spot = Spot {
                from: center - half * cell,
                to: center + half * cell,
                // Beside the bars, whatever their height: at the top.
                top: 1.,
            };
            overlays.push(readout(&t, spot, inner, &labels[i], rows));
        }
        let mut lines = band_paths(&axis, &self.bands);
        if self.y_axis {
            lines.extend(axis.grid(&t));
        }
        let refs = ref_paths(&t, &axis, &self.refs);
        let plot = div()
            .flex_1()
            .gap(4.)
            .child(
                // The grid, with the bars over it (pulled up by a negative
                // margin), reference lines over the bars, labels on top.
                div()
                    .child(
                        shape()
                            .stroke_inset(0., LABEL_H / 2.)
                            .paths(lines)
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
                            .children(cells),
                    )
                    .when(!refs.is_empty(), |d| {
                        d.child(
                            shape()
                                .stroke_inset(0., LABEL_H / 2.)
                                .paths(refs)
                                .h(px(h))
                                .w_full()
                                .with_style(move |s| s.margin.top = -h),
                        )
                    })
                    .when(!overlays.is_empty(), |d| {
                        d.child(overlay(0.).children(overlays))
                    }),
            )
            .when(!labels.iter().all(String::is_empty), |d| {
                d.child(x_labels(cx, &labels, &t, gap, self.ticks.x_every))
            });
        let legend_items: Vec<(String, Color)> = self
            .series
            .iter()
            .enumerate()
            .map(|(k, s)| (s.name.clone(), s.color.unwrap_or_else(|| t.chart_color(k))))
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
                    .when(self.y_axis, |d| d.child(axis.labels(cx, &t, &fmt, h)))
                    .child(plot),
            );
        self.refine.apply(el).into_element()
    }
}

impl BarChart {
    /// Widgets and Live Activities: the grid and the bars in one shape
    /// (nothing to tap or animate there, and bar heights relative to the
    /// plot would not survive the snapshot).
    #[allow(clippy::too_many_arguments)]
    fn render_static(
        self,
        cx: &App,
        t: &Theme,
        axis: &Axis,
        fmt: &Fmt,
        labels: &[String],
        groups: &[Vec<(f64, Color)>],
        highlight: Option<usize>,
    ) -> Element {
        let n = groups.len().max(1) as f32;
        let mut paths = band_paths(axis, &self.bands);
        if self.y_axis {
            paths.extend(axis.grid(t));
        }
        let base = axis.y(axis.lo.max(0.));
        for (i, values) in groups.iter().enumerate() {
            let k = values.len().max(1) as f32;
            let cell = if self.series.is_empty() { 0.72 } else { 0.84 } / n;
            let w = cell / k;
            for (j, (v, c)) in values.iter().enumerate() {
                if !v.is_finite() {
                    continue;
                }
                let top = axis.y(v.max(0.).min(axis.hi));
                let x = (i as f32 + 0.5) / n - cell / 2. + j as f32 * w;
                let faded_bar = highlight.is_some_and(|hl| hl != i);
                // A hairline between grouped bars.
                let wb = if k > 1. { w * 0.9 } else { w };
                paths.push(
                    Path::rect(x, top, wb, (base - top).max(0.)).fill(if faded_bar {
                        faded(*c, 0.35)
                    } else {
                        *c
                    }),
                );
            }
        }
        paths.extend(ref_paths(t, axis, &self.refs));
        let mut spoken = groups
            .iter()
            .zip(labels)
            .map(|(g, l)| self.spoken_group(cx, fmt, l, g))
            .collect::<Vec<_>>()
            .join(", ");
        if !self.refs.is_empty() {
            spoken = format!("{spoken}. {}", spoken_refs(cx, fmt, &self.refs));
        }
        let h = self.height;
        let el = div()
            .flex_row()
            .items_start()
            .gap(8.)
            .when(self.y_axis, |d| d.child(axis.labels(cx, t, fmt, h)))
            .child(
                div()
                    .flex_1()
                    .gap(4.)
                    .child(
                        shape()
                            .stroke_inset(0., LABEL_H / 2.)
                            .paths(paths)
                            .h(px(h))
                            .w_full()
                            .label(spoken),
                    )
                    .when(!labels.iter().all(String::is_empty), |d| {
                        d.child(x_labels(cx, labels, t, 0., self.ticks.x_every))
                    }),
            );
        self.refine.apply(el).into_element()
    }
}

component!(BarChart);

// --- Pie / donut ----------------------------------------------------------------

/// Slices of a whole, with a legend.
pub struct PieChart {
    /// Label, value, color (default: the theme's chart colors, in order).
    pub slices: Vec<(String, f64, Option<Color>)>,
    /// Inner radius (fraction of the radius): a donut.
    pub hole: f32,
    /// Shown in the hole.
    pub center: Option<String>,
    /// Diameter in points.
    pub size: f32,
    /// Shows the legend with each slice's percentage (default).
    pub legend: bool,
    /// Unit of the values (slices are shown and read as percentages).
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
    /// Adds a slice; negative values count as zero.
    pub fn slice(mut self, label: impl Into<String>, value: f64) -> Self {
        self.slices.push((label.into(), value, None));
        self
    }
    /// Adds slices from `(label, value)` pairs.
    pub fn slices<S: Into<String>>(self, slices: impl IntoIterator<Item = (S, f64)>) -> Self {
        slices.into_iter().fold(self, |c, (l, v)| c.slice(l, v))
    }
    /// Adds a slice in its own color.
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
    /// Diameter in points (default 150).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    /// Hides the legend.
    pub fn hide_legend(mut self) -> Self {
        self.legend = false;
        self
    }
    /// Unit of the values (slices are shown and read as percentages).
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
        let chart = div()
            .size(px(size))
            .flex_shrink_0()
            .child(
                shape()
                    .stroke_inset(1., 1.)
                    .paths(paths)
                    .size(px(size))
                    .label(match &self.center {
                        Some(c) => format!("{c}. {spoken}"),
                        None => spoken,
                    }),
            )
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
        // The legend goes under the chart when there is no room beside it
        // (watches, narrow columns).
        let el = div()
            .flex_row()
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap(20.)
            .child(chart)
            .when(self.legend, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w(px(140.))
                        .gap(8.)
                        .accessibility_hidden()
                        .children(rows),
                )
            });
        self.refine.apply(el).into_element()
    }
}

component!(PieChart);

// --- Sparkline ------------------------------------------------------------------

/// A small trend line without axes (in a stat tile, a list row).
pub struct Sparkline {
    /// The values, left to right, scaled to fit the height.
    pub values: Vec<f64>,
    /// Line color (default: the accent).
    pub color: Option<Color>,
    /// Height in points.
    pub height: f32,
    /// Fills under the line (default).
    pub area: bool,
    refine: Refine,
}

/// A sparkline of `values`.
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
    /// Line color.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// Height in points (default 32).
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
            ..Default::default()
        }
    }

    fn paths(&self, t: &Theme) -> Vec<Path> {
        let color = self.color.unwrap_or(t.accent);
        let finite = || self.values.iter().copied().filter(|v| v.is_finite());
        let min = finite().fold(f64::INFINITY, f64::min);
        let max = finite().fold(f64::NEG_INFINITY, f64::max);
        let n = self.values.len();
        let pts: Vec<(f32, f32)> = self
            .values
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
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
            .stroke_inset(3., 3.)
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
    /// Fill, from `0.0` to `1.0`.
    pub value: f64,
    /// Large text in the middle, also the spoken value (default: the percentage).
    pub label: Option<String>,
    /// Small text under the label, also the accessibility label.
    pub caption: Option<String>,
    /// Ring color (default: the accent).
    pub color: Option<Color>,
    /// Diameter in points.
    pub size: f32,
    /// Stroke width in points.
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
    /// Ring color.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// Diameter in points (default 96).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    /// Stroke width in points (default 10).
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
            ..Default::default()
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
            .child(
                shape()
                    .stroke_inset(th / 2., th / 2.)
                    .paths(paths)
                    .size(px(size)),
            )
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
    /// What the figure is ("Revenue").
    pub label: String,
    /// The figure, already formatted ("12 480 €").
    pub value: String,
    /// Text and whether it is good news (green) or bad (red).
    pub delta: Option<(String, bool)>,
    /// Values of an optional sparkline.
    pub trend: Vec<f64>,
    refine: Refine,
}

/// A stat tile: `label` ("Revenue") above `value` ("12 480 €").
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
