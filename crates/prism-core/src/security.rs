//! Secrets, the device owner and sign-in.
//!
//! ```ignore
//! cx.spawn(async move |this, cx| {
//!     // Secrets: the Keychain, Android Keystore, Secret Service, the server.
//!     cx.secure_storage().set("token", &token).await?;
//!     let token = cx.secure_storage().get("token").await?;
//!     // Only after Face ID / Touch ID / a fingerprint (or the passcode).
//!     cx.secure_storage().set_with("pin", "1234", SecureOptions::new().biometric()).await?;
//!     // The same, or the app's own check where the store has none
//!     // (Windows, Linux).
//!     cx.secure_storage().set_with("pin", "1234", SecureOptions::new().confirm_owner()).await?;
//!
//!     // Is it the owner? (Face ID, Touch ID, Optic ID, fingerprint, passcode)
//!     cx.authenticate("Unlock your notes").await?;
//!
//!     // Sign in with Apple.
//!     let apple = cx.sign_in_with_apple(AppleSignIn::new().email().full_name()).await?;
//! });
//! ```
//!
//! | | Secure storage | `authenticate` | Sign in with Apple | Web sign-in (`prism::oauth`) |
//! |---|---|---|---|---|
//! | iOS, iPadOS, visionOS | Keychain | LocalAuthentication | AuthenticationServices | `ASWebAuthenticationSession` |
//! | macOS | Keychain | LocalAuthentication | AuthenticationServices | `ASWebAuthenticationSession` |
//! | watchOS | Keychain | passcode / wrist detection | AuthenticationServices | `ASWebAuthenticationSession` |
//! | tvOS | Keychain | — (no LocalAuthentication) | AuthenticationServices | `ASWebAuthenticationSession` |
//! | Android | Keystore (AES-GCM) | `BiometricPrompt` | — | Custom Tab or browser + deep link |
//! | Linux | Secret Service (D-Bus) | fprintd (fingerprint) | — | browser + loopback redirect |
//! | Windows | Credential Manager | Windows Hello (`UserConsentVerifier`) | — | browser + loopback redirect |
//! | Web | the server, per browser | WebAuthn (platform authenticator) | — | popup + server callback |
//!
//! [`Capabilities`](crate::Capabilities) say what the current platform has
//! (`secure_storage`, `biometry`, `device_auth`, `sign_in_with_apple`,
//! `web_auth`); calls elsewhere fail with [`DeviceError::Unavailable`].

use std::collections::BTreeMap;

use crate::device::{DeviceCall, DeviceError, DeviceValue};
use crate::task::AsyncApp;

/// How a secret is kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SecureOptions {
    /// Reading it needs the device owner: biometrics, or the passcode where
    /// biometrics are missing, checked by the system itself (Keychain
    /// access control, an Android Keystore key requiring authentication,
    /// the server after WebAuthn on the web, on Windows a key only Windows
    /// Hello gives back, which seals the value in Credential Manager).
    /// Changing the enrolled faces or fingers makes it unreadable on Apple
    /// platforms and Android, and so does resetting Windows Hello: store it
    /// again. On Windows, storing it asks for Hello too. Where the system
    /// has no such check (the Linux Secret Service, the macOS login
    /// keychain of an app without a provisioning profile, Windows without
    /// Hello set up), storing fails with [`DeviceError::Unavailable`] and
    /// keeps the previous secret: see [`confirm_owner`](Self::confirm_owner).
    pub biometric: bool,
    /// Like [`biometric`](Self::biometric) on Apple platforms (with a
    /// provisioning profile), Android and the web. On Windows and Linux the
    /// value is stored as it is, and Prism asks for the owner (Windows
    /// Hello, a fingerprint) before handing it to the app, without a prompt
    /// when storing; the store does not check, so another program of the
    /// same user that reads it gets the value without a prompt. Same on
    /// macOS without a provisioning profile (Touch ID, the login keychain).
    pub confirm_owner: bool,
    /// Shared with the app's extensions (widgets): the App Group's keychain
    /// access group on Apple platforms. Elsewhere the app's own store.
    pub shared: bool,
}

impl SecureOptions {
    /// Default options: not biometric, not shared.
    pub fn new() -> Self {
        Self::default()
    }
    /// Require the device owner to read the secret, checked by the store
    /// ([`biometric`](Self::biometric)).
    pub fn biometric(mut self) -> Self {
        self.biometric = true;
        self
    }
    /// Require the device owner to read the secret, checked by the store
    /// where it can, else by the app ([`confirm_owner`](Self::confirm_owner)).
    pub fn confirm_owner(mut self) -> Self {
        self.confirm_owner = true;
        self
    }
    /// Share the secret with the app's extensions.
    pub fn shared(mut self) -> Self {
        self.shared = true;
        self
    }

    /// For backends whose store has no owner check of its own (Credential
    /// Manager, the Secret Service): `Ok(true)` when the value is stored
    /// with a mark and the backend asks for the owner before returning it
    /// ([`confirm_owner`](Self::confirm_owner), where the device can
    /// authenticate), `Ok(false)` when it is stored as it is, and
    /// `Err(Unavailable)` when the owner check cannot be had (a
    /// [`biometric`](Self::biometric) secret needs the store's): store
    /// nothing then, and keep the previous secret.
    pub fn app_gate(&self, can_authenticate: bool) -> Result<bool, DeviceError> {
        match (self.biometric || self.confirm_owner, self.confirm_owner) {
            (false, _) => Ok(false),
            (true, true) if can_authenticate => Ok(true),
            (true, _) => Err(DeviceError::Unavailable),
        }
    }
}

