//! Signed JSON Web Tokens for the store APIs: RS256 for Google service
//! accounts, ES256 for App Store Connect API keys (ring, no OpenSSL).

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, RSA_PKCS1_SHA256, RsaKeyPair,
};

/// The DER bytes of a `-----BEGIN PRIVATE KEY-----` (PKCS#8) PEM block.
pub fn pkcs8_der(pem: &str) -> Result<Vec<u8>, String> {
    let body = pem
        .split("-----BEGIN PRIVATE KEY-----")
        .nth(1)
        .and_then(|rest| rest.split("-----END PRIVATE KEY-----").next())
        .ok_or("not a PKCS#8 private key (-----BEGIN PRIVATE KEY-----)")?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD
        .decode(b64)
        .map_err(|e| format!("private key: invalid base64 ({e})"))
}

fn signing_input(header: &serde_json::Value, claims: &serde_json::Value) -> String {
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    )
}

/// `header.claims.signature` signed with an RSA key (SHA256withRSA).
pub fn rs256(
    pkcs8: &[u8],
    header: &serde_json::Value,
    claims: &serde_json::Value,
) -> Result<String, String> {
    let key = RsaKeyPair::from_pkcs8(pkcs8).map_err(|e| format!("RSA private key: {e}"))?;
    let input = signing_input(header, claims);
    let mut sig = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        input.as_bytes(),
        &mut sig,
    )
    .map_err(|_| "RSA signing failed")?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig)))
}

/// `header.claims.signature` signed with a P-256 key (r‖s, as JWS wants).
pub fn es256(
    pkcs8: &[u8],
    header: &serde_json::Value,
    claims: &serde_json::Value,
) -> Result<String, String> {
    let rng = SystemRandom::new();
    let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8, &rng)
        .map_err(|e| format!("P-256 private key: {e}"))?;
    let input = signing_input(header, claims);
    let sig = key
        .sign(&rng, input.as_bytes())
        .map_err(|_| "ECDSA signing failed")?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(sig.as_ref())))
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, KeyPair, RSA_PKCS1_2048_8192_SHA256};

    /// A fresh P-256 key as a PKCS#8 PEM string, like an App Store Connect
    /// `.p8` key.
    pub fn ec_key() -> String {
        let der =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        let b64 = STANDARD.encode(der.as_ref());
        let lines: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|l| std::str::from_utf8(l).unwrap())
            .collect();
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            lines.join("\n")
        )
    }

    /// A fresh 2048-bit RSA key as a PEM string, made by openssl like a
    /// Google service account's (ring cannot make RSA keys). `None` where
    /// there is no openssl (stock Windows): the test is then skipped.
    pub fn rsa_key() -> Option<String> {
        let out = match std::process::Command::new("openssl")
            .args([
                "genpkey",
                "-algorithm",
                "RSA",
                "-pkeyopt",
                "rsa_keygen_bits:2048",
            ])
            .output()
        {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipped: no openssl to make an RSA key");
                return None;
            }
            Err(e) => panic!("openssl: {e}"),
        };
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(String::from_utf8(out.stdout).unwrap())
    }

    fn split(token: &str) -> (String, Vec<u8>) {
        let (input, sig) = token.rsplit_once('.').unwrap();
        (input.to_string(), URL_SAFE_NO_PAD.decode(sig).unwrap())
    }

    #[test]
    fn rs256_verifies() {
        let Some(pem) = rsa_key() else { return };
        let der = pkcs8_der(&pem).unwrap();
        let token = rs256(
            &der,
            &serde_json::json!({"alg": "RS256", "typ": "JWT"}),
            &serde_json::json!({"iss": "a@b"}),
        )
        .unwrap();
        let (input, sig) = split(&token);
        let key = RsaKeyPair::from_pkcs8(&der).unwrap();
        ring::signature::UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.public().as_ref())
            .verify(input.as_bytes(), &sig)
            .unwrap();
        let header = URL_SAFE_NO_PAD
            .decode(input.split('.').next().unwrap())
            .unwrap();
        assert_eq!(header, br#"{"alg":"RS256","typ":"JWT"}"#);
    }

    #[test]
    fn es256_verifies() {
        let pem = ec_key();
        let der = pkcs8_der(&pem).unwrap();
        let token = es256(
            &der,
            &serde_json::json!({"alg": "ES256", "kid": "ABC"}),
            &serde_json::json!({"aud": "appstoreconnect-v1"}),
        )
        .unwrap();
        let (input, sig) = split(&token);
        assert_eq!(sig.len(), 64);
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &SystemRandom::new())
                .unwrap();
        ring::signature::UnparsedPublicKey::new(
            &ECDSA_P256_SHA256_FIXED,
            key.public_key().as_ref(),
        )
        .verify(input.as_bytes(), &sig)
        .unwrap();
    }

    #[test]
    fn rejects_other_pem() {
        assert!(pkcs8_der("-----BEGIN RSA PRIVATE KEY-----\nabc\n").is_err());
        let der =
            pkcs8_der("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n").unwrap();
        assert!(rs256(&der, &serde_json::json!({}), &serde_json::json!({})).is_err());
    }
}
