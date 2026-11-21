//! Passkeys of the app's own relying party (#94): Windows' WebAuthn API,
//! `webauthn.dll` (Windows 10 1903 and later), the one browsers use. It
//! shows Windows' own dialog (Windows Hello, a security key, a phone),
//! owned by the app's window, and needs no package identity. The options
//! and answers are WebAuthn JSON (`passkey.rs`).
//!
//! The DLL is loaded when first used (from System32 only): Windows 10
//! 1809, which Prism still starts on, has none, and then the app has no
//! passkeys (`capabilities().passkeys` is false). Its API version picks the
//! structures' versions, so a newer Windows' fields are never sent to an
//! older one.
//!
//! A ceremony blocks its own thread until the dialog closes; a new one
//! cancels the one still open (it answers "cancelled"), as quitting does.
//! Conditional mediation (passkeys in a field's suggestions, #203) has no
//! public API for desktop apps: `webauthn_autofill` answers "unavailable",
//! as on macOS and Android.

use std::sync::OnceLock;
use std::time::Duration;

use prism_core::{DeviceError, DeviceValue};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND, NTE_USER_CANCELLED};
use windows::Win32::Networking::WindowsWebServices::*;
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{BOOL, GUID, HRESULT, PCSTR, PCWSTR, s, w};

use crate::passkey::{self, Descriptor, Resident};
use crate::win::{log, wide};

// The pure module's numbers are the SDK's.
const _: () = {
    assert!(passkey::UV_REQUIRED == WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED);
    assert!(passkey::UV_PREFERRED == WEBAUTHN_USER_VERIFICATION_REQUIREMENT_PREFERRED);
    assert!(passkey::UV_DISCOURAGED == WEBAUTHN_USER_VERIFICATION_REQUIREMENT_DISCOURAGED);
    assert!(passkey::ATTACHMENT_ANY == WEBAUTHN_AUTHENTICATOR_ATTACHMENT_ANY);
    assert!(passkey::ATTACHMENT_PLATFORM == WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM);
    assert!(passkey::ATTACHMENT_CROSS_PLATFORM == WEBAUTHN_AUTHENTICATOR_ATTACHMENT_CROSS_PLATFORM);
    assert!(passkey::ATTESTATION_NONE == WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_NONE);
    assert!(passkey::ATTESTATION_INDIRECT == WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_INDIRECT);
    assert!(passkey::ATTESTATION_DIRECT == WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_DIRECT);
    assert!(passkey::transport::USB == WEBAUTHN_CTAP_TRANSPORT_USB);
    assert!(passkey::transport::NFC == WEBAUTHN_CTAP_TRANSPORT_NFC);
    assert!(passkey::transport::BLE == WEBAUTHN_CTAP_TRANSPORT_BLE);
    assert!(passkey::transport::INTERNAL == WEBAUTHN_CTAP_TRANSPORT_INTERNAL);
    assert!(passkey::transport::HYBRID == WEBAUTHN_CTAP_TRANSPORT_HYBRID);
    assert!(passkey::NTE_USER_CANCELLED == NTE_USER_CANCELLED.0);
    assert!(passkey::ERROR_CANCELLED == 0x8007_0000_u32 as i32 | ERROR_CANCELLED.0 as i32);
};

