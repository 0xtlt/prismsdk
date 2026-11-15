//! Charts: ticks, scales, curves, accessibility, empty and odd data.

use std::sync::mpsc;
use std::time::Duration;

use prism_core::prelude::*;
use prism_core::shape::nice_ticks;
use prism_core::{Drawing, NodeKind, Segment, UiEvent};
use prism_testing::Harness;
use prism_ui::{self as ui, Theme};

use crate::{labelled, log, node, show};

/// Drawings of every mounted shape, in tree order.
fn drawings(h: &Harness) -> Vec<Drawing> {
    h.host
        .mounted()
        .into_iter()
        .filter(|id| node(h, *id).kind == NodeKind::Shape)
        .map(|id| Drawing::decode(&node(h, id).text))
        .collect()
}

/// Every y coordinate (end and control points) of a path.
fn ys(p: &prism_core::Path) -> Vec<f32> {
    p.segments
        .iter()
        .flat_map(|s| match *s {
            Segment::Move(_, y) | Segment::Line(_, y) => vec![y],
            Segment::Cubic(_, y1, _, y2, _, y) => vec![y1, y2, y],
            _ => vec![],
        })
        .collect()
}

// --- Ticks ---------------------------------------------------------------------

#[test]
fn ticks_cover_the_data_with_one_round_step() {
    for (min, max) in [
        (0., 1.),
        (3., 12.),
        (-7., 3.),
        (-250., -40.),
        (0.001, 0.0042),
        (1_000., 1_000_000.),
        (99., 101.),
        (42., 42.),
    ] {
        let t = nice_ticks(min, max, 4);
        assert!(t.len() >= 2 && t.len() <= 9, "{min}..{max}: {t:?}");
        assert!(
            t[0] <= min && *t.last().unwrap() >= max,
            "{min}..{max}: {t:?}"
        );
        let step = t[1] - t[0];
        assert!(step > 0.);
        for w in t.windows(2) {
            assert!(((w[1] - w[0]) - step).abs() < step * 1e-6, "{t:?}");
        }
        // 1, 2, 2.5 or 5 times a power of ten.
        let mantissa = step / 10f64.powf(step.log10().floor());
        assert!(
            [1., 2., 2.5, 5., 10.]
                .iter()
                .any(|m| (mantissa - m).abs() < 1e-6),
            "step {step}"
        );
    }
    // Reversed bounds are accepted.
    assert_eq!(nice_ticks(10., 0., 5), nice_ticks(0., 10., 5));
}

/// Runs `f` on another thread, failing (instead of hanging) after a while.
fn within_a_second<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("did not finish (infinite loop?)")
}

#[test]
fn ticks_of_non_finite_data_terminate() {
    for (min, max) in [
        (f64::NAN, 1.),
        (0., f64::INFINITY),
        (f64::INFINITY, f64::NEG_INFINITY),
        (f64::NAN, f64::NAN),
    ] {
        let t = within_a_second(move || nice_ticks(min, max, 4));
        assert!(t.len() >= 2 && t.iter().all(|v| v.is_finite()), "{t:?}");
    }
}

// --- Line charts ---------------------------------------------------------------

#[test]
fn line_charts_scale_values_into_the_plot() {
    let h = show(Theme::light, |_, _| {
        ui::line_chart()
            .series("Revenue", [10., 20., 15.])
            .title("Week")
            .into_element()
    });
    let d = &drawings(&h)[0];
    // Grid lines first (one per tick), then the line.
    let ticks = nice_ticks(10., 20., 4);
    assert_eq!(d.paths.len(), ticks.len() + 1);
    let line = d.paths.last().unwrap();
    let Segment::Move(x0, y0) = line.segments[0] else {
        panic!("{line:?}")
    };
    let (lo, hi) = (ticks[0], *ticks.last().unwrap());
    let y = |v: f64| (1. - (v - lo) / (hi - lo)) as f32;
    assert_eq!((x0, y0), (0., y(10.)));
    assert_eq!(line.segments[2], Segment::Line(1., y(15.)));
    assert_eq!(line.stroke, Some(Theme::light().accent));
    // Spoken: the title, the series and its values.
    labelled(&h, "Week. Revenue: 10, 20, 15");
    // Axis labels, top to bottom, are hidden from screen readers.
    let top = h.host.by_text(&format!("{hi:.1}"));
    assert!(h.host.find(|n| n.children.contains(&top)).is_some());
}

