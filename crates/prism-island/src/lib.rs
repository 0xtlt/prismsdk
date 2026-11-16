//! # prism-island
//!
//! Client-side islands for Prism web apps, written in Rust and compiled to
//! WASM. An island is a small part of a page that runs in the browser (no
//! round trip to the server): a chart following the pointer, a canvas, a
//! drag-heavy editor. The rest of the app stays on the server.
//!
//! An island is a crate of its own (`crate-type = ["cdylib"]`) listed in
//! `Prism.toml` (`[web] islands = ["islands/chart"]`). `prism run/dev/build
//! web` compiles it for `wasm32-unknown-unknown` with a size profile, runs
//! wasm-bindgen (as a library: nothing to install) and serves the result.
//! The crate `chart` becomes the custom element `<prism-island-chart>`; the
//! server shows it with `island("chart")`.
//!
//! ```ignore
//! use prism_island::{Host, Island, Props};
//!
//! struct Hello { host: Host, text: web_sys::Element }
//!
//! impl Island for Hello {
//!     fn mount(host: Host, props: &Props) -> Self {
//!         let text = host.document().create_element("p").unwrap();
//!         host.root().append_child(&text).unwrap();
//!         let mut hello = Hello { host, text };
//!         hello.update(props);
//!         hello
//!     }
//!
//!     fn update(&mut self, changed: &Props) {
//!         if let Some(name) = changed.str("name") {
//!             self.text.set_text_content(Some(&name));
//!             self.host.emit("greeted", name.len() as f64);
//!         }
//!     }
//! }
//!
//! prism_island::register!(Hello);
//! ```
//!
//! ## The contract
//!
//! - **Rendering**: into the element's shadow root ([`Host::root`]). The
//!   element's light DOM holds the server's fallback (children of
//!   `island(…)`), which the shadow root hides once the island mounts and
//!   which Prism keeps patching.
//! - **Props in**: [`Island::mount`] gets all of them, [`Island::update`] the
//!   ones a server render changed (a removed prop is `null`). Read them
//!   with [`Props`]: numbers, strings, booleans, number arrays, any
//!   [`JsValue`]; or typed with serde (`serde` feature: [`Props::parse`]).
//! - **Events out**: [`Host::emit`] dispatches a `CustomEvent` on the element;
//!   the server's `.on(name, …)` handler gets its detail as JSON.
//!
//! ## Size
//!
//! Every dependency ends up in the `.wasm`. This crate only enables a few
//! `web-sys` features and does not need serde: the default path reads props
//! through JS values. `format!` and float formatting add ~10 KB, serde
//! ~25 KB (gzip); `prism build web` prints each island's size and fails
//! above `[web] island_budget_kb`.

#![warn(missing_docs)]

use std::cell::RefCell;

pub use js_sys;
pub use wasm_bindgen;
pub use wasm_bindgen::JsValue;
pub use web_sys;

use wasm_bindgen::JsCast;

/// A client-side island: created when its element appears in the page,
/// updated when the server changes its props, dropped when it is removed.
pub trait Island: 'static {
    /// Builds the island in `host`'s shadow root, with every prop.
    fn mount(host: Host, props: &Props) -> Self
    where
        Self: Sized;

    /// The server changed these props (only these; `null` when removed).
    fn update(&mut self, changed: &Props) {
        let _ = changed;
    }

    /// The element left the page: release what outlives the island (timers,
    /// listeners on `window`). The island is dropped right after.
    fn unmount(&mut self) {}
}

/// The island's custom element and its shadow root.
#[derive(Clone)]
pub struct Host {
    element: web_sys::HtmlElement,
    root: web_sys::ShadowRoot,
}

impl Host {
    /// The custom element (`<prism-island-chart>`): its size and position
    /// come from the server's styles.
    pub fn element(&self) -> &web_sys::HtmlElement {
        &self.element
    }

    /// The shadow root to render into (open, created before mounting).
    pub fn root(&self) -> &web_sys::ShadowRoot {
        &self.root
    }

    /// The element's document (to create elements).
    pub fn document(&self) -> web_sys::Document {
        self.element
            .owner_document()
            .or_else(|| web_sys::window().and_then(|w| w.document()))
            .expect("an island lives in a document")
    }

    /// Dispatches a `CustomEvent` named `name` with `detail` on the element
    /// (bubbling, composed). The server's handler for `name` gets the detail
    /// as JSON: numbers, strings, booleans, arrays and plain objects.
    pub fn emit(&self, name: &str, detail: impl Into<JsValue>) {
        let init = web_sys::CustomEventInit::new();
        init.set_bubbles(true);
        init.set_composed(true);
        init.set_detail(&detail.into());
        if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict(name, &init) {
            let _ = self.element.dispatch_event(&event);
        }
    }

    /// [`emit`](Self::emit) with a detail serialized by serde.
    #[cfg(feature = "serde")]
    pub fn emit_json<T: serde::Serialize + ?Sized>(&self, name: &str, detail: &T) {
        let detail = serde_json::to_string(detail)
            .ok()
            .and_then(|json| js_sys::JSON::parse(&json).ok())
            .unwrap_or(JsValue::NULL);
        self.emit(name, detail);
    }
}

/// Props from the server: a JS object, read by name.
pub struct Props(JsValue);

impl Props {
    /// Props from a JS object (`{points: [1, 2]}`).
    pub fn new(object: JsValue) -> Self {
        Props(object)
    }

    /// The object itself.
    pub fn raw(&self) -> &JsValue {
        &self.0
    }

