//! In the car: CarPlay (iOS) and Android Auto / Android Automotive. Both only
//! show the platform's templates (lists, grids, panes, tabs), so a car screen
//! is described with those, not with views.
//!
//! ```ignore
//! cx.register_car(move |cx| {
//!     CarTemplate::list("Todos").section(
//!         CarSection::new().items(store.read(cx).todos.iter().map(|t| {
//!             let id = t.id;
//!             CarItem::new(&t.title)
//!                 .icon("checkmark.circle")
//!                 .on_select(move |cx| cx.car_push(move |cx| detail(cx, id)))
//!         })),
//!     )
//! });
//! ```
//!
//! The templates are rebuilt whenever the app changes, like widgets; the host
//! only hears about it when they differ. Selections come back through
//! [`crate::Runtime::car_select`], the host's Back through
//! [`crate::Runtime::car_back`].

use std::rc::Rc;

use crate::App;
use crate::surfaces::json_str;

type Handler = Rc<dyn Fn(&mut App)>;
type Builder = Rc<dyn Fn(&mut App) -> CarTemplate>;

/// One car screen.
pub enum CarTemplate {
    /// Rows in sections (CPListTemplate, ListTemplate).
    List {
        /// Shown in the car's navigation bar.
        title: String,
        /// The rows, grouped.
        sections: Vec<CarSection>,
    },
    /// Large buttons with icons (CPGridTemplate, GridTemplate).
    Grid {
        /// Shown in the car's navigation bar.
        title: String,
        /// The grid's buttons, in order.
        buttons: Vec<CarButton>,
    },
    /// Label/value rows and up to two actions (CPInformationTemplate, PaneTemplate).
    Pane {
        /// Shown in the car's navigation bar.
        title: String,
        /// `(label, value)` rows.
        rows: Vec<(String, String)>,
        /// Buttons under the rows (the platforms show up to two).
        actions: Vec<CarButton>,
    },
    /// Tabs, each with its own template (CPTabBarTemplate, TabTemplate).
    Tabs(Vec<CarTab>),
}

impl CarTemplate {
    /// An empty list; add rows with [`section`](Self::section).
    pub fn list(title: impl Into<String>) -> Self {
        CarTemplate::List {
            title: title.into(),
            sections: Vec::new(),
        }
    }
    /// An empty grid; add buttons with [`button`](Self::button).
    pub fn grid(title: impl Into<String>) -> Self {
        CarTemplate::Grid {
            title: title.into(),
            buttons: Vec::new(),
        }
    }
    /// An empty pane; add [`row`](Self::row)s and [`button`](Self::button)s.
    pub fn pane(title: impl Into<String>) -> Self {
        CarTemplate::Pane {
            title: title.into(),
            rows: Vec::new(),
            actions: Vec::new(),
        }
    }
    /// A tab bar. Android Auto shows the first tab only (`TabTemplate` needs
    /// Car API level 6).
    pub fn tabs(tabs: impl IntoIterator<Item = CarTab>) -> Self {
        CarTemplate::Tabs(tabs.into_iter().collect())
    }

    /// Lists: a section.
    pub fn section(mut self, section: CarSection) -> Self {
        if let CarTemplate::List { sections, .. } = &mut self {
            sections.push(section);
        }
        self
    }
    /// Grids: a button. Panes: an action.
    pub fn button(mut self, button: CarButton) -> Self {
        match &mut self {
            CarTemplate::Grid { buttons, .. } => buttons.push(button),
            CarTemplate::Pane { actions, .. } => actions.push(button),
            _ => {}
        }
        self
    }
    /// Panes: a label and its value.
    pub fn row(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        if let CarTemplate::Pane { rows, .. } = &mut self {
            rows.push((label.into(), value.into()));
        }
        self
    }
}

/// A group of list rows, with an optional header.
#[derive(Default)]
pub struct CarSection {
    /// Text above the rows.
    pub header: Option<String>,
    /// The rows, in order.
    pub items: Vec<CarItem>,
}

impl CarSection {
    /// An empty section without a header.
    pub fn new() -> Self {
        Self::default()
    }
    /// Text above the rows.
    pub fn header(mut self, header: impl Into<String>) -> Self {
        self.header = Some(header.into());
        self
    }
    /// Adds a row.
    pub fn item(mut self, item: CarItem) -> Self {
        self.items.push(item);
        self
    }
    /// Adds several rows.
    pub fn items(mut self, items: impl IntoIterator<Item = CarItem>) -> Self {
        self.items.extend(items);
        self
    }
}

