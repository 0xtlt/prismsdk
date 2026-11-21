//! Secrets only Windows Hello opens (`SecureOptions::biometric`, S22): the
//! stored format and the cryptography, apart from the Hello calls
//! (`security`). No Win32 types: tested on every host.
//!
//! Credential Manager has no owner check of its own: every program of the
//! user reads its credentials. So a biometric secret is stored there
//! encrypted, with a key only Windows Hello can give back:
//!
//! - each app has one Hello key, `Prism/<app>/secure-storage`
//!   (`KeyCredentialManager`): an RSA key in the user's Hello container
//!   (the Microsoft Passport key storage provider, in the TPM where there
//!   is one), which signs only after Windows has checked the user (face,
//!   fingerprint or Hello PIN) in its own prompt;
//! - storing draws a random 32-byte challenge and has the key sign
//!   `DOMAIN ‖ challenge`. Hello signs with RSA PKCS#1 v1.5 and SHA-256,
//!   which is deterministic: the same key signs the same message to the
//!   same bytes, so the key gives the same signature again, and nothing
//!   else can make it. The signature is checked against the key's public
//!   half before anything is stored, which also proves it is PKCS#1 v1.5;
//! - the value's key is HKDF-SHA256 (salt: the challenge; secret: the
//!   signature; info: `DOMAIN`, the version, the app and the key's name),
//!   and the value is sealed with AES-256-GCM, the same context as
//!   associated data: a sealed value copied under another name does not
//!   open;
//! - Credential Manager keeps [`MARK`] and the hex of version ‖ challenge ‖
//!   nonce ‖ sealed value. Reading has the key sign the challenge again
//!   (Windows asks the user), derives the key and opens the value.
//!
//! A failed step stores nothing, so the previous secret stays until its
//! replacement is sealed. Resetting Hello (a new PIN after « I forgot my
//! PIN », Hello removed) deletes the key: the secrets no longer open
//! (`Failed`), and the app stores them again, like after new fingers on
//! Apple platforms. Another program of the same user can open the same
//! Hello key and ask for the same signature, but Windows then asks the
//! user, as it does for the app.
//!
//! Why this design: `KeyCredentialManager` is the documented Hello API
//! for desktop apps, packaged or not, and it only signs; the other ways
//! to protect a value (DPAPI, Credential Manager itself) let any program
//! of the user read it without a prompt. Sealing with the public half and
//! opening with the private one would spare the prompt when storing, but
//! Hello keys do not decrypt through any documented API.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::DeviceError;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::hkdf::{HKDF_SHA256, Salt};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};

/// The start of a sealed value in Credential Manager.
pub const MARK: &str = "\u{1}prism-hello\u{1}";
/// What Hello signs and the keys derive from, before the challenge.
const DOMAIN: &[u8] = b"prism.secure-storage.hello.v1\0";
const VERSION: u8 = 1;
const CHALLENGE: usize = 32;
const NONCE: usize = 12;

/// The app's Hello key.
pub trait HelloKey {
    /// The key's signature of `message`; Windows asks the user first.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, DeviceError>;
    /// The key's public half: a PKCS#1 `RSAPublicKey`, DER.
    fn public_key(&self) -> Result<Vec<u8>, DeviceError>;
}

/// The name of `app`'s Hello key.
pub fn key_name(app: &str) -> String {
    format!("Prism/{app}/secure-storage")
}

/// Whether Credential Manager's `stored` value is sealed by [`seal`].
pub fn is_sealed(stored: &str) -> bool {
    stored.starts_with(MARK)
}

/// `value` sealed with `hello` for the secret `key` of `app`: the text
/// Credential Manager keeps. Hello signs once.
pub fn seal(
    hello: &impl HelloKey,
    app: &str,
    key: &str,
    value: &str,
) -> Result<String, DeviceError> {
    let rng = SystemRandom::new();
    let mut challenge = [0u8; CHALLENGE];
    let mut nonce = [0u8; NONCE];
    rng.fill(&mut challenge)
        .and_then(|_| rng.fill(&mut nonce))
        .map_err(|_| failed("no randomness"))?;
    let message = message(&challenge);
    let signature = hello.sign(&message)?;
    let public = hello.public_key()?;
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &public)
        .verify(&message, &signature)
        .map_err(|_| {
            failed("Windows Hello's signature is not RSA PKCS#1 v1.5 with SHA-256: nothing stored")
        })?;
    let mut sealed = value.as_bytes().to_vec();
    value_key(&challenge, &signature, app, key)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(context(app, key)),
            &mut sealed,
        )
        .map_err(|_| failed("sealing failed"))?;
    let mut bytes = vec![VERSION];
    bytes.extend(challenge);
    bytes.extend(nonce);
    bytes.extend(sealed);
    Ok(format!("{MARK}{}", hex(&bytes)))
}

