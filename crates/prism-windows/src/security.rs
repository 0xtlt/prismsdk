//! Secrets, Windows Hello and browser sign-in:
//!
//! - **secure storage**: Credential Manager (`CredWriteW`, `CredReadW`),
//!   generic credentials of this user named `Prism/<app>/<key>` (Control
//!   Panel › Credential Manager › Windows Credentials lists them). A value
//!   longer than one credential holds (2,560 bytes) is split over several.
//!   A `biometric` secret is stored sealed with a key only Windows Hello
//!   gives back (`seal`, S22); a `confirm_owner` one is stored as it is,
//!   and Hello is asked before it is handed to the app;
//! - **biometrics**: Windows Hello (`UserConsentVerifier`, its prompt owned
//!   by the app's window), which also accepts the Hello PIN; the sensor
//!   comes from the Windows Biometric Framework. Without Hello set up,
//!   `authenticate` answers "unavailable";
//! - **sign-in**: the default browser and a loopback redirect
//!   (`http://127.0.0.1:<port>/callback`, RFC 8252 §7.3), like on Linux.

use std::collections::BTreeMap;

use prism_core::{BiometryKind, DeviceError, DeviceValue};
use windows::Security::Credentials::UI::{
    UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
};
use windows::Security::Credentials::{
    KeyCredential, KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus,
};
use windows::Security::Cryptography::Core::CryptographicPublicKeyBlobType;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Storage::Streams::IBuffer;
use windows::Win32::Foundation::{ERROR_NO_SUCH_LOGON_SESSION, ERROR_NOT_FOUND, HWND};
use windows::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW,
    CredDeleteW, CredFree, CredReadW, CredWriteW,
};
use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
use windows::core::{HRESULT, HSTRING, PWSTR, factory};

use crate::seal;
use crate::win::{log, wide};

pub type Answer = Result<DeviceValue, DeviceError>;

fn failed(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Failed(e.to_string())
}

/// Credential Manager's error as Prism's: a logon without the user's
/// password (an SSH key, a service) has no credential store.
fn store_error(e: windows::core::Error) -> DeviceError {
    if e.code() == HRESULT::from_win32(ERROR_NO_SUCH_LOGON_SESSION.0) {
        log!("secure storage: this logon session has no credential store");
        DeviceError::Unavailable
    } else {
        failed(e.message())
    }
}

// --- Credential Manager ----------------------------------------------------------------

/// The first credential of a long value: how many parts follow.
const PARTS: &str = "\u{2}prism-parts\u{2}";

fn target(app: &str, key: &str) -> String {
    format!("Prism/{app}/{key}")
}

fn read_blob(target: &str) -> Result<Option<Vec<u8>>, DeviceError> {
    let name = wide(target);
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        match CredReadW(
            windows::core::PCWSTR(name.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
            &mut cred,
        ) {
            Ok(()) => {
                let c = &*cred;
                let bytes =
                    std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
                        .to_vec();
                CredFree(cred as *const _);
                Ok(Some(bytes))
            }
            Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(None),
            Err(e) => Err(store_error(e)),
        }
    }
}

fn write_blob(target: &str, user: &str, bytes: &[u8]) -> Result<(), DeviceError> {
    let mut name = wide(target);
    let mut user = wide(user);
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_mut_ptr()),
        UserName: PWSTR(user.as_mut_ptr()),
        CredentialBlobSize: bytes.len() as u32,
        CredentialBlob: bytes.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0).map_err(store_error) }
}

fn delete_blob(target: &str) -> Result<(), DeviceError> {
    let name = wide(target);
    match unsafe {
        CredDeleteW(
            windows::core::PCWSTR(name.as_ptr()),
            CRED_TYPE_GENERIC,
            None,
        )
    } {
        Ok(()) => Ok(()),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(()),
        Err(e) => Err(store_error(e)),
    }
}

/// How many parts a stored value has (`None`: it fits in one).
fn parts(first: &[u8]) -> Option<usize> {
    std::str::from_utf8(first)
        .ok()?
        .strip_prefix(PARTS)?
        .parse()
        .ok()
}

