//! Regression tests for the pre-release security review: each test shows
//! the attack a finding describes and checks that it now fails.

use prism_core::surfaces::PlatformRequest;
use prism_core::{
    AuthRequest, DeviceCall, DeviceError, DeviceValue, PlatformServices, SecureOptions,
};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::security::{SecureStore, base64url, base64url_decode};
use crate::session::WebServices;

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "prism-sec-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// --- S1: WebAuthn ceremonies ----------------------------------------------------------------

const ORIGIN: &str = "http://localhost:3000";

/// A platform authenticator: one P-256 key and its credential id.
struct Authenticator {
    id: &'static str,
    pair: EcdsaKeyPair,
}

impl Authenticator {
    fn new(id: &'static str) -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        Authenticator { id, pair }
    }

    fn spki(&self) -> Vec<u8> {
        let mut spki = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        spki.extend_from_slice(self.pair.public_key().as_ref());
        spki
    }

    /// authenticatorData: RP id hash, flags (UP, UV, and AT when
    /// `attested`), the counter, then the attested credential's id.
    fn auth_data(&self, count: u32, attested: bool) -> Vec<u8> {
        let mut a = Sha256::digest(b"localhost").to_vec();
        a.push(if attested { 0x45 } else { 0x05 });
        a.extend(count.to_be_bytes());
        if attested {
            a.extend([0u8; 16]); // AAGUID
            a.extend((self.id.len() as u16).to_be_bytes());
            a.extend(self.id.as_bytes());
        }
        a
    }

    fn client(kind: &str, challenge: &str) -> Vec<u8> {
        json!({ "type": kind, "challenge": challenge, "origin": ORIGIN })
            .to_string()
            .into_bytes()
    }

    /// What the page answers to `navigator.credentials.create`.
    fn register(&self, challenge: &str) -> Value {
        json!({ "webauthn": {
            "type": "create", "id": base64url(self.id.as_bytes()), "alg": -7,
            "key": base64url(&self.spki()),
            "client": base64url(&Self::client("webauthn.create", challenge)),
            "auth": base64url(&self.auth_data(0, true)),
        }})
    }

    /// What the page answers to `navigator.credentials.get`.
    fn assert(&self, challenge: &str, count: u32) -> Value {
        let auth = self.auth_data(count, false);
        let client = Self::client("webauthn.get", challenge);
        let mut signed = auth.clone();
        signed.extend_from_slice(&Sha256::digest(&client));
        let sig = self.pair.sign(&SystemRandom::new(), &signed).unwrap();
        json!({ "webauthn": {
            "type": "get", "id": base64url(self.id.as_bytes()),
            "client": base64url(&client), "auth": base64url(&auth),
            "sig": base64url(sig.as_ref()),
        }})
    }
}

/// A browser session with a platform authenticator.
fn services(dir: &std::path::Path) -> WebServices {
    let mut s = WebServices::new(None);
    s.secure = Some(SecureStore::new(dir, "sid-1", [7u8; 32]));
    s.origin = ORIGIN.into();
    s.uvpa = true;
    s
}

/// Sends a device call; returns the page's message (if any).
fn call(s: &mut WebServices, id: u64, call: DeviceCall) -> Option<Value> {
    s.handle(PlatformRequest::Device { id, call });
    s.take_outbox().pop()
}

fn get_secret(key: &str) -> DeviceCall {
    DeviceCall::SecureGet {
        key: key.into(),
        prompt: None,
        shared: false,
    }
}

fn set_secret(key: &str, value: &str) -> DeviceCall {
    DeviceCall::SecureSet {
        key: key.into(),
        value: value.into(),
        options: SecureOptions::new().biometric(),
    }
}

fn enrolled_id(s: &WebServices) -> Option<String> {
    let cred = s.secure.as_ref()?.credential()?;
    Some(String::from_utf8(base64url_decode(&cred.id)?).unwrap())
}

/// Enrolls `a` through the first biometric secret, as an app would.
fn enroll(s: &mut WebServices, a: &Authenticator) {
    let msg = call(s, 1, set_secret("pin", "1234")).expect("a registration");
    assert_eq!(msg["mode"], "create", "{msg}");
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    assert_eq!(
        s.finish_ceremony(1, &a.register(&challenge)),
        Ok(DeviceValue::Done)
    );
    assert_eq!(enrolled_id(s).as_deref(), Some(a.id));
}

