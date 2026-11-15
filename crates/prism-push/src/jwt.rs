//! Compact JWS (RFC 7515): ES256 for APNs and VAPID, RS256 for Google's
//! OAuth assertion.

use p256::ecdsa::signature::Signer;
use serde_json::Value;

use crate::b64;

/// `header.claims.signature`, signed with ECDSA P-256 / SHA-256 (the
/// signature is R || S, 64 bytes).
pub(crate) fn es256(header: &Value, claims: &Value, key: &p256::ecdsa::SigningKey) -> String {
    let input = signing_input(header, claims);
    let sig: p256::ecdsa::Signature = key.sign(input.as_bytes());
    format!("{input}.{}", b64(&sig.to_bytes()))
}

/// Signed with RSASSA-PKCS1-v1_5 / SHA-256 (ring: constant time).
#[cfg(feature = "send")]
pub(crate) fn rs256(header: &Value, claims: &Value, key: &ring::signature::RsaKeyPair) -> String {
    let input = signing_input(header, claims);
    format!("{input}.{}", b64(&rs256_sign(input.as_bytes(), key)))
}

#[cfg(feature = "send")]
pub(crate) fn rs256_sign(input: &[u8], key: &ring::signature::RsaKeyPair) -> Vec<u8> {
    let mut sig = vec![0; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input,
        &mut sig,
    )
    .expect("RSA signing");
    sig
}

