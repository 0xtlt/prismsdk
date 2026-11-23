//! Accessibility: one description, mapped to ARIA on the web, `UIAccessibility`
//! on iOS, `NSAccessibility` on macOS, GTK's accessible roles on Linux,
//! Android's accessibility nodes and UI Automation on Windows.
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
    /// A table of data: rows of cells under column headers
    /// ([`table`](crate::table())).
    Table,
    /// One row of a [`Role::Table`].
    Row,
    /// The header of a table column: read with each cell of the column.
    ColumnHeader,
    /// One cell of a table row.
    Cell,
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
            Role::Table => "table",
            Role::Row => "row",
            Role::ColumnHeader => "columnheader",
            Role::Cell => "cell",
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
    /// Selected state of one item among several (tabs, options, rows of a
    /// list): `Some(false)` says the item can be selected but is not.
    pub selected: Option<bool>,
    /// The item that stands for where the user is: the current page of a
    /// navigation (a sidebar, breadcrumbs), the current step of a flow.
    pub current: bool,
    /// Shown as unavailable and not operable.
    pub disabled: bool,
    /// Removed from the accessibility tree (decorations, duplicates).
    pub hidden: bool,
    /// The tooltip shown while the pointer rests on it (#216).
    pub tooltip: Option<Tooltip>,
    /// Whether the element shows (`true`) or hides (`false`) more content:
    /// a disclosure, a row that folds its options, a menu button (#291).
    /// `None`: it does neither.
    pub expanded: Option<bool>,
    /// The `.id(…)` name of the element this one shows, hides or changes
    /// (the region a disclosure folds, #291).
    pub controls: Option<String>,
    /// The node [`controls`](Self::controls) names, found by Prism once
    /// both are mounted (backends read this one).
    pub controls_node: Option<crate::NodeId>,
}

/// A tooltip (#216): the platform's own, with its own delay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tooltip {
    /// This text.
    Text(String),
    /// A text's full content, only while it is cut short (an ellipsis, a
    /// line clamp): see [`Accessible::tooltip_when_truncated`].
    WhenTruncated,
}

impl Accessibility {
    /// Whether native screen readers read the element as selected: it is
    /// [`selected`](Self::selected), [`current`](Self::current), or a checked
    /// tab, radio or toggle button.
    pub fn is_selected(&self) -> bool {
        self.selected == Some(true)
            || self.current
            || (self.checked == Some(true)
                && matches!(self.role, Some(Role::Tab | Role::Button | Role::Radio)))
    }
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

    /// Sets the selected state of one item among several (a tab, an option,
    /// a row): `aria-selected` on the web, the selected trait or state on
    /// the native platforms. For the destination a navigation is showing,
    /// use [`accessibility_current`](Self::accessibility_current).
    fn accessibility_selected(mut self, selected: bool) -> Self {
        self.a11y_mut().selected = Some(selected);
        self
    }

    /// Marks the element as the current one of a set: the page shown, in a
    /// sidebar, a tab bar of links or a breadcrumb trail. `aria-current` on
    /// the web (`"page"` on a link), read as selected on the native platforms.
    fn accessibility_current(mut self, current: bool) -> Self {
        self.a11y_mut().current = current;
        self
    }

    /// Says whether the element shows (`true`) or hides (`false`) more
    /// content (#291): a disclosure, a row that folds its options, a
    /// button opening a menu. `aria-expanded` on the web; macOS:
    /// `accessibilityExpanded`; iOS, iPadOS, tvOS 18 and visionOS 2 and
    /// later: the expanded status, before them "expanded" / "collapsed"
    /// before its value; Android: the state description (Android 11+) and the expand
    /// or collapse action; Linux: the expanded state; Windows: the
    /// ExpandCollapse pattern; watchOS: its value.
    ///
    /// ```text
    /// div()
    ///     .child(text("Options"))
    ///     .on_click(cx.listener(|this, _, _| this.open = !this.open))
    ///     .accessibility_expanded(self.open)
    ///     .accessibility_controls("options")
    /// ```
    fn accessibility_expanded(mut self, expanded: bool) -> Self {
        self.a11y_mut().expanded = Some(expanded);
        self
    }

    /// Names the element this one shows, hides or changes: the one given
    /// `.id(name)` (#291). `aria-controls` on the web; macOS: its linked
    /// element; Linux: the controls relation; Windows: its controlled
    /// peer. iOS and Android have no such link (the expanded state is what
    /// they read).
    fn accessibility_controls(mut self, name: impl Into<String>) -> Self {
        self.a11y_mut().controls = Some(name.into());
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

    /// The platform's tooltip, shown while the pointer rests on the element
    /// (#216): `toolTip` on macOS, `UIToolTipInteraction` on iPadOS and
    /// visionOS (iOS 15), the `title` attribute on the web, GTK tooltips,
    /// `setTooltipText` on Android, `ToolTipService` on Windows. Screen
    /// readers read it as a description.
    /// Touch screens without a pointer show none (Android: a long press).
    fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.a11y_mut().tooltip = Some(Tooltip::Text(text.into()));
        self
    }

    /// On a text: its full content as a tooltip, only while it is
    /// truncated (`truncate()`, `line_clamp(n)`), checked by the platform
    /// as the pointer rests on it (#216). The full title of a tab or a
    /// sidebar row without measuring it. macOS, Linux, Android, Windows and
    /// the web; iPadOS shows none.
    fn tooltip_when_truncated(mut self) -> Self {
        self.a11y_mut().tooltip = Some(Tooltip::WhenTruncated);
        self
    }

    /// Marks text as a heading (`<h1>`… on the web, header trait on iOS).
    fn heading(self, level: u8) -> Self {
        self.accessibility_role(Role::Heading(level.clamp(1, 6)))
    }
}
