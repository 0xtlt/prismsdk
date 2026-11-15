//! The Layout card of the Components tab: a grid of equal-height tiles that
//! fits as many 140 pt columns as there is room for, a value and its unit on
//! one baseline, a 16:9 box, a row of chips that scrolls sideways, a card
//! that turns into a row when it is wide enough itself, a sticky heading and
//! a floating button fixed to the screen.

use prism::Color;
use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

pub fn card(cx: &App, taps: u32, on_tap: Handler<ClickEvent>) -> ui::Card {
    let t = Theme::of(cx);
    let tile = |label: String, tint: Color| {
        div()
            .p(12.)
            .rounded(10.)
            .bg(tint)
            .child(text(label).text_size(14.).text_color(t.text))
    };
    let soft = |c: Color| c.with_alpha(0.16);
    let chips: Vec<String> = strings::layout_chips(cx)
        .split(',')
        .map(str::to_string)
        .collect();
    ui::card()
        // Stays at the top of the page while the card is in view.
        .child(
            div()
                .sticky()
                .top(0.)
                .py(6.)
                .bg(t.surface)
                .child(ui::heading(strings::layout_heading(cx))),
        )
        .child(
            text(strings::layout_grid_note(cx))
                .text_size(13.)
                .text_color(t.muted),
        )
        // `repeat(auto-fill, minmax(140px, 1fr))`: rows of equal height.
        .child(
            div()
                .grid()
                .grid_cols([auto_fill(minmax(140., fr(1.)))])
                .gap(10.)
                .child(tile(strings::layout_wide(cx), soft(t.accent)).col_span(2))
                .child(tile(strings::layout_short(cx), soft(t.success)))
                .child(tile(strings::layout_long(cx), soft(t.danger)))
                .child(tile(strings::layout_short(cx), soft(t.muted)))
                .child(tile(strings::layout_short(cx), soft(t.accent))),
        )
        // A big figure and its unit on one baseline.
        .child(
            div()
                .flex_row()
                .items_baseline()
                .gap(6.)
                .child(text("42").text_size(40.).bold().text_color(t.text))
                .child(text("kg").text_size(15.).text_color(t.muted))
                .child(
                    text(strings::layout_baseline(cx))
                        .text_size(13.)
                        .text_color(t.muted),
                ),
        )
        .child(
            div()
                .w_full()
                .max_w(360.)
                .aspect_ratio(16. / 9.)
                .rounded(12.)
                .bg(t.accent)
                .items_center()
                .justify_center()
                .child(text("16 : 9").bold().text_color(t.on_accent)),
        )
        // Chips in one row that scrolls sideways (they never shrink).
        .child(
            scroll_x()
                .gap(8.)
                .py(2.)
                .children(chips.into_iter().map(|c| {
                    div()
                        .px(12.)
                        .py(6.)
                        .rounded(14.)
                        .border(1.)
                        .border_color(t.border)
                        .child(text(c).text_size(14.).text_color(t.text))
                })),
        )
        // Rearranges by its own width, whatever the window.
        .child(
            div()
                .gap(10.)
                .p(12.)
                .rounded(10.)
                .border(1.)
                .border_color(t.border)
                .when_width_at_least(420., |s| s.flex_row().items_center().gap(16.))
                .child(div().size(px(44.)).rounded(22.).bg(t.accent))
                .child(
                    div()
                        .flex_1()
                        .gap(2.)
                        .child(
                            text(strings::layout_own_width(cx))
                                .semibold()
                                .text_color(t.text),
                        )
                        .child(
                            text(strings::layout_own_width_note(cx))
                                .text_size(13.)
                                .text_color(t.muted),
                        ),
                ),
        )
        // Only in windows at least 768 pt wide (any threshold, not just
        // the size classes).
        .child(
            text(strings::layout_wide_window(cx))
                .text_size(13.)
                .text_color(t.success)
                .with_style(|s| s.display_none = true)
                .at_width(768., |s| s.with_style(|s| s.display_none = false)),
        )
        // Fixed to the screen, wherever it is declared.
        .child(
            div()
                .fixed()
                .bottom(24.)
                .right(24.)
                .size(px(56.))
                .rounded(28.)
                .bg(t.accent)
                .items_center()
                .justify_center()
                .child(text(format!("+{taps}")).bold().text_color(t.on_accent))
                .accessibility_role(Role::Button)
                .accessibility_label(strings::layout_fab(cx))
                .on_click(on_tap),
        )
}
