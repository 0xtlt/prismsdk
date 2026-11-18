//! The Forms card of the Components tab: sliders, a select, date and time
//! pickers, text input variants, a text area, a rich note with a toolbar
//! over its selection (#217), blocks selected as one text (#231), a spinner
//! and toasts. It is
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
    /// A paragraph edited with others: their carets are drawn in it (#240).
    shared: Option<String>,
    /// The rich note (#217): its runs, selection, the toolbar's anchor
    /// (the selection's rectangle), what its hooks last did.
    note: Vec<Span>,
    note_sel: SelectionEvent,
    toolbar: Option<Rect>,
    note_log: String,
    /// Paragraphs selected as one text (#231), and how many the selection
    /// spans.
    blocks: Vec<String>,
    blocks_selected: usize,
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
            shared: None,
            note: vec![
                span("Select "),
                span("these words").bold(),
                span(", or ⌘-click "),
                link("a link", "https://github.com/0xtlt/prism"),
                span("."),
            ],
            note_sel: SelectionEvent::default(),
            toolbar: None,
            note_log: String::new(),
            blocks: vec![
                "Select from the middle of this paragraph,".into(),
                "through this one,".into(),
                "into the last one. Then type, or copy.".into(),
            ],
            blocks_selected: 0,
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
            .child(self.shared_draft(cx))
            .child(self.note(cx))
            .child(self.blocks(cx))
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

/// The selection's runs as Markdown: bold and links (what the note copies).
fn markdown(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| match (s.href(), s.is_bold()) {
            (Some(href), _) => format!("[{}]({href})", s.text()),
            (None, true) => format!("**{}**", s.text()),
            (None, false) => s.text().to_string(),
        })
        .collect()
}

impl Forms {
    /// A rich note (#217): a Bold button above the selected words, copies
    /// with Markdown, pastes and ⌘-clicks reported.
    fn note(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let field = rich_input(self.note.clone())
            .placeholder(strings::forms_note_placeholder(cx))
            .accessibility_label(strings::forms_note(cx))
            .on_input(cx.listener(|this, e: &RichInputEvent, _| this.note = e.spans.clone()))
            .on_select(cx.listener(|this, e: &SelectionEvent, _| this.note_sel = *e))
            .selection(self.note_sel)
            .on_selection_rect(cx.listener(|this, e: &SelectionRectEvent, _| {
                this.toolbar = (!e.selection.is_caret()).then_some(e.rect)
            }))
            .on_copy(cx.listener(|this, e: &CopyEvent, cx| {
                e.write(ClipboardContent::text(&e.text).markdown(markdown(&e.spans)));
                this.note_log = strings::forms_note_copied(cx, e.text.chars().count() as i64);
            }))
            .on_paste(cx.listener(|this, e: &PasteEvent, cx| {
                this.note_log = strings::forms_note_pasted(cx, e.types.len() as i64);
            }))
            .on_span_click(cx.listener(|this, e: &SpanClickEvent, cx| {
                this.note_log = strings::forms_note_clicked(cx, e.span as i64);
            }));
        let bold = cx.listener(|this, _, _| {
            this.note = prism::restyle_spans(&this.note, this.note_sel.range(), |s| {
                if s.is_bold() {
                    s.font_weight(FontWeight::Regular)
                } else {
                    s.bold()
                }
            });
        });
        div()
            .flex_col()
            .gap(4.)
            .child(
                // The toolbar's anchor sits at the selection's rectangle,
                // in a wrapper laid out at the field's origin.
                div().child(field).children(self.toolbar.map(|r| {
                    div()
                        .absolute()
                        .left(r.x)
                        .top(r.y)
                        .w(r.width.max(1.))
                        .h(r.height)
                        .child(
                            popover(ui::button(strings::forms_note_bold(cx)).on_click(bold))
                                .open(true)
                                .side(PopoverSide::Top)
                                .size(96., 0.)
                                .take_focus(false)
                                .on_dismiss(cx.listener(|this, _, _| this.toolbar = None)),
                        )
                })),
            )
            .child(text(self.note_log.clone()).text_size(12.))
    }

