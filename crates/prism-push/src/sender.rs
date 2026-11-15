//! The async sender: one HTTP/2 client for every service, with APNs provider
//! tokens and FCM access tokens cached until shortly before they expire.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::apns::ApnsKey;
use crate::fcm::ServiceAccount;
use crate::webpush::VapidKey;
use crate::{HttpRequest, Push, PushError, Service, Subscription};

/// Sends pushes to any [`Subscription`]. Configure the services you use;
/// share one `Sender` (it is `Send + Sync`) across requests.
pub struct Sender {
    http: reqwest::Client,
    apns: Option<ApnsKey>,
    apns_token: Mutex<Option<(String, Instant)>>,
    fcm: Option<ServiceAccount>,
    fcm_token: Mutex<Option<(String, Instant)>>,
    vapid: Option<(VapidKey, String)>,
}

impl Default for Sender {
    fn default() -> Self {
        Self::new()
    }
}

impl Sender {
    pub fn new() -> Self {
        // TLS with ring (no aws-lc build), the system's certificates.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client");
        Sender {
            http,
            apns: None,
            apns_token: Mutex::new(None),
            fcm: None,
            fcm_token: Mutex::new(None),
            vapid: None,
        }
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
                let (status, body) = self.execute(req).await?;
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
                let (status, body) = self.execute(req).await?;
                check(status, &body, crate::fcm::error)
            }
            Service::WebPush | Service::UnifiedPush => {
                let vapid = self.vapid.as_ref().map(|(k, s)| (k, s.as_str()));
                let req = crate::webpush::request(sub, push, vapid, SystemTime::now())?;
                let (status, body) = self.execute(req).await?;
                check(status, &body, |status, body| match status {
                    404 | 410 => PushError::Gone,
                    _ => PushError::Rejected {
                        status,
                        reason: String::from_utf8_lossy(body).into_owned(),
                    },
                })
            }
        }
    }

    async fn fcm_access_token(&self, account: &ServiceAccount) -> Result<String, PushError> {
        if let Some((t, until)) = &*self.fcm_token.lock().unwrap()
            && *until > Instant::now()
        {
            return Ok(t.clone());
        }
        let (status, body) = self
            .execute(account.token_request(SystemTime::now()))
            .await?;
        if status != 200 {
            return Err(PushError::Rejected {
                status,
                reason: String::from_utf8_lossy(&body).into_owned(),
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

    async fn execute(&self, req: HttpRequest) -> Result<(u16, Vec<u8>), PushError> {
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
        let body = res
            .bytes()
            .await
            .map_err(|e| PushError::Http(e.to_string()))?;
        Ok((status, body.to_vec()))
    }
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
        Err(error(status, body))
    }
}
