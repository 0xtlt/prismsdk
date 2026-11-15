//! Firebase Cloud Messaging, HTTP v1: a service account signs an OAuth
//! assertion (RS256), exchanged for an access token, then
//! `POST /v1/projects/<project>/messages:send`.
//!
//! Prism sends **data messages** (high priority unless `Urgency` says
//! otherwise): the app's `FirebaseMessagingService` receives them in the
//! foreground and the background, and shows visible ones itself, so links and
//! handlers work the same as on the other platforms.

use serde_json::{Map, Value, json};

use crate::{HttpRequest, Push, PushError, Urgency};

pub const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

/// The `message` for registration token `token`.
pub fn message(token: &str, push: &Push) -> Value {
    // FCM data values are strings.
    let mut data = Map::new();
    for (k, v) in [
        ("title", &push.title),
        ("subtitle", &push.subtitle),
        ("body", &push.body),
        ("link", &push.link),
        ("id", &push.collapse_id),
    ] {
        if let Some(v) = v {
            data.insert(k.into(), Value::String(v.clone()));
        }
    }
    if push.silent {
        data.insert("silent".into(), Value::String("true".into()));
    }
    for (k, v) in &push.data {
        let text = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        data.insert(k.clone(), Value::String(text));
    }
    let mut android = Map::new();
    android.insert(
        "priority".into(),
        match push.urgency {
            Urgency::Low | Urgency::VeryLow => "normal",
            _ => "high",
        }
        .into(),
    );
    if let Some(ttl) = push.ttl {
        android.insert("ttl".into(), format!("{}s", ttl.as_secs()).into());
    }
    if let Some(c) = &push.collapse_id {
        android.insert("collapse_key".into(), c.clone().into());
    }
    json!({ "message": { "token": token, "data": data, "android": android } })
}

pub fn send_url(project_id: &str) -> String {
    format!("https://fcm.googleapis.com/v1/projects/{project_id}/messages:send")
}

/// Maps an FCM error (`{"error":{"status":…,"details":[{"errorCode":…}]}}`).
pub fn error(status: u16, body: &[u8]) -> PushError {
    let v: Value = serde_json::from_slice(body).unwrap_or_default();
    let code = v["error"]["details"]
        .as_array()
        .and_then(|d| d.iter().find_map(|d| d["errorCode"].as_str()))
        .or_else(|| v["error"]["status"].as_str())
        .unwrap_or_default()
        .to_string();
    if status == 404 || code == "UNREGISTERED" {
        return PushError::Gone;
    }
    PushError::Rejected {
        status,
        reason: code,
    }
}

/// A Google service account (the JSON key from the Firebase console:
/// Project settings › Service accounts).
#[cfg(feature = "send")]
pub struct ServiceAccount {
    pub project_id: String,
    pub client_email: String,
    pub token_uri: String,
    key: ring::signature::RsaKeyPair,
}

#[cfg(feature = "send")]
impl ServiceAccount {
    pub fn from_json(json: &str) -> Result<Self, PushError> {
        let v: Value = serde_json::from_str(json)
            .map_err(|e| PushError::Config(format!("service account: {e}")))?;
        let field = |k: &str| {
            v[k].as_str()
                .map(str::to_string)
                .ok_or_else(|| PushError::Config(format!("service account: no {k}")))
        };
        let der = pem_body(&field("private_key")?)?;
        let key = ring::signature::RsaKeyPair::from_pkcs8(&der)
            .map_err(|e| PushError::Config(format!("service account key: {e}")))?;
        Ok(ServiceAccount {
            project_id: field("project_id")?,
            client_email: field("client_email")?,
            token_uri: field("token_uri")
                .unwrap_or_else(|_| "https://oauth2.googleapis.com/token".into()),
            key,
        })
    }

    /// The signed assertion (RFC 7523) for an access token, valid an hour.
    pub fn assertion(&self, now: std::time::SystemTime) -> String {
        let iat = crate::unix(now);
        crate::jwt::rs256(
            &json!({"alg": "RS256", "typ": "JWT"}),
            &json!({
                "iss": self.client_email,
                "scope": SCOPE,
                "aud": self.token_uri,
                "iat": iat,
                "exp": iat + 3600,
            }),
            &self.key,
        )
    }

