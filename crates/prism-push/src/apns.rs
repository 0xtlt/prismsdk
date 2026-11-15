//! Apple Push Notification service, token-based (a `.p8` signing key from
//! the developer account): `POST /3/device/<token>` over HTTP/2 with an
//! ES256 provider token.
//!
//! The payload follows Apple's keys (`aps`: `alert`, `badge`, `sound`,
//! `thread-id`, `content-available`), with Prism's `link` and custom data at
//! the top level, where the app reads them.

use std::time::SystemTime;

use serde_json::{Map, Value, json};

use crate::{HttpRequest, Push, PushError, Subscription, Urgency};

pub const PRODUCTION: &str = "https://api.push.apple.com";
pub const SANDBOX: &str = "https://api.sandbox.push.apple.com";

/// A token-based APNs key: the `.p8` file, its key id and the team id.
#[derive(Clone)]
pub struct ApnsKey {
    pub key_id: String,
    pub team_id: String,
    key: p256::ecdsa::SigningKey,
}

impl ApnsKey {
    /// `p8`: the contents of `AuthKey_<KEYID>.p8` (a PKCS#8 PEM).
    pub fn from_p8(p8: &str, key_id: &str, team_id: &str) -> Result<Self, PushError> {
        use p256::pkcs8::DecodePrivateKey;
        let key = p256::ecdsa::SigningKey::from_pkcs8_pem(p8)
            .map_err(|e| PushError::Config(format!("APNs key: {e}")))?;
        Ok(ApnsKey {
            key_id: key_id.into(),
            team_id: team_id.into(),
            key,
        })
    }

    /// The provider token (JWT, ES256). APNs accepts one for an hour and
    /// refuses new ones more often than every 20 minutes: reuse it (the
    /// `Sender` does).
    pub fn token(&self, now: SystemTime) -> String {
        crate::jwt::es256(
            &json!({"alg": "ES256", "kid": self.key_id}),
            &json!({"iss": self.team_id, "iat": crate::unix(now)}),
            &self.key,
        )
    }
}

/// The JSON body for `push`.
pub fn payload(push: &Push) -> Value {
    let mut aps = Map::new();
    if !push.silent {
        let mut alert = Map::new();
        for (k, v) in [
            ("title", &push.title),
            ("subtitle", &push.subtitle),
            ("body", &push.body),
        ] {
            if let Some(v) = v {
                alert.insert(k.into(), Value::String(v.clone()));
            }
        }
        if !alert.is_empty() {
            aps.insert("alert".into(), Value::Object(alert));
            aps.insert(
                "sound".into(),
                Value::String(push.sound.clone().unwrap_or_else(|| "default".into())),
            );
        } else if let Some(s) = &push.sound {
            aps.insert("sound".into(), Value::String(s.clone()));
        }
        if let Some(t) = &push.thread {
            aps.insert("thread-id".into(), Value::String(t.clone()));
        }
    }
    if let Some(b) = push.badge {
        aps.insert("badge".into(), b.into());
    }
    if push.silent {
        aps.insert("content-available".into(), 1.into());
    }
    if let Some(la) = &push.live_activity {
        aps.insert("timestamp".into(), crate::unix(SystemTime::now()).into());
        aps.insert("event".into(), la.event.into());
        aps.insert("content-state".into(), la.content_state.clone());
        if let Some(kind) = &la.kind {
            // Prism's ActivityKit attributes (prism-apple-build/apple/Shared).
            aps.insert("attributes-type".into(), "PrismActivityAttributes".into());
            aps.insert("attributes".into(), json!({ "kind": kind }));
        }
        if let Some(t) = la.stale_date {
            aps.insert("stale-date".into(), crate::unix(t).into());
        }
        if let Some(t) = la.dismissal_date {
            aps.insert("dismissal-date".into(), crate::unix(t).into());
        }
    }
    let mut root = Map::new();
    root.insert("aps".into(), Value::Object(aps));
    if let Some(l) = &push.link {
        root.insert("link".into(), Value::String(l.clone()));
    }
    for (k, v) in &push.data {
        if k != "aps" {
            root.insert(k.clone(), v.clone());
        }
    }
    Value::Object(root)
}

/// The request for a device (`sub.token`, `sub.topic`, `sub.sandbox`),
/// authorized with a provider token from [`ApnsKey::token`].
pub fn request(
    sub: &Subscription,
    push: &Push,
    provider_token: &str,
) -> Result<HttpRequest, PushError> {
    let topic = sub
        .topic
        .as_deref()
        .ok_or_else(|| PushError::Config("APNs subscriptions need a topic (bundle id)".into()))?;
    let host = if sub.sandbox { SANDBOX } else { PRODUCTION };
    let (push_type, topic) = if push.live_activity.is_some() {
        ("liveactivity", format!("{topic}.push-type.liveactivity"))
    } else if push.silent {
        ("background", topic.to_string())
    } else {
        ("alert", topic.to_string())
    };
    // Background pushes must use priority 5 (Apple rejects 10 for them).
    let priority = match (push.silent, push.urgency) {
        (true, _) | (_, Urgency::Low | Urgency::VeryLow) => "5",
        _ => "10",
    };
    let mut headers = vec![
        (
            "authorization".to_string(),
            format!("bearer {provider_token}"),
        ),
        ("apns-topic".into(), topic),
        ("apns-push-type".into(), push_type.into()),
        ("apns-priority".into(), priority.into()),
    ];
    if let Some(ttl) = push.ttl {
        let at = crate::unix(SystemTime::now() + ttl);
        headers.push(("apns-expiration".into(), at.to_string()));
    }
    if let Some(id) = &push.collapse_id {
        // At most 64 bytes.
        let mut id = id.clone();
        while id.len() > 64 {
            id.pop();
        }
        headers.push(("apns-collapse-id".into(), id));
    }
    let token: String = sub
        .token
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    Ok(HttpRequest {
        url: format!("{host}/3/device/{token}"),
        headers,
        body: payload(push).to_string().into_bytes(),
    })
}