type Version = unsafe extern "system" fn() -> u32;
type PlatformAvailable = unsafe extern "system" fn(*mut BOOL) -> HRESULT;
type MakeCredential = unsafe extern "system" fn(
    HWND,
    *const WEBAUTHN_RP_ENTITY_INFORMATION,
    *const WEBAUTHN_USER_ENTITY_INFORMATION,
    *const WEBAUTHN_COSE_CREDENTIAL_PARAMETERS,
    *const WEBAUTHN_CLIENT_DATA,
    *const WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS,
    *mut *mut WEBAUTHN_CREDENTIAL_ATTESTATION,
) -> HRESULT;
type GetAssertion = unsafe extern "system" fn(
    HWND,
    PCWSTR,
    *const WEBAUTHN_CLIENT_DATA,
    *const WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS,
    *mut *mut WEBAUTHN_ASSERTION,
) -> HRESULT;
type FreeAttestation = unsafe extern "system" fn(*const WEBAUTHN_CREDENTIAL_ATTESTATION);
type FreeAssertion = unsafe extern "system" fn(*const WEBAUTHN_ASSERTION);
type CancellationId = unsafe extern "system" fn(*mut GUID) -> HRESULT;
type Cancel = unsafe extern "system" fn(*const GUID) -> HRESULT;
type ErrorName = unsafe extern "system" fn(HRESULT) -> PCWSTR;

/// `webauthn.dll`'s functions (all there since API version 1).
struct Api {
    version: u32,
    platform_available: PlatformAvailable,
    make_credential: MakeCredential,
    get_assertion: GetAssertion,
    free_attestation: FreeAttestation,
    free_assertion: FreeAssertion,
    cancellation_id: CancellationId,
    cancel: Cancel,
    error_name: ErrorName,
}

fn load() -> Option<Api> {
    unsafe {
        let dll = LoadLibraryExW(w!("webauthn.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let f = |name: PCSTR| GetProcAddress(dll, name);
        macro_rules! get {
            ($name:literal as $t:ty) => {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, $t>(f(s!($name))?)
            };
        }
        let version = get!("WebAuthNGetApiVersionNumber" as Version);
        Some(Api {
            version: version(),
            platform_available: get!(
                "WebAuthNIsUserVerifyingPlatformAuthenticatorAvailable" as PlatformAvailable
            ),
            make_credential: get!("WebAuthNAuthenticatorMakeCredential" as MakeCredential),
            get_assertion: get!("WebAuthNAuthenticatorGetAssertion" as GetAssertion),
            free_attestation: get!("WebAuthNFreeCredentialAttestation" as FreeAttestation),
            free_assertion: get!("WebAuthNFreeAssertion" as FreeAssertion),
            cancellation_id: get!("WebAuthNGetCancellationId" as CancellationId),
            cancel: get!("WebAuthNCancelCurrentOperation" as Cancel),
            error_name: get!("WebAuthNGetErrorName" as ErrorName),
        })
    }
}

fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let api = load();
        if api.is_none() {
            log!("passkeys: no webauthn.dll (Windows 10 1903 or later has it)");
        }
        api
    })
    .as_ref()
}

/// Whether the app can use passkeys: the DLL is there. A PC without
/// Windows Hello still has security keys and phones.
pub fn available() -> bool {
    api().is_some_and(|a| a.version >= WEBAUTHN_API_VERSION_1)
}

/// Whether Windows Hello can hold passkeys here (`isUserVerifyingPlatformAuthenticatorAvailable`).
pub fn platform_authenticator() -> Option<bool> {
    let api = api()?;
    let mut available = BOOL(0);
    unsafe { (api.platform_available)(&mut available) }
        .ok()
        .ok()?;
    Some(available.as_bool())
}

/// What `WebAuthNGetErrorName` calls `hr` (`NotAllowedError`…).
pub fn error_name(hr: HRESULT) -> String {
    api()
        .map(|api| unsafe { (api.error_name)(hr) })
        .filter(|p| !p.is_null())
        .and_then(|p| unsafe { p.to_string() }.ok())
        .unwrap_or_default()
}

