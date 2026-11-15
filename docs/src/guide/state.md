# Updating state

- State lives in **entities** (`cx.new(|_| MyView { .. })`), GPUI-style.
- Mutate it in a `cx.listener`: the entity re-renders automatically. Elsewhere, call `cx.notify()`.
- Only notified entities re-render. A clean child entity is neither rendered nor diffed: its mounted views are reused as-is. So split your UI into entities where updates are frequent (an input, a live counter, a list row).
- Derived state: `cx.observe(&other, |this, other, cx| this.total = other.read(cx).total())` runs when `other` changes, then re-renders `this`.
- Each entity renders one root node (a fragment gets wrapped in a `div`).
- Globals: `cx.set_global(theme)`, `cx.global::<Theme>()`. `cx.update_global(|t: &mut Theme| …)` re-renders every entity (a theme, a language). For bookkeeping that no view shows while rendering (a debounce table for a slider, a cache, counters), use `cx.update_global_quiet(|d: &mut Debounce| …)` or `cx.global_mut::<Debounce>()`: nothing re-renders.
