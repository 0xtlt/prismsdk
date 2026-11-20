//! Prism's animations on XAML elements.
//!
//! - Opacity and transforms (style transitions, enter and exit poses,
//!   loops) are Composition keyframe animations on the element's own
//!   facades (`Opacity`, `Translation`, `Rotation`, `Scale`), started with
//!   `UIElement::StartAnimation`: they run on the compositor, off the UI
//!   thread. Each curve is the Transition's own cubic Bézier.
//! - Frame changes (`set_frame_animated`) step on each frame XAML renders
//!   (`CompositionTarget::Rendering`), like prism-linux's ticks: an
//!   element's size is layout, which Composition cannot animate.
//!
//! An animation interrupted by another one starts from where the first one
//! was (each flight is remembered: from, to, start). prism-native turns
//! transitions off for « Animation effects » off (reduced motion).

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use prism_core::{Curve, NodeId, Style, Transition};
use windows::Foundation::{EventHandler, TimeSpan};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows_core::{IInspectable, Interface, Result};
use windows_numerics::{Vector2, Vector3};

use crate::scene::Rect;
use crate::winui::bindings::Microsoft::UI::Composition::{
    AnimationDelayBehavior, AnimationDirection, AnimationIterationBehavior,
    CompositionEasingFunction, CompositionObject, Compositor, ICompositionAnimationBase,
    ScalarKeyFrameAnimation, Vector3KeyFrameAnimation,
};
use crate::winui::h;
use crate::winui::xaml::Media::CompositionTarget;
use crate::winui::xaml::UIElement;

/// What an element shows of a style: its opacity and transform
/// (translation, rotation around the center, scale), DIPs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visual {
    pub opacity: f32,
    pub tx: f32,
    pub ty: f32,
    pub rotate: f32,
    pub sx: f32,
    pub sy: f32,
}

impl Visual {
    /// `s` on a `w` × `h` box moved down by `sticky`.
    pub fn of(s: &Style, w: f32, h: f32, sticky: f32) -> Visual {
        let t = s.transform;
        let (px, py) = s.translate_percent;
        Visual {
            opacity: s.opacity.clamp(0., 1.),
            tx: t.x + px * w / 100.,
            ty: t.y + py * h / 100. + sticky,
            rotate: t.rotate,
            sx: t.sx(),
            sy: t.sy(),
        }
    }

    pub fn lerp(a: Visual, b: Visual, p: f32) -> Visual {
        let l = |x: f32, y: f32| x + (y - x) * p;
        Visual {
            // Springs overshoot: opacity stays in range.
            opacity: l(a.opacity, b.opacity).clamp(0., 1.),
            tx: l(a.tx, b.tx),
            ty: l(a.ty, b.ty),
            rotate: l(a.rotate, b.rotate),
            sx: l(a.sx, b.sx),
            sy: l(a.sy, b.sy),
        }
    }

    fn translation(&self) -> Vector3 {
        Vector3 {
            X: self.tx,
            Y: self.ty,
            Z: 0.,
        }
    }

    fn scale(&self) -> Vector3 {
        Vector3 {
            X: self.sx,
            Y: self.sy,
            Z: 1.,
        }
    }
}

/// Sets what `e` shows to `v` around the center of a `w` × `h` box,
/// leaving alone what is already so (a running animation keeps going).
pub fn set_visual(e: &UIElement, v: Visual, w: f32, h: f32) -> Result<()> {
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
    if !near(e.Opacity()? as f32, v.opacity) {
        e.SetOpacity(v.opacity as f64)?;
    }
    let c = e.CenterPoint()?;
    if !near(c.X, w / 2.) || !near(c.Y, h / 2.) {
        e.SetCenterPoint(Vector3 {
            X: w / 2.,
            Y: h / 2.,
            Z: 0.,
        })?;
    }
    let t = e.Translation()?;
    if !near(t.X, v.tx) || !near(t.Y, v.ty) {
        e.SetTranslation(v.translation())?;
    }
    if !near(e.Rotation()?, v.rotate) {
        e.SetRotation(v.rotate)?;
    }
    let s = e.Scale()?;
    if !near(s.X, v.sx) || !near(s.Y, v.sy) {
        e.SetScale(v.scale())?;
    }
    Ok(())
}

/// One element's animations: opacity, translation, rotation, scale.
struct Flight {
    element: UIElement,
    from: Visual,
    to: Visual,
    start: Instant,
    t: Transition,
    started: Vec<ICompositionAnimationBase>,
}

