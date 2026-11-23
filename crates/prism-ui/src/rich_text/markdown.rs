//! A [`RichText`] as Markdown, and back (#294): the subset the model
//! holds, written so that CommonMark readers (and [`read`]) get it back.

use prism_core::{Span, normalize_spans, span};

use super::{BlockKind, RichBlock, RichText};

// --- Writing -------------------------------------------------------------------

/// The document as Markdown.
pub(super) fn write(doc: &RichText) -> String {
    let mut out = String::new();
    let mut prev: Option<BlockKind> = None;
    let mut number = 0;
    // Markdown has no empty paragraph: they go.
    for b in doc
        .blocks
        .iter()
        .filter(|b| !(b.is_empty() && b.kind == BlockKind::Paragraph))
    {
        if let Some(p) = prev {
            // The items of one list, one per line; anything else apart.
            out.push_str(if p == b.kind && b.kind.is_list() {
                "\n"
            } else {
                "\n\n"
            });
        }
        number = match (b.kind, prev) {
            (BlockKind::Numbered, Some(BlockKind::Numbered)) => number + 1,
            (BlockKind::Numbered, _) => 1,
            _ => 0,
        };
        let text = block_start(inline(&b.spans).trim_start());
        match b.kind {
            BlockKind::Paragraph => {}
            BlockKind::Heading(level) => {
                out.push_str(&"#".repeat(usize::from(level.clamp(1, 3))));
                out.push(' ');
            }
            BlockKind::Bullet => out.push_str("- "),
            BlockKind::Numbered => out.push_str(&format!("{number}. ")),
        }
        out.push_str(&text);
        prev = Some(b.kind);
    }
    out
}

/// `text` safe at the start of a block: a `#`, `-`, `+`, `>` or `1.`
/// there would be markup.
fn block_start(text: &str) -> String {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && matches!(text[digits..].chars().next(), Some('.' | ')')) {
        return format!("{}\\{}", &text[..digits], &text[digits..]);
    }
    match text.chars().next() {
        Some('#' | '-' | '+' | '>' | '=') => format!("\\{text}"),
        _ => text.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Bold,
    Italic,
}

impl Mark {
    fn delimiter(self) -> &'static str {
        match self {
            Mark::Bold => "**",
            Mark::Italic => "*",
        }
    }
}

/// The runs of a block as Markdown: links around their runs, emphasis
/// opened and closed where the marks change, never against whitespace.
fn inline(spans: &[Span]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < spans.len() {
        let href = spans[i].href();
        let mut j = i + 1;
        while j < spans.len() && spans[j].href() == href {
            j += 1;
        }
        let body = marks(&spans[i..j]);
        match href {
            Some(h) if !body.trim().is_empty() => {
                out.push('[');
                out.push_str(&body);
                out.push_str("](");
                out.push_str(&destination(h));
                out.push(')');
            }
            _ => out.push_str(&body),
        }
        i = j;
    }
    out
}

/// A link's destination: in angle brackets when it has spaces or parentheses.
fn destination(href: &str) -> String {
    if href.contains([' ', '(', ')', '<', '>']) {
        format!("<{}>", href.replace('<', "%3C").replace('>', "%3E"))
    } else {
        href.to_string()
    }
}

