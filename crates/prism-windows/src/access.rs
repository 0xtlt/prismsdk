//! What UI Automation hears of the elements Prism draws itself: a
//! clickable view (a `Button` without chrome) and a container (a
//! `Canvas`). Their control type, landmark, states and patterns come from
//! Prism's [`Accessibility`], mapped as WinUI maps its own controls and as
//! browsers map the same ARIA roles to UI Automation (Core Accessibility
//! API Mappings). WinUI's real controls (text boxes, sliders, toggle
//! switches…) keep their own peers. No Win32 types: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::{Accessibility, Role};

/// The UI Automation control types Prism's roles become
/// (`AutomationControlType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Button,
    CheckBox,
    ComboBox,
    Group,
    Hyperlink,
    Image,
    List,
    ListItem,
    Pane,
    ProgressBar,
    RadioButton,
    Slider,
    /// A tab bar (UI Automation's `Tab` is the bar, `TabItem` one tab).
    Tab,
    TabItem,
    Text,
    Window,
}

/// A landmark screen readers jump between (`AutomationLandmarkType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Landmark {
    #[default]
    None,
    Main,
    Navigation,
}

/// A live region (`AutomationLiveSetting`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Live {
    #[default]
    Off,
    Polite,
    Assertive,
}

/// What one element tells UI Automation beyond its name and help text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Semantics {
    /// Its control type; `None` leaves WinUI's (a button stays a button,
    /// a named container a group).
    pub control: Option<Control>,
    /// Read as a toggle switch, in WinUI's own (localized) words.
    pub switch: bool,
    pub landmark: Landmark,
    /// Heading level, 1–6; 0 for none.
    pub heading: u8,
    pub live: Live,
    /// A dialog (`IsDialog`): the reader says so and stays inside.
    pub dialog: bool,
    /// The Toggle pattern: on or off (check boxes, switches, pressed
    /// buttons).
    pub toggled: Option<bool>,
    /// The SelectionItem pattern: selected or not (tabs, radios, rows,
    /// the current page of a navigation).
    pub selected: Option<bool>,
    /// The Value pattern, read-only: read after the name ("40 %").
    pub value: Option<String>,
    /// Activated through the Invoke pattern. Check boxes, switches, tabs
    /// and radios are toggled or selected instead, like WinUI's.
    pub invoke: bool,
    /// Shown as unavailable.
    pub disabled: bool,
    /// An element of its own even without a name (a container: WinUI
    /// leaves a nameless `Canvas` out of the tree).
    pub element: bool,
    /// Out of the tree with everything inside (decorations, duplicates).
    pub hidden: bool,
}