impl Flight {
    /// What it shows now.
    fn now(&self) -> Visual {
        let elapsed = self.start.elapsed().as_secs_f32() - self.t.delay;
        if elapsed <= 0. {
            return self.from;
        }
        let p = elapsed / self.t.duration.max(0.001);
        if p >= 1. {
            return self.to;
        }
        Visual::lerp(self.from, self.to, self.t.ease_at(p))
    }

    fn done(&self) -> bool {
        self.start.elapsed().as_secs_f32() >= self.t.delay + self.t.duration
    }

    fn stop(&self) {
        for a in &self.started {
            let _ = self.element.StopAnimation(a);
        }
    }
}

/// A frame moving from `from` to `to`.
struct FrameFlight {
    from: Rect,
    to: Rect,
    start: Instant,
    t: Transition,
}

impl FrameFlight {
    fn at(&self, now: Instant) -> (Rect, bool) {
        let elapsed = now.duration_since(self.start).as_secs_f32() - self.t.delay;
        if elapsed <= 0. {
            return (self.from, false);
        }
        let p = elapsed / self.t.duration.max(0.001);
        if p >= 1. {
            return (self.to, true);
        }
        (lerp_rect(self.from, self.to, self.t.ease_at(p)), false)
    }
}

pub fn lerp_rect(a: Rect, b: Rect, p: f32) -> Rect {
    let l = |x: f32, y: f32| x + (y - x) * p;
    Rect::new(
        l(a.x, b.x),
        l(a.y, b.y),
        l(a.w, b.w).max(0.),
        l(a.h, b.h).max(0.),
    )
}

type Later = Box<dyn FnOnce()>;
type Step = Box<dyn Fn(NodeId, Rect, bool)>;

thread_local! {
    static COMPOSITOR: RefCell<Option<Compositor>> = const { RefCell::new(None) };
    static FLIGHTS: RefCell<HashMap<NodeId, Flight>> = RefCell::new(HashMap::new());
    static LOOPS: RefCell<HashMap<NodeId, Flight>> = RefCell::new(HashMap::new());
    static FRAMES: RefCell<HashMap<NodeId, FrameFlight>> = RefCell::new(HashMap::new());
    /// What places a node at a frame (set by `widgets.rs`).
    static STEP: RefCell<Option<Step>> = const { RefCell::new(None) };
    static RENDERING: RefCell<Option<i64>> = const { RefCell::new(None) };
    static LATER: RefCell<HashMap<usize, Later>> = RefCell::new(HashMap::new());
}

/// The UI thread's compositor (not from an element: an element whose
/// Composition visual was handed out cannot use its facades anymore).
fn compositor() -> Result<Compositor> {
    if let Some(c) = COMPOSITOR.with(|c| c.borrow().clone()) {
        return Ok(c);
    }
    let c = CompositionTarget::GetCompositorForCurrentThread()?;
    COMPOSITOR.with(|cell| *cell.borrow_mut() = Some(c.clone()));
    Ok(c)
}

fn span(secs: f32) -> TimeSpan {
    TimeSpan {
        Duration: (secs.max(0.) as f64 * 10_000_000.) as i64,
    }
}

fn easing(c: &Compositor, t: Transition) -> Result<CompositionEasingFunction> {
    if t.curve == Curve::Linear {
        return CompositionEasingFunction::CreateLinearEasingFunction(c)?.cast();
    }
    let (x1, y1, x2, y2) = t.bezier();
    CompositionEasingFunction::CreateCubicBezierEasingFunction(
        c,
        Vector2 { X: x1, Y: y1 },
        Vector2 { X: x2, Y: y2 },
    )?
    .cast()
}

/// A loop: back and forth (`Alternate`), or from the start each time.
#[derive(Clone, Copy, PartialEq)]
enum Repeat {
    Once,
    Swing,
    Restart,
}

