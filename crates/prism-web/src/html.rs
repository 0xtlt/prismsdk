//! A [`Backend`] that keeps the tree in memory and serializes it to HTML,
//! used for server-side rendering of the first paint. The markup and ARIA
//! attributes match what `client.js` builds, so the page is accessible before
//! any script runs.

use std::collections::HashMap;
use std::fmt::Write;

use prism_core::{Accessibility, Appearance, Backend, NodeId, NodeKind, Patch, Role};

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
    /// A scroll view's bars (#132): `data-prism-scrollbar`.
    scrollbar: prism_core::Scrollbar,
    /// Interaction refinements as CSS rules (`css::states_css`).
    states: String,
    /// Descendants' `group_*` refinements follow it (`data-group`).
    group: bool,
    /// A named group (#212): `data-gn`.
    group_name: Option<String>,
    /// `Style::inert`: the `inert` attribute.
    inert: bool,
    /// An island: its tag and whether it is a WASM island.
    island: Option<(String, bool)>,
    /// An island's props (JSON).
    island_props: std::collections::BTreeMap<String, String>,
    /// A 3D scene, as its updates left it.
    scene: Option<prism_core::scene3d::FlatScene>,
    /// A link's destination (`Patch::SetLink`): an `<a href>`.
    link: Option<String>,
    /// App-defined attributes (`.attr(…)`).
    attributes: Vec<(String, String)>,
    /// A rich input's runs (`Patch::SetRichValue`, #115).
    runs: String,
    /// A shape's box in pixels when its style fixes both sides (`w(px)`,
    /// `h(px)`, no border): its first paint is drawn at that size (#285).
    fixed_size: Option<(f32, f32)>,
}

/// A server-rendered page's parts ([`HtmlBackend::to_page`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PageHtml {
    /// The root's markup, the content of `<main id="prism-root">`.
    pub root: String,
    /// The layers' markup (`cx.add_layer`), bottom first, the content of
    /// `<div id="prism-layers">`.
    pub layers: String,
    /// The rules styling both (`data-ps="pHASH"` attributes), for a
    /// `<style>` element.
    pub css: String,
}

/// Turns a picture source (`logo.png`, `https://…`) into its URL.
pub type AssetUrls = std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>;

