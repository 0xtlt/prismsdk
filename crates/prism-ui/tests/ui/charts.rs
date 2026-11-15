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
    // One point sits in the middle.
    assert_eq!(d[0].paths[0].segments, [Segment::Move(0.5, 0.5)]);
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
