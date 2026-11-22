//! The async sender: one HTTP/2 client for every service, with APNs provider
//! tokens and FCM access tokens cached until shortly before they expire.
//!
//! Web Push and UnifiedPush endpoints come from subscribers, so the client
//! connects only to public addresses (checked after each name lookup),
//! never follows a redirect, ignores proxy environment variables, and reads
//! at most [`MAX_RESPONSE`] bytes of an answer.

use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::apns::ApnsKey;
use crate::endpoint::{Endpoint, Host};
use crate::fcm::ServiceAccount;
use crate::webpush::VapidKey;
use crate::wns::WnsCredentials;
use crate::{HttpRequest, Push, PushError, Service, Subscription};

/// The most of an answer the sender reads (push services answer in a few
/// hundred bytes).
const MAX_RESPONSE: usize = 16 << 10;
/// The most of an answer's body a [`PushError::Rejected`] carries.
const MAX_REASON: usize = 1 << 10;

type Lookup = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send>>
        + Send
        + Sync,
>;

/// Sends pushes to any [`Subscription`]. Configure the services you use;
/// share one `Sender` (it is `Send + Sync`) across requests.
pub struct Sender {
    http: reqwest::Client,
    apns: Option<ApnsKey>,
    apns_token: Mutex<Option<(String, Instant)>>,
    fcm: Option<ServiceAccount>,
    fcm_token: Mutex<Option<(String, Instant)>>,
    vapid: Option<(VapidKey, String)>,
    wns: Option<WnsCredentials>,
    wns_token: Mutex<Option<(String, Instant)>>,
    /// Origins of private distributors ([`Sender::trust_origin`]).
    trusted: Vec<(bool, String, u16)>,
    proxy: Option<String>,
    lookup: Lookup,
}

impl Default for Sender {
    fn default() -> Self {
        Self::new()
    }
}

impl Sender {
    pub fn new() -> Self {
        Self::with_lookup(Arc::new(|name: String| {
            Box::pin(
                async move { Ok(tokio::net::lookup_host((name.as_str(), 0)).await?.collect()) },
            )
        }))
    }

    fn with_lookup(lookup: Lookup) -> Self {
        Sender {
            http: client(&[], None, &lookup).expect("HTTP client"),
            apns: None,
            apns_token: Mutex::new(None),
            fcm: None,
            fcm_token: Mutex::new(None),
            vapid: None,
            wns: None,
            wns_token: Mutex::new(None),
            trusted: Vec::new(),
            proxy: None,
            lookup,
        }
    }

    /// The HTTP client for the current settings.
    fn rebuild(mut self) -> Result<Self, PushError> {
        self.http = client(&self.trusted, self.proxy.as_deref(), &self.lookup)?;
        Ok(self)
    }

    /// Lets Web Push and UnifiedPush endpoints at `origin` (`scheme://host[:port]`)
    /// through as they are: plain HTTP, any port, private addresses. For a
    /// self-hosted UnifiedPush distributor on your network
    /// (`http://ntfy.internal:8080`). Other origins must be public HTTPS
    /// servers on port 443; the same host on another scheme or port is
    /// refused.
    pub fn trust_origin(mut self, origin: &str) -> Result<Self, PushError> {
        let endpoint = Endpoint::parse(origin)?;
        let path = origin.split_once("://").map_or("", |(_, rest)| {
            rest.find(['/', '?', '#']).map_or("", |i| &rest[i..])
        });
        if !path.is_empty() && path != "/" {
            return Err(PushError::Config(format!(
                "trust_origin: {origin} is not an origin"
            )));
        }
        self.trusted.push(endpoint.origin());
        self.rebuild()
    }

    /// Sends every request through the proxy at `url` (`http://proxy:3128`).
    /// The sender ignores `HTTPS_PROXY` and the like; with a proxy, the
    /// proxy looks up endpoint names, so it must refuse private addresses
    /// itself (the sender still refuses private and local hosts written in
    /// endpoints).
    pub fn proxy(mut self, url: &str) -> Result<Self, PushError> {
        self.proxy = Some(url.to_string());
        self.rebuild()
    }

    pub fn apns(mut self, key: ApnsKey) -> Self {
        self.apns = Some(key);
        self
    }

    pub fn fcm(mut self, account: ServiceAccount) -> Self {
        self.fcm = Some(account);
        self
    }

    /// Web Push and UnifiedPush. `subject`: a `mailto:` or `https:` contact
    /// for the push services.
    pub fn vapid(mut self, key: VapidKey, subject: impl Into<String>) -> Self {
        self.vapid = Some((key, subject.into()));
        self
    }

