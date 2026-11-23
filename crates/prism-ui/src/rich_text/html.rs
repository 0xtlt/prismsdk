//! A [`RichText`] as HTML, and HTML (a paste) read into the model's
//! subset (#294).

use prism_core::{Span, normalize_spans, span};

use super::{BlockKind, RichBlock, RichText};

/// Whether a link target may be kept from pasted or imported content: not
/// a `javascript:`, `data:` or `vbscript:` URL (whatever their case, with
/// the spaces and control characters browsers ignore in them).
pub(crate) fn safe_href(href: &str) -> bool {
    let compact: String = href
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .take(16)
        .collect::<String>()
        .to_ascii_lowercase();
    !["javascript:", "data:", "vbscript:"]
        .iter()
        .any(|s| compact.starts_with(s))
}

// --- Writing -------------------------------------------------------------------

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>"),
            c => out.push(c),
        }
    }
}

/// The document as HTML.
pub(super) fn write(doc: &RichText) -> String {
    let mut out = String::new();
    let mut list: Option<BlockKind> = None;
    for b in &doc.blocks {
        let tag = match b.kind {
            BlockKind::Bullet | BlockKind::Numbered => "li",
            BlockKind::Heading(1) => "h1",
            BlockKind::Heading(2) => "h2",
            BlockKind::Heading(_) => "h3",
            BlockKind::Paragraph => "p",
        };
        let in_list = b.kind.is_list().then_some(b.kind);
        if list != in_list {
            match list {
                Some(BlockKind::Bullet) => out.push_str("</ul>"),
                Some(_) => out.push_str("</ol>"),
                None => {}
            }
            match in_list {
                Some(BlockKind::Bullet) => out.push_str("<ul>"),
                Some(_) => out.push_str("<ol>"),
                None => {}
            }
            list = in_list;
        }
        out.push('<');
        out.push_str(tag);
        out.push('>');
        inline(&b.spans, &mut out);
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    }
    match list {
        Some(BlockKind::Bullet) => out.push_str("</ul>"),
        Some(_) => out.push_str("</ol>"),
        None => {}
    }
    out
}

fn inline(spans: &[Span], out: &mut String) {
    for s in spans {
        let text = match s.chip_label() {
            Some(label) if s.is_chip() => label,
            _ if s.is_object() => continue,
            _ => s.text(),
        };
        if let Some(h) = s.href() {
            out.push_str("<a href=\"");
            escape(h, out);
            out.push_str("\">");
        }
        if s.is_bold() {
            out.push_str("<strong>");
        }
        if s.is_italic() {
            out.push_str("<em>");
        }
        escape(text, out);
        if s.is_italic() {
            out.push_str("</em>");
        }
        if s.is_bold() {
            out.push_str("</strong>");
        }
        if s.href().is_some() {
            out.push_str("</a>");
        }
    }
}

// --- Reading -------------------------------------------------------------------

/// What an open inline element sets: bold, italic, a link (`Some(None)`:
/// an `<a>` without a safe target).
#[derive(Default)]
struct Frame {
    tag: String,
    bold: Option<bool>,
    italic: Option<bool>,
    href: Option<Option<String>>,
}

struct Reader {
    blocks: Vec<RichBlock>,
    spans: Vec<Span>,
    kind: BlockKind,
    frames: Vec<Frame>,
    /// The kinds of the lists around (`<ul>`: bullets, `<ol>`: numbers).
    lists: Vec<BlockKind>,
    /// Inside `<pre>`: whitespace kept, a line per block.
    pre: usize,
}

impl Reader {
    fn marks(&self) -> (bool, bool, Option<String>) {
        let bold = self
            .frames
            .iter()
            .rev()
            .find_map(|f| f.bold)
            .unwrap_or(false);
        let italic = self
            .frames
            .iter()
            .rev()
            .find_map(|f| f.italic)
            .unwrap_or(false);
        let href = self
            .frames
            .iter()
            .rev()
            .find_map(|f| f.href.clone())
            .flatten();
        (bold, italic, href)
    }

