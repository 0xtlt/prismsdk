//! OAuth 2 / OpenID Connect sign-in for native and web apps (RFC 8252):
//! the authorization code flow with PKCE (S256), in the system's sign-in
//! browser.
//!
//! ```ignore
//! use prism::oauth::OAuth;
//! cx.spawn(async move |this, cx| {
//!     let google = OAuth::google("1234-abc.apps.googleusercontent.com");
//!     match google.sign_in(&cx).await {
//!         Ok(tokens) => save(tokens.refresh_token),
//!         Err(OAuthError::Cancelled) => {}
//!         Err(e) => show(e),
//!     }
//! });
//! ```
//!
//! | | Browser | Redirect caught by |
//! |---|---|---|
//! | Apple | `ASWebAuthenticationSession` | its callback scheme (`prism-<package>:/oauth`: the cargo package, `_` written `-`; or the provider's) |
//! | Android | Custom Tab (or the default browser) | the app's deep-link scheme |
//! | Linux | the default browser | a loopback server (`http://127.0.0.1:<port>/callback`) |
//! | Web | a popup | the server (`<base>/_prism/auth/callback`) |
//!
//! The code is exchanged at the token endpoint from the app (public client,
//! no secret). With [`OAuth::code_only`], the app gets the code and the
//! PKCE verifier to exchange on its own server instead.
//!
//! Endpoints come from the issuer's metadata: RFC 8414
//! (`/.well-known/oauth-authorization-server`) or OpenID Connect discovery
//! (`/.well-known/openid-configuration`), whichever it publishes. A server
//! with no client registered for the app can register one (RFC 7591,
//! [`OAuth::dynamic_registration`]): once, then kept in secure storage.
//!
//! ```ignore
//! let server = OAuth::server("https://git.example.com", "")
//!     .dynamic_registration("My App")
//!     .scope("read");
//! let server = server.resolve(&cx).await?;   // endpoints + client id: keep it
//! let tokens = server.sign_in(&cx).await?;
//! // Later, on any thread (a blocking API client after a 401):
//! let fresh = server.refresh_blocking(tokens.refresh_token.as_deref().unwrap())?;
//! // Signing out: the server forgets the refresh token (RFC 7009).
//! server.revoke(&cx, &refresh_token, Some(TokenTypeHint::RefreshToken)).await?;
//! ```

use std::collections::HashMap;

use prism_core::{AsyncApp, DeviceError, WebAuthRequest};
use sha2::{Digest, Sha256};

use crate::{Hidden, Redirects, Request};

/// The most bytes of a metadata document or of a token, registration or
/// revocation endpoint's answer.
const MAX_ANSWER: u64 = 1 << 20;

/// How long after its expiry an ID token is still accepted (clocks
/// differ), in seconds.
const CLOCK_SKEW: u64 = 300;

const GOOGLE_ISSUER: &str = "https://accounts.google.com";

/// A provider and the client registered with it.
#[derive(Clone, Default)]
pub struct OAuth {
    /// Shown by the web's "Continue" button when a popup is blocked.
    pub name: String,
    /// The issuer: endpoints left empty are discovered from its metadata
    /// (RFC 8414 or OpenID Connect, see [`discovery_urls`]).
    pub issuer: Option<String>,
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    /// Only for confidential clients on a server (the web backend). Never
    /// ship one in a native app.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    /// A redirect URI the provider requires (Google's
    /// `com.googleusercontent.apps.<id>:/oauth2redirect` on iOS). Platforms
    /// that cannot catch it use their own (see the module docs).
    pub redirect_uri: Option<String>,
    /// Extra authorization parameters (`prompt`, `login_hint`, `audience`…).
    pub params: Vec<(String, String)>,
    /// No shared browser cookies (Apple).
    pub ephemeral: bool,
    /// Return the code instead of exchanging it.
    pub code_only: bool,

    // --- Dynamic client registration (RFC 7591) ---
    /// The registration endpoint (discovered: `registration_endpoint`).
    pub registration_url: String,
    /// Register a client under this name when `client_id` is empty (see
    /// [`OAuth::dynamic_registration`]).
    pub client_name: Option<String>,
    /// An initial access token the registration endpoint requires (sent as
    /// a bearer token), if any.
    pub registration_token: Option<String>,
    /// How the token endpoint expects the client secret:
    /// `client_secret_basic` (HTTP Basic) or `client_secret_post` (in the
    /// form, the default). Set from a registration's answer.
    pub token_endpoint_auth_method: Option<String>,

    // --- Token revocation (RFC 7009) ---
    /// The revocation endpoint (discovered: `revocation_endpoint`), for
    /// [`revoke`](OAuth::revoke).
    pub revocation_url: String,
}

/// What the provider granted.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: Option<String>,
    pub token_type: Option<String>,
    pub refresh_token: Option<String>,
    /// The ID token (OpenID Connect). [`OAuth::sign_in`] and the refreshes
    /// checked its issuer, audience, expiry, subject and nonce; not its
    /// signature (it came straight from the token endpoint, over HTTPS).
    /// Verify it with the provider's keys before trusting it on a server.
    pub id_token: Option<String>,
    /// Seconds.
    pub expires_in: Option<u64>,
    pub scope: Option<String>,
    /// The authorization code (already used unless [`OAuth::code_only`]).
    pub code: String,
    /// The PKCE verifier that goes with `code` (for a server-side exchange).
    pub code_verifier: String,
    /// The redirect URI used (the token request must repeat it).
    pub redirect_uri: String,
}

