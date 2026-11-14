//! A [`Backend`] that keeps the tree in memory and serializes it to HTML,
//! used for server-side rendering of the first paint.

use std::collections::HashMap;
use std::fmt::Write;

use prism_core::{Backend, NodeId, NodeKind, Patch};

use crate::css::css;

#[derive(Default)]
struct HtmlNode {
    kind: Option<NodeKind>,
    css: String,
    text: String,
    placeholder: String,
    clickable: bool,
    children: Vec<NodeId>,
}

#[derive(Default)]
pub struct HtmlBackend {
    nodes: HashMap<NodeId, HtmlNode>,
    root: Option<NodeId>,
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

impl HtmlBackend {
    fn with(&mut self, id: NodeId, f: impl FnOnce(&mut HtmlNode)) {
        if let Some(n) = self.nodes.get_mut(&id) {
            f(n);
        }
    }

    pub fn to_html(&self) -> String {
        let mut out = String::new();
        if let Some(root) = self.root {
            self.write(root, &mut out);
        }
        out
    }

    fn write(&self, id: NodeId, out: &mut String) {
        let Some(n) = self.nodes.get(&id) else { return };
        self.write_with(id, n, false, out);
    }

    fn write_with(&self, id: NodeId, n: &HtmlNode, hidden: bool, out: &mut String) {
        let kind = n.kind.unwrap_or(NodeKind::View);
        let cursor = if n.clickable && kind == NodeKind::View {
            "cursor:pointer;"
        } else {
            ""
        };
        let hidden = if hidden { " hidden" } else { "" };
        let attrs = format!(
            r#"data-pid="{id}" data-kind="{}" style="{}{cursor}"{hidden}"#,
            kind.name(),
            escape(&n.css)
        );
        match kind {
            NodeKind::TextInput => {
                let _ = write!(
                    out,
                    r#"<input type="text" {attrs} value="{}" placeholder="{}">"#,
                    escape(&n.text),
                    escape(&n.placeholder)
                );
            }
            NodeKind::Stack => {
                // Same markup as the JS client builds: header bar + body.
                let top = n.children.last().and_then(|c| self.nodes.get(c));
                let title = top.map(|t| t.text.as_str()).unwrap_or("");
                let back_hidden = if n.children.len() <= 1 { " hidden" } else { "" };
                let _ = write!(
                    out,
                    r#"<div {attrs} class="prism-stack"><div class="prism-header"><button class="prism-back"{back_hidden}>‹ Back</button><div class="prism-title">{}</div></div><div class="prism-body">"#,
                    escape(title)
                );
                let last = n.children.len().saturating_sub(1);
                for (i, c) in n.children.iter().enumerate() {
                    if let Some(child) = self.nodes.get(c) {
                        self.write_with(*c, child, i != last, out);
                    }
                }
                out.push_str("</div></div>");
            }
            kind => {
                let tag = if kind == NodeKind::Button {
                    "button"
                } else {
                    "div"
                };
                // A screen's text is its title (shown in the header), not content.
                let content = if kind == NodeKind::Screen {
                    ""
                } else {
                    n.text.as_str()
                };
                let _ = write!(out, "<{tag} {attrs}>{}", escape(content));
                for c in &n.children {
                    self.write(*c, out);
                }
                let _ = write!(out, "</{tag}>");
            }
        }
    }

    /// Title of the visible screen of the root stack, if any.
    pub fn screen_title(&self) -> Option<String> {
        let root = self.nodes.get(&self.root?)?;
        if root.kind != Some(NodeKind::Stack) {
            return None;
        }
        let top = self.nodes.get(root.children.last()?)?;
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