fn signing_input(header: &Value, claims: &Value) -> String {
    format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use p256::ecdsa::signature::Verifier;

    use crate::unb64;

    /// RFC 7515 Appendix A.3: ES256.
    #[test]
    fn verifies_rfc7515_es256_example() {
        let input = "eyJhbGciOiJFUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ";
        let sig = unb64(
            "DtEhU3ljbEg8L38VWAfUAqOyKAM6-Xx-F4GawxaepmXFCgfTjDxw5djxLa8ISlSApmWQxfKTUJqPP3-Kg6NU1Q",
        )
        .unwrap();
        let d = unb64("jpsQnnGQmL-YBIffH1136cspYG6-0iY7X1fCE9-E9LI").unwrap();
        let key = p256::ecdsa::SigningKey::from_slice(&d).unwrap();
        let point = key.verifying_key().to_encoded_point(false);
        assert_eq!(
            crate::b64(point.x().unwrap()),
            "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU"
        );
        assert_eq!(
            crate::b64(point.y().unwrap()),
            "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0"
        );
        let sig = p256::ecdsa::Signature::from_slice(&sig).unwrap();
        key.verifying_key()
            .verify(input.as_bytes(), &sig)
            .expect("RFC 7515 A.3 signature");
        // Our own tokens verify with the same key.
        let token = super::es256(
            &serde_json::json!({"alg": "ES256"}),
            &serde_json::json!({"iss": "joe"}),
            &key,
        );
        let (signed, s) = token.rsplit_once('.').unwrap();
        let s = p256::ecdsa::Signature::from_slice(&unb64(s).unwrap()).unwrap();
        key.verifying_key().verify(signed.as_bytes(), &s).unwrap();
    }

    /// DER for an RSAPrivateKey (RFC 8017 A.1.2) from JWK components.
    pub(crate) fn rsa_der(parts: &[&str]) -> Vec<u8> {
        fn len(n: usize) -> Vec<u8> {
            match n {
                0..=127 => vec![n as u8],
                128..=255 => vec![0x81, n as u8],
                _ => vec![0x82, (n >> 8) as u8, n as u8],
            }
        }
        fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
            let mut o = vec![tag];
            o.extend(len(body.len()));
            o.extend_from_slice(body);
            o
        }
        let mut body = tlv(0x02, &[0]);
        for p in parts {
            let mut n = unb64(p).unwrap();
            if n[0] & 0x80 != 0 {
                n.insert(0, 0);
            }
            body.extend(tlv(0x02, &n));
        }
        tlv(0x30, &body)
    }

    /// RFC 7515 Appendix A.2's RSA key: n, e, d, p, q, dp, dq, qi.
    pub(crate) const RFC7515_RSA: [&str; 8] = [
        "ofgWCuLjybRlzo0tZWJjNiuSfb4p4fAkd_wWJcyQoTbji9k0l8W26mPddxHmfHQp-Vaw-4qPCJrcS2mJPMEzP1Pt0Bm4d4QlL-yRT-SFd2lZS-pCgNMsD1W_YpRPEwOWvG6b32690r2jZ47soMZo9wGzjb_7OMg0LOL-bSf63kpaSHSXndS5z5rexMdbBYUsLA9e-KXBdQOS-UTo7WTBEMa2R2CapHg665xsmtdVMTBQY4uDZlxvb3qCo5ZwKh9kG4LT6_I5IhlJH7aGhyxXFvUK-DWNmoudF8NAco9_h9iaGNj8q2ethFkMLs91kzk2PAcDTW9gb54h4FRWyuXpoQ",
        "AQAB",
        "Eq5xpGnNCivDflJsRQBXHx1hdR1k6Ulwe2JZD50LpXyWPEAeP88vLNO97IjlA7_GQ5sLKMgvfTeXZx9SE-7YwVol2NXOoAJe46sui395IW_GO-pWJ1O0BkTGoVEn2bKVRUCgu-GjBVaYLU6f3l9kJfFNS3E0QbVdxzubSu3Mkqzjkn439X0M_V51gfpRLI9JYanrC4D4qAdGcopV_0ZHHzQlBjudU2QvXt4ehNYTCBr6XCLQUShb1juUO1ZdiYoFaFQT5Tw8bGUl_x_jTj3ccPDVZFD9pIuhLhBOneufuBiB4cS98l2SR_RQyGWSeWjnczT0QU91p1DhOVRuOopznQ",
        "4BzEEOtIpmVdVEZNCqS7baC4crd0pqnRH_5IB3jw3bcxGn6QLvnEtfdUdiYrqBdss1l58BQ3KhooKeQTa9AB0Hw_Py5PJdTJNPY8cQn7ouZ2KKDcmnPGBY5t7yLc1QlQ5xHdwW1VhvKn-nXqhJTBgIPgtldC-KDV5z-y2XDwGUc",
        "uQPEfgmVtjL0Uyyx88GZFF1fOunH3-7cepKmtH4pxhtCoHqpWmT8YAmZxaewHgHAjLYsp1ZSe7zFYHj7C6ul7TjeLQeZD_YwD66t62wDmpe_HlB-TnBA-njbglfIsRLtXlnDzQkv5dTltRJ11BKBBypeeF6689rjcJIDEz9RWdc",
        "BwKfV3Akq5_MFZDFZCnW-wzl-CCo83WoZvnLQwCTeDv8uzluRSnm71I3QCLdhrqE2e9YkxvuxdBfpT_PI7Yz-FOKnu1R6HsJeDCjn12Sk3vmAktV2zb34MCdy7cpdTh_YVr7tss2u6vneTwrA86rZtu5Mbr1C1XsmvkxHQAdYo0",
        "h_96-mK1R_7glhsum81dZxjTnYynPbZpHziZjeeHcXYsXaaMwkOlODsWa7I9xXDoRwbKgB719rrmI2oKr6N3Do9U0ajaHF-NKJnwgjMd2w9cjz3_-kyNlxAr2v4IKhGNpmM5iIgOS1VZnOZ68m6_pbLBSp3nssTdlqvd0tIiTHU",
        "IYd7DHOhrWvxkwPQsRM2tOgrjbcrfvtQJipd-DlcxyVuuM9sQLdgjVk2oy26F0EmpScGLq2MowX7fhd_QJQ3ydy5cY7YIBi87w93IKLEdfnbJtoOPLUW0ITrJReOgo1cq9SbsxYawBgfp_gh6A5603k2-ZQwVK0JKSHuLFkuQ3U",
    ];

    /// RFC 7515 Appendix A.2: RS256 is deterministic, so the signature
    /// matches the RFC's byte for byte.
    #[test]
    #[cfg(feature = "send")]
    fn signs_rfc7515_rs256_example() {
        let key = ring::signature::RsaKeyPair::from_der(&rsa_der(&RFC7515_RSA)).unwrap();
        let input = "eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFtcGxlLmNvbS9pc19yb290Ijp0cnVlfQ";
        let sig = super::rs256_sign(input.as_bytes(), &key);
        assert_eq!(
            crate::b64(&sig),
            "cC4hiUPoj9Eetdgtv3hF80EGrhuB__dzERat0XF9g2VtQgr9PJbu3XOiZj5RZmh7AAuHIm4Bh-0Qc_lF5YKt_O8W2Fp5jujGbds9uJdbF9CUAr7t1dnZcAcQjbKBYNX4BAynRFdiuB--f_nZLgrnbyTyWzO75vRK5h6xBArLIARNPvkSjtQBMHlb1L07Qe7K0GarZRmB_eSN9383LcOLn6_dO--xi12jzDwusC-eOkHWEsqtFZESc6BfI7noOPqvhJ1phCnvWh6IeYI2w9QOYEUipUTI8np6LbgGY9Fs98rqVt5AXLIhWkWywlVmtVrBp0igcN_IoypGlUPQGe77Rw"
        );
    }
}