/// What the device can check the owner with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BiometryKind {
    /// No sensor.
    #[default]
    None,
    /// Touch ID.
    TouchId,
    /// Face ID.
    FaceId,
    /// Apple Vision Pro.
    OpticId,
    /// A fingerprint sensor (Android).
    Fingerprint,
    /// Face recognition (Android).
    Face,
    /// Iris recognition (Android).
    Iris,
    /// A browser's platform authenticator (Touch ID, Windows Hello, an
    /// Android phone's lock…): the browser does not say which.
    Platform,
}

impl BiometryKind {
    /// The identifier used across the backend bridge (`touch_id`, `face_id`...).
    pub fn name(self) -> &'static str {
        match self {
            BiometryKind::None => "none",
            BiometryKind::TouchId => "touch_id",
            BiometryKind::FaceId => "face_id",
            BiometryKind::OpticId => "optic_id",
            BiometryKind::Fingerprint => "fingerprint",
            BiometryKind::Face => "face",
            BiometryKind::Iris => "iris",
            BiometryKind::Platform => "platform",
        }
    }

    /// The kind called `name` (the inverse of [`name`](Self::name)); unknown names are `None`.
    pub fn from_name(name: &str) -> Self {
        match name {
            "touch_id" => BiometryKind::TouchId,
            "face_id" => BiometryKind::FaceId,
            "optic_id" => BiometryKind::OpticId,
            "fingerprint" => BiometryKind::Fingerprint,
            "face" => BiometryKind::Face,
            "iris" => BiometryKind::Iris,
            "platform" => BiometryKind::Platform,
            _ => BiometryKind::None,
        }
    }

    /// Something that reads the body (not a passcode).
    pub fn is_biometric(self) -> bool {
        self != BiometryKind::None
    }
}

/// Biometrics and passcode, as they are now (`cx.biometry()`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Biometry {
    /// The sensor the device has (even with nothing enrolled).
    pub kind: BiometryKind,
    /// A face or finger is enrolled and the sensor can be used now.
    pub enrolled: bool,
    /// The device has a passcode, PIN or password (the fallback).
    pub passcode: bool,
    /// Too many failures: only the passcode works for now.
    pub locked_out: bool,
}

impl Biometry {
    /// `authenticate` can succeed.
    pub fn can_authenticate(&self) -> bool {
        self.enrolled || self.passcode
    }

    /// From a backend's `Fields` (`kind`, `enrolled`, `passcode`, `locked_out`).
    pub fn from_fields(f: &BTreeMap<String, String>) -> Self {
        let flag = |k: &str| f.get(k).is_some_and(|v| v == "1" || v == "true");
        Biometry {
            kind: BiometryKind::from_name(f.get("kind").map_or("none", String::as_str)),
            enrolled: flag("enrolled"),
            passcode: flag("passcode"),
            locked_out: flag("locked_out"),
        }
    }
}

/// A request to `authenticate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthRequest {
    /// Why: shown in the system prompt (Touch ID, Android, fprintd…). Face ID
    /// shows the Info.plist text (`[permissions] face_id`) instead.
    pub reason: String,
    /// Accept the device passcode/PIN when biometrics fail or are missing.
    pub passcode: bool,
}

impl AuthRequest {
    /// A request with `reason`, accepting the device passcode as a fallback.
    pub fn new(reason: impl Into<String>) -> Self {
        AuthRequest {
            reason: reason.into(),
            passcode: true,
        }
    }
    /// Biometrics only (no passcode fallback). Where the system's owner
    /// check cannot be limited to biometrics (the web's WebAuthn, Windows
    /// Hello, watchOS), it may accept a PIN or password, so the request
    /// fails with [`DeviceError::Unavailable`] there.
    pub fn biometrics_only(mut self) -> Self {
        self.passcode = false;
        self
    }
}

/// What Sign in with Apple asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppleSignIn {
    /// Ask for the user's email address.
    pub email: bool,
    /// Ask for the user's full name.
    pub full_name: bool,
    /// The nonce your server handed out for this sign-in, as it handed it
    /// out. Apple gets its SHA-256 in lower-case hex
    /// ([`apple_nonce`](Self::apple_nonce)), which the identity token
    /// carries as its `nonce` claim: the server hashes the nonce it handed
    /// out and compares, once.
    pub nonce: Option<String>,
    /// An opaque value returned unchanged in the credential, to tie the
    /// response to your request: a credential with another `state` is
    /// refused.
    pub state: Option<String>,
}