impl Tokens {
    /// The ID token's claims (`sub`, `email`, `name`…), decoded without
    /// checking the signature: fine for display; verify it on your server
    /// before trusting it there.
    pub fn id_claims(&self) -> Option<serde_json::Value> {
        jwt_claims(self.id_token.as_deref()?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthError {
    /// The user closed the sign-in window.
    Cancelled,
    /// No sign-in browser on this platform.
    Unavailable,
    /// The provider said no (`access_denied`, `invalid_grant`…).
    Provider {
        error: String,
        description: Option<String>,
    },
    /// The redirect did not match this sign-in (CSRF protection).
    StateMismatch,
    /// The ID token is missing, malformed, or not issued for this sign-in
    /// (issuer, audience, expiry, subject, nonce).
    InvalidIdToken(String),
    Network(String),
    Failed(String),
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OAuthError::Cancelled => f.write_str("cancelled"),
            OAuthError::Unavailable => f.write_str("no sign-in browser on this device"),
            OAuthError::Provider {
                error,
                description: Some(d),
            } => write!(f, "{error}: {d}"),
            OAuthError::Provider { error, .. } => f.write_str(error),
            OAuthError::StateMismatch => f.write_str("state mismatch"),
            OAuthError::InvalidIdToken(m) => write!(f, "invalid ID token: {m}"),
            OAuthError::Network(m) | OAuthError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for OAuthError {}

impl From<DeviceError> for OAuthError {
    fn from(e: DeviceError) -> Self {
        match e {
            DeviceError::Cancelled => OAuthError::Cancelled,
            DeviceError::Unavailable => OAuthError::Unavailable,
            DeviceError::Denied => OAuthError::Provider {
                error: "access_denied".into(),
                description: None,
            },
            DeviceError::Failed(m) | DeviceError::Platform { message: m, .. } => {
                OAuthError::Failed(m)
            }
        }
    }
}

impl OAuth {
    pub fn new(
        authorize_url: impl Into<String>,
        token_url: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        OAuth {
            authorize_url: authorize_url.into(),
            token_url: token_url.into(),
            client_id: client_id.into(),
            ..Default::default()
        }
    }

    /// An OpenID Connect provider: endpoints come from its discovery
    /// document; scopes start with `openid`.
    pub fn oidc(issuer: impl Into<String>, client_id: impl Into<String>) -> Self {
        let issuer = issuer.into().trim_end_matches('/').to_string();
        OAuth {
            name: issuer
                .split("://")
                .nth(1)
                .unwrap_or(&issuer)
                .split('/')
                .next()
                .unwrap_or_default()
                .to_string(),
            issuer: Some(issuer),
            client_id: client_id.into(),
            scopes: vec!["openid".into()],
            ..Default::default()
        }
    }

    /// A plain OAuth 2 authorization server (RFC 8414): endpoints come from
    /// its metadata (`/.well-known/oauth-authorization-server`, or OpenID
    /// Connect discovery if that is what it publishes); no scopes. Pass an
    /// empty `client_id` with [`dynamic_registration`](Self::dynamic_registration)
    /// when the app has none.
    pub fn server(issuer: impl Into<String>, client_id: impl Into<String>) -> Self {
        OAuth {
            scopes: Vec::new(),
            ..Self::oidc(issuer, client_id)
        }
    }

    /// Google (OpenID Connect). With an iOS client id
    /// (`<id>.apps.googleusercontent.com`), Apple platforms redirect to the
    /// reversed id, as Google requires.
    pub fn google(client_id: impl Into<String>) -> Self {
        let client_id = client_id.into();
        let redirect = client_id
            .strip_suffix(".apps.googleusercontent.com")
            .map(|id| format!("com.googleusercontent.apps.{id}:/oauth2redirect"));
        OAuth {
            name: "Google".into(),
            issuer: Some("https://accounts.google.com".into()),
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            revocation_url: "https://oauth2.googleapis.com/revoke".into(),
            client_id,
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
            redirect_uri: redirect,
            ..Default::default()
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
    pub fn scope(mut self, scope: impl Into<String>) -> Self {
        let scope = scope.into();
        if !self.scopes.contains(&scope) {
            self.scopes.push(scope);
        }
        self
    }
    pub fn redirect_uri(mut self, uri: impl Into<String>) -> Self {
        self.redirect_uri = Some(uri.into());
        self
    }
    pub fn param(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.push((key.into(), value.into()));
        self
    }
    pub fn client_secret(mut self, secret: impl Into<String>) -> Self {
        self.client_secret = Some(secret.into());
        self
    }
    pub fn ephemeral(mut self) -> Self {
        self.ephemeral = true;
        self
    }
    pub fn code_only(mut self) -> Self {
        self.code_only = true;
        self
    }

    /// When `client_id` is empty, registers a client named `client_name`
    /// at the server's registration endpoint (RFC 7591): a public client
    /// (`token_endpoint_auth_method: none`) with this platform's redirect
    /// URI and the `authorization_code` and `refresh_token` grants. The
    /// answer (client id, and a secret if the server issues one) is kept in
    /// secure storage, so the app registers once per device (again if the
    /// redirect URI changes, the secret expires, or the server forgets the
    /// client). Where there is no secure storage it registers at every launch.
    pub fn dynamic_registration(mut self, client_name: impl Into<String>) -> Self {
        self.client_name = Some(client_name.into());
        self
    }

    /// An initial access token for the registration endpoint (RFC 7591 §3),
    /// for servers that only let known apps register.
    pub fn registration_token(mut self, token: impl Into<String>) -> Self {
        self.registration_token = Some(token.into());
        self
    }

    /// The client registered by [`register_blocking`](Self::register_blocking)
    /// (or kept from an earlier run): its id, secret and auth method.
    pub fn with_registration(mut self, r: &Registration) -> Self {
        self.client_id = r.client_id.clone();
        self.client_secret = r.client_secret.clone();
        self.token_endpoint_auth_method = r.token_endpoint_auth_method.clone();
        self
    }

    fn wants_registration(&self) -> bool {
        self.client_id.is_empty() && self.client_name.is_some()
    }

    fn needs_discovery(&self) -> bool {
        self.issuer.is_some()
            && (self.authorize_url.is_empty()
                || self.token_url.is_empty()
                || (self.wants_registration() && self.registration_url.is_empty()))
    }

    fn is_oidc(&self) -> bool {
        self.scopes.iter().any(|s| s == "openid")
    }

    /// The authorization URL for one sign-in.
    pub fn authorization_url(
        &self,
        redirect: &str,
        pkce: &Pkce,
        state: &str,
        nonce: Option<&str>,
    ) -> String {
        let mut q: Vec<(&str, &str)> = vec![
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", redirect),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
        ];
        let scope = self.scopes.join(" ");
        if !scope.is_empty() {
            q.push(("scope", &scope));
        }
        if let Some(n) = nonce {
            q.push(("nonce", n));
        }
        for (k, v) in &self.params {
            q.push((k, v));
        }
        let sep = if self.authorize_url.contains('?') {
            '&'
        } else {
            '?'
        };
        format!("{}{sep}{}", self.authorize_url, form(&q))
    }

    /// Fills the endpoints from the issuer's metadata: each of
    /// [`discovery_urls`] in turn until one answers with a document for
    /// this issuer (RFC 8414 §3.3: its `issuer` must match). Does nothing
    /// when the endpoints are already known.
    pub fn discover_blocking(&mut self) -> Result<(), OAuthError> {
        if !self.needs_discovery() {
            return Ok(());
        }
        self.fetch_metadata_blocking()
    }

    /// [`discover_blocking`](Self::discover_blocking) whatever is known
    /// already: only the endpoints still empty are filled.
    fn fetch_metadata_blocking(&mut self) -> Result<(), OAuthError> {
        let Some(issuer) = self.issuer.clone() else {
            return Ok(());
        };
        secure("issuer", &issuer)?;
        let mut last = OAuthError::Failed(format!("discovery: no metadata for {issuer}"));
        for url in discovery_urls(&issuer, self.is_oidc()) {
            let res = match Request::get(&url)
                .header("accept", "application/json")
                .redirects(Redirects::SameOrigin)
                .max_body(MAX_ANSWER)
                .send_blocking()
            {
                Ok(res) => res,
                Err(e) => {
                    last = OAuthError::Network(e.0);
                    continue;
                }
            };
            if !res.ok() {
                last = OAuthError::Failed(format!("discovery: HTTP {} from {url}", res.status));
                continue;
            }
            match serde_json::from_slice::<serde_json::Value>(&res.body) {
                Ok(doc) if doc.is_object() => match self.apply_metadata(&issuer, &doc) {
                    Ok(()) => return Ok(()),
                    Err(e) => last = e,
                },
                Ok(_) => {
                    last = OAuthError::Failed(format!("discovery: not a JSON object at {url}"))
                }
                Err(e) => last = OAuthError::Failed(format!("discovery: {e} at {url}")),
            }
        }
        Err(last)
    }

    /// Takes the endpoints left empty from a metadata document (RFC 8414
    /// or OpenID Connect), after checking it is `issuer`'s.
    pub fn apply_metadata(
        &mut self,
        issuer: &str,
        doc: &serde_json::Value,
    ) -> Result<(), OAuthError> {
        let field = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(str::to_string);
        // RFC 8414 §3.3, OpenID Connect Discovery §4.3: the document names
        // its issuer, and it is this one.
        let claimed = field("issuer").unwrap_or_default();
        if !same_issuer(&claimed, issuer) {
            return Err(OAuthError::Failed(format!(
                "discovery: metadata for {claimed:?}, not {issuer}"
            )));
        }
        // Every endpoint checked before any is taken.
        let endpoint = |k: &str| -> Result<Option<String>, OAuthError> {
            match field(k) {
                Some(url) => secure(k, &url).map(|()| Some(url)),
                None => Ok(None),
            }
        };
        let authorize = endpoint("authorization_endpoint")?;
        let token = endpoint("token_endpoint")?;
        let registration = endpoint("registration_endpoint")?;
        let revocation = endpoint("revocation_endpoint")?;
        if self.authorize_url.is_empty() {
            self.authorize_url = authorize
                .ok_or_else(|| OAuthError::Failed("discovery: no authorization_endpoint".into()))?;
        }
        if self.token_url.is_empty() {
            self.token_url = token.unwrap_or_default();
        }
        if self.registration_url.is_empty() {
            self.registration_url = registration.unwrap_or_default();
        }
        if self.revocation_url.is_empty() {
            self.revocation_url = revocation.unwrap_or_default();
        }
        Ok(())
    }

    /// This provider ready to use: endpoints discovered, and a client id
    /// (registered once if [`dynamic_registration`](Self::dynamic_registration)
    /// asks for it, then read back from secure storage). Keep it for
    /// [`refresh_blocking`](Self::refresh_blocking) on other threads.
    pub async fn resolve(&self, cx: &AsyncApp) -> Result<OAuth, OAuthError> {
        self.resolve_with_redirect(cx)
            .await
            .map(|(config, _)| config)
    }

    /// [`resolve`](Self::resolve), and the redirect URI if it asked for one
    /// (a loopback redirect is a listening socket: use it, not another).
    async fn resolve_with_redirect(
        &self,
        cx: &AsyncApp,
    ) -> Result<(OAuth, Option<String>), OAuthError> {
        let mut config = self.clone();
        if config.needs_discovery() {
            config = cx
                .background(move || config.discover_blocking().map(|_| config))
                .await?;
        }
        if !config.wants_registration() {
            return Ok((config, None));
        }
        let redirect = cx.auth_redirect(config.redirect_uri.as_deref()).await?;
        let key = config.registration_key();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let stored = cx
            .secure_storage()
            .get(&key)
            .await
            .ok()
            .flatten()
            .and_then(|s| Registration::from_json(&s))
            .filter(|r| r.fits(&redirect, now));
        let registration = match stored {
            Some(r) => r,
            None => {
                let (c, uri) = (config.clone(), redirect.clone());
                let r = cx.background(move || c.register_blocking(&uri)).await?;
                // Best effort: without secure storage, register next time again.
                let _ = cx.secure_storage().set(&key, &r.to_json()).await;
                r
            }
        };
        Ok((config.with_registration(&registration), Some(redirect)))
    }

    /// Registers a client for `redirect_uri` (RFC 7591), on this thread.
    /// [`resolve`](Self::resolve) calls it when needed and keeps the answer.
    pub fn register_blocking(&self, redirect_uri: &str) -> Result<Registration, OAuthError> {
        let mut config = self.clone();
        config.discover_blocking()?;
        if config.registration_url.is_empty() {
            return Err(OAuthError::Failed(
                "registration: the server has no registration_endpoint".into(),
            ));
        }
        secure("registration_endpoint", &config.registration_url)?;
        let res = config
            .registration_request(redirect_uri)
            .send_blocking()
            .map_err(|e| OAuthError::Network(e.0))?;
        let mut r = parse_registration_response(res.status, &res.body)?;
        if r.redirect_uris.is_empty() {
            r.redirect_uris.push(redirect_uri.to_string());
        }
        Ok(r)
    }

    /// The registration request (RFC 7591 §3.1) for `redirect_uri`.
    pub fn registration_request(&self, redirect_uri: &str) -> Request {
        let web = redirect_uri.starts_with("https://")
            || (redirect_uri.starts_with("http://") && !is_loopback(redirect_uri));
        let mut body = serde_json::json!({
            "client_name": self.client_name.clone().unwrap_or_else(|| self.name.clone()),
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
            // OpenID Connect Dynamic Registration: custom schemes and
            // loopback redirects are native apps.
            "application_type": if web { "web" } else { "native" },
        });
        if !self.scopes.is_empty() {
            body["scope"] = self.scopes.join(" ").into();
        }
        let mut request = Request::post(&self.registration_url)
            .header("accept", "application/json")
            .redirects(Redirects::None)
            .max_body(MAX_ANSWER)
            .json(body.to_string());
        if let Some(token) = &self.registration_token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request
    }

    /// The secure storage key of the client registered with this server.
    fn registration_key(&self) -> String {
        let server = self.issuer.as_deref().unwrap_or(&self.registration_url);
        let name = self.client_name.as_deref().unwrap_or_default();
        let hash = Sha256::digest(format!("{server}\n{name}").as_bytes());
        format!("prism.oauth.client.{}", base64url(&hash[..12]))
    }

    /// Signs in: opens the provider in the platform's sign-in browser, checks
    /// the redirect, exchanges the code (PKCE) unless [`code_only`](Self::code_only).
    pub async fn sign_in(&self, cx: &AsyncApp) -> Result<Tokens, OAuthError> {
        let (config, redirect) = self.resolve_with_redirect(cx).await?;
        let redirect = match redirect {
            Some(r) => r,
            None => cx.auth_redirect(config.redirect_uri.as_deref()).await?,
        };
        secure("authorization_endpoint", &config.authorize_url)?;
        let pkce = Pkce::new();
        let state = random_token(16);
        let nonce = config.is_oidc().then(|| random_token(16));
        let url = config.authorization_url(&redirect, &pkce, &state, nonce.as_deref());
        let callback = cx
            .web_auth(WebAuthRequest {
                url,
                redirect: redirect.clone(),
                ephemeral: config.ephemeral,
                provider: config.name.clone(),
            })
            .await?;
        let code = parse_callback(&callback, &state)?;
        let mut tokens = Tokens {
            code,
            code_verifier: pkce.verifier,
            redirect_uri: redirect,
            ..Default::default()
        };
        if config.code_only || config.token_url.is_empty() {
            return Ok(tokens);
        }
        let request = config.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", &tokens.code),
            ("redirect_uri", &tokens.redirect_uri),
            ("code_verifier", &tokens.code_verifier),
        ]);
        let granted = cx.background(move || exchange(request)).await;
        let granted = self.forget_rejected_client(cx, granted).await?;
        tokens = Tokens {
            code: tokens.code,
            code_verifier: tokens.code_verifier,
            redirect_uri: tokens.redirect_uri,
            ..granted
        };
        config.check_tokens(&tokens, nonce.as_deref(), unix_now())?;
        Ok(tokens)
    }

    /// New tokens from a refresh token.
    pub async fn refresh(&self, cx: &AsyncApp, refresh_token: &str) -> Result<Tokens, OAuthError> {
        let config = self.resolve(cx).await?;
        let refresh_token = refresh_token.to_string();
        let r = cx
            .background(move || config.refresh_blocking(&refresh_token))
            .await;
        self.forget_rejected_client(cx, r).await
    }

    /// New tokens from a refresh token, on this thread (like
    /// [`Request::send_blocking`]): for code already on a background thread,
    /// such as an API client that refreshes after a 401 and retries.
    /// Discovers the endpoints first if they are unknown (keep the provider
    /// from [`resolve`](Self::resolve) to do it once). The refresh token
    /// stays the same unless the server rotates it.
    pub fn refresh_blocking(&self, refresh_token: &str) -> Result<Tokens, OAuthError> {
        let mut config = self.clone();
        config.discover_blocking()?;
        if config.token_url.is_empty() {
            return Err(OAuthError::Failed("no token endpoint".into()));
        }
        if config.client_id.is_empty() {
            return Err(OAuthError::Failed(
                "no client id: refresh with the provider resolve() returned".into(),
            ));
        }
        let mut t = exchange(config.token_request(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ]))?;
        // A new ID token, if any, is checked like the first (no nonce).
        if t.id_token.is_some() {
            config.check_tokens(&t, None, unix_now())?;
        }
        t.refresh_token
            .get_or_insert_with(|| refresh_token.to_string());
        Ok(t)
    }

    /// Revokes a token (RFC 7009), when signing out: the server forgets
    /// it, and a refresh token's access tokens usually with it. `hint` says
    /// which kind it is (the server looks for it there first). A token the
    /// server does not know (already revoked, expired) is not an error.
    pub async fn revoke(
        &self,
        cx: &AsyncApp,
        token: &str,
        hint: Option<TokenTypeHint>,
    ) -> Result<(), OAuthError> {
        let config = self.resolve(cx).await?;
        let token = token.to_string();
        let r = cx
            .background(move || config.revoke_blocking(&token, hint))
            .await;
        self.forget_rejected_client(cx, r).await
    }

    /// [`revoke`](Self::revoke) on this thread (like
    /// [`refresh_blocking`](Self::refresh_blocking)). The revocation
    /// endpoint comes from the issuer's metadata (`revocation_endpoint`)
    /// when [`revocation_url`](Self::revocation_url) is empty; a server
    /// that publishes none is an error: nothing to call.
    pub fn revoke_blocking(
        &self,
        token: &str,
        hint: Option<TokenTypeHint>,
    ) -> Result<(), OAuthError> {
        let mut config = self.clone();
        config.discover_blocking()?;
        if config.revocation_url.is_empty() && config.issuer.is_some() {
            // The other endpoints are known, not this one: read the metadata.
            config.fetch_metadata_blocking()?;
        }
        if config.revocation_url.is_empty() {
            return Err(OAuthError::Failed("no revocation endpoint".into()));
        }
        if config.client_id.is_empty() {
            return Err(OAuthError::Failed(
                "no client id: revoke with the provider resolve() returned".into(),
            ));
        }
        secure("revocation_endpoint", &config.revocation_url)?;
        let res = config
            .revocation_request(token, hint)
            .send_blocking()
            .map_err(|e| OAuthError::Network(e.0))?;
        parse_revocation_response(res.status, &res.body)
    }

    /// The revocation request (RFC 7009 §2.1) for `token`, with the
    /// client's credentials as for [`token_request`](Self::token_request).
    pub fn revocation_request(&self, token: &str, hint: Option<TokenTypeHint>) -> Request {
        let mut fields = vec![("token", token)];
        if let Some(hint) = hint {
            fields.push(("token_type_hint", hint.as_str()));
        }
        self.client_request(&self.revocation_url, &fields)
    }

    /// Checks the ID token of a token endpoint's answer: required when
    /// signing in with OpenID Connect (`openid` scope), checked whenever
    /// present (see [`check_id_token`]). A plain OAuth answer needs none.
    fn check_tokens(
        &self,
        tokens: &Tokens,
        nonce: Option<&str>,
        now: u64,
    ) -> Result<(), OAuthError> {
        match tokens.id_token.as_deref() {
            Some(token) => check_id_token(token, self, nonce, now).map(|_| ()),
            None if self.is_oidc() && nonce.is_some() => {
                Err(OAuthError::InvalidIdToken("missing".into()))
            }
            None => Ok(()),
        }
    }

    /// A registered client the server no longer knows (`invalid_client`):
    /// forget it, so the next sign-in registers again.
    async fn forget_rejected_client<T>(
        &self,
        cx: &AsyncApp,
        r: Result<T, OAuthError>,
    ) -> Result<T, OAuthError> {
        // `client_name`: also a provider `resolve` already filled in.
        if self.client_name.is_some()
            && matches!(&r, Err(OAuthError::Provider { error, .. }) if error == "invalid_client")
        {
            let _ = cx.secure_storage().delete(&self.registration_key()).await;
        }
        r
    }

    /// A request to the token endpoint with `fields` (`grant_type`…) and the
    /// client's credentials: `client_id` in the form, and the secret in it
    /// too or as HTTP Basic (`client_secret_basic`).
    pub fn token_request(&self, fields: &[(&str, &str)]) -> Request {
        self.client_request(&self.token_url, fields)
    }

    /// A form post to `url` with the client's credentials (RFC 6749 §2.3.1).
    fn client_request(&self, url: &str, fields: &[(&str, &str)]) -> Request {
        let mut body: Vec<(&str, &str)> = fields.to_vec();
        let basic = self.token_endpoint_auth_method.as_deref() == Some("client_secret_basic");
        // Credentials go to this endpoint and no other (a redirect is
        // its answer), and only over HTTPS (see `exchange`).
        let mut request = Request::post(url)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .redirects(Redirects::None)
            .max_body(MAX_ANSWER);
        match &self.client_secret {
            // RFC 6749 §2.3.1: both form-encoded, then base64.
            Some(secret) if basic => {
                let pair = format!("{}:{}", encode(&self.client_id), encode(secret));
                request = request.header(
                    "authorization",
                    format!("Basic {}", base64(pair.as_bytes())),
                );
            }
            Some(secret) => {
                body.push(("client_id", &self.client_id));
                body.push(("client_secret", secret));
            }
            None => body.push(("client_id", &self.client_id)),
        }
        request.body(form(&body))
    }
}

/// Which kind of token is revoked (RFC 7009 §2.1, `token_type_hint`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenTypeHint {
    /// A refresh token (`refresh_token`): what an app revokes when the user
    /// signs out.
    RefreshToken,
    /// An access token (`access_token`).
    AccessToken,
}

impl TokenTypeHint {
    /// As sent in the form.
    pub fn as_str(self) -> &'static str {
        match self {
            TokenTypeHint::RefreshToken => "refresh_token",
            TokenTypeHint::AccessToken => "access_token",
        }
    }
}

/// A revocation endpoint's answer (RFC 7009 §2.2): any 2xx is done (an
/// unknown token included); otherwise the server's error
/// (`unsupported_token_type`, `invalid_client`…), or the HTTP status
/// (503: try again later).
pub fn parse_revocation_response(status: u16, body: &[u8]) -> Result<(), OAuthError> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let v: Option<serde_json::Value> = serde_json::from_slice(body).ok();
    let s = |k: &str| {
        v.as_ref()
            .and_then(|v| v.get(k))
            .and_then(|x| x.as_str())
            .map(str::to_string)
    };
    match s("error") {
        Some(error) => Err(OAuthError::Provider {
            error,
            description: s("error_description"),
        }),
        None => Err(OAuthError::Failed(format!(
            "revocation endpoint: HTTP {status}"
        ))),
    }
}