    /// WNS (Windows apps shipped as an MSIX): the Entra app registration
    /// that sends.
    pub fn wns(mut self, credentials: WnsCredentials) -> Self {
        self.wns = Some(credentials);
        self
    }

    /// Sends `push` to `sub`. [`PushError::Gone`] means the subscription
    /// is dead: remove it.
    pub async fn send(&self, sub: &Subscription, push: &Push) -> Result<(), PushError> {
        match sub.service {
            Service::Apns => {
                let key = self
                    .apns
                    .as_ref()
                    .ok_or_else(|| PushError::Config("no APNs key (Sender::apns)".into()))?;
                let token = cached(&self.apns_token, Duration::from_secs(50 * 60), || {
                    key.token(SystemTime::now())
                });
                let req = crate::apns::request(sub, push, &token)?;
                let (status, _, body) = self.execute(req).await?;
                if status == 403 && body.windows(19).any(|w| w == b"ExpiredProviderToken") {
                    *self.apns_token.lock().unwrap() = None;
                }
                check(status, &body, crate::apns::error)
            }
            Service::Fcm => {
                let account = self.fcm.as_ref().ok_or_else(|| {
                    PushError::Config("no FCM service account (Sender::fcm)".into())
                })?;
                let access = self.fcm_access_token(account).await?;
                let req = account.request(&sub.token, push, &access);
                let (status, _, body) = self.execute(req).await?;
                check(status, &body, crate::fcm::error)
            }
            Service::Wns => {
                let credentials = self
                    .wns
                    .as_ref()
                    .ok_or_else(|| PushError::Config("no WNS credentials (Sender::wns)".into()))?;
                // The channel and the payload, checked before asking for a token.
                crate::wns::request(&sub.token, push, "")?;
                let access = self.wns_access_token(credentials).await?;
                let req = crate::wns::request(&sub.token, push, &access)?;
                let (status, headers, body) = self.execute(req).await?;
                if status == 401 {
                    // The token expired early: the next send asks again.
                    *self.wns_token.lock().unwrap() = None;
                }
                crate::wns::response(status, &headers, &body)
            }
            Service::WebPush | Service::UnifiedPush => {
                self.check_endpoint(&sub.token)?;
                let vapid = self.vapid.as_ref().map(|(k, s)| (k, s.as_str()));
                let req = crate::webpush::build(sub, push, vapid, SystemTime::now())?;
                let (status, _, body) = self.execute(req).await?;
                check(status, &body, |status, body| match status {
                    404 | 410 => PushError::Gone,
                    _ => PushError::Rejected {
                        status,
                        reason: reason(body, &sub.token),
                    },
                })
            }
        }
    }

