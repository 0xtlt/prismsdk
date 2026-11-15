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
//! | Apple | `ASWebAuthenticationSession` | its callback scheme (`prism-<app>:/oauth`, or the provider's) |
//! | Android | Custom Tab (or the default browser) | the app's deep-link scheme |
//! | Linux | the default browser | a loopback server (`http://127.0.0.1:<port>/callback`) |
//! | Web | a popup | the server (`<base>/_prism/auth/callback`) |
//!
//! The code is exchanged at the token endpoint from the app (public client,
//! no secret). With [`OAuth::code_only`], the app gets the code and the
//! PKCE verifier to exchange on its own server instead.

use std::collections::HashMap;

use prism_core::{AsyncApp, DeviceError, WebAuthRequest};
use sha2::{Digest, Sha256};

use crate::Request;

/// A provider and the client registered with it.
#[derive(Clone, Debug, Default)]
pub struct OAuth {
    /// Shown by the web's "Continue" button when a popup is blocked.
    pub name: String,
    /// OpenID Connect issuer: endpoints left empty are discovered from
    /// `<issuer>/.well-known/openid-configuration`.
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
}

/// What the provider granted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: Option<String>,
    pub token_type: Option<String>,
    pub refresh_token: Option<String>,
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
    /// The ID token was not issued for this sign-in (nonce, audience).
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
            DeviceError::Failed(m) => OAuthError::Failed(m),
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

    /// Fills the endpoints from the issuer's discovery document.
    pub fn discover_blocking(&mut self) -> Result<(), OAuthError> {
        let Some(issuer) = &self.issuer else {
            return Ok(());
        };
        if !self.authorize_url.is_empty() && !self.token_url.is_empty() {
            return Ok(());
        }
        let res = Request::get(format!("{issuer}/.well-known/openid-configuration"))
            .send_blocking()
            .map_err(|e| OAuthError::Network(e.0))?;
        if !res.ok() {
            return Err(OAuthError::Failed(format!(
                "discovery: HTTP {} from {issuer}",
                res.status
            )));
        }
        let doc: serde_json::Value = serde_json::from_slice(&res.body)
            .map_err(|e| OAuthError::Failed(format!("discovery: {e}")))?;
        let field = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(str::to_string);
        if self.authorize_url.is_empty() {
            self.authorize_url = field("authorization_endpoint")
                .ok_or_else(|| OAuthError::Failed("discovery: no authorization_endpoint".into()))?;
        }
        if self.token_url.is_empty() {
            self.token_url = field("token_endpoint").unwrap_or_default();
        }
        Ok(())
    }

    /// Signs in: opens the provider in the platform's sign-in browser, checks
    /// the redirect, exchanges the code (PKCE) unless [`code_only`](Self::code_only).
    pub async fn sign_in(&self, cx: &AsyncApp) -> Result<Tokens, OAuthError> {
        let mut config = self.clone();
        if config.authorize_url.is_empty() || config.token_url.is_empty() {
            config = cx
                .background(move || config.discover_blocking().map(|_| config))
                .await?;
        }
        let redirect = cx.auth_redirect(config.redirect_uri.as_deref()).await?;
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
        let granted = cx.background(move || exchange(request)).await?;
        tokens = Tokens {
            code: tokens.code,
            code_verifier: tokens.code_verifier,
            redirect_uri: tokens.redirect_uri,
            ..granted
        };
        if let Some(claims) = tokens.id_claims() {
            check_id_token(&claims, &config.client_id, nonce.as_deref())?;
        }
        Ok(tokens)
    }

    /// New tokens from a refresh token.
    pub async fn refresh(&self, cx: &AsyncApp, refresh_token: &str) -> Result<Tokens, OAuthError> {
        let mut config = self.clone();
        let refresh_token = refresh_token.to_string();
        cx.background(move || {
            config.discover_blocking()?;
            let mut t = exchange(config.token_request(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", &refresh_token),
            ]))?;
            t.refresh_token.get_or_insert(refresh_token);
            Ok(t)
        })
        .await
    }

    fn token_request(&self, fields: &[(&str, &str)]) -> Request {
        let mut body: Vec<(&str, &str)> = fields.to_vec();
        body.push(("client_id", &self.client_id));
        if let Some(secret) = &self.client_secret {
            body.push(("client_secret", secret));
        }
        Request::post(&self.token_url)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(form(&body))
    }
}

fn exchange(request: Request) -> Result<Tokens, OAuthError> {
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

fn check_id_token(
    claims: &serde_json::Value,
    client_id: &str,
    nonce: Option<&str>,
) -> Result<(), OAuthError> {
    let aud_ok = match claims.get("aud") {
        Some(serde_json::Value::String(a)) => a == client_id,
        Some(serde_json::Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err(OAuthError::InvalidIdToken("audience".into()));
    }
    if let Some(n) = nonce
        && claims.get("nonce").and_then(|x| x.as_str()) != Some(n)
    {
        return Err(OAuthError::InvalidIdToken("nonce".into()));
    }
    Ok(())
}

/// The code from a redirect URL, after checking `state`.
pub fn parse_callback(url: &str, state: &str) -> Result<String, OAuthError> {
    // Parameters arrive in the query, or the fragment for some providers.
    let params = url
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or(q))
        .or_else(|| url.split_once('#').map(|(_, f)| f))
        .map(parse_query)
        .unwrap_or_default();
    if params.get("state").map(String::as_str) != Some(state) {
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
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
        assert!(check_id_token(&claims, "c", Some("n")).is_ok());
        assert!(check_id_token(&claims, "other", Some("n")).is_err());
        assert!(check_id_token(&claims, "c", Some("m")).is_err());
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
}