/// S1: a protected read asks for an assertion by the enrolled credential;
/// a registration answered instead neither replaces it nor opens the gate.
#[test]
fn s1_a_registration_cannot_answer_a_protected_read() {
    let dir = scratch("s1-read");
    let mut s = services(&dir);
    let owner = Authenticator::new("owner");
    let intruder = Authenticator::new("intruder");
    enroll(&mut s, &owner);

    let msg = call(&mut s, 2, get_secret("pin")).expect("an assertion");
    assert_eq!(msg["mode"], "get", "{msg}");
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    let answer = s.finish_ceremony(2, &intruder.register(&challenge));
    assert!(answer.is_err(), "the gate opened: {answer:?}");
    assert_eq!(enrolled_id(&s).as_deref(), Some("owner"));

    // The owner's assertion still opens it.
    let msg = call(&mut s, 3, get_secret("pin")).unwrap();
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    assert_eq!(
        s.finish_ceremony(3, &owner.assert(&challenge, 1)),
        Ok(DeviceValue::Text(Some("1234".into())))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// S1: `authenticate` once a credential is enrolled is an assertion too.
#[test]
fn s1_a_registration_cannot_answer_authenticate() {
    let dir = scratch("s1-auth");
    let mut s = services(&dir);
    let owner = Authenticator::new("owner");
    let intruder = Authenticator::new("intruder");
    enroll(&mut s, &owner);
    let msg = call(&mut s, 2, DeviceCall::Authenticate(AuthRequest::new("Pay"))).unwrap();
    let challenge = msg["challenge"].as_str().unwrap().to_string();
    assert!(
        s.finish_ceremony(2, &intruder.register(&challenge))
            .is_err()
    );
    assert_eq!(enrolled_id(&s).as_deref(), Some("owner"));
    let _ = std::fs::remove_dir_all(dir);
}

/// S1: two enrollments started together: the first one wins, the second
/// fails without replacing it.
#[test]
fn s1_concurrent_enrollments_keep_the_first() {
    let dir = scratch("s1-race");
    let mut s = services(&dir);
    let (a, b) = (Authenticator::new("first"), Authenticator::new("second"));
    let auth = || DeviceCall::Authenticate(AuthRequest::new("Unlock"));
    let one = call(&mut s, 1, auth()).unwrap();
    let two = call(&mut s, 2, auth()).unwrap();
    assert_eq!(
        (one["mode"].as_str(), two["mode"].as_str()),
        (Some("create"), Some("create"))
    );
    let c1 = one["challenge"].as_str().unwrap().to_string();
    let c2 = two["challenge"].as_str().unwrap().to_string();
    assert_eq!(
        s.finish_ceremony(1, &a.register(&c1)),
        Ok(DeviceValue::Done)
    );
    assert!(s.finish_ceremony(2, &b.register(&c2)).is_err());
    assert_eq!(enrolled_id(&s).as_deref(), Some("first"));
    let _ = std::fs::remove_dir_all(dir);
}

/// S1: a biometric secret is stored once this browser's owner enrolled, so
/// reading one never enrolls whoever reads it first.
#[test]
fn s1_reading_a_biometric_secret_never_enrolls() {
    let dir = scratch("s1-tofu");
    let mut s = services(&dir);
    // Stored behind the server's back (or the credential was lost).
    s.secure.as_ref().unwrap().set("pin", "1234", true).unwrap();
    let msg = call(&mut s, 1, get_secret("pin"));
    assert!(msg.is_none(), "asked the page: {msg:?}");
    let (id, answer) = s.local.pop().unwrap();
    assert_eq!(id, 1);
    assert!(matches!(answer, Err(DeviceError::Failed(_))), "{answer:?}");
    // Storing one without an authenticator: unavailable, as on read.
    s.uvpa = false;
    assert!(call(&mut s, 2, set_secret("other", "x")).is_none());
    assert_eq!(s.local.pop(), Some((2, Err(DeviceError::Unavailable))));
    let _ = std::fs::remove_dir_all(dir);
}
