//! A delegate boxed for `UIElement::AddHandler`: how a control's own
//! handled events (a text box's presses) still reach Prism
//! (`handledEventsToo`).
//!
//! XAML unboxes the handler with a single `QueryInterface` for
//! `IReference<TDelegate>`, then `get_Value` (`IValueBoxer::UnboxValue`
//! in microsoft-ui-xaml), as C++/WinRT's `box_value` makes it. That
//! interface's id is built from the delegate's WinRT signature,
//! `delegate({guid})`; windows-rs signs a delegate as its bare `{guid}`,
//! so its own `IReference<TDelegate>` has another id that XAML refuses.
//! The box is therefore written by hand, like `outer.rs`: it answers
//! `IUnknown`, `IInspectable`, `IAgileObject` and the right
//! `IReference<TDelegate>`, whose `get_Value` hands out the delegate.

use core::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::System::Com::IAgileObject;
use windows_core::{
    GUID, HRESULT, IInspectable, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface,
};

const E_NOINTERFACE: HRESULT = HRESULT(0x8000_4002_u32 as i32);

/// `IReference<T>`'s vtable: `IInspectable`'s, then `get_Value`.
#[repr(C)]
struct ReferenceVtbl {
    base: IInspectable_Vtbl,
    value: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> HRESULT,
}

#[repr(C)]
struct Boxed {
    vtable: *const ReferenceVtbl,
    refs: AtomicU32,
    /// The delegate, its interface's id, and `IReference<delegate>`'s.
    inner: IUnknown,
    iid: GUID,
    reference: GUID,
}

static VTABLE: ReferenceVtbl = ReferenceVtbl {
    base: IInspectable_Vtbl {
        base: IUnknown_Vtbl {
            QueryInterface: qi,
            AddRef: add_ref,
            Release: release,
        },
        GetIids: get_iids,
        GetRuntimeClassName: class_name,
        GetTrustLevel: trust_level,
    },
    value: get_value,
};

/// The id of `IReference<D>` for the delegate whose interface id is
/// `delegate`, as WinRT (MIDL, C++/WinRT) computes it.
pub fn reference_iid(delegate: &GUID) -> GUID {
    let signature = format!(
        "pinterface({{61c17706-2d65-11e0-9ae8-d48564015472}};delegate({{{}}}))",
        format!("{delegate:?}").to_ascii_lowercase()
    );
    GUID::from_signature(windows_core::imp::ConstBuffer::from_slice(
        signature.as_bytes(),
    ))
}

/// `delegate` as the `IInspectable` handler XAML takes.
pub fn boxed<T: Interface>(delegate: T) -> IInspectable {
    let b = Box::new(Boxed {
        vtable: &VTABLE,
        refs: AtomicU32::new(1),
        inner: delegate
            .cast::<IUnknown>()
            .expect("a delegate is an IUnknown"),
        iid: T::IID,
        reference: reference_iid(&T::IID),
    });
    // SAFETY: a COM object laid out with its vtable first, one reference.
    unsafe { IInspectable::from_raw(Box::into_raw(b) as *mut c_void) }
}

unsafe fn this<'a>(p: *mut c_void) -> &'a Boxed {
    unsafe { &*(p as *const Boxed) }
}

unsafe extern "system" fn qi(p: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let b = this(p);
        let iid = &*iid;
        if *iid == IUnknown::IID
            || *iid == IInspectable::IID
            || *iid == IAgileObject::IID
            || *iid == b.reference
        {
            b.refs.fetch_add(1, Ordering::Relaxed);
            *out = p;
            return HRESULT(0);
        }
        *out = core::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn get_value(p: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let b = this(p);
        b.inner.query(&b.iid, out)
    }
}

unsafe extern "system" fn add_ref(p: *mut c_void) -> u32 {
    unsafe { this(p).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release(p: *mut c_void) -> u32 {
    unsafe {
        let left = this(p).refs.fetch_sub(1, Ordering::Release) - 1;
        if left == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(p as *mut Boxed));
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
        *iids = core::ptr::null_mut();
    }
    HRESULT(0)
}

unsafe extern "system" fn class_name(_: *mut c_void, name: *mut *mut c_void) -> HRESULT {
    unsafe { *name = core::ptr::null_mut() };
    HRESULT(0)
}

unsafe extern "system" fn trust_level(_: *mut c_void, level: *mut i32) -> HRESULT {
    unsafe { *level = 0 };
    HRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winui::xaml::Input::PointerEventHandler;
    use windows::Foundation::IReference;

    #[test]
    fn the_box_answers_winrts_reference_iid() {
        // The hashing: IReference<Int32>'s published id.
        let i4 = GUID::from_signature(windows_core::imp::ConstBuffer::from_slice(
            b"pinterface({61c17706-2d65-11e0-9ae8-d48564015472};i4)",
        ));
        assert_eq!(i4, GUID::from_u128(0x548cefbd_bc8a_5fa0_8df2_957440fc8bf4));
        // A delegate's is signed `delegate({guid})`: not windows-rs's own.
        let ours = reference_iid(&PointerEventHandler::IID);
        assert_ne!(ours, IReference::<PointerEventHandler>::IID);
    }
}
