//! Web Push: message encryption (RFC 8291, `aes128gcm` from RFC 8188) and
//! VAPID authentication (RFC 8292). Used for browsers and for UnifiedPush,
//! whose distributors relay Web Push messages to Android and Linux apps.
//!
//! The receiving side is here too ([`ReceiverKeys`], [`decrypt`]): Prism's
//! UnifiedPush receivers decrypt on the device.

use std::time::{Duration, SystemTime};

use aes_gcm::aead::{Aead, KeyInit};
use hmac::Mac;
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde_json::json;

use crate::{HttpRequest, Push, PushError, Subscription, Urgency, b64, unb64};

type HmacSha256 = hmac::Hmac<sha2::Sha256>;

/// Record size written in the header (one record: anything above the
/// message size works; 4096 like browsers).
const RECORD_SIZE: u32 = 4096;

/// `N` bytes from the system's secure generator (a failure there leaves
/// nothing safe to do: it panics, as key generation does).
fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the system's random number generator");
    bytes
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <HmacSha256 as KeyInit>::new_from_slice(key).expect("HMAC takes any key size");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

/// The content encryption key and nonce (RFC 8291 §3.4).
fn derive(
    ecdh_secret: &[u8],
    auth: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> ([u8; 16], [u8; 12]) {
    let prk_key = hmac(auth, &[ecdh_secret]);
    let ikm = hmac(&prk_key, &[b"WebPush: info\0", ua_public, as_public, &[1]]);
    let prk = hmac(salt, &[&ikm]);
    let cek = hmac(&prk, &[b"Content-Encoding: aes128gcm\0", &[1]]);
    let nonce = hmac(&prk, &[b"Content-Encoding: nonce\0", &[1]]);
    (
        cek[..16].try_into().unwrap(),
        nonce[..12].try_into().unwrap(),
    )
}

fn public_bytes(key: &PublicKey) -> Vec<u8> {
    key.to_sec1_point(false).as_bytes().to_vec()
}

fn parse_public(bytes: &[u8]) -> Result<PublicKey, PushError> {
    PublicKey::from_sec1_bytes(bytes)
        .map_err(|_| PushError::Config("not a P-256 public key (p256dh)".into()))
}

/// Encrypts `plaintext` for a subscription's keys (`p256dh`, `auth`, both
/// decoded), with a fresh key pair and salt.
pub fn encrypt(plaintext: &[u8], p256dh: &[u8], auth: &[u8]) -> Result<Vec<u8>, PushError> {
    encrypt_with(plaintext, p256dh, auth, &SecretKey::generate(), random())
}

/// [`encrypt`] with a given sender key and salt (test vectors).
pub fn encrypt_with(
    plaintext: &[u8],
    p256dh: &[u8],
    auth: &[u8],
    sender: &SecretKey,
    salt: [u8; 16],
) -> Result<Vec<u8>, PushError> {
    // 86-byte header + delimiter + 16-byte tag must fit in 4096 (RFC 8291 §4).
    if plaintext.len() > 3993 {
        return Err(PushError::Config(format!(
            "payload of {} bytes: Web Push allows 3993",
            plaintext.len()
        )));
    }
    let ua = parse_public(p256dh)?;
    let as_public = public_bytes(&sender.public_key());
    let shared = p256::ecdh::diffie_hellman(sender.to_nonzero_scalar(), ua.as_affine());
    let (cek, nonce) = derive(
        shared.raw_secret_bytes(),
        auth,
        &public_bytes(&ua),
        &as_public,
        &salt,
    );
    let mut record = plaintext.to_vec();
    record.push(2); // last (and only) record, no padding
    let sealed = aes_gcm::Aes128Gcm::new_from_slice(&cek)
        .unwrap()
        .encrypt(&nonce.into(), record.as_slice())
        .map_err(|_| PushError::Config("encryption failed".into()))?;
    let mut out = Vec::with_capacity(86 + sealed.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(&as_public);
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// A device's Web Push keys: generated once, the public half and the auth
/// secret go to the server in the subscription.
#[derive(Clone)]
pub struct ReceiverKeys {
    pub secret: SecretKey,
    pub auth: [u8; 16],
}

impl ReceiverKeys {
    pub fn generate() -> Self {
        ReceiverKeys {
            secret: SecretKey::generate(),
            auth: random(),
        }
    }

    /// `p256dh` for the subscription (base64url, uncompressed point).
    pub fn p256dh(&self) -> String {
        b64(&public_bytes(&self.secret.public_key()))
    }

    /// `auth` for the subscription (base64url).
    pub fn auth(&self) -> String {
        b64(&self.auth)
    }

    /// 48 bytes (private key, then auth secret) for storage.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = self.secret.to_bytes().to_vec();
        o.extend_from_slice(&self.auth);
        o
    }

    /// This device's keys, kept in the file `path`: read, or made once and
    /// kept there, private from creation (see
    /// [`secret_file::load_or_create`](crate::secret_file::load_or_create)).
    /// An error when they cannot be read or kept: a subscription announced
    /// with keys that are lost could never be decrypted.
    pub fn load_or_create(path: &std::path::Path) -> std::io::Result<Self> {
        let bytes = crate::secret_file::load_or_create(
            path,
            |b| Self::from_bytes(b).is_some(),
            || Self::generate().to_bytes(),
        )?;
        Self::from_bytes(&bytes).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "not Web Push keys")
        })
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 48 {
            return None;
        }
        Some(ReceiverKeys {
            secret: SecretKey::from_slice(&bytes[..32]).ok()?,
            auth: bytes[32..].try_into().ok()?,
        })
    }

    pub fn decrypt(&self, body: &[u8]) -> Result<Vec<u8>, PushError> {
        decrypt(body, &self.secret, &self.auth)
    }
}