    /// A paragraph other people edit too (#240): Camille's caret with her
    /// name, Noor's selection (idle, her name faded). Their offsets would
    /// come from the collaboration server; here they follow the text.
    fn shared_draft(&mut self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::of(cx);
        let draft = self
            .shared
            .get_or_insert_with(|| strings::forms_shared_text(cx))
            .clone();
        let len = draft.chars().count();
        let carets = vec![
            RemoteCaret::caret(len.min(7), rgb(0xE8590C)).label("Camille"),
            RemoteCaret::new(len.min(34), len.min(48), rgb(0x1C7ED6))
                .label("Noor")
                .idle(true),
        ];
        div()
            .flex_col()
            .gap(4.)
            .child(
                text(strings::forms_shared(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            )
            .child(
                text_area()
                    .value(draft)
                    .rows(2)
                    .remote_carets(carets)
                    .on_input(
                        cx.listener(|this, e: &InputEvent, _| this.shared = Some(e.value.clone())),
                    ),
            )
    }

    /// Paragraphs that share one selection (#231): typing over a range
    /// that spans several merges them, Enter splits there.
    fn blocks(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let flow = text_flow()
            .gap(4.)
            .children(self.blocks.iter().enumerate().map(|(i, b)| {
                rich_input([span(b.clone())])
                    .id(format!("block-{i}"))
                    .placeholder(strings::forms_block_label(cx, i as i64 + 1))
                    .accessibility_label(strings::forms_block_label(cx, i as i64 + 1))
                    .on_input(cx.listener(move |this, e: &RichInputEvent, _| {
                        this.blocks[i] = prism::spans_text(&e.spans)
                    }))
            }))
            .on_selection_across(cx.listener(|this, e: &SelectionAcross, _| {
                this.blocks_selected = if e.is_across() { e.fields.len() } else { 0 }
            }))
            .on_edit_across(cx.listener(|this, e: &EditAcross, cx| {
                let index = |name: &str| {
                    name.strip_prefix("block-")
                        .and_then(|n| n.parse::<usize>().ok())
                };
                let (Some(first), Some(last)) =
                    (e.selection.fields.first(), e.selection.fields.last())
                else {
                    return;
                };
                let (Some(a), Some(z)) = (index(&first.field), index(&last.field)) else {
                    return;
                };
                let head: String = this.blocks[a].chars().take(first.start).collect();
                let tail: String = this.blocks[z].chars().skip(last.end).collect();
                let caret = head.chars().count() + e.text.chars().count();
                let merged = if e.cause == EditCause::Enter {
                    vec![head, tail]
                } else {
                    vec![format!("{head}{}{tail}", e.text)]
                };
                this.blocks.splice(a..=z, merged);
                this.blocks_selected = 0;
                // The caret after the new text (the next block's start after Enter).
                if e.cause == EditCause::Enter {
                    cx.set_selection_across(
                        (format!("block-{}", a + 1), 0),
                        (format!("block-{}", a + 1), 0),
                    );
                } else {
                    cx.set_selection_across(
                        (format!("block-{a}"), caret),
                        (format!("block-{a}"), caret),
                    );
                }
            }));
        // From the app (touch screens have no ⇧↓): the whole document.
        let last = self.blocks.len().saturating_sub(1);
        let end = self.blocks.last().map_or(0, |b| b.chars().count());
        let select_all = ui::button(strings::forms_blocks_select_all(cx)).on_click(cx.listener(
            move |_, _, cx| {
                cx.set_selection_across(("block-0", 0), (format!("block-{last}"), end));
            },
        ));
        div()
            .flex_col()
            .gap(4.)
            .child(text(strings::forms_blocks(cx)).text_size(12.))
            .child(flow)
            .child(div().flex_row().child(select_all))
            .child(
                text(if self.blocks_selected > 1 {
                    strings::forms_blocks_selected(cx, self.blocks_selected as i64)
                } else {
                    String::new()
                })
                .text_size(12.),
            )
    }
}