impl AppleSignIn {
    /// A request for the user's identity only.
    pub fn new() -> Self {
        Self::default()
    }
    /// Also ask for the email address.
    pub fn email(mut self) -> Self {
        self.email = true;
        self
    }
    /// Also ask for the full name.
    pub fn full_name(mut self) -> Self {
        self.full_name = true;
        self
    }
    /// Sets the nonce your server handed out (unhashed): the identity
    /// token's `nonce` claim is its SHA-256 in lower-case hex.
    pub fn nonce(mut self, nonce: impl Into<String>) -> Self {
        self.nonce = Some(nonce.into());
        self
    }
    /// What Apple gets as the request's nonce, and the identity token's
    /// `nonce` claim: the SHA-256 of [`nonce`](Self::nonce) in lower-case
    /// hex. Backends pass this, never the nonce itself.
    pub fn apple_nonce(&self) -> Option<String> {
        use sha2::{Digest, Sha256};
        self.nonce.as_deref().map(|n| {
            Sha256::digest(n.as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
    }
    /// Sets the opaque `state` value returned in the credential.
    pub fn state(mut self, state: impl Into<String>) -> Self {
        self.state = Some(state.into());
        self
    }
}

/// The result of Sign in with Apple. Verify `identity_token` (a JWT signed by
/// Apple) or exchange `authorization_code` on your server before trusting it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AppleCredential {
    /// Stable per app and Apple ID.
    pub user: String,
    /// A JWT signed by Apple: verify it on your server.
    pub identity_token: Option<String>,
    /// A one-time code your server can exchange with Apple.
    pub authorization_code: Option<String>,
    /// Only on the first sign-in (Apple sends them once).
    pub email: Option<String>,
    /// The user's given name.
    pub given_name: Option<String>,
    /// The user's family name.
    pub family_name: Option<String>,
    /// The `state` of the request, echoed back.
    pub state: Option<String>,
}

impl AppleCredential {
    /// From a backend's `Fields` (`user`, `identity_token`...); empty values are `None`.
    pub fn from_fields(f: &BTreeMap<String, String>) -> Self {
        let get = |k: &str| f.get(k).filter(|v| !v.is_empty()).cloned();
        AppleCredential {
            user: get("user").unwrap_or_default(),
            identity_token: get("identity_token"),
            authorization_code: get("authorization_code"),
            email: get("email"),
            given_name: get("given_name"),
            family_name: get("family_name"),
            state: get("state"),
        }
    }
}

/// A WebAuthn ceremony with the options of the app's own relying party
/// (#94): [`AsyncApp::webauthn_create`] / [`AsyncApp::webauthn_get`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebAuthnRequest {
    /// Registers a passkey (`navigator.credentials.create`) rather than
    /// signing in with one (`get`).
    pub create: bool,
    /// The server's options as JSON, in the WebAuthn JSON format
    /// (`PublicKeyCredentialCreationOptionsJSON` /
    /// `PublicKeyCredentialRequestOptionsJSON`: binary fields in base64url),
    /// with or without the `{"publicKey": …}` wrapper.
    pub options: String,
    /// A sign-in offered in a field's suggestions (#203): conditional
    /// mediation, answered when the user picks a passkey there.
    pub conditional: bool,
}

/// The token and code show hidden (S34).
impl std::fmt::Debug for AppleCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleCredential")
            .field("user", &self.user)
            .field(
                "identity_token",
                &Hidden::opt(self.identity_token.as_deref()),
            )
            .field(
                "authorization_code",
                &Hidden::opt(self.authorization_code.as_deref()),
            )
            .field("email", &self.email)
            .field("given_name", &self.given_name)
            .field("family_name", &self.family_name)
            .field("state", &self.state)
            .finish()
    }
}

/// How `Debug` shows a secret: whether there is one, not what (S34).
pub(crate) struct Hidden(&'static str);

impl Hidden {
    pub(crate) fn text(s: &str) -> Self {
        Hidden(if s.is_empty() { "\"\"" } else { "<redacted>" })
    }

    pub(crate) fn opt(s: Option<&str>) -> Self {
        Hidden(if s.is_some() {
            "Some(<redacted>)"
        } else {
            "None"
        })
    }
}

impl std::fmt::Debug for Hidden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// A browser sign-in session (OAuth 2 / OpenID Connect): see `prism::oauth`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebAuthRequest {
    /// The authorization URL.
    pub url: String,
    /// Where the provider redirects (from [`AsyncApp::auth_redirect`]).
    pub redirect: String,
    /// No shared cookies (Apple: `prefersEphemeralWebBrowserSession`).
    pub ephemeral: bool,
    /// The provider's name, for the web's "Continue" button when a popup is
    /// blocked.
    pub provider: String,
}

/// `cx.secure_storage()`: secrets kept by the platform's secure store.
pub struct SecureStorage<'a> {
    cx: &'a AsyncApp,
    shared: bool,
}