fn marks(spans: &[Span]) -> String {
    let mut out = String::new();
    let mut open: Vec<Mark> = Vec::new();
    // Whitespace after the last text: inside the marks if they go on,
    // after their closing otherwise.
    let mut pending = String::new();
    // The marks every run the pending whitespace came from has: those
    // closing before it stay open across it, the others close first.
    let mut pending_marks: Option<Vec<Mark>> = None;
    for s in spans {
        let text = match s.chip_label() {
            Some(label) if s.is_chip() => label.to_string(),
            _ if s.is_object() => continue,
            _ => s.text().to_string(),
        };
        let mut want = Vec::new();
        if s.is_bold() {
            want.push(Mark::Bold);
        }
        if s.is_italic() {
            want.push(Mark::Italic);
        }
        let trimmed = text.trim_start();
        let lead = &text[..text.len() - trimmed.len()];
        let core = trimmed.trim_end();
        let trail = &trimmed[core.len()..];
        if core.is_empty() {
            pending.push_str(&text);
            let had = pending_marks.take().unwrap_or_else(|| want.clone());
            pending_marks = Some(had.into_iter().filter(|m| want.contains(m)).collect());
            continue;
        }
        // Close what this run does not have, what the whitespace before it
        // does not have, and what is open inside those.
        let keep = open
            .iter()
            .take_while(|m| {
                want.contains(m) && pending_marks.as_ref().is_none_or(|p| p.contains(m))
            })
            .count();
        for m in open.drain(keep..).rev() {
            out.push_str(m.delimiter());
        }
        out.push_str(&escape(&pending));
        pending.clear();
        out.push_str(&escape(lead));
        for m in want {
            if !open.contains(&m) {
                out.push_str(m.delimiter());
                open.push(m);
            }
        }
        out.push_str(&escape(core));
        pending.push_str(trail);
        pending_marks = (!trail.is_empty()).then(|| open.clone());
    }
    for m in open.drain(..).rev() {
        out.push_str(m.delimiter());
    }
    out.push_str(&escape(&pending));
    out
}

/// Characters Markdown would read as markup, escaped; a line break as a
/// hard break.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '*' | '_' | '[' | ']' | '`' | '<' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\\n"),
            _ => out.push(c),
        }
    }
    out
}

// --- Reading -------------------------------------------------------------------

/// Reads Markdown into the model.
pub(super) fn read(markdown: &str) -> RichText {
    let mut blocks = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut kind = BlockKind::Paragraph;
    let mut fence: Option<String> = None;
    for line in markdown.split('\n') {
        let line = line.trim_end_matches('\r');
        // Fenced code: its lines as they are.
        if let Some(f) = &fence {
            if line.trim_start().starts_with(f.as_str()) {
                fence = None;
            } else {
                blocks.push(RichBlock::paragraph([span(line)]));
            }
            continue;
        }
        let stripped = line.trim_start();
        let indent = line.len() - stripped.len();
        if stripped.is_empty() {
            flush(&mut blocks, &mut para, kind);
            continue;
        }
        if indent < 4 {
            if stripped.starts_with("```") || stripped.starts_with("~~~") {
                flush(&mut blocks, &mut para, kind);
                fence = Some(stripped[..3].to_string());
                continue;
            }
            // A setext heading: a paragraph underlined.
            if !para.is_empty() && kind == BlockKind::Paragraph {
                let t = stripped.trim_end();
                if !t.is_empty() && t.chars().all(|c| c == '=') {
                    flush(&mut blocks, &mut para, BlockKind::Heading(1));
                    continue;
                }
                if !t.is_empty() && t.chars().all(|c| c == '-') {
                    flush(&mut blocks, &mut para, BlockKind::Heading(2));
                    continue;
                }
            }
            if thematic_break(stripped) {
                flush(&mut blocks, &mut para, kind);
                continue;
            }
            if let Some((level, text)) = heading(stripped) {
                flush(&mut blocks, &mut para, kind);
                para.push(text.to_string());
                flush(&mut blocks, &mut para, BlockKind::heading(level));
                continue;
            }
            if let Some((item, text)) = list_item(stripped) {
                flush(&mut blocks, &mut para, kind);
                kind = item;
                para.push(text.to_string());
                continue;
            }
            if let Some(quoted) = stripped.strip_prefix('>') {
                let quoted = quoted.strip_prefix(' ').unwrap_or(quoted);
                if quoted.trim().is_empty() {
                    flush(&mut blocks, &mut para, kind);
                } else {
                    if para.is_empty() {
                        kind = BlockKind::Paragraph;
                    }
                    para.push(quoted.to_string());
                }
                continue;
            }
        }
        if para.is_empty() {
            kind = BlockKind::Paragraph;
        }
        para.push(line.to_string());
    }
    flush(&mut blocks, &mut para, kind);
    RichText::new(blocks)
}

