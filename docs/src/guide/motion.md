# Motion

Transitions, transforms, enter/exit animations and loops, native on every backend: UIKit and Core Animation (iOS, tvOS, visionOS), AppKit (macOS), property animators (Android), SwiftUI (watchOS), CSS and the Web Animations API (web), GTK's frame clock (Linux).

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

- **Curves**: `ease`, `ease_in`, `ease_out`, `linear`, `spring` (a slight overshoot), each with `.delay(ms)`.
- **Poses**: `Pose::fade()`, `Pose::slide(x, y)`, `Pose::pop()`, or `Pose::new().opacity(…).offset(…).scale(…).rotate(…)`; unset fields keep the element's values.
- **Exits** animate a picture of the removed view (snapshot, overlay, clone), so its subtree can be freed at once.
- **Loops**: `.repeat(pose, t)` swings back and forth (easing in and out); `.repeat_from_start(pose, t)` plays to the pose and starts over, with the transition's own curve (`linear` for a steady spin; springs ease in and out). Angles animate through every degree in between on every backend, so `rotate(360.)` is a full turn: Core Animation gets key frames every 30°, Android, SwiftUI, GTK and the Web Animations API animate the angle as a number.
- **Non-uniform scale**: `scale_x` / `scale_y` on the style and on poses, multiplied with `scale`. Native everywhere (`CGAffineTransform`, `setScaleX/Y`, `scaleEffect(x:y:)`, `gsk::Transform::scale`, CSS `scale(x, y)`).
- **Offsets in percent of the element's own size**: `.translate_percent(x, y)` (CSS `translate(x%, y%)`), for anchoring a label on a point without negative margins or fixed-width boxes. Visual only, like `offset`; mirrored right to left. The native backends apply it when they place the view (after the shared layout, so it costs nothing per frame).
- Offsets follow the layout direction (mirrored right to left). **Reduce Motion** (system setting, `prefers-reduced-motion`, GTK's animations switch) turns all of it off.
- `prism::ui` components take `.enter()`, `.exit()` and `.transition()` too.