impl SecureStorage<'_> {
    /// Items shared with the app's extensions (see [`SecureOptions::shared`]).
    pub fn shared(mut self) -> Self {
        self.shared = true;
        self
    }

    /// The secret, `None` if there is none. Biometric items ask the owner
    /// first (`Err(Cancelled)` if they cancel).
    pub async fn get(&self, key: &str) -> Result<Option<String>, DeviceError> {
        self.get_with_prompt(key, None).await
    }

    /// Like [`get`](Self::get), with the reason shown by the biometric prompt.
    pub async fn get_with_prompt(
        &self,
        key: &str,
        prompt: Option<&str>,
    ) -> Result<Option<String>, DeviceError> {
        let call = DeviceCall::SecureGet {
            key: key.to_string(),
            prompt: prompt.map(str::to_string),
            shared: self.shared,
        };
        match self.cx.device(call).await? {
            DeviceValue::Text(t) => Ok(t),
            DeviceValue::Done => Ok(None),
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Stores a secret under `key` (replacing the previous one) with the default options.
    pub async fn set(&self, key: &str, value: &str) -> Result<(), DeviceError> {
        let options = SecureOptions {
            shared: self.shared,
            ..Default::default()
        };
        self.set_with(key, value, options).await
    }

    /// Stores a secret (replacing the previous one) with `options`.
    pub async fn set_with(
        &self,
        key: &str,
        value: &str,
        mut options: SecureOptions,
    ) -> Result<(), DeviceError> {
        options.shared |= self.shared;
        // The owner's check, by the store where it can (S22).
        options.biometric |= options.confirm_owner;
        let call = DeviceCall::SecureSet {
            key: key.to_string(),
            value: value.to_string(),
            options,
        };
        self.cx.device(call).await.map(|_| ())
    }

    /// Removes a secret (no error if there was none).
    pub async fn delete(&self, key: &str) -> Result<(), DeviceError> {
        let call = DeviceCall::SecureDelete {
            key: key.to_string(),
            shared: self.shared,
        };
        self.cx.device(call).await.map(|_| ())
    }
}

impl AsyncApp {
    /// Secrets (tokens, keys): the Keychain, the Android Keystore, the
    /// Secret Service, the server on the web. Not synced, not backed up.
    pub fn secure_storage(&self) -> SecureStorage<'_> {
        SecureStorage {
            cx: self,
            shared: false,
        }
    }

    /// The biometric sensor and whether it (or a passcode) can be used now.
    pub async fn biometry(&self) -> Biometry {
        match self.device(DeviceCall::BiometryStatus).await {
            Ok(DeviceValue::Fields(f)) => Biometry::from_fields(&f),
            _ => Biometry::default(),
        }
    }

    /// Asks the device owner to authenticate (biometrics, then the passcode).
    /// `Ok(())` once they did; `Err(Cancelled)`, `Err(Denied)` (failed) or
    /// `Err(Unavailable)` (nothing to authenticate with) otherwise.
    pub async fn authenticate(&self, reason: impl Into<String>) -> Result<(), DeviceError> {
        self.authenticate_with(AuthRequest::new(reason)).await
    }

    /// Like [`authenticate`](Self::authenticate), with a full [`AuthRequest`].
    pub async fn authenticate_with(&self, request: AuthRequest) -> Result<(), DeviceError> {
        self.device(DeviceCall::Authenticate(request))
            .await
            .map(|_| ())
    }

    /// Sign in with Apple (`capabilities().sign_in_with_apple`). Needs the
    /// `com.apple.developer.applesignin` entitlement: `[ios] sign_in_with_apple
    /// = true` in Prism.toml and a provisioning profile with the capability.
    ///
    /// The identity token carries the SHA-256 of the request's
    /// [`nonce`](AppleSignIn::nonce) (lower-case hex): verify the token on
    /// your server (Apple's signature, `iss` `https://appleid.apple.com`,
    /// `aud` your bundle id, `exp`) and compare that claim with the hash of
    /// the nonce it handed out. A credential whose `state` is not the
    /// request's is an error.
    pub async fn sign_in_with_apple(
        &self,
        request: AppleSignIn,
    ) -> Result<AppleCredential, DeviceError> {
        let state = request.state.clone();
        match self.device(DeviceCall::SignInWithApple(request)).await? {
            DeviceValue::Fields(f) => {
                let credential = AppleCredential::from_fields(&f);
                if state.is_some() && credential.state != state {
                    return Err(DeviceError::Failed(
                        "Sign in with Apple answered another request (its state differs)".into(),
                    ));
                }
                Ok(credential)
            }
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Registers a passkey with the app's own WebAuthn relying party (#94):
    /// `options` are what the server produced for this ceremony
    /// (`PublicKeyCredentialCreationOptionsJSON`; the `{"publicKey": …}`
    /// wrapper is accepted), and the answer is the new credential as JSON
    /// (`RegistrationResponseJSON`, what `PublicKeyCredential.toJSON()`
    /// gives), to send back to the server for verification. The server
    /// checks everything; Prism only runs the ceremony.
    ///
    /// Web: `navigator.credentials.create` in the page (if the browser
    /// wants a fresh tap, the page asks for one); Apple platforms:
    /// `ASAuthorizationPlatformPublicKeyCredentialProvider`, which needs the
    /// `webcredentials:` associated domain of the relying party; Android
    /// 14+: the Credential Manager; Windows: `webauthn.dll`, Windows' own
    /// dialog (the client data's origin is `https://<rpId>`, as a
    /// browser's). `Err(Unavailable)` where there are no passkeys
    /// (`capabilities().passkeys`: Linux, watchOS, older Android, a PC
    /// without `webauthn.dll`), `Err(Cancelled)` when the user
    /// declines.
    pub async fn webauthn_create(&self, options: impl Into<String>) -> Result<String, DeviceError> {
        self.webauthn(true, false, options.into()).await
    }

    /// Signs in with a passkey of the app's own relying party (#94):
    /// `options` are the server's `PublicKeyCredentialRequestOptionsJSON`,
    /// and the answer is the assertion as JSON (`AuthenticationResponseJSON`)
    /// for the server to verify. See [`webauthn_create`](Self::webauthn_create).
    pub async fn webauthn_get(&self, options: impl Into<String>) -> Result<String, DeviceError> {
        self.webauthn(false, false, options.into()).await
    }

    /// Offers the app's passkeys in the suggestions of a sign-in field
    /// (#203, « conditional mediation »): starts at once with the server's
    /// `PublicKeyCredentialRequestOptionsJSON` (no `allowCredentials`) and
    /// answers the assertion JSON when the user picks a passkey from the
    /// suggestions of a field marked `.attr("autocomplete", "username webauthn")`.
    /// Start it as the sign-in screen appears; a
    /// [`webauthn_get`](Self::webauthn_get) or another call replaces it
    /// (this one then answers `Err(Cancelled)`).
    ///
    /// Web: `navigator.credentials.get({ mediation: "conditional" })` where
    /// the browser has it; iOS, iPadOS, visionOS:
    /// `ASAuthorizationController.performAutoFillAssistedRequests` (the
    /// field gets `textContentType = .username` from its `autocomplete`).
    /// `Err(Unavailable)` elsewhere (macOS, Android, Linux, Windows, older
    /// browsers): keep a « Sign in with a passkey » button calling
    /// `webauthn_get`.
    pub async fn webauthn_autofill(
        &self,
        options: impl Into<String>,
    ) -> Result<String, DeviceError> {
        self.webauthn(false, true, options.into()).await
    }

    async fn webauthn(
        &self,
        create: bool,
        conditional: bool,
        options: String,
    ) -> Result<String, DeviceError> {
        // The bare options: backends hand them to the platform as they are.
        let options = match serde_json::from_str::<serde_json::Value>(&options) {
            Ok(serde_json::Value::Object(mut o)) => match o.remove("publicKey") {
                Some(inner @ serde_json::Value::Object(_)) => inner.to_string(),
                _ => serde_json::Value::Object(o).to_string(),
            },
            _ => {
                return Err(DeviceError::Failed(
                    "the WebAuthn options are not a JSON object".into(),
                ));
            }
        };
        match self
            .device(DeviceCall::WebAuthn(WebAuthnRequest {
                create,
                options,
                conditional,
            }))
            .await?
        {
            DeviceValue::Text(Some(json)) => Ok(json),
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// The redirect URI this platform catches for a browser sign-in
    /// (`preferred` if it can: a provider-specific scheme on Apple and
    /// Android; a loopback port on Linux and Windows). Used by
    /// `prism::oauth`.
    pub async fn auth_redirect(&self, preferred: Option<&str>) -> Result<String, DeviceError> {
        match self
            .device(DeviceCall::AuthRedirect(preferred.map(str::to_string)))
            .await?
        {
            DeviceValue::Text(Some(uri)) => Ok(uri),
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }

    /// Opens a browser sign-in session; resolves with the URL the provider
    /// redirected to (with `code` and `state`). Used by `prism::oauth`.
    pub async fn web_auth(&self, request: WebAuthRequest) -> Result<String, DeviceError> {
        match self.device(DeviceCall::WebAuth(request)).await? {
            DeviceValue::Text(Some(url)) => Ok(url),
            DeviceValue::Text(None) => Err(DeviceError::Cancelled),
            other => Err(DeviceError::Failed(format!("unexpected answer {other:?}"))),
        }
    }
}

// --- Browser sign-in redirects ----------------------------------------------------------

/// Whether `callback`, a URL the browser came back to, answers the sign-in
/// that opened the authorization URL `url` with the redirect URI
/// `redirect` ([`WebAuthRequest`]):
///
/// - the same scheme, host, port and path as `redirect` (scheme and host
///   in any case; the path exactly: `/callback/x` is not `/callback`);
/// - when `url` carries a `state`, the same `state`, once.
///
/// Backends that catch the redirect themselves (a loopback listener, a
/// deep link) finish the sign-in only with such a callback, and keep
/// waiting past others: a stray request, or another process's callback,
/// cannot end it. The app checks the `state` again (`prism::oauth`).
pub fn is_sign_in_callback(url: &str, redirect: &str, callback: &str) -> bool {
    if !same_target(redirect, callback) {
        return false;
    }
    let (Some(sent), Some(got)) = (redirect_parts(url), redirect_parts(callback)) else {
        return false;
    };
    let Some(state) = param_values(sent.params, "state").next() else {
        return true;
    };
    let mut states = param_values(got.params, "state");
    states.next().as_deref() == Some(state.as_str()) && states.next().is_none()
}

/// Serves a loopback redirect URI (`http://127.0.0.1:<port>/callback`,
/// RFC 8252 §7.3) on `listener` until the sign-in that opened `url` comes
/// back to `redirect` ([`is_sign_in_callback`]); its full URL, port
/// included. Other requests (a favicon, a callback for another attempt)
/// are answered (`404`, `400`) and the wait goes on.
/// [`DeviceError::Cancelled`] after `timeout`. The Linux and Windows
/// backends use it.
pub fn catch_loopback_redirect(
    listener: &std::net::TcpListener,
    url: &str,
    redirect: &str,
    timeout: std::time::Duration,
) -> Result<String, DeviceError> {
    use std::io::{Read, Write};
    let origin = redirect_parts(redirect)
        .and_then(|p| Some(format!("{}://{}", p.scheme, p.authority?)))
        .ok_or_else(|| DeviceError::Failed(format!("not a loopback redirect: {redirect}")))?;
    let _ = listener.set_nonblocking(true);
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let Ok((mut stream, _)) = listener.accept() else {
            std::thread::sleep(std::time::Duration::from_millis(50));
            continue;
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        // The request line (a request target is short: the rest is not read).
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buf.contains(&b'\n') && buf.len() < 16 * 1024 {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let request = String::from_utf8_lossy(&buf);
        let target = request
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("GET "))
            .and_then(|l| l.split(' ').next())
            .filter(|t| t.starts_with('/'))
            .unwrap_or("");
        let callback = format!("{origin}{target}");
        if target.is_empty() || !same_target(redirect, &callback) {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            continue;
        }
        if !is_sign_in_callback(url, redirect, &callback) {
            let page = "This link is not for the sign-in in progress.";
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                )
                .as_bytes(),
            );
            continue;
        }
        let page = "<!doctype html><meta charset=utf-8><title>Signed in</title><p style=\"font:16px system-ui;margin:40px;text-align:center\">Signed in. You can close this tab and return to the app.</p>";
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{page}",
                page.len()
            )
            .as_bytes(),
        );
        return Ok(callback);
    }
    Err(DeviceError::Cancelled)
}

/// Whether `callback` is for the redirect URI `redirect` (its scheme, host,
/// port and path), whatever its parameters.
fn same_target(redirect: &str, callback: &str) -> bool {
    match (redirect_parts(redirect), redirect_parts(callback)) {
        (Some(want), Some(got)) => {
            (&want.scheme, &want.authority, want.path) == (&got.scheme, &got.authority, got.path)
        }
        _ => false,
    }
}

/// A URL as a sign-in redirect is compared.
struct RedirectParts<'a> {
    /// Lower-case.
    scheme: String,
    /// Host and port, lower-case; `None` without `//` (`app:/oauth`).
    authority: Option<String>,
    /// `/` when empty after an authority.
    path: &'a str,
    /// The query, or the fragment when there is no query (as
    /// `prism::oauth` reads the parameters).
    params: &'a str,
}

