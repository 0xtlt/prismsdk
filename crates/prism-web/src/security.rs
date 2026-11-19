//! Secrets, the device owner and sign-in on the web, where the app runs on
//! the server:
//!
//! - **`secure_storage`** lives on the server, per browser (the `HttpOnly`
//!   session cookie), encrypted at rest with AES-256-GCM. The key is
//!   `PRISM_SECRET_KEY` (64 hex digits) or one generated in the data
//!   directory. Secrets never reach the browser.
//! - **`authenticate`** is WebAuthn with the browser's platform
//!   authenticator and user verification (Touch ID, Windows Hello, an
//!   Android lock…). The first time registers a credential for this browser;
//!   the server checks every assertion (challenge, origin, RP id, UV flag,
//!   ES256 or RS256 signature).
//! - **OAuth** opens the provider in a popup; it redirects to
//!   `<base>/_prism/auth/callback`, which hands the URL to the session that
//!   started it (matched by `state`) and closes the popup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, mpsc};

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_ASN1, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// --- Secure storage ---------------------------------------------------------------------

/// The server's key for secrets at rest.
pub(crate) fn server_key(data_root: &Path) -> [u8; 32] {
    static KEYS: OnceLock<Mutex<HashMap<PathBuf, [u8; 32]>>> = OnceLock::new();
    let mut keys = KEYS.get_or_init(Default::default).lock().unwrap();
    if let Some(k) = keys.get(data_root) {
        return *k;
    }
    let key = std::env::var("PRISM_SECRET_KEY")
        .ok()
        .and_then(|h| hex_decode(h.trim()))
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .unwrap_or_else(|| {
            let path = data_root.join(".secret-key");
            if let Some(k) = std::fs::read(&path)
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
            {
                return k;
            }
            let mut k = [0u8; 32];
            SystemRandom::new().fill(&mut k).expect("system random");
            let _ = std::fs::create_dir_all(data_root);
            write_private(&path, &k);
            k
        });
    keys.insert(data_root.to_path_buf(), key);
    key
}

fn write_private(path: &Path, data: &[u8]) {
    let tmp = path.with_extension("tmp~");
    let ok = std::fs::write(&tmp, data).is_ok() && {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, path).is_ok()
    };
    if !ok {
        eprintln!("prism-web: could not write {}", path.display());
    }
}

/// One browser's secrets: `<session>/secure/<sha256(key)>`, each
/// `nonce ‖ AES-256-GCM(flags ‖ value)`, bound to the session and key.
pub(crate) struct SecureStore {
    dir: PathBuf,
    sid: String,
    key: LessSafeKey,
}

pub(crate) struct Secret {
    pub value: String,
    /// Reading it needs WebAuthn first.
    pub biometric: bool,
}

impl SecureStore {
    pub fn new(session_dir: &Path, sid: &str, key: [u8; 32]) -> Self {
        SecureStore {
            dir: session_dir.join("secure"),
            sid: sid.to_string(),
            key: LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).expect("AES key")),
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(hex(&Sha256::digest(key.as_bytes())))
    }

    fn aad(&self, key: &str) -> Vec<u8> {
        format!("{}\0{key}", self.sid).into_bytes()
    }

    pub fn set(&self, key: &str, value: &str, biometric: bool) -> Result<(), String> {
        let mut nonce = [0u8; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| "no random source")?;
        let mut data = Vec::with_capacity(value.len() + 1);
        data.push(u8::from(biometric));
        data.extend_from_slice(value.as_bytes());
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.aad(key)),
                &mut data,
            )
            .map_err(|_| "encryption failed")?;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let mut file = nonce.to_vec();
        file.extend(data);
        write_private(&self.path(key), &file);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Option<Secret>, String> {
        let Ok(mut file) = std::fs::read(self.path(key)) else {
            return Ok(None);
        };
        if file.len() < 12 + 16 + 1 {
            return Err("corrupt secret".into());
        }
        let mut data = file.split_off(12);
        let nonce = Nonce::try_assume_unique_for_key(&file).map_err(|_| "corrupt secret")?;
        let plain = self
            .key
            .open_in_place(nonce, Aad::from(self.aad(key)), &mut data)
            .map_err(|_| "secret unreadable (PRISM_SECRET_KEY changed?)")?;
        Ok(Some(Secret {
            biometric: plain[0] == 1,
            value: String::from_utf8_lossy(&plain[1..]).into_owned(),
        }))
    }

    pub fn delete(&self, key: &str) {
        let _ = std::fs::remove_file(self.path(key));
    }

    fn credential_path(&self) -> PathBuf {
        self.dir.join("webauthn.json")
    }

    /// This browser's WebAuthn credential, once registered.
    pub fn credential(&self) -> Option<Credential> {
        let v: Value = serde_json::from_slice(&std::fs::read(self.credential_path()).ok()?).ok()?;
        Some(Credential {
            id: v.get("id")?.as_str()?.to_string(),
            alg: v.get("alg")?.as_i64()?,
            key: base64url_decode(v.get("key")?.as_str()?)?,
            count: v.get("count").and_then(Value::as_u64).unwrap_or(0) as u32,
        })
    }

    pub fn save_credential(&self, c: &Credential) {
        let _ = std::fs::create_dir_all(&self.dir);
        let v = json!({ "id": c.id, "alg": c.alg, "key": base64url(&c.key), "count": c.count });
        write_private(&self.credential_path(), v.to_string().as_bytes());
    }
}