/// The semantics of a clickable view (`clickable`) or a container.
pub fn semantics(a: &Accessibility, clickable: bool) -> Semantics {
    let role = a.role;
    let control = match role {
        Some(Role::Button | Role::Switch) => Some(Control::Button),
        Some(Role::Link) => Some(Control::Hyperlink),
        Some(Role::Heading(_) | Role::Text) => Some(Control::Text),
        Some(Role::Image) => Some(Control::Image),
        Some(Role::Group | Role::RadioGroup | Role::Navigation | Role::Main | Role::Alert) => {
            Some(Control::Group)
        }
        Some(Role::List) => Some(Control::List),
        Some(Role::ListItem) => Some(Control::ListItem),
        Some(Role::Checkbox) => Some(Control::CheckBox),
        Some(Role::Tab) => Some(Control::TabItem),
        Some(Role::TabList) => Some(Control::Tab),
        Some(Role::TabPanel) => Some(Control::Pane),
        Some(Role::ProgressBar) => Some(Control::ProgressBar),
        Some(Role::Slider) => Some(Control::Slider),
        Some(Role::ComboBox) => Some(Control::ComboBox),
        Some(Role::Radio) => Some(Control::RadioButton),
        Some(Role::Dialog) => Some(Control::Window),
        // A live region is no element of its own: its children stay
        // reachable, its value is announced.
        Some(Role::Status) => None,
        None if clickable => Some(Control::Button),
        None => None,
    };
    let toggled = match role {
        Some(Role::Switch | Role::Checkbox) => Some(a.checked.unwrap_or(false)),
        // A pressed button (`aria-pressed`): a toggle button, as WinUI's.
        Some(Role::Button) => a.checked,
        _ => None,
    };
    let selected = match role {
        Some(Role::Tab | Role::Radio) => Some(a.is_selected()),
        _ if toggled.is_none() && (a.selected.is_some() || a.current) => Some(a.is_selected()),
        _ => None,
    };
    let value = a
        .value
        .clone()
        .filter(|v| !v.is_empty() && !matches!(role, Some(Role::Status | Role::Alert)));
    Semantics {
        control,
        switch: role == Some(Role::Switch),
        landmark: match role {
            Some(Role::Main) => Landmark::Main,
            Some(Role::Navigation) => Landmark::Navigation,
            _ => Landmark::None,
        },
        heading: match role {
            Some(Role::Heading(l)) => l.clamp(1, 6),
            _ => 0,
        },
        live: match role {
            Some(Role::Alert) => Live::Assertive,
            Some(Role::Status) => Live::Polite,
            _ => Live::Off,
        },
        dialog: role == Some(Role::Dialog),
        invoke: clickable
            && toggled.is_none()
            && !matches!(
                role,
                Some(Role::Tab | Role::Radio | Role::Checkbox | Role::Switch)
            ),
        toggled,
        selected,
        value,
        disabled: a.disabled,
        element: control.is_some() && !a.hidden,
        hidden: a.hidden,
    }
}

/// What a sheet or dialog shown as a panel tells UI Automation: a dialog
/// window named by its title.
pub fn dialog() -> Semantics {
    Semantics {
        control: Some(Control::Window),
        dialog: true,
        element: true,
        ..Default::default()
    }
}

