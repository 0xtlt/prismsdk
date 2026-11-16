//! A [`Backend`] that keeps the tree in memory and serializes it to HTML,
//! used for server-side rendering of the first paint. The markup and ARIA
//! attributes match what `client.js` builds, so the page is accessible before
//! any script runs.

use std::collections::HashMap;
use std::fmt::Write;

use prism_core::{Accessibility, Appearance, Backend, NodeId, NodeKind, Patch, Role};

use crate::css::css;

#[derive(Default)]
struct HtmlNode {
    kind: Option<NodeKind>,
    css: String,
    text: String,
    placeholder: String,
    icon: String,
    config: String,
    value: f64,
    a11y: Accessibility,
    clickable: bool,
    children: Vec<NodeId>,
    /// A horizontal scroll view (`prism-scroll-x`).
    hscroll: bool,
    /// Interaction refinements as CSS rules (`css::states_css`).
    states: String,
    /// Descendants' `group_*` refinements follow it (`data-group`).
    group: bool,
    /// `Style::inert`: the `inert` attribute.
    inert: bool,
    /// An island: its tag and whether it is a WASM island.
    island: Option<(String, bool)>,
    /// An island's props (JSON).
    island_props: std::collections::BTreeMap<String, String>,
    /// A link's destination (`Patch::SetLink`): an `<a href>`.
    link: Option<String>,
}

/// Turns a picture source (`logo.png`, `https://…`) into its URL.
pub type AssetUrls = std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Default)]
pub struct HtmlBackend {
    nodes: HashMap<NodeId, HtmlNode>,
    root: Option<NodeId>,
    appearance: Appearance,
    pub env: prism_core::RenderEnvironment,
    /// Hashed asset URLs (`/assets/logo.<hash>.png`); `/assets/<name>` if unset.
    pub asset_urls: Option<AssetUrls>,
    /// Where the app is mounted (`/app`, empty at the root): links' `href`s
    /// start with it.
    pub base: String,
    /// While [`to_html_and_css`](HtmlBackend::to_html_and_css) runs: the
    /// rules collected instead of `style` attributes.
    sheet: std::cell::RefCell<Option<Sheet>>,
}

/// The first paint's styles as rules: one per distinct declaration set,
/// matched by a `data-ps` attribute holding the style's short hash (no
/// `style` attribute, so a strict `style-src` without `'unsafe-inline'`
/// allows the page).
#[derive(Default)]
struct Sheet {
    ids: HashMap<String, String>,
    taken: std::collections::HashSet<String>,
    css: String,
}

/// A style's short name: `p` + the FNV-1a hash of its code points in base
/// 36, the class `client.js` gives nodes with this style (`styleHash`).
pub(crate) fn style_class(css: &str) -> String {
    let mut h: u32 = 0x811c9dc5;
    for c in css.chars() {
        h = (h ^ c as u32).wrapping_mul(0x01000193);
    }
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit(h % 36, 36).unwrap_or('0'));
        h /= 36;
        if h == 0 {
            break;
        }
    }
    std::iter::once('p')
        .chain(digits.into_iter().rev())
        .collect()
}

