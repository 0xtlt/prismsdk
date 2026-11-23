//! Custom UI Automation peers for the elements Prism draws itself.
//!
//! A clickable view is a `Button` without chrome and a container a
//! `Canvas`: WinUI gives the first a button's peer and the second none (a
//! group's when it has a name). Prism's roles and states (`access.rs`)
//! need more: a control type of their own (a link, a tab, a check box…),
//! the Toggle, SelectionItem, Value and ExpandCollapse patterns, a
//! disabled state, a
//! container that is an element without a name, a dialog, and the app's
//! content out of the tree while a modal panel is open.
//!
//! WinUI asks an element for its peer through the overridable
//! `UIElement.OnCreateAutomationPeer`: overriding it means deriving from
//! the element's class, and a peer overrides `AutomationPeer`'s `…Core`
//! methods the same way. windows-rs `#[implement]` cannot derive from
//! (aggregate) a composable WinRT class, so, as `outer.rs` does for the
//! `Application`, the outer objects are written by hand: an identity
//! (`IInspectable`) and the interfaces they implement themselves, every
//! other interface answered by the inner object WinUI composes with them.
//!
//! - The element (`Button`, `Canvas`) overrides `IUIElementOverrides`:
//!   `OnCreateAutomationPeer` returns the custom peer; its other methods,
//!   like an element that needs no custom peer, call the base class's.
//! - The peer (`ButtonAutomationPeer`, `FrameworkElementAutomationPeer`)
//!   overrides nine of `IAutomationPeerOverrides`' 47 methods (the
//!   others call the base class's) and provides `IToggleProvider`,
//!   `ISelectionItemProvider`, `IValueProvider`, `IExpandCollapseProvider`
//!   (a disclosure) and, for a table and its
//!   cells (`table.rs`), `IGridProvider`, `ITableProvider`,
//!   `IGridItemProvider` and `ITableItemProvider`.
//!
//! Both read the element's [`Semantics`] when asked, from a table the
//! widgets keep up to date ([`set`]): they hold no XAML object.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::ptr::null_mut;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use prism_core::{NodeId, UiEvent};
use windows::Foundation::{Point, PropertyValue, Rect};
use windows_core::{
    GUID, HRESULT, HSTRING, IInspectable, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface,
    Result,
};

use crate::access::{Control, Semantics};
use crate::winui::h;
use crate::winui::xaml::Automation::Peers::{
    AutomationControlType, AutomationEvents, AutomationNotificationKind,
    AutomationNotificationProcessing, AutomationPeer, ButtonAutomationPeer,
    FrameworkElementAutomationPeer, IAutomationPeerOverrides, IButtonAutomationPeerFactory,
    IFrameworkElementAutomationPeerFactory, PatternInterface,
};
use crate::winui::xaml::Automation::Provider::{
    IExpandCollapseProvider, IGridItemProvider, IGridProvider, IInvokeProvider,
    IRawElementProviderSimple, ISelectionItemProvider, ITableItemProvider, ITableProvider,
    IToggleProvider, IValueProvider,
};
use crate::winui::xaml::Automation::{
    AutomationElementIdentifiers, ExpandCollapsePatternIdentifiers, ExpandCollapseState,
    RowOrColumnMajor, SelectionItemPatternIdentifiers, TogglePatternIdentifiers,
    ValuePatternIdentifiers,
};
use crate::winui::xaml::Controls::{Button, Canvas, IButtonFactory, ICanvasFactory};
use crate::winui::xaml::{FrameworkElement, IUIElementOverrides, UIElement};

/// Which element a peer stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A node's canvas.
    Node(NodeId),
    /// A clickable view's button (it stands for the node).
    Clickable(NodeId),
    /// The panel of a sheet or dialog shown over the window.
    Dialog(NodeId),
    /// The app's content (out of the tree while a modal panel is open).
    Content,
}

thread_local! {
    static TABLE: RefCell<HashMap<Key, Semantics>> = RefCell::default();
    /// A sheet or dialog is open: the app's content is out of reach.
    static MODAL: Cell<bool> = const { Cell::new(false) };
    /// WinUI's words for a toggle switch, in the user's language.
    static SWITCH_WORDS: RefCell<Option<HSTRING>> = const { RefCell::new(None) };
    /// WinUI's words for a tab (`TabViewItem`), in the user's language.
    static TAB_WORDS: RefCell<Option<HSTRING>> = const { RefCell::new(None) };
}

/// An element's semantics; returns the previous ones.
pub fn set(key: Key, s: Semantics) -> Option<Semantics> {
    if s.switch {
        switch_words();
    }
    TABLE.with(|t| t.borrow_mut().insert(key, s))
}

/// The element is gone.
pub fn forget(key: Key) {
    TABLE.with(|t| t.borrow_mut().remove(&key));
}

fn lookup(key: Key) -> Option<Semantics> {
    TABLE.with(|t| t.borrow().get(&key).cloned())
}

/// A sheet or dialog opened (`true`) or the last one closed: the app's
/// content leaves the tree (or comes back), so a reader stays inside.
pub fn set_modal(on: bool) {
    MODAL.set(on);
}

