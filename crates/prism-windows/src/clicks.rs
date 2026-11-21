//! Clicks counted in quick series (#159), as Windows counts a double
//! click: the next press on the same view, within the double-click time
//! (`GetDoubleClickTime`) and the double-click rectangle around the last
//! press (`SM_CXDOUBLECLK` × `SM_CYDOUBLECLK`), continues the series
//! (2, then 3…); any other press starts a new one. No Win32 types: tested
//! on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::NodeId;

/// The system's double-click limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    /// The longest time between two presses, in milliseconds.
    pub time: u64,
    /// The rectangle centered on the last press the next one lands in
    /// (the window's units).
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    node: NodeId,
    pointer: u32,
    /// The press's timestamp, in microseconds.
    time: u64,
    /// Where it was in the window.
    at: (f32, f32),
    count: u32,
}

/// The last press and its number in its series.
#[derive(Clone, Debug, Default)]
pub struct Series {
    last: Option<Press>,
}

impl Series {
    /// A press of `pointer` at `time` (µs), at `at` in the window, on view
    /// `node`; returns its number in its series. The views around `node`
    /// hear the same press again: it keeps its count and its view.
    pub fn press(
        &mut self,
        node: NodeId,
        pointer: u32,
        time: u64,
        at: (f32, f32),
        limits: Limits,
    ) -> u32 {
        if let Some(p) = self.last
            && p.pointer == pointer
            && p.time == time
        {
            return p.count;
        }
        let count = match self.last {
            Some(p)
                if p.node == node
                    && time >= p.time
                    && time - p.time <= limits.time.saturating_mul(1000)
                    && (at.0 - p.at.0).abs() * 2. <= limits.width
                    && (at.1 - p.at.1).abs() * 2. <= limits.height =>
            {
                p.count + 1
            }
            _ => 1,
        };
        self.last = Some(Press {
            node,
            pointer,
            time,
            at,
            count,
        });
        count
    }

    /// The number of view `node`'s last press in its series: 1 when
    /// another view was pressed since (or none was).
    pub fn count(&self, node: NodeId) -> u32 {
        self.last.filter(|p| p.node == node).map_or(1, |p| p.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: Limits = Limits {
        time: 500,
        width: 4.,
        height: 4.,
    };

    #[test]
    fn quick_presses_in_place_count_up() {
        let mut s = Series::default();
        assert_eq!(s.press(7, 1, 1_000_000, (10., 10.), LIMITS), 1);
        assert_eq!(s.press(7, 1, 1_300_000, (11., 9.), LIMITS), 2);
        assert_eq!(s.press(7, 1, 1_700_000, (12., 9.), LIMITS), 3);
        assert_eq!(s.count(7), 3);
        assert_eq!(s.count(8), 1);
    }

    #[test]
    fn a_slow_far_or_other_press_starts_again() {
        let mut s = Series::default();
        s.press(7, 1, 0, (10., 10.), LIMITS);
        // Too late.
        assert_eq!(s.press(7, 1, 600_000, (10., 10.), LIMITS), 1);
        // Too far (more than half the rectangle away).
        assert_eq!(s.press(7, 1, 700_000, (13., 10.), LIMITS), 1);
        // Another view.
        assert_eq!(s.press(8, 1, 800_000, (13., 10.), LIMITS), 1);
        assert_eq!(s.count(7), 1);
    }

    #[test]
    fn the_same_press_heard_around_keeps_its_view_and_count() {
        let mut s = Series::default();
        s.press(7, 1, 0, (10., 10.), LIMITS);
        assert_eq!(s.press(7, 1, 200_000, (10., 10.), LIMITS), 2);
        // A clickable view around the pressed one.
        assert_eq!(s.press(3, 1, 200_000, (10., 10.), LIMITS), 2);
        assert_eq!(s.count(7), 2);
        assert_eq!(s.count(3), 1);
    }
}
