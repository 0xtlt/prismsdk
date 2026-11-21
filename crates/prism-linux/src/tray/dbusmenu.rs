//! `com.canonical.dbusmenu`: a tray's menu as panels read it. The panel asks
//! for the tree with `GetLayout` and sends clicks back with `Event`; nothing
//! here touches the bus, so the item's thread (item.rs) serves it and the
//! tests call it directly.

use gtk::glib::{self, Variant, VariantTy, prelude::*};

/// The introspection data of the interface, served at
/// [`MENU_PATH`](super::item::MENU_PATH).
pub(crate) const XML: &str = r#"<node>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg name="parentId" type="i" direction="in"/>
      <arg name="recursionDepth" type="i" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="revision" type="u" direction="out"/>
      <arg name="layout" type="(ia{sv}av)" direction="out"/>
    </method>
    <method name="GetGroupProperties">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="propertyNames" type="as" direction="in"/>
      <arg name="properties" type="a(ia{sv})" direction="out"/>
    </method>
    <method name="GetProperty">
      <arg name="id" type="i" direction="in"/>
      <arg name="name" type="s" direction="in"/>
      <arg name="value" type="v" direction="out"/>
    </method>
    <method name="Event">
      <arg name="id" type="i" direction="in"/>
      <arg name="eventId" type="s" direction="in"/>
      <arg name="data" type="v" direction="in"/>
      <arg name="timestamp" type="u" direction="in"/>
    </method>
    <method name="EventGroup">
      <arg name="events" type="a(isvu)" direction="in"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <method name="AboutToShow">
      <arg name="id" type="i" direction="in"/>
      <arg name="needUpdate" type="b" direction="out"/>
    </method>
    <method name="AboutToShowGroup">
      <arg name="ids" type="ai" direction="in"/>
      <arg name="updatesNeeded" type="ai" direction="out"/>
      <arg name="idErrors" type="ai" direction="out"/>
    </method>
    <signal name="ItemsPropertiesUpdated">
      <arg name="updatedProps" type="a(ia{sv})"/>
      <arg name="removedProps" type="a(ias)"/>
    </signal>
    <signal name="LayoutUpdated">
      <arg name="revision" type="u"/>
      <arg name="parent" type="i"/>
    </signal>
    <signal name="ItemActivationRequested">
      <arg name="id" type="i"/>
      <arg name="timestamp" type="u"/>
    </signal>
  </interface>
</node>"#;

pub(crate) const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// A menu item the tray thread can own.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    Action {
        id: u64,
        title: String,
        enabled: bool,
        checked: Option<bool>,
    },
    Separator,
    Sub {
        title: String,
        items: Vec<Item>,
    },
}

/// One entry of the tree; entry 0 is the root, which panels never show.
#[derive(Debug, Default)]
struct Node {
    label: String,
    separator: bool,
    disabled: bool,
    checked: Option<bool>,
    /// The `MenuSelect` id a click reports.
    action: Option<u64>,
    children: Vec<usize>,
}

/// A menu with the ids panels know its items by.
#[derive(Debug)]
pub(crate) struct Menu {
    nodes: Vec<Node>,
    /// Items are `base + 1`, `base + 2`… (the root is 0). A new menu starts
    /// after the old one's ids, so a click on an item the panel still shows
    /// from before is refused instead of taken for another item.
    base: i32,
}