    /// The form request exchanging the assertion for an access token.
    pub fn token_request(&self, now: std::time::SystemTime) -> HttpRequest {
        HttpRequest {
            url: self.token_uri.clone(),
            headers: vec![(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            body: format!(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={}",
                self.assertion(now)
            )
            .into_bytes(),
        }
    }

    /// The send request, with an access token from [`token_request`](Self::token_request).
    pub fn request(&self, token: &str, push: &Push, access_token: &str) -> HttpRequest {
        HttpRequest {
            url: send_url(&self.project_id),
            headers: vec![
                ("Authorization".into(), format!("Bearer {access_token}")),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: message(token, push).to_string().into_bytes(),
        }
    }
}

#[cfg(feature = "send")]
fn pem_body(pem: &str) -> Result<Vec<u8>, PushError> {
    let body: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect::<Vec<_>>()
        .join("");
    crate::unb64(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_message() {
        let push = Push::new()
            .title("Hi")
            .link("/inbox")
            .data("count", 3)
            .collapse_id("inbox")
            .ttl(std::time::Duration::from_secs(60));
        assert_eq!(
            message("TOKEN", &push),
            json!({"message": {
                "token": "TOKEN",
                "data": {"title": "Hi", "link": "/inbox", "id": "inbox", "count": "3"},
                "android": {"priority": "high", "ttl": "60s", "collapse_key": "inbox"},
            }})
        );
        let silent = message("T", &Push::new().silent().urgency(Urgency::Low));
        assert_eq!(silent["message"]["data"]["silent"], "true");
        assert_eq!(silent["message"]["android"]["priority"], "normal");
    }

    #[test]
    fn errors() {
        let body = br#"{"error":{"code":404,"status":"NOT_FOUND","details":[{"@type":"type.googleapis.com/google.firebase.fcm.v1.FcmError","errorCode":"UNREGISTERED"}]}}"#;
        assert!(matches!(error(404, body), PushError::Gone));
        assert!(matches!(
            error(400, br#"{"error":{"status":"INVALID_ARGUMENT"}}"#),
            PushError::Rejected { status: 400, .. }
        ));
    }

    /// A service account JSON with RFC 7515's RSA key (PKCS#8 PEM).
    #[cfg(feature = "send")]
    #[test]
    fn signs_oauth_assertions() {
        use crate::jwt::tests::{RFC7515_RSA, rsa_der};
        let rsa = rsa_der(&RFC7515_RSA);
        // PrivateKeyInfo { 0, rsaEncryption, OCTET STRING rsa }.
        let mut alg = vec![0x30, 0x0d, 0x06, 0x09];
        alg.extend([
            0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
        ]);
        let mut body = vec![0x02, 0x01, 0x00];
        body.extend(alg);
        body.push(0x04);
        body.extend([0x82, (rsa.len() >> 8) as u8, rsa.len() as u8]);
        body.extend(&rsa);
        let mut pkcs8 = vec![0x30, 0x82, (body.len() >> 8) as u8, body.len() as u8];
        pkcs8.extend(body);
        use base64::Engine;
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(&pkcs8)
        );
        let sa = ServiceAccount::from_json(
            &json!({
                "type": "service_account",
                "project_id": "demo-project",
                "private_key": pem,
                "client_email": "push@demo-project.iam.gserviceaccount.com",
                "token_uri": "https://oauth2.googleapis.com/token",
            })
            .to_string(),
        )
        .unwrap();
        let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let jwt = sa.assertion(now);
        let (signed, sig) = jwt.rsplit_once('.').unwrap();
        let claims: Value =
            serde_json::from_slice(&crate::unb64(signed.split('.').nth(1).unwrap()).unwrap())
                .unwrap();
        assert_eq!(claims["iss"], "push@demo-project.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], SCOPE);
        assert_eq!(claims["exp"], 1_700_003_600);
        let public = ring::signature::RsaPublicKeyComponents {
            n: crate::unb64(RFC7515_RSA[0]).unwrap(),
            e: crate::unb64(RFC7515_RSA[1]).unwrap(),
        };
        public
            .verify(
                &ring::signature::RSA_PKCS1_2048_8192_SHA256,
                signed.as_bytes(),
                &crate::unb64(sig).unwrap(),
            )
            .unwrap();
        let req = sa.token_request(now);
        assert!(
            String::from_utf8(req.body)
                .unwrap()
                .contains("assertion=ey")
        );
        assert_eq!(
            sa.request("T", &Push::new(), "ya29").url,
            "https://fcm.googleapis.com/v1/projects/demo-project/messages:send"
        );
    }
}