/// What a registration endpoint issued (RFC 7591 §3.2.1).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Registration {
    /// The client id to use from now on.
    pub client_id: String,
    /// Only if the server made a confidential client anyway.
    pub client_secret: Option<String>,
    /// When the secret expires (Unix seconds; `0` or `None`: never).
    pub client_secret_expires_at: Option<u64>,
    /// How the token endpoint expects the secret (`none` for public clients).
    pub token_endpoint_auth_method: Option<String>,
    /// For reading or updating the registration (RFC 7592).
    pub registration_access_token: Option<String>,
    /// Where to read or update the registration (RFC 7592).
    pub registration_client_uri: Option<String>,
    /// The redirect URIs registered.
    pub redirect_uris: Vec<String>,
}

impl Registration {
    /// As stored in secure storage.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "client_id": self.client_id,
            "client_secret": self.client_secret,
            "client_secret_expires_at": self.client_secret_expires_at,
            "token_endpoint_auth_method": self.token_endpoint_auth_method,
            "registration_access_token": self.registration_access_token,
            "registration_client_uri": self.registration_client_uri,
            "redirect_uris": self.redirect_uris,
        })
        .to_string()
    }

    /// From [`to_json`](Self::to_json) or a registration response.
    pub fn from_json(json: &str) -> Option<Registration> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        Self::from_value(&v)
    }

    fn from_value(v: &serde_json::Value) -> Option<Registration> {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Some(Registration {
            client_id: s("client_id").filter(|id| !id.is_empty())?,
            client_secret: s("client_secret"),
            client_secret_expires_at: v.get("client_secret_expires_at").and_then(|x| x.as_u64()),
            token_endpoint_auth_method: s("token_endpoint_auth_method"),
            registration_access_token: s("registration_access_token"),
            registration_client_uri: s("registration_client_uri"),
            redirect_uris: v
                .get("redirect_uris")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|u| u.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Still usable for `redirect` at `now` (Unix seconds): registered for
    /// it (a loopback redirect on any port, RFC 8252 §7.3), secret not expired.
    pub fn fits(&self, redirect: &str, now: u64) -> bool {
        let expired = self
            .client_secret_expires_at
            .is_some_and(|t| t != 0 && t <= now);
        let target = without_loopback_port(redirect);
        !expired
            && self
                .redirect_uris
                .iter()
                .any(|u| without_loopback_port(u) == target)
    }
}

/// A registration endpoint's answer: the client, or the server's error
/// (`invalid_redirect_uri`, `invalid_client_metadata`…).
pub fn parse_registration_response(status: u16, body: &[u8]) -> Result<Registration, OAuthError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| OAuthError::Failed(format!("registration: HTTP {status}")))?;
    if let Some(error) = v.get("error").and_then(|x| x.as_str()) {
        return Err(OAuthError::Provider {
            error: error.to_string(),
            description: v
                .get("error_description")
                .and_then(|x| x.as_str())
                .map(str::to_string),
        });
    }
    if !(200..300).contains(&status) {
        return Err(OAuthError::Failed(format!("registration: HTTP {status}")));
    }
    Registration::from_value(&v)
        .ok_or_else(|| OAuthError::Failed("registration: no client_id".into()))
}