/// What `PRISM_DEVICE_REPORT` says of passkeys: the DLL's API version,
/// Windows Hello as an authenticator, and how failures read (their
/// WebAuthn names, Prism's errors), without opening Windows' dialog.
pub fn report() -> serde_json::Value {
    let Some(api) = api() else {
        return serde_json::json!({ "dll": false });
    };
    let errors: serde_json::Map<String, serde_json::Value> = passkey::ERROR_NAMES
        .iter()
        .map(|&(hr, _)| hr)
        .chain([passkey::UNNAMED])
        .map(|hr| {
            let hr = HRESULT(hr);
            (
                format!("0x{:08X}", hr.0 as u32),
                serde_json::json!([error_name(hr), format!("{:?}", failure(hr))]),
            )
        })
        .collect();
    let (make, get) = passkey::option_versions(api.version);
    serde_json::json!({
        "dll": true,
        "api_version": api.version,
        "make_credential_options_version": make,
        "get_assertion_options_version": get,
        "platform_authenticator": platform_authenticator(),
        "errors": errors,
    })
}

/// The ceremonies, one at a time (`passkey::Turns`).
static TURNS: passkey::Turns<GUID> = passkey::Turns::new();

/// How long a new ceremony waits for the one it cancelled to return.
const CANCELLED_ENDS_WITHIN: Duration = Duration::from_secs(5);

fn cancel_ceremony(api: &'static Api, id: Option<GUID>) {
    if let Some(id) = id {
        unsafe {
            let _ = (api.cancel)(&id);
        }
    }
}

/// Closes Windows' dialog of the ceremony still open, which answers
/// "cancelled", and drops the ones waiting for their turn.
pub fn cancel() {
    if let Some(api) = api() {
        cancel_ceremony(api, TURNS.cancel_all());
    }
}

/// Runs a ceremony off the UI thread and gives its answer (the credential
/// as JSON) to `done` on the UI thread.
pub fn ceremony(
    owner: HWND,
    request: prism_core::security::WebAuthnRequest,
    done: impl FnOnce(Result<DeviceValue, DeviceError>) + Send + 'static,
) {
    let Some(api) = api() else {
        return done(Err(DeviceError::Unavailable));
    };
    if request.conditional {
        return done(Err(DeviceError::Unavailable));
    }
    // One dialog at a time: the open one answers "cancelled", and this
    // one starts once it has returned (Windows runs one call at a time).
    let mut id = GUID::zeroed();
    let id = unsafe { (api.cancellation_id)(&mut id) }
        .is_ok()
        .then_some(id);
    let (ticket, open) = TURNS.begin(id);
    cancel_ceremony(api, open);
    let owner = crate::win::hwnd_value(owner);
    std::thread::spawn(move || {
        let owner = crate::win::hwnd(owner);
        let r = match TURNS.wait_turn(&ticket, CANCELLED_ENDS_WITHIN) {
            passkey::Turn::Superseded => Err(DeviceError::Cancelled),
            turn => {
                if turn == passkey::Turn::Late {
                    log!(
                        "passkeys: the cancelled ceremony has not returned in {} s; this one starts anyway",
                        CANCELLED_ENDS_WITHIN.as_secs()
                    );
                }
                let mut id = ticket.id;
                let r = if request.create {
                    passkey::registration(&request.options)
                        .and_then(|o| create(api, owner, &o, id.as_mut()))
                } else {
                    passkey::assertion(&request.options)
                        .and_then(|o| get(api, owner, &o, id.as_mut()))
                };
                TURNS.end(&ticket);
                r
            }
        };
        if let Err(e) = &r {
            log!("passkeys: {e:?}");
        }
        crate::shell::on_ui(move || done(r.map(|json| DeviceValue::Text(Some(json)))));
    });
}

fn failure(hr: HRESULT) -> DeviceError {
    passkey::error(
        hr.0,
        &error_name(hr),
        &windows::core::Error::from_hresult(hr).message(),
    )
}

/// The client data, owned for the call.
struct ClientData {
    json: String,
}

impl ClientData {
    fn new(create: bool, challenge: &[u8], rp_id: &str) -> ClientData {
        ClientData {
            json: passkey::client_data(create, challenge, rp_id),
        }
    }

    fn raw(&self) -> WEBAUTHN_CLIENT_DATA {
        WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: self.json.len() as u32,
            // Only read.
            pbClientDataJSON: self.json.as_ptr() as *mut u8,
            pwszHashAlgId: WEBAUTHN_HASH_ALGORITHM_SHA_256,
        }
    }
}