/// Decrypts an `aes128gcm` Web Push message (one record) with the
/// receiver's private key and auth secret.
pub fn decrypt(body: &[u8], receiver: &SecretKey, auth: &[u8]) -> Result<Vec<u8>, PushError> {
    let bad = |m: &str| PushError::Config(format!("web push message: {m}"));
    if body.len() < 21 {
        return Err(bad("too short"));
    }
    let salt = &body[..16];
    let id_len = body[20] as usize;
    let start = 21 + id_len;
    if body.len() < start + 17 {
        return Err(bad("truncated"));
    }
    let as_public = parse_public(&body[21..start])?;
    let shared = p256::ecdh::diffie_hellman(receiver.to_nonzero_scalar(), as_public.as_affine());
    let (cek, nonce) = derive(
        shared.raw_secret_bytes(),
        auth,
        &public_bytes(&receiver.public_key()),
        &public_bytes(&as_public),
        salt,
    );
    let mut plain = aes_gcm::Aes128Gcm::new_from_slice(&cek)
        .unwrap()
        .decrypt(&nonce.into(), &body[start..])
        .map_err(|_| bad("authentication failed (wrong keys?)"))?;
    // Padding: zeros after the delimiter, which must be 2 (last record).
    while plain.last() == Some(&0) {
        plain.pop();
    }
    if plain.pop() != Some(2) {
        return Err(bad("bad padding delimiter"));
    }
    Ok(plain)
}

/// The application server's VAPID key (ECDSA P-256).
#[derive(Clone)]
pub struct VapidKey {
    key: p256::ecdsa::SigningKey,
}

impl VapidKey {
    /// A new key: keep [`private_base64`](Self::private_base64) secret on
    /// the server and give [`public_key`](Self::public_key) to the web app
    /// (`PRISM_VAPID_PUBLIC_KEY`) and to UnifiedPush.
    pub fn generate() -> Self {
        VapidKey {
            key: p256::ecdsa::SigningKey::generate(),
        }
    }

    /// The raw 32-byte private key, base64url (the common format of Web Push
    /// libraries).
    pub fn from_base64(private: &str) -> Result<Self, PushError> {
        let bytes = unb64(private)?;
        p256::ecdsa::SigningKey::from_slice(&bytes)
            .map(|key| VapidKey { key })
            .map_err(|_| PushError::Config("VAPID key: expected 32 bytes".into()))
    }

    /// A PKCS#8 PEM (`openssl ecparam -name prime256v1 -genkey | openssl pkcs8 -topk8 -nocrypt`).
    pub fn from_pem(pem: &str) -> Result<Self, PushError> {
        use p256::pkcs8::DecodePrivateKey;
        p256::ecdsa::SigningKey::from_pkcs8_pem(pem)
            .map(|key| VapidKey { key })
            .map_err(|e| PushError::Config(format!("VAPID key: {e}")))
    }

    pub fn private_base64(&self) -> String {
        b64(&self.key.to_bytes())
    }

    /// The public key (base64url, uncompressed point): the browser's
    /// `applicationServerKey`.
    pub fn public_key(&self) -> String {
        b64(self.key.verifying_key().to_sec1_point(false).as_bytes())
    }

    /// The `Authorization` header for `endpoint` (RFC 8292): a JWT for the
    /// endpoint's origin, valid 12 hours.
    pub fn authorization(&self, endpoint: &str, subject: &str, now: SystemTime) -> String {
        let claims = json!({
            "aud": origin(endpoint),
            "exp": crate::unix(now) + 12 * 3600,
            "sub": subject,
        });
        let token = crate::jwt::es256(&json!({"typ": "JWT", "alg": "ES256"}), &claims, &self.key);
        format!("vapid t={token}, k={}", self.public_key())
    }
}