/// Keyframe animations from `from` to `to` on what differs, started on `e`.
fn start(
    e: &UIElement,
    from: Visual,
    to: Visual,
    t: Transition,
    repeat: Repeat,
) -> Result<Vec<ICompositionAnimationBase>> {
    let c = compositor()?;
    let ease = easing(&c, t)?;
    let mut out = Vec::new();
    let timing = |a: &CompositionObject| -> Result<()> {
        let k: crate::winui::bindings::Microsoft::UI::Composition::KeyFrameAnimation = a.cast()?;
        k.SetDuration(span(t.duration.max(0.001)))?;
        if t.delay > 0. {
            k.SetDelayTime(span(t.delay))?;
            k.SetDelayBehavior(AnimationDelayBehavior::SetInitialValueBeforeDelay)?;
        }
        if repeat != Repeat::Once {
            k.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
            if repeat == Repeat::Swing {
                k.SetDirection(AnimationDirection::Alternate)?;
            }
        }
        Ok(())
    };
    let mut scalar = |name: &str, a: f32, b: f32| -> Result<()> {
        if (a - b).abs() < 1e-4 {
            return Ok(());
        }
        let k: ScalarKeyFrameAnimation = c.CreateScalarKeyFrameAnimation()?;
        k.InsertKeyFrame(0., a)?;
        k.InsertKeyFrameWithEasingFunction(1., b, &ease)?;
        timing(&k.cast()?)?;
        k.SetTarget(&h(name))?;
        let base: ICompositionAnimationBase = k.cast()?;
        e.StartAnimation(&base)?;
        out.push(base);
        Ok(())
    };
    scalar("Opacity", from.opacity, to.opacity)?;
    scalar("Rotation", from.rotate, to.rotate)?;
    let mut vector = |name: &str, a: Vector3, b: Vector3| -> Result<()> {
        if (a.X - b.X).abs() < 1e-4 && (a.Y - b.Y).abs() < 1e-4 {
            return Ok(());
        }
        let k: Vector3KeyFrameAnimation = c.CreateVector3KeyFrameAnimation()?;
        k.InsertKeyFrame(0., a)?;
        k.InsertKeyFrameWithEasingFunction(1., b, &ease)?;
        timing(&k.cast()?)?;
        k.SetTarget(&h(name))?;
        let base: ICompositionAnimationBase = k.cast()?;
        e.StartAnimation(&base)?;
        out.push(base);
        Ok(())
    };
    vector("Translation", from.translation(), to.translation())?;
    vector("Scale", from.scale(), to.scale())?;
    Ok(out)
}

/// What node `id` shows now: its running animation's current value, else
/// `rest`.
pub fn current(id: NodeId, rest: Visual) -> Visual {
    FLIGHTS.with(|f| {
        f.borrow()
            .get(&id)
            .filter(|f| !f.done())
            .map_or(rest, Flight::now)
    })
}

/// Animates node `id` (element `e`, a `w` × `h` box) from what it shows
/// to `to`, already set as its value.
pub fn animate(id: NodeId, e: &UIElement, from: Visual, to: Visual, t: Transition) {
    let from = current(id, from);
    if let Some(old) = FLIGHTS.with(|f| f.borrow_mut().remove(&id)) {
        old.stop();
    }
    if from == to {
        return;
    }
    match start(e, from, to, t, Repeat::Once) {
        Ok(started) => FLIGHTS.with(|f| {
            f.borrow_mut().insert(
                id,
                Flight {
                    element: e.clone(),
                    from,
                    to,
                    start: Instant::now(),
                    t,
                    started,
                },
            );
        }),
        Err(e) => crate::win::log!("animation of node {id}: {e}"),
    }
}

/// Stops node `id`'s running animation (its value shows at once).
pub fn settle(id: NodeId) {
    if let Some(f) = FLIGHTS.with(|f| f.borrow_mut().remove(&id)) {
        f.stop();
    }
}

/// Loops node `id` between `rest` and `to` until called with `None`.
pub fn repeat(id: NodeId, e: &UIElement, rest: Visual, to: Option<(Visual, Transition, bool)>) {
    if let Some(old) = LOOPS.with(|l| l.borrow_mut().remove(&id)) {
        old.stop();
    }
    let Some((to, t, restart)) = to else {
        return;
    };
    // Loops ease in and out: a spring would overshoot every swing.
    let t = match (t.curve, restart) {
        (Curve::Spring, _) | (_, false) => Transition {
            curve: Curve::EaseInOut,
            ..t
        },
        _ => t,
    };
    let mode = if restart {
        Repeat::Restart
    } else {
        Repeat::Swing
    };
    match start(e, rest, to, t, mode) {
        Ok(started) => LOOPS.with(|l| {
            l.borrow_mut().insert(
                id,
                Flight {
                    element: e.clone(),
                    from: rest,
                    to,
                    start: Instant::now(),
                    t,
                    started,
                },
            );
        }),
        Err(e) => crate::win::log!("loop of node {id}: {e}"),
    }
}

/// Animates `e` from `from` to `to` once (an overlay's own motion: not
/// tracked).
pub fn run(e: &UIElement, from: Visual, to: Visual, t: Transition) {
    if let Err(err) = start(e, from, to, t, Repeat::Once) {
        crate::win::log!("animation: {err}");
    }
}

