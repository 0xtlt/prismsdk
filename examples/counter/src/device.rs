//! The Components gallery's device card: location, gyroscope, photos and
//! camera, haptics, clipboard, sharing.

use std::time::Duration;

use prism::prelude::*;
use prism::ui::{self, Theme};
use prism::{Div, Text, UrlTarget};

use crate::strings;

#[derive(Default)]
pub struct DeviceDemo {
    location: Option<Result<Location, String>>,
    locating: bool,
    follow: Option<Watch>,
    motion: Option<Result<Motion, String>>,
    gyro: Option<Watch>,
    photos: Vec<PickedFile>,
    status: Option<String>,
}

impl DeviceDemo {
    fn locate(&mut self, cx: &mut Context<Self>) {
        self.locating = true;
        cx.spawn(async move |this, cx| {
            let result = match cx.request_permission(Permission::Location).await {
                s if s.is_granted() => cx.current_location().await.map_err(|e| e.to_string()),
                s => Err(format!("{s:?}")),
            };
            cx.update_entity(&this, |d, _| {
                d.locating = false;
                d.location = Some(result);
            });
        });
    }

    fn toggle_follow(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(w) = self.follow.take() {
            cx.stop_watching(w);
        }
        if on {
            let me = cx.entity();
            self.follow = Some(cx.watch_location(Accuracy::Best, move |l, app| {
                let l = l.clone().map_err(|e| e.to_string());
                me.update(app, |d, cx| {
                    d.location = Some(l);
                    cx.notify();
                });
            }));
        }
    }

    fn toggle_gyro(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(w) = self.gyro.take() {
            cx.stop_watching(w);
        }
        self.motion = None;
        if on {
            let me = cx.entity();
            self.gyro = Some(cx.watch_motion(Duration::from_millis(100), move |m, app| {
                let m = m.clone().map_err(|e| e.to_string());
                me.update(app, |d, cx| {
                    d.motion = Some(m);
                    cx.notify();
                });
            }));
        }
    }

    fn pick(&mut self, options: PickImages, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = cx.pick_images(options).await;
            cx.update_entity(&this, |d, _| match picked {
                Ok(files) => d.photos.extend(files),
                Err(e) => d.status = Some(e.to_string()),
            });
        });
    }
}

impl Render for DeviceDemo {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let caps = cx.capabilities();
        let muted = |s: String| text(s).text_size(13.).text_color(t.muted);
        let deg = |r: f64| format!("{:.0}°", r.to_degrees());

        let location = match &self.location {
            _ if self.locating => strings::device_locating(cx),
            None => strings::device_location_none(cx),
            Some(Ok(l)) => strings::device_location(
                cx,
                cx.format_number(l.latitude, 4),
                cx.format_number(l.longitude, 4),
                cx.format_number(l.accuracy, 0),
            ),
            Some(Err(e)) => strings::device_error(cx, e),
        };
        let motion = match &self.motion {
            None if self.gyro.is_some() => strings::device_waiting(cx),
            None => String::new(),
            Some(Ok(m)) => strings::device_motion(
                cx,
                deg(m.attitude[0]),
                deg(m.attitude[1]),
                deg(m.attitude[2]),
            ),
            Some(Err(e)) => strings::device_error(cx, e),
        };
        // A level: the dot follows the device's roll.
        let roll = self
            .motion
            .as_ref()
            .and_then(|m| m.as_ref().ok())
            .map_or(0., |m| m.attitude[0]);
        let level = div()
            .h(px(8.))
            .rounded(4.)
            .bg(t.border)
            .flex_row()
            .justify_center()
            .child(
                div()
                    .size(px(8.))
                    .rounded(4.)
                    .bg(t.accent)
                    .ml((roll.clamp(-1.5, 1.5) * 160.) as f32)
                    .animate(100),
            )
            .accessibility_hidden();