/// The metadata documents tried for `issuer`, in order: RFC 8414
/// (`/.well-known/oauth-authorization-server`, inserted before the
/// issuer's path: `https://h/.well-known/oauth-authorization-server/tenant`)
/// and OpenID Connect discovery (`<issuer>/.well-known/openid-configuration`),
/// OpenID Connect first when `oidc_first`; then, for an issuer with a path,
/// the OpenID document inserted the RFC 8414 way (RFC 8414 §5).
pub fn discovery_urls(issuer: &str, oidc_first: bool) -> Vec<String> {
    let issuer = issuer.trim_end_matches('/');
    let after_scheme = issuer.find("://").map_or(0, |i| i + 3);
    let (origin, path) = match issuer[after_scheme..].find('/') {
        Some(i) => issuer.split_at(after_scheme + i),
        None => (issuer, ""),
    };
    let oauth = format!("{origin}/.well-known/oauth-authorization-server{path}");
    let oidc = format!("{issuer}/.well-known/openid-configuration");
    let mut urls = if oidc_first {
        vec![oidc, oauth]
    } else {
        vec![oauth, oidc]
    };
    if !path.is_empty() {
        urls.push(format!("{origin}/.well-known/openid-configuration{path}"));
    }
    urls
}

/// `http://<host>[:port]…` split at the host's end: (host, port, rest).
fn loopback_parts(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("http://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => authority,
    };
    matches!(host, "127.0.0.1" | "[::1]" | "localhost").then_some((host, &rest[end..]))
}

fn is_loopback(uri: &str) -> bool {
    loopback_parts(uri).is_some()
}

/// `http://127.0.0.1:5123/callback` → `http://127.0.0.1/callback`.
fn without_loopback_port(uri: &str) -> String {
    match loopback_parts(uri) {
        Some((host, rest)) => format!("http://{host}{rest}"),
        None => uri.to_string(),
    }
}

/// Standard base64 with padding (HTTP Basic).
fn base64(data: &[u8]) -> String {
    let mut s: String = base64url(data)
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            c => c,
        })
        .collect();
    while !s.len().is_multiple_of(4) {
        s.push('=');
    }
    s
}

fn exchange(request: Request) -> Result<Tokens, OAuthError> {
    secure("token_endpoint", &request.url)?;
    let res = request
        .send_blocking()
        .map_err(|e| OAuthError::Network(e.0))?;
    parse_token_response(res.status, &res.body)
}