/// A credential list (`excludeCredentials`, `allowCredentials`), owned for
/// the call: the list points into `items`, which point into the ids.
struct CredentialList {
    _ids: Vec<Vec<u8>>,
    _items: Vec<WEBAUTHN_CREDENTIAL_EX>,
    pointers: Vec<*mut WEBAUTHN_CREDENTIAL_EX>,
    list: WEBAUTHN_CREDENTIAL_LIST,
}

impl CredentialList {
    fn new(descriptors: &[Descriptor]) -> Box<CredentialList> {
        let mut ids: Vec<Vec<u8>> = descriptors.iter().map(|d| d.id.clone()).collect();
        let mut items: Vec<WEBAUTHN_CREDENTIAL_EX> = descriptors
            .iter()
            .zip(ids.iter_mut())
            .map(|(d, id)| WEBAUTHN_CREDENTIAL_EX {
                dwVersion: WEBAUTHN_CREDENTIAL_EX_CURRENT_VERSION,
                cbId: id.len() as u32,
                pbId: id.as_mut_ptr(),
                pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
                dwTransports: d.transports,
            })
            .collect();
        let pointers: Vec<*mut WEBAUTHN_CREDENTIAL_EX> =
            items.iter_mut().map(|i| i as *mut _).collect();
        let mut list = Box::new(CredentialList {
            _ids: ids,
            _items: items,
            pointers,
            list: WEBAUTHN_CREDENTIAL_LIST::default(),
        });
        list.list = WEBAUTHN_CREDENTIAL_LIST {
            cCredentials: list.pointers.len() as u32,
            ppCredentials: list.pointers.as_mut_ptr(),
        };
        list
    }

    /// The list for the options; null when it is empty.
    fn raw(&mut self) -> *mut WEBAUTHN_CREDENTIAL_LIST {
        if self.pointers.is_empty() {
            std::ptr::null_mut()
        } else {
            &mut self.list
        }
    }
}

/// The bytes at `p` (`n` of them); empty for null.
unsafe fn bytes<'a>(p: *const u8, n: u32) -> &'a [u8] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(p, n as usize) }
    }
}

