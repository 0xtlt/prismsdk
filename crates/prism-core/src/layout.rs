//! Grid tracks and scroll axes: the layout vocabulary beyond flexbox.
//!
//! ```ignore
//! // As many 172 pt columns as fit, sharing the leftover space (CSS
//! // `repeat(auto-fill, minmax(172px, 1fr))`): cards in rows of equal height.
//! div().grid().grid_cols([auto_fill(minmax(172., fr(1.)))]).gap(12.)
//!     .children(cards)
//! // A sidebar and a content column; a wide card spanning two columns.
//! div().grid().grid_cols([px(240.).into(), fr(1.)])
//! card().col_span(2)
//! ```
//!
//! The web emits CSS grid; native backends lay grids out with the same
//! algorithm (taffy's CSS grid), so both agree.

use crate::style::Length;

/// The size of one grid column or row (CSS `<track-size>`), or a repetition
/// of tracks (`repeat()`).
#[derive(Clone, Debug, PartialEq)]
pub enum GridTrack {
    /// Fixed size, points (`172px`).
    Px(f32),
    /// Fraction of the grid's width (or height), `1.0` is 100%.
    Percent(f32),
    /// A share of the free space (`1fr`).
    Fr(f32),
    /// Sized by its content (`auto`).
    Auto,
    /// As small as its content can get (`min-content`).
    MinContent,
    /// As wide as its content wants (`max-content`).
    MaxContent,
    /// At least `min`, at most `max` (`minmax(172px, 1fr)`). `min` cannot be
    /// an `Fr`.
    MinMax(Box<GridTrack>, Box<GridTrack>),
    /// `tracks` repeated (`repeat(3, 1fr)`, `repeat(auto-fill, …)`).
    Repeat(Repetition, Vec<GridTrack>),
}

/// How many times a [`GridTrack::Repeat`] repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repetition {
    /// A fixed number of times.
    Count(u16),
    /// As many times as fit; empty tracks keep their size (`auto-fill`).
    AutoFill,
    /// As many times as fit; empty tracks collapse, so items stretch
    /// (`auto-fit`).
    AutoFit,
}

impl From<f32> for GridTrack {
    fn from(v: f32) -> Self {
        GridTrack::Px(v)
    }
}

impl From<f64> for GridTrack {
    fn from(v: f64) -> Self {
        GridTrack::Px(v as f32)
    }
}

impl From<i32> for GridTrack {
    fn from(v: i32) -> Self {
        GridTrack::Px(v as f32)
    }
}

impl From<Length> for GridTrack {
    fn from(l: Length) -> Self {
        match l {
            Length::Auto => GridTrack::Auto,
            Length::Px(v) => GridTrack::Px(v),
            Length::Fraction(f) => GridTrack::Percent(f),
        }
    }
}

impl GridTrack {
    /// The same track with every point size multiplied by `factor` (zoom).
    pub fn scaled(&self, factor: f32) -> GridTrack {
        match self {
            GridTrack::Px(v) => GridTrack::Px(v * factor),
            GridTrack::MinMax(a, b) => {
                GridTrack::MinMax(Box::new(a.scaled(factor)), Box::new(b.scaled(factor)))
            }
            GridTrack::Repeat(r, t) => {
                GridTrack::Repeat(*r, t.iter().map(|t| t.scaled(factor)).collect())
            }
            other => other.clone(),
        }
    }

    /// CSS notation (`minmax(172px,1fr)`).
    pub fn css(&self) -> String {
        match self {
            GridTrack::Px(v) => format!("{v}px"),
            GridTrack::Percent(f) => format!("{}%", f * 100.),
            GridTrack::Fr(f) => format!("{f}fr"),
            GridTrack::Auto => "auto".into(),
            GridTrack::MinContent => "min-content".into(),
            GridTrack::MaxContent => "max-content".into(),
            GridTrack::MinMax(a, b) => format!("minmax({},{})", a.css(), b.css()),
            GridTrack::Repeat(r, t) => {
                let count = match r {
                    Repetition::Count(n) => n.to_string(),
                    Repetition::AutoFill => "auto-fill".into(),
                    Repetition::AutoFit => "auto-fit".into(),
                };
                let tracks: Vec<String> = t.iter().map(GridTrack::css).collect();
                format!("repeat({count},{})", tracks.join(" "))
            }
        }
    }
}

/// A share of the free space: `fr(1.)` is CSS `1fr`.
pub fn fr(share: f32) -> GridTrack {
    GridTrack::Fr(share)
}

/// At least `min`, at most `max`: `minmax(172., fr(1.))`.
pub fn minmax(min: impl Into<GridTrack>, max: impl Into<GridTrack>) -> GridTrack {
    GridTrack::MinMax(Box::new(min.into()), Box::new(max.into()))
}

/// `track` repeated `count` times: `repeat(3, fr(1.))`.
pub fn repeat(count: u16, track: impl Into<GridTrack>) -> GridTrack {
    GridTrack::Repeat(Repetition::Count(count), vec![track.into()])
}

/// As many `track`s as fit (CSS `repeat(auto-fill, …)`): give it a
/// definite minimum, `auto_fill(minmax(172., fr(1.)))`.
pub fn auto_fill(track: impl Into<GridTrack>) -> GridTrack {
    GridTrack::Repeat(Repetition::AutoFill, vec![track.into()])
}

/// Like [`auto_fill`], but empty columns collapse so the items stretch
/// across the whole row (CSS `repeat(auto-fit, …)`).
pub fn auto_fit(track: impl Into<GridTrack>) -> GridTrack {
    GridTrack::Repeat(Repetition::AutoFit, vec![track.into()])
}

/// Which way a [`scroll`](crate::scroll) view scrolls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollAxis {
    /// Top to bottom (the default).
    #[default]
    Vertical,
    /// Leading to trailing: a row of chips, a wide table on a phone.
    Horizontal,
}
