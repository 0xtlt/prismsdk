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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
use std::time::{Duration, Instant};

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_ASN1, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// --- Secure storage ---------------------------------------------------------------------

/// The server's key for secrets at rest: `PRISM_SECRET_KEY`, else the one
/// in the data directory, made by the first worker that needs it.
pub(crate) fn server_key(data_root: &Path) -> Result<[u8; 32], String> {
    static KEYS: OnceLock<Mutex<HashMap<PathBuf, [u8; 32]>>> = OnceLock::new();
    let mut keys = KEYS.get_or_init(Default::default).lock().unwrap();
    if let Some(k) = keys.get(data_root) {
        return Ok(*k);
    }
    let key = key_from(std::env::var("PRISM_SECRET_KEY").ok(), data_root)?;
    keys.insert(data_root.to_path_buf(), key);
    Ok(key)
}

/// The key `configured` (64 hex digits), else the data directory's: read,
/// or created once for every worker sharing the directory. A configured key
/// that does not parse, or a key file that does not hold one, is an error:
/// secrets are never sealed with a key that would be lost.
fn key_from(configured: Option<String>, data_root: &Path) -> Result<[u8; 32], String> {
    if let Some(hex) = configured {
        return hex_decode(hex.trim())
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or_else(|| "PRISM_SECRET_KEY must be 64 hex digits".to_string());
    }
    let path = data_root.join(".secret-key");
    let read = |path: &Path| -> Result<Option<[u8; 32]>, String> {
        match std::fs::read(path) {
            Ok(b) => <[u8; 32]>::try_from(b)
                .map(Some)
                .map_err(|_| format!("{}: not a key (32 bytes)", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    };
    if let Some(k) = read(&path)? {
        return Ok(k);
    }
    let mut k = [0u8; 32];
    SystemRandom::new()
        .fill(&mut k)
        .map_err(|_| "no random source")?;
    private_dir(data_root).map_err(|e| format!("{}: {e}", data_root.display()))?;
    // Written aside (private from the start), then linked in place: the link
    // fails if another worker made the key first, and then its key is used.
    let tmp = write_aside(&path, &k).map_err(|e| format!("{}: {e}", path.display()))?;
    let linked = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => Ok(k),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            read(&path)?.ok_or_else(|| format!("{}: vanished", path.display()))
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Creates `dir` (and its parents) readable by this user only.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
}

/// A new file beside `path`, under a name of its own, readable by this user
/// only from its creation, holding `data`.
fn write_aside(path: &Path, data: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp~", random_id()));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let written = open.open(&tmp).and_then(|mut f| {
        f.write_all(data)?;
        f.sync_all()
    });
    match written {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Replaces `path` with `data` at once: readers see the old file or the new.
fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp = write_aside(path, data).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// The file in a browser's `secure/` naming the id its secrets are sealed
/// for, once the browser's id changed (S10): they keep it.
const BINDING: &str = ".binding";

/// Moves the data of the browser `old` (`from`, its session directory) to
/// its new id's directory `to` (S10): one rename, so nothing is left half
/// moved and pages still on the old id find nothing there. Its secrets
/// keep the id they were sealed for ([`BINDING`]). A directory that does
/// not exist has nothing to move. Windows refuses to rename a directory
/// while a file inside is open (another page of the old id writing): the
/// rename is tried a few times, then fails, and the data stays.
pub(crate) fn move_browser(from: &Path, to: &Path, old: &str) -> Result<(), String> {
    let secure = from.join("secure");
    if secure.is_dir() && !secure.join(BINDING).exists() {
        write_private(&secure.join(BINDING), old.as_bytes())?;
    }
    let mut tries = 0u64;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !from.exists() => return Ok(()),
            Err(_) if tries < 5 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * tries));
            }
            Err(e) => return Err(format!("the browser's data cannot move: {e}")),
        }
    }
}

/// One browser's secrets: `<session>/secure/<sha256(key)>`, each
/// `nonce ‖ AES-256-GCM(flags ‖ value)`, bound to the session and key.
pub(crate) struct SecureStore {
    dir: PathBuf,
    /// The id the secrets are sealed for: the browser's, or the one it had
    /// when they were first written ([`BINDING`]).
    sid: String,
    /// The server key, or why there is none (every operation fails then).
    key: Result<LessSafeKey, String>,
}

pub(crate) struct Secret {
    pub value: String,
    /// Reading it needs WebAuthn first.
    pub biometric: bool,
}