/// The secret stored under `key`, or `None`.
pub fn secret_get(app: &str, key: &str) -> Result<Option<String>, DeviceError> {
    let t = target(app, key);
    let Some(first) = read_blob(&t)? else {
        return Ok(None);
    };
    let bytes = match parts(&first) {
        None => first,
        Some(n) => {
            let mut all = Vec::new();
            for i in 0..n {
                match read_blob(&format!("{t}#{i}"))? {
                    Some(b) => all.extend(b),
                    None => return Err(failed("a part of the secret is missing")),
                }
            }
            all
        }
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| failed("the stored secret is not UTF-8"))
}

/// Stores `value` under `key`, replacing what was there.
pub fn secret_set(app: &str, key: &str, value: &str) -> Result<(), DeviceError> {
    secret_delete(app, key)?;
    let t = target(app, key);
    let bytes = value.as_bytes();
    let max = CRED_MAX_CREDENTIAL_BLOB_SIZE as usize;
    if bytes.len() <= max {
        return write_blob(&t, key, bytes);
    }
    let chunks: Vec<&[u8]> = bytes.chunks(max).collect();
    for (i, c) in chunks.iter().enumerate() {
        write_blob(&format!("{t}#{i}"), key, c)?;
    }
    write_blob(&t, key, format!("{PARTS}{}", chunks.len()).as_bytes())
}

pub fn secret_delete(app: &str, key: &str) -> Result<(), DeviceError> {
    let t = target(app, key);
    if let Some(first) = read_blob(&t)?
        && let Some(n) = parts(&first)
    {
        for i in 0..n {
            delete_blob(&format!("{t}#{i}"))?;
        }
    }
    delete_blob(&t)
}

// --- Windows Hello ---------------------------------------------------------------------

/// What the machine has: the sensor, and whether Hello (a sensor or the
/// Hello PIN) can verify the user now.
#[derive(Clone, Copy, Debug)]
pub struct Hello {
    pub kind: BiometryKind,
    pub availability: Option<UserConsentVerifierAvailability>,
}

impl Hello {
    pub fn usable(&self) -> bool {
        self.availability == Some(UserConsentVerifierAvailability::Available)
    }

    /// `cx.biometry()`'s fields: `kind`, `enrolled` (Hello can verify
    /// with the sensor), `passcode` (Hello can verify: the Hello PIN or a
    /// sensor is set up), `locked_out`.
    pub fn fields(&self) -> BTreeMap<String, String> {
        let flag = |b: bool| (if b { "1" } else { "0" }).to_string();
        let mut f = BTreeMap::new();
        f.insert("kind".into(), self.kind.name().into());
        f.insert(
            "enrolled".into(),
            flag(self.usable() && self.kind != BiometryKind::None),
        );
        f.insert("passcode".into(), flag(self.usable()));
        // Hello says nothing of lockouts before a prompt.
        f.insert("locked_out".into(), flag(false));
        f
    }
}

/// The biometric sensors this machine has (Windows Biometric Framework):
/// a face camera first, then a fingerprint reader.
fn sensor() -> BiometryKind {
    use windows::Win32::Devices::BiometricFramework::{
        WINBIO_UNIT_SCHEMA, WinBioEnumBiometricUnits, WinBioFree,
    };
    // winbio_types.h: WINBIO_TYPE_FACIAL_FEATURES, _FINGERPRINT, _IRIS.
    for (factor, kind) in [
        (0x2u32, BiometryKind::Face),
        (0x8, BiometryKind::Fingerprint),
        (0x10, BiometryKind::Iris),
    ] {
        let mut units: *mut WINBIO_UNIT_SCHEMA = std::ptr::null_mut();
        let mut count = 0usize;
        let found = unsafe {
            let r = WinBioEnumBiometricUnits(factor, &mut units, &mut count);
            if !units.is_null() {
                let _ = WinBioFree(units as *const _);
            }
            r.is_ok() && count > 0
        };
        if found {
            return kind;
        }
    }
    BiometryKind::None
}

