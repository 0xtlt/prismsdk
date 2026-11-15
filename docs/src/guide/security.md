# Security: secrets, biometrics, sign-in

```rust
cx.spawn(async move |this, cx| {
    // Secrets: never in plain files, never synced or backed up.
    cx.secure_storage().set("refresh_token", &token).await?;
    let token = cx.secure_storage().get("refresh_token").await?;            // Option<String>
    cx.secure_storage().set_with("pin", "1234", SecureOptions::new().biometric()).await?;  // read = Face ID…
    cx.secure_storage().delete("pin").await?;

    // Is it the owner? Face ID, Touch ID, Optic ID, a fingerprint, then the passcode.
    match cx.authenticate("Unlock your notes").await {
        Ok(()) => unlock(),
        Err(DeviceError::Cancelled) => {}
        Err(e) => show(e),
    }
    let b = cx.biometry().await;            // kind (FaceId, Fingerprint…), enrolled, passcode, locked_out

    // Sign in with Apple, and OAuth 2 / OpenID Connect with PKCE (Google, Microsoft, Auth0, Keycloak…).
    let apple = cx.sign_in_with_apple(AppleSignIn::new().email().full_name()).await?;
    let tokens = prism::oauth::OAuth::google("1234-abc.apps.googleusercontent.com").sign_in(&cx).await?;
    let email = tokens.id_claims().and_then(|c| c["email"].as_str().map(String::from));
});
```

| | `secure_storage` | `authenticate` (`capabilities().biometry`) | Sign in with Apple | `oauth` sign-in browser → redirect |
|---|---|---|---|---|
| iOS, iPadOS | Keychain (this device only; `biometric`: `SecAccessControl` `.biometryCurrentSet`, else `.userPresence`) | LocalAuthentication: Face ID / Touch ID, then the passcode | AuthenticationServices | `ASWebAuthenticationSession` → `prism-<package>:/oauth` (or the provider's scheme) |
| visionOS | Keychain | Optic ID, then the passcode | AuthenticationServices | `ASWebAuthenticationSession` |
| macOS | login keychain; data protection keychain with a provisioning profile | Touch ID, then the password | needs a Mac provisioning profile | `ASWebAuthenticationSession` |
| watchOS | Keychain (`biometric` = `.userPresence`) | passcode / wrist detection (no biometric policy on the watch) | AuthenticationServices | `ASWebAuthenticationSession` |
| tvOS | Keychain | — (the SDK marks `LAContext` unavailable) | AuthenticationServices | `ASWebAuthenticationSession` (tvOS 16+) |
| Android | Keystore: AES-256-GCM key in `AndroidKeyStore`, ciphertext in `files/secure` (`biometric`: an RSA key needing the owner wraps the value's key) | `BiometricPrompt` (framework, no androidx), then the PIN (Android 11+ with keys) | — | Custom Tab (protocol extra, no androidx) or the browser → deep link |
| Linux | Secret Service over D-Bus (GNOME Keyring, KWallet) | fprintd (fingerprint readers) | — | default browser → loopback `http://127.0.0.1:<port>/callback` |
| Web | on the server, per browser (the `HttpOnly` session cookie), AES-256-GCM at rest | WebAuthn platform authenticator with user verification, checked on the server | — | popup → `<base>/_prism/auth/callback` |

- **Biometric secrets** need the owner every time they are read; new fingers or faces make them unreadable (Apple, Android: store them again). Setting one never prompts.
- **Face ID** needs a usage text: `face_id = "…"` under `[permissions]` in Prism.toml (`NSFaceIDUsageDescription`). Without it the call fails with a message saying so instead of crashing.
- **Sign in with Apple** needs the `com.apple.developer.applesignin` entitlement: `[ios] sign_in_with_apple = true` takes it from the provisioning profile (enable the capability for the App ID). `capabilities().sign_in_with_apple` is true only in bundles built with it. Verify `identity_token` (or exchange `authorization_code`) on your server.
- **Redirect URI** on iOS, iPadOS, visionOS, macOS, watchOS, tvOS and Android: `prism-<package>:/oauth`, where `<package>` is the cargo package name (`[app] package` in `Prism.toml`, else `Cargo.toml`'s `[package] name`) with `_` written `-`. It is neither the display name nor the bundle id. A package `my_dashboard` (bundle id `com.example.dashboard`, name "My Dashboard") registers exactly `prism-my-dashboard:/oauth` with the provider. `.redirect_uri(…)` with a custom scheme the app receives replaces it; `OAuth::google` uses Google's reversed client id on Apple.
- **OAuth**: authorization code + PKCE (S256), random `state` checked on return, `nonce` and audience checked in the ID token. Public clients only on devices (no secret); `.code_only()` hands the code and verifier to your server instead. `OAuth::oidc(issuer, client_id)` discovers the endpoints. Google requires the reversed client id as the scheme for iOS clients (`OAuth::google` does it on Apple); for Android it only accepts App Links or Credential Manager (Play services), so use a provider that allows custom schemes, or bounce through your server.
- **Web secrets** are a server-side store, because the app runs there: the browser never sees them. Set `PRISM_SECRET_KEY` (64 hex digits) in production; otherwise a key is generated in the data directory. WebAuthn needs a secure context (`https://` or `localhost`); the first `authenticate` registers this browser's platform credential. When a popup or WebAuthn needs a fresh tap (Safari), the page shows a "Continue" button.
- **Linux** has no desktop standard for checking a password on behalf of an app (polkit authorizes system actions, not apps): `authenticate` uses fprintd, otherwise `Unavailable`. Biometric secrets are checked by the app with fprintd before the keyring hands them out.
- Test sign-in without real accounts: `python3 scripts/mock-oidc.py` (a local OpenID provider checking PKCE); `scripts/mock-fprintd.py` stands in for a fingerprint reader (`PRISM_FPRINTD_BUS=session`).