/// The localized control type WinUI's peer of `element` says, asked once.
fn words_of<E: Interface>(
    cell: &'static std::thread::LocalKey<RefCell<Option<HSTRING>>>,
    element: impl FnOnce() -> Result<E>,
) -> Option<HSTRING> {
    cell.with(|w| {
        let mut w = w.borrow_mut();
        if w.is_none() {
            let words = element().and_then(|e| {
                let e: UIElement = e.cast()?;
                FrameworkElementAutomationPeer::CreatePeerForElement(&e)?.GetLocalizedControlType()
            });
            *w = words.ok().filter(|s| !s.is_empty());
        }
        w.clone()
    })
}

/// « toggle switch » as WinUI says it for its `ToggleSwitch`.
fn switch_words() -> Option<HSTRING> {
    words_of(
        &SWITCH_WORDS,
        crate::winui::xaml::Controls::ToggleSwitch::new,
    )
}

/// « tab item » as WinUI says it for its `TabViewItem`.
pub fn tab_words() -> Option<HSTRING> {
    words_of(&TAB_WORDS, crate::winui::xaml::Controls::TabViewItem::new)
}

/// A clickable view's button: its peer is [`Key::Clickable`]'s.
pub fn button(id: NodeId) -> Result<Button> {
    thread_local! {
        static F: RefCell<Option<IButtonFactory>> = const { RefCell::new(None) };
    }
    let f = factory(&F, windows_core::factory::<Button, IButtonFactory>)?;
    compose(
        Kind::Button,
        Key::Clickable(id),
        |outer, inner, out| unsafe { (f.vtable().CreateInstance)(f.as_raw(), outer, inner, out) },
    )?
    .cast()
}

/// A canvas whose peer is `key`'s: a custom one while its semantics make
/// it an element (or for the app's content), else the base class's (none,
/// or a group's when it has a name).
pub fn canvas(key: Key) -> Result<Canvas> {
    thread_local! {
        static F: RefCell<Option<ICanvasFactory>> = const { RefCell::new(None) };
    }
    let f = factory(&F, windows_core::factory::<Canvas, ICanvasFactory>)?;
    compose(Kind::Canvas, key, |outer, inner, out| unsafe {
        (f.vtable().CreateInstance)(f.as_raw(), outer, inner, out)
    })?
    .cast()
}

fn factory<I: Interface + Clone>(
    cell: &'static std::thread::LocalKey<RefCell<Option<I>>>,
    make: impl FnOnce() -> Result<I>,
) -> Result<I> {
    cell.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(f) = &*c {
            return Ok(f.clone());
        }
        let f = make()?;
        *c = Some(f.clone());
        Ok(f)
    })
}

/// Tells listening clients (a screen reader) that the states of
/// `element` changed from `old` to `new`.
pub fn changed(element: &UIElement, old: Option<&Semantics>, new: &Semantics) -> Result<()> {
    let Some(old) = old else { return Ok(()) };
    // Only an element has a peer of its own to speak for it.
    if old == new
        || !new.element
        || !AutomationPeer::ListenerExists(AutomationEvents::PropertyChanged)?
    {
        return Ok(());
    }
    let peer = FrameworkElementAutomationPeer::FromElement(element)?;
    let state = |t: Option<bool>| PropertyValue::CreateInt32(i32::from(t == Some(true)));
    if old.toggled != new.toggled && new.toggled.is_some() {
        peer.RaisePropertyChangedEvent(
            &TogglePatternIdentifiers::ToggleStateProperty()?,
            &state(old.toggled)?,
            &state(new.toggled)?,
        )?;
    }
    if old.expanded != new.expanded
        && let Some(open) = new.expanded
    {
        let state = |e: bool| {
            PropertyValue::CreateInt32(if e {
                ExpandCollapseState::Expanded.0
            } else {
                ExpandCollapseState::Collapsed.0
            })
        };
        peer.RaisePropertyChangedEvent(
            &ExpandCollapsePatternIdentifiers::ExpandCollapseStateProperty()?,
            &state(old.expanded == Some(true))?,
            &state(open)?,
        )?;
    }
    if old.selected != new.selected && new.selected.is_some() {
        peer.RaisePropertyChangedEvent(
            &SelectionItemPatternIdentifiers::IsSelectedProperty()?,
            &PropertyValue::CreateBoolean(old.selected == Some(true))?,
            &PropertyValue::CreateBoolean(new.selected == Some(true))?,
        )?;
    }
    if old.value != new.value
        && let Some(v) = &new.value
    {
        peer.RaisePropertyChangedEvent(
            &ValuePatternIdentifiers::ValueProperty()?,
            &PropertyValue::CreateString(&h(old.value.as_deref().unwrap_or("")))?,
            &PropertyValue::CreateString(&h(v))?,
        )?;
    }
    if old.disabled != new.disabled {
        peer.RaisePropertyChangedEvent(
            &AutomationElementIdentifiers::IsEnabledProperty()?,
            &PropertyValue::CreateBoolean(!old.disabled)?,
            &PropertyValue::CreateBoolean(!new.disabled)?,
        )?;
    }
    Ok(())
}

