//! Interaction states: style refinements while a node is hovered, pressed,
//! focused, contains the focus, or is disabled (`.hover(|s| …)`,
//! `.active(|s| …)`…), resolved natively by each backend (CSS
//! pseudo-classes, AppKit tracking areas, UIKit gestures and the focus
//! engine, GTK state flags, Android touch, hover and focus listeners).

use std::rc::Rc;

use crate::style::{Style, StyleContext};

/// One interaction state a style can refine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Interaction {
    /// The pointer is over the node (or one of its descendants). Devices
    /// without a pointer never hover; on tvOS the focused node hovers.
    Hover,
    /// The node or one of its descendants has the keyboard focus.
    FocusWithin,
    /// The node has the keyboard focus (a text field being edited, a control
    /// reached with Tab or the TV remote).
    Focus,
    /// The node is pressed (pointer or finger down on it).
    Active,
    /// The node is disabled (`accessibility_disabled(true)`, a disabled
    /// control).
    Disabled,
    /// An ancestor marked [`group`](crate::Styled::group) is hovered.
    GroupHover,
    /// An ancestor marked [`group`](crate::Styled::group) contains the
    /// keyboard focus (show a field's clear button while it is edited).
    GroupFocusWithin,
    /// An ancestor marked [`group`](crate::Styled::group) is pressed.
    GroupActive,
    /// Files are dragged over the node, a drop target that accepts them
    /// ([`Div::on_drop_files`](crate::Div::on_drop_files), #111).
    DropHover,
}

impl Interaction {
    /// Every state, in the order refinements apply (later ones win): a
    /// group's states first, then the node's own.
    pub const ALL: [Interaction; 9] = [
        Interaction::GroupHover,
        Interaction::GroupFocusWithin,
        Interaction::GroupActive,
        Interaction::Hover,
        Interaction::FocusWithin,
        Interaction::Focus,
        Interaction::Active,
        Interaction::DropHover,
        Interaction::Disabled,
    ];

    fn bit(self) -> u16 {
        match self {
            Interaction::Hover => 1,
            Interaction::FocusWithin => 2,
            Interaction::Focus => 4,
            Interaction::Active => 8,
            Interaction::Disabled => 16,
            Interaction::GroupHover => 32,
            Interaction::GroupFocusWithin => 64,
            Interaction::GroupActive => 128,
            Interaction::DropHover => 256,
        }
    }

    /// The state a group's descendants see when the group is in `self`
    /// (`Hover` → `GroupHover`…), if any.
    pub fn for_group(self) -> Option<Interaction> {
        match self {
            Interaction::Hover => Some(Interaction::GroupHover),
            Interaction::FocusWithin => Some(Interaction::GroupFocusWithin),
            Interaction::Active => Some(Interaction::GroupActive),
            _ => None,
        }
    }

    /// Short name (`hover`, `focus_within`, `focus`, `active`, `disabled`,
    /// `group_hover`…), as remote hosts report it.
    pub fn name(self) -> &'static str {
        match self {
            Interaction::Hover => "hover",
            Interaction::FocusWithin => "focus_within",
            Interaction::Focus => "focus",
            Interaction::Active => "active",
            Interaction::Disabled => "disabled",
            Interaction::GroupHover => "group_hover",
            Interaction::GroupFocusWithin => "group_focus_within",
            Interaction::GroupActive => "group_active",
            Interaction::DropHover => "drop_hover",
        }
    }

    /// The state named `name` (see [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<Interaction> {
        Interaction::ALL.into_iter().find(|i| i.name() == name)
    }
}

/// A set of [`Interaction`]s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Interactions(u16);

impl Interactions {
    /// No state.
    pub const NONE: Interactions = Interactions(0);

    /// Whether `state` is in the set.
    pub fn contains(self, state: Interaction) -> bool {
        self.0 & state.bit() != 0
    }

    /// The set with `state` added (`on`) or removed.
    pub fn with(self, state: Interaction, on: bool) -> Interactions {
        if on {
            Interactions(self.0 | state.bit())
        } else {
            Interactions(self.0 & !state.bit())
        }
    }

    /// Whether the set is empty.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The states in the set, in the order they apply.
    pub fn iter(self) -> impl Iterator<Item = Interaction> {
        Interaction::ALL
            .into_iter()
            .filter(move |i| self.contains(*i))
    }
}

impl From<Interaction> for Interactions {
    fn from(i: Interaction) -> Self {
        Interactions(i.bit())
    }
}