// --- WebAuthn -------------------------------------------------------------------------------

/// A registered public key: `alg` -7 (ES256, `key` an uncompressed P-256
/// point) or -257 (RS256, `key` a PKCS#1 RSAPublicKey).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Credential {
    pub id: String,
    pub alg: i64,
    pub key: Vec<u8>,
    pub count: u32,
}

/// What the server expects from one ceremony.
pub(crate) struct Ceremony {
    pub challenge: String,
    pub origin: String,
    pub rp_id: String,
}

impl Ceremony {
    pub fn new(origin: &str) -> Self {
        let mut c = [0u8; 32];
        SystemRandom::new().fill(&mut c).expect("system random");
        Ceremony {
            challenge: base64url(&c),
            origin: origin.to_string(),
            rp_id: rp_id(origin),
        }
    }
}

/// The RP id: the origin's host.
pub(crate) fn rp_id(origin: &str) -> String {
    let host = origin.split("://").nth(1).unwrap_or(origin);
    let host = host.split('/').next().unwrap_or(host);
    match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6).to_string(),
        None => host.rsplit_once(':').map_or(host, |(h, _)| h).to_string(),
    }
}

fn b64(v: &Value, k: &str) -> Result<Vec<u8>, String> {
    v.get(k)
        .and_then(Value::as_str)
        .and_then(base64url_decode)
        .ok_or_else(|| format!("webauthn: missing {k}"))
}

/// Checks clientDataJSON and authenticatorData; returns the sign count.
fn check_common(c: &Ceremony, kind: &str, client: &[u8], auth: &[u8]) -> Result<u32, String> {
    let data: Value = serde_json::from_slice(client).map_err(|_| "webauthn: bad client data")?;
    if data.get("type").and_then(Value::as_str) != Some(kind) {
        return Err("webauthn: wrong ceremony".into());
    }
    if data.get("challenge").and_then(Value::as_str) != Some(c.challenge.as_str()) {
        return Err("webauthn: challenge mismatch".into());
    }
    if data.get("origin").and_then(Value::as_str) != Some(c.origin.as_str()) {
        return Err("webauthn: origin mismatch".into());
    }
    if auth.len() < 37 || auth[..32] != Sha256::digest(c.rp_id.as_bytes())[..] {
        return Err("webauthn: relying party mismatch".into());
    }
    let flags = auth[32];
    // User present and verified (biometrics or the device's PIN).
    if flags & 0x01 == 0 || flags & 0x04 == 0 {
        return Err("denied".into());
    }
    Ok(u32::from_be_bytes([auth[33], auth[34], auth[35], auth[36]]))
}

/// A registration (`navigator.credentials.create`): the new credential.
pub(crate) fn verify_registration(c: &Ceremony, r: &Value) -> Result<Credential, String> {
    let client = b64(r, "client")?;
    let auth = b64(r, "auth")?;
    let count = check_common(c, "webauthn.create", &client, &auth)?;
    let alg = r.get("alg").and_then(Value::as_i64).unwrap_or(0);
    let spki = b64(r, "key")?;
    let key = spki_key(&spki).ok_or("webauthn: unreadable public key")?;
    match alg {
        -7 if key.len() == 65 && key[0] == 4 => {}
        -257 => {}
        _ => return Err(format!("webauthn: unsupported algorithm {alg}")),
    }
    let id = r
        .get("id")
        .and_then(Value::as_str)
        .ok_or("webauthn: missing id")?;
    Ok(Credential {
        id: id.to_string(),
        alg,
        key,
        count,
    })
}