/// Says `text` to a screen reader from `element`'s peer (a live region's
/// new value): queued after what it is saying, or in its place.
pub fn announce(element: &UIElement, text: &str, important: bool) -> Result<()> {
    let peer = FrameworkElementAutomationPeer::FromElement(element)?;
    peer.RaiseNotificationEvent(
        AutomationNotificationKind::Other,
        if important {
            AutomationNotificationProcessing::ImportantMostRecent
        } else {
            AutomationNotificationProcessing::MostRecent
        },
        &h(text),
        &h("prism.live"),
    )
}

// ---------------------------------------------------------------------------
// The outer objects
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Button,
    Canvas,
    ButtonPeer,
    CanvasPeer,
}

/// An outer object: its interfaces' vtables first (the identity, then up
/// to eight interfaces it implements), so that an interface pointer is the
/// address of its vtable pointer.
#[repr(C)]
struct Outer {
    vtables: [*const c_void; 10],
    refs: AtomicU32,
    /// The inner (non-delegating) object, the base class.
    inner: UnsafeCell<Option<IInspectable>>,
    kind: Kind,
    key: Key,
}

impl Outer {
    fn inner(&self) -> Option<&IInspectable> {
        // SAFETY: only `release` changes it, once nothing else uses it.
        unsafe { (*self.inner.get()).as_ref() }
    }

    fn identity(&self) -> *mut c_void {
        self as *const Outer as *mut c_void
    }

    /// The base class's implementation of interface `iid`.
    fn base(&self, iid: &GUID) -> Option<IUnknown> {
        let inner = self.inner()?;
        let mut p = null_mut();
        // SAFETY: a QueryInterface call on a live object.
        let hr = unsafe { (inner.vtable().base.QueryInterface)(inner.as_raw(), iid, &mut p) };
        // SAFETY: QueryInterface returned an owned reference.
        (hr.is_ok() && !p.is_null()).then(|| unsafe { IUnknown::from_raw(p) })
    }
}

/// The interface slot (0: the identity) of `iid` in an outer of `kind`.
fn slot_of(kind: Kind, iid: &GUID) -> Option<usize> {
    if *iid == IUnknown::IID || *iid == IInspectable::IID {
        return Some(0);
    }
    match kind {
        Kind::Button | Kind::Canvas => (*iid == IUIElementOverrides::IID).then_some(1),
        Kind::ButtonPeer | Kind::CanvasPeer => [
            IAutomationPeerOverrides::IID,
            IToggleProvider::IID,
            ISelectionItemProvider::IID,
            IValueProvider::IID,
            IGridProvider::IID,
            ITableProvider::IID,
            IGridItemProvider::IID,
            ITableItemProvider::IID,
            IExpandCollapseProvider::IID,
        ]
        .iter()
        .position(|i| i == iid)
        .map(|p| p + 1),
    }
}

const S_OK: HRESULT = HRESULT(0);
const E_FAIL: HRESULT = HRESULT(0x8000_4005_u32 as i32);
const E_POINTER: HRESULT = HRESULT(0x8000_4003_u32 as i32);
const E_NOINTERFACE: HRESULT = HRESULT(0x8000_4002_u32 as i32);
const E_ACCESSDENIED: HRESULT = HRESULT(0x8007_0005_u32 as i32);
const E_INVALIDARG: HRESULT = HRESULT(0x8007_0057_u32 as i32);

/// The outer object behind interface pointer `this`, its slot `K`.
unsafe fn outer<'a, const K: usize>(this: *mut c_void) -> &'a Outer {
    unsafe { &*((this as *mut *const c_void).sub(K) as *const Outer) }
}

unsafe extern "system" fn qi<const K: usize>(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        if out.is_null() || iid.is_null() {
            return E_POINTER;
        }
        let o = outer::<K>(this);
        match slot_of(o.kind, &*iid) {
            Some(slot) => {
                *out = &o.vtables[slot] as *const *const c_void as *mut c_void;
                o.refs.fetch_add(1, Ordering::Relaxed);
                S_OK
            }
            None => match o.inner() {
                Some(inner) => (inner.vtable().base.QueryInterface)(inner.as_raw(), iid, out),
                None => {
                    *out = null_mut();
                    E_NOINTERFACE
                }
            },
        }
    }
}

