//! `/components/svg`: SVG documents as design and diagram tools export
//! them, drawn natively by `svg()` (#342): gradients, clip paths, shadows,
//! images, text, masks, `<use>` and style sheets.
//!
//! Each one is what a browser draws of the same markup, so a backend is
//! checked by putting this screen next to the document in a browser.

use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

/// A card as a design tool exports it: rounded corners (a clip path), a
/// diagonal gradient, a soft shadow (a filter), an image cropped to the
/// card's right edge, bold white text, and two overlapping squares at 50%
/// as one group (the overlap is not darker).
pub const CARD: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="200" viewBox="0 0 320 200">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#6366f1"/><stop offset="1" stop-color="#ec4899"/>
    </linearGradient>
    <clipPath id="card"><rect x="20" y="20" width="280" height="160" rx="16"/></clipPath>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="140%">
      <feDropShadow dx="0" dy="4" stdDeviation="8" flood-color="#000" flood-opacity="0.25"/>
    </filter>
  </defs>
  <g filter="url(#shadow)">
    <g clip-path="url(#card)">
      <rect x="0" y="0" width="320" height="200" fill="url(#bg) #6366f1"/>
      <image x="200" y="20" width="100" height="160" preserveAspectRatio="xMidYMid slice"
             href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAIElEQVR42mMQ2bEMiH7vVwEiZDYDTglMIQgbtwQd7AAAc89qAVdFlZoAAAAASUVORK5CYII="/>
    </g>
  </g>
  <text x="40" y="110" font-family="sans-serif" font-size="24" font-weight="700" fill="#fff">Hello</text>
  <g opacity="0.5">
    <rect x="240" y="140" width="40" height="40" fill="#000"/>
    <rect x="260" y="150" width="40" height="40" fill="#000"/>
  </g>
</svg>"##;

