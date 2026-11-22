//! Windows Push Notification Services, for Windows App SDK apps shipped as
//! an MSIX: a Microsoft Entra app registration's client credentials are
//! exchanged for an access token (`scope=https://wns.windows.com/.default`),
//! then the push is `POST`ed to the app's channel URI as a **raw**
//! notification (`X-WNS-Type: wns/raw`): its body is the Prism payload,
//! which the app reads and shows itself, so links and handlers work the
//! same as on the other platforms.
//!
//! Channel URIs come from devices: the sender posts only to WNS hosts
//! (`https://*.notify.windows.com`), never elsewhere with the token.
//!
//! The registration needs a mapping between the app's package family name
//! and the Entra app, which Microsoft makes on request (Windows App SDK
//! push documentation).

use crate::{HttpRequest, Push, PushError};

pub const SCOPE: &str = "https://wns.windows.com/.default";

/// The Entra app registration that sends: its tenant (directory) id, its
/// application (client) id and a client secret.
#[derive(Clone)]
pub struct WnsCredentials {
    pub tenant_id: String,
    pub client_id: String,
    client_secret: String,
}

impl std::fmt::Debug for WnsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WnsCredentials")
            .field("tenant_id", &self.tenant_id)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<hidden>")
            .finish()
    }
}

/// `tenant` and client ids: GUIDs (a tenant may also be a domain name).
fn plain_id(what: &str, id: &str) -> Result<(), PushError> {
    let ok = !id.is_empty()
        && id.len() <= 253
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
    if ok {
        Ok(())
    } else {
        Err(PushError::Config(format!(
            "WNS: {what} {id:?} is not an id"
        )))
    }
}