    async fn wns_access_token(&self, credentials: &WnsCredentials) -> Result<String, PushError> {
        if let Some((t, until)) = &*self.wns_token.lock().unwrap()
            && *until > Instant::now()
        {
            return Ok(t.clone());
        }
        let (status, _, body) = self.execute(credentials.token_request()).await?;
        if status != 200 {
            return Err(PushError::Rejected {
                status,
                reason: reason(&body, ""),
            });
        }
        let v: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| PushError::Http(e.to_string()))?;
        let token = v["access_token"]
            .as_str()
            .ok_or_else(|| PushError::Http("no access_token".into()))?
            .to_string();
        let lifetime = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(300);
        *self.wns_token.lock().unwrap() = Some((
            token.clone(),
            Instant::now() + Duration::from_secs(lifetime),
        ));
        Ok(token)
    }

    async fn fcm_access_token(&self, account: &ServiceAccount) -> Result<String, PushError> {
        if let Some((t, until)) = &*self.fcm_token.lock().unwrap()
            && *until > Instant::now()
        {
            return Ok(t.clone());
        }
        let (status, _, body) = self
            .execute(account.token_request(SystemTime::now()))
            .await?;
        if status != 200 {
            return Err(PushError::Rejected {
                status,
                reason: reason(&body, ""),
            });
        }
        let v: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| PushError::Http(e.to_string()))?;
        let token = v["access_token"]
            .as_str()
            .ok_or_else(|| PushError::Http("no access_token".into()))?
            .to_string();
        let lifetime = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(300);
        *self.fcm_token.lock().unwrap() = Some((
            token.clone(),
            Instant::now() + Duration::from_secs(lifetime),
        ));
        Ok(token)
    }

    /// Posts `req`; returns the status, the response's headers (WNS says
    /// in them whether it delivered) and its body (capped).
    async fn execute(
        &self,
        req: HttpRequest,
    ) -> Result<(u16, Vec<(String, String)>, Vec<u8>), PushError> {
        // APNs needs HTTP/2: ALPN negotiates it (reqwest`s `http2` feature).
        let mut builder = self.http.post(&req.url).body(req.body);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        let res = builder
            .send()
            .await
            .map_err(|e| PushError::Http(e.to_string()))?;
        let status = res.status().as_u16();
        let headers = res
            .headers()
            .iter()
            .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
            .collect();
        Ok((status, headers, read_capped(res).await?))
    }

    /// Whether the sender may post to Web Push endpoint `url`: a trusted
    /// origin, or a public HTTPS one. Both URL parsers must agree on the
    /// host the client will connect to.
    fn check_endpoint(&self, url: &str) -> Result<(), PushError> {
        let endpoint = Endpoint::parse(url)?;
        let origin = endpoint.origin();
        if !self.trusted.contains(&origin) {
            endpoint.check_public()?;
            if self.trusted.iter().any(|(_, host, _)| *host == origin.1) {
                return Err(PushError::Config(format!(
                    "push endpoint: {} is trusted at another origin",
                    origin.1
                )));
            }
        }
        let parsed = reqwest::Url::parse(url).map_err(|e| PushError::Config(e.to_string()))?;
        let host = parsed.host_str().unwrap_or("");
        let same_host = match &endpoint.host {
            Host::Name(n) => n == host.trim_end_matches('.'),
            Host::Ip(ip) => host.trim_matches(['[', ']']).parse() == Ok(*ip),
        };
        if !same_host || parsed.port_or_known_default() != Some(endpoint.port) {
            return Err(PushError::Config("push endpoint: ambiguous URL".into()));
        }
        Ok(())
    }
}

/// The HTTP client: no redirects, no proxy but `proxy`, and names checked
/// by [`PublicOnly`].
fn client(
    trusted: &[(bool, String, u16)],
    proxy: Option<&str>,
    lookup: &Lookup,
) -> Result<reqwest::Client, PushError> {
    // TLS with ring (no aws-lc build), the system's certificates.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let config = |e: reqwest::Error| PushError::Config(format!("HTTP client: {e}"));
    // Names whose addresses are not checked: trusted distributors and the
    // proxy, which the deployment chose.
    let mut open: HashSet<String> = trusted.iter().map(|(_, host, _)| host.clone()).collect();
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if let Some(proxy) = proxy {
        let url =
            reqwest::Url::parse(proxy).map_err(|e| PushError::Config(format!("proxy: {e}")))?;
        open.extend(
            url.host_str()
                .map(|h| h.trim_matches(['[', ']']).to_ascii_lowercase()),
        );
        builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(config)?);
    }
    builder
        .dns_resolver(Arc::new(PublicOnly {
            open,
            lookup: lookup.clone(),
        }))
        .build()
        .map_err(config)
}

/// The body of `res`, or an error once it passes [`MAX_RESPONSE`].
async fn read_capped(mut res: reqwest::Response) -> Result<Vec<u8>, PushError> {
    let too_long = || PushError::Http(format!("the answer is over {MAX_RESPONSE} bytes"));
    if res
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE as u64)
    {
        return Err(too_long());
    }
    let mut body = Vec::new();
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| PushError::Http(e.to_string()))?
    {
        if body.len() + chunk.len() > MAX_RESPONSE {
            return Err(too_long());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// The resolver: a name connects only if every address it has is public
/// (a name with one private address among public ones is refused), except
/// the names the deployment chose ([`Sender::trust_origin`], the proxy).
struct PublicOnly {
    open: HashSet<String>,
    lookup: Lookup,
}

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let name = name.as_str().trim_end_matches('.').to_ascii_lowercase();
        let open = self.open.contains(&name);
        let lookup = (self.lookup)(name.clone());
        Box::pin(async move {
            let addrs = screen(&name, lookup.await?, open)?;
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The addresses `name` may connect to, or why none.
fn screen(name: &str, addrs: Vec<SocketAddr>, open: bool) -> std::io::Result<Vec<SocketAddr>> {
    let refuse = |why: &str| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{name}: {why}"),
        )
    };
    if addrs.is_empty() {
        return Err(refuse("no address"));
    }
    if !open && addrs.iter().any(|a| !crate::endpoint::is_public(a.ip())) {
        return Err(refuse("resolves to a private or reserved address"));
    }
    Ok(addrs)
}

fn cached(
    slot: &Mutex<Option<(String, Instant)>>,
    life: Duration,
    make: impl FnOnce() -> String,
) -> String {
    let mut slot = slot.lock().unwrap();
    if let Some((t, until)) = &*slot
        && *until > Instant::now()
    {
        return t.clone();
    }
    let t = make();
    *slot = Some((t.clone(), Instant::now() + life));
    t
}

fn check(
    status: u16,
    body: &[u8],
    error: impl Fn(u16, &[u8]) -> PushError,
) -> Result<(), PushError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        let mut end = body.len().min(MAX_REASON);
        while end > 0 && end < body.len() && std::str::from_utf8(&body[..end]).is_err() {
            end -= 1;
        }
        Err(error(status, &body[..end]))
    }
}

