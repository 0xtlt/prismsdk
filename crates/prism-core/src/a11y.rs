//! Accessibility: one description, mapped to ARIA on the web, `UIAccessibility`
//! on iOS and `NSAccessibility` on macOS.
//!
//! Defaults are automatic: buttons, inputs, toggles, tabs and screens carry the
//! right role; a clickable `div` becomes a button labelled with its text. Use
//! the [`Accessible`] methods to refine.

/// What an element *is* for assistive technologies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Performs an action when activated.
    Button,
    /// Navigates somewhere (a `link(...)` destination).
    Link,
    /// A heading, level 1–6.
    Heading(u8),
    /// A picture; its label describes it.
    Image,
    /// Static text.
    Text,
    /// Groups children (announced together on Apple platforms).
    Group,
    /// A list of items.
    List,
    /// One item of a [`Role::List`].
    ListItem,
    /// An on/off control.
    Switch,
    /// A checkable box.
    Checkbox,
    /// One tab of a [`Role::TabList`].
    Tab,
    /// A tab bar.
    TabList,
    /// The content shown for the selected tab.
    TabPanel,
    /// Navigation landmark (bars, sidebars).
    Navigation,
    /// Main content landmark.
    Main,
    /// Important, time-sensitive information, announced when it appears.
    Alert,
    /// Progress of a task.
    ProgressBar,
    /// An adjustable value (increment / decrement with assistive technologies).
    Slider,
    /// A control that opens a list of options (a select, a pop-up button).
    ComboBox,
    /// A polite live region: its `value` is announced whenever it changes
    /// (toasts, "Saved"). Not an element by itself: children stay reachable.
    Status,
    /// One choice of a [`Role::RadioGroup`].
    Radio,
    /// A set of mutually exclusive choices.
    RadioGroup,
    /// A modal dialog or sheet.
    Dialog,
}

impl Role {
    /// The ARIA role name.
    pub fn aria(self) -> &'static str {
        match self {
            Role::Button => "button",
            Role::Link => "link",
            Role::Heading(_) => "heading",
            Role::Image => "img",
            Role::Text => "text",
            Role::Group => "group",
            Role::List => "list",
            Role::ListItem => "listitem",
            Role::Switch => "switch",
            Role::Checkbox => "checkbox",
            Role::Tab => "tab",
            Role::TabList => "tablist",
            Role::TabPanel => "tabpanel",
            Role::Navigation => "navigation",
            Role::Main => "main",
            Role::Alert => "alert",
            Role::ProgressBar => "progressbar",
            Role::Slider => "slider",
            Role::ComboBox => "combobox",
            Role::Status => "status",
            Role::Radio => "radio",
            Role::RadioGroup => "radiogroup",
            Role::Dialog => "dialog",
        }
    }

    /// Whether the element is operable (focusable, activatable from keyboard
    /// and assistive technologies).
    pub fn is_interactive(self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::Link
                | Role::Switch
                | Role::Checkbox
                | Role::Tab
                | Role::Slider
                | Role::ComboBox
                | Role::Radio
        )
    }
}

/// The accessibility description of a node.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Accessibility {
    /// What the element is. `None` lets the backend infer it (a clickable `div` becomes a button).
    pub role: Option<Role>,
    /// What is read out. Defaults to the element's text.
    pub label: Option<String>,
    /// Extra guidance, read after a pause ("Opens the detail screen").
    pub hint: Option<String>,
    /// Current value for adjustable or stateful controls ("On", "3 of 5").
    pub value: Option<String>,
    /// Checked state for switches, checkboxes and selected tabs.
    pub checked: Option<bool>,
    /// Shown as unavailable and not operable.
    pub disabled: bool,
    /// Removed from the accessibility tree (decorations, duplicates).
    pub hidden: bool,
}

/// Accessibility modifiers, available on every element builder.
pub trait Accessible: Sized {
    /// The description these modifiers edit.
    fn a11y_mut(&mut self) -> &mut Accessibility;

    /// Overrides the inferred role.
    fn accessibility_role(mut self, role: Role) -> Self {
        self.a11y_mut().role = Some(role);
        self
    }

    /// Sets what is read out instead of the element's text.
    fn accessibility_label(mut self, label: impl Into<String>) -> Self {
        self.a11y_mut().label = Some(label.into());
        self
    }

    /// Adds guidance read after the label ("Opens the detail screen").
    fn accessibility_hint(mut self, hint: impl Into<String>) -> Self {
        self.a11y_mut().hint = Some(hint.into());
        self
    }

    /// Sets the current value ("On", "3 of 5").
    fn accessibility_value(mut self, value: impl Into<String>) -> Self {
        self.a11y_mut().value = Some(value.into());
        self
    }

    /// Sets the checked state (switches, checkboxes, selected tabs).
    fn accessibility_checked(mut self, checked: bool) -> Self {
        self.a11y_mut().checked = Some(checked);
        self
    }

    /// Marks the element as unavailable.
    fn accessibility_disabled(mut self, disabled: bool) -> Self {
        self.a11y_mut().disabled = disabled;
        self
    }

    /// Hides a decorative element from screen readers.
    fn accessibility_hidden(mut self) -> Self {
        self.a11y_mut().hidden = true;
        self
    }

    /// Marks text as a heading (`<h1>`… on the web, header trait on iOS).
    fn heading(self, level: u8) -> Self {
        self.accessibility_role(Role::Heading(level.clamp(1, 6)))
    }
}
