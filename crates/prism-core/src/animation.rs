//! Motion helpers: transforms, enter/exit animations, loops and animated
//! updates, on every backend (UIKit / AppKit animations, Android property
//! animators, CSS and the Web Animations API, GTK, Composition on Windows).
//!
//! ```ignore
//! div()
//!     .enter(Pose::slide(0., 16.))      // fades in from 16 pt below
//!     .exit(Pose::pop())                // shrinks and fades out when removed
//!     .stagger(index, 40)               // each row 40 ms after the previous one
//!     .spring(450);                     // and every change animates
//!
//! image("heart.fill").repeat(Pose::new().scale(1.15), Transition::ease(700));
//! image("arrow.clockwise").repeat_from_start(Pose::new().rotate(360.), Transition::linear(900)); // spins
//! div().repeat(Pose::new().scale_y(0.1), Transition::ease(120));   // blinks
//!
//! cx.with_animation(Transition::spring(400), |cx| store.update(cx, |s, _| s.expanded ^= true));
//! ```
//!
//! Everything is skipped when the user asked for less motion.

use crate::style::Transition;

/// A visual offset, scale and rotation around the center. It never changes
/// the layout: neighbours stay where they are (like CSS `transform`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Points, right (trailing in right-to-left layouts).
    pub x: f32,
    /// Points, down.
    pub y: f32,
    /// Size factor, `1.0` is the element's own size.
    pub scale: f32,
    /// Degrees, clockwise.
    pub rotate: f32,
    /// Horizontal size factor, multiplied with `scale`.
    pub scale_x: f32,
    /// Vertical size factor, multiplied with `scale`.
    pub scale_y: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    /// No offset, scale 1, no rotation.
    pub const IDENTITY: Transform = Transform {
        x: 0.,
        y: 0.,
        scale: 1.,
        rotate: 0.,
        scale_x: 1.,
        scale_y: 1.,
    };

    /// Whether this is [`Transform::IDENTITY`].
    pub fn is_identity(&self) -> bool {
        *self == Transform::IDENTITY
    }

    /// The horizontal size factor: `scale × scale_x`.
    pub fn sx(&self) -> f32 {
        self.scale * self.scale_x
    }

    /// The vertical size factor: `scale × scale_y`.
    pub fn sy(&self) -> f32 {
        self.scale * self.scale_y
    }

    /// The `scale(…)` part of a CSS transform (`scale(s)`, or `scale(sx,sy)`
    /// when they differ).
    pub fn css_scale(&self) -> String {
        if self.scale_x == self.scale_y {
            format!("scale({})", self.sx())
        } else {
            format!("scale({},{})", self.sx(), self.sy())
        }
    }

    /// CSS `transform` (`none` for the identity).
    pub fn css(&self) -> String {
        if self.is_identity() {
            return "none".into();
        }
        format!(
            "translate({}px,{}px) rotate({}deg) {}",
            self.x,
            self.y,
            self.rotate,
            self.css_scale()
        )
    }

    /// `t` of the way from `self` to `to` (`t` in `0..=1`), field by field:
    /// angles go through every degree in between, so 0° to 360° is a full
    /// turn (matrices would see no change).
    pub fn lerp(&self, to: Transform, t: f32) -> Transform {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Transform {
            x: l(self.x, to.x),
            y: l(self.y, to.y),
            scale: l(self.scale, to.scale),
            rotate: l(self.rotate, to.rotate),
            scale_x: l(self.scale_x, to.scale_x),
            scale_y: l(self.scale_y, to.scale_y),
        }
    }

    /// The 2D affine matrix `[a, b, c, d, tx, ty]` of this transform around
    /// the origin: scaled, rotated, then offset.
    pub fn matrix(&self) -> [f64; 6] {
        let (sx, sy) = (self.sx() as f64, self.sy() as f64);
        let r = (self.rotate as f64).to_radians();
        [
            sx * r.cos(),
            sx * r.sin(),
            -sy * r.sin(),
            sy * r.cos(),
            self.x as f64,
            self.y as f64,
        ]
    }

    /// The 2D affine matrix `[a, b, c, d, tx, ty]` of this transform around a
    /// `w` × `h` box's center, for platforms whose origin is a corner.
    pub fn matrix_around_center(&self, w: f32, h: f32) -> [f64; 6] {
        let [a, b, c, d, _, _] = self.matrix();
        let (cx, cy) = (w as f64 / 2., h as f64 / 2.);
        // translate(center + offset) · rotate·scale · translate(-center)
        let tx = cx + self.x as f64 - (a * cx + c * cy);
        let ty = cy + self.y as f64 - (b * cx + d * cy);
        [a, b, c, d, tx, ty]
    }
}