/// An assertion (`navigator.credentials.get`) by `cred`; updates its count.
pub(crate) fn verify_assertion(
    c: &Ceremony,
    cred: &mut Credential,
    r: &Value,
) -> Result<(), String> {
    if r.get("id").and_then(Value::as_str) != Some(cred.id.as_str()) {
        return Err("webauthn: unknown credential".into());
    }
    let client = b64(r, "client")?;
    let auth = b64(r, "auth")?;
    let sig = b64(r, "sig")?;
    let count = check_common(c, "webauthn.get", &client, &auth)?;
    let mut signed = auth.clone();
    signed.extend_from_slice(&Sha256::digest(&client));
    let alg: &dyn ring::signature::VerificationAlgorithm = match cred.alg {
        -7 => &ECDSA_P256_SHA256_ASN1,
        -257 => &RSA_PKCS1_2048_8192_SHA256,
        _ => return Err("webauthn: unsupported algorithm".into()),
    };
    UnparsedPublicKey::new(alg, &cred.key)
        .verify(&signed, &sig)
        .map_err(|_| "denied")?;
    // A counter that goes back means a cloned authenticator (0: not counted).
    if count != 0 && cred.count != 0 && count <= cred.count {
        return Err("webauthn: signature counter went back".into());
    }
    cred.count = count;
    Ok(())
}

/// The key bits of a DER SubjectPublicKeyInfo (`SEQUENCE { algorithm,
/// BIT STRING }`): an EC point, or an RSAPublicKey.
fn spki_key(der: &[u8]) -> Option<Vec<u8>> {
    fn tlv(d: &[u8]) -> Option<(u8, &[u8], &[u8])> {
        let tag = *d.first()?;
        let first = *d.get(1)? as usize;
        let (len, head) = if first < 0x80 {
            (first, 2)
        } else {
            let n = first & 0x7f;
            let mut len = 0usize;
            for i in 0..n {
                len = len << 8 | *d.get(2 + i)? as usize;
            }
            (len, 2 + n)
        };
        let body = d.get(head..head + len)?;
        Some((tag, body, &d[head + len..]))
    }
    let (tag, spki, _) = tlv(der)?;
    if tag != 0x30 {
        return None;
    }
    let (_, _, rest) = tlv(spki)?; // AlgorithmIdentifier
    let (tag, bits, _) = tlv(rest)?;
    (tag == 0x03 && bits.first() == Some(&0)).then(|| bits[1..].to_vec())
}

// --- OAuth popups -------------------------------------------------------------------------

/// Sessions waiting for a provider's redirect, by `state`.
type Waiting = HashMap<String, (mpsc::Sender<crate::session::Inbound>, u64)>;

fn waiting() -> &'static Mutex<Waiting> {
    static W: OnceLock<Mutex<Waiting>> = OnceLock::new();
    W.get_or_init(Default::default)
}

pub(crate) fn wait_for_redirect(state: String, tx: mpsc::Sender<crate::session::Inbound>, id: u64) {
    waiting().lock().unwrap().insert(state, (tx, id));
}

pub(crate) fn forget_redirect(id: u64) {
    waiting().lock().unwrap().retain(|_, (_, i)| *i != id);
}

/// `GET <base>/_prism/auth/callback?…`: hands the URL to its session.
pub(crate) fn redirect_page(query: &str, nonce: Option<&crate::CspNonce>) -> String {
    let state = query_map(query).remove("state").unwrap_or_default();
    let found = waiting().lock().unwrap().remove(&state);
    let delivered = found.is_some_and(|(tx, id)| {
        tx.send(crate::session::Inbound::AuthRedirect(
            id,
            format!("/_prism/auth/callback?{query}"),
        ))
        .is_ok()
    });
    let message = if delivered {
        "Signed in. You can close this window."
    } else {
        "This sign-in has expired. Close this window and try again."
    };
    // No `style` attribute, and the host's nonce: a strict CSP allows it.
    let n = crate::host::nonce_attr(nonce);
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Sign-in</title>\
<style{n}>p{{font:16px system-ui;margin:40px;text-align:center}}</style>\
<p>{message}</p>\
<script{n}>if (window.opener) window.close();</script>"
    )
}

/// A query string's parameters, decoded (`+` is a space).
fn query_map(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((
                k.to_string(),
                crate::app::percent_decode(&v.replace('+', " ")),
            ))
        })
        .collect()
}

/// `state` of an authorization URL.
pub(crate) fn state_of(url: &str) -> Option<String> {
    query_map(url.split_once('?')?.1.split('#').next()?).remove("state")
}

// --- Encoding helpers -----------------------------------------------------------------------