/// Animates `e` (a still copy of a leaving node) to `to`, then runs `done`.
pub fn leave(
    e: &UIElement,
    from: Visual,
    to: Visual,
    t: Transition,
    done: impl FnOnce() + 'static,
) {
    if let Err(err) = start(e, from, to, t, Repeat::Once) {
        crate::win::log!("exit animation: {err}");
    }
    after(
        Duration::from_secs_f32(t.delay + t.duration + 0.05),
        Box::new(done),
    );
}

/// Node `id` is gone: its animations with it.
pub fn forget(id: NodeId) {
    FLIGHTS.with(|f| f.borrow_mut().remove(&id));
    LOOPS.with(|l| l.borrow_mut().remove(&id));
    FRAMES.with(|f| f.borrow_mut().remove(&id));
}

// --- Frames ------------------------------------------------------------------

/// How the ticks place a node at a frame: `(id, frame, last)`.
pub fn set_step(step: impl Fn(NodeId, Rect, bool) + 'static) {
    STEP.with(|s| *s.borrow_mut() = Some(Box::new(step)));
}

/// Where node `id` is drawn now (its moving frame), if it moves.
pub fn frame_now(id: NodeId) -> Option<Rect> {
    FRAMES.with(|f| f.borrow().get(&id).map(|f| f.at(Instant::now()).0))
}

/// Moves node `id` from `from` to `to` frame by frame.
pub fn move_frame(id: NodeId, from: Rect, to: Rect, t: Transition) {
    let from = frame_now(id).unwrap_or(from);
    FRAMES.with(|f| {
        f.borrow_mut().insert(
            id,
            FrameFlight {
                from,
                to,
                start: Instant::now(),
                t,
            },
        )
    });
    tick_on();
}

/// Node `id` was placed without animation: its move stops.
pub fn stop_frame(id: NodeId) -> bool {
    FRAMES.with(|f| f.borrow_mut().remove(&id).is_some())
}

fn tick_on() {
    if RENDERING.with(|r| r.borrow().is_some()) {
        return;
    }
    let handler = EventHandler::<IInspectable>::new(|_, _| {
        tick();
        Ok(())
    });
    match CompositionTarget::Rendering(&handler) {
        Ok(token) => RENDERING.with(|r| *r.borrow_mut() = Some(token)),
        Err(e) => crate::win::log!("frame animations: {e}"),
    }
}

fn tick() {
    let now = Instant::now();
    let steps: Vec<(NodeId, Rect, bool)> = FRAMES.with(|f| {
        f.borrow()
            .iter()
            .map(|(id, fl)| {
                let (r, last) = fl.at(now);
                (*id, r, last)
            })
            .collect()
    });
    STEP.with(|s| {
        if let Some(step) = s.borrow().as_ref() {
            for (id, r, last) in &steps {
                step(*id, *r, *last);
            }
        }
    });
    FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        for (id, _, last) in &steps {
            if *last {
                f.remove(id);
            }
        }
    });
    if FRAMES.with(|f| f.borrow().is_empty())
        && let Some(token) = RENDERING.with(|r| r.borrow_mut().take())
    {
        let _ = CompositionTarget::RemoveRendering(token);
    }
}

// --- Later -------------------------------------------------------------------

unsafe extern "system" fn fire(_: HWND, _: u32, id: usize, _: u32) {
    unsafe {
        let _ = KillTimer(None, id);
    }
    if let Some(f) = LATER.with(|l| l.borrow_mut().remove(&id)) {
        f();
    }
}

/// Runs `f` on the UI thread after `delay` (a thread timer: the message
/// loop runs it).
pub fn after(delay: Duration, f: Later) {
    let ms = delay.as_millis().clamp(1, u32::MAX as u128) as u32;
    let id = unsafe { SetTimer(None, 0, ms, Some(fire)) };
    if id == 0 {
        f();
        return;
    }
    LATER.with(|l| l.borrow_mut().insert(id, f));
}

// --- The window's nodes ------------------------------------------------------

thread_local! {
    /// Nodes leaving with an exit animation, and their descendants: their
    /// elements stay where they are until it ends.
    static GHOSTS: RefCell<std::collections::HashSet<NodeId>> = RefCell::default();
}

/// Whether node `id` belongs to a leaving node's still copy (its element
/// must stay in place); forgets it.
pub fn take_ghost(id: NodeId) -> bool {
    GHOSTS.with(|g| g.borrow_mut().remove(&id))
}