/// A diagram as a charting library writes it: a style sheet with classes,
/// a symbol used three times, lines of text (`tspan`s), centered and
/// right-aligned labels, a rotated axis label, a dashed link.
pub const DIAGRAM: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="320" height="180" viewBox="0 0 320 180">
  <style>
    .box { fill: #eef2ff; stroke: #6366f1; stroke-width: 1.5 }
    .box.done { fill: #dcfce7; stroke: #16a34a }
    .label { font-family: sans-serif; font-size: 12px; fill: #1e1b4b; text-anchor: middle }
    .small { font-size: 9px; fill: #64748b }
    #link { stroke: #94a3b8; stroke-width: 1.5; stroke-dasharray: 4 3; fill: none }
    g.axis text { font-family: monospace; font-size: 9px; fill: #475569 }
  </style>
  <defs>
    <symbol id="node" viewBox="0 0 80 40"><rect x="1" y="1" width="78" height="38" rx="8"/></symbol>
  </defs>
  <path id="link" d="M100 50 H140 M220 50 H240 V110 H200"/>
  <use href="#node" class="box done" x="20" y="30" width="80" height="40"/>
  <use xlink:href="#node" class="box" x="140" y="30" width="80" height="40"/>
  <use href="#node" class="box" x="120" y="90" width="80" height="40"/>
  <text class="label" x="60" y="48">Parse<tspan class="small" x="60" dy="1.3em">20 tests</tspan></text>
  <text class="label" x="180" y="54">Draw</text>
  <text class="label" x="160" y="114" font-weight="bold" font-style="italic">Ship</text>
  <g class="axis">
    <line x1="20" y1="160" x2="300" y2="160" stroke="#cbd5e1"/>
    <text x="20" y="172">start</text>
    <text x="300" y="172" text-anchor="end">end</text>
    <text transform="translate(310 90) rotate(-90)" text-anchor="middle">steps</text>
  </g>
</svg>"##;

/// Effects: a photo-like panel fading out through a luminance mask, a
/// radial gradient with its focal point off center, a glow (a blurred
/// copy under the shape), two stacked shadows as a design tool exports
/// them, a blurred shape, and a gradient stroke.
pub const EFFECTS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="180" viewBox="0 0 320 180">
  <defs>
    <linearGradient id="fade" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0.2" stop-color="#fff"/><stop offset="1" stop-color="#000"/>
    </linearGradient>
    <mask id="fadeOut"><rect x="10" y="10" width="90" height="160" fill="url(#fade)"/></mask>
    <linearGradient id="sky" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#0ea5e9"/><stop offset="1" stop-color="#22c55e"/>
    </linearGradient>
    <radialGradient id="ball" cx="0.5" cy="0.5" r="0.5" fx="0.3" fy="0.3">
      <stop offset="0" stop-color="#fff"/><stop offset="0.3" stop-color="#f59e0b"/><stop offset="1" stop-color="#7c2d12"/>
    </radialGradient>
    <linearGradient id="ring" gradientUnits="userSpaceOnUse" x1="230" y1="20" x2="300" y2="90">
      <stop offset="0" stop-color="#a855f7"/><stop offset="1" stop-color="#06b6d4"/>
    </linearGradient>
    <filter id="glow" x="-50%" y="-50%" width="200%" height="200%">
      <feGaussianBlur stdDeviation="5" result="blur"/>
      <feMerge><feMergeNode in="blur"/><feMergeNode in="SourceGraphic"/></feMerge>
    </filter>
    <filter id="soft" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="3"/></filter>
    <filter id="elevation" x="-30%" y="-30%" width="160%" height="180%" filterUnits="objectBoundingBox" color-interpolation-filters="sRGB">
      <feFlood flood-opacity="0" result="BackgroundImageFix"/>
      <feColorMatrix in="SourceAlpha" type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 127 0" result="hardAlpha"/>
      <feOffset dy="8"/>
      <feGaussianBlur stdDeviation="6"/>
      <feColorMatrix type="matrix" values="0 0 0 0 0.39 0 0 0 0 0.4 0 0 0 0 0.95 0 0 0 0.35 0"/>
      <feBlend mode="normal" in2="BackgroundImageFix" result="effect1_dropShadow"/>
      <feColorMatrix in="SourceAlpha" type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 127 0" result="hardAlpha"/>
      <feOffset dy="1"/>
      <feGaussianBlur stdDeviation="1"/>
      <feColorMatrix type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0.3 0"/>
      <feBlend mode="normal" in2="effect1_dropShadow" result="effect2_dropShadow"/>
      <feBlend mode="normal" in="SourceGraphic" in2="effect2_dropShadow" result="shape"/>
    </filter>
  </defs>
  <g mask="url(#fadeOut)">
    <rect x="10" y="10" width="90" height="160" rx="10" fill="url(#sky)"/>
    <circle cx="55" cy="60" r="22" fill="#fde047"/>
    <path d="M10 140 L45 90 L70 120 L85 100 L100 125 V170 H10Z" fill="#14532d"/>
  </g>
  <circle cx="160" cy="55" r="36" fill="url(#ball)"/>
  <circle cx="265" cy="55" r="30" fill="none" stroke="url(#ring)" stroke-width="10"/>
  <rect x="125" y="115" width="70" height="40" rx="10" fill="#fff" filter="url(#elevation)"/>
  <path d="M250 120 l8 16 18 3 -13 12 3 18 -16 -9 -16 9 3 -18 -13 -12 18 -3z" fill="#f43f5e" filter="url(#glow)"/>
  <circle cx="300" cy="150" r="10" fill="#6366f1" filter="url(#soft)"/>
</svg>"##;

/// The documents on a screen of their own, each at its own size and again
/// half as large (strokes, text and shadows follow).
pub struct VectorsScreen;

impl Render for VectorsScreen {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let (screen, body) = crate::page(strings::vectors_title(cx), cx);
        let sample = |title: String, alt: String, markup: &str| {
            ui::card().child(ui::heading(title)).child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .items_end()
                    .gap(16.)
                    .child(svg(markup).alt(alt.clone()))
                    .child(svg(markup).alt(alt).w(px(160.))),
            )
        };
        screen.child(
            body.child(text(strings::vectors_note(cx)).text_color(t.muted))
                .child(sample(
                    strings::vectors_card(cx),
                    strings::vectors_card_alt(cx),
                    CARD,
                ))
                .child(sample(
                    strings::vectors_diagram(cx),
                    strings::vectors_diagram_alt(cx),
                    DIAGRAM,
                ))
                .child(sample(
                    strings::vectors_effects(cx),
                    strings::vectors_effects_alt(cx),
                    EFFECTS,
                )),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_documents_are_drawn_whole() {
        for doc in [super::CARD, super::DIAGRAM, super::EFFECTS] {
            let svg = prism::svg::parse(doc).unwrap();
            assert!(svg.skipped.is_empty(), "{:?}", svg.skipped);
            assert!(!svg.drawing.items.is_empty());
        }
    }
}
