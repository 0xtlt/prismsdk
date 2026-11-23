//! Prism's accessibility on the XAML elements: name, help text, heading
//! level, live region, tooltip on every node's element; for the elements
//! Prism draws itself (clickable views, containers) their role, states
//! and patterns through custom automation peers (`peers.rs`, mapped in
//! `access.rs`); live regions' new values announced; sheets and dialogs
//! as dialog windows with the app's content out of reach meanwhile; under
//! a contrast theme, each node's colors (`contrast.rs`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use prism_core::{Accessibility, IdMap, NodeId, NodeKind, Role};
use windows_core::{Interface, Result};

use crate::access::{self, Landmark};
use crate::contrast::{self, Scheme};
use crate::fluent::{Accent, Palette};
use crate::peers::{self, Key};
use crate::scene::Scene;
use crate::view::View;
use crate::widgets::Win;
use crate::winui::h;
use crate::winui::xaml::Automation::AutomationProperties;
use crate::winui::xaml::Automation::Peers::{
    AccessibilityView, AutomationHeadingLevel, AutomationLandmarkType, AutomationLiveSetting,
};
use crate::winui::xaml::Controls::Canvas;
use crate::winui::xaml::UIElement;

thread_local! {
    /// The app's content: live regions speak from its peer.
    static CONTENT: RefCell<Option<UIElement>> = const { RefCell::new(None) };
    /// Each live region's last value.
    static LIVE: RefCell<HashMap<NodeId, String>> = RefCell::default();
    /// The sheets and dialogs shown as panels.
    static DIALOGS: RefCell<HashSet<NodeId>> = RefCell::default();
    /// Each disclosure's region (`accessibility_controls`): its element's
    /// `ControlledPeers`.
    static CONTROLS: RefCell<HashMap<NodeId, NodeId>> = RefCell::default();
}

/// The app's content canvas (`peers::canvas(Key::Content)`).
pub fn set_content(e: &impl Interface) {
    CONTENT.with(|c| *c.borrow_mut() = e.cast().ok());
}

/// Prism's accessibility on node `id`'s element: its clickable button,
/// else its control or text, else its canvas. `text` is the node's own
/// (a clickable text's button is named by it).
pub fn apply(v: &View, id: NodeId, a: &Accessibility, text: &str) -> Result<()> {
    let target = element(v)?;
    let own = (v.button.is_some() && v.content.element().is_some()).then_some(text);
    let name = a.label.as_deref().or(own).unwrap_or("");
    AutomationProperties::SetName(&target, &h(name))?;
    AutomationProperties::SetHelpText(&target, &h(a.hint.as_deref().unwrap_or("")))?;
    if let crate::controls::Content::Date(d) = &v.content {
        d.name_time(name, a.hint.as_deref().unwrap_or(""))?;
    }
    AutomationProperties::SetHeadingLevel(
        &target,
        match a.role {
            Some(Role::Heading(l)) => AutomationHeadingLevel(l.clamp(1, 9) as i32),
            _ => AutomationHeadingLevel::None,
        },
    )?;
    AutomationProperties::SetLiveSetting(
        &target,
        match a.role {
            Some(Role::Alert) => AutomationLiveSetting::Assertive,
            Some(Role::Status) => AutomationLiveSetting::Polite,
            _ => AutomationLiveSetting::Off,
        },
    )?;
    AutomationProperties::SetAccessibilityView(
        &target,
        if a.hidden {
            AccessibilityView::Raw
        } else {
            AccessibilityView::Content
        },
    )?;
    crate::tips::apply(v, id, &target, a.tooltip.as_ref(), text)?;
    // Drawn by Prism (a clickable view, a container): the role and states
    // through its custom peer. WinUI's controls describe themselves.
    // A stack's or a tab bar's container is no element (its header and
    // its bar are WinUI's), as on macOS.
    let bare = matches!(v.kind, NodeKind::Stack | NodeKind::Tabs) && v.button.is_none();
    if !bare && (v.button.is_some() || v.content.element().is_none()) {
        let mut s = access::semantics(a, v.button.is_some());
        // A view listening to keys whose canvas takes the focus.
        if v.button.is_none() && crate::keyview::is(id) {
            s = access::key_view(s);
        }
        AutomationProperties::SetLandmarkType(
            &target,
            match s.landmark {
                Landmark::None => AutomationLandmarkType::None,
                Landmark::Main => AutomationLandmarkType::Main,
                Landmark::Navigation => AutomationLandmarkType::Navigation,
            },
        )?;
        // A clickable view's button stands for it, not its canvas.
        let (key, other) = if v.button.is_some() {
            (Key::Clickable(id), Key::Node(id))
        } else {
            (Key::Node(id), Key::Clickable(id))
        };
        peers::forget(other);
        let old = peers::set(key, s.clone());
        if let Err(e) = peers::changed(&target, old.as_ref(), &s) {
            crate::win::log!("automation events of node {id}: {e}");
        }
        // A table's and its cells' elements, which the table patterns
        // hand to a reader (#284, `table.rs`).
        if matches!(a.role, Some(Role::Table | Role::Cell | Role::ColumnHeader)) {
            crate::table::set_element(id, &target);
        }
    }
    live(id, a);
    Ok(())
}