#[test]
fn smooth_curves_stay_within_their_segments() {
    let h = show(Theme::light, |_, _| {
        ui::line_chart()
            .series("s", [3., 9., 2., 2., 8., 7.5, 0., 4.])
            .smooth()
            .hide_y_axis()
            .into_element()
    });
    let line = &drawings(&h)[0].paths[0];
    let mut prev = None;
    for s in &line.segments {
        match *s {
            Segment::Move(_, y) => prev = Some(y),
            Segment::Cubic(_, y1, _, y2, _, y) => {
                let y0 = prev.unwrap();
                let (lo, hi) = (y0.min(y), y0.max(y));
                for c in [y1, y2] {
                    assert!(c >= lo - 1e-4 && c <= hi + 1e-4, "{c} outside {lo}..{hi}");
                }
                prev = Some(y);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(ys(line).iter().all(|y| (0.0..=1.0).contains(y)));
}

#[test]
fn single_points_empty_and_negative_series() {
    let h = show(Theme::light, |_, _| {
        div()
            .child(ui::line_chart().series("one", [5.]).hide_y_axis())
            .child(ui::line_chart().hide_y_axis())
            .child(ui::line_chart().series("neg", [-4., -1., -9.]).points())
            .into_element()
    });
    let d = drawings(&h);
    // One point sits in the middle, drawn as a dot (a line would be invisible).
    assert_eq!(
        d[0].paths[0].segments,
        [Segment::Move(0.5, 0.5), Segment::Line(0.5, 0.5)]
    );
    assert!(d[0].paths[0].round && d[0].paths[0].stroke.is_some());
    // No series: nothing but an empty plot, still announced (empty).
    assert!(d[1].paths.is_empty());
    // Negative values: the axis goes below zero, the line stays inside.
    let neg = &d[2];
    assert!(
        neg.paths
            .iter()
            .all(|p| ys(p).iter().all(|y| (0.0..=1.0).contains(y)))
    );
    h.host.by_text("-10");
    labelled(&h, "neg: -4, -1, -9");
}

#[test]
fn charts_with_missing_values_still_render() {
    let h = within_a_second(|| {
        let h = show(Theme::light, |_, _| {
            div()
                .child(ui::line_chart().series("nan", [f64::NAN, f64::NAN]))
                .child(ui::sparkline([f64::NAN]))
                .into_element()
        });
        h.describe()
    });
    assert!(h.contains("shape"));
}

#[test]
fn several_series_get_a_legend_and_distinct_colors() {
    let h = show(Theme::light, |_, _| {
        ui::line_chart()
            .series("a", [1., 2.])
            .series("b", [2., 1.])
            .hide_y_axis()
            .into_element()
    });
    h.host.by_text("a");
    h.host.by_text("b");
    let d = &drawings(&h)[0];
    let t = Theme::light();
    assert_eq!(d.paths[0].stroke, Some(t.chart_color(0)));
    assert_eq!(d.paths[1].stroke, Some(t.chart_color(1)));
    assert_ne!(t.chart_color(1), t.chart_color(2));
    labelled(&h, "a: 1, 2. b: 2, 1");
}

#[test]
fn long_series_are_summarized_by_their_range() {
    let h = show(Theme::light, |_, _| {
        ui::line_chart()
            .series("t", (0..30).map(|i| i as f64 * 0.5))
            .into_element()
    });
    labelled(&h, "t: 0.0 – 14.5");
}

// --- Bars, pies, rings, stats --------------------------------------------------

#[test]
fn bars_are_separate_tappable_elements() {
    let mut h = show(Theme::light, |p, cx| {
        ui::bar_chart()
            .bar("Mon", 3.)
            .bar("Tue", 6.)
            .bar("Wed", -2.)
            .highlight(p.on.then_some(1))
            .on_select(cx.listener(|p, e: &ChangeEvent, _| {
                p.log.push(format!("bar {}", e.index()));
                p.on = true;
            }))
            .into_element()
    });
    let tue = labelled(&h, "Tue: 6");
    assert_eq!(node(&h, tue).a11y.role, Some(Role::Button));
    labelled(&h, "Mon: 3");
    // Negative values are announced but drawn empty from zero.
    let wed = labelled(&h, "Wed: -2");
    let fill = node(&h, node(&h, wed).children[0]).style.clone().unwrap();
    assert_eq!(fill.height, prism_core::Length::Fraction(0.));
    h.dispatch(UiEvent::Click(tue));
    assert_eq!(log(&h), ["bar 1"]);
    // The others fade.
    let opacity = |l: &str| {
        let bar = labelled(&h, l);
        node(&h, node(&h, bar).children[0])
            .style
            .clone()
            .unwrap()
            .opacity
    };
    assert_eq!((opacity("Mon: 3"), opacity("Tue: 6")), (0.35, 1.));
}

#[test]
fn bar_heights_are_fractions_of_the_axis() {
    let h = show(Theme::light, |_, _| {
        ui::bar_chart()
            .bars([("a", 5.), ("b", 10.), ("z", 0.)])
            .into_element()
    });
    let height = |l: &str| {
        let bar = labelled(&h, l);
        node(&h, node(&h, bar).children[0])
            .style
            .clone()
            .unwrap()
            .height
    };
    let top = *nice_ticks(0., 10., 4).last().unwrap() as f32;
    assert_eq!(height("a: 5"), prism_core::Length::Fraction(5. / top));
    assert_eq!(height("b: 10"), prism_core::Length::Fraction(10. / top));
    assert_eq!(height("z: 0"), prism_core::Length::Fraction(0.));
    // Without on_select, bars are static text.
    assert_eq!(node(&h, labelled(&h, "a: 5")).a11y.role, Some(Role::Text));
}

#[test]
fn pies_split_the_circle_by_share() {
    let h = show(Theme::light, |_, _| {
        ui::pie_chart()
            .slice("Rent", 750.)
            .slice("Food", 250.)
            .slice("Refund", -50.)
            .donut("1 000 €")
            .into_element()
    });
    labelled(&h, "1 000 €. Rent: 75%, Food: 25%, Refund: 0%");
    let d = &drawings(&h)[0];
    // Negative slices take no room: two sectors and an empty one.
    let sweeps: Vec<(f32, f32)> = d
        .paths
        .iter()
        .filter_map(|p| match p.segments.first() {
            Some(Segment::Arc { start, end, .. }) => Some((*start, *end)),
            _ => None,
        })
        .collect();
    assert_eq!(sweeps[0], (0., 270.));
    assert_eq!(sweeps[1], (270., 360.));
}

#[test]
fn empty_pies_show_a_placeholder() {
    let h = show(Theme::light, |_, _| {
        ui::pie_chart().slice("A", 0.).into_element()
    });
    labelled(&h, "A: 0%");
    let d = &drawings(&h)[0];
    assert_eq!(d.paths.len(), 1);
    assert_eq!(d.paths[0].fill, Some(Theme::light().surface_alt));
}

#[test]
fn rings_are_progress_bars() {
    let h = show(Theme::light, |_, _| {
        div()
            .child(ui::ring(0.72).caption("Goal"))
            .child(ui::ring(3.).caption("Over").label("Done"))
            .child(ui::ring(0.).caption("Empty"))
            .into_element()
    });
    let goal = labelled(&h, "Goal");
    assert_eq!(node(&h, goal).a11y.role, Some(Role::ProgressBar));
    assert_eq!(node(&h, goal).a11y.value.as_deref(), Some("72%"));
    assert_eq!(
        node(&h, labelled(&h, "Over")).a11y.value.as_deref(),
        Some("Done")
    );
    let d = drawings(&h);
    // Clamped to a full turn; an empty ring only has its track.
    let Segment::Arc { end, .. } = d[1].paths[1].segments[0] else {
        panic!()
    };
    assert_eq!(end, 360.);
    assert_eq!(d[2].paths.len(), 1);
}

#[test]
fn stats_read_label_value_and_change() {
    let h = show(Theme::light, |_, _| {
        div()
            .child(
                ui::stat("Visitors", "12 400")
                    .delta("+12%", true)
                    .trend([3., 5., 4.]),
            )
            .child(ui::stat("Errors", "3").delta("+1", false))
            .into_element()
    });
    labelled(&h, "Visitors: 12 400, +12%");
    let t = Theme::light();
    let color = |s: &str| {
        node(&h, h.host.by_text(s))
            .style
            .clone()
            .unwrap()
            .text_color
    };
    assert_eq!(color("+12%"), Some(t.success));
    assert_eq!(color("+1"), Some(t.danger));
    // The trend is a sparkline ending on a dot at the last value.
    let spark = drawings(&h).into_iter().last().unwrap();
    assert!(spark.paths.last().unwrap().round);
}

// --- Reference lines, bands, ticks, gaps, groups, read-outs ------------------------

/// Subpaths of a path (each starts with a move).
fn subpaths(p: &prism_core::Path) -> usize {
    p.segments
        .iter()
        .filter(|s| matches!(s, Segment::Move(..)))
        .count()
}

#[test]
fn missing_values_leave_gaps_unless_bridged() {
    let h = show(Theme::light, |_, _| {
        div()
            .child(
                ui::line_chart()
                    .series("kWh", [1., 2., f64::NAN, 4., 5.])
                    .hide_y_axis(),
            )
            .child(
                ui::line_chart()
                    .series("kWh", [1., 2., f64::NAN, 4., 5.])
                    .connect_gaps()
                    .hide_y_axis(),
            )
            .into_element()
    });
    let d = drawings(&h);
    let stroked = |d: &Drawing| d.paths.iter().filter(|p| p.stroke.is_some()).count();
    // Two runs: two lines, nothing drawn across the gap.
    assert_eq!(stroked(&d[0]), 2);
    assert_eq!(stroked(&d[1]), 1);
    assert_eq!(subpaths(&d[1].paths[0]), 1);
    // Spoken: the missing value is a dash.
    let n = h
        .host
        .find(|n| n.a11y.label.as_deref() == Some("kWh: 1, 2, –, 4, 5"));
    assert!(n.is_some(), "{}", h.describe());
}

#[test]
fn min_max_bands_dashed_series_reference_lines_and_bands() {
    let t = Theme::light();
    let h = show(Theme::light, |_, _| {
        ui::line_chart()
            .series("Temp", [10., 12., 11.])
            .min_max([8., 9., 7.], [13., 15., 14.])
            .series("Forecast", [11., 13., 12.])
            .dashed()
            .labels(["Mon", "Tue", "Wed"])
            .reference_line(11., "Average")
            .band_labeled(12., 20., rgb(0xE5484D), "Hot")
            .y_ticks([0., 10., 20.])
            .into_element()
    });
    let d = &drawings(&h)[0];
    // The band first (behind), at 14 % opacity, from 12 to 20 (top of the axis).
    let band = &d.paths[0];
    assert_eq!(band.fill.map(|c| (c.a * 100.).round()), Some(14.));
    assert_eq!(band.segments[0], Segment::Move(0., 0.));
    // Grid lines only at the ticks asked for.
    let grid = d
        .paths
        .iter()
        .filter(|p| p.stroke == Some(t.border))
        .count();
    assert_eq!(grid, 3);
    // The min–max band: a filled shape around the first line.
    assert!(
        d.paths
            .iter()
            .any(|p| p.fill.is_some() && subpaths(p) == 1 && p.segments.len() > 4)
    );
    // The forecast is dashed, the reference line too, at y = 11 (0.45 from the top).
    assert!(d.paths.iter().filter(|p| p.dash.is_some()).count() >= 2);
    let reference = d.paths.last().unwrap();
    assert_eq!(reference.segments[0], Segment::Move(0., 0.45));
    // Their labels, and what screen readers hear.
    h.host.by_text("Average");
    h.host.by_text("Hot");
    for tick in ["0", "10", "20"] {
        h.host.by_text(tick);
    }
    labelled(&h, "Temp: 10, 12, 11. Forecast: 11, 13, 12. Average: 11");
}

#[test]
fn ticks_can_be_formatted_and_thinned() {
    let h = show(Theme::light, |_, _| {
        ui::line_chart()
            .series("Power", [100., 250., 400., 300., 200., 150.])
            .labels(["0h", "4h", "8h", "12h", "16h", "20h"])
            .y_tick_count(2)
            .format_y(|v| format!("{v:.0} W"))
            .x_label_every(3)
            .into_element()
    });
    // About two round steps, written by the formatter.
    for tick in ["0 W", "200 W", "400 W"] {
        h.host.by_text(tick);
    }
    labelled(&h, "Power: 100 W, 250 W, 400 W, 300 W, 200 W, 150 W");
    // Every third x label.
    let shown: Vec<bool> = ["0h", "4h", "8h", "12h", "16h", "20h"]
        .iter()
        .map(|l| h.host.find(|n| n.text == *l).is_some())
        .collect();
    assert_eq!(shown, [true, false, false, true, false, false]);
}

#[test]
fn uneven_ticks_sit_on_their_grid_lines() {
    let h = show(Theme::light, |_, _| {
        ui::line_chart()
            .series("pH", [6.5, 7.2, 8.1])
            .y_ticks([0., 6., 7., 14.])
            .height(114.)
            .into_element()
    });
    // Each label (a 14 pt line) centered on its grid line: 100 pt of plot,
    // 14 at the top, 7 halfway (50), 6 at 50 + 100 / 14, 0 at the bottom.
    let margin = |t: &str| {
        node(&h, h.host.by_text(t))
            .style
            .clone()
            .unwrap()
            .margin
            .top
    };
    assert_eq!(margin("14"), 0.);
    assert_eq!(margin("7"), 50. - 14.);
    assert!((margin("6") - (100. / 14.0 - 14.)).abs() < 1e-3);
    assert!((margin("0") - (100. - (50. + 100. / 14.) - 14.)).abs() < 1e-3);
}

#[test]
fn grouped_bars_one_per_series_in_each_category() {
    let mut h = show(Theme::light, |p, cx| {
        ui::bar_chart()
            .labels(["Q1", "Q2"])
            .series("2025", [3., 4.])
            .series("2026", [5., f64::NAN])
            .selected(p.on.then_some(1))
            .on_select(cx.listener(|p, e: &ChangeEvent, _| {
                p.log.push(format!("group {}", e.index()));
                p.on = true;
            }))
            .into_element()
    });
    let q1 = labelled(&h, "Q1: 2025: 3, 2026: 5");
    assert_eq!(node(&h, q1).a11y.role, Some(Role::Button));
    labelled(&h, "Q2: 2025: 4, 2026: –");
    // Two bars side by side in each cell.
    let row = node(&h, node(&h, q1).children[0]).clone();
    assert_eq!(row.children.len(), 2);
    let t = Theme::light();
    let bg = |i: usize| node(&h, row.children[i]).style.clone().unwrap().background;
    assert_eq!(
        (bg(0), bg(1)),
        (Some(t.chart_color(0)), Some(t.chart_color(1)))
    );
    // A legend names the series.
    h.host.by_text("2025");
    h.host.by_text("2026");
    // Tapping a group selects it: the read-out shows its values.
    let q2 = labelled(&h, "Q2: 2025: 4, 2026: –");
    h.dispatch(UiEvent::Click(q2));
    assert_eq!(log(&h), ["group 1"]);
    // The read-out: its title and values (the x label is there too).
    assert_eq!(
        h.host
            .mounted()
            .iter()
            .filter(|id| node(&h, **id).text == "Q2")
            .count(),
        2
    );
    h.host.by_text("–");
}

#[test]
fn line_charts_select_the_nearest_point_and_show_a_read_out() {
    let mut h = show(Theme::light, |p, cx| {
        ui::line_chart()
            .series("Sales", [3., 5., 4.])
            .series("Costs", [1., 2., 2.5])
            .labels(["Mon", "Tue", "Wed"])
            .unit(" k€")
            .selected(p.on.then_some(1))
            .on_select(cx.listener(|p, e: &ChangeEvent, _| {
                p.log.push(format!("point {}", e.index()));
                p.on = true;
            }))
            .into_element()
    });
    // One tap target per point, spoken with every series' value.
    let tue = labelled(&h, "Tue: Sales: 5 k€, Costs: 2 k€");
    assert_eq!(node(&h, tue).a11y.role, Some(Role::Button));
    let before = drawings(&h)[0].paths.len();
    h.dispatch(UiEvent::Click(tue));
    assert_eq!(log(&h), ["point 1"]);
    // The read-out: a rule and two dots (each two paths) in the drawing,
    // a bubble with the values.
    assert_eq!(drawings(&h)[0].paths.len(), before + 5);
    h.host.by_text("5 k€");
    let bubble = h
        .host
        .find(|n| {
            n.style
                .as_ref()
                .is_some_and(|s| s.translate_percent == (0., -100.))
        })
        .expect("a bubble right of the point, at the bottom (the points are high)");
    assert_eq!(
        node(&h, bubble).style.clone().unwrap().inset.left,
        prism_core::Length::Fraction(0.5)
    );
}

#[test]
fn grouped_bars_and_reference_lines_in_widgets() {
    use prism_core::surfaces::{SNode, snapshot_one};
    fn shapes(n: &SNode, out: &mut Vec<String>) {
        if n.kind == NodeKind::Shape {
            out.push(n.text.clone());
        }
        n.children.iter().for_each(|c| shapes(c, out));
    }
    let mut app = App::new();
    ui::set_theme(&mut app, Theme::light());
    let node = snapshot_one(
        &mut app,
        ui::bar_chart()
            .labels(["a", "b"])
            .series("x", [1., 2.])
            .series("y", [3., 4.])
            .reference_line(2.5, "Target"),
    );
    let mut found = Vec::new();
    shapes(&node, &mut found);
    assert_eq!(found.len(), 1);
    let d = Drawing::decode(&found[0]);
    // Four bars and the dashed reference line, all in the one shape.
    assert_eq!(d.paths.iter().filter(|p| p.fill.is_some()).count(), 4);
    assert!(d.paths.iter().any(|p| p.dash.is_some()));
}