/// Checks Hello (`CheckAvailabilityAsync`) and the sensors: 100 ms or
/// more (seconds when cold). Blocks: called off the UI thread (at
/// startup, `probe::start`).
pub fn hello() -> Hello {
    let availability = UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|op| op.join())
        .map_err(|e| log!("Windows Hello: {e}"))
        .ok();
    Hello {
        kind: sensor(),
        availability,
    }
}

/// Why Hello cannot verify, as Prism's error.
fn unavailable(a: Option<UserConsentVerifierAvailability>) -> DeviceError {
    let why = match a {
        Some(UserConsentVerifierAvailability::DeviceNotPresent) => "no Windows Hello device",
        Some(UserConsentVerifierAvailability::NotConfiguredForUser) => {
            "Windows Hello is not set up for this user"
        }
        Some(UserConsentVerifierAvailability::DisabledByPolicy) => {
            "Windows Hello is turned off by policy"
        }
        Some(UserConsentVerifierAvailability::DeviceBusy) => "the Hello device is busy",
        _ => "Windows Hello is not available",
    };
    log!("authenticate: {why}");
    DeviceError::Unavailable
}

/// Asks the user to verify with Windows Hello, the prompt owned by
/// `owner`. `done` runs on the UI thread.
pub fn authenticate(owner: HWND, reason: &str, done: impl FnOnce(Answer) + Send + 'static) {
    let message = HSTRING::from(reason);
    // HWNDs cross threads as integers.
    let owner = crate::win::hwnd_value(owner);
    std::thread::spawn(move || {
        let state = hello();
        if !state.usable() {
            let e = unavailable(state.availability);
            return crate::shell::on_ui(move || done(Err(e)));
        }
        // The prompt belongs to the window: ask from the UI thread.
        crate::shell::on_ui(move || {
            let op = factory::<UserConsentVerifier, IUserConsentVerifierInterop>().and_then(
                |interop| unsafe {
                    interop.RequestVerificationForWindowAsync::<
                        windows_future::IAsyncOperation<UserConsentVerificationResult>,
                    >(crate::win::hwnd(owner), &message)
                },
            );
            let op = match op {
                Ok(op) => op,
                Err(e) => return done(Err(failed(e.message()))),
            };
            crate::win::when("Windows Hello", &op, move |r| {
                let answer = match r {
                    Ok(UserConsentVerificationResult::Verified) => Ok(DeviceValue::Done),
                    Ok(UserConsentVerificationResult::Canceled) => Err(DeviceError::Cancelled),
                    Ok(UserConsentVerificationResult::RetriesExhausted) => Err(DeviceError::Denied),
                    Ok(
                        UserConsentVerificationResult::DeviceNotPresent
                        | UserConsentVerificationResult::NotConfiguredForUser
                        | UserConsentVerificationResult::DisabledByPolicy,
                    ) => Err(DeviceError::Unavailable),
                    Ok(other) => Err(failed(format!("Windows Hello: {other:?}"))),
                    Err(e) => Err(failed(e.message())),
                };
                crate::shell::on_ui(move || done(answer));
            });
        });
    });
}

// --- Secrets only Windows Hello opens (S22; the format: `seal`) ---------------------------

/// The app's Hello key (`KeyCredentialManager`). Windows shows its own
/// prompt for each signature: the API takes no window.
struct HelloCredential(KeyCredential);

impl seal::HelloKey for HelloCredential {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, DeviceError> {
        let data = CryptographicBuffer::CreateFromByteArray(message).map_err(win_failed)?;
        let r = self
            .0
            .RequestSignAsync(&data)
            .and_then(|op| op.join())
            .map_err(win_failed)?;
        match r.Status().map_err(win_failed)? {
            KeyCredentialStatus::Success => buffer_bytes(&r.Result().map_err(win_failed)?),
            status => Err(key_error(status)),
        }
    }