/// What a push service answered, for an error: at most [`MAX_REASON`]
/// bytes, on one line (control characters as spaces), without the
/// subscription's `endpoint` nor the long parts of its path (its id, a
/// capability), should the service repeat them (S34).
fn reason(body: &[u8], endpoint: &str) -> String {
    let mut text = String::from_utf8_lossy(body).into_owned();
    if !endpoint.is_empty() {
        text = text.replace(endpoint, "<endpoint>");
        let path = endpoint
            .split_once("://")
            .map_or(endpoint, |(_, rest)| rest);
        for part in path
            .split(['/', '?', '&', '=', '#'])
            .filter(|p| p.len() >= 16)
        {
            text = text.replace(part, "<redacted>");
        }
    }
    let mut text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.len() > MAX_REASON {
        let mut end = MAX_REASON;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// S34: what a push service answered, in an error, is short, on one
    /// line, and does not repeat the subscription's endpoint.
    #[test]
    fn s34_rejections_do_not_repeat_the_endpoint() {
        let endpoint = "https://push.example/wpush/v2/gAAAAAS34MARKopaque0123456789?k=S34MARKq";
        let body = format!(
            "no subscription at {endpoint} (/wpush/v2/gAAAAAS34MARKopaque0123456789)\r\nX-Injected: 1\x1b[2J{}",
            "x".repeat(4000)
        );
        let reason = super::reason(body.as_bytes(), endpoint);
        assert!(!reason.contains("S34MARK"), "{reason}");
        assert!(!reason.contains(['\r', '\n', '\x1b']), "{reason:?}");
        assert!(reason.len() <= MAX_REASON + 3, "{}", reason.len());
        assert!(reason.starts_with("no subscription at "), "{reason}");
    }
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn addr(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), 0)
    }

    #[test]
    fn s5_names_with_any_private_address_are_refused() {
        assert!(
            screen(
                "p.example",
                vec![addr("8.8.8.8"), addr("2a00:1450::1")],
                false
            )
            .is_ok()
        );
        for bad in [
            vec![addr("10.0.0.1")],
            vec![addr("169.254.169.254")],
            vec![addr("fe80::1")],
            vec![addr("::ffff:192.168.0.1")],
            // Mixed: one public, one private.
            vec![addr("8.8.8.8"), addr("127.0.0.1")],
            vec![],
        ] {
            assert!(screen("p.example", bad.clone(), false).is_err(), "{bad:?}");
        }
        // A name the deployment chose.
        assert!(screen("ntfy.lan", vec![addr("192.168.1.2")], true).is_ok());
    }

    /// A public-looking name that resolves to a loopback server: the
    /// sender refuses it before connecting.
    #[test]
    fn s5_a_name_resolving_to_a_private_address_is_never_connected() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let lookups = Arc::new(AtomicUsize::new(0));
        let count = lookups.clone();
        let sender = Sender::with_lookup(Arc::new(move |_name: String| {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))]) })
        }));
        let keys = crate::webpush::ReceiverKeys::generate();
        let sub = Subscription::from_json(
            &serde_json::json!({
                "service": "webpush",
                "endpoint": "https://push.example/abc",
                "keys": {"p256dh": keys.p256dh(), "auth": keys.auth()},
            })
            .to_string(),
        )
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt
            .block_on(sender.send(&sub, &Push::new().title("hi")))
            .unwrap_err();
        assert!(matches!(err, PushError::Http(_)), "{err}");
        assert_eq!(lookups.load(Ordering::SeqCst), 1);
        assert!(listener.accept().is_err(), "the sender connected");
    }
}
