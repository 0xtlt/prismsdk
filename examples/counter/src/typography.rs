//! The Typography card of the Components tab: an overline title, a big
//! tabular figure at a custom weight and line height, a monospaced code,
//! a two-line clamp, single-line truncation, decorations and families, a
//! rich-text paragraph with a bold run, an inline link, an icon and a
//! clickable run, underlines with their own color and line, and a
//! clickable text.

use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

#[derive(Default)]
pub struct Typography {
    text_taps: u32,
    run_taps: u32,
}

impl Render for Typography {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let word = |s: String| text(s).text_size(15.);
        ui::card()
            .child(ui::heading(strings::typography_title(cx)))
            .child(
                text(strings::typography_overline(cx))
                    .text_size(11.)
                    .semibold()
                    .uppercase()
                    .tracking(1.2)
                    .text_color(t.muted),
            )
            .child(
                text(cx.format_number(1284., 0))
                    .text_size(44.)
                    .weight(650)
                    .tabular_nums()
                    .leading(1.),
            )
            .child(
                text(strings::typography_figure_caption(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            )
            .child(
                div()
                    .flex_row()
                    .gap(8.)
                    .items_center()
                    .child(text(strings::typography_order_id(cx)).text_color(t.muted))
                    .child(text("ORD-7F3A-29C1").font_mono().text_size(14.)),
            )
            .child(
                text(strings::typography_clamp(cx))
                    .line_clamp(2)
                    .leading(1.35)
                    .tooltip_when_truncated(),
            )
            .child(
                text(strings::typography_truncate(cx))
                    .truncate()
                    .text_color(t.muted)
                    .tooltip_when_truncated(),
            )
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(12.)
                    .child(word(strings::typography_italic(cx)).italic())
                    .child(word(strings::typography_underline(cx)).underline())
                    .child(word(strings::typography_strike(cx)).strikethrough())
                    .child(word(strings::typography_serif(cx)).font_serif())
                    .child(word(strings::typography_rounded(cx)).font_rounded().bold())
                    .child(word(strings::typography_mono(cx)).font_mono()),
            )
            // Underlines with their own color and line, and a sized icon (#221).
            .child(rich_text([
                span(strings::typography_underline(cx))
                    .underline_color(t.accent)
                    .underline_style(UnderlineStyle::Thick),
                span(" "),
                span(strings::typography_underline(cx))
                    .underline_color(t.danger)
                    .underline_style(UnderlineStyle::Dotted),
                span(" "),
                span(strings::typography_underline(cx)).underline_style(UnderlineStyle::Double),
                span(" "),
                span(strings::typography_underline(cx))
                    .underline_color(t.accent)
                    .underline_style(UnderlineStyle::Dashed),
                span(" "),
                Span::icon("bell.fill", 12.).text_color(t.danger),
            ]))
            .child(rich_text([
                span(strings::typography_signed_in(cx)),
                span("Ada Lovelace").bold(),
                span(" · "),
                link(strings::typography_settings_link(cx), "/settings").text_color(t.accent),
                span(" · "),
                icon("star.fill").text_color(t.accent),
                span(" "),
                span(strings::typography_tap_run(cx))
                    .semibold()
                    .underline()
                    .text_color(t.accent)
                    .on_click(cx.listener(|this, _, _| this.run_taps += 1)),
                span(" ("),
                span(strings::typography_run_taps(cx, self.run_taps as i64)).font_mono(),
                span(")"),
            ]))
            .child(
                text(strings::typography_tap_text(cx, self.text_taps as i64))
                    .text_color(t.accent)
                    .on_click(cx.listener(|this, _, _| this.text_taps += 1)),
            )
    }
}