/// A token endpoint's answer: tokens, or the provider's error.
pub fn parse_token_response(status: u16, body: &[u8]) -> Result<Tokens, OAuthError> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| {
        OAuthError::Failed(format!(
            "token endpoint: HTTP {status}: {}",
            String::from_utf8_lossy(body)
                .chars()
                .take(200)
                .collect::<String>()
        ))
    })?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    if let Some(error) = s("error") {
        return Err(OAuthError::Provider {
            error,
            description: s("error_description"),
        });
    }
    if !(200..300).contains(&status) {
        return Err(OAuthError::Failed(format!("token endpoint: HTTP {status}")));
    }
    Ok(Tokens {
        access_token: s("access_token"),
        token_type: s("token_type"),
        refresh_token: s("refresh_token"),
        id_token: s("id_token"),
        expires_in: v.get("expires_in").and_then(|x| {
            x.as_u64()
                .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
        }),
        scope: s("scope"),
        ..Default::default()
    })
}

/// Checks an ID token from the token endpoint (OpenID Connect Core
/// §3.1.3.7) and returns its claims: a JWT with a signing algorithm (not
/// `none`), from this provider (`iss`, when the issuer is known), for
/// this client (`aud`, and `azp` when it has several audiences), not
/// expired at `now` (Unix seconds, give or take five minutes), about a
/// subject (`sub`), for this sign-in (`nonce`, when one was sent).
///
/// The signature is not verified: the token came straight from the token
/// endpoint over HTTPS, which §3.1.3.7 step 6 accepts in its place.
fn check_id_token(
    token: &str,
    config: &OAuth,
    nonce: Option<&str>,
    now: u64,
) -> Result<serde_json::Value, OAuthError> {
    use serde_json::Value;
    let bad = |what: &str| OAuthError::InvalidIdToken(what.into());
    let object = |part: &str| {
        base64url_decode(part)
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .filter(Value::is_object)
    };
    let parts: Vec<&str> = token.split('.').collect();
    let [header, payload, _signature] = parts[..] else {
        return Err(bad("malformed"));
    };
    let (Some(header), Some(claims)) = (object(header), object(payload)) else {
        return Err(bad("malformed"));
    };
    let alg = header.get("alg").and_then(Value::as_str).unwrap_or("none");
    if alg.is_empty() || alg.eq_ignore_ascii_case("none") {
        return Err(bad("unsigned"));
    }
    let text = |k: &str| claims.get(k).and_then(Value::as_str);
    if let Some(issuer) = config.issuer.as_deref() {
        let iss = text("iss").unwrap_or_default();
        // Google also writes its issuer without the scheme.
        let google = issuer == GOOGLE_ISSUER && iss == "accounts.google.com";
        if !same_issuer(iss, issuer) && !google {
            return Err(bad("issuer"));
        }
    }
    let audiences: Vec<&str> = match claims.get("aud") {
        Some(Value::String(a)) => vec![a.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !audiences.contains(&config.client_id.as_str()) {
        return Err(bad("audience"));
    }
    if audiences.len() > 1 && text("azp") != Some(config.client_id.as_str()) {
        return Err(bad("authorized party"));
    }
    let exp = claims.get("exp").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_f64().filter(|f| *f >= 0.).map(|f| f as u64))
    });
    if !exp.is_some_and(|exp| now < exp.saturating_add(CLOCK_SKEW)) {
        return Err(bad("expired"));
    }
    if text("sub").is_none_or(str::is_empty) {
        return Err(bad("subject"));
    }
    if let Some(n) = nonce
        && text("nonce") != Some(n)
    {
        return Err(bad("nonce"));
    }
    Ok(claims)
}

/// Whether two issuer identifiers are the same (a trailing `/` aside: the
/// constructors drop it, some providers keep it).
fn same_issuer(a: &str, b: &str) -> bool {
    !a.is_empty() && a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// `Ok` when `url` may carry credentials: HTTPS, or plain HTTP to this
/// machine (`localhost`, `127.0.0.1`, `[::1]`: development servers).
fn secure(what: &str, url: &str) -> Result<(), OAuthError> {
    let https = url
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("https://"));
    if https || is_loopback(url) {
        Ok(())
    } else {
        Err(OAuthError::Failed(format!("{what} must use https: {url}")))
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The code from a redirect URL, after checking `state`.
pub fn parse_callback(url: &str, state: &str) -> Result<String, OAuthError> {
    // Parameters arrive in the query, or the fragment for some providers.
    let query = url
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or(q))
        .or_else(|| url.split_once('#').map(|(_, f)| f))
        .unwrap_or_default();
    let params = parse_query(query);
    // Once: with two, which one the app read would be a guess.
    let states = query
        .split('&')
        .filter(|kv| decode(kv.split_once('=').map_or(*kv, |(k, _)| k)) == "state")
        .count();
    if states != 1 || params.get("state").map(String::as_str) != Some(state) {
        return Err(OAuthError::StateMismatch);
    }
    if let Some(error) = params.get("error") {
        return Err(if error == "access_denied" {
            OAuthError::Cancelled
        } else {
            OAuthError::Provider {
                error: error.clone(),
                description: params.get("error_description").cloned(),
            }
        });
    }
    params
        .get("code")
        .cloned()
        .ok_or_else(|| OAuthError::Failed("no code in the redirect".into()))
}

/// A PKCE pair (RFC 7636): a random verifier and its S256 challenge.
#[derive(Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

// Secrets show hidden in `Debug` output (S34): logs keep whether there is
// one, not what.

impl std::fmt::Debug for Pkce {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &Hidden::text(&self.verifier))
            .field("challenge", &self.challenge)
            .finish()
    }
}

impl std::fmt::Debug for OAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuth")
            .field("name", &self.name)
            .field("issuer", &self.issuer)
            .field("authorize_url", &self.authorize_url)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &Hidden::opt(self.client_secret.as_deref()))
            .field("scopes", &self.scopes)
            .field("redirect_uri", &self.redirect_uri)
            .field("params", &self.params)
            .field("ephemeral", &self.ephemeral)
            .field("code_only", &self.code_only)
            .field("registration_url", &self.registration_url)
            .field("client_name", &self.client_name)
            .field(
                "registration_token",
                &Hidden::opt(self.registration_token.as_deref()),
            )
            .field(
                "token_endpoint_auth_method",
                &self.token_endpoint_auth_method,
            )
            .field("revocation_url", &self.revocation_url)
            .finish()
    }
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &Hidden::opt(self.access_token.as_deref()))
            .field("token_type", &self.token_type)
            .field("refresh_token", &Hidden::opt(self.refresh_token.as_deref()))
            .field("id_token", &Hidden::opt(self.id_token.as_deref()))
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .field("code", &Hidden::text(&self.code))
            .field("code_verifier", &Hidden::text(&self.code_verifier))
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("client_id", &self.client_id)
            .field("client_secret", &Hidden::opt(self.client_secret.as_deref()))
            .field("client_secret_expires_at", &self.client_secret_expires_at)
            .field(
                "token_endpoint_auth_method",
                &self.token_endpoint_auth_method,
            )
            .field(
                "registration_access_token",
                &Hidden::opt(self.registration_access_token.as_deref()),
            )
            .field("registration_client_uri", &self.registration_client_uri)
            .field("redirect_uris", &self.redirect_uris)
            .finish()
    }
}

