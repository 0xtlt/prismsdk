//! The Settings tab's security card: biometrics, a secret note in the
//! platform's secure store (optionally behind Face ID / a fingerprint),
//! Sign in with Apple and an OpenID Connect sign-in.
//!
//! The OpenID provider is a local test one (`scripts/mock-oidc.py`):
//! `PRISM_OAUTH_ISSUER`, by default `http://127.0.0.1:8765` (on the Android
//! emulator, after `adb reverse tcp:8765 tcp:8765`: plain HTTP only goes to
//! this machine). Real apps use
//! `OAuth::google(client_id)` or `OAuth::oidc(issuer, client_id)`.
//!
//! The demo has no client id: it registers one (RFC 7591) the first time,
//! kept in secure storage. Signed in, it shows the user's picture, which
//! needs the access token (`picture(url).header(…)`), and refreshes the
//! tokens on a background thread (`refresh_blocking`).
//! `PRISM_OAUTH_TOKEN` starts with that access token (for screenshots:
//! `mock-oidc.py --token <it>` accepts it).
//!
//! Tests: with `PRISM_AUTH` set, « Verify » asks even where the app's
//! capabilities say the device cannot (the call's own « unavailable »);
//! `PRISM_AUTH=biometrics` asks for biometrics only
//! (`AuthRequest::biometrics_only`), which fails with « unavailable » where
//! the system check could accept a PIN (the web, Windows Hello, watchOS).
//! `PRISM_APPLE_NONCE` and `PRISM_APPLE_STATE` show « Sign in with Apple »
//! even without the entitlement, and pass that nonce and state.
//! « Create a passkey » registers one for a demo relying party (the page's
//! host on the web, else `example.com`) and keeps nothing;
//! `PRISM_PASSKEY_TIMEOUT` is the ceremony's timeout in milliseconds
//! (120000).

use prism::oauth::OAuth;
use prism::prelude::*;
use prism::ui::{self, Theme};
use prism::{Biometry, DeviceError};

use crate::strings;

const NOTE: &str = "secret-note";

#[derive(Default)]
pub struct SecurityDemo {
    biometry: Option<Biometry>,
    note: String,
    gated: bool,
    status: Option<String>,
    user: Option<String>,
    /// The provider as `resolve` returned it (endpoints, registered client).
    provider: Option<OAuth>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

fn issuer() -> String {
    std::env::var("PRISM_OAUTH_ISSUER").unwrap_or_else(|_| "http://127.0.0.1:8765".into())
}

impl SecurityDemo {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Enrollment changes while the app runs: ask now, not at launch.
        cx.spawn(async move |this, cx| {
            let b = cx.biometry().await;
            cx.update_entity(&this, |d, _| d.biometry = Some(b));
        });
        SecurityDemo {
            access_token: std::env::var("PRISM_OAUTH_TOKEN").ok(),
            ..Default::default()
        }
    }

    fn provider() -> OAuth {
        OAuth::oidc(issuer(), "")
            .dynamic_registration("Prism Counter")
            .name("Prism ID")
            .scope("email")
            .scope("profile")
    }

    fn error(cx: &App, e: &DeviceError) -> String {
        match e {
            DeviceError::Cancelled => strings::security_cancelled(cx),
            DeviceError::Unavailable => strings::security_unavailable(cx),
            DeviceError::Denied => strings::security_denied(cx),
            DeviceError::Failed(m) | DeviceError::Platform { message: m, .. } => {
                strings::device_error(cx, m)
            }
        }
    }