    fn public_key(&self) -> Result<Vec<u8>, DeviceError> {
        let key = self
            .0
            .RetrievePublicKeyWithBlobType(CryptographicPublicKeyBlobType::Pkcs1RsaPublicKey)
            .map_err(win_failed)?;
        buffer_bytes(&key)
    }
}

fn win_failed(e: windows::core::Error) -> DeviceError {
    failed(e.message())
}

fn buffer_bytes(buffer: &IBuffer) -> Result<Vec<u8>, DeviceError> {
    let mut bytes = windows::core::Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(buffer, &mut bytes).map_err(win_failed)?;
    Ok(bytes.to_vec())
}

/// A Hello key operation's failure as Prism's error.
fn key_error(status: KeyCredentialStatus) -> DeviceError {
    match status {
        KeyCredentialStatus::UserCanceled | KeyCredentialStatus::UserPrefersPassword => {
            DeviceError::Cancelled
        }
        // Too many failed attempts: Hello is locked for a while.
        KeyCredentialStatus::SecurityDeviceLocked => DeviceError::Denied,
        KeyCredentialStatus::NotFound => {
            failed("the app's Windows Hello key is gone (Hello was reset): store the secret again")
        }
        other => failed(format!("Windows Hello: {other:?}")),
    }
}

/// Whether this user has Windows Hello set up (a PIN at least), which
/// Hello keys need.
fn hello_keys() -> bool {
    KeyCredentialManager::IsSupportedAsync()
        .and_then(|op| op.join())
        .unwrap_or_else(|e| {
            log!("Windows Hello keys: {e}");
            false
        })
}

/// `app`'s Hello key; `create`: made the first time (Windows asks the
/// user). Without Hello: `Unavailable`.
fn hello_key(app: &str, create: bool) -> Result<HelloCredential, DeviceError> {
    if !hello_keys() {
        log!("secure storage: Windows Hello is not set up: no biometric secrets");
        return Err(DeviceError::Unavailable);
    }
    let name = HSTRING::from(seal::key_name(app));
    let open = || {
        KeyCredentialManager::OpenAsync(&name)
            .and_then(|op| op.join())
            .map_err(win_failed)
    };
    let mut r = open()?;
    if create && r.Status().map_err(win_failed)? == KeyCredentialStatus::NotFound {
        // Never `ReplaceExisting`: the app's other biometric secrets need
        // the key.
        r = KeyCredentialManager::RequestCreateAsync(
            &name,
            KeyCredentialCreationOption::FailIfExists,
        )
        .and_then(|op| op.join())
        .map_err(win_failed)?;
        if r.Status().map_err(win_failed)? == KeyCredentialStatus::CredentialAlreadyExists {
            // Another thread made it meanwhile.
            r = open()?;
        }
    }
    match r.Status().map_err(win_failed)? {
        KeyCredentialStatus::Success => Ok(HelloCredential(r.Credential().map_err(win_failed)?)),
        status => Err(key_error(status)),
    }
}

/// Stores `value` under `key` sealed with the app's Hello key: Windows
/// asks the user. Blocks: call it off the UI thread. Without Hello, or if
/// the user cancels, nothing is stored and the previous secret stays.
pub fn sealed_set(app: &str, key: &str, value: &str) -> Result<(), DeviceError> {
    let hello = hello_key(app, true)?;
    let stored = seal::seal(&hello, app, key, value)?;
    secret_set(app, key, &stored)
}

/// The secret `sealed_set` stored (`stored`: what Credential Manager
/// has), opened with the app's Hello key: Windows asks the user. Blocks.
pub fn sealed_get(app: &str, key: &str, stored: &str) -> Result<String, DeviceError> {
    let hello = hello_key(app, false)?;
    seal::open(&hello, app, key, stored)
}

// --- Loopback redirect (RFC 8252 §7.3) -------------------------------------------------

/// The listener waiting for the next sign-in (an unused one closes after
/// 10 minutes: `OAuth::resolve` asks for a redirect it does not use).
#[derive(Default)]
pub struct Loopback {
    listener: prism_core::security::LoopbackListener,
}