/// A visual state to animate from or to. Unset fields keep the element's own
/// value.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pose {
    /// Opacity, `0.0..=1.0`.
    pub opacity: Option<f32>,
    /// Offset `(x, y)` in points.
    pub offset: Option<(f32, f32)>,
    /// Size factor.
    pub scale: Option<f32>,
    /// Rotation in degrees, clockwise.
    pub rotate: Option<f32>,
    /// Horizontal size factor (multiplied with `scale`).
    pub scale_x: Option<f32>,
    /// Vertical size factor (multiplied with `scale`).
    pub scale_y: Option<f32>,
}

impl Pose {
    /// A pose that changes nothing (set fields with the builders).
    pub fn new() -> Self {
        Self::default()
    }
    /// Transparent.
    pub fn fade() -> Self {
        Pose::new().opacity(0.)
    }
    /// Transparent, `x`, `y` points away (`slide(0., 16.)`: from below).
    pub fn slide(x: f32, y: f32) -> Self {
        Pose::fade().offset(x, y)
    }
    /// Transparent and smaller.
    pub fn pop() -> Self {
        Pose::fade().scale(0.85)
    }
    /// Sets the opacity (clamped to `0.0..=1.0`).
    pub fn opacity(mut self, o: f32) -> Self {
        self.opacity = Some(o.clamp(0., 1.));
        self
    }
    /// Sets the offset, in points.
    pub fn offset(mut self, x: f32, y: f32) -> Self {
        self.offset = Some((x, y));
        self
    }
    /// Sets the horizontal offset, keeping the vertical one.
    pub fn x(self, x: f32) -> Self {
        let y = self.offset.map_or(0., |o| o.1);
        self.offset(x, y)
    }
    /// Sets the vertical offset, keeping the horizontal one.
    pub fn y(self, y: f32) -> Self {
        let x = self.offset.map_or(0., |o| o.0);
        self.offset(x, y)
    }
    /// Sets the size factor.
    pub fn scale(mut self, s: f32) -> Self {
        self.scale = Some(s);
        self
    }
    /// Sets the rotation, in degrees clockwise. Animations turn through
    /// every degree in between: `rotate(360.)` looped with
    /// [`repeat_from_start`](crate::Styled::repeat_from_start) spins.
    pub fn rotate(mut self, degrees: f32) -> Self {
        self.rotate = Some(degrees);
        self
    }
    /// Sets the horizontal size factor alone (a bar growing sideways).
    pub fn scale_x(mut self, s: f32) -> Self {
        self.scale_x = Some(s);
        self
    }
    /// Sets the vertical size factor alone (a blink, a bar growing upwards).
    pub fn scale_y(mut self, s: f32) -> Self {
        self.scale_y = Some(s);
        self
    }

    /// `(opacity, transform)` of an element at rest at `opacity`, `rest`.
    pub fn apply(&self, opacity: f32, rest: Transform) -> (f32, Transform) {
        let (x, y) = self.offset.unwrap_or((rest.x, rest.y));
        (
            self.opacity.unwrap_or(opacity),
            Transform {
                x,
                y,
                scale: self.scale.unwrap_or(rest.scale),
                rotate: self.rotate.unwrap_or(rest.rotate),
                scale_x: self.scale_x.unwrap_or(rest.scale_x),
                scale_y: self.scale_y.unwrap_or(rest.scale_y),
            },
        )
    }