    /// Prop `name`, if present (a removed prop is present, as `null`).
    pub fn get(&self, name: &str) -> Option<JsValue> {
        if !self.0.is_object() {
            return None;
        }
        js_sys::Reflect::get(&self.0, &JsValue::from_str(name))
            .ok()
            .filter(|v| !v.is_undefined())
    }

    /// Whether prop `name` is present.
    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Prop `name` as a number.
    pub fn f64(&self, name: &str) -> Option<f64> {
        self.get(name)?.as_f64()
    }

    /// Prop `name` as a string.
    pub fn str(&self, name: &str) -> Option<String> {
        self.get(name)?.as_string()
    }

    /// Prop `name` as a boolean.
    pub fn bool(&self, name: &str) -> Option<bool> {
        self.get(name)?.as_bool()
    }

    /// Prop `name` as an array of numbers (`Vec<f64>` on the server).
    /// Entries that are not numbers read as `NaN`.
    pub fn f64s(&self, name: &str) -> Option<Vec<f64>> {
        let v = self.get(name)?;
        if !js_sys::Array::is_array(&v) {
            return None;
        }
        Some(js_sys::Float64Array::new(&v).to_vec())
    }

    /// Prop `name` as an array of JS values.
    pub fn array(&self, name: &str) -> Option<Vec<JsValue>> {
        let v = self.get(name)?;
        js_sys::Array::is_array(&v).then(|| js_sys::Array::from(&v).to_vec())
    }

    /// Prop `name` parsed as `T` (from its JSON).
    #[cfg(feature = "serde")]
    pub fn parse<T: serde::de::DeserializeOwned>(&self, name: &str) -> Option<T> {
        let json: String = js_sys::JSON::stringify(&self.get(name)?).ok()?.into();
        serde_json::from_str(&json).ok()
    }
}

/// A plain JS object built in Rust (an event detail): `Object::new().num("x", 1.)`.
pub struct Object(js_sys::Object);

impl Object {
    /// An empty object.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Object(js_sys::Object::new())
    }

    /// Sets `key` to `value`.
    pub fn set(self, key: &str, value: impl Into<JsValue>) -> Self {
        let _ = js_sys::Reflect::set(&self.0, &JsValue::from_str(key), &value.into());
        self
    }
}

impl From<Object> for JsValue {
    fn from(o: Object) -> JsValue {
        o.0.into()
    }
}

/// `[a, b, …]` as a JS array (an event detail the server reads as a tuple).
pub fn array(items: impl IntoIterator<Item = JsValue>) -> JsValue {
    items.into_iter().collect::<js_sys::Array>().into()
}

trait Mounted {
    fn update(&mut self, changed: &Props);
    fn unmount(&mut self);
}

impl<T: Island> Mounted for T {
    fn update(&mut self, changed: &Props) {
        Island::update(self, changed)
    }

    fn unmount(&mut self) {
        Island::unmount(self)
    }
}

thread_local! {
    /// Mounted islands, by handle (index + 1).
    static MOUNTED: RefCell<Vec<Option<Box<dyn Mounted>>>> = const { RefCell::new(Vec::new()) };
}

/// Used by [`register!`]; not public API.
#[doc(hidden)]
pub mod __private {
    use super::*;

    pub fn mount<T: Island>(element: JsValue, root: JsValue, props: JsValue) -> u32 {
        let host = Host {
            element: element.unchecked_into(),
            root: root.unchecked_into(),
        };
        let island: Box<dyn Mounted> = Box::new(T::mount(host, &Props(props)));
        MOUNTED.with(|m| {
            let mut m = m.borrow_mut();
            match m.iter().position(Option::is_none) {
                Some(i) => {
                    m[i] = Some(island);
                    i as u32 + 1
                }
                None => {
                    m.push(Some(island));
                    m.len() as u32
                }
            }
        })
    }

    pub fn update(handle: u32, changed: JsValue) {
        // Taken out while it runs: an update that mounts another island
        // (or emits synchronously into one) cannot hit a borrowed table.
        let Some(mut island) = take(handle) else {
            return;
        };
        island.update(&Props(changed));
        put(handle, island);
    }

    pub fn unmount(handle: u32) {
        if let Some(mut island) = take(handle) {
            island.unmount();
        }
    }

    fn take(handle: u32) -> Option<Box<dyn Mounted>> {
        let i = (handle as usize).checked_sub(1)?;
        MOUNTED.with(|m| m.borrow_mut().get_mut(i)?.take())
    }

    fn put(handle: u32, island: Box<dyn Mounted>) {
        MOUNTED.with(|m| m.borrow_mut()[handle as usize - 1] = Some(island));
    }
}

/// Exports an [`Island`] type as this crate's island (once per crate):
/// `prism_island::register!(Chart);`. `prism` generates the custom element
/// around these exports.
#[macro_export]
macro_rules! register {
    ($island:ty) => {
        #[doc(hidden)]
        #[$crate::wasm_bindgen::prelude::wasm_bindgen(wasm_bindgen = $crate::wasm_bindgen)]
        pub fn __prism_mount(
            element: $crate::JsValue,
            root: $crate::JsValue,
            props: $crate::JsValue,
        ) -> u32 {
            $crate::__private::mount::<$island>(element, root, props)
        }

        #[doc(hidden)]
        #[$crate::wasm_bindgen::prelude::wasm_bindgen(wasm_bindgen = $crate::wasm_bindgen)]
        pub fn __prism_update(handle: u32, changed: $crate::JsValue) {
            $crate::__private::update(handle, changed)
        }

        #[doc(hidden)]
        #[$crate::wasm_bindgen::prelude::wasm_bindgen(wasm_bindgen = $crate::wasm_bindgen)]
        pub fn __prism_unmount(handle: u32) {
            $crate::__private::unmount(handle)
        }
    };
}