impl crate::widgets::Win {
    fn visual(&self, id: NodeId) -> Option<Visual> {
        let n = self.scene.get(id)?;
        let sticky = self.scene.sticky_offset(n);
        Some(Visual::of(&n.style, n.frame.w, n.frame.h, sticky))
    }

    /// `Widgets::set_style_animated`: the style at once, its opacity and
    /// transform animated from what is shown.
    pub fn style_animated(&mut self, id: NodeId, style: &Style, t: Transition) {
        let from = self.visual(id).map(|v| current(id, v));
        prism_native::Widgets::set_style(self, &id, style);
        let (Some(from), Some(to), Some(e)) = (from, self.visual(id), self.outer(id)) else {
            return;
        };
        animate(id, &e, from, to, t);
    }

    /// `Widgets::set_frame_animated`: moves and resizes frame by frame.
    pub fn frame_animated(&mut self, id: NodeId, to: Rect, t: Transition) {
        let Some(from) = self.scene.get(id).map(|n| n.frame) else {
            return;
        };
        let from = frame_now(id).unwrap_or(from);
        prism_native::Widgets::set_frame(self, &id, to.x, to.y, to.w, to.h);
        self.place_at(id, from);
        set_step(|id, r, _| {
            crate::imp::with_state(|s| s.renderer.widgets.place_at(id, r));
        });
        move_frame(id, from, to, t);
    }

    /// Draws node `id` at frame `r` (its layout frame stays).
    pub fn place_at(&mut self, id: NodeId, r: Rect) {
        let (Some(n), Some(v)) = (self.scene.get(id), self.views.get_mut(&id)) else {
            return;
        };
        let sticky = self.scene.sticky_offset(n);
        let mut at = n.clone();
        at.frame = r;
        if let Err(e) = v.place(&at, &self.palette, sticky) {
            crate::win::log!("frame of node {id}: {e}");
        }
    }

    /// `Widgets::animate_exit`: the node's elements stay in its parent,
    /// above the other children, animate to `to` and go.
    pub fn exit_animated(&mut self, parent: NodeId, id: NodeId, to: &Style, t: Transition) {
        let (Some(from), Some(e)) = (self.visual(id), self.outer(id)) else {
            return;
        };
        let Some(n) = self.scene.get(id) else { return };
        if n.frame.w <= 0. || n.frame.h <= 0. {
            return;
        }
        let (w, h, sticky) = (n.frame.w, n.frame.h, self.scene.sticky_offset(n));
        let Some(p) = self.views.get_mut(&parent) else {
            return;
        };
        let Some(i) = p.children.iter().position(|c| *c == id) else {
            return;
        };
        let Ok(coll) = p.kids.Children() else { return };
        if coll.RemoveAt(i as u32).is_err() {
            return;
        }
        p.children.remove(i);
        if coll.Append(&e).is_err() {
            return;
        }
        // Its descendants stay in it while it leaves.
        let mut stack = vec![id];
        GHOSTS.with(|g| {
            let mut g = g.borrow_mut();
            while let Some(c) = stack.pop() {
                g.insert(c);
                if let Some(cn) = self.scene.get(c) {
                    stack.extend(cn.children.iter().copied());
                }
            }
        });
        settle(id);
        let from = current(id, from);
        let to = Visual::of(to, w, h, sticky);
        let ghost = e.clone();
        leave(&e, from, to, t, move || {
            let _ = crate::view::detach(&ghost);
        });
    }

    /// `Widgets::set_repeat`.
    pub fn repeat_style(
        &mut self,
        id: NodeId,
        style: &Style,
        r: Option<(prism_core::Pose, Transition)>,
    ) {
        let Some(e) = self.outer(id) else { return };
        let (w, h, sticky) = self.scene.get(id).map_or((0., 0., 0.), |n| {
            (n.frame.w, n.frame.h, self.scene.sticky_offset(n))
        });
        let rest = Visual::of(style, w, h, sticky);
        let to = r.map(|(pose, t)| {
            let (opacity, transform) = pose.apply(style.opacity, style.transform);
            let posed = Style {
                opacity,
                transform,
                ..style.clone()
            };
            (
                Visual::of(&posed, w, h, sticky),
                t,
                style.animations.restart,
            )
        });
        repeat(id, &e, rest, to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_ease_from_start_to_end() {
        let a = Rect::new(0., 0., 10., 10.);
        let b = Rect::new(100., 50., 30., 10.);
        assert_eq!(lerp_rect(a, b, 0.), a);
        assert_eq!(lerp_rect(a, b, 1.), b);
        assert_eq!(lerp_rect(a, b, 0.5), Rect::new(50., 25., 20., 10.));
    }
}
