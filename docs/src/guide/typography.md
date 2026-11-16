# Text and typography

Text styles apply to the node they are set on (they are not inherited), on
texts, and on buttons and inputs where the platform allows it.

```rust
text("Orders this week").text_size(11.).semibold().uppercase().tracking(1.2)
text(cx.format_number(1284., 0)).text_size(44.).weight(650).tabular_nums().leading(1.)
text("ORD-7F3A-29C1").font_mono()
text(description).line_clamp(2)       // two lines, then "…"
text(file_name).truncate()            // one line, cut with "…"
text(code).nowrap()                   // never wraps, cut at the edge
text("Deprecated").strikethrough().italic()
```

| Builder | Style field | Web | Apple (AppKit, UIKit, SwiftUI) | Android | GTK |
|---|---|---|---|---|---|
| `.font_family(…)`, `.font_mono()`, `.font_serif()`, `.font_rounded()` | `font_family` | `font-family` | system designs (SF Mono, New York, SF Rounded) or a family | `MONOSPACE`, `SERIF`, a bundled family | `monospace`, `serif`, a family |
| `.font_weight(…)`, `.weight(650)`, `.bold()`… | `font_weight` (100–900) | `font-weight` | any weight (the system font is variable) | `Typeface.create(…, weight, …)` | Pango weight |
| `.italic()` | `italic` | `font-style` | italic trait | italic typeface | Pango style |
| `.line_height(20.)`, `.line_height(relative(1.4))`, `.leading(1.4)` | `line_height` | `line-height` | paragraph line height | `setLineHeight` | absolute line height |
| `.letter_spacing(0.8)`, `.tracking(0.8)` | `letter_spacing` (points) | `letter-spacing` | kern | `setLetterSpacing` | letter spacing |
| `.uppercase()`, `.lowercase()`, `.text_transform(…)` | `text_transform` | `text-transform` | transformed text | transformed text | transformed text |
| `.underline()`, `.strikethrough()` / `.line_through()` | `underline`, `strikethrough` | `text-decoration-line` | underline / strikethrough style | spans | Pango attributes |
| `.tabular_nums()` | `tabular_numbers` | `font-variant-numeric` | `tnum` feature | `tnum` | `tnum` |
| `.truncate()`, `.line_clamp(n)` | `max_lines` | `text-overflow`, `line-clamp` | number of lines, tail truncation | `maxLines` + ellipsis | lines + ellipsize |
| `.nowrap()` | `no_wrap` | `white-space: pre` | clipping | horizontal scrolling off, clipped | ellipsized (see below) |

Screen readers read the text as written (`uppercase()` only changes how it
looks). Line heights and letter spacings in points grow with Dynamic Type
like the font size; `relative(…)` line heights follow the size.

Measuring is native: the shared flexbox layout asks each platform for the
size of the very label it draws, so line heights, clamps and letter spacing
count. A clamped, truncated or unwrapped text may be narrower than its words
(like CSS `overflow: hidden`), and a one-line text is measured on one line,
then cut by its frame.

## Bundled fonts

Put font files in `assets/fonts/` and use their family name:

```text
assets/fonts/Inter.ttf            ← variable font: every weight
assets/fonts/Inter-Italic.ttf
```

```rust
text("Hello").font_family("Inter").weight(650)
```

Every backend registers them at startup:

- **Web**: one `@font-face` per file in Prism's stylesheet, pointing at the
  hashed asset (`font-display: swap`). The family, weight (a range for
  variable fonts) and style come from the font's own tables for `.ttf` and
  `.otf`; `.woff2` / `.woff` files are named `Family-WeightStyle`
  (`Inter-SemiBoldItalic.woff2`) and preferred when both exist.
- **macOS, iOS, iPadOS, tvOS, visionOS, watchOS**: `CTFontManagerRegisterFontsForURL`
  (process scope) from the bundle's `assets/fonts` (and the dev assets folder).
- **Android**: `Typeface.createFromAsset`, found by the family name read from
  the font file (also by its file name).
- **Linux**: fontconfig (`FcConfigAppFontAddFile`), seen by Pango.

An unknown family falls back to the system font.

## Rich text

`rich_text` is one paragraph made of runs: it wraps between words (not
between runs), and screen readers read it as one paragraph.

```rust
rich_text([
    span("Signed in as "),
    span("Ada Lovelace").bold(),
    span(" · "),
    link("account settings", "/settings"),           // navigates in the app
    span(" · "),
    icon("star.fill").text_color(t.accent),          // inline SF Symbol
    span(" "),
    span("tap me").underline().on_click(cx.listener(|this, _, _| this.taps += 1)),
    link("docs", "https://example.com/docs"),        // opens the browser
])
```

A span takes `bold()`, `semibold()`, `medium()`, `weight(…)`, `italic()`,
`underline()` / `no_underline()`, `strikethrough()`, `text_color(…)`,
`bg(…)` (highlights, inline code), `font_family(…)` / `font_mono()`,
`text_size(…)` (scaled with the paragraph), `on_click(…)` and `label(…)`
(what screen readers say instead, e.g. for an icon, which is decorative
otherwise). The paragraph's own styles (size, color, alignment, line height,
clamp…) apply to every run.

| | Runs | Links |
|---|---|---|
| Web | inline `<span>`s | `<a href>` (app paths under the mount point), keyboard and screen reader links |
| macOS | `NSAttributedString` in the label | click; Tab moves from link to link (Keyboard Navigation on), Space/Return activates; VoiceOver actions |
| iOS, iPadOS, visionOS | `NSAttributedString` | tap; VoiceOver actions (also Full Keyboard Access) |
| tvOS | `NSAttributedString` | a text with links takes the focus; select opens the first one, the others are VoiceOver actions |
| watchOS | `AttributedString` | tap (`openURL` action) |
| Android | `SpannableString` | `ClickableSpan`: tap, keyboard / D-pad, TalkBack links |
| Linux | Pango attributes and markup | GTK label links: click, keyboard; one label for screen readers (GTK 4.8) |

Inline icons are SF Symbols on Apple platforms (sized and tinted like the
text) and their fallback glyph elsewhere (see `prism::glyphs`).

## Clickable text

A whole text can be clicked or be a link, without wrapping it in a `div`:

```rust
text("Forgot password?").text_color(t.accent).on_click(cx.listener(|this, _, cx| …))
text("Terms of use").underline().link("/terms")
```

It is a button (or a link) for assistive technologies and reachable from the
keyboard where the platform focuses buttons. On the web a text made a link is an `<a href>`
(see [Links](navigation.md#links)); links inside it are then plain runs that
still report their clicks, since links don't nest.

## Where platforms differ

- **Rounded** is SF Rounded on Apple platforms (and `ui-rounded` in Safari);
  elsewhere the system font.
- **Weights between hundreds** (650) are exact with variable fonts (the
  Apple system fonts, most web fonts, variable fonts on Android); other fonts
  use the nearest face. Widgets and watchOS SwiftUI `Font.Weight` have nine
  steps.
- **Line height** on watchOS and in widgets is extra line spacing: it can add
  space but not tighten below the font's own. Android puts the extra space
  below each line.
- **`nowrap`** on Linux ends cut lines with an ellipsis (GTK never makes a
  label narrower than an unwrapped line).
- Widgets and Live Activities get the node styles (family, weight, italic,
  decoration, case, tracking, lines) but not runs or links.