impl SecureStore {
    pub fn new(session_dir: &Path, sid: &str, key: Result<[u8; 32], String>) -> Self {
        let dir = session_dir.join("secure");
        let bound = std::fs::read_to_string(dir.join(BINDING))
            .ok()
            .filter(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()));
        SecureStore {
            sid: bound.unwrap_or_else(|| sid.to_string()),
            dir,
            key: key.map(|k| LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &k).expect("AES key"))),
        }
    }

    fn cipher(&self) -> Result<&LessSafeKey, String> {
        self.key
            .as_ref()
            .map_err(|e| format!("secure storage unavailable: {e}"))
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
        self.cipher()?
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.aad(key)),
                &mut data,
            )
            .map_err(|_| "encryption failed")?;
        private_dir(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let mut file = nonce.to_vec();
        file.extend(data);
        write_private(&self.path(key), &file)
    }

    pub fn get(&self, key: &str) -> Result<Option<Secret>, String> {
        let cipher = self.cipher()?;
        let mut file = match std::fs::read(self.path(key)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("secret unreadable: {e}")),
        };
        if file.len() < 12 + 16 + 1 {
            return Err("corrupt secret".into());
        }
        let mut data = file.split_off(12);
        let nonce = Nonce::try_assume_unique_for_key(&file).map_err(|_| "corrupt secret")?;
        let plain = cipher
            .open_in_place(nonce, Aad::from(self.aad(key)), &mut data)
            .map_err(|_| "secret unreadable (PRISM_SECRET_KEY changed?)")?;
        Ok(Some(Secret {
            biometric: plain[0] == 1,
            value: String::from_utf8_lossy(&plain[1..]).into_owned(),
        }))
    }

    /// Removes a secret; one that is not there is removed already.
    pub fn delete(&self, key: &str) -> Result<(), String> {
        match std::fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("secret not deleted: {e}")),
        }
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

    /// Updates the enrolled credential (its signature counter).
    pub fn save_credential(&self, c: &Credential) -> Result<(), String> {
        private_dir(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let v = json!({ "id": c.id, "alg": c.alg, "key": base64url(&c.key), "count": c.count });
        write_private(&self.credential_path(), v.to_string().as_bytes())
    }

    /// Enrolls this browser's first credential. Fails when one is already
    /// enrolled, even by a ceremony that finished meanwhile: an enrolled
    /// credential is never replaced through WebAuthn.
    pub fn enroll(&self, c: &Credential) -> Result<(), String> {
        static ENROLLING: Mutex<()> = Mutex::new(());
        let _one_at_a_time = ENROLLING.lock().unwrap_or_else(|e| e.into_inner());
        private_dir(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let path = self.credential_path();
        if path.exists() {
            return Err("webauthn: a credential is already enrolled".into());
        }
        let v = json!({ "id": c.id, "alg": c.alg, "key": base64url(&c.key), "count": c.count });
        // Written aside, then linked in place: the link fails if another
        // process enrolled meanwhile, and nobody reads a partial file.
        let tmp = write_aside(&path, v.to_string().as_bytes()).map_err(|e| e.to_string())?;
        let linked = std::fs::hard_link(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
        linked.map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                "webauthn: a credential is already enrolled".into()
            }
            _ => e.to_string(),
        })
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

/// The operation the server chose for a ceremony: the page cannot answer
/// another one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Enroll this browser's first credential.
    Register,
    /// An assertion by the enrolled credential with this id.
    Assert(String),
}

/// What the server expects from one ceremony.
pub(crate) struct Ceremony {
    pub challenge: String,
    pub origin: String,
    pub rp_id: String,
    pub mode: Mode,
}

