//! Secrets, the device owner and sign-in.
//!
//! ```ignore
//! cx.spawn(async move |this, cx| {
//!     // Secrets: the Keychain, Android Keystore, Secret Service, the server.
//!     cx.secure_storage().set("token", &token).await?;
//!     let token = cx.secure_storage().get("token").await?;
//!     // Only after Face ID / Touch ID / a fingerprint (or the passcode).
//!     cx.secure_storage().set_with("pin", "1234", SecureOptions::new().biometric()).await?;
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
    /// biometrics are missing (Keychain access control, an Android Keystore
    /// key requiring authentication, WebAuthn on the web). Changing the
    /// enrolled faces or fingers makes it unreadable on Apple platforms and
    /// Android: store it again.
    pub biometric: bool,
    /// Shared with the app's extensions (widgets): the App Group's keychain
    /// access group on Apple platforms. Elsewhere the app's own store.
    pub shared: bool,
}

impl SecureOptions {
    /// Default options: not biometric, not shared.
    pub fn new() -> Self {
        Self::default()
    }
    /// Require the device owner to read the secret.
    pub fn biometric(mut self) -> Self {
        self.biometric = true;
        self
    }
    /// Share the secret with the app's extensions.
    pub fn shared(mut self) -> Self {
        self.shared = true;
        self
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
    /// Biometrics only (no passcode fallback).
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
    /// Your server's nonce, hashed (SHA-256, hex) into the identity token.
    pub nonce: Option<String>,
    /// An opaque value returned unchanged in the credential, to tie the response to your request.
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
    /// Sets the nonce your server expects, hashed into the identity token.
    pub fn nonce(mut self, nonce: impl Into<String>) -> Self {
        self.nonce = Some(nonce.into());
        self
    }
    /// Sets the opaque `state` value returned in the credential.
    pub fn state(mut self, state: impl Into<String>) -> Self {
        self.state = Some(state.into());
        self
    }
}

/// The result of Sign in with Apple. Verify `identity_token` (a JWT signed by
/// Apple) or exchange `authorization_code` on your server before trusting it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    pub async fn sign_in_with_apple(
        &self,
        request: AppleSignIn,
    ) -> Result<AppleCredential, DeviceError> {
        match self.device(DeviceCall::SignInWithApple(request)).await? {
            DeviceValue::Fields(f) => Ok(AppleCredential::from_fields(&f)),
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
    /// 14+: the Credential Manager. `Err(Unavailable)` where there are no
    /// passkeys (`capabilities().passkeys`), `Err(Cancelled)` when the user
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
    /// `Err(Unavailable)` elsewhere (macOS, Android, older browsers): keep a
    /// « Sign in with a passkey » button calling `webauthn_get`.
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
    /// Android). Used by `prism::oauth`.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