unsafe extern "system" fn add_ref<const K: usize>(this: *mut c_void) -> u32 {
    unsafe { outer::<K>(this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release<const K: usize>(this: *mut c_void) -> u32 {
    unsafe {
        let o = outer::<K>(this);
        let left = o.refs.fetch_sub(1, Ordering::Release) - 1;
        if left == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            // Never freed twice while the base class goes (it may call back).
            o.refs.store(1 << 30, Ordering::Relaxed);
            let p = o.identity() as *mut Outer;
            drop((*(*p).inner.get()).take());
            drop(Box::from_raw(p));
        }
        left
    }
}

unsafe extern "system" fn get_iids(
    _: *mut c_void,
    count: *mut u32,
    iids: *mut *mut GUID,
) -> HRESULT {
    unsafe {
        *count = 0;
        *iids = null_mut();
    }
    S_OK
}

/// The base class's name: XAML styles and templates it as the class it is.
unsafe extern "system" fn class_name<const K: usize>(
    this: *mut c_void,
    name: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        match outer::<K>(this).inner() {
            Some(inner) => (inner.vtable().GetRuntimeClassName)(inner.as_raw(), name),
            None => {
                *name = null_mut();
                S_OK
            }
        }
    }
}

unsafe extern "system" fn trust_level(_: *mut c_void, level: *mut i32) -> HRESULT {
    unsafe { *level = 0 };
    S_OK
}

fn inspectable<const K: usize>() -> IInspectable_Vtbl {
    IInspectable_Vtbl {
        base: IUnknown_Vtbl {
            QueryInterface: qi::<K>,
            AddRef: add_ref::<K>,
            Release: release::<K>,
        },
        GetIids: get_iids,
        GetRuntimeClassName: class_name::<K>,
        GetTrustLevel: trust_level,
    }
}

/// An interface's vtable: `IInspectable`, then `N` methods.
#[repr(C)]
struct Vtbl<const N: usize> {
    base: IInspectable_Vtbl,
    methods: [usize; N],
}

struct Tables {
    identity: IInspectable_Vtbl,
    element: Vtbl<8>,
    peer: Vtbl<47>,
    toggle: Vtbl<2>,
    selection: Vtbl<5>,
    value: Vtbl<3>,
    grid: Vtbl<3>,
    table: Vtbl<3>,
    grid_item: Vtbl<5>,
    table_item: Vtbl<2>,
    expand: Vtbl<3>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| Tables {
        identity: inspectable::<0>(),
        element: Vtbl {
            base: inspectable::<1>(),
            methods: [
                create_peer as *const () as usize,
                e0::<1> as *const () as usize,
                e_touch::<2> as *const () as usize,
                e1::<3> as *const () as usize,
                e1::<4> as *const () as usize,
                e1::<5> as *const () as usize,
                e1::<6> as *const () as usize,
                e2::<7> as *const () as usize,
            ],
        },
        peer: Vtbl {
            base: inspectable::<1>(),
            methods: [
                pattern as *const () as usize, // GetPatternCore
                p1::<1> as *const () as usize,
                p1::<2> as *const () as usize,
                control_type as *const () as usize, // GetAutomationControlTypeCore
                p1::<4> as *const () as usize,
                p1::<5> as *const () as usize,
                children as *const () as usize, // GetChildrenCore
                p_int::<7> as *const () as usize,
                p1::<8> as *const () as usize,
                p1::<9> as *const () as usize,
                p1::<10> as *const () as usize,
                p1::<11> as *const () as usize,
                p1::<12> as *const () as usize,
                p1::<13> as *const () as usize,
                localized_type as *const () as usize, // GetLocalizedControlTypeCore
                p1::<15> as *const () as usize,
                p1::<16> as *const () as usize,
                p1::<17> as *const () as usize,
                content_element as *const () as usize, // IsContentElementCore
                control_element as *const () as usize, // IsControlElementCore
                enabled as *const () as usize,         // IsEnabledCore
                focusable as *const () as usize,       // IsKeyboardFocusableCore
                p1::<22> as *const () as usize,
                p1::<23> as *const () as usize,
                p1::<24> as *const () as usize,
                p0::<25> as *const () as usize,
                p_point::<26> as *const () as usize,
                p_point::<27> as *const () as usize,
                p1::<28> as *const () as usize,
                p1::<29> as *const () as usize,
                p0::<30> as *const () as usize,
                p1::<31> as *const () as usize,
                p1::<32> as *const () as usize,
                p1::<33> as *const () as usize,
                p1::<34> as *const () as usize,
                p1::<35> as *const () as usize,
                p1::<36> as *const () as usize,
                p1::<37> as *const () as usize,
                p1::<38> as *const () as usize,
                p1::<39> as *const () as usize,
                p1::<40> as *const () as usize,
                p1::<41> as *const () as usize,
                p1::<42> as *const () as usize,
                p1::<43> as *const () as usize,
                p1::<44> as *const () as usize,
                p1::<45> as *const () as usize,
                is_dialog as *const () as usize, // IsDialogCore
            ],
        },
        toggle: Vtbl {
            base: inspectable::<2>(),
            methods: [
                toggle_state as *const () as usize,
                toggle as *const () as usize,
            ],
        },
        selection: Vtbl {
            base: inspectable::<3>(),
            methods: [
                is_selected as *const () as usize,
                selection_container as *const () as usize,
                select as *const () as usize, // AddToSelection
                unselect as *const () as usize,
                select as *const () as usize,
            ],
        },
        value: Vtbl {
            base: inspectable::<4>(),
            methods: [
                read_only as *const () as usize,
                value as *const () as usize,
                set_value as *const () as usize,
            ],
        },
        grid: Vtbl {
            base: inspectable::<5>(),
            methods: [
                column_count as *const () as usize,
                row_count as *const () as usize,
                grid_item as *const () as usize, // GetItem
            ],
        },
        table: Vtbl {
            base: inspectable::<6>(),
            methods: [
                row_major as *const () as usize,
                column_headers as *const () as usize,
                no_headers as *const () as usize, // GetRowHeaders
            ],
        },
        grid_item: Vtbl {
            base: inspectable::<7>(),
            methods: [
                item_column as *const () as usize,
                span as *const () as usize, // ColumnSpan
                containing_grid as *const () as usize,
                item_row as *const () as usize,
                span as *const () as usize, // RowSpan
            ],
        },
        table_item: Vtbl {
            base: inspectable::<8>(),
            methods: [
                column_header_items as *const () as usize,
                no_headers as *const () as usize, // GetRowHeaderItems
            ],
        },
        expand: Vtbl {
            base: inspectable::<9>(),
            methods: [
                expand_state as *const () as usize,
                collapse as *const () as usize,
                expand as *const () as usize,
            ],
        },
    })
}