    fn text(&mut self, raw: &str) {
        let decoded = decode(raw);
        if self.pre > 0 {
            let mut lines = decoded.split('\n');
            if let Some(first) = lines.next() {
                self.push(first.to_string());
            }
            for line in lines {
                self.end_block();
                self.push(line.to_string());
            }
            return;
        }
        // Whitespace collapsed, as browsers show it.
        let mut text = String::with_capacity(decoded.len());
        let mut space = false;
        for c in decoded.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                space = true;
            } else {
                if space {
                    text.push(' ');
                }
                space = false;
                text.push(if c == '\u{a0}' { ' ' } else { c });
            }
        }
        if space {
            text.push(' ');
        }
        // No space at a block's start, nor twice.
        let at_start = self.spans.iter().all(|s| s.text().is_empty());
        let after_space = self.spans.last().is_some_and(|s| s.text().ends_with(' '));
        let text = if at_start || after_space {
            text.trim_start().to_string()
        } else {
            text
        };
        self.push(text);
    }

    fn push(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let (bold, italic, href) = self.marks();
        let mut s = span(text);
        if bold {
            s = s.bold();
        }
        if italic {
            s = s.italic();
        }
        if let Some(h) = href {
            s = s.link_to(h);
        }
        self.spans.push(s);
    }

    /// Ends the block being read (if it holds text); the next one is of
    /// `kind`.
    fn end_block(&mut self) {
        let mut spans = normalize_spans(std::mem::take(&mut self.spans));
        // No space at its end.
        while let Some(last) = spans.last_mut() {
            let trimmed = last.text().trim_end().to_string();
            if trimmed.is_empty() && !last.is_object() {
                spans.pop();
            } else {
                *last = last.clone().with_text(trimmed);
                break;
            }
        }
        if !spans.is_empty() {
            self.blocks.push(RichBlock::new(self.kind, spans));
        }
    }

    /// The kind of a block starting here: the enclosing item's, or a paragraph.
    fn base_kind(&self) -> BlockKind {
        if self.frames.iter().any(|f| f.tag == "li") {
            *self.lists.last().unwrap_or(&BlockKind::Bullet)
        } else {
            BlockKind::Paragraph
        }
    }

    fn open(&mut self, tag: &str, attrs: &str) {
        match tag {
            // A style wins over the tag: word processors wrap a whole
            // paste in `<b style="font-weight:normal">`.
            "b" | "strong" | "i" | "em" | "cite" | "dfn" | "var" => {
                let style = attr(attrs, "style")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let (bold, italic) = style_marks(&style);
                if matches!(tag, "b" | "strong") {
                    self.inline(tag, bold.or(Some(true)), italic, None)
                } else {
                    self.inline(tag, bold, italic.or(Some(true)), None)
                }
            }
            "a" => {
                let href = attr(attrs, "href").filter(|h| safe_href(h) && !h.is_empty());
                self.inline(tag, None, None, Some(href))
            }
            "br" => {
                self.end_block();
                self.kind = self.base_kind();
            }
            "ul" | "ol" => {
                self.end_block();
                self.lists.push(if tag == "ul" {
                    BlockKind::Bullet
                } else {
                    BlockKind::Numbered
                });
            }
            "li" => {
                self.end_block();
                self.frames.push(Frame {
                    tag: tag.into(),
                    ..Frame::default()
                });
                self.kind = *self.lists.last().unwrap_or(&BlockKind::Bullet);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.end_block();
                let level = tag[1..].parse::<u8>().unwrap_or(3);
                self.kind = BlockKind::heading(level);
                self.frames.push(Frame {
                    tag: tag.into(),
                    ..Frame::default()
                });
            }
            "pre" => {
                self.end_block();
                self.kind = self.base_kind();
                self.pre += 1;
            }
            _ if is_block(tag) => {
                self.end_block();
                self.kind = self.base_kind();
            }
            // `<span style>`, `<font>`, … : their weight and style.
            _ => {
                let style = attr(attrs, "style")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let (bold, italic) = style_marks(&style);
                if bold.is_some() || italic.is_some() {
                    self.inline(tag, bold, italic, None);
                } else if !is_void(tag) {
                    self.inline(tag, None, None, None);
                }
            }
        }
    }

    fn inline(
        &mut self,
        tag: &str,
        bold: Option<bool>,
        italic: Option<bool>,
        href: Option<Option<String>>,
    ) {
        self.frames.push(Frame {
            tag: tag.into(),
            bold,
            italic,
            href,
        });
    }

    fn close(&mut self, tag: &str) {
        match tag {
            "ul" | "ol" => {
                self.end_block();
                self.lists.pop();
                self.kind = self.base_kind();
            }
            "pre" => {
                self.end_block();
                self.pre = self.pre.saturating_sub(1);
                self.kind = self.base_kind();
            }
            _ => {
                let block = tag == "li" || tag.starts_with('h') && tag.len() == 2 || is_block(tag);
                if block {
                    self.end_block();
                }
                if let Some(i) = self.frames.iter().rposition(|f| f.tag == tag) {
                    self.frames.truncate(i);
                }
                if block {
                    self.kind = self.base_kind();
                }
            }
        }
    }
}

