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
text(subtitle).text_pretty()          // no lone word on the last line
text(heading).text_balance()          // lines of about the same length
text("Deprecated").strikethrough().italic()
```

| Builder | Style field | Web | Apple (AppKit, UIKit, SwiftUI) | Android | GTK |
|---|---|---|---|---|---|
| `.font_family(…)`, `.font_mono()`, `.font_serif()`, `.font_rounded()` | `font_family` | `font-family` | system designs (SF Mono, New York, SF Rounded) or a family | `MONOSPACE`, `SERIF`, a bundled family | `monospace`, `serif`, a family |
| `.font_weight(…)`, `.weight(650)`, `.bold()`… | `font_weight` (100–900) | `font-weight` | any weight (the system font is variable) | `Typeface.create(…, weight, …)` | Pango weight |
| `.italic()` | `italic` | `font-style` | italic trait | italic typeface | Pango style |
| `.line_height(20.)`, `.line_height(relative(1.4))`, `.leading(1.4)` | `line_height` | `line-height` | paragraph line height | `setLineHeight` | absolute line height |
| `.letter_spacing(0.8)`, `.tracking(0.8)` | `letter_spacing` (points) | `letter-spacing` | kern | `setLetterSpacing` | letter spacing |
| `.uppercase()`, `.lowercase()`, `.capitalize()`, `.text_transform(…)` | `text_transform` | `text-transform` | transformed text | transformed text | transformed text |
| `.underline()`, `.strikethrough()` / `.line_through()` | `underline`, `strikethrough` | `text-decoration-line` | underline / strikethrough style | spans | Pango attributes |
| `.tabular_nums()` | `tabular_numbers` | `font-variant-numeric` | `tnum` feature | `tnum` | `tnum` |
| `.truncate()`, `.line_clamp(n)` | `max_lines` | `text-overflow`, `line-clamp` | number of lines, tail truncation | `maxLines` + ellipsis | lines + ellipsize |
| `.nowrap()` | `no_wrap` | `white-space: pre` | clipping | horizontal scrolling off, clipped | ellipsized (see below) |
| `.text_pretty()`, `.text_balance()`, `.text_wrap(…)` | `text_wrap` | `text-wrap: pretty` / `balance` | `lineBreakStrategy = .pushOut` (both) | `BREAK_STRATEGY_HIGH_QUALITY` / `BALANCED` | ignored (see below) |

Screen readers read the text as written (`uppercase()` and `capitalize()`
only change how it looks), so put data in as it is (`text(name).capitalize()`
rather than capitalizing it in Rust). `capitalize()` raises the first letter
of each word, like CSS: « l'été à paris » reads « L'été À Paris », and a word
split across rich-text spans is capitalized once. On text fields,
`capitalize(Capitalize::Words)` is something else: the keyboard's automatic
capitalization while typing. Line heights and letter spacings in points grow with Dynamic Type
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

Fonts compiled into the binary (`Application::embedded_assets`, names under
`fonts/`: see [Assets compiled into the binary](../platforms/web.md#assets-compiled-into-the-binary))
count too, on the web, macOS, iOS and Linux.

Every backend registers them at startup:

- **Web**: one `@font-face` per file in Prism's stylesheet, pointing at the
  hashed asset (`font-display: swap`). The family, weight (a range for
  variable fonts) and style come from the font's own tables for `.ttf` and
  `.otf`; `.woff2` / `.woff` files are named `Family-WeightStyle`
  (`Inter-SemiBoldItalic.woff2`) and preferred when both exist.
- **macOS, iOS, iPadOS, tvOS, visionOS, watchOS**: `CTFontManagerRegisterFontsForURL`
  (process scope) from the bundle's `assets/fonts` (and the dev assets folder);
  embedded fonts with `CTFontManagerRegisterGraphicsFont`, from memory (macOS,
  iOS, iPadOS, tvOS, visionOS; the first face of a `.ttc`).
- **Android**: `Typeface.createFromAsset`, found by the family name read from
  the font file (also by its file name). Only the APK's assets: keep the fonts
  in the assets folder too.
- **Linux**: fontconfig (`FcConfigAppFontAddFile`), seen by Pango. Embedded
  fonts are written once to `~/.cache/prism-fonts/` (fontconfig reads files).

An unknown family falls back to the system font.

## Fonts loaded at run time

Fonts known only while the app runs (a catalogue your server offers, a file
the user uploaded) are registered with `cx.load_font`, for the session on
the web and for the process natively. Texts already shown in that family
are drawn again once it is ready.

```rust
cx.load_font(FontFace::url("Lora", "https://cdn.example.com/lora.woff2").weights(400, 700));
cx.load_font(FontFace::bytes("Uploaded 1", uploaded_bytes).weight(400));
cx.load_font(FontFace::url("Extra", "fonts/extra.ttf"));   // an asset, embedded ones included
text("Lora").font_family("Lora")
```

Asking for the same face again does nothing, so it can be called from
`render` (only for the screens that need it).

| | Registers with | Formats | URLs |
|---|---|---|---|
| Web | `new FontFace(…)` + `document.fonts` | WOFF2, WOFF, TTF, OTF | loaded by the browser (the page's `font-src` applies); an asset's name becomes its hashed URL, like a picture's; bytes are sent with the message |
| macOS, iOS, iPadOS, tvOS, visionOS, watchOS | `CTFontManagerRegisterGraphicsFont` (process scope) | TTF, OTF | loaded in the background, like pictures (`https://`, `file://`, assets: embedded ones on macOS, iOS and Linux) |
| Android | `Typeface.Builder(File)` / `createFromFile` | TTF, OTF, TTC | same |
| Linux | fontconfig (`FcConfigAppFontAddFile`), then Pango's font map reloads | TTF, OTF, TTC | same |

