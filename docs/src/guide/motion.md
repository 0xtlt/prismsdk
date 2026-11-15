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

// Animate everything one update changes (SwiftUI's withAnimation)
cx.with_animation(Transition::spring(400), |cx| store.update(cx, |s, cx| s.delete(id, cx)));
```

- **Curves**: `ease`, `ease_in`, `ease_out`, `linear`, `spring` (a slight overshoot), each with `.delay(ms)`.
- **Poses**: `Pose::fade()`, `Pose::slide(x, y)`, `Pose::pop()`, or `Pose::new().opacity(…).offset(…).scale(…).rotate(…)`; unset fields keep the element's values.
- **Exits** animate a picture of the removed view (snapshot, overlay, clone), so its subtree can be freed at once.
- Offsets follow the layout direction (mirrored right to left). **Reduce Motion** (system setting, `prefers-reduced-motion`, GTK's animations switch) turns all of it off.
- `prism::ui` components take `.enter()`, `.exit()` and `.transition()` too.
