//! The Forms card of the Components tab: sliders, a select, date and time
//! pickers, text input variants, a text area, a spinner and toasts. It is
//! its own entity, so dragging a slider only re-renders this card.

use std::time::{Duration, SystemTime};

use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

pub struct Forms {
    volume: f64,
    brightness: f64,
    size: Option<usize>,
    due: SystemTime,
    reminder: SystemTime,
    meeting: SystemTime,
    password: String,
    phone: String,
    website: String,
    notes: String,
}

impl Default for Forms {
    fn default() -> Self {
        let now = SystemTime::now();
        Forms {
            volume: 40.,
            brightness: 0.7,
            size: None,
            due: now + Duration::from_secs(3 * 86_400),
            reminder: now,
            meeting: now + Duration::from_secs(86_400),
            password: String::new(),
            phone: String::new(),
            website: String::new(),
            notes: String::new(),
        }
    }
}

impl Render for Forms {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let sizes: Vec<String> = strings::forms_sizes(cx)
            .split(',')
            .map(str::to_string)
            .collect();
        let now = SystemTime::now();
        ui::card()
            .child(ui::heading(strings::forms_heading(cx)))
            .child(
                ui::slider(strings::forms_volume(cx), self.volume)
                    .range(0., 100.)
                    .step(1.)
                    .format(|v| format!("{v:.0} %"))
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.volume = e.value)),
            )
            .child(
                div()
                    .flex_row()
                    .items_center()
                    .gap(12.)
                    .child(image("sun.max").text_color(t.muted))
                    .child(
                        // The bare platform slider (no label row, system tint).
                        ui::slider(strings::forms_brightness(cx), self.brightness)
                            .native()
                            .on_change(
                                cx.listener(|this, e: &ChangeEvent, _| this.brightness = e.value),
                            )
                            .style(|s| s.flex_grow = 1.),
                    ),
            )
            .child(
                ui::select(strings::forms_size(cx), sizes, self.size)
                    .placeholder(strings::forms_choose(cx))
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.size = Some(e.index()))),
            )
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(16.)
                    .child(
                        ui::date_field(strings::forms_due(cx), self.due)
                            .min(now)
                            .on_change(cx.listener(|this, e: &ChangeEvent, _| this.due = e.date())),
                    )
                    .child(
                        ui::date_field(strings::forms_reminder(cx), self.reminder)
                            .time()
                            .on_change(
                                cx.listener(|this, e: &ChangeEvent, _| this.reminder = e.date()),
                            ),
                    ),
            )
            .child(
                ui::date_field(strings::forms_meeting(cx), self.meeting)
                    .date_time()
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.meeting = e.date())),
            )
            .child(
                ui::text_field(strings::forms_password(cx), self.password.clone())
                    .placeholder(strings::forms_password_placeholder(cx))
                    .secure()
                    .return_key(ReturnKey::Go)
                    .on_input(
                        cx.listener(|this, e: &InputEvent, _| this.password = e.value.clone()),
                    )
                    .on_submit(cx.listener(|this, _, cx| {
                        let n = this.password.chars().count() as i64;
                        let toast = ui::toast(strings::forms_signed_in(cx, n));
                        ui::show_toast(cx, toast);
                    })),
            )
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(12.)
                    .child(
                        ui::text_field(strings::forms_phone(cx), self.phone.clone())
                            .placeholder("+33 6 12 34 56 78")
                            .keyboard(Keyboard::Phone)
                            .return_key(ReturnKey::Done)
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.phone = e.value.clone()),
                            )
                            .style(|s| s.flex_grow = 1.),
                    )
                    .child(
                        ui::text_field(strings::forms_website(cx), self.website.clone())
                            .placeholder("example.com")
                            .keyboard(Keyboard::Url)
                            .capitalize(Capitalize::None)
                            .autocorrect(false)
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| {
                                    this.website = e.value.clone()
                                }),
                            )
                            .style(|s| s.flex_grow = 1.),
                    ),
            )
            .child(
                // A bar above the on-screen keyboard while the notes are
                // edited (#242): iOS, Android, phones' browsers.
                div()
                    .child(
                        ui::text_area(strings::forms_notes(cx), self.notes.clone())
                            .placeholder(strings::forms_notes_placeholder(cx))
                            .helper(strings::forms_notes_helper(
                                cx,
                                self.notes.chars().count() as i64,
                            ))
                            .max_lines(5)
                            .on_input(
                                cx.listener(|this, e: &InputEvent, _| this.notes = e.value.clone()),
                            ),
                    )
                    .keyboard_toolbar(
                        div()
                            .flex_row()
                            .justify_end()
                            .gap(8.)
                            .px(12.)
                            .py(6.)
                            .bg(t.surface_alt)
                            .child(
                                ui::button(strings::forms_keyboard_clear(cx))
                                    .on_click(cx.listener(|this, _, _| this.notes.clear())),
                            )
                            .child(
                                ui::button(strings::forms_keyboard_done(cx))
                                    .on_click(cx.listener(|_, _, cx| cx.dismiss_keyboard())),
                            ),
                    ),
            )
            .child(ui::spinner().label(strings::forms_syncing(cx)))
            .child(
                div()
                    .flex_row()
                    .flex_wrap()
                    .gap(8.)
                    .child(
                        ui::button(strings::forms_show_toast(cx)).on_click(cx.listener(
                            |_, _, cx| {
                                let undo = cx.listener(|_, _, cx| {
                                    let toast = ui::toast(strings::forms_toast_undone(cx));
                                    ui::show_toast(cx, toast)
                                });
                                let toast = ui::toast(strings::forms_toast_saved(cx))
                                    .action(strings::forms_undo(cx), undo);
                                ui::show_toast(cx, toast);
                            },
                        )),
                    )
                    .child(
                        ui::button(strings::forms_error(cx))
                            .secondary()
                            .on_click(cx.listener(|_, _, cx| {
                                let toast = ui::toast(strings::forms_toast_error(cx))
                                    .tone(ui::Variant::Danger);
                                ui::show_toast(cx, toast)
                            })),
                    ),
            )
    }
}