    /// Offsets multiplied by `factor` (zoom); `mirror` flips x (right to left).
    pub fn scaled(&self, factor: f32, mirror: bool) -> Pose {
        Pose {
            offset: self
                .offset
                .map(|(x, y)| (x * factor * if mirror { -1. } else { 1. }, y * factor)),
            ..*self
        }
    }
}

/// How an element enters, leaves and loops.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Animations {
    /// Shown from this pose, animated to the element's style.
    pub enter: Option<Pose>,
    /// Animated to this pose when removed, then gone.
    pub exit: Option<Pose>,
    /// Back and forth between the style and this pose, forever.
    pub repeat: Option<(Pose, Transition)>,
    /// The loop starts over from the style instead of reversing (spinners,
    /// a pulse that only grows).
    pub restart: bool,
}

impl Animations {
    /// Whether no enter, exit or loop animation is set.
    pub fn is_empty(&self) -> bool {
        *self == Animations::default()
    }

    /// Offsets multiplied by `factor` (zoom); `mirror` flips x (right to left).
    pub fn scaled(&self, factor: f32, mirror: bool) -> Animations {
        Animations {
            enter: self.enter.map(|p| p.scaled(factor, mirror)),
            exit: self.exit.map(|p| p.scaled(factor, mirror)),
            repeat: self.repeat.map(|(p, t)| (p.scaled(factor, mirror), t)),
            restart: self.restart,
        }
    }
}

/// Enter animations without a transition of their own.
pub const DEFAULT_ENTER: Transition = Transition {
    duration: 0.45,
    curve: crate::style::Curve::Spring,
    delay: 0.,
};

/// Exit animations without a transition of their own.
pub const DEFAULT_EXIT: Transition = Transition {
    duration: 0.22,
    curve: crate::style::Curve::EaseIn,
    delay: 0.,
};

impl crate::App {
    /// Animates every change made by `f` with `t`, for elements without a
    /// transition of their own (SwiftUI's `withAnimation`): layout moves,
    /// colors, opacity, transforms.
    pub fn with_animation<R>(&mut self, t: Transition, f: impl FnOnce(&mut crate::App) -> R) -> R {
        let r = f(self);
        if !self.reduce_motion() {
            self.animation = Some(t);
        }
        r
    }

    pub(crate) fn take_animation(&mut self) -> Option<Transition> {
        self.animation.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_rotates_and_scales_around_the_center() {
        let t = Transform {
            scale: 2.,
            ..Transform::IDENTITY
        };
        // A 10 × 10 box scaled 2× around its center: (5, 5) stays put.
        let [a, _, _, d, tx, ty] = t.matrix_around_center(10., 10.);
        assert_eq!((a * 5. + tx, d * 5. + ty), (5., 5.));
        let r = Transform {
            rotate: 90.,
            ..Transform::IDENTITY
        };
        let [a, b, c, d, tx, ty] = r.matrix_around_center(10., 10.);
        let center = (a * 5. + c * 5. + tx, b * 5. + d * 5. + ty);
        assert!((center.0 - 5.).abs() < 1e-9 && (center.1 - 5.).abs() < 1e-9);
    }

    #[test]
    fn easing_curves() {
        let t = Transition::ease(100);
        assert_eq!((t.ease_at(0.), t.ease_at(1.)), (0., 1.));
        assert!((t.ease_at(0.5) - 0.5).abs() < 1e-3);
        assert!((Transition::linear(100).ease_at(0.3) - 0.3).abs() < 1e-3);
        // Springs overshoot before settling.
        let spring = Transition::spring(100);
        assert!((0..100).any(|i| spring.ease_at(i as f32 / 100.) > 1.));
    }

    #[test]
    fn poses_keep_unset_fields() {
        let rest = Transform {
            x: 3.,
            ..Transform::IDENTITY
        };
        let (o, t) = Pose::new().scale(0.5).apply(0.8, rest);
        assert_eq!((o, t.x, t.scale), (0.8, 3., 0.5));
        assert_eq!(Pose::slide(4., 2.).scaled(2., true).offset, Some((-8., 4.)));
    }
}