/// Composes a new object of `kind` (the outer) with its base class,
/// created by `create(outer, &mut inner, &mut instance)`; returns the
/// instance.
fn compose(
    kind: Kind,
    key: Key,
    create: impl FnOnce(*mut c_void, *mut *mut c_void, *mut *mut c_void) -> HRESULT,
) -> Result<IInspectable> {
    let t = tables();
    let vt = |v: &'static IInspectable_Vtbl| v as *const IInspectable_Vtbl as *const c_void;
    let vtables = match kind {
        Kind::Button | Kind::Canvas => {
            let mut v = [core::ptr::null(); 10];
            v[0] = vt(&t.identity);
            v[1] = vt(&t.element.base);
            v
        }
        Kind::ButtonPeer | Kind::CanvasPeer => [
            vt(&t.identity),
            vt(&t.peer.base),
            vt(&t.toggle.base),
            vt(&t.selection.base),
            vt(&t.value.base),
            vt(&t.grid.base),
            vt(&t.table.base),
            vt(&t.grid_item.base),
            vt(&t.table_item.base),
            vt(&t.expand.base),
        ],
    };
    let o = Box::into_raw(Box::new(Outer {
        vtables,
        refs: AtomicU32::new(1),
        inner: UnsafeCell::new(None),
        kind,
        key,
    }));
    let mut inner = null_mut();
    let mut instance = null_mut();
    let hr = create(o as *mut c_void, &mut inner, &mut instance);
    // SAFETY: `o` is alive (our reference), the pointers are owned
    // references or null.
    unsafe {
        if !inner.is_null() {
            *(*o).inner.get() = Some(IInspectable::from_raw(inner));
        }
        let instance = (!instance.is_null()).then(|| IInspectable::from_raw(instance));
        // The instance holds the outer from now on.
        release::<0>(o as *mut c_void);
        match instance {
            Some(i) if hr.is_ok() => Ok(i),
            _ => Err(if hr.is_err() { hr } else { E_FAIL }.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Methods that call the base class's
// ---------------------------------------------------------------------------

/// The address of method `slot` (after `IInspectable`'s six) of `i`.
unsafe fn method(i: &IUnknown, slot: usize) -> usize {
    unsafe { *(*(i.as_raw() as *const *const usize)).add(6 + slot) }
}

macro_rules! base_methods {
    ($iface:ty; $($name:ident($($a:ident: $t:ty),*);)*) => {$(
        /// Calls the base class's method `S` of the interface.
        unsafe extern "system" fn $name<const S: usize>(this: *mut c_void, $($a: $t),*) -> HRESULT {
            unsafe {
                let Some(b) = outer::<1>(this).base(&<$iface as Interface>::IID) else {
                    return E_FAIL;
                };
                let f: unsafe extern "system" fn(*mut c_void, $($t),*) -> HRESULT =
                    core::mem::transmute(method(&b, S));
                f(b.as_raw(), $($a),*)
            }
        }
    )*};
}

base_methods!(IUIElementOverrides;
    e0();
    e1(a: *mut c_void);
    e2(a: *mut c_void, b: *mut c_void);
    e_touch(a: Point, b: Rect, c: *mut c_void);
);

base_methods!(IAutomationPeerOverrides;
    p0();
    p1(a: *mut c_void);
    p_int(a: i32, b: *mut c_void);
    p_point(a: Point, b: *mut c_void);
);

// ---------------------------------------------------------------------------
// The element: `OnCreateAutomationPeer`
// ---------------------------------------------------------------------------

unsafe extern "system" fn create_peer(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        let custom = match o.kind {
            Kind::Button => true,
            _ => {
                o.key == Key::Content
                    || lookup(o.key).is_some_and(|s| s.element || s.dialog || s.hidden)
            }
        };
        if custom {
            match new_peer(o) {
                Ok(p) => {
                    *out = p.into_raw();
                    return S_OK;
                }
                Err(e) => crate::win::log!("automation peer of {:?}: {e}", o.key),
            }
        }
        e1::<0>(this, out as *mut c_void)
    }
}

fn new_peer(o: &Outer) -> Result<AutomationPeer> {
    let identity = o.identity();
    // SAFETY: our own identity, alive while WinUI asks for its peer.
    let me = unsafe { IInspectable::from_raw_borrowed(&identity) }.ok_or(E_POINTER)?;
    let peer = if o.kind == Kind::Button {
        thread_local! {
            static F: RefCell<Option<IButtonAutomationPeerFactory>> = const { RefCell::new(None) };
        }
        let f = factory(&F, || {
            windows_core::factory::<ButtonAutomationPeer, IButtonAutomationPeerFactory>()
        })?;
        let owner: Button = me.cast()?;
        compose(Kind::ButtonPeer, o.key, |outer, inner, out| unsafe {
            (f.vtable().CreateInstanceWithOwner)(f.as_raw(), owner.as_raw(), outer, inner, out)
        })?
    } else {
        thread_local! {
            static F: RefCell<Option<IFrameworkElementAutomationPeerFactory>> =
                const { RefCell::new(None) };
        }
        let f = factory(&F, || {
            windows_core::factory::<
                FrameworkElementAutomationPeer,
                IFrameworkElementAutomationPeerFactory,
            >()
        })?;
        let owner: FrameworkElement = me.cast()?;
        compose(Kind::CanvasPeer, o.key, |outer, inner, out| unsafe {
            (f.vtable().CreateInstanceWithOwner)(f.as_raw(), owner.as_raw(), outer, inner, out)
        })?
    };
    peer.cast()
}

// ---------------------------------------------------------------------------
// The peer: `IAutomationPeerOverrides`
// ---------------------------------------------------------------------------

/// Returns our own identity as a pattern provider.
unsafe fn provide(o: &Outer, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        add_ref::<0>(o.identity());
        *out = o.identity();
    }
    S_OK
}

unsafe extern "system" fn pattern(this: *mut c_void, which: i32, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if let Some(s) = lookup(o.key) {
            let which = PatternInterface(which);
            let id = node(o.key);
            let grid = id
                .and_then(|t| crate::table::with_grid(t, |_| ()))
                .is_some();
            let item = id.and_then(crate::table::place).is_some();
            let mine = (which == PatternInterface::Toggle && s.toggled.is_some())
                || (which == PatternInterface::SelectionItem && s.selected.is_some())
                || (which == PatternInterface::Value && s.value.is_some())
                || (which == PatternInterface::ExpandCollapse && s.expanded.is_some())
                || (matches!(which, PatternInterface::Grid | PatternInterface::Table) && grid)
                || (matches!(
                    which,
                    PatternInterface::GridItem | PatternInterface::TableItem
                ) && item);
            if mine {
                return provide(o, out);
            }
            if which == PatternInterface::Invoke && o.kind == Kind::ButtonPeer && !s.invoke {
                *out = null_mut();
                return S_OK;
            }
        }
        p_int::<0>(this, which, out as *mut c_void)
    }
}

fn control(c: Control) -> AutomationControlType {
    match c {
        Control::Button => AutomationControlType::Button,
        Control::CheckBox => AutomationControlType::CheckBox,
        Control::ComboBox => AutomationControlType::ComboBox,
        Control::DataItem => AutomationControlType::DataItem,
        Control::Group => AutomationControlType::Group,
        Control::HeaderItem => AutomationControlType::HeaderItem,
        Control::Hyperlink => AutomationControlType::Hyperlink,
        Control::Image => AutomationControlType::Image,
        Control::List => AutomationControlType::List,
        Control::ListItem => AutomationControlType::ListItem,
        Control::ProgressBar => AutomationControlType::ProgressBar,
        Control::RadioButton => AutomationControlType::RadioButton,
        Control::Slider => AutomationControlType::Slider,
        Control::Tab => AutomationControlType::Tab,
        Control::TabItem => AutomationControlType::TabItem,
        Control::Table => AutomationControlType::Table,
        Control::Text => AutomationControlType::Text,
        Control::Window => AutomationControlType::Window,
    }
}

unsafe extern "system" fn control_type(this: *mut c_void, out: *mut i32) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if let Some(c) = lookup(o.key).and_then(|s| s.control) {
            *out = control(c).0;
            return S_OK;
        }
        p1::<3>(this, out as *mut c_void)
    }
}