/// The name of a sheet or dialog: its own label, else its first heading,
/// else its first text (`texts` in reading order, with their heading
/// level, 0 for plain text).
pub fn dialog_name<'a>(label: Option<&'a str>, texts: &[(&'a str, u8)]) -> Option<&'a str> {
    label.filter(|l| !l.is_empty()).or_else(|| {
        texts
            .iter()
            .filter(|(t, _)| !t.trim().is_empty())
            .min_by_key(|(_, level)| if *level == 0 { 7 } else { *level })
            .map(|(t, _)| *t)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a11y(role: Role) -> Accessibility {
        Accessibility {
            role: Some(role),
            ..Default::default()
        }
    }

    #[test]
    fn roles_become_their_control_types() {
        let cases = [
            (Role::Button, Control::Button),
            (Role::Link, Control::Hyperlink),
            (Role::Heading(2), Control::Text),
            (Role::Image, Control::Image),
            (Role::List, Control::List),
            (Role::ListItem, Control::ListItem),
            (Role::Switch, Control::Button),
            (Role::Checkbox, Control::CheckBox),
            (Role::Tab, Control::TabItem),
            (Role::TabList, Control::Tab),
            (Role::TabPanel, Control::Pane),
            (Role::Navigation, Control::Group),
            (Role::Main, Control::Group),
            (Role::ProgressBar, Control::ProgressBar),
            (Role::Slider, Control::Slider),
            (Role::ComboBox, Control::ComboBox),
            (Role::Radio, Control::RadioButton),
            (Role::RadioGroup, Control::Group),
            (Role::Dialog, Control::Window),
        ];
        for (role, control) in cases {
            assert_eq!(
                semantics(&a11y(role), true).control,
                Some(control),
                "{role:?}"
            );
        }
        // A live region is no element of its own.
        let status = semantics(&a11y(Role::Status), false);
        assert_eq!((status.control, status.element), (None, false));
        assert_eq!(status.live, Live::Polite);
        // A clickable view without a role is a button.
        let plain = semantics(&Accessibility::default(), true);
        assert_eq!(plain.control, Some(Control::Button));
        assert!(plain.invoke && plain.element);
        assert_eq!(semantics(&Accessibility::default(), false).control, None);
    }

    #[test]
    fn switches_and_check_boxes_toggle() {
        let mut a = a11y(Role::Switch);
        let s = semantics(&a, true);
        assert_eq!((s.toggled, s.switch, s.invoke), (Some(false), true, false));
        a.checked = Some(true);
        assert_eq!(semantics(&a, true).toggled, Some(true));
        let mut c = a11y(Role::Checkbox);
        c.checked = Some(true);
        let s = semantics(&c, true);
        assert_eq!(
            (s.control, s.toggled),
            (Some(Control::CheckBox), Some(true))
        );
        // A pressed button is a toggle button; a plain one is not.
        let mut b = a11y(Role::Button);
        assert_eq!(semantics(&b, true).toggled, None);
        b.checked = Some(false);
        let s = semantics(&b, true);
        assert_eq!(
            (s.toggled, s.selected, s.invoke),
            (Some(false), None, false)
        );
    }

    #[test]
    fn tabs_radios_and_rows_are_selected() {
        let mut t = a11y(Role::Tab);
        assert_eq!(semantics(&t, true).selected, Some(false));
        t.selected = Some(true);
        let s = semantics(&t, true);
        assert_eq!((s.selected, s.invoke), (Some(true), false));
        let mut r = a11y(Role::Radio);
        r.checked = Some(true);
        assert_eq!(semantics(&r, true).selected, Some(true));
        // A row says it can be selected only when the app says so; it is
        // still activated.
        let mut row = a11y(Role::ListItem);
        assert_eq!(semantics(&row, true).selected, None);
        row.selected = Some(false);
        let s = semantics(&row, true);
        assert_eq!((s.selected, s.invoke), (Some(false), true));
        // The current page of a navigation reads as selected.
        let mut link = a11y(Role::Link);
        link.current = true;
        assert_eq!(semantics(&link, true).selected, Some(true));
    }

    #[test]
    fn values_landmarks_headings_and_states() {
        let mut s = a11y(Role::Slider);
        s.value = Some("40 %".into());
        assert_eq!(semantics(&s, false).value.as_deref(), Some("40 %"));
        // A status announces its value instead.
        let mut st = a11y(Role::Status);
        st.value = Some("Saved".into());
        assert_eq!(semantics(&st, false).value, None);
        assert_eq!(
            semantics(&a11y(Role::Navigation), false).landmark,
            Landmark::Navigation
        );
        assert_eq!(semantics(&a11y(Role::Main), false).landmark, Landmark::Main);
        assert_eq!(semantics(&a11y(Role::Heading(9)), false).heading, 6);
        assert_eq!(semantics(&a11y(Role::Alert), false).live, Live::Assertive);
        let mut d = a11y(Role::Group);
        d.disabled = true;
        assert!(semantics(&d, false).disabled);
        d.hidden = true;
        let s = semantics(&d, false);
        assert!(!s.element && s.hidden);
        assert!(semantics(&a11y(Role::Dialog), false).dialog);
        assert!(dialog().dialog && dialog().element);
    }

    #[test]
    fn a_dialog_is_named_by_its_label_or_its_first_heading() {
        let texts = [("Pick a date", 0), ("New task", 2), ("Title", 1)];
        assert_eq!(dialog_name(Some("Filters"), &texts), Some("Filters"));
        assert_eq!(dialog_name(None, &texts), Some("Title"));
        assert_eq!(dialog_name(Some(""), &texts[..2]), Some("New task"));
        assert_eq!(dialog_name(None, &[(" ", 0), ("Body", 0)]), Some("Body"));
        assert_eq!(dialog_name(None, &[]), None);
    }
}