impl Ceremony {
    pub fn new(origin: &str, mode: Mode) -> Self {
        let mut c = [0u8; 32];
        SystemRandom::new().fill(&mut c).expect("system random");
        Ceremony {
            challenge: base64url(&c),
            origin: origin.to_string(),
            rp_id: rp_id(origin),
            mode,
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

/// The credential id in authenticatorData's attested credential data
/// (flag AT: AAGUID, id length, id, then the COSE key).
fn attested_credential_id(auth: &[u8]) -> Option<&[u8]> {
    if auth.get(32)? & 0x40 == 0 {
        return None;
    }
    let len = u16::from_be_bytes([*auth.get(53)?, *auth.get(54)?]) as usize;
    auth.get(55..55 + len)
}

/// A registration (`navigator.credentials.create`): the new credential.
pub(crate) fn verify_registration(c: &Ceremony, r: &Value) -> Result<Credential, String> {
    if c.mode != Mode::Register {
        return Err("webauthn: wrong ceremony".into());
    }
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
    // The id the authenticator attested, not another one.
    if attested_credential_id(&auth) != base64url_decode(id).as_deref() {
        return Err("webauthn: credential id mismatch".into());
    }
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
    if c.mode != Mode::Assert(cred.id.clone()) {
        return Err("webauthn: wrong ceremony".into());
    }
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

/// How long a sign-in popup has to come back; its `state` is forgotten
/// after.
pub(crate) const REDIRECT_TTL: Duration = Duration::from_secs(10 * 60);
/// Sign-ins one session, and all of them, may have waiting at once.
const REDIRECTS_PER_SESSION: usize = 8;
const REDIRECTS: usize = 4096;

/// A session's call waiting for a provider's redirect.
struct Waiting {
    tx: mpsc::Sender<crate::session::Inbound>,
    /// The session ([`session_key`]) and its call.
    session: u64,
    id: u64,
    since: Instant,
}

/// Sign-ins waiting for their redirect, by `state`.
fn waiting() -> MutexGuard<'static, HashMap<String, Waiting>> {
    static W: OnceLock<Mutex<HashMap<String, Waiting>>> = OnceLock::new();
    W.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// A number of its own for each live session: its sign-ins are filed under
/// it, as call numbers start again in each session.
pub(crate) fn session_key() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Files call `id` of `session` under `state` until the provider
/// redirects. `false` when the session has too many sign-ins waiting, or
/// the server, or another session waits on that `state`.
pub(crate) fn wait_for_redirect(
    state: String,
    tx: mpsc::Sender<crate::session::Inbound>,
    session: u64,
    id: u64,
) -> bool {
    let now = Instant::now();
    let mut w = waiting();
    w.retain(|_, p| now.duration_since(p.since) < REDIRECT_TTL);
    let mine = w.values().filter(|p| p.session == session).count();
    let taken = w.get(&state).is_some_and(|p| p.session != session);
    if taken || mine >= REDIRECTS_PER_SESSION || w.len() >= REDIRECTS {
        return false;
    }
    let since = now;
    w.insert(
        state,
        Waiting {
            tx,
            session,
            id,
            since,
        },
    );
    true
}

/// Forgets call `id` of `session` (its page could not open the popup).
pub(crate) fn forget_redirect(session: u64, id: u64) {
    waiting().retain(|_, p| !(p.session == session && p.id == id));
}

/// Forgets every sign-in of `session` (it ended).
pub(crate) fn forget_session(session: u64) {
    waiting().retain(|_, p| p.session != session);
}

/// The sign-in waiting on `state`, once: `None` if there is none or it
/// waited [`REDIRECT_TTL`] by `now`.
pub(crate) fn take_redirect(
    state: &str,
    now: Instant,
) -> Option<(mpsc::Sender<crate::session::Inbound>, u64)> {
    let p = waiting().remove(state)?;
    (now.duration_since(p.since) < REDIRECT_TTL).then_some((p.tx, p.id))
}

/// `GET <base>/_prism/auth/callback?…`: hands the URL to its session.
pub(crate) fn redirect_page(query: &str, nonce: Option<&crate::CspNonce>) -> String {
    let state = query_map(query).remove("state").unwrap_or_default();
    let found = take_redirect(&state, Instant::now());
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
    use crate::temp_dir::TempDir;
    use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    /// S25: workers sharing a data directory (processes; here threads, past
    /// the per-process cache) all seal with the one key that is persisted.
    #[test]
    fn s25_workers_share_one_persisted_key() {
        let root = TempDir::new("s25-workers");
        for round in 0..20 {
            let dir = root.join(round.to_string());
            let start = std::sync::Arc::new(std::sync::Barrier::new(8));
            let keys: Vec<[u8; 32]> = (0..8)
                .map(|_| {
                    let (dir, start) = (dir.clone(), start.clone());
                    std::thread::spawn(move || {
                        start.wait();
                        key_from(None, &dir).unwrap()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|t| t.join().unwrap())
                .collect();
            let persisted = std::fs::read(dir.join(".secret-key")).unwrap();
            assert!(keys.iter().all(|k| k[..] == persisted[..]), "round {round}");
            let names: Vec<_> = std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(names, [".secret-key"], "round {round}");
        }
    }

    #[test]
    fn s25_a_key_that_does_not_parse_is_an_error() {
        let dir = TempDir::new("s25-bad-key");
        assert!(key_from(Some("not hex".into()), &dir).is_err());
        assert!(key_from(Some("ab".repeat(31)), &dir).is_err());
        assert_eq!(key_from(Some("ab".repeat(32)), &dir).unwrap(), [0xab; 32]);
        assert!(!dir.exists(), "a configured key writes no key file");
        // A key file that holds no key is left alone, not replaced.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".secret-key"), "short").unwrap();
        assert!(key_from(None, &dir).is_err());
        assert_eq!(std::fs::read(dir.join(".secret-key")).unwrap(), b"short");
        // Without a key, the store fails instead of sealing with another.
        let s = SecureStore::new(
            &dir,
            "sid",
            Err("PRISM_SECRET_KEY must be 64 hex digits".into()),
        );
        let err = s.set("token", "v", false).unwrap_err();
        assert!(err.contains("PRISM_SECRET_KEY"), "{err}");
        assert!(s.get("token").is_err());
    }

    #[test]
    fn s25_failed_writes_and_deletes_are_errors() {
        let dir = TempDir::new("s25-failures");
        let s = SecureStore::new(&dir, "sid", Ok([7; 32]));
        s.set("token", "old", false).unwrap();
        // The secret's path becomes a directory with something in it: it can
        // be neither replaced nor removed.
        std::fs::remove_file(s.path("token")).unwrap();
        std::fs::create_dir_all(s.path("token").join("x")).unwrap();
        assert!(s.set("token", "new", false).is_err(), "reported saved");
        assert!(s.delete("token").is_err(), "reported deleted");
        // Nothing left behind by the failed write.
        let names: Vec<_> = std::fs::read_dir(dir.join("secure"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with("tmp~")), "{names:?}");
    }

    #[cfg(unix)]
    #[test]
    fn s25_secrets_are_private_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = TempDir::new("s25-private");
        std::fs::create_dir_all(&dir).unwrap();
        // Created private, not made private after (whatever the umask).
        let tmp = write_aside(&dir.join("k"), b"x").unwrap();
        assert_eq!(mode(&tmp), 0o600);
        let data = dir.join("data");
        key_from(None, &data).unwrap();
        assert_eq!(mode(&data), 0o700);
        assert_eq!(mode(&data.join(".secret-key")), 0o600);
        let s = SecureStore::new(&data.join("sid"), "sid", Ok([7; 32]));
        s.set("token", "v", false).unwrap();
        assert_eq!(mode(&data.join("sid/secure")), 0o700);
        assert_eq!(mode(&s.path("token")), 0o600);
    }

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
        let dir = TempDir::new("secure");
        let key = [7u8; 32];
        let a = SecureStore::new(&dir.join("a"), "sid-a", Ok(key));
        a.set("token", "s3cret", false).unwrap();
        a.set("pin", "1234", true).unwrap();
        let raw = std::fs::read(a.path("token")).unwrap();
        assert!(!raw.windows(6).any(|w| w == b"s3cret"));
        let t = a.get("token").unwrap().unwrap();
        assert_eq!((t.value.as_str(), t.biometric), ("s3cret", false));
        assert!(a.get("pin").unwrap().unwrap().biometric);
        assert!(a.get("missing").unwrap().is_none());
        // Another session (or key) cannot open it.
        let b = SecureStore::new(&dir.join("a"), "sid-b", Ok(key));
        assert!(b.get("token").is_err());
        let c = SecureStore::new(&dir.join("a"), "sid-a", Ok([8u8; 32]));
        assert!(c.get("token").is_err());
        a.delete("token").unwrap();
        assert!(a.get("token").unwrap().is_none());
        a.delete("token").unwrap();
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
        let reg = Ceremony::new(origin, Mode::Register);
        // Attested credential data: AAGUID, the id's length, the id.
        let mut attested = auth_data("localhost", 0x45, 0);
        attested.extend([0u8; 16]);
        let raw_id = base64url_decode("cred1").unwrap();
        attested.extend((raw_id.len() as u16).to_be_bytes());
        attested.extend(&raw_id);
        let response = json!({
            "id": "cred1", "alg": -7, "key": base64url(&spki),
            "client": base64url(&client("webauthn.create", &reg.challenge, origin)),
            "auth": base64url(&attested),
        });
        // Another id than the attested one.
        let mut other = response.clone();
        other["id"] = json!(base64url(b"other"));
        assert!(verify_registration(&reg, &other).is_err());
        let mut cred = verify_registration(&reg, &response).unwrap();
        assert_eq!(cred.key, pair.public_key().as_ref());

        let get = Ceremony::new(origin, Mode::Assert("cred1".into()));
        // Each ceremony takes only the operation the server chose.
        assert!(verify_registration(&get, &response).is_err());
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