/// The value [`seal`] sealed for the secret `key` of `app`: Hello signs its
/// challenge again. A damaged value fails before Hello asks the user.
pub fn open(
    hello: &impl HelloKey,
    app: &str,
    key: &str,
    stored: &str,
) -> Result<String, DeviceError> {
    let (challenge, nonce, sealed) = parse(stored)?;
    let signature = hello.sign(&message(&challenge))?;
    let mut sealed = sealed;
    let value = value_key(&challenge, &signature, app, key)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(context(app, key)),
            &mut sealed,
        )
        .map_err(|_| {
            failed("the app's Windows Hello key does not open this secret (Hello was reset?): store it again")
        })?;
    String::from_utf8(value.to_vec()).map_err(|_| failed("the stored secret is not UTF-8"))
}

fn failed(why: &str) -> DeviceError {
    DeviceError::Failed(why.into())
}

/// What Hello signs.
fn message(challenge: &[u8; CHALLENGE]) -> Vec<u8> {
    [DOMAIN, challenge].concat()
}

/// The secret's context: HKDF's info, AES-GCM's associated data. Lengths
/// first, so that no two names share one.
fn context(app: &str, key: &str) -> Vec<u8> {
    let mut c = DOMAIN.to_vec();
    c.push(VERSION);
    for part in [app, key] {
        c.extend((part.len() as u32).to_be_bytes());
        c.extend(part.as_bytes());
    }
    c
}

fn value_key(
    challenge: &[u8; CHALLENGE],
    signature: &[u8],
    app: &str,
    key: &str,
) -> Result<LessSafeKey, DeviceError> {
    let info = context(app, key);
    let info = [info.as_slice()];
    let prk = Salt::new(HKDF_SHA256, challenge).extract(signature);
    let okm = prk
        .expand(&info, &AES_256_GCM)
        .map_err(|_| failed("key derivation failed"))?;
    Ok(LessSafeKey::new(UnboundKey::from(okm)))
}

type Parsed = ([u8; CHALLENGE], [u8; NONCE], Vec<u8>);