impl Pkce {
    pub fn new() -> Self {
        Self::from_verifier(random_token(32))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// `n` random bytes, base64url (unpadded).
pub fn random_token(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("prism: no system random source");
    base64url(&bytes)
}

/// Base64url without padding (RFC 4648 §5).
pub fn base64url(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = match chunk.len() {
            3 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32,
            2 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8,
            _ => (chunk[0] as u32) << 16,
        };
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Decodes base64url or standard base64 (padding optional).
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
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

/// A JWT's payload, unverified.
pub fn jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    serde_json::from_slice(&base64url_decode(payload)?).ok()
}

/// `application/x-www-form-urlencoded`.
pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A query string's parameters (`+` is a space).
pub fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(v)) => {
                out.push(v);
                i += 2;
            }
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// S34: a provider's client secret, a registration's secrets, tokens and
    /// the PKCE verifier do not show when printed (`{:?}`).
    #[test]
    fn s34_oauth_secrets_print_hidden() {
        let oauth = OAuth {
            client_id: "client-1".into(),
            client_secret: Some("S34-MARK-CS".into()),
            registration_token: Some("S34-MARK-RT".into()),
            ..Default::default()
        };
        let tokens = Tokens {
            access_token: Some("S34-MARK-AT".into()),
            refresh_token: Some("S34-MARK-RF".into()),
            id_token: Some("S34-MARK-ID".into()),
            code: "S34-MARK-CODE".into(),
            code_verifier: "S34-MARK-CV".into(),
            token_type: Some("Bearer".into()),
            ..Default::default()
        };
        let registration = Registration {
            client_id: "client-1".into(),
            client_secret: Some("S34-MARK-RS".into()),
            registration_access_token: Some("S34-MARK-RAT".into()),
            ..Default::default()
        };
        let pkce = Pkce {
            verifier: "S34-MARK-PV".into(),
            challenge: "the-challenge".into(),
        };
        for shown in [
            format!("{oauth:?}"),
            format!("{oauth:#?}"),
            format!("{tokens:?}"),
            format!("{tokens:#?}"),
            format!("{registration:?}"),
            format!("{pkce:?}"),
        ] {
            assert!(!shown.contains("S34-MARK"), "{shown}");
        }
        assert!(format!("{oauth:?}").contains("client-1"));
        assert!(format!("{tokens:?}").contains("Bearer"));
        assert!(format!("{pkce:?}").contains("the-challenge"));
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        // RFC 7636, appendix B.
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let random = Pkce::new();
        assert_eq!(random.verifier.len(), 43);
        assert_ne!(random.verifier, Pkce::new().verifier);
        assert!(
            random
                .verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }

    #[test]
    fn base64url_round_trips() {
        for data in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0xfb, 0xff],
        ] {
            assert_eq!(base64url_decode(&base64url(data)).unwrap(), data);
        }
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn builds_the_authorization_url() {
        let o = OAuth::new(
            "https://id.example/auth",
            "https://id.example/token",
            "app 1",
        )
        .scope("openid")
        .scope("email")
        .param("prompt", "consent");
        let p = Pkce::from_verifier("v".repeat(43));
        let url = o.authorization_url("prism-app:/oauth", &p, "s1", Some("n1"));
        assert!(url.starts_with("https://id.example/auth?response_type=code&client_id=app%201"));
        let q = parse_query(url.split_once('?').unwrap().1);
        assert_eq!(q["redirect_uri"], "prism-app:/oauth");
        assert_eq!(q["code_challenge"], p.challenge);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["scope"], "openid email");
        assert_eq!((q["state"].as_str(), q["nonce"].as_str()), ("s1", "n1"));
        assert_eq!(q["prompt"], "consent");
    }

    #[test]
    fn checks_the_redirect() {
        assert_eq!(
            parse_callback("prism-app:/oauth?code=a%2Fb&state=s1", "s1"),
            Ok("a/b".into())
        );
        assert_eq!(
            parse_callback("http://127.0.0.1:5/callback?code=x&state=evil", "s1"),
            Err(OAuthError::StateMismatch)
        );
        // S33: a second `state` (the first wrong) is not a guess the app makes.
        assert_eq!(
            parse_callback("x:/o?state=evil&code=c&state=s1", "s1"),
            Err(OAuthError::StateMismatch)
        );
        assert_eq!(
            parse_callback("x:/o?state=s1&code=c&state=s1", "s1"),
            Err(OAuthError::StateMismatch)
        );
        assert_eq!(
            parse_callback("x:/o?error=access_denied&state=s1", "s1"),
            Err(OAuthError::Cancelled)
        );
        assert!(matches!(
            parse_callback("x:/o#error=invalid_scope&error_description=no+way&state=s1", "s1"),
            Err(OAuthError::Provider { error, description: Some(d) }) if error == "invalid_scope" && d == "no way"
        ));
    }

    #[test]
    fn parses_tokens_and_errors() {
        let t = parse_token_response(
            200,
            br#"{"access_token":"at","token_type":"Bearer","expires_in":3600,"refresh_token":"rt","id_token":"h.eyJzdWIiOiIxIiwiYXVkIjoiYyIsIm5vbmNlIjoibiJ9.s"}"#,
        )
        .unwrap();
        assert_eq!(t.access_token.as_deref(), Some("at"));
        assert_eq!(t.expires_in, Some(3600));
        let claims = t.id_claims().unwrap();
        assert_eq!(claims["sub"], "1");
        assert_eq!(
            parse_token_response(400, br#"{"error":"invalid_grant"}"#),
            Err(OAuthError::Provider {
                error: "invalid_grant".into(),
                description: None
            })
        );
    }

    #[test]
    fn google_uses_the_reversed_client_id() {
        let g = OAuth::google("123-abc.apps.googleusercontent.com");
        assert_eq!(
            g.redirect_uri.as_deref(),
            Some("com.googleusercontent.apps.123-abc:/oauth2redirect")
        );
        assert!(g.scopes.contains(&"openid".to_string()));
        assert_eq!(
            OAuth::oidc("https://id.example.com/", "c").name,
            "id.example.com"
        );
    }

    // --- RFC 8414 discovery, RFC 7591 registration, blocking refresh (#32) ---

    /// A local server: `answer(method, path, headers, body)` gives
    /// `(status, JSON body)`. Returns its origin.
    fn serve(answer: impl Fn(&str, &str, &str, &str) -> (u16, String) + Send + 'static) -> String {
        serve_with(move |m, p, h, b| {
            let (status, json) = answer(m, p, h, b);
            (status, String::new(), json)
        })
    }

    /// [`serve`], with a `Location` for answers that name one.
    fn serve_with(
        answer: impl Fn(&str, &str, &str, &str) -> (u16, String, String) + Send + 'static,
    ) -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                let mut parts = line.split_whitespace();
                let (method, path) = (
                    parts.next().unwrap_or("").to_string(),
                    parts.next().unwrap_or("").to_string(),
                );
                let (mut headers, mut len) = (String::new(), 0);
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    headers.push_str(&h);
                }
                let mut body = vec![0u8; len];
                let _ = reader.read_exact(&mut body);
                let (status, location, json) =
                    answer(&method, &path, &headers, &String::from_utf8_lossy(&body));
                let location = if location.is_empty() {
                    String::new()
                } else {
                    format!("Location: {location}\r\n")
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                );
            }
        });
        origin
    }

    #[test]
    fn discovery_urls_follow_rfc8414_path_insertion() {
        assert_eq!(
            discovery_urls("https://as.example/", false),
            [
                "https://as.example/.well-known/oauth-authorization-server",
                "https://as.example/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            discovery_urls("https://as.example/tenant/1", true),
            [
                "https://as.example/tenant/1/.well-known/openid-configuration",
                "https://as.example/.well-known/oauth-authorization-server/tenant/1",
                "https://as.example/.well-known/openid-configuration/tenant/1",
            ]
        );
    }

    #[test]
    fn metadata_must_be_the_issuers() {
        let mut o = OAuth::server("https://as.example", "c");
        let doc = serde_json::json!({
            "issuer": "https://evil.example",
            "authorization_endpoint": "https://evil.example/a",
        });
        assert!(o.apply_metadata("https://as.example", &doc).is_err());
        assert!(o.authorize_url.is_empty());
        let doc = serde_json::json!({
            "issuer": "https://as.example/",
            "authorization_endpoint": "https://as.example/a",
            "token_endpoint": "https://as.example/t",
            "registration_endpoint": "https://as.example/r",
            "revocation_endpoint": "https://as.example/revoke",
        });
        o.apply_metadata("https://as.example", &doc).unwrap();
        assert_eq!(o.revocation_url, "https://as.example/revoke");
        assert_eq!(
            (
                o.authorize_url.as_str(),
                o.token_url.as_str(),
                o.registration_url.as_str()
            ),
            (
                "https://as.example/a",
                "https://as.example/t",
                "https://as.example/r"
            )
        );
        assert!(o.scopes.is_empty());
    }

    #[test]
    fn falls_back_from_rfc8414_to_openid_connect_and_back() {
        // Only OpenID Connect discovery: a plain OAuth server tries RFC 8414 first.
        let oidc_only = serve(|_, path, headers, _| {
            if path == "/.well-known/openid-configuration" {
                let endpoints = [("authorization_endpoint", "A"), ("token_endpoint", "T")];
                (200, meta(headers, "", &endpoints))
            } else {
                (404, "{}".into())
            }
        });
        let mut o = OAuth::server(&oidc_only, "c");
        o.discover_blocking().unwrap();
        assert_eq!(
            (o.authorize_url, o.token_url),
            (format!("{oidc_only}/A"), format!("{oidc_only}/T"))
        );
        // Only RFC 8414, with the issuer's path inserted.
        let plain = serve(|_, path, headers, _| {
            if path == "/.well-known/oauth-authorization-server/realm" {
                let endpoints = [
                    ("authorization_endpoint", "A2"),
                    ("token_endpoint", "T2"),
                    ("registration_endpoint", "R2"),
                ];
                (200, meta(headers, "/realm", &endpoints))
            } else {
                (404, "{}".into())
            }
        });
        let mut o = OAuth::oidc(format!("{plain}/realm"), "c");
        o.discover_blocking().unwrap();
        assert_eq!(
            (o.authorize_url, o.registration_url),
            (format!("{plain}/A2"), format!("{plain}/R2"))
        );
        // Nothing anywhere: the last error.
        let none = serve(|_, _, _, _| (404, "{}".into()));
        assert!(matches!(
            OAuth::server(&none, "c").discover_blocking(),
            Err(OAuthError::Failed(m)) if m.contains("HTTP 404")
        ));
    }

    #[test]
    fn registers_a_public_client() {
        let origin = serve(|method, path, headers, body| match (method, path) {
            ("GET", "/.well-known/oauth-authorization-server") => (
                200,
                meta(
                    headers,
                    "",
                    &[
                        ("authorization_endpoint", "A"),
                        ("token_endpoint", "T"),
                        ("registration_endpoint", "register"),
                    ],
                ),
            ),
            ("POST", "/register") => {
                let req: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(req["client_name"], "My App");
                assert_eq!(req["token_endpoint_auth_method"], "none");
                assert_eq!(req["application_type"], "native");
                assert_eq!(req["grant_types"][1], "refresh_token");
                assert_eq!(req["scope"], "read");
                assert!(headers.contains("Bearer init-token"));
                (
                    201,
                    serde_json::json!({
                        "client_id": "dyn-1",
                        "redirect_uris": req["redirect_uris"],
                        "token_endpoint_auth_method": "none",
                    })
                    .to_string(),
                )
            }
            _ => (404, "{}".into()),
        });
        let o = OAuth::server(&origin, "")
            .dynamic_registration("My App")
            .registration_token("init-token")
            .scope("read");
        assert!(o.wants_registration() && o.needs_discovery());
        let r = o
            .register_blocking("http://127.0.0.1:5000/callback")
            .unwrap();
        assert_eq!(r.client_id, "dyn-1");
        assert_eq!(r.redirect_uris, ["http://127.0.0.1:5000/callback"]);
        // Kept as JSON; a loopback redirect fits on any port.
        let back = Registration::from_json(&r.to_json()).unwrap();
        assert_eq!(back, r);
        assert!(back.fits("http://127.0.0.1:6123/callback", 0));
        assert!(!back.fits("prism-app:/oauth", 0));
        let o = o.with_registration(&back);
        assert_eq!(o.client_id, "dyn-1");
        assert!(!o.wants_registration());
    }

    fn host(headers: &str) -> String {
        headers
            .lines()
            .find_map(|l| l.strip_prefix("host: ").or(l.strip_prefix("Host: ")))
            .unwrap_or("")
            .trim()
            .to_string()
    }

    /// A metadata document of the server that got `headers`: issuer
    /// `http://<host><path>`, endpoints `http://<host>/<name>`.
    fn meta(headers: &str, path: &str, endpoints: &[(&str, &str)]) -> String {
        let origin = format!("http://{}", host(headers));
        let mut doc = serde_json::json!({ "issuer": format!("{origin}{path}") });
        for (k, v) in endpoints {
            doc[*k] = format!("{origin}/{v}").into();
        }
        doc.to_string()
    }

    #[test]
    fn registration_answers_and_secrets() {
        assert!(matches!(
            parse_registration_response(400, br#"{"error":"invalid_redirect_uri","error_description":"no"}"#),
            Err(OAuthError::Provider { error, .. }) if error == "invalid_redirect_uri"
        ));
        assert!(parse_registration_response(201, br#"{"client_secret":"s"}"#).is_err());
        let r = parse_registration_response(
            201,
            br#"{"client_id":"c 1","client_secret":"p:w","client_secret_expires_at":100,"token_endpoint_auth_method":"client_secret_basic","redirect_uris":["x:/o"]}"#,
        )
        .unwrap();
        assert!(r.fits("x:/o", 99) && !r.fits("x:/o", 100));
        // A secret the server issued: HTTP Basic, both parts form-encoded.
        let o = OAuth::new("A", "https://as.example/t", "").with_registration(&r);
        let req = o.token_request(&[("grant_type", "refresh_token")]);
        let auth = req
            .headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert_eq!(
            base64url_decode(auth.strip_prefix("Basic ").unwrap()).unwrap(),
            b"c%201:p%3Aw"
        );
        let body = String::from_utf8(req.body.unwrap()).unwrap();
        assert_eq!(body, "grant_type=refresh_token");
        // client_secret_post: in the form.
        let o = OAuth::new("A", "T", "c").client_secret("s");
        let body = String::from_utf8(o.token_request(&[]).body.unwrap()).unwrap();
        assert_eq!(body, "client_id=c&client_secret=s");
        assert_eq!(base64(b"ab"), "YWI=");
    }

    #[test]
    fn refreshes_on_this_thread() {
        let origin = serve(|method, path, headers, body| match (method, path) {
            ("GET", "/.well-known/oauth-authorization-server") => (404, "{}".into()),
            ("GET", "/.well-known/openid-configuration") => (
                200,
                meta(
                    headers,
                    "",
                    &[("authorization_endpoint", "A"), ("token_endpoint", "TOKEN")],
                ),
            ),
            ("POST", "/token") => {
                let f = parse_query(body);
                if f["grant_type"] == "refresh_token"
                    && f["refresh_token"] == "rt"
                    && f["client_id"] == "c"
                {
                    (
                        200,
                        r#"{"access_token":"new","token_type":"Bearer","expires_in":60}"#.into(),
                    )
                } else {
                    (400, r#"{"error":"invalid_grant"}"#.into())
                }
            }
            _ => (404, "{}".into()),
        });
        let mut o = OAuth::server(&origin, "c");
        // Discovery points the token endpoint at this server.
        o.token_url = format!("{origin}/token");
        o.authorize_url = format!("{origin}/authorize");
        let t = std::thread::spawn(move || o.refresh_blocking("rt"))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(t.access_token.as_deref(), Some("new"));
        assert_eq!(t.refresh_token.as_deref(), Some("rt"));
        // Endpoints unknown: discovered first (RFC 8414 404, then OpenID).
        let mut o = OAuth::server(&origin, "c");
        o.discover_blocking().unwrap();
        assert_eq!(o.token_url, format!("{origin}/TOKEN"));
        let bad = OAuth::new("A", format!("{origin}/token"), "c");
        assert!(matches!(
            bad.refresh_blocking("wrong"),
            Err(OAuthError::Provider { error, .. }) if error == "invalid_grant"
        ));
        assert!(
            OAuth::new("A", format!("{origin}/token"), "")
                .refresh_blocking("rt")
                .is_err()
        );
    }

    // --- RFC 7009 revocation (#77) ---

    #[test]
    fn revocation_requests_and_answers() {
        let mut o = OAuth::new("A", "T", "c 1");
        o.revocation_url = "https://as.example/revoke".into();
        let req = o.revocation_request("rt/1", Some(TokenTypeHint::RefreshToken));
        assert_eq!(req.url, "https://as.example/revoke");
        let body = String::from_utf8(req.body.unwrap()).unwrap();
        assert_eq!(
            body,
            "token=rt%2F1&token_type_hint=refresh_token&client_id=c%201"
        );
        let body = String::from_utf8(o.revocation_request("at", None).body.unwrap()).unwrap();
        assert_eq!(body, "token=at&client_id=c%201");
        assert_eq!(
            OAuth::google("1-a.apps.googleusercontent.com").revocation_url,
            "https://oauth2.googleapis.com/revoke"
        );
        // 200 with any body (or none) is done; errors come back as the provider's.
        assert_eq!(parse_revocation_response(200, b""), Ok(()));
        assert!(matches!(
            parse_revocation_response(400, br#"{"error":"unsupported_token_type"}"#),
            Err(OAuthError::Provider { error, .. }) if error == "unsupported_token_type"
        ));
        assert!(matches!(
            parse_revocation_response(503, b"busy"),
            Err(OAuthError::Failed(m)) if m.contains("503")
        ));
    }

    #[test]
    fn revokes_on_this_thread() {
        let origin = serve(|method, path, headers, body| {
            match (method, path) {
                ("GET", "/.well-known/oauth-authorization-server") => (
                    200,
                    meta(
                        headers,
                        "",
                        &[
                            ("authorization_endpoint", "A"),
                            ("token_endpoint", "T"),
                            ("revocation_endpoint", "revoke"),
                        ],
                    ),
                ),
                ("POST", "/revoke") => {
                    let f = parse_query(body);
                    if f.get("client_id").map(String::as_str) != Some("c") {
                        (401, r#"{"error":"invalid_client"}"#.into())
                    } else if f.get("token_type_hint").map(String::as_str) == Some("refresh_token")
                    {
                        // Known or not, the answer is the same (RFC 7009 §2.2).
                        (200, String::new())
                    } else {
                        (400, r#"{"error":"unsupported_token_type"}"#.into())
                    }
                }
                _ => (404, "{}".into()),
            }
        });
        // Endpoints already known (as `resolve` leaves them): the revocation
        // endpoint is still read from the metadata.
        let mut o = OAuth::server(&origin, "c");
        o.authorize_url = "A".into();
        o.token_url = "T".into();
        let o2 = o.clone();
        std::thread::spawn(move || o2.revoke_blocking("rt", Some(TokenTypeHint::RefreshToken)))
            .join()
            .unwrap()
            .unwrap();
        assert!(matches!(
            o.revoke_blocking("at", Some(TokenTypeHint::AccessToken)),
            Err(OAuthError::Provider { error, .. }) if error == "unsupported_token_type"
        ));
        // Discovered along with the others.
        let mut d = OAuth::server(&origin, "c");
        d.discover_blocking().unwrap();
        assert_eq!(d.revocation_url, format!("{origin}/revoke"));
        // No client id, or a server without a revocation endpoint.
        assert!(
            OAuth::server(&origin, "")
                .revoke_blocking("rt", None)
                .is_err()
        );
        let none = serve(|_, _, headers, _| {
            (
                200,
                meta(
                    headers,
                    "",
                    &[("authorization_endpoint", "A"), ("token_endpoint", "T")],
                ),
            )
        });
        assert!(matches!(
            OAuth::server(&none, "c").revoke_blocking("rt", None),
            Err(OAuthError::Failed(m)) if m == "no revocation endpoint"
        ));
    }

    // --- Security review ---

    fn id_token(header: serde_json::Value, claims: serde_json::Value) -> String {
        format!(
            "{}.{}.c2ln",
            base64url(header.to_string().as_bytes()),
            base64url(claims.to_string().as_bytes())
        )
    }

    /// S17: an OpenID Connect sign-in needs a well-formed ID token from its
    /// provider, for this client and sign-in, not expired (fixed clock).
    #[test]
    fn s17_id_tokens_are_checked_whole() {
        const NOW: u64 = 1_700_000_000;
        let config = OAuth::oidc("https://id.example", "c");
        let rs256 = serde_json::json!({ "alg": "RS256", "kid": "k1" });
        let good = serde_json::json!({
            "iss": "https://id.example", "aud": "c", "sub": "u1",
            "nonce": "n", "exp": NOW + 600, "iat": NOW,
        });
        let with = |patch: serde_json::Value| {
            let mut c = good.clone();
            for (k, v) in patch.as_object().unwrap() {
                if v.is_null() {
                    c.as_object_mut().unwrap().remove(k);
                } else {
                    c[k] = v.clone();
                }
            }
            id_token(rs256.clone(), c)
        };
        let check = |token: &str| check_id_token(token, &config, Some("n"), NOW);
        let why = |token: &str| match check(token) {
            Err(OAuthError::InvalidIdToken(why)) => why,
            other => panic!("{other:?}"),
        };
        assert_eq!(check(&with(serde_json::json!({}))).unwrap()["sub"], "u1");
        // Within the clock skew; a trailing slash on the issuer.
        assert!(
            check(&with(
                serde_json::json!({ "exp": NOW - 200, "iss": "https://id.example/" })
            ))
            .is_ok()
        );
        for (token, reason) in [
            ("abc".to_string(), "malformed"),
            ("a.b".to_string(), "malformed"),
            (
                format!("{}.bm90IGpzb24.c2ln", base64url(b"{}")),
                "malformed",
            ),
            (
                id_token(serde_json::json!({ "alg": "none" }), good.clone()),
                "unsigned",
            ),
            (
                with(serde_json::json!({ "iss": "https://evil.example" })),
                "issuer",
            ),
            (with(serde_json::json!({ "iss": null })), "issuer"),
            (with(serde_json::json!({ "aud": "other" })), "audience"),
            (
                with(serde_json::json!({ "aud": ["c", "other"] })),
                "authorized party",
            ),
            (
                with(serde_json::json!({ "aud": ["c", "other"], "azp": "other" })),
                "authorized party",
            ),
            (with(serde_json::json!({ "exp": NOW - 400 })), "expired"),
            (with(serde_json::json!({ "exp": null })), "expired"),
            (with(serde_json::json!({ "sub": null })), "subject"),
            (with(serde_json::json!({ "sub": "" })), "subject"),
            (with(serde_json::json!({ "nonce": "replayed" })), "nonce"),
            (with(serde_json::json!({ "nonce": null })), "nonce"),
        ] {
            assert_eq!(why(&token), reason, "{token}");
        }
        assert!(
            check(&with(
                serde_json::json!({ "aud": ["c", "other"], "azp": "c" })
            ))
            .is_ok()
        );
        // Google writes its issuer without the scheme too; only Google.
        let google = OAuth::google("g");
        let token = id_token(
            rs256.clone(),
            serde_json::json!({
                "iss": "accounts.google.com", "aud": "g", "sub": "1", "exp": NOW + 60,
            }),
        );
        assert!(check_id_token(&token, &google, None, NOW).is_ok());
        assert!(check(&with(serde_json::json!({ "iss": "id.example" }))).is_err());
        // Signing in: the ID token is required with OpenID Connect, not
        // with plain OAuth.
        let tokens = |id_token: Option<String>| Tokens {
            access_token: Some("at".into()),
            id_token,
            ..Default::default()
        };
        assert_eq!(
            config.check_tokens(&tokens(None), Some("n"), NOW),
            Err(OAuthError::InvalidIdToken("missing".into()))
        );
        assert!(
            config
                .check_tokens(&tokens(Some(with(serde_json::json!({})))), Some("n"), NOW)
                .is_ok()
        );
        let plain = OAuth::server("https://as.example", "c");
        assert!(plain.check_tokens(&tokens(None), None, NOW).is_ok());
        // Metadata must name its issuer.
        let mut o = OAuth::oidc("https://id.example", "c");
        let doc = serde_json::json!({ "authorization_endpoint": "https://id.example/a" });
        assert!(o.apply_metadata("https://id.example", &doc).is_err());
    }

    /// S18: the client's credentials and tokens go to the configured
    /// endpoint and nowhere else: no redirect is followed, plain HTTP only
    /// to this machine, and answers are bounded.
    #[test]
    fn s18_credentials_never_follow_a_redirect() {
        use std::sync::{Arc, Mutex};
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let elsewhere = serve(move |_, path, _, body| {
            log.lock().unwrap().push(format!("{path} {body}"));
            (200, r#"{"access_token":"stolen","client_id":"x"}"#.into())
        });
        for status in [307, 308] {
            let to = elsewhere.clone();
            let endpoint =
                serve_with(move |_, path, _, _| (status, format!("{to}{path}"), "{}".into()));
            let mut o = OAuth::new(
                format!("{endpoint}/authorize"),
                format!("{endpoint}/token"),
                "c",
            )
            .client_secret("s3cret");
            o.revocation_url = format!("{endpoint}/revoke");
            o.registration_url = format!("{endpoint}/register");
            assert!(o.refresh_blocking("rt").is_err());
            assert!(o.revoke_blocking("rt", None).is_err());
            let mut r = o.clone();
            r.client_id.clear();
            assert!(r.register_blocking("http://127.0.0.1/cb").is_err());
        }
        assert_eq!(*seen.lock().unwrap(), Vec::<String>::new());
        // Plain HTTP elsewhere than this machine: refused before sending.
        for o in [
            OAuth::new("https://as.example/a", "http://as.example/token", "c"),
            OAuth::new("https://as.example/a", "HTTP://as.example/token", "c"),
        ] {
            assert!(matches!(
                o.refresh_blocking("rt"),
                Err(OAuthError::Failed(m)) if m.contains("https")
            ));
        }
        let mut revoke = OAuth::new("https://as.example/a", "https://as.example/t", "c");
        revoke.revocation_url = "http://as.example/revoke".into();
        assert!(matches!(
            revoke.revoke_blocking("rt", None),
            Err(OAuthError::Failed(m)) if m.contains("https")
        ));
        assert!(matches!(
            OAuth::server("http://as.example", "c").discover_blocking(),
            Err(OAuthError::Failed(m)) if m.contains("https")
        ));
        let mut o = OAuth::server("https://as.example", "c");
        let doc = serde_json::json!({
            "issuer": "https://as.example",
            "authorization_endpoint": "https://as.example/a",
            "token_endpoint": "http://as.example/t",
        });
        assert!(o.apply_metadata("https://as.example", &doc).is_err());
        assert!(
            o.authorize_url.is_empty(),
            "nothing taken from a refused document"
        );
        // Answers are bounded.
        let huge = serve(|_, _, _, _| {
            (
                200,
                format!(r#"{{"access_token":"{}"}}"#, "a".repeat(2 << 20)),
            )
        });
        assert!(matches!(
            OAuth::new("A", format!("{huge}/token"), "c").refresh_blocking("rt"),
            Err(OAuthError::Network(_))
        ));
        // The endpoint itself still answers.
        let fine = serve(|_, _, _, _| (200, r#"{"access_token":"ok"}"#.into()));
        let t = OAuth::new("A", format!("{fine}/token"), "c")
            .refresh_blocking("rt")
            .unwrap();
        assert_eq!(t.access_token.as_deref(), Some("ok"));
    }
}
