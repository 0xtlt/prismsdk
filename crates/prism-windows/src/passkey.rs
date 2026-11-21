//! Passkeys of the app's own relying party (#94) through Windows' WebAuthn
//! API (`webauthn.dll`, `webauthn.rs`): the server's options (WebAuthn
//! JSON, binary fields in base64url) as the API's parameters, its answers
//! as the WebAuthn JSON forms (`toJSON()`), its errors as Prism's. No Win32
//! types: tested on every host.
//!
//! Windows leaves the client's part to the caller, as a browser does: the
//! app builds the client data (`clientDataJSON`) and Windows hashes it.
//! Its origin is `https://<relying party id>`, what Apple platforms give a
//! native app, so a server that accepts the app there accepts it here.
//! Windows checks no link between the app and the relying party's domain
//! (Apple's `webcredentials:`, Android's asset links): the server's checks
//! (challenge, origin, signature) are what protect the account.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::DeviceError;
use serde_json::{Map, Value, json};

/// `WEBAUTHN_CTAP_TRANSPORT_*`.
pub mod transport {
    pub const USB: u32 = 0x1;
    pub const NFC: u32 = 0x2;
    pub const BLE: u32 = 0x4;
    pub const INTERNAL: u32 = 0x10;
    pub const HYBRID: u32 = 0x20;
}

/// `WEBAUTHN_AUTHENTICATOR_ATTACHMENT_*`.
pub const ATTACHMENT_ANY: u32 = 0;
pub const ATTACHMENT_PLATFORM: u32 = 1;
pub const ATTACHMENT_CROSS_PLATFORM: u32 = 2;
/// `WEBAUTHN_USER_VERIFICATION_REQUIREMENT_*`.
pub const UV_REQUIRED: u32 = 1;
pub const UV_PREFERRED: u32 = 2;
pub const UV_DISCOURAGED: u32 = 3;
/// `WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_*`.
pub const ATTESTATION_NONE: u32 = 1;
pub const ATTESTATION_INDIRECT: u32 = 2;
pub const ATTESTATION_DIRECT: u32 = 3;
/// `COSE_ALGORITHM_ECDSA_P256_WITH_SHA256`, `…_RSASSA_PKCS1_V1_5_WITH_SHA256`:
/// what WebAuthn uses when the server names none.
pub const DEFAULT_ALGORITHMS: [i32; 2] = [-7, -257];
/// Five minutes when the server gives no timeout; ten at most.
pub const DEFAULT_TIMEOUT_MS: u32 = 300_000;
pub const MAX_TIMEOUT_MS: u32 = 600_000;

/// A credential the server names (`excludeCredentials`, `allowCredentials`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub id: Vec<u8>,
    /// `WEBAUTHN_CTAP_TRANSPORT_*` bits; 0: any.
    pub transports: u32,
}

/// Whether the new credential must be discoverable (a passkey).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resident {
    Required,
    Preferred,
    Discouraged,
}

/// `navigator.credentials.create`'s options.
#[derive(Clone, Debug, PartialEq)]
pub struct Registration {
    pub rp_id: String,
    pub rp_name: String,
    pub user_id: Vec<u8>,
    pub user_name: String,
    pub user_display_name: String,
    pub challenge: Vec<u8>,
    /// COSE algorithms, the server's order.
    pub algorithms: Vec<i32>,
    pub timeout_ms: u32,
    pub exclude: Vec<Descriptor>,
    pub attachment: u32,
    pub resident: Resident,
    pub user_verification: u32,
    pub attestation: u32,
    /// The `credProps` extension: say whether the credential is discoverable.
    pub cred_props: bool,
}

/// `navigator.credentials.get`'s options.
#[derive(Clone, Debug, PartialEq)]
pub struct Assertion {
    pub rp_id: String,
    pub challenge: Vec<u8>,
    pub timeout_ms: u32,
    pub allow: Vec<Descriptor>,
    pub user_verification: u32,
}

// --- base64url -------------------------------------------------------------------------

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url without padding, as WebAuthn's JSON forms write bytes.
pub fn b64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// base64url (or standard base64), padded or not.
pub fn unb64url(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // A lone sixth of a byte is no base64.
    (bits < 6).then_some(out)
}