fn parse(stored: &str) -> Result<Parsed, DeviceError> {
    let damaged = || failed("the stored secret is damaged: store it again");
    let bytes = stored
        .strip_prefix(MARK)
        .and_then(unhex)
        .ok_or_else(damaged)?;
    let (&version, rest) = bytes.split_first().ok_or_else(damaged)?;
    if version != VERSION {
        return Err(failed("the stored secret has an unknown format"));
    }
    if rest.len() < CHALLENGE + NONCE + AES_256_GCM.tag_len() {
        return Err(damaged());
    }
    let (challenge, rest) = rest.split_at(CHALLENGE);
    let (nonce, sealed) = rest.split_at(NONCE);
    Ok((
        challenge.try_into().map_err(|_| damaged())?,
        nonce.try_into().map_err(|_| damaged())?,
        sealed.to_vec(),
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
    use std::cell::Cell;

    /// Hello's key as a test RSA key (`testdata/fake-hello-key.pk8`, made
    /// for these tests only), signing like Hello: PKCS#1 v1.5, SHA-256.
    struct FakeHello {
        key: RsaKeyPair,
        signatures: Cell<usize>,
    }

    impl FakeHello {
        fn new() -> Self {
            Self {
                key: RsaKeyPair::from_pkcs8(include_bytes!("../testdata/fake-hello-key.pk8"))
                    .unwrap(),
                signatures: Cell::new(0),
            }
        }
    }

    impl HelloKey for FakeHello {
        fn sign(&self, message: &[u8]) -> Result<Vec<u8>, DeviceError> {
            self.signatures.set(self.signatures.get() + 1);
            let mut signature = vec![0; self.key.public().modulus_len()];
            self.key
                .sign(
                    &RSA_PKCS1_SHA256,
                    &SystemRandom::new(),
                    message,
                    &mut signature,
                )
                .unwrap();
            Ok(signature)
        }
        fn public_key(&self) -> Result<Vec<u8>, DeviceError> {
            Ok(self.key.public().as_ref().to_vec())
        }
    }

    /// A key that answers otherwise: another key after Hello was reset,
    /// a randomised signature, the user cancelling.
    struct Other<F: Fn(&[u8]) -> Result<Vec<u8>, DeviceError>>(FakeHello, F);

    impl<F: Fn(&[u8]) -> Result<Vec<u8>, DeviceError>> HelloKey for Other<F> {
        fn sign(&self, message: &[u8]) -> Result<Vec<u8>, DeviceError> {
            (self.1)(message)
        }
        fn public_key(&self) -> Result<Vec<u8>, DeviceError> {
            self.0.public_key()
        }
    }

    #[test]
    fn a_sealed_secret_opens_with_hello_only() {
        let hello = FakeHello::new();
        for value in ["s3cret ✓", "", &"0123456789abcdef✓".repeat(400)] {
            let stored = seal(&hello, "app", "token", value).unwrap();
            assert!(is_sealed(&stored));
            assert!(!stored.contains("s3cret") && !stored.contains("0123456789abcdef"));
            assert_eq!(open(&hello, "app", "token", &stored).as_deref(), Ok(value));
        }
        // One signature to store, one to read, each time.
        assert_eq!(hello.signatures.get(), 6);
        assert!(!is_sealed("s3cret"));
        assert!(!is_sealed("\u{1}prism-gated\u{1}s3cret"));
    }

    #[test]
    fn each_secret_has_its_own_challenge() {
        let hello = FakeHello::new();
        let a = seal(&hello, "app", "token", "same").unwrap();
        let b = seal(&hello, "app", "token", "same").unwrap();
        assert_ne!(a, b);
        assert_ne!(parse(&a).unwrap().0, parse(&b).unwrap().0);
        assert_eq!(open(&hello, "app", "token", &b).as_deref(), Ok("same"));
    }

    #[test]
    fn another_key_or_another_name_does_not_open_it() {
        let hello = FakeHello::new();
        let stored = seal(&hello, "app", "token", "s3cret").unwrap();
        // Hello reset: a new key signs to other bytes.
        let reset = Other(FakeHello::new(), |m: &[u8]| {
            let mut s = FakeHello::new().sign(m)?;
            s[0] ^= 1;
            Ok(s)
        });
        assert!(matches!(
            open(&reset, "app", "token", &stored),
            Err(DeviceError::Failed(m)) if m.contains("store it again")
        ));
        // The value copied under another secret's or another app's name.
        for (app, key) in [("app", "other"), ("other", "token"), ("ap", "ptoken")] {
            assert!(matches!(
                open(&hello, app, key, &stored),
                Err(DeviceError::Failed(_))
            ));
        }
    }

    #[test]
    fn a_damaged_secret_fails_before_hello_asks() {
        let hello = FakeHello::new();
        let stored = seal(&hello, "app", "token", "s3cret").unwrap();
        let asked = hello.signatures.get();
        let cut = &stored[..stored.len() - 2 * (AES_256_GCM.tag_len() + 7)];
        let odd = &stored[..stored.len() - 1];
        let not_hex = format!("{MARK}zz");
        let future = format!("{MARK}02{}", &stored[MARK.len() + 2..]);
        for bad in [cut, odd, MARK, &not_hex, &future] {
            assert!(matches!(
                open(&hello, "app", "token", bad),
                Err(DeviceError::Failed(_))
            ));
        }
        assert_eq!(hello.signatures.get(), asked);
        // A changed byte of the sealed value: Hello asks, the value does
        // not open.
        let mut flipped = stored.clone().into_bytes();
        let last = flipped.len() - 1;
        flipped[last] = if flipped[last] == b'0' { b'1' } else { b'0' };
        let flipped = String::from_utf8(flipped).unwrap();
        assert!(matches!(
            open(&hello, "app", "token", &flipped),
            Err(DeviceError::Failed(_))
        ));
    }

    /// A signature that changes each time could never open the value
    /// again: nothing is sealed with it.
    #[test]
    fn a_signature_that_does_not_verify_stores_nothing() {
        let random = Other(FakeHello::new(), |_: &[u8]| {
            let mut s = vec![0; 256];
            SystemRandom::new().fill(&mut s).unwrap();
            Ok(s)
        });
        assert!(matches!(
            seal(&random, "app", "token", "s3cret"),
            Err(DeviceError::Failed(m)) if m.contains("nothing stored")
        ));
        // A signature of another message.
        let wrong = Other(FakeHello::new(), |m: &[u8]| {
            FakeHello::new().sign(&[m, b"!"].concat())
        });
        assert!(seal(&wrong, "app", "token", "s3cret").is_err());
    }

    #[test]
    fn hello_s_answer_comes_back() {
        let hello = FakeHello::new();
        let stored = seal(&hello, "app", "token", "s3cret").unwrap();
        for e in [
            DeviceError::Cancelled,
            DeviceError::Denied,
            DeviceError::Unavailable,
        ] {
            let no = Other(FakeHello::new(), |_: &[u8]| Err(e.clone()));
            assert_eq!(seal(&no, "app", "token", "s3cret"), Err(e.clone()));
            assert_eq!(open(&no, "app", "token", &stored), Err(e.clone()));
        }
    }

    #[test]
    fn hex_round_trip() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(unhex(&hex(&bytes)), Some(bytes));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("é0"), None);
    }
}
