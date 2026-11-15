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
    value: f64,
    a11y: Accessibility,
    clickable: bool,
    children: Vec<NodeId>,
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
                Patch::SetIcon { id, icon } => self.with(*id, |n| n.icon = icon.clone()),
                Patch::SetAccessibility { id, a11y } => self.with(*id, |n| n.a11y = a11y.clone()),
                Patch::SetChildren { id, children } => {
                    self.with(*id, |n| n.children = children.clone())
                }
                Patch::SetRoot { id } => self.root = Some(*id),
                Patch::Destroy { id } => {
                    self.nodes.remove(id);
                }
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
    if kind != NodeKind::Toggle && matches!(a.role, Some(Role::Switch | Role::Checkbox)) {
        let _ = write!(out, r#" aria-checked="{}""#, a.checked.unwrap_or(false));
    }
    if let (Some(Role::Button), Some(pressed)) = (a.role, a.checked) {
        let _ = write!(out, r#" aria-pressed="{pressed}""#);
    }
    if a.role == Some(Role::Tab) {
        let _ = write!(out, r#" aria-selected="{}""#, a.checked.unwrap_or(false));
    }
    if a.disabled {
        out.push_str(r#" aria-disabled="true""#);
        if matches!(
            kind,
            NodeKind::Button | NodeKind::TextInput | NodeKind::Toggle
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
        let attrs = format!(
            r#"data-pid="{id}" data-kind="{}" style="{}"{hidden}{aria}"#,
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
                let _ = write!(
                    out,
                    r#"<input type="text" {attrs} value="{}" placeholder="{}">"#,
                    escape(&n.text),
                    escape(&n.placeholder)
                );
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
