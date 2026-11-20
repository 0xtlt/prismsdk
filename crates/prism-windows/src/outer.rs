//! The app's `Microsoft.UI.Xaml.Application`, composed (COM aggregation)
//! with an outer object that answers `IXamlMetadataProvider` by forwarding
//! to `XamlControlsXamlMetaDataProvider`.
//!
//! Without it, merging `XamlControlsResources` (the Windows 11 / Fluent
//! control styles) fails: the XAML parser asks `Application.Current` for the
//! types of the controls library. C++/C# apps get this from the XAML
//! compiler (`App.xaml` + generated `XamlTypeInfo`); windows-rs `#[implement]`
//! cannot aggregate a composable class, so the outer object is written by
//! hand: QueryInterface answers IUnknown, IInspectable and
//! IXamlMetadataProvider itself and delegates everything else to the inner
//! (non-delegating) Application.
use core::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_core::{
    GUID, HRESULT, IInspectable, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface, Result,
};

use crate::winui::xaml::{
    Application, IApplicationFactory,
    Markup::{IXamlMetadataProvider, IXamlMetadataProvider_Vtbl, XmlnsDefinition},
    XamlTypeInfo::XamlControlsXamlMetaDataProvider,
};

#[repr(C)]
struct Outer {
    inspectable: *const IInspectable_Vtbl,
    provider: *const IXamlMetadataProvider_Vtbl,
    refs: AtomicU32,
    inner: Option<IInspectable>,
    controls: IXamlMetadataProvider,
}

const E_NOINTERFACE: HRESULT = HRESULT(0x8000_4002_u32 as i32);

static INSPECTABLE_VTBL: IInspectable_Vtbl = IInspectable_Vtbl {
    base: IUnknown_Vtbl {
        QueryInterface: qi,
        AddRef: add_ref,
        Release: release,
    },
    GetIids: get_iids,
    GetRuntimeClassName: class_name,
    GetTrustLevel: trust_level,
};

static PROVIDER_VTBL: IXamlMetadataProvider_Vtbl = IXamlMetadataProvider_Vtbl {
    base__: IInspectable_Vtbl {
        base: IUnknown_Vtbl {
            QueryInterface: p_qi,
            AddRef: p_add_ref,
            Release: p_release,
        },
        GetIids: p_get_iids,
        GetRuntimeClassName: p_class_name,
        GetTrustLevel: p_trust_level,
    },
    GetXamlType: get_xaml_type,
    GetXamlTypeByFullName: get_xaml_type_by_name,
    GetXmlnsDefinitions: get_xmlns,
};

unsafe fn outer<'a>(this: *mut c_void) -> &'a Outer {
    unsafe { &*(this as *const Outer) }
}
unsafe fn from_provider(this: *mut c_void) -> *mut c_void {
    unsafe { (this as *mut *const c_void).sub(1) as *mut c_void }
}

unsafe extern "system" fn qi(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let o = outer(this);
        let iid = &*iid;
        if *iid == IUnknown::IID || *iid == IInspectable::IID {
            *out = this;
        } else if *iid == IXamlMetadataProvider::IID {
            *out = &o.provider as *const _ as *mut c_void;
        } else if let Some(inner) = &o.inner {
            return (inner.vtable().base.QueryInterface)(inner.as_raw(), iid, out);
        } else {
            *out = core::ptr::null_mut();
            return E_NOINTERFACE;
        }
        o.refs.fetch_add(1, Ordering::Relaxed);
        HRESULT(0)
    }
}
unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe { outer(this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}
unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    unsafe {
        let left = outer(this).refs.fetch_sub(1, Ordering::Release) - 1;
        if left == 0 {
            drop(Box::from_raw(this as *mut Outer));
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
    let h = windows_core::HSTRING::from("Prism.App");
    unsafe { *name = core::mem::transmute::<windows_core::HSTRING, *mut c_void>(h) };
    HRESULT(0)
}
unsafe extern "system" fn trust_level(_: *mut c_void, level: *mut i32) -> HRESULT {
    unsafe { *level = 0 };
    HRESULT(0)
}

unsafe extern "system" fn p_qi(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe { qi(from_provider(this), iid, out) }
}
unsafe extern "system" fn p_add_ref(this: *mut c_void) -> u32 {
    unsafe { add_ref(from_provider(this)) }
}
unsafe extern "system" fn p_release(this: *mut c_void) -> u32 {
    unsafe { release(from_provider(this)) }
}
unsafe extern "system" fn p_get_iids(this: *mut c_void, c: *mut u32, i: *mut *mut GUID) -> HRESULT {
    unsafe { get_iids(from_provider(this), c, i) }
}
unsafe extern "system" fn p_class_name(this: *mut c_void, n: *mut *mut c_void) -> HRESULT {
    unsafe { class_name(from_provider(this), n) }
}
unsafe extern "system" fn p_trust_level(this: *mut c_void, l: *mut i32) -> HRESULT {
    unsafe { trust_level(from_provider(this), l) }
}

unsafe extern "system" fn get_xaml_type(
    this: *mut c_void,
    ty: core::mem::MaybeUninit<crate::winui::bindings::Windows::UI::Xaml::Interop::TypeName>,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let c = &outer(from_provider(this)).controls;
        (c.vtable().GetXamlType)(c.as_raw(), ty, out)
    }
}
unsafe extern "system" fn get_xaml_type_by_name(
    this: *mut c_void,
    name: *mut c_void,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe {
        let c = &outer(from_provider(this)).controls;
        (c.vtable().GetXamlTypeByFullName)(c.as_raw(), name, out)
    }
}
unsafe extern "system" fn get_xmlns(
    this: *mut c_void,
    len: *mut u32,
    out: *mut *mut core::mem::MaybeUninit<XmlnsDefinition>,
) -> HRESULT {
    unsafe {
        let c = &outer(from_provider(this)).controls;
        (c.vtable().GetXmlnsDefinitions)(c.as_raw(), len, out)
    }
}

/// Creates the application object (once per process; it lives until exit).
pub fn create_app() -> Result<Application> {
    let controls: IXamlMetadataProvider = XamlControlsXamlMetaDataProvider::new()?.cast()?;
    let outer = Box::into_raw(Box::new(Outer {
        inspectable: &INSPECTABLE_VTBL,
        provider: &PROVIDER_VTBL,
        refs: AtomicU32::new(1),
        inner: None,
        controls,
    }));
    let factory: IApplicationFactory = windows_core::factory::<Application, IApplicationFactory>()?;
    let mut inner = core::ptr::null_mut();
    let mut app = core::ptr::null_mut();
    unsafe {
        (factory.vtable().CreateInstance)(
            factory.as_raw(),
            outer as *mut c_void,
            &mut inner,
            &mut app,
        )
        .ok()?;
        (*outer).inner = Some(IInspectable::from_raw(inner));
        // Our own reference (refs = 1) is never released: the application
        // lives as long as the process.
        Ok(Application::from_raw(app))
    }
}