/// A live region's new value is announced (status: after what the reader
/// is saying; alert: in its place).
fn live(id: NodeId, a: &Accessibility) {
    let important = match a.role {
        Some(Role::Status) => false,
        Some(Role::Alert) => true,
        _ => {
            LIVE.with(|l| l.borrow_mut().remove(&id));
            return;
        }
    };
    let text = a
        .value
        .clone()
        .or_else(|| important.then(|| a.label.clone()).flatten())
        .unwrap_or_default();
    let changed = LIVE.with(|l| l.borrow_mut().insert(id, text.clone()).as_ref() != Some(&text));
    if !changed || text.is_empty() {
        return;
    }
    let content = CONTENT.with(|c| c.borrow().clone());
    if let Some(e) = content
        && let Err(err) = peers::announce(&e, &text, important)
    {
        crate::win::log!("announcing {text:?}: {err}");
    }
}

/// The element that speaks for a view: its clickable button, else its
/// control or text, else its canvas.
fn element(v: &View) -> Result<UIElement> {
    Ok(match (&v.button, v.content.named()) {
        (Some(b), _) => b.cast()?,
        (None, Some(e)) => e,
        (None, None) => v.outer(),
    })
}

/// Node `id` controls `target`'s region, or none.
pub fn set_controls(id: NodeId, target: Option<NodeId>) {
    CONTROLS.with(|c| match target {
        Some(t) => c.borrow_mut().insert(id, t),
        None => c.borrow_mut().remove(&id),
    });
}

/// The `ControlledPeers` of node `id` (a disclosure) and of the
/// disclosures controlling it, once their elements are made.
pub fn link_controls(views: &IdMap<NodeId, View>, id: NodeId) {
    let pairs: Vec<(NodeId, Option<NodeId>)> = CONTROLS.with(|c| {
        let c = c.borrow();
        let own = (id, c.get(&id).copied());
        std::iter::once(own)
            .chain(
                c.iter()
                    .filter(|(_, t)| **t == id)
                    .map(|(k, t)| (*k, Some(*t))),
            )
            .collect()
    });
    for (from, to) in pairs {
        let Some(v) = views.get(&from) else { continue };
        let r = (|| {
            let peers = AutomationProperties::GetControlledPeers(&element(v)?)?;
            peers.Clear()?;
            if let Some(t) = to.and_then(|t| views.get(&t)) {
                peers.Append(&element(t)?)?;
            }
            Ok::<_, windows_core::Error>(())
        })();
        if let Err(e) = r {
            crate::win::log!("controlled region of node {from}: {e}");
        }
    }
}

/// Node `id` is gone.
pub fn forget(id: NodeId) {
    CONTROLS.with(|c| c.borrow_mut().remove(&id));
    peers::forget(Key::Node(id));
    peers::forget(Key::Clickable(id));
    LIVE.with(|l| l.borrow_mut().remove(&id));
    crate::tips::forget(id);
    crate::table::forget(id);
    dialog_hidden(id);
}

/// A sheet or dialog shows as a panel (`card`): a dialog named `name`
/// for UI Automation, the only part of the window a reader reaches.
pub fn dialog_shown(id: NodeId, card: &Canvas, name: Option<&str>) {
    let _ = AutomationProperties::SetName(card, &h(name.unwrap_or("")));
    peers::set(Key::Dialog(id), access::dialog());
    DIALOGS.with(|d| d.borrow_mut().insert(id));
    peers::set_modal(true);
}

