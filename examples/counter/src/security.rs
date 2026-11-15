//! The Settings tab's security card: biometrics, a secret note in the
//! platform's secure store (optionally behind Face ID / a fingerprint),
//! Sign in with Apple and an OpenID Connect sign-in.
//!
//! The OpenID provider is a local test one (`scripts/mock-oidc.py`):
//! `PRISM_OAUTH_ISSUER`, by default `http://127.0.0.1:8765` (the Android
//! emulator reaches the computer at 10.0.2.2). Real apps use
//! `OAuth::google(client_id)` or `OAuth::oidc(issuer, client_id)`.

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
}

fn issuer() -> String {
    std::env::var("PRISM_OAUTH_ISSUER").unwrap_or_else(|_| {
        if cfg!(target_os = "android") {
            "http://10.0.2.2:8765".into()
        } else {
            "http://127.0.0.1:8765".into()
        }
    })
}

impl SecurityDemo {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Enrollment changes while the app runs: ask now, not at launch.
        cx.spawn(async move |this, cx| {
            let b = cx.biometry().await;
            cx.update_entity(&this, |d, _| d.biometry = Some(b));
        });
        SecurityDemo::default()
    }

    fn error(cx: &App, e: &DeviceError) -> String {
        match e {
            DeviceError::Cancelled => strings::security_cancelled(cx),
            DeviceError::Unavailable => strings::security_unavailable(cx),
            DeviceError::Denied => strings::security_denied(cx),
            DeviceError::Failed(m) => strings::device_error(cx, m),
        }
    }

    fn verify(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let reason = cx.update(|app| strings::security_reason(app));
            let r = cx.authenticate(reason).await;
            cx.update_entity(&this, |d, cx| {
                d.status = Some(match r {
                    Ok(()) => strings::security_verified(cx),
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
            let r = cx
                .sign_in_with_apple(AppleSignIn::new().email().full_name())
                .await;
            cx.update_entity(&this, |d, cx| match r {
                Ok(c) => d.user = Some(c.email.unwrap_or(c.user)),
                Err(e) => d.status = Some(Self::error(cx, &e)),
            });
        });
    }

    fn oauth(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let provider = OAuth::oidc(issuer(), "prism-demo")
                .name("Prism ID")
                .scope("email")
                .scope("profile");
            let r = provider.sign_in(&cx).await;
            cx.update_entity(&this, |d, cx| match r {
                Ok(tokens) => {
                    let claims = tokens.id_claims().unwrap_or_default();
                    let who = claims["email"]
                        .as_str()
                        .or(claims["sub"].as_str())
                        .unwrap_or("?")
                        .to_string();
                    d.user = Some(who);
                    d.status = Some(strings::security_tokens(
                        cx,
                        tokens.access_token.map_or(0, |t| t.len()).to_string(),
                    ));
                }
                Err(prism::oauth::OAuthError::Cancelled) => {
                    d.status = Some(strings::security_cancelled(cx))
                }
                Err(e) => d.status = Some(strings::device_error(cx, e)),
            });
        });
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
                    .disabled(!caps.device_auth)
                    .on_click(cx.listener(|this, _, cx| this.verify(cx))),
            )
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
            .child(muted(match &self.user {
                Some(u) => strings::security_signed_in(cx, u),
                None => strings::security_signed_out(cx),
            }))
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .when(caps.sign_in_with_apple, |d| {
                        d.child(
                            ui::button(strings::security_apple(cx))
                                .small()
                                .on_click(cx.listener(|this, _, cx| this.apple(cx))),
                        )
                    })
                    .when(caps.web_auth, |d| {
                        d.child(
                            ui::button(strings::security_oauth(cx))
                                .small()
                                .secondary()
                                .on_click(cx.listener(|this, _, cx| this.oauth(cx))),
                        )
                    })
                    .when(self.user.is_some(), |d| {
                        d.child(
                            ui::button(strings::security_sign_out(cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, _| this.user = None)),
                        )
                    }),
            )
            .when_some(self.status.clone(), |d, s| d.child(muted(s)))
    }
}
