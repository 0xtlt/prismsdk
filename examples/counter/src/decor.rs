//! The Decorations card of the Components tab: shadows (drop, inset,
//! stacked), dashed and per-side borders, gradients, clipping to rounded
//! corners, a frosted (backdrop-blurred) bar, colors relative to the text
//! color, and interaction states (hover, press, focus within, groups).

use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

#[derive(Default)]
pub struct Decor {
    query: String,
    pressed: u32,
}

impl Render for Decor {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let tint = |a: f32| Color::current().with_alpha(a);
        let tile = |label: String| {
            div()
                .flex_1()
                .h(72.)
                .rounded(12.)
                .items_center()
                .justify_center()
                .text_color(t.text)
                .child(text(label).text_size(13.).semibold().text_color(t.text))
        };
        let rows = (1..=3).map(|n| {
            div()
                .flex_row()
                .justify_between()
                .px(14.)
                .py(12.)
                .text_color(t.text)
                // Separators: only the bottom side, except on the last row.
                .when(n < 3, |d| d.border_bottom(1., tint(0.12)))
                .hover(move |s| s.bg(tint(0.06)))
                .active(move |s| s.bg(tint(0.14)))
                .accessibility_role(Role::Button)
                .on_click(cx.listener(|this, _, _| this.pressed += 1))
                .child(text(strings::decor_row(cx, n)).text_color(t.text))
                .child(text("›").text_color(t.muted))
        });
        let stripes = [0xFF453A, 0xFF9F0A, 0xFFD60A, 0x30D158, 0x0A84FF, 0xBF5AF2];
        ui::card()
            .child(ui::heading(strings::decor_title(cx)))
            .child(
                div()
                    .flex_row()
                    .gap(12.)
                    .child(
                        tile(strings::decor_shadow(cx))
                            .bg(t.surface)
                            .shadow(shadow_md())
                            .shadow(shadow_sm()),
                    )
                    .child(
                        tile(strings::decor_inset(cx))
                            .bg(t.surface_alt)
                            .shadow(Shadow::new(0., 2., 6., rgba(0x000000, 0.3)).inset()),
                    )
                    .child(
                        tile(strings::decor_offline(cx))
                            .border(2.)
                            .border_color(tint(0.35))
                            .border_dashed(),
                    ),
            )
            .child(
                text(strings::decor_rows(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            )
            .child(
                div()
                    .rounded(12.)
                    .clip()
                    .border(1.)
                    .border_color(t.border)
                    .children(rows),
            )
            .child(
                div()
                    .flex_row()
                    .gap(12.)
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .h(64.)
                            .rounded(12.)
                            .bg_gradient(Gradient::linear(135., [t.accent, rgb(0xBF5AF2)]))
                            .items_center()
                            .justify_center()
                            .child(
                                text(strings::decor_gradient(cx))
                                    .semibold()
                                    .text_color(Color::WHITE),
                            ),
                    )
                    .child(
                        div()
                            .size(64.)
                            .rounded(32.)
                            .bg_gradient(Gradient::radial([
                                rgb(0xFFD60A),
                                rgb(0xFF9F0A),
                                rgb(0xFF453A),
                            ]))
                            .accessibility_label(strings::decor_radial(cx)),
                    ),
            )
            .child(
                // Square stripes clipped to the rounded corners, under a
                // frosted bar.
                div()
                    .h(110.)
                    .rounded(18.)
                    .clip()
                    .child(
                        div()
                            .flex_row()
                            .size_full()
                            .children(stripes.map(|c| div().flex_1().bg(rgb(c)))),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(0.)
                            .right(0.)
                            .bottom(0.)
                            .h(44.)
                            .backdrop_blur(16.)
                            .bg(adaptive(rgba(0xFFFFFF, 0.45), rgba(0x1C1C1E, 0.45)))
                            .border_top(1., rgba(0xFFFFFF, 0.35))
                            .items_center()
                            .justify_center()
                            .child(
                                text(strings::decor_frosted(cx))
                                    .semibold()
                                    .text_color(t.text),
                            ),
                    ),
            )
            .child(
                text(strings::decor_focus_hint(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            )
            .child(
                div()
                    .group()
                    .flex_row()
                    .items_center()
                    .gap(8.)
                    .p(6.)
                    .rounded(12.)
                    .border(1.)
                    .border_color(t.border)
                    .focus_within(move |s| {
                        s.border_color(t.accent)
                            .shadow(Shadow::ring(3., t.accent.with_alpha(0.25)))
                    })
                    .child(
                        text_input()
                            .value(self.query.clone())
                            .placeholder(strings::decor_search(cx))
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.query = e.value.clone()),
                            )
                            .flex_1(),
                    )
                    .child(
                        ui::button(strings::decor_clear(cx))
                            .small()
                            .ghost()
                            .on_click(cx.listener(|this, _, _| this.query.clear()))
                            .style(|s| s.display_none = true)
                            .style(|s| {
                                *s = s.clone().group_focus_within(|s| {
                                    s.with_style(|s| s.display_none = false)
                                })
                            }),
                    ),
            )
    }
}