impl Sheet {
    /// ` data-ps="pHASH"` for `css` (nothing when empty), adding its rule once.
    fn attr(&mut self, css: &str) -> String {
        if css.is_empty() {
            return String::new();
        }
        if let Some(id) = self.ids.get(css) {
            return format!(r#" data-ps="{id}""#);
        }
        // Two styles with one hash (rare): the second gets a suffix.
        let base = style_class(css);
        let mut id = base.clone();
        let mut n = 2;
        while self.taken.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        // `#prism-root` outranks Prism's own sheet like an inline style.
        let _ = writeln!(
            self.css,
            "#prism-root [data-ps=\"{id}\"]{{{}}}",
            crate::host::css_text(css)
        );
        self.taken.insert(id.clone());
        self.ids.insert(css.to_string(), id.clone());
        format!(r#" data-ps="{id}""#)
    }
}

impl Backend for HtmlBackend {
    fn apply(&mut self, patches: &[Patch]) {
        for p in patches {
            match p {
                Patch::Create { id, kind } => {
                    self.nodes.insert(
                        *id,
                        HtmlNode {
                            kind: Some(*kind),
                            ..Default::default()
                        },
                    );
                }
                Patch::SetStyle { id, style } => {
                    if let Some(n) = self.nodes.get_mut(id) {
                        n.css = css(n.kind.unwrap_or(NodeKind::View), style);
                        n.hscroll = style.scroll_axis == prism_core::ScrollAxis::Horizontal;
                        n.group = style.group;
                        n.states =
                            crate::css::states_css(n.kind.unwrap_or(NodeKind::View), *id, style);
                        n.inert = style.inert;
                    }
                }
                Patch::SetText { id, text } => self.with(*id, |n| n.text = text.clone()),
                Patch::SetPlaceholder { id, text } => {
                    self.with(*id, |n| n.placeholder = text.clone())
                }
                Patch::SetClickable { id, clickable } => {
                    self.with(*id, |n| n.clickable = *clickable)
                }
                Patch::SetAppearance { appearance } => self.appearance = *appearance,
                Patch::SetEnvironment { env } => self.env = env.clone(),
                // Menus open on interaction, once the client runs.
                Patch::SetMenu { .. } | Patch::SetSwipe { .. } | Patch::SetScrollEvents { .. } => {}
                Patch::SetValue { id, value } => self.with(*id, |n| n.value = *value),
                Patch::SetConfig { id, config } => self.with(*id, |n| n.config = config.clone()),
                Patch::SetIcon { id, icon } => self.with(*id, |n| n.icon = icon.clone()),
                Patch::SetAccessibility { id, a11y } => self.with(*id, |n| n.a11y = a11y.clone()),
                Patch::SetChildren { id, children } => {
                    self.with(*id, |n| n.children = children.clone())
                }
                Patch::SetRoot { id } => self.root = Some(*id),
                Patch::Destroy { id } => {
                    self.nodes.remove(id);
                }
                // Applied by the client once it runs.
                Patch::SetPointerEvents { .. }
                | Patch::SetKeyEvents { .. }
                | Patch::Focus { .. }
                | Patch::DismissKeyboard
                | Patch::ScrollIntoView { .. }
                | Patch::SetLayers { .. } => {}
                Patch::SetIsland {
                    id, tag, source, ..
                } => {
                    let wasm = matches!(source, prism_core::IslandSource::Wasm(_));
                    self.with(*id, |n| n.island = Some((tag.clone(), wasm)))
                }
                Patch::SetLink { id, href } => self.with(*id, |n| n.link.clone_from(href)),
                Patch::SetIslandProps { id, props } => self.with(*id, |n| {
                    for (k, v) in props {
                        n.island_props.insert(k.clone(), v.clone());
                    }
                }),
            }
        }
    }
}

/// An island's attributes in the first paint: a web component's string,
/// number and boolean props (like `client.js` reflects them).
fn island_attrs(n: &HtmlNode, wasm: bool) -> String {
    let mut out = String::from(" data-island");
    if wasm {
        return out;
    }
    for (name, json) in &n.island_props {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(json) {
            Ok(serde_json::Value::String(s)) => {
                let _ = write!(out, r#" {name}="{}""#, escape(&s));
            }
            Ok(serde_json::Value::Number(v)) => {
                let _ = write!(out, r#" {name}="{v}""#);
            }
            Ok(serde_json::Value::Bool(true)) => {
                let _ = write!(out, " {name}");
            }
            _ => {}
        }
    }
    out
}

/// ARIA attributes for a node (mirrors `applyA11y` in client.js).
pub fn a11y_attrs(kind: NodeKind, a: &Accessibility) -> String {
    let mut out = String::new();
    if a.hidden {
        out.push_str(r#" aria-hidden="true""#);
    }
    if matches!(
        kind,
        NodeKind::Stack | NodeKind::Tabs | NodeKind::Tab | NodeKind::Screen
    ) {
        return out;
    }
    let native = match kind {
        NodeKind::Button => Some(Role::Button),
        NodeKind::Toggle => Some(Role::Switch),
        NodeKind::Slider => Some(Role::Slider),
        NodeKind::Picker => Some(Role::ComboBox),
        NodeKind::Checkbox => Some(Role::Checkbox),
        _ => None,
    };
    if let Some(role) = a.role.filter(|r| *r != Role::Text && Some(*r) != native) {
        let _ = write!(out, r#" role="{}""#, role.aria());
        if let Role::Heading(level) = role {
            let _ = write!(out, r#" aria-level="{level}""#);
        }
    }
    if let Some(label) = &a.label {
        let _ = write!(out, r#" aria-label="{}""#, escape(label));
    }
    if let Some(hint) = &a.hint {
        let _ = write!(out, r#" aria-description="{}""#, escape(hint));
    }
    if !matches!(kind, NodeKind::Toggle | NodeKind::Checkbox)
        && matches!(a.role, Some(Role::Switch | Role::Checkbox))
    {
        let _ = write!(out, r#" aria-checked="{}""#, a.checked.unwrap_or(false));
    }
    if let (Some(Role::Button), Some(pressed)) = (a.role, a.checked) {
        let _ = write!(out, r#" aria-pressed="{pressed}""#);
    }
    if a.role == Some(Role::Tab) {
        let _ = write!(out, r#" aria-selected="{}""#, a.checked.unwrap_or(false));
    }
    if let (Some(Role::Slider), Some(v)) = (a.role, &a.value) {
        let _ = write!(out, r#" aria-valuetext="{}""#, escape(v));
    }
    if a.role == Some(Role::Status) {
        out.push_str(r#" aria-live="polite""#);
    }
    if a.disabled {
        out.push_str(r#" aria-disabled="true""#);
        if matches!(
            kind,
            NodeKind::Button
                | NodeKind::TextInput
                | NodeKind::Toggle
                | NodeKind::Checkbox
                | NodeKind::TextArea
                | NodeKind::Slider
                | NodeKind::Picker
                | NodeKind::DatePicker
        ) {
            out.push_str(" disabled");
        }
    }
    if matches!(kind, NodeKind::View | NodeKind::Text)
        && a.role.is_some_and(Role::is_interactive)
        && !a.hidden
        && !a.disabled
    {
        out.push_str(r#" tabindex="0""#);
    }
    out
}

impl HtmlBackend {
    fn with(&mut self, id: NodeId, f: impl FnOnce(&mut HtmlNode)) {
        if let Some(n) = self.nodes.get_mut(&id) {
            f(n);
        }
    }

    fn kind(&self, id: NodeId) -> Option<NodeKind> {
        self.nodes.get(&id).and_then(|n| n.kind)
    }

    fn write_sheets(&self, sheets: &[NodeId], out: &mut String) {
        for s in sheets {
            self.write(*s, false, out);
        }
    }

    pub fn to_html(&self) -> String {
        let mut out = String::new();
        // Hover, press and focus rules until the client takes over (it
        // replaces the root, and this with it).
        let mut ids: Vec<&NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| !n.states.is_empty())
            .map(|(id, _)| id)
            .collect();
        if !ids.is_empty() {
            ids.sort();
            // Rules never contain `<`: nothing can close the element.
            let rules: String = ids
                .into_iter()
                .map(|id| self.nodes[id].states.replace('<', ""))
                .collect();
            match self.sheet.borrow_mut().as_mut() {
                // A page: in its one `<style>` block, with the CSP nonce.
                Some(sheet) => sheet.css.push_str(&rules),
                None => {
                    out.push_str("<style data-prism-states>");
                    out.push_str(&rules);
                    out.push_str("</style>");
                }
            }
        }
        if let Some(root) = self.root {
            self.write(root, false, &mut out);
        }
        out
    }

    fn write(&self, id: NodeId, hidden: bool, out: &mut String) {
        let Some(n) = self.nodes.get(&id) else { return };
        let kind = n.kind.unwrap_or(NodeKind::View);
        let hidden = if hidden { " hidden" } else { "" };
        let css = n.css.clone();
        // A view or a text made a link is an `<a href>` (not an island,
        // whose tag is its custom element's): a link by itself, focusable.
        let link = match (&n.link, &n.island, kind) {
            (Some(path), None, NodeKind::View | NodeKind::Text) => Some(path.as_str()),
            _ => None,
        };
        // A picture's description is its `alt` (like the client sets it).
        let aria = if kind == NodeKind::Picture {
            String::new()
        } else if link.is_some() {
            let mut a = n.a11y.clone();
            if a.role == Some(Role::Link) {
                a.role = None;
            }
            // Focusable by itself; out of the tab order while hidden or
            // disabled (like client.js).
            let mut attrs = a11y_attrs(kind, &a).replace(r#" tabindex="0""#, "");
            if a.hidden || a.disabled {
                attrs.push_str(r#" tabindex="-1""#);
            }
            attrs
        } else {
            a11y_attrs(kind, &n.a11y)
        };
        let group = if n.group { " data-group" } else { "" };
        let inert = if n.inert { " inert" } else { "" };
        // A checkbox's accessible name is its visible label.
        let aria = if kind == NodeKind::Checkbox {
            if n.a11y.hidden {
                r#" aria-hidden="true""#.to_string()
            } else {
                String::new()
            }
        } else {
            aria
        };
        let style = match self.sheet.borrow_mut().as_mut() {
            Some(sheet) => sheet.attr(&css),
            None => format!(r#" style="{}""#, escape(&css)),
        };
        let attrs = format!(
            r#"data-pid="{id}" data-kind="{}"{style}{hidden}{inert}{aria}{group}"#,
            kind.name(),
        );
        // Sheets attached to a stack or tab bar are not screens or tabs: they
        // open as dialogs on top (like the client does).
        let (sheets, kids): (Vec<NodeId>, Vec<NodeId>) = n
            .children
            .iter()
            .partition(|c| self.kind(**c) == Some(NodeKind::Modal));
        match kind {
            NodeKind::TextInput => {
                let suggestions = prism_core::controls::input_suggestions(&n.config);
                let list = if suggestions.is_empty() {
                    String::new()
                } else {
                    format!(r#" list="prism-list-{id}""#)
                };
                let icon = if n.icon.is_empty() {
                    String::new()
                } else {
                    format!(
                        r#" class="prism-icon-field" data-icon="{}""#,
                        escape(prism_core::glyphs::glyph(&n.icon))
                    )
                };
                let _ = write!(
                    out,
                    r#"<input type="{}" name="prism-{id}" {attrs}{icon} value="{}" placeholder="{}"{}{list}>"#,
                    input_type(&n.config),
                    escape(&n.text),
                    escape(&n.placeholder),
                    input_attrs(&n.config)
                );
                if !suggestions.is_empty() {
                    let _ = write!(out, r#"<datalist id="prism-list-{id}">"#);
                    for s in &suggestions {
                        let _ = write!(out, r#"<option value="{}"></option>"#, escape(s));
                    }
                    out.push_str("</datalist>");
                }
            }
            NodeKind::Checkbox => {
                let checked = if n.value != 0. { " checked" } else { "" };
                let disabled = if n.a11y.disabled { " disabled" } else { "" };
                let _ = write!(
                    out,
                    r#"<label class="prism-check" {attrs}><input type="checkbox" name="prism-{id}"{checked}{disabled}><span>{}</span></label>"#,
                    escape(&n.text)
                );
            }
            NodeKind::RadioGroup => {
                let c = prism_core::controls::RadioConfig::decode(&n.config);
                let row = if c.horizontal { r#" data-row="1""# } else { "" };
                let _ = write!(out, r#"<div class="prism-radios" {attrs}{row}>"#);
                let disabled = if n.a11y.disabled { " disabled" } else { "" };
                for (i, o) in c.options.iter().enumerate() {
                    let checked = if i as f64 == n.value { " checked" } else { "" };
                    let _ = write!(
                        out,
                        r#"<label><input type="radio" name="prism-radio-{id}" value="{i}"{checked}{disabled}><span>{}</span></label>"#,
                        escape(o)
                    );
                }
                out.push_str("</div>");
            }
            // Closed until the client opens it (its content is in the top layer).
            NodeKind::Overlay => {
                let _ = write!(out, r#"<div class="prism-overlay" {attrs}></div>"#);
            }
            NodeKind::TextArea => {
                let _ = write!(
                    out,
                    r#"<textarea name="prism-{id}" {attrs} class="prism-textarea" rows="3" placeholder="{}"{}>{}</textarea>"#,
                    escape(&n.placeholder),
                    input_attrs(&n.config),
                    escape(&n.text)
                );
            }
            NodeKind::Slider => {
                let c = prism_core::controls::SliderConfig::decode(&n.config);
                let step = if c.step > 0. {
                    c.step.to_string()
                } else {
                    "any".into()
                };
                let _ = write!(
                    out,
                    r#"<input type="range" name="prism-{id}" class="prism-slider" {attrs} min="{}" max="{}" step="{step}" value="{}">"#,
                    c.min, c.max, n.value
                );
            }
            NodeKind::Picker => {
                let c = prism_core::controls::PickerConfig::decode(&n.config);
                let selected = n.value as i64;
                let _ = write!(
                    out,
                    r#"<select name="prism-{id}" class="prism-select" {attrs}>"#
                );
                if selected < 0 {
                    let _ = write!(
                        out,
                        r#"<option value="" disabled selected hidden>{}</option>"#,
                        escape(&n.placeholder)
                    );
                }
                for (i, o) in c.options.iter().enumerate() {
                    let sel = if i as i64 == selected {
                        " selected"
                    } else {
                        ""
                    };
                    let _ = write!(out, r#"<option value="{i}"{sel}>{}</option>"#, escape(o));
                }
                out.push_str("</select>");
            }
            // The browser's time zone formats it: the client fills it in.
            NodeKind::DatePicker => {
                let c = prism_core::controls::DateConfig::decode(&n.config);
                let ty = match c.mode {
                    prism_core::DateMode::Date => "date",
                    prism_core::DateMode::Time | prism_core::DateMode::TimeOfDay => "time",
                    prism_core::DateMode::DateTime => "datetime-local",
                };
                // A time of day is the same everywhere: written as is.
                let value = if c.mode == prism_core::DateMode::TimeOfDay {
                    let m = (n.value / 60.).round().rem_euclid(1440.) as u32;
                    format!(r#" value="{:02}:{:02}""#, m / 60, m % 60)
                } else {
                    String::new()
                };
                let _ = write!(
                    out,
                    r#"<input type="{ty}" name="prism-{id}" class="prism-date" {attrs} data-value="{}"{value}>"#,
                    n.value
                );
            }
            NodeKind::Spinner => {
                let size = if n.value == 1. {
                    r#" data-size="large""#
                } else {
                    ""
                };
                let _ = write!(out, r#"<span class="prism-spinner" {attrs}{size}></span>"#);
            }
            NodeKind::Image => {
                let _ = write!(
                    out,
                    r#"<span {attrs} class="prism-symbol">{}</span>"#,
                    prism_core::glyphs::glyph(&n.icon)
                );
            }
            NodeKind::Picture => {
                let alt = match &n.a11y.label {
                    Some(label) if !n.a11y.hidden => label.as_str(),
                    _ => "",
                };
                let _ = write!(
                    out,
                    r#"<img {attrs} class="prism-picture{}" src="{}" alt="{}" loading="lazy" decoding="async">"#,
                    // Fill (cover) or fit (contain), as in `prism.css`.
                    if n.value == 1. { " prism-fit" } else { "" },
                    escape(&match &self.asset_urls {
                        Some(urls) => urls(&n.text),
                        None => picture_url(&n.text),
                    }),
                    escape(alt)
                );
            }
            // Drawn by the client once the box has a size (see client.js).
            NodeKind::Shape => {
                let _ = write!(out, r#"<div {attrs} class="prism-shape"></div>"#);
            }
            NodeKind::Progress => {
                let _ = write!(
                    out,
                    r#"<progress {attrs} max="1" value="{}"></progress>"#,
                    n.value
                );
            }
            NodeKind::Toggle => {
                let checked = if n.value != 0. { " checked" } else { "" };
                let _ = write!(
                    out,
                    r#"<input type="checkbox" role="switch" name="prism-{id}" class="prism-switch" {attrs}{checked}>"#
                );
            }
            NodeKind::Stack => {
                // Same markup as the JS client builds: header bar + body.
                let top = kids.last().and_then(|c| self.nodes.get(c));
                let title = top.map(|t| t.text.as_str()).unwrap_or("");
                let back_hidden = if kids.len() <= 1 { " hidden" } else { "" };
                // No header bar: `Router::header(false)` or a bare top screen.
                let no_header = (n.value as u32) & prism_core::STACK_NO_HEADER != 0
                    || top.is_some_and(|t| t.value == 1.);
                let header_hidden = if no_header { " hidden" } else { "" };
                let back = escape(if self.env.back.is_empty() {
                    "Back"
                } else {
                    &self.env.back
                });
                // The top screen's bar buttons, after Back and after the title (#30).
                let bar = |trailing| {
                    top.map_or(String::new(), |t| {
                        crate::navbar::buttons(&t.config, trailing)
                    })
                };
                let (leading, trailing) = (bar(false), bar(true));
                let _ = write!(
                    out,
                    r#"<div {attrs} class="prism-stack"><div class="prism-header"{header_hidden}><div class="prism-bar prism-bar-leading"><button class="prism-back" aria-label="{back}"{back_hidden}>‹ {back}</button>{leading}</div><h1 class="prism-title" tabindex="-1">{}</h1><div class="prism-bar prism-bar-trailing">{trailing}</div></div><div class="prism-body">"#,
                    escape(title)
                );
                let last = kids.len().saturating_sub(1);
                for (i, c) in kids.iter().enumerate() {
                    self.write(*c, i != last, out);
                }
                out.push_str("</div>");
                self.write_sheets(&sheets, out);
                out.push_str("</div>");
            }
            NodeKind::Tabs => {
                let selected = n.value as usize;
                let _ = write!(
                    out,
                    r#"<div {attrs} class="prism-tabs"><div class="prism-panels">"#
                );
                for (i, c) in kids.iter().enumerate() {
                    self.write(*c, i != selected, out);
                }
                out.push_str(r#"</div><div class="prism-tabbar" role="tablist">"#);
                for (i, c) in kids.iter().enumerate() {
                    let tab = self.nodes.get(c);
                    let title = tab.map(|t| t.text.as_str()).unwrap_or("");
                    let icon = match tab.map(|t| t.icon.as_str()) {
                        Some("") | None => "",
                        Some(icon) => prism_core::glyphs::glyph(icon),
                    };
                    let sel = i == selected;
                    let _ = write!(
                        out,
                        r#"<button type="button" role="tab" class="prism-tab" id="prism-tab-{c}" aria-controls="prism-panel-{c}" aria-selected="{sel}" tabindex="{}"><span class="prism-tab-icon" aria-hidden="true">{icon}</span><span>{}</span></button>"#,
                        if sel { 0 } else { -1 },
                        escape(title)
                    );
                }
                out.push_str("</div>");
                self.write_sheets(&sheets, out);
                out.push_str("</div>");
            }
            NodeKind::Modal => {
                let _ = write!(
                    out,
                    r#"<dialog {attrs} class="prism-sheet" aria-modal="true" open>"#
                );
                for c in &n.children {
                    self.write(*c, false, out);
                }
                out.push_str("</dialog>");
            }
            NodeKind::Tab => {
                let _ = write!(
                    out,
                    r#"<div {attrs} id="prism-panel-{id}" role="tabpanel" aria-labelledby="prism-tab-{id}">"#
                );
                for c in &n.children {
                    self.write(*c, false, out);
                }
                out.push_str("</div>");
            }
            kind => {
                let (tag, class) = match kind {
                    NodeKind::Button => ("button", ""),
                    NodeKind::Screen => ("section", ""),
                    NodeKind::Scroll if n.hscroll => {
                        ("div", r#" class="prism-scroll prism-scroll-x""#)
                    }
                    NodeKind::Scroll => ("div", r#" class="prism-scroll""#),
                    _ => ("div", ""),
                };
                let label = if kind == NodeKind::Screen {
                    format!(r#" aria-label="{}""#, escape(&n.text))
                } else {
                    String::new()
                };
                // A screen's text is its title (shown in the header), not content.
                let content = if kind == NodeKind::Screen {
                    ""
                } else {
                    n.text.as_str()
                };
                // A rich text's runs become inline spans and links (spans
                // inside a text that is a link: links don't nest).
                let content = if kind == NodeKind::Text && !n.config.is_empty() {
                    let base = if link.is_some() {
                        None
                    } else {
                        Some(self.base.as_str())
                    };
                    // A page: the runs' styles join the `<style>` block too.
                    match self.sheet.borrow_mut().as_mut() {
                        Some(sheet) => runs_html_with(&n.config, base, &mut |css| sheet.attr(css)),
                        None => runs_html_with(&n.config, base, &mut |css| {
                            format!(r#" style="{}""#, escape(css))
                        }),
                    }
                } else {
                    escape(content)
                };
                // An island: its custom element, around the fallback.
                let (tag, island) = match (&n.island, link) {
                    (Some((island_tag, wasm)), _) => (island_tag.as_str(), island_attrs(n, *wasm)),
                    (None, Some(path)) => ("a", link_attrs(&self.base, path)),
                    (None, None) => (tag, String::new()),
                };
                let _ = write!(out, "<{tag} {attrs}{class}{label}{island}>{content}");
                for c in &n.children {
                    self.write(*c, false, out);
                }
                let _ = write!(out, "</{tag}>");
            }
        }
    }

    /// The markup without any `style` attribute, and the CSS rules that
    /// style it (for a `<style>` element, with the page's CSP nonce). Nodes
    /// carry `data-ps="pHASH"`; the rules match them inside `#prism-root`.
    pub fn to_html_and_css(&self) -> (String, String) {
        *self.sheet.borrow_mut() = Some(Sheet::default());
        let html = self.to_html();
        let sheet = self.sheet.borrow_mut().take().unwrap_or_default();
        (html, sheet.css)
    }

    /// CSS `color-scheme` for the page.
    pub fn color_scheme(&self) -> &'static str {
        match self.appearance {
            Appearance::System => "light dark",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }

    /// Title of the visible screen (root stack or selected tab), if any.
    pub fn screen_title(&self) -> Option<String> {
        let screens = |n: &HtmlNode| -> Vec<NodeId> {
            n.children
                .iter()
                .copied()
                .filter(|c| self.kind(*c) != Some(NodeKind::Modal))
                .collect()
        };
        let mut node = self.nodes.get(&self.root?)?;
        if node.kind == Some(NodeKind::Tabs) {
            let tab = self.nodes.get(screens(node).get(node.value as usize)?)?;
            node = self.nodes.get(tab.children.first()?)?;
        }
        if node.kind != Some(NodeKind::Stack) {
            return None;
        }
        let top = self.nodes.get(screens(node).last()?)?;
        Some(top.text.clone()).filter(|t| !t.is_empty())
    }
}

/// The `type` of an `<input>` for a text input config.
pub fn input_type(config: &str) -> &'static str {
    use prism_core::controls::{InputConfig, Keyboard};
    let c = InputConfig::decode(config);
    match c.keyboard {
        _ if c.secure => "password",
        Keyboard::Email => "email",
        Keyboard::Url => "url",
        Keyboard::Phone => "tel",
        _ => "text",
    }
}

/// `inputmode`, `enterkeyhint`, `autocapitalize`, `autocorrect`, `spellcheck`
/// (mirrors `applyInputConfig` in client.js).
pub fn input_attrs(config: &str) -> String {
    use prism_core::controls::{Capitalize, InputConfig, Keyboard, ReturnKey};
    let c = InputConfig::decode(config);
    let mut out = String::new();
    let mode = match c.keyboard {
        Keyboard::Number => Some("numeric"),
        Keyboard::Decimal => Some("decimal"),
        _ => None,
    };
    if let Some(m) = mode {
        let _ = write!(out, r#" inputmode="{m}""#);
    }
    if let Some(max) = c.max_length {
        let _ = write!(out, r#" maxlength="{max}""#);
    }
    let enter = match c.return_key {
        ReturnKey::Default => None,
        ReturnKey::Done => Some("done"),
        ReturnKey::Go => Some("go"),
        ReturnKey::Next => Some("next"),
        ReturnKey::Search => Some("search"),
        ReturnKey::Send => Some("send"),
    };
    if let Some(e) = enter {
        let _ = write!(out, r#" enterkeyhint="{e}""#);
    }
    let cap = match c.capitalize {
        _ if c.secure => Some("none"),
        Capitalize::Sentences => None,
        Capitalize::None => Some("none"),
        Capitalize::Words => Some("words"),
        Capitalize::All => Some("characters"),
    };
    if let Some(a) = cap {
        let _ = write!(out, r#" autocapitalize="{a}""#);
    }
    if !c.autocorrect || c.secure {
        out.push_str(r#" autocorrect="off" spellcheck="false""#);
    }
    out
}

pub fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            c => o.push(c),
        }
    }
    o
}

/// A rich text's runs (`prism_core::spans`) as inline HTML, the same
/// markup `client.js` builds: `<span>` per styled run, `<a class="prism-link"
/// href data-link>` per link (`<span role="link" tabindex="0">` for a
/// clickable run without a path), `<span class="prism-symbol">` per icon.
#[cfg(test)]
pub fn runs_html(config: &str) -> String {
    runs_html_with(config, Some(""), &mut |css| {
        format!(r#" style="{}""#, escape(css))
    })
}

/// [`runs_html`], each run's CSS (never empty) turned into attributes by
/// `style` (a `style` attribute, or a class of the page's sheet). Links'
/// app paths start with `base`; with `None` (the text is a link itself)
/// they are `<span>`s, reported by their click only.
fn runs_html_with(
    config: &str,
    base: Option<&str>,
    style: &mut dyn FnMut(&str) -> String,
) -> String {
    let mut out = String::new();
    for r in prism_core::decode_runs(config) {
        let css = crate::css::run_css(&r);
        let style = if css.is_empty() {
            String::new()
        } else {
            style(&css)
        };
        let text = escape(&r.text);
        if r.icon.is_some() {
            let _ = write!(
                out,
                r#"<span class="prism-symbol" aria-hidden="true"{style}>{text}</span>"#
            );
        } else if let Some(link) = r.link {
            match (&r.href, base) {
                (Some(href), Some(base)) => {
                    let _ = write!(
                        out,
                        r#"<a class="prism-link" href="{}" data-link="{link}"{style}>{text}</a>"#,
                        escape(&link_href(base, href))
                    );
                }
                (_, None) => {
                    let _ = write!(
                        out,
                        r#"<span class="prism-link" data-link="{link}"{style}>{text}</span>"#
                    );
                }
                (None, Some(_)) => {
                    let _ = write!(
                        out,
                        r#"<span class="prism-link" role="link" tabindex="0" data-link="{link}"{style}>{text}</span>"#
                    );
                }
            }
        } else if style.is_empty() {
            out.push_str(&text);
        } else {
            let _ = write!(out, "<span{style}>{text}</span>");
        }
    }
    out
}

/// A link's `href` (mirrors `urlFor` in client.js): a URL (`https:`,
/// `mailto:`…) as is, an app path under the mount point `base`.
pub fn link_href(base: &str, path: &str) -> String {
    if has_scheme(path) {
        return path.to_string();
    }
    let (p, query) = path.split_at(path.find('?').unwrap_or(path.len()));
    if p == "/" && !base.is_empty() {
        format!("{base}{query}")
    } else {
        format!("{base}{p}{query}")
    }
}

/// `scheme:` at the start (`/^[a-z][a-z0-9+.-]*:/i` in client.js).
fn has_scheme(path: &str) -> bool {
    let Some((scheme, _)) = path.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// ` href="…"` of a node made a link, with `rel="noopener"` for a URL.
fn link_attrs(base: &str, path: &str) -> String {
    let rel = if has_scheme(path) {
        r#" rel="noopener""#
    } else {
        ""
    };
    format!(r#" href="{}"{rel}"#, escape(&link_href(base, path)))
}

/// Remote URLs as-is; anything else is an app asset under `/assets/`.
/// Headers (`Picture::header`) are dropped: only a [`WebApp`](crate::WebApp)
/// can serve such a picture (its server fetches it with them).
pub fn picture_url(src: &str) -> String {
    let src = prism_core::picture_source(src).0;
    if src.starts_with("http://")
        || src.starts_with("https://")
        || src.starts_with("data:")
        || src.starts_with('/')
    {
        src.to_string()
    } else {
        format!("/assets/{src}")
    }
}