unsafe extern "system" fn localized_type(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if lookup(o.key).is_some_and(|s| s.switch)
            && let Some(words) = SWITCH_WORDS.with(|w| w.borrow().clone())
        {
            *out = core::mem::transmute::<HSTRING, *mut c_void>(words);
            return S_OK;
        }
        p1::<14>(this, out as *mut c_void)
    }
}

/// The app's content has no children while a modal panel is open, a
/// hidden view none ever.
unsafe extern "system" fn children(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if (o.key == Key::Content && MODAL.get()) || lookup(o.key).is_some_and(|s| s.hidden) {
            *out = null_mut();
            return S_OK;
        }
        p1::<6>(this, out as *mut c_void)
    }
}

/// A container is in the control and content views when it is an
/// element; the app's content never is (its children are).
unsafe fn element_view(this: *mut c_void, out: *mut bool, slot: usize) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if o.kind == Kind::CanvasPeer {
            *out = o.key != Key::Content && lookup(o.key).is_some_and(|s| s.element || s.dialog);
            return S_OK;
        }
        if slot == 18 {
            p1::<18>(this, out as *mut c_void)
        } else {
            p1::<19>(this, out as *mut c_void)
        }
    }
}

unsafe extern "system" fn content_element(this: *mut c_void, out: *mut bool) -> HRESULT {
    unsafe { element_view(this, out, 18) }
}

