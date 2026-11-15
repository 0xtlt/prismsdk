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
    /// `Style::inert`: the `inert` attribute.
    inert: bool,
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
                | Patch::ScrollIntoView { .. }
                | Patch::SetLayers { .. } => {}
            }
        }
    }
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
    if kind == NodeKind::View
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
        if let Some(root) = self.root {
            self.write(root, false, &mut out);
        }
        out
    }

    fn write(&self, id: NodeId, hidden: bool, out: &mut String) {
        let Some(n) = self.nodes.get(&id) else { return };
        let kind = n.kind.unwrap_or(NodeKind::View);
        let hidden = if hidden { " hidden" } else { "" };
        // One `style` attribute (browsers ignore a second one).
        let css = match kind {
            NodeKind::Picture => {
                let fit = if n.value == 1. { "contain" } else { "cover" };
                format!("{}object-fit:{fit};", n.css)
            }
            _ => n.css.clone(),
        };
        // A picture's description is its `alt` (like the client sets it).
        let aria = if kind == NodeKind::Picture {
            String::new()
        } else {
            a11y_attrs(kind, &n.a11y)
        };
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
        let attrs = format!(
            r#"data-pid="{id}" data-kind="{}" style="{}"{hidden}{inert}{aria}"#,
            kind.name(),
            escape(&css),
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
                    r#"<input type="{}" {attrs}{icon} value="{}" placeholder="{}"{}{list}>"#,
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
                    r#"<label class="prism-check" {attrs}><input type="checkbox"{checked}{disabled}><span>{}</span></label>"#,
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
                    r#"<textarea {attrs} class="prism-textarea" rows="3" placeholder="{}"{}>{}</textarea>"#,
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
                    r#"<input type="range" class="prism-slider" {attrs} min="{}" max="{}" step="{step}" value="{}">"#,
                    c.min, c.max, n.value
                );
            }
            NodeKind::Picker => {
                let c = prism_core::controls::PickerConfig::decode(&n.config);
                let selected = n.value as i64;
                let _ = write!(out, r#"<select class="prism-select" {attrs}>"#);
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
                    r#"<input type="{ty}" class="prism-date" {attrs} data-value="{}"{value}>"#,
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
                    r#"<img {attrs} class="prism-picture" src="{}" alt="{}" loading="lazy" decoding="async">"#,
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
                    r#"<input type="checkbox" role="switch" class="prism-switch" {attrs}{checked}>"#
                );
            }
            NodeKind::Stack => {
                // Same markup as the JS client builds: header bar + body.
                let top = kids.last().and_then(|c| self.nodes.get(c));
                let title = top.map(|t| t.text.as_str()).unwrap_or("");
                let back_hidden = if kids.len() <= 1 { " hidden" } else { "" };
                let back = escape(if self.env.back.is_empty() {
                    "Back"
                } else {
                    &self.env.back
                });
                let _ = write!(
                    out,
                    r#"<div {attrs} class="prism-stack"><div class="prism-header"><button class="prism-back" aria-label="{back}"{back_hidden}>‹ {back}</button><h1 class="prism-title" tabindex="-1">{}</h1></div><div class="prism-body">"#,
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
                let _ = write!(out, "<{tag} {attrs}{class}{label}>{}", escape(content));
                for c in &n.children {
                    self.write(*c, false, out);
                }
                let _ = write!(out, "</{tag}>");
            }
        }
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

/// Remote URLs as-is; anything else is an app asset under `/assets/`.
pub fn picture_url(src: &str) -> String {
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