fn redirect_parts(url: &str) -> Option<RedirectParts<'_>> {
    let (scheme, rest) = url.split_once(':')?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let (authority, rest) = match rest.strip_prefix("//") {
        Some(rest) => {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            (Some(rest[..end].to_ascii_lowercase()), &rest[end..])
        }
        None => (None, rest),
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let path = match &rest[..end] {
        "" if authority.is_some() => "/",
        path => path,
    };
    let after = &rest[end..];
    let params = match after.strip_prefix('?') {
        Some(query) => query.split('#').next().unwrap_or(query),
        None => after.strip_prefix('#').unwrap_or(""),
    };
    Some(RedirectParts {
        scheme: scheme.to_ascii_lowercase(),
        authority,
        path,
        params,
    })
}

/// The loopback redirect listener of a desktop backend (RFC 8252 §7.3),
/// for [`DeviceCall::AuthRedirect`] and [`DeviceCall::WebAuth`]. The Linux
/// and Windows backends use it.
///
/// [`redirect`](Self::redirect) gives the URI of the listener that waits,
/// or binds one: `OAuth::resolve` asks for a redirect it never uses (to
/// check its registration), and the next sign-in takes that listener
/// rather than leaving it open. [`take`](Self::take) hands it to the
/// sign-in that opens the browser. One that no sign-in takes closes after
/// the idle time. Two sign-ins that both ask before either opens the
/// browser share the URI: the second one to open it fails.
#[doc(hidden)]
pub struct LoopbackListener {
    waiting: std::sync::Arc<std::sync::Mutex<Option<Waiting>>>,
    idle: std::time::Duration,
}