- **The family name**: on the web the face is registered under the name you
  give. Natively the platform knows a font by the family its file declares,
  so Prism reads it and makes your name an alias of it: `FontFace::bytes("Uploaded 1", …)`
  then `.font_family("Uploaded 1")` works whatever the file calls itself.
- **Weights**: `.weight(700)` for a static face, `.weights(100, 900)` for a
  variable one; without them the web treats the face as regular (bold is
  synthesized) and native platforms read the file.
- **First paint**: faces given by URL during the first render are in the
  page's `@font-face` rules; bytes arrive with the live page.
- WOFF and WOFF2 are web formats: native platforms refuse them (logged). A
  catalogue for every platform serves TTF or OTF files natively.

### A stylesheet for one screen (web)

`cx.add_stylesheet(url)` adds a `<link rel="stylesheet">` to the page while
the returned `Stylesheet` lives. Keep it in the entity of the screen that
needs it: when the screen closes, the entity and the handle go, and so does
the link (handles for the same URL share one link).

```rust
struct FontPicker { _fonts_css: Stylesheet }

let picker = cx.new_entity(|cx| FontPicker {
    _fonts_css: cx.add_stylesheet("/fonts/catalogue.css"),
});
```

It is in the first paint's `<head>`, carries the page's CSP nonce when the
host gives one, and a reconnected page keeps exactly the session's sheets.
Native backends ignore it: register the fonts with `cx.load_font` there.

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
- **`text_pretty` and `text_balance`** (#95) only change where lines break,
  never the box: the text keeps its width and alignment. Apple platforms have
  no balanced breaking, so both keep a lone word off the last line there
  (`pushOut`, which UIKit labels already do for plain text). Android's
  default breaker already avoids ragged lines, so `pretty` keeps it. GTK,
  watchOS, widgets and Live Activities have no such setting and break as
  usual. Browsers without `text-wrap` (before Safari 17.5, Chrome 114,
  Firefox 121) ignore it; `pretty` needs Safari 26 or Chrome 117.
- Widgets and Live Activities get the node styles (family, weight, italic,
  decoration, case, tracking, lines) but not runs or links.