/// What a method call answers, and the items it clicked.
pub(crate) struct Answer {
    /// The reply's arguments (`None`: no arguments), or a D-Bus error.
    pub reply: Result<Option<Variant>, (&'static str, String)>,
    pub clicked: Vec<u64>,
}

impl Answer {
    fn value(v: Variant) -> Self {
        Answer {
            reply: Ok(Some(v)),
            clicked: Vec::new(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Answer {
            reply: Err((INVALID_ARGS, message.into())),
            clicked: Vec::new(),
        }
    }
}

fn add(nodes: &mut Vec<Node>, parent: usize, items: &[Item]) {
    for item in items {
        let index = nodes.len();
        nodes.push(match item {
            Item::Action {
                id,
                title,
                enabled,
                checked,
            } => Node {
                label: title.clone(),
                disabled: !enabled,
                checked: *checked,
                action: Some(*id),
                ..Node::default()
            },
            Item::Separator => Node {
                separator: true,
                ..Node::default()
            },
            Item::Sub { title, .. } => Node {
                label: title.clone(),
                ..Node::default()
            },
        });
        nodes[parent].children.push(index);
        if let Item::Sub { items, .. } = item {
            add(nodes, index, items);
        }
    }
}

impl Menu {
    pub(crate) fn new(items: &[Item]) -> Self {
        Self::with_base(items, 0)
    }

    fn with_base(items: &[Item], base: i32) -> Self {
        let mut nodes = vec![Node::default()];
        add(&mut nodes, 0, items);
        // Ids start over once they would pass i32::MAX.
        let fits = i32::try_from(nodes.len())
            .ok()
            .and_then(|n| base.checked_add(n))
            .is_some();
        Menu {
            nodes,
            base: if fits { base } else { 0 },
        }
    }

    /// The menu that replaces this one, with ids this one never used.
    pub(crate) fn next(&self, items: &[Item]) -> Self {
        let base = i32::try_from(self.nodes.len())
            .ok()
            .and_then(|n| self.base.checked_add(n))
            .unwrap_or(0);
        Self::with_base(items, base)
    }

    fn id(&self, index: usize) -> i32 {
        if index == 0 {
            0
        } else {
            self.base + index as i32
        }
    }

    fn index(&self, id: i32) -> Option<usize> {
        if id == 0 {
            return Some(0);
        }
        let index = usize::try_from(id.checked_sub(self.base)?).ok()?;
        (index > 0 && index < self.nodes.len()).then_some(index)
    }

    /// The item's properties (`a{sv}`), defaults left out as the
    /// specification asks; `names` empty means all of them.
    fn properties(&self, index: usize, names: &[String]) -> Variant {
        let node = &self.nodes[index];
        let want = |name: &str| names.is_empty() || names.iter().any(|n| n == name);
        let dict = glib::VariantDict::new(None);
        if node.separator && want("type") {
            dict.insert("type", "separator");
        }
        if !node.label.is_empty() && want("label") {
            // An underscore marks the access key: a title's own is doubled.
            dict.insert("label", node.label.replace('_', "__"));
        }
        if node.disabled && want("enabled") {
            dict.insert("enabled", false);
        }
        if let Some(on) = node.checked {
            if want("toggle-type") {
                dict.insert("toggle-type", "checkmark");
            }
            if want("toggle-state") {
                dict.insert("toggle-state", i32::from(on));
            }
        }
        if !node.children.is_empty() && want("children-display") {
            dict.insert("children-display", "submenu");
        }
        dict.end()
    }

    /// The subtree at `index` (`(ia{sv}av)`), `depth` levels of children
    /// deep (-1: all).
    fn layout(&self, index: usize, depth: i32, names: &[String]) -> Variant {
        let children = if depth == 0 {
            Vec::new()
        } else {
            self.nodes[index]
                .children
                .iter()
                .map(|&c| Variant::from_variant(&self.layout(c, (depth - 1).max(-1), names)))
                .collect()
        };
        Variant::tuple_from_iter([
            self.id(index).to_variant(),
            self.properties(index, names),
            Variant::array_from_iter_with_type(VariantTy::VARIANT, children),
        ])
    }

    /// The action a click on `id` reports: `Err` for an unknown id, `None`
    /// for an item that reports nothing (separator, submenu, disabled).
    fn click(&self, id: i32) -> Result<Option<u64>, ()> {
        let node = &self.nodes[self.index(id).ok_or(())?];
        Ok(node.action.filter(|_| !node.disabled))
    }

    /// Answers a `com.canonical.dbusmenu` method call.
    pub(crate) fn call(&self, revision: u32, method: &str, params: &Variant) -> Answer {
        match method {
            "GetLayout" => {
                let Some((parent, depth, names)) = params.get::<(i32, i32, Vec<String>)>() else {
                    return Answer::error("bad arguments");
                };
                match self.index(parent) {
                    Some(index) => Answer::value(Variant::tuple_from_iter([
                        revision.to_variant(),
                        self.layout(index, depth.max(-1), &names),
                    ])),
                    None => Answer::error("parentId not found"),
                }
            }
            "GetGroupProperties" => {
                let Some((ids, names)) = params.get::<(Vec<i32>, Vec<String>)>() else {
                    return Answer::error("bad arguments");
                };
                // No ids: every item.
                let indices: Vec<usize> = if ids.is_empty() {
                    (0..self.nodes.len()).collect()
                } else {
                    ids.iter().filter_map(|&id| self.index(id)).collect()
                };
                let entries = indices.into_iter().map(|i| {
                    Variant::tuple_from_iter([self.id(i).to_variant(), self.properties(i, &names)])
                });
                let ty = VariantTy::new("(ia{sv})").expect("type");
                Answer::value(Variant::tuple_from_iter([
                    Variant::array_from_iter_with_type(ty, entries),
                ]))
            }
            "GetProperty" => {
                let Some((id, name)) = params.get::<(i32, String)>() else {
                    return Answer::error("bad arguments");
                };
                let Some(index) = self.index(id) else {
                    return Answer::error("id not found");
                };
                let props = glib::VariantDict::new(Some(
                    &self.properties(index, std::slice::from_ref(&name)),
                ));
                match props.lookup_value(&name, None) {
                    Some(v) => Answer::value(Variant::tuple_from_iter([Variant::from_variant(&v)])),
                    None => Answer::error("property not found"),
                }
            }
            "Event" => {
                let Some((id, event, _data, _time)) = params.get::<(i32, String, Variant, u32)>()
                else {
                    return Answer::error("bad arguments");
                };
                match self.click(id) {
                    Ok(action) => Answer {
                        reply: Ok(None),
                        clicked: action.filter(|_| event == "clicked").into_iter().collect(),
                    },
                    Err(()) => Answer::error("id not found"),
                }
            }
            "EventGroup" => {
                let Some((events,)) = params.get::<(Vec<(i32, String, Variant, u32)>,)>() else {
                    return Answer::error("bad arguments");
                };
                let mut errors = Vec::new();
                let mut clicked = Vec::new();
                for (id, event, _, _) in &events {
                    match self.click(*id) {
                        Ok(action) if event == "clicked" => clicked.extend(action),
                        Ok(_) => {}
                        Err(()) => errors.push(*id),
                    }
                }
                if !events.is_empty() && errors.len() == events.len() {
                    return Answer::error("none of the ids found");
                }
                Answer {
                    reply: Ok(Some((errors,).to_variant())),
                    clicked,
                }
            }
            // The menu is complete when the panel asks: nothing to update.
            "AboutToShow" => match params.get::<(i32,)>() {
                Some((id,)) if self.index(id).is_some() => Answer::value((false,).to_variant()),
                _ => Answer::error("id not found"),
            },
            "AboutToShowGroup" => {
                let Some((ids,)) = params.get::<(Vec<i32>,)>() else {
                    return Answer::error("bad arguments");
                };
                let errors: Vec<i32> = ids
                    .iter()
                    .copied()
                    .filter(|&id| self.index(id).is_none())
                    .collect();
                if !ids.is_empty() && errors.len() == ids.len() {
                    return Answer::error("none of the ids found");
                }
                Answer::value((Vec::<i32>::new(), errors).to_variant())
            }
            _ => Answer {
                reply: Err((
                    "org.freedesktop.DBus.Error.UnknownMethod",
                    format!("no method {method}"),
                )),
                clicked: Vec::new(),
            },
        }
    }
}

/// A `com.canonical.dbusmenu` property.
pub(crate) fn property(name: &str) -> Variant {
    match name {
        "Version" => 3u32.to_variant(),
        "TextDirection" => "ltr".to_variant(),
        "Status" => "normal".to_variant(),
        _ => Vec::<String>::new().to_variant(), // IconThemePath
    }
}