// --- Options ---------------------------------------------------------------------------

fn failed(m: &str) -> DeviceError {
    DeviceError::Failed(m.into())
}

fn bytes(v: Option<&Value>) -> Option<Vec<u8>> {
    v?.as_str().and_then(unb64url)
}

fn timeout(o: &Map<String, Value>) -> u32 {
    o.get("timeout")
        .and_then(Value::as_f64)
        .filter(|t| *t > 0.)
        .map_or(DEFAULT_TIMEOUT_MS, |t| t.min(MAX_TIMEOUT_MS as f64) as u32)
}

fn user_verification(v: Option<&Value>) -> u32 {
    match v.and_then(Value::as_str) {
        Some("required") => UV_REQUIRED,
        Some("discouraged") => UV_DISCOURAGED,
        _ => UV_PREFERRED,
    }
}

/// `transports` names as `WEBAUTHN_CTAP_TRANSPORT_*` bits (unknown ones
/// skipped: any transport then).
pub fn transport_bits(names: &[&str]) -> u32 {
    names.iter().fold(0, |bits, n| {
        bits | match *n {
            "usb" => transport::USB,
            "nfc" => transport::NFC,
            "ble" => transport::BLE,
            "internal" => transport::INTERNAL,
            "hybrid" => transport::HYBRID,
            _ => 0,
        }
    })
}

/// The transports' names, sorted (as `getTransports()` gives them).
pub fn transport_names(bits: u32) -> Vec<&'static str> {
    [
        (transport::BLE, "ble"),
        (transport::HYBRID, "hybrid"),
        (transport::INTERNAL, "internal"),
        (transport::NFC, "nfc"),
        (transport::USB, "usb"),
    ]
    .into_iter()
    .filter(|(bit, _)| bits & bit != 0)
    .map(|(_, name)| name)
    .collect()
}

fn descriptors(v: Option<&Value>) -> Vec<Descriptor> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|d| {
            d.get("type")
                .and_then(Value::as_str)
                .unwrap_or("public-key")
                == "public-key"
        })
        .filter_map(|d| {
            let names: Vec<&str> = d
                .get("transports")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            Some(Descriptor {
                id: bytes(d.get("id"))?,
                transports: transport_bits(&names),
            })
        })
        .collect()
}

fn object(options: &str) -> Result<Map<String, Value>, DeviceError> {
    match serde_json::from_str(options) {
        Ok(Value::Object(o)) => Ok(o),
        _ => Err(failed("the WebAuthn options are not a JSON object")),
    }
}

fn challenge(o: &Map<String, Value>) -> Result<Vec<u8>, DeviceError> {
    bytes(o.get("challenge"))
        .filter(|c| !c.is_empty())
        .ok_or_else(|| failed("the WebAuthn options have no challenge"))
}

fn no_rp() -> DeviceError {
    failed("the WebAuthn options name no relying party id")
}