/// A list row.
pub struct CarItem {
    /// The row's main text.
    pub title: String,
    /// Secondary text under the title.
    pub detail: Option<String>,
    /// SF Symbol name (drawn as a glyph where symbols do not exist).
    pub icon: Option<String>,
    /// Shows a chevron: selecting opens another screen.
    pub browsable: bool,
    /// Runs when the driver selects the row.
    pub on_select: Option<Handler>,
}

impl CarItem {
    /// A row with a title only.
    pub fn new(title: impl Into<String>) -> Self {
        CarItem {
            title: title.into(),
            detail: None,
            icon: None,
            browsable: false,
            on_select: None,
        }
    }
    /// Secondary text under the title.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    /// An SF Symbol name (drawn as a glyph where symbols do not exist).
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    /// Shows a chevron: selecting opens another screen.
    pub fn browsable(mut self) -> Self {
        self.browsable = true;
        self
    }
    /// Runs `f` when the driver selects the row (push a screen with
    /// `cx.car_push`).
    pub fn on_select(mut self, f: impl Fn(&mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

/// How a car button is drawn.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum CarButtonStyle {
    /// The platform's regular button.
    #[default]
    Normal,
    /// Emphasized: the main action.
    Primary,
    /// Marks an action that deletes or cancels something.
    Destructive,
}

/// A grid button or a pane action.
pub struct CarButton {
    /// The button's label.
    pub title: String,
    /// SF Symbol name (drawn as a glyph where symbols do not exist).
    pub icon: Option<String>,
    /// Normal, primary or destructive.
    pub style: CarButtonStyle,
    /// Runs when the driver selects the button.
    pub on_select: Option<Handler>,
}

impl CarButton {
    /// A normal button with a label only.
    pub fn new(title: impl Into<String>) -> Self {
        CarButton {
            title: title.into(),
            icon: None,
            style: CarButtonStyle::Normal,
            on_select: None,
        }
    }
    /// An SF Symbol name (drawn as a glyph where symbols do not exist).
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    /// Normal, primary or destructive.
    pub fn style(mut self, style: CarButtonStyle) -> Self {
        self.style = style;
        self
    }
    /// Runs `f` when the driver selects the button.
    pub fn on_select(mut self, f: impl Fn(&mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
}

/// One tab of [`CarTemplate::tabs`].
pub struct CarTab {
    /// The tab's label.
    pub title: String,
    /// SF Symbol name (drawn as a glyph where symbols do not exist).
    pub icon: Option<String>,
    /// What the tab shows.
    pub template: CarTemplate,
}

impl CarTab {
    /// A tab showing `template`.
    pub fn new(title: impl Into<String>, template: CarTemplate) -> Self {
        CarTab {
            title: title.into(),
            icon: None,
            template,
        }
    }
    /// An SF Symbol name for the tab bar.
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }
}

/// The car's screens: the registered root and what the app pushed.
#[derive(Default)]
pub(crate) struct CarState {
    root: Option<Builder>,
    pushed: Vec<Builder>,
    /// Handlers of the last build, by id.
    handlers: Vec<Handler>,
    /// The last JSON sent, to send only changes.
    last: String,
    pub connected: bool,
}

impl App {
    /// The car's root screen, rebuilt whenever the app changes.
    pub fn register_car(&mut self, root: impl Fn(&mut App) -> CarTemplate + 'static) {
        self.car.root = Some(Rc::new(root));
        self.car.last.clear();
    }

    /// Opens a screen on top (Back returns).
    pub fn car_push(&mut self, screen: impl Fn(&mut App) -> CarTemplate + 'static) {
        self.car.pushed.push(Rc::new(screen));
    }

    /// Closes the top screen.
    pub fn car_pop(&mut self) {
        self.car.pushed.pop();
    }

    /// Back to the root screen.
    pub fn car_pop_to_root(&mut self) {
        self.car.pushed.clear();
    }

    /// Whether the app has car screens (bundlers declare CarPlay / Android Auto).
    pub fn has_car(&self) -> bool {
        self.car.root.is_some()
    }

    /// Whether a car screen shows the app.
    pub fn car_connected(&self) -> bool {
        self.car.connected
    }

    /// The stack as JSON when it changed since the last call (backends).
    pub(crate) fn car_update(&mut self) -> Option<String> {
        let root = self.car.root.clone()?;
        if !self.car.connected {
            return None;
        }
        let builders: Vec<Builder> = std::iter::once(root)
            .chain(self.car.pushed.iter().cloned())
            .collect();
        let mut handlers = Vec::new();
        let screens: Vec<String> = builders
            .iter()
            .map(|b| {
                let template = b(self);
                template_json(&template, &mut handlers)
            })
            .collect();
        let json = format!(r#"{{"stack":[{}]}}"#, screens.join(","));
        self.car.handlers = handlers;
        if json == self.car.last {
            return None;
        }
        self.car.last = json.clone();
        Some(json)
    }

    pub(crate) fn car_select(&mut self, id: u64) {
        if let Some(h) = self.car.handlers.get(id as usize).cloned() {
            h(self);
        }
    }

    /// The host went back to screen `depth` (0: the root).
    pub(crate) fn car_back(&mut self, depth: usize) {
        self.car.pushed.truncate(depth);
    }

    pub(crate) fn set_car_connected(&mut self, connected: bool) {
        self.car.connected = connected;
        self.car.last.clear();
        if !connected {
            self.car.pushed.clear();
        }
    }
}

fn opt(s: &Option<String>) -> String {
    s.as_deref().map_or("null".into(), json_str)
}

/// `"icon":"…","glyph":"…"` (the glyph for hosts without SF Symbols).
fn icon_json(icon: &Option<String>) -> String {
    match icon {
        Some(i) => format!(
            r#""icon":{},"glyph":{}"#,
            json_str(i),
            json_str(crate::glyphs::glyph(i))
        ),
        None => r#""icon":null,"glyph":null"#.into(),
    }
}

fn handler_id(h: &Option<Handler>, handlers: &mut Vec<Handler>) -> String {
    match h {
        Some(h) => {
            handlers.push(h.clone());
            (handlers.len() - 1).to_string()
        }
        None => "null".into(),
    }
}

fn button_json(b: &CarButton, handlers: &mut Vec<Handler>) -> String {
    let style = match b.style {
        CarButtonStyle::Normal => "normal",
        CarButtonStyle::Primary => "primary",
        CarButtonStyle::Destructive => "destructive",
    };
    format!(
        r#"{{"title":{},{},"style":"{style}","id":{}}}"#,
        json_str(&b.title),
        icon_json(&b.icon),
        handler_id(&b.on_select, handlers)
    )
}

fn template_json(t: &CarTemplate, handlers: &mut Vec<Handler>) -> String {
    match t {
        CarTemplate::List { title, sections } => {
            let sections: Vec<String> = sections
                .iter()
                .map(|s| {
                    let items: Vec<String> = s
                        .items
                        .iter()
                        .map(|i| {
                            format!(
                                r#"{{"title":{},"detail":{},{},"browsable":{},"id":{}}}"#,
                                json_str(&i.title),
                                opt(&i.detail),
                                icon_json(&i.icon),
                                i.browsable,
                                handler_id(&i.on_select, handlers)
                            )
                        })
                        .collect();
                    format!(
                        r#"{{"header":{},"items":[{}]}}"#,
                        opt(&s.header),
                        items.join(",")
                    )
                })
                .collect();
            format!(
                r#"{{"t":"list","title":{},"sections":[{}]}}"#,
                json_str(title),
                sections.join(",")
            )
        }
        CarTemplate::Grid { title, buttons } => {
            let buttons: Vec<String> = buttons.iter().map(|b| button_json(b, handlers)).collect();
            format!(
                r#"{{"t":"grid","title":{},"buttons":[{}]}}"#,
                json_str(title),
                buttons.join(",")
            )
        }
        CarTemplate::Pane {
            title,
            rows,
            actions,
        } => {
            let rows: Vec<String> = rows
                .iter()
                .map(|(l, v)| format!("[{},{}]", json_str(l), json_str(v)))
                .collect();
            let actions: Vec<String> = actions.iter().map(|b| button_json(b, handlers)).collect();
            format!(
                r#"{{"t":"pane","title":{},"rows":[{}],"actions":[{}]}}"#,
                json_str(title),
                rows.join(","),
                actions.join(",")
            )
        }
        CarTemplate::Tabs(tabs) => {
            let tabs: Vec<String> = tabs
                .iter()
                .map(|tab| {
                    format!(
                        r#"{{"title":{},{},"template":{}}}"#,
                        json_str(&tab.title),
                        icon_json(&tab.icon),
                        template_json(&tab.template, handlers)
                    )
                })
                .collect();
            format!(r#"{{"t":"tabs","tabs":[{}]}}"#, tabs.join(","))
        }
    }
}
