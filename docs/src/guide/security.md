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

    // A self-hosted OAuth 2 server: metadata (RFC 8414 or OpenID), and a client registered once (RFC 7591).
    let server = OAuth::server("https://git.example.com", "").dynamic_registration("My App").scope("read");
    let server = server.resolve(&cx).await?;     // endpoints + client id: keep it
    let tokens = server.sign_in(&cx).await?;
    // Later, on any thread (a blocking API client after a 401):
    // server.refresh_blocking(&refresh_token)?
    // Signing out: the server forgets the refresh token (RFC 7009).
    server.revoke(&cx, &refresh_token, Some(TokenTypeHint::RefreshToken)).await?;
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
- **Simulators** need no provisioning profile for `secure_storage`: `prism` gives simulator builds the `application-identifier` entitlement, which the keychain uses as the app's access group ([iOS](../platforms/ios.md)).
- **Face ID** needs a usage text: `face_id = "…"` under `[permissions]` in Prism.toml (`NSFaceIDUsageDescription`). Without it the call fails with a message saying so instead of crashing.
- **Sign in with Apple** needs the `com.apple.developer.applesignin` entitlement: `[ios] sign_in_with_apple = true` takes it from the provisioning profile (enable the capability for the App ID). `capabilities().sign_in_with_apple` is true only in bundles built with it. Verify `identity_token` (or exchange `authorization_code`) on your server.
- **Redirect URI** on iOS, iPadOS, visionOS, macOS, watchOS, tvOS and Android: `prism-<package>:/oauth`, where `<package>` is the cargo package name (`[app] package` in `Prism.toml`, else `Cargo.toml`'s `[package] name`) with `_` written `-`. It is neither the display name nor the bundle id. A package `my_dashboard` (bundle id `com.example.dashboard`, name "My Dashboard") registers exactly `prism-my-dashboard:/oauth` with the provider. `.redirect_uri(…)` with a custom scheme the app receives replaces it; `OAuth::google` uses Google's reversed client id on Apple.
- **OAuth**: authorization code + PKCE (S256), random `state` checked on return, `nonce` and audience checked in the ID token. Public clients only on devices (no secret); `.code_only()` hands the code and verifier to your server instead. `OAuth::oidc(issuer, client_id)` and `OAuth::server(issuer, client_id)` (plain OAuth 2, no `openid` scope) discover the endpoints.
- **Discovery** tries RFC 8414 metadata (`/.well-known/oauth-authorization-server`, inserted between the host and the issuer's path: `https://h/.well-known/oauth-authorization-server/realm`) and OpenID Connect discovery (`<issuer>/.well-known/openid-configuration`), OpenID first when the scopes include `openid`, then the OpenID document with the path inserted (RFC 8414 §5). A document whose `issuer` is not this issuer is refused (mix-up attacks). `discovery_urls(issuer, oidc_first)` lists them; `discover_blocking()` and `apply_metadata()` are public.
- **Dynamic client registration** (RFC 7591): with an empty client id and `.dynamic_registration("My App")`, the first sign-in posts to the server's `registration_endpoint` (discovered) a public client: `client_name`, this platform's redirect URI, `grant_types` `authorization_code` and `refresh_token`, `response_types` `code`, `token_endpoint_auth_method: none`, `application_type` `native` (custom scheme or loopback) or `web`. `.registration_token(t)` sends an initial access token. The answer is kept in secure storage (per issuer and client name), so the app registers once per device, and again when the redirect URI changes (a loopback redirect matches on any port, RFC 8252 §7.3), when a returned secret expires, or after the server answers `invalid_client`. If the server issues a `client_secret` anyway, it is stored with the id and sent as it asks (`client_secret_basic` or `client_secret_post`); a per-install secret is not a shipped secret. Without secure storage the app registers at each launch. On the web, registrations are per browser (the server-side store). `register_blocking`, `registration_request`, `parse_registration_response` and `Registration` are there for code that manages it itself.
- **Refreshing on any thread**: `resolve(&cx)` returns the provider with its endpoints and client id; keep it, and `refresh_blocking(refresh_token)` works from any thread (like `Request::send_blocking`), sharing the token request (`token_request`, public) with `sign_in` and `refresh`. The refresh token is kept unless the server rotates it. Google requires the reversed client id as the scheme for iOS clients (`OAuth::google` does it on Apple); for Android it only accepts App Links or Credential Manager (Play services), so use a provider that allows custom schemes, or bounce through your server.
- **Signing out** (RFC 7009): `revoke(&cx, token, hint)` (or `revoke_blocking(token, hint)` on any thread) posts the token, its `token_type_hint` (`TokenTypeHint::RefreshToken` or `AccessToken`, optional) and the client's credentials to the server's `revocation_endpoint`, read from its metadata (kept in `revocation_url`; `OAuth::google` knows Google's). Revoking the refresh token usually ends its access tokens too. A token the server does not know is not an error (RFC 7009 answers 200 either way); a server without a revocation endpoint is. `revocation_request` and `parse_revocation_response` are public, like the token request.
- **Web secrets** are a server-side store, because the app runs there: the browser never sees them. Set `PRISM_SECRET_KEY` (64 hex digits) in production; otherwise a key is generated in the data directory. WebAuthn needs a secure context (`https://` or `localhost`); the first `authenticate` registers this browser's platform credential. When a popup or WebAuthn needs a fresh tap (Safari), the page shows a "Continue" button.
- **Linux** has no desktop standard for checking a password on behalf of an app (polkit authorizes system actions, not apps): `authenticate` uses fprintd, otherwise `Unavailable`. Biometric secrets are checked by the app with fprintd before the keyring hands them out.
- Test sign-in without real accounts: `python3 scripts/mock-oidc.py` (a local OpenID provider checking PKCE, with RFC 8414 metadata, a registration endpoint and a `/picture` behind the bearer token; `--no-oidc` for a plain OAuth 2 server, `--path /realm` for an issuer with a path, `--secret` to hand out client secrets); `scripts/mock-fprintd.py` stands in for a fingerprint reader (`PRISM_FPRINTD_BUS=session`).
