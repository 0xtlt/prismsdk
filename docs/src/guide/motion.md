# Motion

Transitions, transforms, enter/exit animations and loops, native on every backend: UIKit and Core Animation (iOS, tvOS, visionOS), AppKit (macOS), property animators (Android), SwiftUI (watchOS), CSS and the Web Animations API (web), GTK's frame clock (Linux), Composition animations (Windows).

```rust
div().spring(400)                                   // animate this element's changes (frame, colors, opacity, transform)
div().transition(Transition::ease_out(250).delay(100))
div().offset(0., -4.).scale(1.05).rotate(3.)        // visual only: the layout does not move

// Appear and disappear (keyed children: the removed one leaves)
div().key(id).enter(Pose::slide(0., 12.)).exit(Pose::slide(-40., 0.))
row.stagger(index, 40)                              // cascading entrances, 40 ms apart
image("heart.fill").repeat(Pose::new().scale(1.2), Transition::ease(700))   // pulse forever
image("arrow.clockwise")                            // spin: a full turn, starting over
    .repeat_from_start(Pose::new().rotate(360.), Transition::linear(900))
div().repeat(Pose::new().scale_y(0.1), Transition::ease(120))   // blink: vertical scale only
div().scale_x(0.5)                                  // squash horizontally (with `.scale()`)

// Center a label on 30 % of its parent's width, just above a line
text("Avg").absolute().left(relative(0.3)).top(px(y)).translate_percent(-50., -100.)

// Animate everything one update changes (SwiftUI's withAnimation)
cx.with_animation(Transition::spring(400), |cx| store.update(cx, |s, cx| s.delete(id, cx)));
```

- **What animates**: changes made to an element after it appeared. Its first style and frame apply at once, on every backend, including the rules on its own width (`.when_width_at_least`) that its first layout picks. An element that starts animating (`.animate`, `.spring`, `.transition`) where the previous render had one that did not, at the same place among its siblings, is a new element: it does not animate from the other one's style (no background, no padding). A key (or being an entity's root) keeps it the same element, so its change animates.
- **Curves**: `ease`, `ease_in`, `ease_out`, `linear`, `spring` (a slight overshoot), each with `.delay(ms)`.
- **Poses**: `Pose::fade()`, `Pose::slide(x, y)`, `Pose::pop()`, or `Pose::new().opacity(…).offset(…).scale(…).rotate(…)`; unset fields keep the element's values.
- **Exits** animate a picture of the removed view (snapshot, overlay, clone), so its subtree can be freed at once.
- **Loops**: `.repeat(pose, t)` swings back and forth (easing in and out); `.repeat_from_start(pose, t)` plays to the pose and starts over, with the transition's own curve (`linear` for a steady spin; springs ease in and out). Angles animate through every degree in between on every backend, so `rotate(360.)` is a full turn: Core Animation gets key frames every 30°, Android, SwiftUI, GTK and the Web Animations API animate the angle as a number.
- **Non-uniform scale**: `scale_x` / `scale_y` on the style and on poses, multiplied with `scale`. Native everywhere (`CGAffineTransform`, `setScaleX/Y`, `scaleEffect(x:y:)`, `gsk::Transform::scale`, CSS `scale(x, y)`).
- **Offsets in percent of the element's own size**: `.translate_percent(x, y)` (CSS `translate(x%, y%)`), for anchoring a label on a point without negative margins or fixed-width boxes. Visual only, like `offset`; mirrored right to left. The native backends apply it when they place the view (after the shared layout, so it costs nothing per frame).
- Offsets follow the layout direction (mirrored right to left). **Reduce Motion** (system setting, `prefers-reduced-motion`, GTK's animations switch) turns all of it off.
- `prism::ui` components take `.enter()`, `.exit()` and `.transition()` too.

## Frame callbacks

Code that must run once per refresh of the screen (an auto-scroll, a game
loop, a drag follow-up, a time-based animation `Transition` cannot express)
asks for frames instead of a 16 ms timer, which drifts against the display
(#311):

```rust
// Every frame until stopped: 1,200 points a second, whatever the refresh rate.
self.ticker = Some(cx.frames(|this, frame, cx| {
    cx.scroll_to("list", 0., 1200. * frame.elapsed.as_secs_f32(), false);
}));
cx.stop_frames(ticker);

// Once, on the next frame: time to first frame, a measurement after layout.
cx.on_next_frame(|this, frame, _| this.first_frame = Some(frame.timestamp - this.started));
```

- **`FrameInfo`**: `timestamp` (the refresh the callbacks run for), `target_timestamp` (when what they change reaches the screen), `elapsed` (since the ticker's first frame) and `delta` (since the previous frame). Use the times rather than counting frames: screens run at 60, 90 or 120 Hz, and a slow frame skips refreshes.
- **Entities**: `cx.frames` and `cx.on_next_frame` in a `Context<T>` hand the callback the entity and re-render it after, like a listener; a released entity's ticker stops. `App::frames(|frame, app| …)` and `App::on_next_frame` take the app instead.
- **What drives them**: `CADisplayLink` on iOS, iPadOS, tvOS and visionOS; the window's display link on macOS 14 and later (`CVDisplayLink` on macOS 13); `Choreographer` on Android; the GTK frame clock on Linux; `CompositionTarget.Rendering` on Windows; `requestAnimationFrame` on the web, each frame waiting for the server's answer to the previous one, so a slow connection slows frames down instead of queueing them. watchOS has no display link: a 60 Hz timer stands in. The callbacks run before the platform lays out and draws the frame, and what they change is on that frame.
- **Cost**: the display link runs only while a callback waits, and stops with the last one. Frames pause while the app is in the background (and on the web while the tab is hidden), and go on when it comes back.