/// 16 random bytes, base64url (a WebAuthn user handle).
pub(crate) fn random_id() -> String {
    let mut b = [0u8; 16];
    SystemRandom::new().fill(&mut b).expect("system random");
    base64url(&b)
}

pub(crate) fn base64url(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub(crate) fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    s.len()
        .is_multiple_of(2)
        .then(|| {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
                .collect()
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    fn auth_data(rp: &str, flags: u8, count: u32) -> Vec<u8> {
        let mut a = Sha256::digest(rp.as_bytes()).to_vec();
        a.push(flags);
        a.extend(count.to_be_bytes());
        a
    }

    fn client(kind: &str, challenge: &str, origin: &str) -> Vec<u8> {
        json!({ "type": kind, "challenge": challenge, "origin": origin })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn secrets_are_encrypted_and_bound_to_the_session() {
        let dir = std::env::temp_dir().join(format!("prism-secure-{}", std::process::id()));
        let key = [7u8; 32];
        let a = SecureStore::new(&dir.join("a"), "sid-a", key);
        a.set("token", "s3cret", false).unwrap();
        a.set("pin", "1234", true).unwrap();
        let raw = std::fs::read(a.path("token")).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"s3cret"));
        let t = a.get("token").unwrap().unwrap();
        assert_eq!((t.value.as_str(), t.biometric), ("s3cret", false));
        assert!(a.get("pin").unwrap().unwrap().biometric);
        assert!(a.get("missing").unwrap().is_none());
        // Another session (or key) cannot open it.
        let b = SecureStore::new(&dir.join("a"), "sid-b", key);
        assert!(b.get("token").is_err());
        let c = SecureStore::new(&dir.join("a"), "sid-a", [8u8; 32]);
        assert!(c.get("token").is_err());
        a.delete("token");
        assert!(a.get("token").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verifies_webauthn_registration_and_assertion() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        // SubjectPublicKeyInfo for P-256: fixed header + the point.
        let mut spki = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        spki.extend_from_slice(pair.public_key().as_ref());

        let origin = "http://localhost:3000";
        assert_eq!(rp_id(origin), "localhost");
        let reg = Ceremony::new(origin);
        let response = json!({
            "id": "cred1", "alg": -7, "key": base64url(&spki),
            "client": base64url(&client("webauthn.create", &reg.challenge, origin)),
            "auth": base64url(&auth_data("localhost", 0x45, 0)),
        });
        let mut cred = verify_registration(&reg, &response).unwrap();
        assert_eq!(cred.key, pair.public_key().as_ref());

        let get = Ceremony::new(origin);
        let sign = |flags: u8, count: u32, challenge: &str, origin: &str| {
            let auth = auth_data("localhost", flags, count);
            let client = client("webauthn.get", challenge, origin);
            let mut msg = auth.clone();
            msg.extend_from_slice(&Sha256::digest(&client));
            let sig = pair.sign(&rng, &msg).unwrap();
            json!({ "id": "cred1", "client": base64url(&client), "auth": base64url(&auth), "sig": base64url(sig.as_ref()) })
        };
        assert!(verify_assertion(&get, &mut cred, &sign(0x05, 3, &get.challenge, origin)).is_ok());
        assert_eq!(cred.count, 3);
        // Replayed counter, no user verification, another challenge or origin.
        assert!(verify_assertion(&get, &mut cred, &sign(0x05, 3, &get.challenge, origin)).is_err());
        assert_eq!(
            verify_assertion(&get, &mut cred, &sign(0x01, 9, &get.challenge, origin)),
            Err("denied".into())
        );
        assert!(verify_assertion(&get, &mut cred, &sign(0x05, 9, "other", origin)).is_err());
        assert!(
            verify_assertion(
                &get,
                &mut cred,
                &sign(0x05, 9, &get.challenge, "https://evil.example")
            )
            .is_err()
        );
        // A tampered signature.
        let mut bad = sign(0x05, 9, &get.challenge, origin);
        bad["auth"] = json!(base64url(&auth_data("localhost", 0x05, 10)));
        assert!(verify_assertion(&get, &mut cred, &bad).is_err());
    }

    #[test]
    fn reads_the_state_and_rp_id() {
        assert_eq!(
            state_of("https://id.example/auth?client_id=a&state=x%2By#f").as_deref(),
            Some("x+y")
        );
        assert_eq!(rp_id("https://app.example.com"), "app.example.com");
        assert_eq!(rp_id("http://[::1]:8080"), "::1");
        assert_eq!(
            base64url_decode(&base64url(b"hello!?")).unwrap(),
            b"hello!?"
        );
        assert_eq!(hex_decode("0aff"), Some(vec![10, 255]));
    }
}
