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
        let cursor = if n.clickable && n.kind == Some(NodeKind::View) {
            "cursor:pointer;"
        } else {
            ""
        };
        let attrs = format!(r#"data-pid="{id}" style="{}{cursor}""#, escape(&n.css));
        match n.kind.unwrap_or(NodeKind::View) {
            NodeKind::TextInput => {
                let _ = write!(
                    out,
                    r#"<input type="text" {attrs} value="{}" placeholder="{}">"#,
                    escape(&n.text),
                    escape(&n.placeholder)
                );
            }
            kind => {
                let tag = if kind == NodeKind::Button {
                    "button"
                } else {
                    "div"
                };
                let _ = write!(out, "<{tag} {attrs}>{}", escape(&n.text));
                for c in &n.children {
                    self.write(*c, out);
                }
                let _ = write!(out, "</{tag}>");
            }
        }
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