    fn verify(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let reason = cx.update(|app| strings::security_reason(app));
            let mut request = AuthRequest::new(reason);
            if std::env::var("PRISM_AUTH").is_ok_and(|v| v == "biometrics") {
                request = request.biometrics_only();
            }
            let r = cx.authenticate_with(request).await;
            cx.update_entity(&this, |d, cx| {
                d.status = Some(match r {
                    Ok(()) => strings::security_verified(cx),
                    Err(e) => Self::error(cx, &e),
                })
            });
        });
    }

    /// Registers a passkey for a demo relying party: the options a server
    /// would send (`PublicKeyCredentialCreationOptionsJSON`). There is no
    /// server here, so the new credential is only measured, never kept.
    fn passkey(&mut self, cx: &mut Context<Self>) {
        let rp = cx
            .page_origin()
            .and_then(|o| {
                o.split("://")
                    .nth(1)
                    .map(|h| h.split(':').next().unwrap_or(h))
            })
            .unwrap_or("example.com")
            .to_string();
        // A host name: nothing to escape in the JSON.
        let options = format!(
            r#"{{"rp":{{"id":"{rp}","name":"Prism Counter"}},"user":{{"id":"{user}","name":"demo@example.com","displayName":"Prism Counter demo"}},"challenge":"{challenge}","pubKeyCredParams":[{{"type":"public-key","alg":-7}},{{"type":"public-key","alg":-257}}],"authenticatorSelection":{{"residentKey":"required","userVerification":"preferred"}},"timeout":{timeout}}}"#,
            user = base64url(b"prism-counter-demo-user"),
            challenge = base64url(&challenge()),
            timeout = std::env::var("PRISM_PASSKEY_TIMEOUT")
                .ok()
                .and_then(|t| t.parse::<u32>().ok())
                .unwrap_or(120_000),
        );
        cx.spawn(async move |this, cx| {
            let r = cx.webauthn_create(options).await;
            cx.update_entity(&this, |d, cx| {
                d.status = Some(match r {
                    Ok(json) => strings::security_passkey_created(cx, json.len().to_string()),
                    Err(e) => Self::error(cx, &e),
                })
            });
        });
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (note, gated) = (self.note.clone(), self.gated);
        cx.spawn(async move |this, cx| {
            let options = if gated {
                SecureOptions::new().biometric()
            } else {
                SecureOptions::new()
            };
            let r = cx.secure_storage().set_with(NOTE, &note, options).await;
            cx.update_entity(&this, |d, cx| match r {
                Ok(()) => {
                    d.note.clear();
                    d.status = Some(strings::security_saved(cx));
                }
                Err(e) => d.status = Some(Self::error(cx, &e)),
            });
        });
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let prompt = cx.update(|app| strings::security_reason(app));
            let r = cx
                .secure_storage()
                .get_with_prompt(NOTE, Some(&prompt))
                .await;
            cx.update_entity(&this, |d, cx| {
                d.status = Some(match r {
                    Ok(Some(v)) => strings::security_read(cx, &v),
                    Ok(None) => strings::security_nothing(cx),
                    Err(e) => Self::error(cx, &e),
                })
            });
        });
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let r = cx.secure_storage().delete(NOTE).await;
            cx.update_entity(&this, |d, cx| {
                d.status = Some(match r {
                    Ok(()) => strings::security_deleted(cx),
                    Err(e) => Self::error(cx, &e),
                })
            });
        });
    }

    fn apple(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut request = AppleSignIn::new().email().full_name();
            if let Ok(nonce) = std::env::var("PRISM_APPLE_NONCE") {
                request = request.nonce(nonce);
            }
            if let Ok(state) = std::env::var("PRISM_APPLE_STATE") {
                request = request.state(state);
            }
            let r = cx.sign_in_with_apple(request).await;
            cx.update_entity(&this, |d, cx| match r {
                Ok(c) => d.user = Some(c.email.unwrap_or(c.user)),
                Err(e) => d.status = Some(Self::error(cx, &e)),
            });
        });
    }

    fn oauth(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // Discovery and (once) registration, then the sign-in.
            let r = match Self::provider().resolve(&cx).await {
                Ok(provider) => provider.sign_in(&cx).await.map(|t| (provider, t)),
                Err(e) => Err(e),
            };
            cx.update_entity(&this, |d, cx| match r {
                Ok((provider, tokens)) => {
                    let claims = tokens.id_claims().unwrap_or_default();
                    let who = claims["email"]
                        .as_str()
                        .or(claims["sub"].as_str())
                        .unwrap_or("?")
                        .to_string();
                    d.user = Some(who);
                    d.status = Some(strings::security_tokens(
                        cx,
                        tokens
                            .access_token
                            .as_ref()
                            .map_or(0, |t| t.len())
                            .to_string(),
                    ));
                    d.provider = Some(provider);
                    d.access_token = tokens.access_token;
                    d.refresh_token = tokens.refresh_token;
                }
                Err(prism::oauth::OAuthError::Cancelled) => {
                    d.status = Some(strings::security_cancelled(cx))
                }
                Err(e) => d.status = Some(strings::device_error(cx, e)),
            });
        });
    }
}

impl SecurityDemo {
    /// New tokens, from a background thread (as a blocking API client would
    /// after a 401).
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(provider), Some(refresh)) = (self.provider.clone(), self.refresh_token.clone())
        else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let r = cx
                .background(move || provider.refresh_blocking(&refresh))
                .await;
            cx.update_entity(&this, |d, cx| match r {
                Ok(t) => {
                    d.status = Some(strings::security_refreshed(
                        cx,
                        t.access_token.as_ref().map_or(0, |a| a.len()).to_string(),
                    ));
                    d.access_token = t.access_token;
                    d.refresh_token = t.refresh_token;
                }
                Err(e) => d.status = Some(strings::device_error(cx, e)),
            });
        });
    }

    fn sign_out(&mut self) {
        self.user = None;
        self.access_token = None;
        self.refresh_token = None;
    }
}

