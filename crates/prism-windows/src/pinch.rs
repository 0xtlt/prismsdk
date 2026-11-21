//! Two fingers pinching on a view (#133): the touches down on it and how
//! far apart they were at the last report. XAML gives the raw touches
//! (`PointerPressed`, `PointerMoved` per finger); a pinch is the change of
//! their distance, reported as the ratio since the previous move, like
//! GTK's `GestureZoom` and AppKit's `magnifyWithEvent:`. No Win32: tested
//! on every host.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

/// The touches on one view.
#[derive(Clone, Debug, Default)]
pub struct Pinch {
    /// Touches down (pointer id, position), oldest first.
    touches: Vec<(u32, (f32, f32))>,
    /// The two first touches' distance at the last report, while pinching.
    last: Option<f32>,
}

/// What a touch changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// A second finger came down: the pinch starts, centered there.
    Start((f32, f32)),
    /// The fingers moved: scale by this since the last step, around there.
    Move(f32, (f32, f32)),
    /// A finger of the pinch lifted.
    End((f32, f32)),
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn center(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) / 2., (a.1 + b.1) / 2.)
}

impl Pinch {
    /// Two fingers are down: a pinch is going on.
    pub fn pinching(&self) -> bool {
        self.last.is_some()
    }

    /// Whether touch `id` is one of the pinch's.
    pub fn has(&self, id: u32) -> bool {
        self.touches.iter().any(|(t, _)| *t == id)
    }

    /// The middle of the two first fingers.
    pub fn center(&self) -> Option<(f32, f32)> {
        match self.touches[..] {
            [(_, a), (_, b), ..] => Some(center(a, b)),
            _ => None,
        }
    }

    /// The first finger still down (id, position).
    pub fn first(&self) -> Option<(u32, (f32, f32))> {
        self.touches.first().copied()
    }

    /// Touch `id` went down at `at`.
    pub fn down(&mut self, id: u32, at: (f32, f32)) -> Option<Step> {
        self.touches.retain(|(t, _)| *t != id);
        self.touches.push((id, at));
        if self.last.is_some() || self.touches.len() < 2 {
            return None;
        }
        let (a, b) = (self.touches[0].1, self.touches[1].1);
        self.last = Some(distance(a, b).max(1.));
        Some(Step::Start(center(a, b)))
    }

    /// Touch `id` moved to `at`.
    pub fn moved(&mut self, id: u32, at: (f32, f32)) -> Option<Step> {
        let i = self.touches.iter().position(|(t, _)| *t == id)?;
        self.touches[i].1 = at;
        let last = self.last?;
        if i > 1 {
            return None;
        }
        let (a, b) = (self.touches[0].1, self.touches[1].1);
        let now = distance(a, b).max(1.);
        if (now - last).abs() < 0.5 {
            return None;
        }
        self.last = Some(now);
        Some(Step::Move(now / last, center(a, b)))
    }

    /// Touch `id` lifted (or was cancelled).
    pub fn up(&mut self, id: u32) -> Option<Step> {
        let i = self.touches.iter().position(|(t, _)| *t == id)?;
        let ended = self.last.is_some() && i < 2;
        let at = if self.touches.len() >= 2 {
            center(self.touches[0].1, self.touches[1].1)
        } else {
            self.touches[i].1
        };
        self.touches.remove(i);
        if ended {
            self.last = None;
            return Some(Step::End(at));
        }
        None
    }
}

/// The scale of a Ctrl + wheel turn of `raw` units (a notch is 120;
/// precision touchpads send their pinches this way, in smaller steps):
/// the 3D scenes' wheel zoom, about 1.2 per notch.
pub fn wheel_scale(raw: f32) -> f32 {
    (raw / 600.).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_fingers_spreading_scale_up_until_one_lifts() {
        let mut p = Pinch::default();
        assert_eq!(p.down(1, (100., 100.)), None);
        assert!(!p.pinching());
        assert_eq!(p.down(2, (200., 100.)), Some(Step::Start((150., 100.))));
        // 100 apart, then 200: twice as large, around the middle.
        assert_eq!(p.moved(2, (300., 100.)), Some(Step::Move(2., (200., 100.))));
        // Then back to 100: half.
        assert_eq!(
            p.moved(1, (200., 100.)),
            Some(Step::Move(0.5, (250., 100.)))
        );
        // Barely moving reports nothing.
        assert_eq!(p.moved(1, (200.2, 100.)), None);
        assert_eq!(p.up(2), Some(Step::End((250.1, 100.))));
        assert!(!p.pinching());
        // The last finger lifting ends nothing more.
        assert_eq!(p.up(1), None);
    }

    #[test]
    fn a_third_finger_does_not_change_the_pinch() {
        let mut p = Pinch::default();
        p.down(1, (0., 0.));
        p.down(2, (10., 0.));
        assert_eq!(p.down(3, (50., 50.)), None);
        assert_eq!(p.moved(3, (60., 60.)), None);
        assert_eq!(p.up(3), None);
        assert!(p.pinching());
    }

    #[test]
    fn a_wheel_notch_zooms_like_the_scenes_wheel() {
        assert!((wheel_scale(120.) - 0.2f32.exp()).abs() < 1e-6);
        assert!((wheel_scale(-120.) * wheel_scale(120.) - 1.).abs() < 1e-6);
    }
}