/// Maps an APNs error response (`{"reason": "…"}`) to a [`PushError`].
pub fn error(status: u16, body: &[u8]) -> PushError {
    let reason = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v["reason"].as_str().map(str::to_string))
        .unwrap_or_default();
    match (status, reason.as_str()) {
        (410, _) | (400, "BadDeviceToken") | (400, "DeviceTokenNotForTopic") => PushError::Gone,
        _ => PushError::Rejected { status, reason },
    }
}

#[cfg(test)]
mod tests {
    use p256::ecdsa::signature::Verifier;
    use p256::pkcs8::EncodePrivateKey;

    use super::*;
    use crate::unb64;

    fn key() -> ApnsKey {
        let secret = p256::ecdsa::SigningKey::random(&mut rand_core::OsRng);
        let pem = secret.to_pkcs8_pem(Default::default()).unwrap();
        ApnsKey::from_p8(&pem, "ABC123DEFG", "TEAM123456").unwrap()
    }

    fn sub() -> Subscription {
        Subscription {
            service: crate::Service::Apns,
            token: "a1b2c3".into(),
            keys: None,
            topic: Some("dev.prism.counter".into()),
            sandbox: true,
        }
    }

    #[test]
    fn provider_token_is_es256_with_kid() {
        let k = key();
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let t = k.token(now);
        let parts: Vec<&str> = t.split('.').collect();
        let header: Value = serde_json::from_slice(&unb64(parts[0]).unwrap()).unwrap();
        let claims: Value = serde_json::from_slice(&unb64(parts[1]).unwrap()).unwrap();
        assert_eq!(header, json!({"alg": "ES256", "kid": "ABC123DEFG"}));
        assert_eq!(claims, json!({"iss": "TEAM123456", "iat": 1_700_000_000}));
        let sig = p256::ecdsa::Signature::from_slice(&unb64(parts[2]).unwrap()).unwrap();
        k.key
            .verifying_key()
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
    }

    #[test]
    fn alert_push() {
        let push = Push::new()
            .title("Order shipped")
            .body("Tomorrow")
            .link("/orders/42")
            .data("order", 42)
            .badge(3)
            .collapse_id("order-42");
        let r = request(&sub(), &push, "JWT").unwrap();
        assert_eq!(r.url, "https://api.sandbox.push.apple.com/3/device/a1b2c3");
        assert_eq!(r.header("authorization"), Some("bearer JWT"));
        assert_eq!(r.header("apns-topic"), Some("dev.prism.counter"));
        assert_eq!(r.header("apns-push-type"), Some("alert"));
        assert_eq!(r.header("apns-priority"), Some("10"));
        assert_eq!(r.header("apns-collapse-id"), Some("order-42"));
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(
            body,
            json!({
                "aps": {"alert": {"title": "Order shipped", "body": "Tomorrow"}, "sound": "default", "badge": 3},
                "link": "/orders/42",
                "order": 42,
            })
        );
    }

    #[test]
    fn background_push() {
        let push = Push::new().silent().data("sync", "inbox");
        let r = request(&sub(), &push, "JWT").unwrap();
        assert_eq!(r.header("apns-push-type"), Some("background"));
        assert_eq!(r.header("apns-priority"), Some("5"));
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(
            body,
            json!({"aps": {"content-available": 1}, "sync": "inbox"})
        );
    }

    #[test]
    fn live_activity_push() {
        let push = Push::new().live_activity(r#"{"json":"{\"sm\":\"5 min\"}"}"#, false);
        let r = request(&sub(), &push, "JWT").unwrap();
        assert_eq!(r.header("apns-push-type"), Some("liveactivity"));
        assert_eq!(
            r.header("apns-topic"),
            Some("dev.prism.counter.push-type.liveactivity")
        );
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(body["aps"]["event"], "update");
        assert_eq!(body["aps"]["content-state"]["json"], "{\"sm\":\"5 min\"}");
        assert!(body["aps"]["timestamp"].is_u64());
    }

    #[test]
    fn errors() {
        assert!(matches!(
            error(410, br#"{"reason":"Unregistered"}"#),
            PushError::Gone
        ));
        assert!(matches!(
            error(403, br#"{"reason":"InvalidProviderToken"}"#),
            PushError::Rejected { status: 403, .. }
        ));
    }
}