fn create(
    api: &Api,
    owner: HWND,
    o: &passkey::Registration,
    cancel_id: Option<&mut GUID>,
) -> Result<String, DeviceError> {
    let (version, _) = passkey::option_versions(api.version);
    let (rp_id, rp_name) = (wide(&o.rp_id), wide(&o.rp_name));
    let rp = WEBAUTHN_RP_ENTITY_INFORMATION {
        dwVersion: WEBAUTHN_RP_ENTITY_INFORMATION_CURRENT_VERSION,
        pwszId: PCWSTR(rp_id.as_ptr()),
        pwszName: PCWSTR(rp_name.as_ptr()),
        pwszIcon: PCWSTR::null(),
    };
    let mut user_id = o.user_id.clone();
    let (user_name, display_name) = (wide(&o.user_name), wide(&o.user_display_name));
    let user = WEBAUTHN_USER_ENTITY_INFORMATION {
        dwVersion: WEBAUTHN_USER_ENTITY_INFORMATION_CURRENT_VERSION,
        cbId: user_id.len() as u32,
        pbId: user_id.as_mut_ptr(),
        pwszName: PCWSTR(user_name.as_ptr()),
        pwszIcon: PCWSTR::null(),
        pwszDisplayName: PCWSTR(display_name.as_ptr()),
    };
    let mut params: Vec<WEBAUTHN_COSE_CREDENTIAL_PARAMETER> = o
        .algorithms
        .iter()
        .map(|alg| WEBAUTHN_COSE_CREDENTIAL_PARAMETER {
            dwVersion: WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION,
            pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
            lAlg: *alg,
        })
        .collect();
    let params = WEBAUTHN_COSE_CREDENTIAL_PARAMETERS {
        cCredentialParameters: params.len() as u32,
        pCredentialParameters: params.as_mut_ptr(),
    };
    let client = ClientData::new(true, &o.challenge, &o.rp_id);
    let client_data = client.raw();
    let mut exclude = CredentialList::new(&o.exclude);
    let options = WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS {
        dwVersion: version,
        dwTimeoutMilliseconds: o.timeout_ms,
        dwAuthenticatorAttachment: o.attachment,
        bRequireResidentKey: (o.resident == Resident::Required).into(),
        dwUserVerificationRequirement: o.user_verification,
        dwAttestationConveyancePreference: o.attestation,
        pCancellationId: cancel_id.map_or(std::ptr::null_mut(), |id| id as *mut _),
        pExcludeCredentialList: exclude.raw(),
        // Version 4 and later; older ones make a non-discoverable credential.
        bPreferResidentKey: (o.resident == Resident::Preferred).into(),
        ..Default::default()
    };
    let mut out = std::ptr::null_mut();
    let hr = unsafe {
        (api.make_credential)(owner, &rp, &user, &params, &client_data, &options, &mut out)
    };
    if hr.is_err() || out.is_null() {
        return Err(failure(hr));
    }
    let json = unsafe {
        let a = &*out;
        let json = passkey::registration_json(
            &passkey::NewCredential {
                id: bytes(a.pbCredentialId, a.cbCredentialId),
                client_data: &client.json,
                attestation_object: bytes(a.pbAttestationObject, a.cbAttestationObject),
                authenticator_data: bytes(a.pbAuthenticatorData, a.cbAuthenticatorData),
                transport: (a.dwVersion >= WEBAUTHN_CREDENTIAL_ATTESTATION_VERSION_3)
                    .then_some(a.dwUsedTransport),
                resident: (a.dwVersion >= WEBAUTHN_CREDENTIAL_ATTESTATION_VERSION_4)
                    .then(|| a.bResidentKey.as_bool()),
            },
            o.cred_props,
        );
        (api.free_attestation)(out);
        json
    };
    Ok(json)
}

fn get(
    api: &Api,
    owner: HWND,
    o: &passkey::Assertion,
    cancel_id: Option<&mut GUID>,
) -> Result<String, DeviceError> {
    let (_, version) = passkey::option_versions(api.version);
    let rp_id = wide(&o.rp_id);
    let client = ClientData::new(false, &o.challenge, &o.rp_id);
    let client_data = client.raw();
    let mut allow = CredentialList::new(&o.allow);
    let options = WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS {
        dwVersion: version,
        dwTimeoutMilliseconds: o.timeout_ms,
        dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_ANY,
        dwUserVerificationRequirement: o.user_verification,
        pCancellationId: cancel_id.map_or(std::ptr::null_mut(), |id| id as *mut _),
        pAllowCredentialList: allow.raw(),
        ..Default::default()
    };
    let mut out = std::ptr::null_mut();
    let hr = unsafe {
        (api.get_assertion)(
            owner,
            PCWSTR(rp_id.as_ptr()),
            &client_data,
            &options,
            &mut out,
        )
    };
    if hr.is_err() || out.is_null() {
        return Err(failure(hr));
    }
    let json = unsafe {
        let a = &*out;
        let json = passkey::assertion_json(&passkey::Signature {
            id: bytes(a.Credential.pbId, a.Credential.cbId),
            client_data: &client.json,
            authenticator_data: bytes(a.pbAuthenticatorData, a.cbAuthenticatorData),
            signature: bytes(a.pbSignature, a.cbSignature),
            user_handle: bytes(a.pbUserId, a.cbUserId),
            transport: (a.dwVersion >= WEBAUTHN_ASSERTION_VERSION_4).then_some(a.dwUsedTransport),
        });
        (api.free_assertion)(out);
        json
    };
    Ok(json)
}