impl Render for SecurityDemo {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let caps = cx.capabilities();
        let muted = |s: String| text(s).text_size(13.).text_color(t.muted);
        let kind = self.biometry.as_ref().map_or(caps.biometry, |b| b.kind);
        let sensor = match kind {
            BiometryKind::FaceId => "Face ID".to_string(),
            BiometryKind::TouchId => "Touch ID".to_string(),
            BiometryKind::OpticId => "Optic ID".to_string(),
            BiometryKind::Fingerprint => strings::security_fingerprint(cx),
            BiometryKind::Face => strings::security_face(cx),
            BiometryKind::Iris => strings::security_iris(cx),
            BiometryKind::Platform => strings::security_platform(cx),
            BiometryKind::None => strings::security_none(cx),
        };
        let state = match &self.biometry {
            Some(b) if b.enrolled => strings::security_enrolled(cx),
            Some(b) if b.passcode => strings::security_passcode(cx),
            Some(_) if kind.is_biometric() => strings::security_not_enrolled(cx),
            _ => String::new(),
        };
        let lock = match kind {
            BiometryKind::None => strings::security_require_passcode(cx),
            _ => strings::security_require(cx, &sensor),
        };

        ui::card()
            .child(ui::heading(strings::security_title(cx)))
            .child(muted(if state.is_empty() {
                sensor.clone()
            } else {
                format!("{sensor} · {state}")
            }))
            .child(
                ui::button(strings::security_verify(cx))
                    .small()
                    .disabled(!caps.device_auth && std::env::var_os("PRISM_AUTH").is_none())
                    .on_click(cx.listener(|this, _, cx| this.verify(cx))),
            )
            .when(caps.passkeys, |d| {
                d.child(
                    ui::button(strings::security_passkey(cx))
                        .small()
                        .secondary()
                        .on_click(cx.listener(|this, _, cx| this.passkey(cx))),
                )
            })
            .child(ui::divider())
            .child(
                ui::text_field(strings::security_note(cx), self.note.clone())
                    .placeholder(strings::security_note_placeholder(cx))
                    .helper(strings::security_note_helper(cx))
                    .on_input(cx.listener(|this, e: &InputEvent, _| this.note = e.value.clone())),
            )
            .when(caps.device_auth, |d| {
                d.child(
                    ui::switch(lock, self.gated).on_change(
                        cx.listener(|this, e: &ChangeEvent, _| this.gated = e.value != 0.),
                    ),
                )
            })
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .child(
                        ui::button(strings::security_save(cx))
                            .small()
                            .disabled(!caps.secure_storage || self.note.is_empty())
                            .on_click(cx.listener(|this, _, cx| this.save(cx))),
                    )
                    .child(
                        ui::button(strings::security_load(cx))
                            .small()
                            .secondary()
                            .disabled(!caps.secure_storage)
                            .on_click(cx.listener(|this, _, cx| this.read(cx))),
                    )
                    .child(
                        ui::button(strings::security_delete(cx))
                            .small()
                            .ghost()
                            .disabled(!caps.secure_storage)
                            .on_click(cx.listener(|this, _, cx| this.delete(cx))),
                    ),
            )
            .child(ui::divider())
            .child(text(strings::security_sign_in(cx)).semibold())
            .child(
                div()
                    .flex_row()
                    .items_center()
                    .gap(12.)
                    // The user's picture: it only loads with the access token.
                    .when_some(self.access_token.clone(), |d, token| {
                        d.child(
                            picture(format!("{}/picture", issuer()))
                                .header("Authorization", format!("Bearer {token}"))
                                .alt(strings::security_avatar(cx))
                                .size(px(40.))
                                .rounded(20.),
                        )
                    })
                    .child(muted(match &self.user {
                        Some(u) => strings::security_signed_in(cx, u),
                        None if self.access_token.is_some() => {
                            strings::security_signed_in(cx, "Test User")
                        }
                        None => strings::security_signed_out(cx),
                    })),
            )
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .when(
                        caps.sign_in_with_apple || std::env::var_os("PRISM_APPLE_NONCE").is_some(),
                        |d| {
                            d.child(
                                ui::button(strings::security_apple(cx))
                                    .small()
                                    .on_click(cx.listener(|this, _, cx| this.apple(cx))),
                            )
                        },
                    )
                    .when(caps.web_auth, |d| {
                        d.child(
                            ui::button(strings::security_oauth(cx))
                                .small()
                                .secondary()
                                .on_click(cx.listener(|this, _, cx| this.oauth(cx))),
                        )
                    })
                    .when(self.refresh_token.is_some(), |d| {
                        d.child(
                            ui::button(strings::security_refresh(cx))
                                .small()
                                .secondary()
                                .on_click(cx.listener(|this, _, cx| this.refresh(cx))),
                        )
                    })
                    .when(self.user.is_some() || self.access_token.is_some(), |d| {
                        d.child(
                            ui::button(strings::security_sign_out(cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _| this.sign_out())),
                        )
                    }),
            )
            .when_some(self.status.clone(), |d, s| d.child(muted(s)))
    }
}

/// A demo challenge: a real one comes from the server, random and used once.
fn challenge() -> Vec<u8> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut bytes = b"prism-counter-demo-".to_vec();
    bytes.extend_from_slice(&nanos.to_le_bytes());
    bytes
}

/// Base64url without padding, as WebAuthn's JSON wants binary fields.
fn base64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, &b| n << 8 | u32::from(b)) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}