/// `PublicKeyCredentialCreationOptionsJSON`.
pub fn registration(options: &str) -> Result<Registration, DeviceError> {
    let o = object(options)?;
    let challenge = challenge(&o)?;
    let rp = o.get("rp");
    let rp_id = rp
        .and_then(|r| r.get("id"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(no_rp)?
        .to_string();
    let rp_name = rp
        .and_then(|r| r.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&rp_id)
        .to_string();
    let user = o.get("user");
    let (Some(user_id), Some(user_name)) = (
        bytes(user.and_then(|u| u.get("id"))).filter(|id| !id.is_empty()),
        user.and_then(|u| u.get("name")).and_then(Value::as_str),
    ) else {
        return Err(failed("the WebAuthn options have no user"));
    };
    let user_display_name = user
        .and_then(|u| u.get("displayName"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(user_name)
        .to_string();
    let mut algorithms: Vec<i32> = o
        .get("pubKeyCredParams")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| {
            p.get("type")
                .and_then(Value::as_str)
                .unwrap_or("public-key")
                == "public-key"
        })
        .filter_map(|p| p.get("alg")?.as_i64()?.try_into().ok())
        .collect();
    if algorithms.is_empty() {
        algorithms = DEFAULT_ALGORITHMS.to_vec();
    }
    let selection = o.get("authenticatorSelection");
    let get = |k: &str| selection.and_then(|s| s.get(k));
    let attachment = match get("authenticatorAttachment").and_then(Value::as_str) {
        Some("platform") => ATTACHMENT_PLATFORM,
        Some("cross-platform") => ATTACHMENT_CROSS_PLATFORM,
        _ => ATTACHMENT_ANY,
    };
    // `residentKey` wins over the older `requireResidentKey`.
    let resident = match get("residentKey").and_then(Value::as_str) {
        Some("required") => Resident::Required,
        Some("preferred") => Resident::Preferred,
        Some("discouraged") => Resident::Discouraged,
        _ if get("requireResidentKey").and_then(Value::as_bool) == Some(true) => Resident::Required,
        _ => Resident::Discouraged,
    };
    let attestation = match o.get("attestation").and_then(Value::as_str) {
        Some("direct" | "enterprise") => ATTESTATION_DIRECT,
        Some("indirect") => ATTESTATION_INDIRECT,
        _ => ATTESTATION_NONE,
    };
    let cred_props = o
        .get("extensions")
        .and_then(|e| e.get("credProps"))
        .and_then(Value::as_bool)
        == Some(true);
    Ok(Registration {
        rp_id,
        rp_name,
        user_id,
        user_name: user_name.to_string(),
        user_display_name,
        challenge,
        algorithms,
        timeout_ms: timeout(&o),
        exclude: descriptors(o.get("excludeCredentials")),
        attachment,
        resident,
        user_verification: user_verification(get("userVerification")),
        attestation,
        cred_props,
    })
}

/// `PublicKeyCredentialRequestOptionsJSON`.
pub fn assertion(options: &str) -> Result<Assertion, DeviceError> {
    let o = object(options)?;
    let challenge = challenge(&o)?;
    let rp_id = o
        .get("rpId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(no_rp)?
        .to_string();
    Ok(Assertion {
        rp_id,
        challenge,
        timeout_ms: timeout(&o),
        allow: descriptors(o.get("allowCredentials")),
        user_verification: user_verification(o.get("userVerification")),
    })
}

/// The client data Windows hashes and the server checks: the ceremony,
/// the challenge, the origin (`https://<rp id>`).
pub fn client_data(create: bool, challenge: &[u8], rp_id: &str) -> String {
    // Written in the order browsers use (`type` first).
    format!(
        r#"{{"type":{},"challenge":{},"origin":{},"crossOrigin":false}}"#,
        json!(if create {
            "webauthn.create"
        } else {
            "webauthn.get"
        }),
        json!(b64url(challenge)),
        json!(format!("https://{rp_id}")),
    )
}

/// The structure versions `webauthn.dll` of API version `api` takes:
/// (make credential options, get assertion options). A newer version than
/// the DLL knows is refused, an older one leaves the newer fields out.
pub fn option_versions(api: u32) -> (u32, u32) {
    match api {
        0..=2 => (3, 4),
        3 => (4, 5),
        4 | 5 => (5, 6),
        6 => (6, 6),
        _ => (7, 7),
    }
}

// --- Answers ---------------------------------------------------------------------------

fn attachment_name(transport: Option<u32>) -> Value {
    match transport {
        Some(t) if t & transport::INTERNAL != 0 => "platform".into(),
        Some(t) if t != 0 => "cross-platform".into(),
        _ => Value::Null,
    }
}

/// What `webauthn.dll` gave for a new credential.
pub struct NewCredential<'a> {
    pub id: &'a [u8],
    pub client_data: &'a str,
    pub attestation_object: &'a [u8],
    pub authenticator_data: &'a [u8],
    /// The transport used, when the DLL says (`dwUsedTransport`).
    pub transport: Option<u32>,
    /// Whether it is discoverable, when the DLL says (`bResidentKey`).
    pub resident: Option<bool>,
}

/// `RegistrationResponseJSON`.
pub fn registration_json(c: &NewCredential, cred_props: bool) -> String {
    let id = b64url(c.id);
    let mut response = json!({
        "clientDataJSON": b64url(c.client_data.as_bytes()),
        "attestationObject": b64url(c.attestation_object),
        "authenticatorData": b64url(c.authenticator_data),
        "transports": c.transport.map(transport_names).unwrap_or_default(),
    });
    if c.transport.is_none() {
        response.as_object_mut().map(|r| r.remove("transports"));
    }
    let mut extensions = Map::new();
    if cred_props && let Some(rk) = c.resident {
        extensions.insert("credProps".into(), json!({ "rk": rk }));
    }
    let mut out = json!({
        "id": id,
        "rawId": id,
        "type": "public-key",
        "response": response,
        "clientExtensionResults": extensions,
    });
    let attachment = attachment_name(c.transport);
    if !attachment.is_null() {
        out["authenticatorAttachment"] = attachment;
    }
    out.to_string()
}

/// What `webauthn.dll` gave for a sign-in.
pub struct Signature<'a> {
    pub id: &'a [u8],
    pub client_data: &'a str,
    pub authenticator_data: &'a [u8],
    pub signature: &'a [u8],
    pub user_handle: &'a [u8],
    pub transport: Option<u32>,
}

/// `AuthenticationResponseJSON`.
pub fn assertion_json(s: &Signature) -> String {
    let id = b64url(s.id);
    let mut response = json!({
        "clientDataJSON": b64url(s.client_data.as_bytes()),
        "authenticatorData": b64url(s.authenticator_data),
        "signature": b64url(s.signature),
    });
    if !s.user_handle.is_empty() {
        response["userHandle"] = b64url(s.user_handle).into();
    }
    let mut out = json!({
        "id": id,
        "rawId": id,
        "type": "public-key",
        "response": response,
        "clientExtensionResults": {},
    });
    let attachment = attachment_name(s.transport);
    if !attachment.is_null() {
        out["authenticatorAttachment"] = attachment;
    }
    out.to_string()
}

// --- Errors ----------------------------------------------------------------------------

/// `NTE_USER_CANCELLED`: the user closed Windows' dialog.
pub const NTE_USER_CANCELLED: i32 = 0x8009_0036_u32 as i32;
/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`: `WebAuthNCancelCurrentOperation`.
pub const ERROR_CANCELLED: i32 = 0x8007_04C7_u32 as i32;

/// The HRESULTs `WebAuthNGetErrorName` names, with their names (the web's
/// `DOMException` names), as `webauthn.h` lists them. `PRISM_DEVICE_REPORT`
/// asks the DLL for each, and `scripts/windows/device-report-check.py`
/// expects these names (a test keeps its table equal to this one).
pub const ERROR_NAMES: &[(i32, &str)] = &[
    (0x8009_000F_u32 as i32, "InvalidStateError"), // NTE_EXISTS
    (0x8007_0032_u32 as i32, "ConstraintError"),   // HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED)
    (0x8009_0029_u32 as i32, "ConstraintError"),   // NTE_NOT_SUPPORTED
    (0x8009_0023_u32 as i32, "ConstraintError"),   // NTE_TOKEN_KEYSET_STORAGE_FULL
    (0x8009_0027_u32 as i32, "NotSupportedError"), // NTE_INVALID_PARAMETER
    (0x8009_0035_u32 as i32, "NotAllowedError"),   // NTE_DEVICE_NOT_FOUND
    (0x8009_0011_u32 as i32, "NotAllowedError"),   // NTE_NOT_FOUND
    (ERROR_CANCELLED, "NotAllowedError"),
    (NTE_USER_CANCELLED, "NotAllowedError"),
    (0x8007_05B4_u32 as i32, "NotAllowedError"), // HRESULT_FROM_WIN32(ERROR_TIMEOUT)
];
/// `E_INVALIDARG`, one HRESULT `webauthn.h` does not name: the DLL calls
/// it `UnknownError`, like any other.
pub const UNNAMED: i32 = 0x8007_0057_u32 as i32;

/// A failed ceremony as Prism's error, like the web's (#189): `name` is
/// what `WebAuthNGetErrorName` calls the HRESULT, the web's `DOMException`
/// name. `NotAllowedError` (dismissed, timed out, no passkey to use) is
/// `Cancelled`, `NotSupportedError` (no authenticator does what the server
/// asks) `Unavailable`; `InvalidStateError` (the passkey being registered
/// is already on this authenticator) and `ConstraintError` keep their name.
pub fn error(hr: i32, name: &str, message: &str) -> DeviceError {
    if hr == NTE_USER_CANCELLED || hr == ERROR_CANCELLED {
        return DeviceError::Cancelled;
    }
    let message = if message.trim().is_empty() {
        format!("WebAuthn error 0x{:08X}", hr as u32)
    } else {
        message.trim().to_string()
    };
    match name {
        "NotAllowedError" => DeviceError::Cancelled,
        "NotSupportedError" => DeviceError::Unavailable,
        "InvalidStateError" | "ConstraintError" => DeviceError::Platform {
            name: name.into(),
            message,
        },
        _ => DeviceError::Failed(format!("passkeys: {message} (0x{:08X})", hr as u32)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn base64url_round_trips() {
        for n in 0..40u8 {
            let bytes: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37) ^ 0xA5).collect();
            assert_eq!(unb64url(&b64url(&bytes)).unwrap(), bytes);
        }
        assert_eq!(b64url(b"\xfb\xff\xbf"), "-_-_");
        assert_eq!(b64url(b"ab"), "YWI");
        // Padded and standard base64 are read too.
        assert_eq!(unb64url("YWI=").unwrap(), b"ab");
        assert_eq!(unb64url("+/+/").unwrap(), b"\xfb\xff\xbf");
        assert_eq!(unb64url("a"), None);
        assert_eq!(unb64url("a*b"), None);
    }

    const CREATE: &str = r#"{
        "rp": {"id": "example.com", "name": "Example"},
        "user": {"id": "dXNlci0x", "name": "ada@example.com", "displayName": "Ada"},
        "challenge": "Y2hhbGxlbmdl",
        "pubKeyCredParams": [{"type": "public-key", "alg": -8}, {"type": "public-key", "alg": -7}, {"type": "other", "alg": -1}],
        "timeout": 60000,
        "excludeCredentials": [{"type": "public-key", "id": "AQID", "transports": ["internal", "hybrid", "warp"]}],
        "authenticatorSelection": {"authenticatorAttachment": "platform", "residentKey": "preferred", "userVerification": "required"},
        "attestation": "direct",
        "extensions": {"credProps": true}
    }"#;

    #[test]
    fn creation_options() {
        let r = registration(CREATE).unwrap();
        assert_eq!(r.rp_id, "example.com");
        assert_eq!(r.rp_name, "Example");
        assert_eq!(r.user_id, b"user-1");
        assert_eq!(r.user_name, "ada@example.com");
        assert_eq!(r.user_display_name, "Ada");
        assert_eq!(r.challenge, b"challenge");
        assert_eq!(r.algorithms, [-8, -7]);
        assert_eq!(r.timeout_ms, 60_000);
        assert_eq!(
            r.exclude,
            [Descriptor {
                id: vec![1, 2, 3],
                transports: transport::INTERNAL | transport::HYBRID
            }]
        );
        assert_eq!(r.attachment, ATTACHMENT_PLATFORM);
        assert_eq!(r.resident, Resident::Preferred);
        assert_eq!(r.user_verification, UV_REQUIRED);
        assert_eq!(r.attestation, ATTESTATION_DIRECT);
        assert!(r.cred_props);
    }

    #[test]
    fn creation_defaults() {
        let r = registration(
            r#"{"rp": {"id": "example.com"}, "user": {"id": "AA", "name": "ada"}, "challenge": "AQ",
                "authenticatorSelection": {"requireResidentKey": true}}"#,
        )
        .unwrap();
        assert_eq!(r.rp_name, "example.com");
        assert_eq!(r.user_display_name, "ada");
        assert_eq!(r.algorithms, DEFAULT_ALGORITHMS);
        assert_eq!(r.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(r.attachment, ATTACHMENT_ANY);
        assert_eq!(r.resident, Resident::Required);
        assert_eq!(r.user_verification, UV_PREFERRED);
        assert_eq!(r.attestation, ATTESTATION_NONE);
        assert!(!r.cred_props);
        // Ten minutes at most.
        let long = r#"{"rp": {"id": "a"}, "user": {"id": "AA", "name": "a"}, "challenge": "AQ", "timeout": 9e9}"#;
        assert_eq!(registration(long).unwrap().timeout_ms, MAX_TIMEOUT_MS);
    }

    #[test]
    fn creation_needs_a_challenge_a_relying_party_and_a_user() {
        let err = |o: &str| match registration(o) {
            Err(DeviceError::Failed(m)) => m,
            other => panic!("{other:?}"),
        };
        assert!(err("[]").contains("not a JSON object"));
        assert!(
            err(r#"{"rp": {"id": "a"}, "user": {"id": "AA", "name": "a"}}"#).contains("challenge")
        );
        assert!(
            err(r#"{"rp": {"name": "A"}, "user": {"id": "AA", "name": "a"}, "challenge": "AQ"}"#)
                .contains("relying party")
        );
        assert!(
            err(r#"{"rp": {"id": "a"}, "user": {"name": "a"}, "challenge": "AQ"}"#)
                .contains("user")
        );
    }

    #[test]
    fn request_options() {
        let a = assertion(
            r#"{"rpId": "example.com", "challenge": "Y2hhbGxlbmdl", "userVerification": "discouraged",
                "allowCredentials": [{"type": "public-key", "id": "AQID", "transports": ["usb", "nfc"]}, {"type": "public-key", "id": "BA"}]}"#,
        )
        .unwrap();
        assert_eq!(a.rp_id, "example.com");
        assert_eq!(a.challenge, b"challenge");
        assert_eq!(a.user_verification, UV_DISCOURAGED);
        assert_eq!(a.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(
            a.allow,
            [
                Descriptor {
                    id: vec![1, 2, 3],
                    transports: transport::USB | transport::NFC
                },
                Descriptor {
                    id: vec![4],
                    transports: 0
                }
            ]
        );
        assert!(matches!(
            assertion(r#"{"challenge": "AQ"}"#),
            Err(DeviceError::Failed(m)) if m.contains("relying party")
        ));
    }

    #[test]
    fn client_data_is_what_browsers_send() {
        let c = client_data(true, b"challenge", "example.com");
        assert_eq!(
            c,
            r#"{"type":"webauthn.create","challenge":"Y2hhbGxlbmdl","origin":"https://example.com","crossOrigin":false}"#
        );
        let v = value(&client_data(false, &[0xfb, 0xff], "a\"b"));
        assert_eq!(v["type"], "webauthn.get");
        assert_eq!(v["challenge"], "-_8");
        assert_eq!(v["origin"], "https://a\"b");
    }

    #[test]
    fn option_versions_follow_the_api() {
        assert_eq!(option_versions(1), (3, 4));
        assert_eq!(option_versions(3), (4, 5));
        assert_eq!(option_versions(4), (5, 6));
        assert_eq!(option_versions(5), (5, 6));
        assert_eq!(option_versions(6), (6, 6));
        assert_eq!(option_versions(7), (7, 7));
        assert_eq!(option_versions(9), (7, 7));
    }

    #[test]
    fn registration_answer() {
        let j = value(&registration_json(
            &NewCredential {
                id: &[1, 2, 3],
                client_data: "{}",
                attestation_object: &[9],
                authenticator_data: &[8, 7],
                transport: Some(transport::INTERNAL),
                resident: Some(true),
            },
            true,
        ));
        assert_eq!(j["id"], "AQID");
        assert_eq!(j["rawId"], "AQID");
        assert_eq!(j["type"], "public-key");
        assert_eq!(j["authenticatorAttachment"], "platform");
        assert_eq!(j["response"]["clientDataJSON"], "e30");
        assert_eq!(j["response"]["attestationObject"], "CQ");
        assert_eq!(j["response"]["authenticatorData"], "CAc");
        assert_eq!(j["response"]["transports"], json!(["internal"]));
        assert_eq!(
            j["clientExtensionResults"],
            json!({"credProps": {"rk": true}})
        );
        // Unknown transport: no attachment, no transports.
        let j = value(&registration_json(
            &NewCredential {
                id: &[1],
                client_data: "{}",
                attestation_object: &[],
                authenticator_data: &[],
                transport: None,
                resident: None,
            },
            true,
        ));
        assert!(j.get("authenticatorAttachment").is_none());
        assert!(j["response"].get("transports").is_none());
        assert_eq!(j["clientExtensionResults"], json!({}));
    }

    #[test]
    fn assertion_answer() {
        let j = value(&assertion_json(&Signature {
            id: &[1],
            client_data: "{}",
            authenticator_data: &[2],
            signature: &[3],
            user_handle: b"user-1",
            transport: Some(transport::USB),
        }));
        assert_eq!(j["id"], "AQ");
        assert_eq!(j["authenticatorAttachment"], "cross-platform");
        assert_eq!(j["response"]["authenticatorData"], "Ag");
        assert_eq!(j["response"]["signature"], "Aw");
        assert_eq!(j["response"]["userHandle"], "dXNlci0x");
        let j = value(&assertion_json(&Signature {
            id: &[1],
            client_data: "{}",
            authenticator_data: &[],
            signature: &[],
            user_handle: &[],
            transport: None,
        }));
        assert!(j["response"].get("userHandle").is_none());
        assert!(j.get("authenticatorAttachment").is_none());
    }

    #[test]
    fn transports() {
        assert_eq!(
            transport_bits(&["usb", "ble", "x"]),
            transport::USB | transport::BLE
        );
        assert_eq!(
            transport_names(transport::USB | transport::HYBRID | transport::INTERNAL),
            ["hybrid", "internal", "usb"]
        );
    }

    #[test]
    fn errors_are_the_webs() {
        assert_eq!(
            error(NTE_USER_CANCELLED, "NotAllowedError", "x"),
            DeviceError::Cancelled
        );
        assert_eq!(
            error(ERROR_CANCELLED, "UnknownError", "x"),
            DeviceError::Cancelled
        );
        assert_eq!(
            error(1, "NotAllowedError", "timed out"),
            DeviceError::Cancelled
        );
        assert_eq!(error(1, "NotSupportedError", "x"), DeviceError::Unavailable);
        assert_eq!(
            error(
                0x8009_000F_u32 as i32,
                "InvalidStateError",
                " The object already exists. "
            ),
            DeviceError::Platform {
                name: "InvalidStateError".into(),
                message: "The object already exists.".into()
            }
        );
        assert_eq!(
            error(0x8007_0057_u32 as i32, "UnknownError", ""),
            DeviceError::Failed("passkeys: WebAuthn error 0x80070057 (0x80070057)".into())
        );
    }

    /// The report checker's table (`KNOWN`) is `ERROR_NAMES` and what
    /// `error` makes of each: a wrong name there fails on every PC.
    #[test]
    fn the_report_checker_knows_each_error() {
        let checker = include_str!("../../../scripts/windows/device-report-check.py");
        let kind = |e: DeviceError| {
            let debug = format!("{e:?}");
            debug
                .split(['(', ' '])
                .next()
                .unwrap_or_default()
                .to_string()
        };
        let all = ERROR_NAMES
            .iter()
            .copied()
            .chain([(UNNAMED, "UnknownError")]);
        for (hr, name) in all {
            let row = format!(
                "\"0x{:08X}\": (\"{name}\", \"{}\")",
                hr as u32,
                kind(error(hr, name, "m"))
            );
            assert!(checker.contains(&row), "device-report-check.py lacks {row}");
        }
        let rows = checker.matches("\": (\"").count();
        assert_eq!(
            rows,
            ERROR_NAMES.len() + 1,
            "device-report-check.py has other rows"
        );
    }
}