/// `https://push.example.net:8443/p/abc` → `https://push.example.net:8443`.
fn origin(url: &str) -> String {
    let after = url.find("://").map_or(0, |i| i + 3);
    let end = url[after..].find('/').map_or(url.len(), |i| after + i);
    url[..end].to_string()
}

/// The request delivering `push` to a Web Push or UnifiedPush subscription.
///
/// The endpoint must be a public HTTPS URL on port 443 (not `localhost`,
/// not a private or reserved address). Your HTTP client must also refuse
/// a name that resolves to such an address, and redirects: the subscriber
/// chose this URL. `Sender` does both, and can trust a
/// private distributor's origin.
pub fn request(
    sub: &Subscription,
    push: &Push,
    vapid: Option<(&VapidKey, &str)>,
    now: SystemTime,
) -> Result<HttpRequest, PushError> {
    crate::endpoint::Endpoint::parse(&sub.token)?.check_public()?;
    build(sub, push, vapid, now)
}

/// [`request`], with the endpoint already checked.
pub(crate) fn build(
    sub: &Subscription,
    push: &Push,
    vapid: Option<(&VapidKey, &str)>,
    now: SystemTime,
) -> Result<HttpRequest, PushError> {
    let keys = sub
        .keys
        .as_ref()
        .ok_or_else(|| PushError::Config("this subscription has no keys".into()))?;
    let body = encrypt(
        push.payload().to_string().as_bytes(),
        &unb64(&keys.p256dh)?,
        &unb64(&keys.auth)?,
    )?;
    let ttl = push.ttl.unwrap_or(Duration::from_secs(24 * 3600)).as_secs();
    let mut headers = vec![
        ("TTL".to_string(), ttl.to_string()),
        ("Content-Encoding".into(), "aes128gcm".into()),
        ("Content-Type".into(), "application/octet-stream".into()),
        (
            "Urgency".into(),
            match push.urgency {
                Urgency::VeryLow => "very-low",
                Urgency::Low => "low",
                Urgency::Normal => "normal",
                Urgency::High => "high",
            }
            .into(),
        ),
    ];
    if let Some(topic) = &push.collapse_id {
        // RFC 8030 §5.4: at most 32 base64url characters.
        let topic: String = topic
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .take(32)
            .collect();
        headers.push(("Topic".into(), topic));
    }
    if let Some((key, subject)) = vapid {
        headers.push((
            "Authorization".into(),
            key.authorization(&sub.token, subject, now),
        ));
    }
    Ok(HttpRequest {
        url: sub.token.clone(),
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::signature::Verifier;

    use super::*;

    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const UA_PRIVATE: &str = "q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94";
    const UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AS_PRIVATE: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const PLAINTEXT: &str = "When I grow up, I want to be a watermelon";
    /// RFC 8291 §5: the encrypted message.
    const MESSAGE: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    #[test]
    fn rfc8291_intermediate_values() {
        let ua = SecretKey::from_slice(&unb64(UA_PRIVATE).unwrap()).unwrap();
        let sender = SecretKey::from_slice(&unb64(AS_PRIVATE).unwrap()).unwrap();
        assert_eq!(b64(&public_bytes(&ua.public_key())), UA_PUBLIC);
        let shared =
            p256::ecdh::diffie_hellman(sender.to_nonzero_scalar(), ua.public_key().as_affine());
        assert_eq!(
            b64(shared.raw_secret_bytes()),
            "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs"
        );
        let auth = unb64(AUTH).unwrap();
        assert_eq!(
            b64(&hmac(&auth, &[shared.raw_secret_bytes()])),
            "Snr3JMxaHVDXHWJn5wdC52WjpCtd2EIEGBykDcZW32k"
        );
        let (cek, nonce) = derive(
            shared.raw_secret_bytes(),
            &auth,
            &unb64(UA_PUBLIC).unwrap(),
            &public_bytes(&sender.public_key()),
            &unb64(SALT).unwrap(),
        );
        assert_eq!(b64(&cek), "oIhVW04MRdy2XN9CiKLxTg");
        assert_eq!(b64(&nonce), "4h_95klXJ5E_qnoN");
    }

    #[test]
    fn rfc8291_encrypts_the_example() {
        let sender = SecretKey::from_slice(&unb64(AS_PRIVATE).unwrap()).unwrap();
        let out = encrypt_with(
            PLAINTEXT.as_bytes(),
            &unb64(UA_PUBLIC).unwrap(),
            &unb64(AUTH).unwrap(),
            &sender,
            unb64(SALT).unwrap().try_into().unwrap(),
        )
        .unwrap();
        assert_eq!(b64(&out), MESSAGE);
    }

    #[test]
    fn rfc8291_decrypts_the_example() {
        let ua = SecretKey::from_slice(&unb64(UA_PRIVATE).unwrap()).unwrap();
        let plain = decrypt(&unb64(MESSAGE).unwrap(), &ua, &unb64(AUTH).unwrap()).unwrap();
        assert_eq!(plain, PLAINTEXT.as_bytes());
        // A wrong auth secret fails.
        assert!(decrypt(&unb64(MESSAGE).unwrap(), &ua, &[0; 16]).is_err());
    }

    #[test]
    fn round_trips_with_fresh_keys() {
        let keys = ReceiverKeys::generate();
        let body = encrypt(
            b"{\"title\":\"Hi\"}",
            &unb64(&keys.p256dh()).unwrap(),
            &unb64(&keys.auth()).unwrap(),
        )
        .unwrap();
        let again = ReceiverKeys::from_bytes(&keys.to_bytes()).unwrap();
        assert_eq!(again.decrypt(&body).unwrap(), b"{\"title\":\"Hi\"}");
    }

    /// RFC 8292 §2.4: the example token verifies with the example key.
    #[test]
    fn rfc8292_example_token_verifies() {
        let token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9.eyJhdWQiOiJodHRwczovL3B1c2guZXhhbXBsZS5uZXQiLCJleHAiOjE0NTM1MjM3NjgsInN1YiI6Im1haWx0bzpwdXNoQGV4YW1wbGUuY29tIn0.i3CYb7t4xfxCDquptFOepC9GAu_HLGkMlMuCGSK2rpiUfnK9ojFwDXb1JrErtmysazNjjvW2L9OkSSHzvoD1oA";
        let k = "BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs";
        let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(&unb64(k).unwrap()).unwrap();
        let (signed, sig) = token.rsplit_once('.').unwrap();
        let sig = p256::ecdsa::Signature::from_slice(&unb64(sig).unwrap()).unwrap();
        key.verify(signed.as_bytes(), &sig).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&unb64(signed.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example.net");
    }

    #[test]
    fn vapid_headers_are_valid_tokens() {
        let key = VapidKey::generate();
        let again = VapidKey::from_base64(&key.private_base64()).unwrap();
        assert_eq!(again.public_key(), key.public_key());
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let header = key.authorization(
            "https://fcm.googleapis.com/fcm/send/abc:def",
            "mailto:ops@example.com",
            now,
        );
        let rest = header.strip_prefix("vapid t=").unwrap();
        let (token, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, key.public_key());
        let (signed, sig) = token.rsplit_once('.').unwrap();
        let verifier = p256::ecdsa::VerifyingKey::from_sec1_bytes(&unb64(k).unwrap()).unwrap();
        verifier
            .verify(
                signed.as_bytes(),
                &p256::ecdsa::Signature::from_slice(&unb64(sig).unwrap()).unwrap(),
            )
            .unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&unb64(signed.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        assert_eq!(claims["exp"], 1_700_000_000 + 12 * 3600);
        assert_eq!(claims["sub"], "mailto:ops@example.com");
    }

    #[test]
    fn requests_are_encrypted_for_the_subscription() {
        let keys = ReceiverKeys::generate();
        let sub = Subscription {
            service: crate::Service::UnifiedPush,
            token: "https://ntfy.example/upAbC?up=1".into(),
            keys: Some(crate::Keys {
                p256dh: keys.p256dh(),
                auth: keys.auth(),
            }),
            topic: None,
            sandbox: false,
        };
        let vapid = VapidKey::generate();
        let push = Push::new()
            .title("Hi")
            .link("/inbox")
            .collapse_id("inbox")
            .urgency(Urgency::High);
        let r = request(
            &sub,
            &push,
            Some((&vapid, "mailto:a@b.c")),
            SystemTime::now(),
        )
        .unwrap();
        assert_eq!(r.url, sub.token);
        assert_eq!(r.header("Content-Encoding"), Some("aes128gcm"));
        assert_eq!(r.header("TTL"), Some("86400"));
        assert_eq!(r.header("Urgency"), Some("high"));
        assert_eq!(r.header("Topic"), Some("inbox"));
        assert!(r.header("Authorization").unwrap().starts_with("vapid t="));
        let plain: serde_json::Value =
            serde_json::from_slice(&keys.decrypt(&r.body).unwrap()).unwrap();
        assert_eq!(
            plain,
            json!({"title": "Hi", "link": "/inbox", "id": "inbox"})
        );
    }
}