/// The paragraph or item of `lines` as blocks: lines joined by a space, a
/// hard break (`\` or two spaces at the end) starting a new paragraph.
fn flush(blocks: &mut Vec<RichBlock>, lines: &mut Vec<String>, kind: BlockKind) {
    if lines.is_empty() {
        return;
    }
    let mut kind = kind;
    let mut text = String::new();
    let n = lines.len();
    for (i, line) in lines.drain(..).enumerate() {
        let hard = i + 1 < n && (line.ends_with("  ") || line.ends_with('\\'));
        let line = line.trim();
        let line = if hard {
            line.strip_suffix('\\').unwrap_or(line)
        } else {
            line
        };
        if !text.is_empty() && !text.ends_with(' ') {
            text.push(' ');
        }
        text.push_str(line.trim_end());
        if hard {
            blocks.push(RichBlock::new(kind, inline_spans(&text)));
            text.clear();
            kind = BlockKind::Paragraph;
        }
    }
    blocks.push(RichBlock::new(kind, inline_spans(&text)));
}

/// `---`, `***`, `___` (three or more, spaces allowed).
fn thematic_break(line: &str) -> bool {
    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    chars.len() >= 3 && matches!(chars[0], '-' | '*' | '_') && chars.iter().all(|c| *c == chars[0])
}

/// An ATX heading: `#` to `######`, a space, its text (closing `#`s dropped).
fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let text = rest.trim();
    // A closing sequence (` ##`).
    let text = match text.trim_end_matches('#') {
        t if t.is_empty() => t,
        t if t.ends_with([' ', '\t']) => t.trim_end(),
        _ => text,
    };
    Some((level.min(3) as u8, text))
}