impl WnsCredentials {
    pub fn new(
        tenant_id: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Result<Self, PushError> {
        let c = WnsCredentials {
            tenant_id: tenant_id.into(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
        };
        plain_id("tenant", &c.tenant_id)?;
        plain_id("client id", &c.client_id)?;
        if c.client_secret.is_empty() {
            return Err(PushError::Config("WNS: an empty client secret".into()));
        }
        Ok(c)
    }

    /// The token endpoint of the tenant.
    pub fn token_url(&self) -> String {
        format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        )
    }

    /// The client-credentials request for an access token.
    pub fn token_request(&self) -> HttpRequest {
        HttpRequest {
            url: self.token_url(),
            headers: vec![(
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
            body: format!(
                "grant_type=client_credentials&client_id={}&client_secret={}&scope={}",
                form(&self.client_id),
                form(&self.client_secret),
                form(SCOPE)
            )
            .into_bytes(),
        }
    }
}

/// `application/x-www-form-urlencoded` text.
fn form(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Whether `uri` is a WNS channel: `https://<name>.notify.windows.com/…`,
/// on the default port, without credentials.
pub fn check_channel(uri: &str) -> Result<(), PushError> {
    let bad = || PushError::Config("WNS: the channel is not a notify.windows.com URL".into());
    let rest = uri.strip_prefix("https://").ok_or_else(bad)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains(['@', ':', '\\', '%']) {
        return Err(bad());
    }
    let host = authority.to_ascii_lowercase();
    let label = host.strip_suffix(".notify.windows.com").ok_or_else(bad)?;
    let ok = !label.is_empty()
        && label
            .split('.')
            .all(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    if ok { Ok(()) } else { Err(bad()) }
}

/// The most a notification carries (WNS answers 413 above it).
pub const MAX_PAYLOAD: usize = 5000;

/// The raw notification for channel `uri`, with an access token from
/// [`WnsCredentials::token_request`].
///
/// Headers, from Microsoft's « Push notification service request and
/// response headers »: `X-WNS-Type: wns/raw`; `X-WNS-Cache-Policy: cache`,
/// always, since raw notifications are not cached by default and one
/// sent while the PC is offline would be lost (WNS then keeps the latest
/// one per channel); `X-WNS-TTL` from `ttl`. `collapse_id` and `urgency`
/// have no raw counterpart there (`X-WNS-Tag` is for tiles only, and no
/// priority header is documented), so they are not sent.
pub fn request(uri: &str, push: &Push, access_token: &str) -> Result<HttpRequest, PushError> {
    check_channel(uri)?;
    let body = push.payload().to_string().into_bytes();
    if body.len() > MAX_PAYLOAD {
        return Err(PushError::Config(format!(
            "WNS: the payload is {} bytes ({MAX_PAYLOAD} at most)",
            body.len()
        )));
    }
    let mut headers = vec![
        ("Authorization".into(), format!("Bearer {access_token}")),
        ("Content-Type".into(), "application/octet-stream".into()),
        ("X-WNS-Type".into(), "wns/raw".into()),
        ("X-WNS-Cache-Policy".into(), "cache".into()),
    ];
    if let Some(ttl) = push.ttl {
        headers.push(("X-WNS-TTL".into(), ttl.as_secs().to_string()));
    }
    Ok(HttpRequest {
        url: uri.to_string(),
        headers,
        body,
    })
}

/// A header of an answer, by name in any case.
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
}

/// Maps WNS's answer to a send:
///
/// - 2xx with `X-WNS-Status: received` (or none): sent. `dropped` (an
///   error, or the client refuses these notifications) and
///   `channelthrottled` are errors, not successes;
/// - 404 (an unknown channel) and 410 for an expired channel are
///   [`PushError::Gone`]; another 410, such as « Domain Blocked » (WNS
///   blocked the sender), is a rejection, so the server does not delete
///   every channel. The 410's `X-WNS-Error-Description` tells them apart;
///   a 410 without one is the documented expired channel.
pub fn response(status: u16, headers: &[(String, String)], body: &[u8]) -> Result<(), PushError> {
    let description = header(headers, "X-WNS-Error-Description").unwrap_or_default();
    let reason = |fallback: &str| {
        let text = if description.is_empty() {
            String::from_utf8_lossy(&body[..body.len().min(200)])
                .trim()
                .to_string()
        } else {
            description.to_string()
        };
        if text.is_empty() {
            fallback.to_string()
        } else {
            text
        }
    };
    if (200..300).contains(&status) {
        return match header(headers, "X-WNS-Status")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("received") => Ok(()),
            Some(other) => Err(PushError::Rejected {
                status,
                reason: format!(
                    "WNS: {other}: {}",
                    reason("the notification was not delivered")
                ),
            }),
        };
    }
    let lower = description.to_ascii_lowercase();
    let expired = description.is_empty() || lower.contains("expired");
    match status {
        404 => Err(PushError::Gone),
        410 if expired && !lower.contains("block") => Err(PushError::Gone),
        _ => Err(PushError::Rejected {
            status,
            reason: reason(""),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Urgency;

    const CHANNEL: &str = "https://wns2-par02p.notify.windows.com/w/?token=AwYAAAB%2bQ";

    #[test]
    fn raw_notifications() {
        let push = Push::new()
            .title("Hi")
            .link("/inbox")
            .collapse_id("inbox-messages-1234")
            .ttl(std::time::Duration::from_secs(60))
            .urgency(Urgency::High);
        let req = request(CHANNEL, &push, "eyJ0").unwrap();
        assert_eq!(req.url, CHANNEL);
        assert_eq!(req.header("Authorization"), Some("Bearer eyJ0"));
        assert_eq!(req.header("X-WNS-Type"), Some("wns/raw"));
        assert_eq!(req.header("X-WNS-TTL"), Some("60"));
        // Cached while the PC is offline, always; no tile-only tag, no
        // undocumented priority.
        assert_eq!(req.header("X-WNS-Cache-Policy"), Some("cache"));
        assert_eq!(req.header("X-WNS-Tag"), None);
        assert_eq!(req.header("X-WNS-Priority"), None);
        let plain = request(CHANNEL, &Push::new().title("Hi"), "t").unwrap();
        assert_eq!(plain.header("X-WNS-Cache-Policy"), Some("cache"));
        assert_eq!(plain.header("X-WNS-TTL"), None);
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["title"], "Hi");
        assert_eq!(body["link"], "/inbox");
        let silent = request(CHANNEL, &Push::new().silent(), "t").unwrap();
        assert_eq!(silent.body, br#"{"silent":true}"#);
        // 5000 bytes at most (the payload is `{"body":"…"}`: 11 more).
        let fits = Push::new().body("x".repeat(MAX_PAYLOAD - 11));
        assert_eq!(
            request(CHANNEL, &fits, "t").unwrap().body.len(),
            MAX_PAYLOAD
        );
        let big = Push::new().body("x".repeat(MAX_PAYLOAD - 10));
        assert!(request(CHANNEL, &big, "t").is_err());
    }

    /// The token goes to WNS only.
    #[test]
    fn channels_are_wns_urls() {
        assert!(check_channel(CHANNEL).is_ok());
        assert!(check_channel("https://DB5.notify.windows.com/?token=1").is_ok());
        for bad in [
            "http://wns2-par02p.notify.windows.com/w/?token=1",
            "https://notify.windows.com/?token=1",
            "https://evil.example/?h=.notify.windows.com",
            "https://evil.example#.notify.windows.com",
            "https://x.notify.windows.com.evil.example/",
            "https://user@x.notify.windows.com/",
            "https://x.notify.windows.com:8443/",
            "https://x..notify.windows.com/",
            "https://x%2enotify.windows.com/",
        ] {
            assert!(check_channel(bad).is_err(), "{bad}");
            assert!(request(bad, &Push::new(), "t").is_err(), "{bad}");
        }
    }

    #[test]
    fn credentials() {
        let c = WnsCredentials::new(
            "72f988bf-86f1-41af-91ab-2d7cd011db47",
            "7edfab6c-25ae-4678-b406-d1848f97919a",
            "a~b&c=d",
        )
        .unwrap();
        let req = c.token_request();
        assert_eq!(
            req.url,
            "https://login.microsoftonline.com/72f988bf-86f1-41af-91ab-2d7cd011db47/oauth2/v2.0/token"
        );
        assert_eq!(
            String::from_utf8(req.body).unwrap(),
            "grant_type=client_credentials&client_id=7edfab6c-25ae-4678-b406-d1848f97919a\
             &client_secret=a~b%26c%3Dd&scope=https%3A%2F%2Fwns.windows.com%2F.default"
        );
        assert!(!format!("{c:?}").contains("a~b"));
        assert!(WnsCredentials::new("a/b", "x", "s").is_err());
        assert!(WnsCredentials::new("t", "x", "").is_err());
    }

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn answers() {
        assert!(response(200, &h(&[("X-WNS-Status", "received")]), b"").is_ok());
        assert!(response(200, &[], b"").is_ok());
        // Dropped or throttled: not sent.
        let dropped = response(200, &h(&[("x-wns-status", "dropped")]), b"").unwrap_err();
        assert!(
            matches!(&dropped, PushError::Rejected { status: 200, reason } if reason.contains("dropped")),
            "{dropped:?}"
        );
        assert!(response(200, &h(&[("X-WNS-Status", "channelthrottled")]), b"").is_err());
        // Gone: an unknown or expired channel only.
        assert!(matches!(response(404, &[], b""), Err(PushError::Gone)));
        assert!(matches!(response(410, &[], b""), Err(PushError::Gone)));
        let expired = h(&[("X-WNS-Error-Description", "The channel expired.")]);
        assert!(matches!(response(410, &expired, b""), Err(PushError::Gone)));
        let blocked = h(&[("X-WNS-Error-Description", "Domain Blocked")]);
        let err = response(410, &blocked, b"").unwrap_err();
        assert!(
            matches!(&err, PushError::Rejected { status: 410, reason } if reason == "Domain Blocked"),
            "{err:?}"
        );
        assert!(matches!(
            response(401, &[], b"token"),
            Err(PushError::Rejected { status: 401, .. })
        ));
    }
}