impl Loopback {
    /// `http://127.0.0.1:<port>/callback`.
    pub fn redirect(&mut self) -> Answer {
        let uri = self.listener.redirect().map_err(failed)?;
        Ok(DeviceValue::Text(Some(uri)))
    }

    /// Opens `url` in the default browser and waits (on a thread) for the
    /// redirect to `redirect`; `done` runs on the UI thread.
    pub fn start(&mut self, url: &str, redirect: &str, done: impl FnOnce(Answer) + Send + 'static) {
        let Some(listener) = self.listener.take(redirect) else {
            return done(Err(failed(
                "unknown redirect URI (ask auth_redirect first)",
            )));
        };
        if let Err(e) = crate::services::open(url) {
            return done(Err(e));
        }
        let (url, redirect) = (url.to_string(), redirect.to_string());
        std::thread::spawn(move || {
            let timeout = std::time::Duration::from_secs(600);
            let r =
                prism_core::security::catch_loopback_redirect(&listener, &url, &redirect, timeout)
                    .map(|callback| DeviceValue::Text(Some(callback)));
            crate::shell::on_ui(move || done(r));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A secret round trip through Credential Manager, a long one split in
    /// parts; everything is deleted after.
    #[test]
    fn credential_round_trip() {
        let app = format!("prism-test-{}", std::process::id());
        if secret_set(&app, "token", "probe") == Err(DeviceError::Unavailable) {
            eprintln!("skipped: no credential store in this logon session (SSH key logon)");
            return;
        }
        let long: String = "0123456789abcdef✓".repeat(400);
        for value in ["s3cret ✓", long.as_str()] {
            secret_set(&app, "token", value).unwrap();
            assert_eq!(secret_get(&app, "token").unwrap().as_deref(), Some(value));
        }
        secret_set(&app, "token", "short").unwrap();
        assert_eq!(secret_get(&app, "token").unwrap().as_deref(), Some("short"));
        // The parts of the long one are gone.
        assert_eq!(
            read_blob(&format!("{}#0", target(&app, "token"))).unwrap(),
            None
        );
        secret_delete(&app, "token").unwrap();
        assert_eq!(secret_get(&app, "token").unwrap(), None);
        secret_delete(&app, "token").unwrap();
    }

    /// Without Windows Hello, a biometric secret is not stored, and the
    /// previous one stays (S22). Where Hello is set up, storing would show
    /// its prompt: skipped.
    #[test]
    fn a_biometric_secret_without_hello_keeps_the_previous_one() {
        if hello_keys() {
            eprintln!("skipped: Windows Hello is set up here (storing would ask the user)");
            return;
        }
        let app = format!("prism-test-hello-{}", std::process::id());
        if secret_set(&app, "token", "previous") == Err(DeviceError::Unavailable) {
            eprintln!("skipped: no credential store in this logon session (SSH key logon)");
            return;
        }
        assert_eq!(
            sealed_set(&app, "token", "new"),
            Err(DeviceError::Unavailable)
        );
        assert_eq!(
            secret_get(&app, "token").unwrap().as_deref(),
            Some("previous")
        );
        // A sealed value cannot be read without Hello either.
        assert_eq!(
            sealed_get(&app, "token", &format!("{}00", seal::MARK)),
            Err(DeviceError::Unavailable)
        );
        secret_delete(&app, "token").unwrap();
    }

    #[test]
    fn loopback_catches_the_redirect() {
        let mut lb = Loopback::default();
        let Ok(DeviceValue::Text(Some(uri))) = lb.redirect() else {
            panic!()
        };
        let listener = lb.listener.take(&uri).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut page = String::new();
            let _ = s.read_to_string(&mut page);
            page
        });
        let r = prism_core::security::catch_loopback_redirect(
            &listener,
            "https://idp.example/authorize?state=xyz",
            &uri,
            std::time::Duration::from_secs(10),
        );
        assert_eq!(
            r,
            Ok(format!(
                "http://127.0.0.1:{port}/callback?code=abc&state=xyz"
            ))
        );
        assert!(client.join().unwrap().contains("200 OK"));
    }
}