/// A list item's marker: `-`, `*`, `+` or `1.` / `1)`, then a space.
fn list_item(line: &str) -> Option<(BlockKind, &str)> {
    for marker in ['-', '*', '+'] {
        if let Some(rest) = line.strip_prefix(marker)
            && (rest.is_empty() || rest.starts_with([' ', '\t']))
        {
            return Some((BlockKind::Bullet, rest.trim_start()));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if (1..=9).contains(&digits) {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(['.', ')'])
            && (rest.is_empty() || rest.starts_with([' ', '\t']))
        {
            return Some((BlockKind::Numbered, rest.trim_start()));
        }
    }
    None
}

/// A piece of inline content while it is read.
#[derive(Clone)]
enum Tok {
    Text {
        text: String,
        bold: bool,
        italic: bool,
        href: Option<String>,
    },
    Delim {
        ch: char,
        count: usize,
        original: usize,
        can_open: bool,
        can_close: bool,
    },
}

fn text_tok(text: impl Into<String>) -> Tok {
    Tok::Text {
        text: text.into(),
        bold: false,
        italic: false,
        href: None,
    }
}

/// The runs of a paragraph's inline Markdown.
pub(super) fn inline_spans(text: &str) -> Vec<Span> {
    let toks = inline_toks(text);
    normalize_spans(toks.into_iter().filter_map(|t| match t {
        Tok::Text {
            text,
            bold,
            italic,
            href,
        } => {
            let mut s = span(text);
            if bold {
                s = s.bold();
            }
            if italic {
                s = s.italic();
            }
            if let Some(h) = href.filter(|h| super::html::safe_href(h)) {
                s = s.link_to(h);
            }
            Some(s)
        }
        Tok::Delim { .. } => None,
    }))
}

/// Inline Markdown as text runs, emphasis and links resolved.
fn inline_toks(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut toks: Vec<Tok> = Vec::new();
    let mut buf = String::new();
    let flush = |buf: &mut String, toks: &mut Vec<Tok>| {
        if !buf.is_empty() {
            toks.push(text_tok(std::mem::take(buf)));
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() && chars[i + 1].is_ascii_punctuation() => {
                buf.push(chars[i + 1]);
                i += 2;
            }
            '`' => {
                let n = run(&chars, i, '`');
                match find_run(&chars, i + n, '`', n) {
                    Some(end) => {
                        flush(&mut buf, &mut toks);
                        let code: String = chars[i + n..end].iter().collect();
                        let code = if code.len() > 2 && code.starts_with(' ') && code.ends_with(' ')
                        {
                            code[1..code.len() - 1].to_string()
                        } else {
                            code
                        };
                        toks.push(text_tok(code));
                        i = end + n;
                    }
                    None => {
                        buf.extend(&chars[i..i + n]);
                        i += n;
                    }
                }
            }
            '<' => match autolink(&chars, i) {
                Some((url, end)) => {
                    flush(&mut buf, &mut toks);
                    let href = if url.contains('@') && !url.contains(':') {
                        format!("mailto:{url}")
                    } else {
                        url.clone()
                    };
                    toks.push(Tok::Text {
                        text: url,
                        bold: false,
                        italic: false,
                        href: Some(href),
                    });
                    i = end;
                }
                None => {
                    buf.push(c);
                    i += 1;
                }
            },
            '!' if chars.get(i + 1) == Some(&'[') => match link(&chars, i + 1) {
                // An image: its alternative text.
                Some((label, _, end)) => {
                    flush(&mut buf, &mut toks);
                    toks.extend(inline_toks(&label));
                    i = end;
                }
                None => {
                    buf.push(c);
                    i += 1;
                }
            },
            '[' => match link(&chars, i) {
                Some((label, href, end)) => {
                    flush(&mut buf, &mut toks);
                    for mut t in inline_toks(&label) {
                        if let Tok::Text { href: h, .. } = &mut t {
                            *h = Some(href.clone());
                        }
                        toks.push(t);
                    }
                    i = end;
                }
                None => {
                    buf.push(c);
                    i += 1;
                }
            },
            '*' | '_' => {
                flush(&mut buf, &mut toks);
                let n = run(&chars, i, c);
                let before = i.checked_sub(1).map(|j| chars[j]);
                let after = chars.get(i + n).copied();
                let ws = |c: Option<char>| c.is_none_or(char::is_whitespace);
                let punct = |c: Option<char>| c.is_some_and(|c| c.is_ascii_punctuation());
                let left = !ws(after) && (!punct(after) || ws(before) || punct(before));
                let right = !ws(before) && (!punct(before) || ws(after) || punct(after));
                let (can_open, can_close) = if c == '*' {
                    (left, right)
                } else {
                    (
                        left && (!right || punct(before)),
                        right && (!left || punct(after)),
                    )
                };
                toks.push(Tok::Delim {
                    ch: c,
                    count: n,
                    original: n,
                    can_open,
                    can_close,
                });
                i += n;
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    flush(&mut buf, &mut toks);
    emphasis(&mut toks);
    // Delimiters left unmatched are text.
    toks.into_iter()
        .map(|t| match t {
            Tok::Delim { ch, count, .. } => text_tok(ch.to_string().repeat(count)),
            t => t,
        })
        .filter(|t| !matches!(t, Tok::Text { text, .. } if text.is_empty()))
        .collect()
}

/// How many `c` start at `i`.
fn run(chars: &[char], i: usize, c: char) -> usize {
    chars[i..].iter().take_while(|x| **x == c).count()
}

/// The next run of exactly `n` `c` from `from`.
fn find_run(chars: &[char], from: usize, c: char, n: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == c {
            let len = run(chars, i, c);
            if len == n {
                return Some(i);
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}

/// `<scheme:…>` or `<name@host>` at `i`: its URL and where it ends.
fn autolink(chars: &[char], i: usize) -> Option<(String, usize)> {
    let end = i + 1 + chars[i + 1..].iter().position(|c| *c == '>')?;
    let url: String = chars[i + 1..end].iter().collect();
    if url.is_empty() || url.contains(char::is_whitespace) || url.contains('<') {
        return None;
    }
    let scheme = url.split(':').next().unwrap_or("");
    let is_url = url.contains(':')
        && scheme.len() >= 2
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    let is_mail = !url.contains(':') && url.contains('@') && !url.starts_with('@');
    (is_url || is_mail).then_some((url, end + 1))
}

/// `[label](destination "title")` at `i`: its label, destination and end.
fn link(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    // The matching `]`.
    let mut depth = 0;
    let mut j = i;
    let close = loop {
        match chars.get(j)? {
            '\\' => j += 1,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break j;
                }
            }
            _ => {}
        }
        j += 1;
    };
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let mut k = close + 2;
    while chars.get(k).is_some_and(|c| c.is_whitespace()) {
        k += 1;
    }
    let mut dest = String::new();
    if chars.get(k) == Some(&'<') {
        k += 1;
        loop {
            match chars.get(k)? {
                '>' => {
                    k += 1;
                    break;
                }
                '\n' | '<' => return None,
                '\\' if chars.get(k + 1).is_some_and(char::is_ascii_punctuation) => {
                    dest.push(chars[k + 1]);
                    k += 2;
                }
                c => {
                    dest.push(*c);
                    k += 1;
                }
            }
        }
    } else {
        let mut parens = 0;
        loop {
            match chars.get(k) {
                None => return None,
                Some(c) if c.is_whitespace() => break,
                Some('(') => {
                    parens += 1;
                    dest.push('(');
                    k += 1;
                }
                Some(')') if parens == 0 => break,
                Some(')') => {
                    parens -= 1;
                    dest.push(')');
                    k += 1;
                }
                Some('\\') if chars.get(k + 1).is_some_and(char::is_ascii_punctuation) => {
                    dest.push(chars[k + 1]);
                    k += 2;
                }
                Some(c) => {
                    dest.push(*c);
                    k += 1;
                }
            }
        }
    }
    // An optional title, skipped.
    while chars.get(k).is_some_and(|c| c.is_whitespace()) {
        k += 1;
    }
    if let Some(&q @ ('"' | '\'' | '(')) = chars.get(k) {
        let end = if q == '(' { ')' } else { q };
        k += 1;
        while chars.get(k)? != &end {
            if chars[k] == '\\' {
                k += 1;
            }
            k += 1;
        }
        k += 1;
        while chars.get(k).is_some_and(|c| c.is_whitespace()) {
            k += 1;
        }
    }
    if chars.get(k) != Some(&')') {
        return None;
    }
    let label: String = chars[i + 1..close].iter().collect();
    Some((label, dest, k + 1))
}

/// Matches emphasis delimiters (CommonMark's algorithm, simplified): each
/// closer takes the nearest opener of its character before it; two
/// characters on both sides make bold, one italic.
fn emphasis(toks: &mut [Tok]) {
    let mut i = 0;
    while i < toks.len() {
        let Tok::Delim {
            ch,
            count,
            original,
            can_close: true,
            can_open: closer_opens,
        } = toks[i].clone()
        else {
            i += 1;
            continue;
        };
        if count == 0 {
            i += 1;
            continue;
        }
        // The nearest opener of the same character.
        let opener = (0..i).rev().find(|&j| match &toks[j] {
            Tok::Delim {
                ch: c,
                count: n,
                original: o,
                can_open: true,
                can_close: opener_closes,
            } => {
                *c == ch
                    && *n > 0
                    // The rule of three.
                    && !((*opener_closes || closer_opens)
                        && (o + original) % 3 == 0
                        && !(o % 3 == 0 && original % 3 == 0))
            }
            _ => false,
        });
        let Some(j) = opener else {
            i += 1;
            continue;
        };
        let Tok::Delim { count: open_n, .. } = toks[j] else {
            unreachable!()
        };
        let used = if open_n >= 2 && count >= 2 { 2 } else { 1 };
        for t in &mut toks[j + 1..i] {
            match t {
                Tok::Text { bold, italic, .. } => {
                    if used == 2 {
                        *bold = true;
                    } else {
                        *italic = true;
                    }
                }
                // Delimiters between them stay unmatched: text.
                Tok::Delim {
                    can_open,
                    can_close,
                    ..
                } => {
                    *can_open = false;
                    *can_close = false;
                }
            }
        }
        if let Tok::Delim { count, .. } = &mut toks[j] {
            *count -= used;
        }
        if let Tok::Delim { count, .. } = &mut toks[i] {
            *count -= used;
        }
        // The closer may close more (`***`): look at it again.
    }
}