        ui::card()
            .child(ui::heading(strings::device_title(cx)))
            // Location
            .child(text(strings::device_location_title(cx)).semibold())
            .child(muted(location))
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .items_center()
                    .child(
                        ui::button(strings::device_locate(cx))
                            .small()
                            .disabled(!caps.location || self.locating)
                            .on_click(cx.listener(|this, _, cx| this.locate(cx))),
                    )
                    .child(
                        ui::switch(strings::device_follow(cx), self.follow.is_some()).on_change(
                            cx.listener(|this, e: &ChangeEvent, cx| {
                                this.toggle_follow(e.value != 0., cx)
                            }),
                        ),
                    ),
            )
            .child(ui::divider())
            // Motion
            .child(
                ui::switch(strings::device_gyro(cx), self.gyro.is_some()).on_change(
                    cx.listener(|this, e: &ChangeEvent, cx| this.toggle_gyro(e.value != 0., cx)),
                ),
            )
            .when(self.gyro.is_some(), |d| d.child(muted(motion)).child(level))
            .child(ui::divider())
            // Photos and camera
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .child(
                        ui::button(strings::device_pick(cx))
                            .small()
                            .secondary()
                            .disabled(!caps.photos)
                            .on_click(
                                cx.listener(|this, _, cx| {
                                    this.pick(PickImages::new().limit(3), cx)
                                }),
                            ),
                    )
                    .when(caps.camera, |d| {
                        d.child(
                            ui::button(strings::device_camera(cx))
                                .small()
                                .secondary()
                                .on_click(cx.listener(|this, _, cx| {
                                    this.pick(PickImages::new().camera(), cx)
                                })),
                        )
                    }),
            )
            .when(!self.photos.is_empty(), |d| {
                d.child(div().flex_row().flex_wrap().gap(8.).children(
                    self.photos.iter().rev().take(6).map(|p| {
                        picture(p.path.clone())
                            .alt(p.name.clone())
                            .fill()
                            .size(px(72.))
                            .rounded(10.)
                    }),
                ))
            })
            .child(ui::divider())
            // Haptics, clipboard, share
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .when(caps.haptics, |d| {
                        d.child(
                            ui::button(strings::device_haptic(cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|_, _, cx| cx.haptic(Haptic::Success))),
                        )
                    })
                    .when(caps.clipboard, |d| {
                        d.child(
                            ui::button(strings::device_copy(cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|this, _, cx| {
                                    // Text and HTML: rich editors paste the bold (#246).
                                    cx.write_clipboard(
                                        ClipboardItem::new()
                                            .text("Prism ♥ Rust")
                                            .html("<b>Prism</b> ♥ Rust"),
                                    );
                                    this.status = Some(strings::device_copied(cx));
                                })),
                        )
                        .child(
                            ui::button(strings::device_paste(cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|_, _, cx| {
                                    cx.spawn(async move |this, cx| {
                                        // Every type (#246): a screenshot is image/png.
                                        let item = cx.read_clipboard_item().await;
                                        cx.update_entity(&this, |d, cx| {
                                            d.status = Some(match item {
                                                Some(i) => match (i.as_image(), i.as_text()) {
                                                    (None, Some(text)) => text,
                                                    (Some(img), _) => format!(
                                                        "{} ({} KB)",
                                                        img.mime,
                                                        img.bytes.len().div_ceil(1024)
                                                    ),
                                                    _ => i.types().join(", "),
                                                },
                                                None => strings::device_empty(cx),
                                            })
                                        });
                                    });
                                })),
                        )
                    })
                    .when(caps.share, |d| {
                        d.child(
                            ui::button(strings::device_share(cx))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|_, _, cx| {
                                    cx.share(
                                        Share::text(
                                            "Prism: one Rust UI for the web, macOS and iOS",
                                        )
                                        .url("https://www.rust-lang.org"),
                                    )
                                })),
                        )
                    }),
            )
            .when_some(self.status.clone(), |d, s| d.child(muted(s)))
            .child(ui::divider())
            // The page: its address (web only) and leaving it.
            .child(page_section(cx, &muted))
    }
}

/// Where the page was opened (`cx.page_origin()`, `cx.request()`), and
/// `open_url_in`: this tab or a new one. Natively there is no address.
fn page_section(cx: &mut Context<DeviceDemo>, muted: &dyn Fn(String) -> Text) -> Div {
    let section = div()
        .flex_col()
        .gap(6.)
        .child(text(strings::page_title(cx)).semibold());
    let section = match (cx.page_origin(), cx.request()) {
        (Some(origin), Some(request)) => section
            .child(muted(strings::page_origin(cx, origin)))
            .child(muted(strings::page_path(cx, request.path()))),
        _ => section.child(muted(strings::page_native(cx))),
    };
    let url = cx.page_url().map(str::to_string);
    section.child(
        div()
            .flex_row()
            .flex_wrap()
            .gap(8.)
            .when_some(url, |d, url| {
                d.child(
                    ui::button(strings::page_reload(cx))
                        .small()
                        .ghost()
                        .on_click(cx.listener(move |_, _, cx| {
                            cx.open_url_in(url.clone(), UrlTarget::SameTab)
                        })),
                )
            })
            .child(
                ui::button(strings::page_new_tab(cx))
                    .small()
                    .ghost()
                    .on_click(cx.listener(|_, _, cx| {
                        cx.open_url_in("https://www.rust-lang.org", UrlTarget::NewTab)
                    })),
            ),
    )
}