/// The panel of sheet or dialog `id` closed.
pub fn dialog_hidden(id: NodeId) {
    let was = DIALOGS.with(|d| d.borrow_mut().remove(&id));
    if was {
        peers::forget(Key::Dialog(id));
        peers::set_modal(DIALOGS.with(|d| !d.borrow().is_empty()));
    }
}

/// What a sheet or dialog is called: its label, else its first heading,
/// else its first text.
pub fn dialog_name(scene: &Scene, id: NodeId) -> Option<String> {
    let label = scene.get(id).and_then(|n| n.a11y.label.clone());
    let mut texts: Vec<(String, u8)> = Vec::new();
    let mut stack = vec![id];
    while let Some(n) = stack.pop().and_then(|i| scene.get(i)) {
        if n.kind == NodeKind::Text && !n.a11y.hidden {
            let level = match n.a11y.role {
                Some(Role::Heading(l)) => l,
                _ => 0,
            };
            texts.push((n.text.clone(), level));
        }
        // Depth first, in reading order.
        stack.extend(n.children.iter().rev().copied());
        if texts.len() > 32 {
            break;
        }
    }
    let texts: Vec<(&str, u8)> = texts.iter().map(|(t, l)| (t.as_str(), *l)).collect();
    access::dialog_name(label.as_deref(), &texts).map(str::to_owned)
}

/// The window's palette: Fluent's, or a contrast theme's while one is on.
pub fn palette(dark: bool, accent: Accent) -> Palette {
    match crate::system::high_contrast() {
        Some(sys) => Palette::high_contrast(sys),
        None => Palette::new(dark, accent),
    }
}

impl Win {
    /// The palette node `id` is drawn with: the window's; under a contrast
    /// theme, with where the node stands (clickable, in a link, disabled,
    /// selected, on a highlight).
    pub fn palette_for(&self, id: NodeId) -> Palette {
        let mut pal = self.palette;
        let (Some(f), Some(n)) = (pal.forced.as_mut(), self.scene.get(id)) else {
            return pal;
        };
        let mut s = Scheme {
            button: self.is_button(id),
            selected: n.a11y.is_selected(),
            ..Default::default()
        };
        // The fill behind its text: its own, else the nearest ancestor's.
        let mut behind = false;
        let mut at = Some(id);
        while let Some(m) = at.and_then(|i| self.scene.get(i)) {
            s.disabled |= m.a11y.disabled;
            s.link |= m.a11y.role == Some(Role::Link);
            s.in_button |= at.is_some_and(|i| self.is_button(i));
            let fill = m.style.background.or_else(|| {
                m.style
                    .gradient
                    .as_ref()
                    .and_then(|g| g.stops.first().map(|s| s.color))
            });
            if !behind && let Some(c) = fill.filter(|c| c.a >= 0.02) {
                behind = true;
                let c = self.palette.resolve(c);
                s.behind = if m.a11y.is_selected() && !s.disabled {
                    contrast::Fill::Highlight
                } else {
                    contrast::classify(c, f.reference)
                };
            }
            at = m.parent;
        }
        f.scheme = s;
        pal
    }

    /// Whether node `id` is a button: a clickable view (drawn by Prism in a
    /// button of its own) or a native one; not a clickable scroll view.
    fn is_button(&self, id: NodeId) -> bool {
        self.scene
            .get(id)
            .is_some_and(|n| n.kind == NodeKind::Button)
            || self.views.get(&id).is_some_and(|v| v.button.is_some())
    }

    /// Restyles node `id`; under a contrast theme its descendants too
    /// (their text takes its fill and states into account).
    pub fn restyle_tree(&mut self, id: NodeId) {
        self.restyle(id);
        if self.palette.forced.is_none() {
            return;
        }
        let mut stack: Vec<NodeId> = self
            .scene
            .get(id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        while let Some(c) = stack.pop() {
            self.restyle(c);
            if let Some(n) = self.scene.get(c) {
                stack.extend(n.children.iter().copied());
            }
        }
    }
}

/// An item of the tab bar (WinUI's `SelectorBar`, whose items say
/// « SelectorBarItem »): a tab item, in WinUI's words for one.
pub fn tab_item(item: &impl Interface) -> Result<()> {
    use crate::winui::xaml::Automation::Peers::AutomationControlType;
    let e: UIElement = item.cast()?;
    AutomationProperties::SetAutomationControlType(&e, AutomationControlType::TabItem)?;
    if let Some(words) = peers::tab_words() {
        AutomationProperties::SetLocalizedControlType(&e, &words)?;
    }
    Ok(())
}