/// Bold and italic set by a `style` attribute.
fn style_marks(style: &str) -> (Option<bool>, Option<bool>) {
    let mut bold = None;
    let mut italic = None;
    for decl in style.split(';') {
        let Some((prop, value)) = decl.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match prop.trim() {
            "font-weight" => {
                bold = Some(match value {
                    "bold" | "bolder" => true,
                    "normal" | "lighter" => false,
                    v => v.parse::<u16>().is_ok_and(|w| w >= 600),
                })
            }
            "font-style" => italic = Some(value == "italic" || value == "oblique"),
            _ => {}
        }
    }
    (bold, italic)
}

fn is_block(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "blockquote"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "aside"
            | "nav"
            | "figure"
            | "figcaption"
            | "table"
            | "tr"
            | "dl"
            | "dt"
            | "dd"
            | "address"
            | "hr"
            | "details"
            | "summary"
    )
}

fn is_void(tag: &str) -> bool {
    matches!(
        tag,
        "img" | "input" | "meta" | "link" | "hr" | "col" | "area" | "base" | "wbr" | "source"
    )
}

/// The value of attribute `name` in a tag's attributes, decoded.
fn attr(attrs: &str, name: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b'/'
        {
            i += 1;
        }
        let key = attrs[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                let from = i;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                value = attrs[from..i].to_string();
                i += 1;
            } else {
                let from = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = attrs[from..i].to_string();
            }
        }
        if key == name {
            return Some(decode(&value).trim().to_string());
        }
        if i == start {
            i += 1;
        }
    }
    None
}

/// HTML character references decoded (the usual named ones, and numbers).
fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .map(|e| e + 1);
        let (name, after) = match end {
            Some(e) if rest[e..].starts_with(';') => (&rest[1..e], &rest[e + 1..]),
            _ => {
                out.push('&');
                rest = &rest[1..];
                continue;
            }
        };
        let c = if let Some(num) = name.strip_prefix('#') {
            let n = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse::<u32>().ok(),
            };
            n.and_then(char::from_u32).filter(|c| *c != '\0')
        } else {
            named(name)
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = after;
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn named(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "ensp" | "emsp" | "thinsp" => ' ',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "times" => '×',
        "euro" => '€',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "agrave" => 'à',
        "acirc" => 'â',
        "ccedil" => 'ç',
        "ocirc" => 'ô',
        "ucirc" => 'û',
        "ugrave" => 'ù',
        "icirc" => 'î',
        "iuml" => 'ï',
        "euml" => 'ë',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "szlig" => 'ß',
        "ntilde" => 'ñ',
        _ => return None,
    })
}

/// Reads HTML into the model.
pub(super) fn read(html: &str) -> RichText {
    let mut r = Reader {
        blocks: Vec::new(),
        spans: Vec::new(),
        kind: BlockKind::Paragraph,
        frames: Vec::new(),
        lists: Vec::new(),
        pre: 0,
    };
    // Only the body of a whole document.
    let lower = html.to_ascii_lowercase();
    let mut rest = match lower.find("<body") {
        Some(i) => html[i..].split_once('>').map_or("", |(_, b)| b),
        None => html,
    };
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            r.text(rest);
            break;
        };
        r.text(&rest[..lt]);
        rest = &rest[lt..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.split_once("-->").map_or("", |(_, b)| b);
            continue;
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            rest = rest.split_once('>').map_or("", |(_, b)| b);
            continue;
        }
        let closing = rest.starts_with("</");
        let name_start = if closing { 2 } else { 1 };
        let name_len = rest[name_start..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != ':')
            .unwrap_or(rest.len() - name_start);
        if name_len == 0 {
            // A lone `<`: text.
            r.text("<");
            rest = &rest[1..];
            continue;
        }
        let tag = rest[name_start..name_start + name_len].to_ascii_lowercase();
        let Some(gt) = tag_end(rest) else {
            break;
        };
        let attrs = &rest[name_start + name_len..gt];
        rest = &rest[gt + 1..];
        if closing {
            r.close(&tag);
            continue;
        }
        // Content that is not text: skipped whole.
        if matches!(
            tag.as_str(),
            "script"
                | "style"
                | "template"
                | "head"
                | "title"
                | "noscript"
                | "iframe"
                | "object"
                | "svg"
                | "math"
                | "select"
                | "textarea"
        ) {
            let close = format!("</{tag}");
            let lower_rest = rest.to_ascii_lowercase();
            rest = match lower_rest.find(&close) {
                Some(i) => rest[i..].split_once('>').map_or("", |(_, b)| b),
                None => "",
            };
            continue;
        }
        let self_closing = attrs.trim_end().ends_with('/');
        r.open(&tag, attrs);
        if self_closing && !matches!(tag.as_str(), "br" | "hr") && !is_void(&tag) {
            r.close(&tag);
        }
        if tag == "hr" {
            r.end_block();
        }
    }
    r.end_block();
    RichText::new(r.blocks)
}

/// Where a tag that starts `s` ends (its `>`, outside quoted values).
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}