unsafe extern "system" fn control_element(this: *mut c_void, out: *mut bool) -> HRESULT {
    unsafe { element_view(this, out, 19) }
}

unsafe extern "system" fn enabled(this: *mut c_void, out: *mut bool) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if lookup(o.key).is_some_and(|s| s.disabled) {
            *out = false;
            return S_OK;
        }
        if o.kind == Kind::CanvasPeer {
            *out = true;
            return S_OK;
        }
        p1::<20>(this, out as *mut c_void)
    }
}

/// A view listening to keys takes the keyboard focus (`keyview.rs`).
unsafe extern "system" fn focusable(this: *mut c_void, out: *mut bool) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if lookup(o.key).is_some_and(|s| s.focusable && !s.disabled) {
            *out = true;
            return S_OK;
        }
        p1::<21>(this, out as *mut c_void)
    }
}

unsafe extern "system" fn is_dialog(this: *mut c_void, out: *mut bool) -> HRESULT {
    unsafe {
        let o = outer::<1>(this);
        if lookup(o.key).is_some_and(|s| s.dialog) {
            *out = true;
            return S_OK;
        }
        p1::<46>(this, out as *mut c_void)
    }
}

// ---------------------------------------------------------------------------
// The peer's patterns
// ---------------------------------------------------------------------------

/// Toggles or selects the element: a click, as a screen reader's Invoke.
fn activate(o: &Outer) -> HRESULT {
    if o.kind == Kind::ButtonPeer {
        let invoke = o
            .base(&IInvokeProvider::IID)
            .and_then(|b| b.cast::<IInvokeProvider>().ok());
        return match invoke.map(|i| i.Invoke()) {
            Some(Ok(())) => S_OK,
            Some(Err(e)) => e.code(),
            None => E_FAIL,
        };
    }
    match o.key {
        Key::Node(id) | Key::Clickable(id) => {
            crate::imp::dispatch(UiEvent::Click(id));
            S_OK
        }
        _ => E_FAIL,
    }
}

unsafe extern "system" fn toggle_state(this: *mut c_void, out: *mut i32) -> HRESULT {
    unsafe {
        let on = lookup(outer::<2>(this).key).and_then(|s| s.toggled) == Some(true);
        *out = i32::from(on);
    }
    S_OK
}

unsafe extern "system" fn toggle(this: *mut c_void) -> HRESULT {
    unsafe { activate(outer::<2>(this)) }
}

unsafe extern "system" fn expand_state(this: *mut c_void, out: *mut i32) -> HRESULT {
    unsafe {
        *out = match lookup(outer::<9>(this).key).and_then(|s| s.expanded) {
            Some(true) => ExpandCollapseState::Expanded.0,
            _ => ExpandCollapseState::Collapsed.0,
        };
    }
    S_OK
}

/// Expands or collapses the region: a click, unless it already is.
fn expand_to(this: *mut c_void, open: bool) -> HRESULT {
    unsafe {
        let o = outer::<9>(this);
        if lookup(o.key).and_then(|s| s.expanded) == Some(open) {
            return S_OK;
        }
        activate(o)
    }
}

unsafe extern "system" fn collapse(this: *mut c_void) -> HRESULT {
    expand_to(this, false)
}

unsafe extern "system" fn expand(this: *mut c_void) -> HRESULT {
    expand_to(this, true)
}

unsafe extern "system" fn is_selected(this: *mut c_void, out: *mut bool) -> HRESULT {
    unsafe { *out = lookup(outer::<3>(this).key).and_then(|s| s.selected) == Some(true) };
    S_OK
}

unsafe extern "system" fn selection_container(_: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    unsafe { *out = null_mut() };
    S_OK
}

unsafe extern "system" fn select(this: *mut c_void) -> HRESULT {
    unsafe {
        let o = outer::<3>(this);
        if lookup(o.key).and_then(|s| s.selected) == Some(true) {
            return S_OK;
        }
        activate(o)
    }
}

/// An item cannot be unselected by itself (a radio, a tab).
unsafe extern "system" fn unselect(_: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn read_only(_: *mut c_void, out: *mut bool) -> HRESULT {
    unsafe { *out = true };
    S_OK
}

unsafe extern "system" fn value(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let v = lookup(outer::<4>(this).key)
            .and_then(|s| s.value)
            .unwrap_or_default();
        *out = core::mem::transmute::<HSTRING, *mut c_void>(h(&v));
    }
    S_OK
}

unsafe extern "system" fn set_value(_: *mut c_void, _: *mut c_void) -> HRESULT {
    E_ACCESSDENIED
}

// ---------------------------------------------------------------------------
// Tables (#284): the table's Grid and Table patterns, its cells' GridItem
// and TableItem (see `table.rs`)
// ---------------------------------------------------------------------------