struct Waiting {
    uri: String,
    listener: std::net::TcpListener,
    until: std::time::Instant,
}

impl Default for LoopbackListener {
    /// Closes an unused listener after 10 minutes: longer than a sign-in
    /// takes between asking for its redirect and opening the browser
    /// (discovery, registration).
    fn default() -> Self {
        Self::new(std::time::Duration::from_secs(600))
    }
}

impl LoopbackListener {
    /// A listener nobody takes closes `idle` after it was last asked for.
    pub fn new(idle: std::time::Duration) -> Self {
        LoopbackListener {
            waiting: Default::default(),
            idle,
        }
    }

    /// `http://127.0.0.1:<port>/callback`: the listener that waits (asked
    /// again, it waits `idle` more), or a new one.
    pub fn redirect(&self) -> std::io::Result<String> {
        let until = std::time::Instant::now() + self.idle;
        let mut waiting = lock(&self.waiting);
        if let Some(w) = waiting.as_mut() {
            w.until = until;
            return Ok(w.uri.clone());
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let uri = format!(
            "http://127.0.0.1:{}/callback",
            listener.local_addr()?.port()
        );
        *waiting = Some(Waiting {
            uri: uri.clone(),
            listener,
            until,
        });
        // Closes it once unused for `idle`, unless a sign-in took it.
        let (weak, watched) = (std::sync::Arc::downgrade(&self.waiting), uri.clone());
        std::thread::Builder::new()
            .name("prism-loopback".into())
            .spawn(move || {
                loop {
                    let pause = {
                        let Some(shared) = weak.upgrade() else {
                            return;
                        };
                        let mut waiting = lock(&shared);
                        let until = match waiting.as_ref() {
                            Some(w) if w.uri == watched => w.until,
                            _ => return,
                        };
                        let now = std::time::Instant::now();
                        if until <= now {
                            *waiting = None;
                            return;
                        }
                        until - now
                    };
                    std::thread::sleep(pause);
                }
            })?;
        Ok(uri)
    }

    /// The listener of `uri` for the sign-in that opens the browser; `None`
    /// once taken or closed, or for another URI.
    pub fn take(&self, uri: &str) -> Option<std::net::TcpListener> {
        let mut waiting = lock(&self.waiting);
        match waiting.take() {
            Some(w) if w.uri == uri => Some(w.listener),
            other => {
                *waiting = other;
                None
            }
        }
    }
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The values of parameter `name` in `params` (`a=1&b=2`), decoded.
fn param_values<'a>(params: &'a str, name: &'a str) -> impl Iterator<Item = String> + 'a {
    params.split('&').filter_map(move |kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (form_decode(k) == name).then(|| form_decode(v))
    })
}

/// A form-encoded value: `%XX` escapes, `+` for a space.
fn form_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(byte)) => {
                out.push(byte);
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

    fn loopback_port(uri: &str) -> u16 {
        uri.trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/callback")
            .parse()
            .unwrap()
    }

    /// `OAuth::resolve` then `sign_in`: both ask for a redirect, one
    /// listener serves them, and the sign-in takes it once.
    #[test]
    fn a_loopback_listener_serves_the_next_sign_in() {
        let lb = LoopbackListener::default();
        let uri = lb.redirect().unwrap();
        assert!(uri.starts_with("http://127.0.0.1:") && uri.ends_with("/callback"));
        assert_eq!(lb.redirect().unwrap(), uri);
        assert!(lb.take("http://127.0.0.1:1/callback").is_none());
        let listener = lb.take(&uri).expect("the sign-in takes it");
        assert_eq!(listener.local_addr().unwrap().port(), loopback_port(&uri));
        assert!(lb.take(&uri).is_none(), "once");
        // The next sign-in gets a listener of its own.
        assert_ne!(lb.redirect().unwrap(), uri);
    }

    /// A listener no sign-in takes closes after the idle time.
    #[test]
    fn an_unused_loopback_listener_closes() {
        let lb = LoopbackListener::new(std::time::Duration::from_millis(300));
        let uri = lb.redirect().unwrap();
        let port = loopback_port(&uri);
        assert!(
            std::net::TcpListener::bind(("127.0.0.1", port)).is_err(),
            "listening"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        // Closed: its port is free again.
        while std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
            assert!(std::time::Instant::now() < deadline, "still listening");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(lb.take(&uri).is_none());
        // Dropped with the backend, the watchdog ends too. The watchdog may
        // hold the shared state for a moment while it checks the time, so
        // the port is free once it lets go, not always at the drop itself.
        let lb = LoopbackListener::new(std::time::Duration::from_secs(600));
        let uri = lb.redirect().unwrap();
        let port = loopback_port(&uri);
        drop(lb);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "still listening after the drop"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// S39: a credential whose `state` is not the request's is refused (it
    /// answers another request); without a `state` sent, none is checked.
    #[test]
    fn s39_a_credential_must_carry_the_request_state() {
        use std::sync::{Arc, Mutex};
        type Got = Arc<Mutex<BTreeMap<&'static str, Result<AppleCredential, DeviceError>>>>;
        let got: Got = Default::default();
        let out = got.clone();
        let init: crate::AppInit = Arc::new(move |cx: &mut crate::App| {
            for (label, state) in [
                ("other", Some("st-39")),
                ("none", None),
                ("same", Some("st-39")),
            ] {
                let out = out.clone();
                let mut request = AppleSignIn::new().nonce("server-nonce-39");
                if let Some(s) = state {
                    request = request.state(s);
                }
                cx.spawn(move |cx| async move {
                    let r = cx.sign_in_with_apple(request).await;
                    out.lock().unwrap().insert(label, r);
                });
            }
        });
        let mut rt = crate::Runtime::new(&init);
        rt.poll();
        let ids: Vec<u64> = rt
            .take_requests()
            .into_iter()
            .filter_map(|r| match r {
                crate::surfaces::PlatformRequest::Device {
                    id,
                    call: DeviceCall::SignInWithApple(_),
                } => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 3);
        let answer = |state: &str| {
            let mut f = BTreeMap::new();
            f.insert("user".to_string(), "001.abc".to_string());
            f.insert("state".to_string(), state.to_string());
            Ok(DeviceValue::Fields(f))
        };
        rt.device_reply(ids[0], answer("st-other"));
        rt.device_reply(ids[1], answer(""));
        rt.device_reply(ids[2], answer("st-39"));
        let got = got.lock().unwrap();
        assert!(got["other"].is_err(), "{:?}", got["other"]);
        assert_eq!(got["none"].as_ref().unwrap().user, "001.abc");
        assert_eq!(
            got["same"].as_ref().unwrap().state.as_deref(),
            Some("st-39")
        );
        // What every backend passes to Apple.
        assert_eq!(
            AppleSignIn::new()
                .nonce("server-nonce-39")
                .apple_nonce()
                .as_deref(),
            Some("3c0f900191b65765eddb001bcd76dc504a5b1a4cc3a231e1463606a05a69e377")
        );
        assert_eq!(AppleSignIn::new().apple_nonce(), None);
    }

    /// S34: Sign in with Apple's token and code do not show when printed.
    #[test]
    fn s34_apple_credentials_print_hidden() {
        let c = AppleCredential {
            user: "001.abc".into(),
            identity_token: Some("S34-MARK-IT".into()),
            authorization_code: Some("S34-MARK-AC".into()),
            ..Default::default()
        };
        for shown in [format!("{c:?}"), format!("{c:#?}")] {
            assert!(!shown.contains("S34-MARK"), "{shown}");
        }
        assert!(format!("{c:?}").contains("001.abc"));
    }

    /// S22: Credential Manager and the Secret Service give any process of
    /// the user the stored value: a prompt before Prism returns it does not
    /// protect it. A `biometric` secret needs the store's own owner check,
    /// so these stores refuse it; `confirm_owner` asks for the app's check.
    #[test]
    fn s22_stores_without_an_owner_check_refuse_biometric_secrets() {
        let biometric = SecureOptions::new().biometric();
        assert_eq!(biometric.app_gate(true), Err(DeviceError::Unavailable));
        assert_eq!(biometric.app_gate(false), Err(DeviceError::Unavailable));
        assert_eq!(SecureOptions::new().app_gate(true), Ok(false));
        assert_eq!(SecureOptions::new().app_gate(false), Ok(false));
        // The app's own check, when the app accepts it.
        let confirm = SecureOptions::new().confirm_owner();
        assert_eq!(confirm.app_gate(true), Ok(true));
        assert_eq!(confirm.app_gate(false), Err(DeviceError::Unavailable));
        // As `set_with` sends it: both.
        assert_eq!(confirm.biometric().app_gate(true), Ok(true));
    }

    #[test]
    fn biometry_from_fields() {
        let mut f = BTreeMap::new();
        f.insert("kind".to_string(), "face_id".to_string());
        f.insert("enrolled".to_string(), "1".to_string());
        f.insert("passcode".to_string(), "true".to_string());
        let b = Biometry::from_fields(&f);
        assert_eq!(b.kind, BiometryKind::FaceId);
        assert!(b.enrolled && b.passcode && !b.locked_out && b.can_authenticate());
        assert_eq!(Biometry::from_fields(&BTreeMap::new()), Biometry::default());
        for k in [
            BiometryKind::None,
            BiometryKind::TouchId,
            BiometryKind::FaceId,
            BiometryKind::OpticId,
            BiometryKind::Fingerprint,
            BiometryKind::Face,
            BiometryKind::Iris,
            BiometryKind::Platform,
        ] {
            assert_eq!(BiometryKind::from_name(k.name()), k);
        }
    }

    #[test]
    fn apple_credential_from_fields() {
        let mut f = BTreeMap::new();
        f.insert("user".to_string(), "001.abc".to_string());
        f.insert("email".to_string(), String::new());
        f.insert("identity_token".to_string(), "eyJ".to_string());
        let c = AppleCredential::from_fields(&f);
        assert_eq!(c.user, "001.abc");
        assert_eq!(c.email, None);
        assert_eq!(c.identity_token.as_deref(), Some("eyJ"));
    }

    /// S33: only the callback of the sign-in in progress finishes it: the
    /// redirect's exact scheme, host, port and path, and its `state`, once.
    #[test]
    fn s33_a_sign_in_ends_only_with_its_own_callback() {
        let url = "https://idp.example/authorize?client_id=c&state=s%2B1&code_challenge=x";
        let loopback = "http://127.0.0.1:50123/callback";
        let ok = |callback: &str| is_sign_in_callback(url, loopback, callback);
        assert!(ok("http://127.0.0.1:50123/callback?code=abc&state=s%2B1"));
        assert!(ok("HTTP://127.0.0.1:50123/callback?state=s%2B1&code=abc"));
        // The provider's refusal is for this attempt too.
        assert!(ok(
            "http://127.0.0.1:50123/callback?error=access_denied&state=s%2B1"
        ));
        for wrong in [
            // Another attempt, or none.
            "http://127.0.0.1:50123/callback?code=abc&state=other",
            "http://127.0.0.1:50123/callback?code=abc",
            "http://127.0.0.1:50123/callback?code=abc&state=s+1",
            // Twice, even if one is right.
            "http://127.0.0.1:50123/callback?state=other&code=abc&state=s%2B1",
            "http://127.0.0.1:50123/callback?state=s%2B1&state=s%2B1&code=abc",
            // Elsewhere.
            "http://127.0.0.1/callback?code=abc&state=s%2B1",
            "http://127.0.0.1:50124/callback?code=abc&state=s%2B1",
            "http://localhost:50123/callback?code=abc&state=s%2B1",
            "https://127.0.0.1:50123/callback?code=abc&state=s%2B1",
            "http://127.0.0.1:50123/callbackx?code=abc&state=s%2B1",
            "http://127.0.0.1:50123/callback/x?code=abc&state=s%2B1",
            "http://127.0.0.1:50123/?code=abc&state=s%2B1",
            "/callback?code=abc&state=s%2B1",
            "",
        ] {
            assert!(!ok(wrong), "{wrong}");
        }
        // A custom scheme (Android), with or without an authority.
        let app = "com.example.app:/oauth";
        assert!(is_sign_in_callback(
            url,
            app,
            "com.example.app:/oauth?code=a&state=s%2B1"
        ));
        assert!(is_sign_in_callback(
            url,
            app,
            "com.example.app:/oauth#code=a&state=s%2B1"
        ));
        assert!(!is_sign_in_callback(
            url,
            app,
            "com.example.app://evil/oauth?state=s%2B1"
        ));
        assert!(!is_sign_in_callback(
            url,
            app,
            "com.example.app:/oauth/x?state=s%2B1"
        ));
        assert!(!is_sign_in_callback(
            url,
            app,
            "com.example.app:/oauth?state=x"
        ));
        let host = "com.example.app://auth/done";
        assert!(is_sign_in_callback(
            url,
            host,
            "com.example.app://AUTH/done?state=s%2B1"
        ));
        assert!(!is_sign_in_callback(
            url,
            host,
            "com.example.app://other/done?state=s%2B1"
        ));
        // An authorization URL without `state`: the redirect alone.
        let bare = "https://idp.example/authorize?client_id=c";
        assert!(is_sign_in_callback(
            bare,
            loopback,
            "http://127.0.0.1:50123/callback?code=a"
        ));
        assert!(!is_sign_in_callback(
            bare,
            loopback,
            "http://127.0.0.1:50123/x?code=a"
        ));
    }

    /// S33: the loopback listener answers stray requests and callbacks of
    /// other attempts, and keeps waiting for the one in progress, port
    /// included in the URL it returns.
    #[test]
    fn s33_the_loopback_waits_past_other_callbacks() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let redirect = format!("http://127.0.0.1:{port}/callback");
        let url = "https://idp.example/authorize?state=xyz";
        let client = std::thread::spawn(move || {
            let mut answers = Vec::new();
            for target in [
                "/favicon.ico",
                "/callbackx?code=evil&state=xyz",
                "/callback?code=evil&state=guess",
                "/callback?code=evil",
                "/callback?code=abc&state=xyz",
            ] {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                s.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                    .unwrap();
                let mut page = String::new();
                let _ = s.read_to_string(&mut page);
                answers.push(page.lines().next().unwrap_or_default().to_string());
            }
            answers
        });
        let r = catch_loopback_redirect(
            &listener,
            url,
            &redirect,
            std::time::Duration::from_secs(20),
        );
        assert_eq!(
            r,
            Ok(format!(
                "http://127.0.0.1:{port}/callback?code=abc&state=xyz"
            ))
        );
        assert_eq!(
            client.join().unwrap(),
            [
                "HTTP/1.1 404 Not Found",
                "HTTP/1.1 404 Not Found",
                "HTTP/1.1 400 Bad Request",
                "HTTP/1.1 400 Bad Request",
                "HTTP/1.1 200 OK",
            ]
        );
        // Nobody comes back: cancelled when the time is up.
        let r = catch_loopback_redirect(
            &listener,
            url,
            &redirect,
            std::time::Duration::from_millis(200),
        );
        assert_eq!(r, Err(DeviceError::Cancelled));
    }
}