#[derive(Default)]
pub struct HtmlBackend {
    nodes: HashMap<NodeId, HtmlNode>,
    root: Option<NodeId>,
    /// The app's layers (`cx.add_layer`), bottom first (`Patch::SetLayers`).
    layers: Vec<NodeId>,
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
    /// While [`to_html`](HtmlBackend::to_html) runs: the nodes another one
    /// controls (`aria-controls`, #291), which get an `id`.
    controlled: std::cell::RefCell<std::collections::HashSet<NodeId>>,
    /// Where the app serves its schemes' pages (#343,
    /// `WebApp::web_scheme_path`): a web view's `data-src`.
    pub(crate) scheme_paths: Vec<(String, String)>,
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
        // An id outranks Prism's own sheet like an inline style; layers
        // (`#prism-layers`) are outside the root.
        let _ = writeln!(
            self.css,
            ":is(#prism-root,#prism-layers) [data-ps=\"{id}\"]{{{}}}",
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
                        let (kind, island) = (n.kind.unwrap_or(NodeKind::View), n.island.is_some());
                        n.css = crate::css::node_css(kind, island, style);
                        n.hscroll = style.scroll_axis == prism_core::ScrollAxis::Horizontal;
                        n.scrollbar = style.scrollbar;
                        n.group = style.group;
                        n.group_name = style.group_name.as_deref().map(str::to_string);
                        n.states = crate::css::states_css(kind, island, *id, style);
                        n.inert = style.inert;
                        n.fixed_size = match (style.width, style.height) {
                            (prism_core::Length::Px(w), prism_core::Length::Px(h))
                                if style.border_none || style.border_width <= 0. =>
                            {
                                Some((w, h))
                            }
                            _ => None,
                        };
                    }
                }
                Patch::SetText { id, text } => self.with(*id, |n| n.text = text.clone()),
                Patch::SetScene { id, update } => self.with(*id, |n| {
                    n.scene.get_or_insert_with(Default::default).apply(update)
                }),
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
                | Patch::SetDragDelay { .. }
                | Patch::SetDropTarget { .. }
                | Patch::SetDragAndDrop { .. }
                | Patch::SetKeyEvents { .. }
                | Patch::SetConsumedKeys { .. }
                | Patch::SetLayoutEvents { .. }
                | Patch::ScrollTo { .. }
                | Patch::SetSelection { .. }
                | Patch::SetSelectEvents { .. }
                | Patch::SetBackRequests { .. }
                | Patch::SetRemoteCarets { .. }
                | Patch::Focus { .. }
                | Patch::DismissKeyboard
                | Patch::ScrollIntoView { .. }
                | Patch::ShowMenu { .. }
                | Patch::PostWebMessage { .. }
                | Patch::ReloadWebView { .. }
                | Patch::SetEditEvents { .. }
                | Patch::WriteClipboard { .. }
                | Patch::InsertText { .. }
                | Patch::SetSelectionGroup { .. }
                | Patch::SetSelectionHighlight { .. }
                | Patch::SetSelectionAcross { .. }
                // The live page copies; the first paint has no script.
                | Patch::SetCopyOnClick { .. } => {}
                Patch::SetLayers { ids } => self.layers.clone_from(ids),
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
                Patch::SetAttributes { id, attributes } => {
                    self.with(*id, |n| n.attributes = attributes.clone())
                }
                Patch::SetRichValue { id, runs } => self.with(*id, |n| n.runs = runs.clone()),
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
        if !reflects_attr(name) {
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

/// Whether a web component's prop is reflected as an attribute (S20; the
/// same rule as `reflectsAttr` in client.js): an ordinary name, and never
/// an event handler (`on…`), styles, the element's identity or Prism's own
/// attributes. Props can come from data, and an attribute's escaped value
/// cannot make a handler safe.
pub(crate) fn reflects_attr(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !lower.starts_with("on")
        && !lower.starts_with("xmlns")
        && !matches!(
            lower.as_str(),
            "style"
                | "class"
                | "id"
                | "is"
                | "slot"
                | "part"
                | "nonce"
                | "data-pid"
                | "data-kind"
                | "data-island"
        )
}

/// ARIA attributes for a node (mirrors `applyA11y` in client.js).
pub fn a11y_attrs(kind: NodeKind, a: &Accessibility) -> String {
    let mut out = String::new();
    if a.hidden {
        out.push_str(r#" aria-hidden="true""#);
    }
    // A tooltip (#216); one shown while truncated waits for the client.
    if let Some(prism_core::Tooltip::Text(t)) = &a.tooltip {
        let _ = write!(out, r#" title="{}""#, escape(t));
    }
    if matches!(
        kind,
        NodeKind::Stack | NodeKind::Tabs | NodeKind::Tab | NodeKind::Screen
    ) {
        return out;
    }
    // A table's parts (#284) have their roles by their tags, and a cell is
    // read from its content (its label is for native screen readers).
    if kind.is_table_part() {
        if let Some(role) = a.role.filter(|r| Some(*r) != kind.default_role()) {
            let _ = write!(out, r#" role="{}""#, role.aria());
        }
        if let Some(label) = a.label.as_ref().filter(|_| !kind.is_table_cell()) {
            let _ = write!(out, r#" aria-label="{}""#, escape(label));
        }
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
        && matches!(a.role, Some(Role::Switch | Role::Checkbox | Role::Radio))
    {
        let _ = write!(out, r#" aria-checked="{}""#, a.checked.unwrap_or(false));
    }
    if let (Some(Role::Button), Some(pressed)) = (a.role, a.checked) {
        let _ = write!(out, r#" aria-pressed="{pressed}""#);
    }
    if a.role == Some(Role::Tab) {
        let selected = a.selected.or(a.checked).unwrap_or(false);
        let _ = write!(out, r#" aria-selected="{selected}""#);
    } else if let Some(selected) = a.selected {
        let _ = write!(out, r#" aria-selected="{selected}""#);
    }
    if let Some(expanded) = a.expanded {
        let _ = write!(out, r#" aria-expanded="{expanded}""#);
    }
    if a.current {
        let current = if a.role == Some(Role::Link) {
            "page"
        } else {
            "true"
        };
        let _ = write!(out, r#" aria-current="{current}""#);
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
    /// The 3D scenes of the page, with the blobs they hold (meshes,
    /// instances, clips…), taken out of the backend.
    pub(crate) fn take_scenes(&mut self) -> Vec<prism_core::scene3d::FlatScene> {
        self.nodes
            .values_mut()
            .filter_map(|n| n.scene.take())
            .collect()
    }

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
        *self.controlled.borrow_mut() = self
            .nodes
            .values()
            .filter_map(|n| n.a11y.controls_node)
            .filter(|t| self.nodes.contains_key(t))
            .collect();
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

    /// Node `id`'s element `id` (the one `client.js` gives it too).
    fn dom_id(&self, id: NodeId) -> String {
        if self.kind(id) == Some(NodeKind::Tab) {
            format!("prism-panel-{id}")
        } else {
            format!("prism-n{id}")
        }
    }

    fn write(&self, id: NodeId, hidden: bool, out: &mut String) {
        let Some(n) = self.nodes.get(&id) else { return };
        let kind = n.kind.unwrap_or(NodeKind::View);
        let hidden = if hidden { " hidden" } else { "" };
        let mut css = n.css.clone();
        // A picture's focal point (#218).
        if kind == NodeKind::Picture && !n.config.is_empty() {
            css += &object_position_css(&n.config);
        }
        // A text area's minimum lines (#235).
        let rows = prism_core::controls::InputConfig::decode(&n.config).min_rows();
        if kind == NodeKind::TextArea && rows != 3 {
            css += &format!("--prism-rows:{rows};");
        }
        // A view or a text made a link is an `<a href>` (not an island,
        // whose tag is its custom element's): a link by itself, focusable.
        let link = match (&n.link, &n.island, kind) {
            (Some(path), None, NodeKind::View | NodeKind::Text | NodeKind::Button) => {
                Some(path.as_str())
            }
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
            let mut attrs = a11y_attrs(kind, &a)
                .replace(r#" tabindex="0""#, "")
                // The current entry of a navigation is a page (#144).
                .replace(r#" aria-current="true""#, r#" aria-current="page""#);
            if a.hidden || a.disabled {
                attrs.push_str(r#" tabindex="-1""#);
            }
            attrs
        } else {
            a11y_attrs(kind, &n.a11y)
        };
        let mut group = if n.group { " data-group" } else { "" }.to_string();
        if let Some(name) = &n.group_name {
            group += &format!(r#" data-gn="{}""#, escape(name));
        }
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
        let app = app_attrs(&n.attributes);
        // A disclosure's region (#291), by the `id` it gets (a tab's panel
        // has its own).
        let controls = match n.a11y.controls_node {
            Some(t) if self.nodes.contains_key(&t) => {
                format!(r#" aria-controls="{}""#, self.dom_id(t))
            }
            _ => String::new(),
        };
        let dom_id = if kind != NodeKind::Tab && self.controlled.borrow().contains(&id) {
            format!(r#" id="{}""#, self.dom_id(id))
        } else {
            String::new()
        };
        let attrs = format!(
            r#"data-pid="{id}" data-kind="{}"{dom_id}{style}{hidden}{inert}{aria}{controls}{group}{app}"#,
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
                    r#"<textarea name="prism-{id}" {attrs} class="prism-textarea" rows="{rows}" placeholder="{}"{}>{}</textarea>"#,
                    escape(&n.placeholder),
                    input_attrs(&n.config),
                    escape(&n.text)
                );
            }
            // Editable once the client runs (#115): its runs, as a text's.
            NodeKind::RichInput => {
                let content = match self.sheet.borrow_mut().as_mut() {
                    Some(sheet) => runs_html_with(&n.runs, None, &mut |css| sheet.attr(css)),
                    None => runs_html_with(&n.runs, None, &mut |css| {
                        format!(r#" style="{}""#, escape(css))
                    }),
                };
                let empty = if n.text.is_empty() { " data-empty" } else { "" };
                let _ = write!(
                    out,
                    r#"<div {attrs} class="prism-rich" role="textbox" aria-multiline="true" data-placeholder="{}"{empty}>{content}</div>"#,
                    escape(&n.placeholder),
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
                let value = if c.mode == prism_core::DateMode::TimeOfDay && n.value.is_finite() {
                    let m = (n.value / 60.).round().rem_euclid(1440.) as u32;
                    format!(r#" value="{:02}:{:02}""#, m / 60, m % 60)
                } else {
                    String::new()
                };
                // Empty (#89): no value; only optional pickers may be emptied.
                let data = if n.value.is_finite() {
                    n.value.to_string()
                } else {
                    String::new()
                };
                let required = if c.optional { "" } else { " required" };
                let _ = write!(
                    out,
                    r#"<input type="{ty}" name="prism-{id}" class="prism-date" {attrs} data-value="{data}"{value}{required}>"#,
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
            // A video (#207): the browser's player and controls.
            NodeKind::Video => {
                let c = prism_core::VideoConfig::decode(&n.config);
                let url = |src: &str| match &self.asset_urls {
                    Some(urls) => urls(src),
                    None => picture_url(src),
                };
                let flag = |on: bool, name: &str| {
                    if on {
                        format!(" {name}")
                    } else {
                        String::new()
                    }
                };
                let _ = write!(
                    out,
                    r#"<video {attrs} class="prism-video" src="{}"{}{}{}{}{} playsinline preload="metadata"></video>"#,
                    escape(&media_url(&url(&n.text))),
                    flag(c.controls, "controls"),
                    flag(c.autoplay, "autoplay"),
                    flag(c.looping, "loop"),
                    flag(c.muted, "muted"),
                    c.poster
                        .as_deref()
                        .map(|p| format!(r#" poster="{}""#, escape(&url(p))))
                        .unwrap_or_default(),
                );
            }
            // The live tree replaces the first paint: the frame loads its
            // page then (`data-src`), once, rather than twice.
            NodeKind::WebView => {
                let c = prism_core::web_view::WebViewConfig::decode(&n.config);
                let sandbox = match c.sandbox {
                    Some(s) => format!(r#" sandbox="{}""#, s.attribute()),
                    None => String::new(),
                };
                let title = match &n.a11y.label {
                    Some(label) => format!(r#" title="{}""#, escape(label)),
                    None => String::new(),
                };
                // A page of an app scheme (#343): the path the app serves
                // it from, or none.
                let src = match crate::schemes::rewrite(&self.scheme_paths, &n.text) {
                    Some(path) => path,
                    None if is_app_scheme_url(&n.text) => {
                        crate::schemes::unserved(&n.text);
                        String::new()
                    }
                    None => n.text.clone(),
                };
                let _ = write!(
                    out,
                    r#"<iframe {attrs} class="prism-web-view" data-src="{}"{title}{sandbox}></iframe>"#,
                    escape(&src)
                );
            }
            // In the first paint like text (#285): an `<svg>` drawn at the
            // box's fixed size, the way `drawShape` (client.js) draws it;
            // the client draws it again whenever the box is resized.
            NodeKind::Shape => {
                // What a document defines (gradients, clips) is named after
                // its node: unique in the page.
                let prefix = format!("pv{id}");
                let svg = match self.sheet.borrow_mut().as_mut() {
                    Some(sheet) => {
                        shape_svg(&n.text, n.fixed_size, &prefix, &mut |css| sheet.attr(css))
                    }
                    None => shape_svg(&n.text, n.fixed_size, &prefix, &mut |css| {
                        format!(r#" style="{}""#, escape(css))
                    }),
                };
                let _ = write!(out, r#"<div {attrs} class="prism-shape">{svg}</div>"#);
            }
            // Drawn by the client with WebGL 2: it starts the scene from
            // `data-scene` (a full update) before the socket connects.
            NodeKind::Scene3d => {
                let full =
                    prism_core::scene3d::SceneUpdate::Full(n.scene.clone().unwrap_or_default());
                let scene = scene_urls(&full, &|src: &str| match &self.asset_urls {
                    Some(urls) => urls(src),
                    None => picture_url(src),
                })
                .to_json();
                let _ = write!(
                    out,
                    r#"<div {attrs} class="prism-scene3d" data-scene="{}"><canvas aria-hidden="true"></canvas></div>"#,
                    escape(&scene)
                );
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
            // A table (#284): its rows of column headers in `<thead>`.
            NodeKind::Table => {
                let header = |r: &NodeId| {
                    self.nodes
                        .get(r)
                        .and_then(|row| row.children.first())
                        .is_some_and(|c| self.kind(*c) == Some(NodeKind::ColumnHeader))
                };
                let (head, body): (Vec<NodeId>, Vec<NodeId>) =
                    n.children.iter().partition(|r| header(r));
                let _ = write!(out, r#"<table {attrs} class="prism-table"><thead>"#);
                for c in &head {
                    self.write(*c, false, out);
                }
                out.push_str("</thead><tbody>");
                for c in &body {
                    self.write(*c, false, out);
                }
                out.push_str("</tbody></table>");
            }
            NodeKind::TableRow => {
                let _ = write!(out, "<tr {attrs}>");
                for c in &n.children {
                    self.write(*c, false, out);
                }
                out.push_str("</tr>");
            }
            NodeKind::ColumnHeader | NodeKind::TableCell => {
                let tag = if kind == NodeKind::ColumnHeader {
                    r#"th scope="col""#
                } else {
                    "td"
                };
                let _ = write!(out, r#"<{tag} {attrs}><div class="prism-cell">"#);
                for c in &n.children {
                    self.write(*c, false, out);
                }
                let _ = write!(out, "</div></{}>", &tag[..2]);
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
                    // A link button (#230): an `<a>` with the button's look.
                    NodeKind::Button if link.is_some() => {
                        ("button", r#" class="prism-link-button""#)
                    }
                    NodeKind::Button => ("button", ""),
                    NodeKind::Screen => ("section", ""),
                    NodeKind::Scroll if n.hscroll => {
                        ("div", r#" class="prism-scroll prism-scroll-x""#)
                    }
                    NodeKind::Scroll => ("div", r#" class="prism-scroll""#),
                    _ => ("div", ""),
                };
                let class = match (kind, n.scrollbar) {
                    (NodeKind::Scroll, b) if b != prism_core::Scrollbar::Auto => {
                        format!(r#"{class} data-prism-scrollbar="{}""#, b.name())
                    }
                    _ => class.to_string(),
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
    /// carry `data-ps="pHASH"`; the rules match them inside `#prism-root`
    /// and `#prism-layers`. The layers are left out: see
    /// [`to_page`](HtmlBackend::to_page).
    pub fn to_html_and_css(&self) -> (String, String) {
        let page = self.to_page();
        (page.root, page.css)
    }

    /// The first paint of a page: the root's markup (for `#prism-root`),
    /// the layers' (`cx.add_layer`: a toaster, an app-wide dialog, for
    /// `#prism-layers`, bottom first) and the CSS rules that style both,
    /// without any `style` attribute.
    pub fn to_page(&self) -> PageHtml {
        *self.sheet.borrow_mut() = Some(Sheet::default());
        let root = self.to_html();
        let mut layers = String::new();
        for id in &self.layers {
            self.write(*id, false, &mut layers);
        }
        let sheet = self.sheet.borrow_mut().take().unwrap_or_default();
        PageHtml {
            root,
            layers,
            css: sheet.css,
        }
    }

    /// The layers' roots (`cx.add_layer`), bottom first.
    pub fn layers(&self) -> &[NodeId] {
        &self.layers
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

/// App-defined attributes (`.attr(…)`), as `client.js` sets them: names
/// checked again, values escaped.
fn app_attrs(attributes: &[(String, String)]) -> String {
    let mut out = String::new();
    for (name, value) in attributes {
        if prism_core::attributes::is_app_attribute(name) {
            let _ = write!(out, r#" {name}="{}""#, escape(value));
        }
    }
    out
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

/// A picture's `object_position` (#218) as CSS: `object-position:20% 0%;`.
pub(crate) fn object_position_css(config: &str) -> String {
    let (x, y) = prism_core::picture_position(config);
    format!("object-position:{}% {}%;", x * 100., y * 100.)
}

/// A shape's drawing (`prism_core::shape::Drawing::encode`) as the
/// `<svg>` `drawShape` (client.js) builds: in pixels when the box's size is
/// known (`size`), else scaled to the box (`preserveAspectRatio="none"`,
/// strokes that don't scale) if that draws it the same, i.e. without insets
/// or arcs (circles would turn into ellipses); else nothing until the
/// client measures the box. `style` gives each path its paint, as CSS
/// (` data-ps="…"` on a page, so no `style` attribute is needed).
///
/// An SVG document (gradients, text, images, groups, or a size of its own,
/// #342) is drawn by [`crate::drawing`], its definitions' ids starting
/// with `prefix`.
pub(crate) fn shape_svg(
    text: &str,
    size: Option<(f32, f32)>,
    prefix: &str,
    style: &mut dyn FnMut(&str) -> String,
) -> String {
    use prism_core::shape::{Drawing, Segment};
    let d = Drawing::decode(text);
    if crate::drawing::is_document(&d) {
        return crate::drawing::document_svg(&d, size, prefix, style);
    }
    match size {
        Some(size) => shape_svg_with(text, size, false, style),
        None => {
            let arcs = d
                .paths
                .iter()
                .flat_map(|p| &p.segments)
                .any(|s| matches!(s, Segment::Arc { .. }));
            if arcs || d.inset != (0., 0.) {
                return String::new();
            }
            let svg = shape_svg_with(text, (100., 100.), true, style);
            svg.replacen(
                r#"viewBox="0 0 100 100""#,
                r#"viewBox="0 0 100 100" preserveAspectRatio="none""#,
                1,
            )
        }
    }
}

/// [`shape_svg`] in a `w` × `h` box; with `non_scaling`, strokes keep
/// their width whatever size the `<svg>` is shown at, unless the drawing
/// has a size of its own: its strokes follow the box.
fn shape_svg_with(
    text: &str,
    (w, h): (f32, f32),
    non_scaling: bool,
    style: &mut dyn FnMut(&str) -> String,
) -> String {
    use prism_core::shape::{Drawing, Segment};
    let d = Drawing::decode(text);
    if d.paths.is_empty() {
        return String::new();
    }
    let (ix, iy) = d.inset;
    let (iw, ih) = ((w - 2. * ix).max(0.), (h - 2. * iy).max(0.));
    let side = iw.min(ih);
    // What strokes and dashes are multiplied by (`Drawing::resolve`).
    let own = d
        .size
        .filter(|(sw, sh)| *sw > 0. && *sh > 0.)
        .map(|(sw, sh)| (iw / sw).min(ih / sh));
    let scaled = non_scaling && own.is_none();
    let stroke = own.unwrap_or(1.);
    // Two decimals: the browser can't show more, and the page stays small.
    let num = |v: f32| {
        let v = (v * 100.).round() / 100.;
        if v == 0. {
            "0".to_string()
        } else {
            v.to_string()
        }
    };
    let x = |v: f32| num(ix + v * iw);
    let y = |v: f32| num(iy + v * ih);
    let mut o = format!(
        r#"<svg aria-hidden="true" viewBox="0 0 {} {}""#,
        num(w),
        num(h)
    );
    o.push('>');
    for p in &d.paths {
        let mut path = String::new();
        let mut open = false;
        for s in &p.segments {
            match *s {
                Segment::Move(a, b) | Segment::Line(a, b) => {
                    let c = if open && matches!(s, Segment::Line(..)) {
                        'L'
                    } else {
                        'M'
                    };
                    let _ = write!(path, "{c}{} {}", x(a), y(b));
                    open = true;
                }
                Segment::Cubic(a, b, c, e, f, g) => {
                    if !open {
                        let _ = write!(path, "M{} {}", x(a), y(b));
                    }
                    let _ = write!(
                        path,
                        "C{} {} {} {} {} {}",
                        x(a),
                        y(b),
                        x(c),
                        y(e),
                        x(f),
                        y(g)
                    );
                    open = true;
                }
                Segment::Arc {
                    cx,
                    cy,
                    r,
                    start,
                    end,
                } => {
                    // Degrees clockwise from 12 o'clock; the radius is a
                    // fraction of the smaller side.
                    let (cx, cy, r) = (ix + cx * iw, iy + cy * ih, r * side);
                    let at = |a: f32| {
                        let a = a.to_radians();
                        format!("{} {}", num(cx + r * a.sin()), num(cy - r * a.cos()))
                    };
                    let _ = write!(path, "{}{}", if open { 'L' } else { 'M' }, at(start));
                    open = true;
                    let sweep = (end - start).clamp(-360., 360.);
                    // One SVG arc can't draw a full circle: two halves at most.
                    let parts = if sweep.abs() > 180. { 2 } else { 1 };
                    for k in 1..=parts {
                        let _ = write!(
                            path,
                            "A{} {} 0 0 {} {}",
                            num(r),
                            num(r),
                            if sweep > 0. { 1 } else { 0 },
                            at(start + sweep / parts as f32 * k as f32)
                        );
                    }
                }
                Segment::Close => {
                    path.push('Z');
                    open = false;
                }
            }
        }
        let paint = |c: Option<prism_core::Color>| c.map_or("none".to_string(), |c| c.to_css());
        let mut css = format!("fill:{};stroke:{};", paint(p.fill), paint(p.stroke));
        if p.stroke.is_some() {
            let _ = write!(css, "stroke-width:{};", num(p.width * stroke));
            if scaled {
                css.push_str("vector-effect:non-scaling-stroke;");
            }
        }
        if p.round {
            css.push_str("stroke-linecap:round;stroke-linejoin:round;");
        }
        if let Some((dash, gap)) = p.dash {
            let _ = write!(
                css,
                "stroke-dasharray:{} {};",
                num(dash * stroke),
                num(gap * stroke)
            );
        }
        if p.even_odd {
            css.push_str("fill-rule:evenodd;");
        }
        let _ = write!(o, r#"<path d="{}"{}></path>"#, escape(&path), style(&css));
    }
    o.push_str("</svg>");
    o
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

/// A chip (#115) as the markup `client.js` (`fillChip`) builds: one
/// non-editable unit, its icon (a symbol, or an `<svg>` one em tall, #329)
/// then its label (styled runs), its box, tooltip, state, test id and
/// attributes. `style` turns CSS into attributes.
fn chip_html(out: &mut String, r: &prism_core::TextRun, style: &mut dyn FnMut(&str) -> String) {
    let state = r.chip_state.as_deref().map(|st| {
        st.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
            .collect::<String>()
    });
    let state = state.filter(|s| !s.is_empty());
    let mut attrs = String::new();
    if let Some(st) = &state {
        let _ = write!(attrs, r#" data-state="{st}""#);
    }
    if let Some(l) = r.link {
        let _ = write!(attrs, r#" data-link="{l}""#);
    }
    if let Some(t) = &r.chip_test_id {
        let _ = write!(attrs, r#" data-testid="{}""#, escape(t));
    }
    if let Some(t) = &r.tooltip {
        let _ = write!(attrs, r#" title="{}""#, escape(t));
    }
    for (name, value) in &r.chip_attrs {
        let own = matches!(
            name.as_str(),
            "data-link" | "data-selected" | "data-state" | "data-testid"
        );
        if name.starts_with("data-") && prism_core::attributes::is_app_attribute(name) && !own {
            let _ = write!(attrs, r#" {name}="{}""#, escape(value));
        }
    }
    let icon = match (&r.chip_drawing, &r.chip_icon) {
        (Some(d), _) => {
            // One em tall, `ratio` wide; strokes in points whatever its size.
            let ratio = r.chip_icon_ratio.filter(|v| *v > 0.).unwrap_or(1.);
            let d = prism_core::Drawing {
                // Insets in points at 16 to the em.
                inset: (d.inset.0 * 100. / 16., d.inset.1 * 100. / 16.),
                ..d.plain()
            };
            let svg = shape_svg_with(&d.encode(), (100. * ratio, 100.), true, style);
            let size = style(&format!("width:{ratio}em"));
            svg.replacen(
                "<svg ",
                &format!(r#"<svg class="prism-chip-icon"{size} "#),
                1,
            )
        }
        (None, Some(g)) => format!(
            r#"<span class="prism-symbol" aria-hidden="true">{}</span>"#,
            escape(prism_core::glyphs::glyph(g))
        ),
        (None, None) => String::new(),
    };
    let mut label = String::new();
    if r.chip_label_runs.is_empty() {
        label = escape(r.chip_label.as_deref().unwrap_or(""));
    } else {
        for l in &r.chip_label_runs {
            let css = crate::css::run_css(&prism_core::TextRun {
                text: String::new(),
                weight: l.weight,
                italic: l.italic,
                color: l.color,
                ..Default::default()
            });
            if css.is_empty() {
                label.push_str(&escape(&l.text));
            } else {
                let _ = write!(label, "<span{}>{}</span>", style(&css), escape(&l.text));
            }
        }
    }
    let cut = if r.chip_max_width.is_some() {
        style("min-width:0;overflow:clip;text-overflow:ellipsis")
    } else {
        String::new()
    };
    let css = crate::css::chip_css(r);
    let box_style = if css.is_empty() {
        String::new()
    } else {
        style(&css)
    };
    let class = state.map_or(String::new(), |st| format!(" prism-chip-{st}"));
    let _ = write!(
        out,
        r#"<span class="prism-chip{class}" contenteditable="false"{attrs}{box_style}>{icon}<span class="prism-chip-label"{cut}>{label}</span></span>"#
    );
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
    let style_fn = style;
    for r in prism_core::decode_runs(config) {
        let css = crate::css::run_css(&r);
        let style = if css.is_empty() || r.chip.is_some() {
            String::new()
        } else {
            style_fn(&css)
        };
        let text = escape(&r.text);
        let title = r
            .tooltip
            .as_deref()
            .map_or(String::new(), |t| format!(r#" title="{}""#, escape(t)));
        if r.chip.is_some() {
            chip_html(&mut out, &r, style_fn);
        } else if let Some(name) = r.icon.as_deref() {
            // In a rich input an icon is one unit (#221): U+FFFC, drawn as
            // its glyph and not editable. Read-only, its text is the glyph.
            let object = r.text == prism_core::OBJECT_CHAR.to_string();
            let (editable, glyph) = if object {
                (
                    r#" contenteditable="false""#,
                    escape(prism_core::glyphs::glyph(name)),
                )
            } else {
                ("", text)
            };
            let _ = write!(
                out,
                r#"<span class="prism-symbol" aria-hidden="true"{editable}{title}{style}>{glyph}</span>"#
            );
        } else if let Some(link) = r.link {
            let href = base
                .zip(r.href.as_deref())
                .and_then(|(b, h)| link_href(b, h));
            match (href, base) {
                (Some(href), _) => {
                    let _ = write!(
                        out,
                        r#"<a class="prism-link" href="{}" data-link="{link}"{title}{style}>{text}</a>"#,
                        escape(&href)
                    );
                }
                (None, None) => {
                    let _ = write!(
                        out,
                        r#"<span class="prism-link" data-link="{link}"{title}{style}>{text}</span>"#
                    );
                }
                (None, Some(_)) => {
                    let _ = write!(
                        out,
                        r#"<span class="prism-link" role="link" tabindex="0" data-link="{link}"{title}{style}>{text}</span>"#
                    );
                }
            }
        } else if style.is_empty() && title.is_empty() {
            out.push_str(&text);
        } else {
            let _ = write!(out, "<span{title}{style}>{text}</span>");
        }
    }
    out
}

/// A scene update with its textures' sources turned into URLs by `url`,
/// like pictures' (the browser cannot resolve app asset names). Embedded
/// images (`p3d:<blob id>`) stay as they are: the client fetches them from
/// `<base>/_prism/3d/<id>`.
pub(crate) fn scene_urls<'a>(
    update: &'a prism_core::scene3d::SceneUpdate,
    url: &dyn Fn(&str) -> String,
) -> std::borrow::Cow<'a, prism_core::scene3d::SceneUpdate> {
    use prism_core::scene3d::Material;
    fn sources(m: &mut Material) -> [&mut Option<String>; 5] {
        [
            &mut m.texture,
            &mut m.mr_map,
            &mut m.normal_map,
            &mut m.emissive_map,
            &mut m.ao_map,
        ]
    }
    let resolvable = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.starts_with("p3d:"));
    // Most scenes have no texture: they go as they are.
    let any = update.nodes().iter().any(|n| {
        let m = &n.material;
        [
            &m.texture,
            &m.mr_map,
            &m.normal_map,
            &m.emissive_map,
            &m.ao_map,
        ]
        .into_iter()
        .any(resolvable)
    });
    if !any {
        return std::borrow::Cow::Borrowed(update);
    }
    let mut update = update.clone();
    for n in update.nodes_mut() {
        for src in sources(&mut n.material) {
            if resolvable(src)
                && let Some(s) = src.as_mut()
            {
                *s = url(s);
            }
        }
    }
    std::borrow::Cow::Owned(update)
}

/// Schemes a link or `open_url` never gets: they run script in the page
/// (`javascript:`, `vbscript:`) or show content the app did not serve
/// (`data:`, `blob:`, `filesystem:`). Other schemes (`mailto:`, `tel:`,
/// another app's own) are the browser's to hand over.
const BLOCKED_SCHEMES: [&str; 5] = ["javascript", "vbscript", "data", "blob", "filesystem"];

/// `url` as a browser reads it (leading and trailing controls and spaces
/// trimmed, tabs and newlines dropped), or `None` if its scheme is a
/// [blocked](BLOCKED_SCHEMES) one. Mirrors `safeUrl` in client.js.
pub(crate) fn safe_url(url: &str) -> Option<String> {
    let url: String = url
        .trim_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let blocked = has_scheme(&url)
        && url.split_once(':').is_some_and(|(scheme, _)| {
            BLOCKED_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
        });
    (!blocked).then_some(url)
}

/// A link's `href` (mirrors `hrefFor` in client.js): a URL (`https:`,
/// `mailto:`…) as is, an app path under the mount point `base`; `None` for
/// a URL that would run script ([`safe_url`]).
pub fn link_href(base: &str, path: &str) -> Option<String> {
    let path = safe_url(path)?;
    if has_scheme(&path) {
        return Some(path);
    }
    let (p, query) = path.split_at(path.find('?').unwrap_or(path.len()));
    Some(if p == "/" && !base.is_empty() {
        format!("{base}{query}")
    } else {
        format!("{base}{p}{query}")
    })
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

/// ` href="…"` of a node made a link, with `rel="noopener"` for a URL;
/// nothing for a URL that would run script.
fn link_attrs(base: &str, path: &str) -> String {
    let Some(href) = link_href(base, path) else {
        return String::new();
    };
    let rel = if has_scheme(&href) {
        r#" rel="noopener""#
    } else {
        ""
    };
    format!(r#" href="{}"{rel}"#, escape(&href))
}

/// Remote URLs as-is; anything else is an app asset under `/assets/`.
/// Headers (`Picture::header`) are dropped: only a [`WebApp`](crate::WebApp)
/// can serve such a picture (its server fetches it with them).
/// A video's URL (#207): a proxied source (headers) is served by the media
/// route, which answers ranges.
pub fn media_url(url: &str) -> String {
    url.replacen("/_prism/picture/", "/_prism/media/", 1)
}

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

/// Whether `url` is of a scheme the app serves (#343).
fn is_app_scheme_url(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        prism_core::web_view::is_web_scheme(&scheme.trim().to_ascii_lowercase())
    })
}