/// The node an element's peer stands for.
fn node(key: Key) -> Option<NodeId> {
    match key {
        Key::Node(id) | Key::Clickable(id) => Some(id),
        _ => None,
    }
}

/// The provider a client gets for node `id`'s element: its peer's, from
/// peer `o` (`ProviderFromPeer` is a peer's own method).
fn provider_of(o: &Outer, id: NodeId) -> Option<IRawElementProviderSimple> {
    let element = crate::table::element(id)?;
    let peer = FrameworkElementAutomationPeer::CreatePeerForElement(&element).ok()?;
    let identity = o.identity();
    // SAFETY: our own identity, alive while a client calls us.
    let me: AutomationPeer = unsafe { IInspectable::from_raw_borrowed(&identity) }?
        .cast()
        .ok()?;
    me.ProviderFromPeer(&peer).ok()
}

/// Hands a client the providers of `ids`' elements (a received array).
unsafe fn providers(
    o: &Outer,
    ids: &[NodeId],
    len: *mut u32,
    out: *mut *mut *mut c_void,
) -> HRESULT {
    let found: Vec<Option<IRawElementProviderSimple>> = ids
        .iter()
        .filter_map(|id| provider_of(o, *id))
        .map(Some)
        .collect();
    let (data, n) = windows_core::Array::<IRawElementProviderSimple>::from_slice(&found).into_abi();
    unsafe {
        *out = data;
        *len = n;
    }
    S_OK
}

/// What `f` says of the table a Grid or Table provider stands for.
fn of_grid<R>(o: &Outer, f: impl FnOnce(&crate::table::Grid) -> R) -> Option<R> {
    crate::table::with_grid(node(o.key)?, f)
}

unsafe extern "system" fn column_count(this: *mut c_void, out: *mut i32) -> HRESULT {
    let n = of_grid(unsafe { outer::<5>(this) }, |g| g.column_count());
    unsafe { *out = n.unwrap_or(0) as i32 };
    S_OK
}

unsafe extern "system" fn row_count(this: *mut c_void, out: *mut i32) -> HRESULT {
    let n = of_grid(unsafe { outer::<5>(this) }, |g| g.row_count());
    unsafe { *out = n.unwrap_or(0) as i32 };
    S_OK
}

unsafe extern "system" fn grid_item(
    this: *mut c_void,
    row: i32,
    column: i32,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        *out = null_mut();
        let o = outer::<5>(this);
        let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) else {
            return E_INVALIDARG;
        };
        let Some(cell) = of_grid(o, |g| g.at(row, column)) else {
            return E_FAIL;
        };
        match cell {
            Some(cell) => {
                if let Some(p) = provider_of(o, cell) {
                    *out = p.into_raw();
                }
                S_OK
            }
            None => E_INVALIDARG,
        }
    }
}

unsafe extern "system" fn row_major(_: *mut c_void, out: *mut i32) -> HRESULT {
    unsafe { *out = RowOrColumnMajor::RowMajor.0 };
    S_OK
}

unsafe extern "system" fn column_headers(
    this: *mut c_void,
    len: *mut u32,
    out: *mut *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let o = outer::<6>(this);
        let ids = of_grid(o, |g| g.column_headers()).unwrap_or_default();
        providers(o, &ids, len, out)
    }
}

/// No row headers (a table's, a cell's): a table has none.
unsafe extern "system" fn no_headers(
    _: *mut c_void,
    len: *mut u32,
    out: *mut *mut *mut c_void,
) -> HRESULT {
    unsafe {
        *len = 0;
        *out = null_mut();
    }
    S_OK
}

/// Where the cell a GridItem or TableItem provider stands for is.
fn place_of(o: &Outer) -> Option<crate::table::Place> {
    crate::table::place(node(o.key)?)
}

unsafe extern "system" fn item_column(this: *mut c_void, out: *mut i32) -> HRESULT {
    let at = place_of(unsafe { outer::<7>(this) });
    unsafe { *out = at.map_or(0, |p| p.column as i32) };
    S_OK
}

unsafe extern "system" fn item_row(this: *mut c_void, out: *mut i32) -> HRESULT {
    let at = place_of(unsafe { outer::<7>(this) });
    unsafe { *out = at.map_or(0, |p| p.row as i32) };
    S_OK
}

/// A cell spans one row and one column.
unsafe extern "system" fn span(_: *mut c_void, out: *mut i32) -> HRESULT {
    unsafe { *out = 1 };
    S_OK
}

unsafe extern "system" fn containing_grid(this: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let o = outer::<7>(this);
        *out = place_of(o)
            .and_then(|p| provider_of(o, p.table))
            .map_or(null_mut(), Interface::into_raw);
    }
    S_OK
}

unsafe extern "system" fn column_header_items(
    this: *mut c_void,
    len: *mut u32,
    out: *mut *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let o = outer::<8>(this);
        let ids = place_of(o)
            .and_then(|p| crate::table::with_grid(p.table, |g| g.headers_of(p.row, p.column)))
            .unwrap_or_default();
        providers(o, &ids, len, out)
    }
}