impl std::ops::BitOr for Interactions {
    type Output = Interactions;
    fn bitor(self, rhs: Interactions) -> Interactions {
        Interactions(self.0 | rhs.0)
    }
}

type Rule = Rc<dyn Fn(Style) -> Style>;

/// The interaction refinements of a style (`.hover(|s| …)`…).
///
/// Once the style is resolved (see [`Style::resolve`]), each state also has
/// its own resolved style, the base style with only that refinement applied
/// ([`resolved`](Self::resolved)): CSS-based backends turn the differences
/// into pseudo-class rules. Several states at once apply in the order of
/// [`Interaction::ALL`] ([`Style::interacted`]).
#[derive(Clone, Default)]
pub struct States {
    rules: Vec<(Interaction, Rule)>,
    resolved: Vec<(Interaction, Style)>,
    cx: Option<StyleContext>,
}

impl States {
    /// Whether no refinement is set.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The states that have a refinement.
    pub fn used(&self) -> Interactions {
        self.rules
            .iter()
            .fold(Interactions::NONE, |set, (i, _)| set.with(*i, true))
    }

    /// Each refined state with the resolved style it gives on its own, in
    /// the order they apply (empty until the style is resolved).
    pub fn resolved(&self) -> impl Iterator<Item = (Interaction, &Style)> {
        Interaction::ALL.into_iter().flat_map(move |i| {
            self.resolved
                .iter()
                .filter(move |(j, _)| *j == i)
                .map(|(j, s)| (*j, s))
        })
    }

    /// The same refinements with every resolved style changed by `f` (the
    /// transformations native backends apply to the base style: zoom,
    /// right-to-left mirroring, resolved colors).
    pub fn map_resolved(&self, f: impl Fn(&Style) -> Style) -> States {
        States {
            rules: self.rules.clone(),
            resolved: self.resolved.iter().map(|(i, s)| (*i, f(s))).collect(),
            cx: self.cx,
        }
    }

    pub(crate) fn push(&mut self, state: Interaction, rule: Rule) {
        self.rules.push((state, rule));
        self.resolved.clear();
        self.cx = None;
    }
}

impl std::fmt::Debug for States {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.rules.iter().map(|(i, _)| i))
            .finish()
    }
}

/// Rules are closures: unresolved styles with refinements are never equal;
/// resolved ones compare by what each state gives.
impl PartialEq for States {
    fn eq(&self, other: &Self) -> bool {
        if self.rules.is_empty() && other.rules.is_empty() {
            return true;
        }
        self.cx.is_some()
            && other.cx.is_some()
            && self.rules.len() == other.rules.len()
            && self.resolved == other.resolved
    }
}

impl Style {
    /// The base style with no interaction refinement (what a state rule
    /// starts from).
    fn without_states(&self) -> Style {
        let mut s = self.clone();
        s.states = States::default();
        s
    }

    /// Resolves the interaction refinements against this (already resolved)
    /// style: each state's own style is computed once, so that styles can be
    /// compared and CSS-based backends can read them.
    pub(crate) fn resolve_states(&mut self, cx: StyleContext) {
        if self.states.rules.is_empty() {
            return;
        }
        let base = self.without_states();
        let resolved = self
            .states
            .rules
            .iter()
            .map(|(i, rule)| (*i, rule(base.clone()).resolve(cx).without_states()))
            .collect();
        self.states.resolved = resolved;
        self.states.cx = Some(cx);
    }

    /// This style while in `states` (hovered and pressed…): the refinements
    /// of those states applied in the order of [`Interaction::ALL`]. The
    /// result keeps the refinements (backends still need them).
    pub fn interacted(&self, states: Interactions) -> Style {
        let active: Vec<&Rule> = Interaction::ALL
            .into_iter()
            .filter(|i| states.contains(*i))
            .flat_map(|i| {
                self.states
                    .rules
                    .iter()
                    .filter(move |(j, _)| *j == i)
                    .map(|(_, r)| r)
            })
            .collect();
        if active.is_empty() {
            return self.clone();
        }
        let mut s = self.without_states();
        for rule in active {
            s = rule(s);
        }
        if let Some(cx) = self.states.cx {
            s = s.resolve(cx);
        }
        s.states = self.states.clone();
        s
    }

    /// This style for a disabled node: its `disabled` refinement applied,
    /// and no other (a disabled node does not react to hover, press or
    /// focus).
    pub fn disabled_applied(&self) -> Style {
        let mut s = self.interacted(Interaction::Disabled.into());
        s.states = States::default();
        s
    }
}
