//! The Forms card of the Components tab: sliders, a select, date and time
//! pickers, text input variants, a text area, a rich note with a toolbar
//! over its selection (#217), blocks selected as one text (#231), a rich
//! text editor with its toolbar and Markdown (#294), styled chips (#329),
//! a template field whose `{{` opens a completion list at the caret
//! (#328), a spinner
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
    /// Styled chips (#329): the field's runs and the chip opened by a
    /// click or a tap.
    chips: Vec<Span>,
    chip_open: Option<String>,
    /// Paragraphs selected as one text (#231), and how many the selection
    /// spans.
    blocks: Vec<String>,
    blocks_selected: usize,
    /// The template (#328): its text, its caret, the completion item
    /// highlighted, and whether Escape closed the list.
    template: String,
    template_sel: SelectionEvent,
    completion_item: usize,
    completion_closed: bool,
    /// The rich text editor (#294), and its value as Markdown.
    editor: Option<Entity<ui::RichTextInput>>,
    editor_markdown: String,
    /// Whether the note and the blocks have their sample texts, made in
    /// the app's language on the first render.
    samples: bool,
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
            note: Vec::new(),
            note_sel: SelectionEvent::default(),
            toolbar: None,
            note_log: String::new(),
            chips: Vec::new(),
            chip_open: None,
            blocks: Vec::new(),
            blocks_selected: 0,
            template: String::new(),
            template_sel: SelectionEvent::default(),
            completion_item: 0,
            completion_closed: false,
            editor: None,
            editor_markdown: String::new(),
            samples: false,
        }
    }
}

impl Render for Forms {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.samples {
            self.samples = true;
            self.note = note_sample(cx);
            self.chips = chips_sample(cx);
            self.blocks = vec![
                strings::forms_block_first(cx),
                strings::forms_block_second(cx),
                strings::forms_block_third(cx),
            ];
            self.make_editor(cx);
        }
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
            .child(self.chips(cx))
            .child(self.blocks(cx))
            .child(self.template(cx))
            .child(self.editor(cx))
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

/// The fields a template can insert (#328).
const TEMPLATE_FIELDS: [&str; 6] = ["name", "email", "company", "city", "date", "due_date"];

/// The `{{query` typed just before `caret` in `text`: where its braces
/// start (characters) and the query, if the caret is in one.
fn completion_query(text: &str, caret: usize) -> Option<(usize, String)> {
    let before: String = text.chars().take(caret).collect();
    let at = before.rfind("{{")?;
    let query = &before[at + 2..];
    query
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_')
        .then(|| (before[..at].chars().count(), query.to_string()))
}

/// The selection's runs as Markdown: bold, links and chips (what the
/// note copies).
fn markdown(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| match (s.href(), s.is_bold()) {
            _ if s.is_chip() => format!("[{}]", s.chip_label().unwrap_or_default()),
            (Some(href), _) => format!("[{}]({href})", s.text()),
            (None, true) => format!("**{}**", s.text()),
            (None, false) => s.text().to_string(),
        })
        .collect()
}

/// The rich note's sample text (#217), in the app's language: bold words,
/// then a link a modifier-click opens (a key's click: desktops and the
/// web; touch screens have none) and a chip a click or a tap opens.
fn note_sample(cx: &App) -> Vec<Span> {
    // Where the template puts each styled part.
    const WORDS: char = '\u{E000}';
    const LINK: char = '\u{E001}';
    const PAGE: char = '\u{E002}';
    let (words, link_, page) = (WORDS.to_string(), LINK.to_string(), PAGE.to_string());
    let click = match cx.platform() {
        Platform::Macos => Some(strings::forms_note_click_mac(cx)),
        Platform::Windows | Platform::Linux => Some(strings::forms_note_click(cx)),
        Platform::Web => Some(strings::forms_note_click_web(cx)),
        _ => None,
    };
    let template = match click {
        Some(click) => strings::forms_note_text(cx, &words, click, &link_, &page),
        None => strings::forms_note_text_touch(cx, &words, &page),
    };
    let mut spans = Vec::new();
    let mut plain = String::new();
    for c in template.chars() {
        let part = match c {
            WORDS => span(strings::forms_note_words(cx)).bold(),
            LINK => link(
                strings::forms_note_link(cx),
                "https://github.com/0xtlt/prismsdk",
            ),
            PAGE => chip("page:roadmap", strings::forms_note_page(cx)).chip_icon("doc.text"),
            c => {
                plain.push(c);
                continue;
            }
        };
        if !plain.is_empty() {
            spans.push(span(std::mem::take(&mut plain)));
        }
        spans.push(part);
    }
    if !plain.is_empty() {
        spans.push(span(plain));
    }
    spans
}

/// Where the chips sample's template puts each chip.
const PERSON: char = '\u{E010}';
const TAG: char = '\u{E011}';
const PAGE_CHIP: char = '\u{E012}';

/// The styled chips' sample (#329): a person (a round avatar drawn as a
/// vector, a bold name and a grey summary cut at 150 points, its whole
/// text as a tooltip, a pill), a tag (an SVG icon, a ring, a taller box)
/// and a plain chip as it was.
fn chips_sample(cx: &App) -> Vec<Span> {
    let template = strings::forms_chips_text(
        cx,
        PERSON.to_string(),
        TAG.to_string(),
        PAGE_CHIP.to_string(),
    );
    let t = Theme::of(cx);
    let mut spans = Vec::new();
    let mut plain = String::new();
    for c in template.chars() {
        let part = match c {
            PERSON => {
                let avatar = [
                    Path::circle(0.5, 0.5, 0.5).fill(t.accent),
                    Path::circle(0.5, 0.38, 0.17).fill(t.on_accent),
                    Path::new()
                        .move_to(0.2, 0.86)
                        .curve_to((0.28, 0.6), (0.72, 0.6), (0.8, 0.86))
                        .fill(t.on_accent),
                ];
                chip("user:ada", "")
                    .chip_label_runs([
                        span(strings::forms_chips_person(cx)).bold(),
                        span(strings::forms_chips_person_summary(cx)).text_color(t.muted),
                    ])
                    .chip_icon(prism::ChipIcon::shape(avatar))
                    .tooltip(strings::forms_chips_person_tooltip(cx))
                    .chip_max_width(150.)
                    .chip_radius(11.)
                    .chip_height(22.)
                    .chip_test_id("chip-ada")
                    .chip_attr("data-person", "ada")
            }
            TAG => chip("tag:design", strings::forms_chips_tag(cx))
                .chip_icon(prism::ChipIcon::svg(
                    r#"<svg viewBox="0 0 16 16"><path d="M2 2h6l6 6-6 6-6-6z" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round"/><circle cx="5.5" cy="5.5" r="1.2" fill="currentColor"/></svg>"#,
                ))
                .text_color(t.accent)
                .bg(t.accent.with_alpha(0.12))
                .chip_ring(1., t.accent.with_alpha(0.45))
                .chip_radius(6.)
                .chip_height(24.)
                .chip_test_id("chip-tag"),
            PAGE_CHIP => chip("page:roadmap", strings::forms_chips_page(cx)).chip_icon("doc.text"),
            c => {
                plain.push(c);
                continue;
            }
        };
        if !plain.is_empty() {
            spans.push(span(std::mem::take(&mut plain)));
        }
        spans.push(part);
    }
    if !plain.is_empty() {
        spans.push(span(plain));
    }
    spans
}

impl Forms {
    /// Styled chips (#329): a click or a tap opens a chip, marked with the
    /// `open` state (a class and `data-state` on the web) and a thicker
    /// accent ring on every platform.
    fn chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let spans: Vec<Span> = self
            .chips
            .iter()
            .map(|s| {
                let Some(key) = s.chip_key().map(str::to_string) else {
                    return s.clone();
                };
                let open = self.chip_open.as_deref() == Some(key.as_str());
                let s = if open {
                    s.clone().chip_state("open").chip_ring(2., t.accent)
                } else if s.chip_state_value().is_some() {
                    // Closed again: its own ring back (the tag's), or none.
                    let ring = if key == "tag:design" { 1. } else { 0. };
                    s.clone()
                        .chip_state("")
                        .chip_ring(ring, t.accent.with_alpha(0.45))
                } else {
                    s.clone()
                };
                s.on_click(cx.listener(move |this, _: &ClickEvent, _| {
                    this.chip_open =
                        (this.chip_open.as_deref() != Some(key.as_str())).then(|| key.clone());
                }))
            })
            .collect();
        let hint = match self
            .chip_open
            .as_ref()
            .and_then(|k| self.chips.iter().find(|s| s.chip_key() == Some(k.as_str())))
            .and_then(|s| s.chip_label())
        {
            Some(label) => strings::forms_chips_open(cx, label),
            None => strings::forms_chips_hint(cx),
        };
        div()
            .flex_col()
            .gap(4.)
            .child(
                rich_input(spans)
                    .accessibility_label(strings::forms_chips(cx))
                    .on_input(
                        cx.listener(|this, e: &RichInputEvent, _| this.chips = e.spans.clone()),
                    ),
            )
            .child(text(hint).text_size(12.))
    }

    /// A rich note (#217): a Bold button above the selected words, copies
    /// with Markdown, pastes and ⌘-clicks reported.
    fn note(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // A tap or click on a chip opens its page (here, says so).
        let spans: Vec<Span> = self
            .note
            .iter()
            .map(|s| match s.chip_label() {
                Some(label) if s.is_chip() => {
                    let label = label.to_string();
                    s.clone()
                        .on_click(cx.listener(move |this, _: &ClickEvent, cx| {
                            this.note_log = strings::forms_note_chip(cx, &label);
                        }))
                }
                _ => s.clone(),
            })
            .collect();
        let field = rich_input(spans)
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

    /// The fields matching the `{{query` at the template's caret (#328),
    /// and where the braces start; `None` while no list shows.
    fn completions(&self) -> Option<(usize, String, Vec<&'static str>)> {
        if self.completion_closed {
            return None;
        }
        let (start, query) = completion_query(&self.template, self.template_sel.end)?;
        let found: Vec<&str> = TEMPLATE_FIELDS
            .iter()
            .copied()
            .filter(|f| f.starts_with(&query))
            .collect();
        (!found.is_empty()).then_some((start, query, found))
    }

    /// Replaces the `{{query` at the caret with `{{field}}`, the caret
    /// after it.
    fn complete(&mut self, field: &str) {
        let Some((start, query, _)) = self.completions() else {
            return;
        };
        let chars: Vec<char> = self.template.chars().collect();
        let end = (start + 2 + query.chars().count()).min(chars.len());
        let inserted = format!("{{{{{field}}}}}");
        let caret = start + inserted.chars().count();
        self.template = chars[..start].iter().collect::<String>()
            + &inserted
            + &chars[end..].iter().collect::<String>();
        self.template_sel = SelectionEvent::new(caret, caret);
        self.completion_item = 0;
    }

    /// A template field (#328): typing `{{` opens the fields it can insert
    /// in a list at the caret, which follows it as the user types. The
    /// field keeps the focus and the keys: ↑ and ↓ move through the list,
    /// Enter or Tab insert, Escape closes it; a click on an item inserts it.
    fn template(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::of(cx);
        let completions = self.completions();
        let open = completions.is_some();
        let field = text_input()
            .value(self.template.clone())
            .selection(self.template_sel)
            .placeholder(strings::forms_template_placeholder(cx, "{{"))
            .accessibility_label(strings::forms_template(cx))
            .on_input(cx.listener(|this, e: &InputEvent, _| {
                // The caret moves with what was typed; `on_select` says
                // exactly where right after.
                let (old, new) = (this.template.chars().count(), e.value.chars().count());
                let caret = (this.template_sel.end + new).saturating_sub(old).min(new);
                this.template = e.value.clone();
                this.template_sel = SelectionEvent::new(caret, caret);
                this.completion_closed = false;
                this.completion_item = 0;
            }))
            .on_select(cx.listener(|this, e: &SelectionEvent, _| this.template_sel = *e))
            .flex_1();
        let list = completions.map(|(_, _, found)| {
            let current = self.completion_item.min(found.len() - 1);
            div()
                .flex_col()
                .p(4.)
                .gap(2.)
                .bg(t.surface)
                .rounded(8.)
                .border(1.)
                .border_color(t.border)
                .shadow(shadow_md())
                .children(found.into_iter().enumerate().map(|(i, f)| {
                    let selected = i == current;
                    div()
                        .px(8.)
                        .py(4.)
                        .rounded(6.)
                        .bg(if selected { t.accent } else { t.surface })
                        .child(text(format!("{{{{{f}}}}}")).text_color(if selected {
                            t.on_accent
                        } else {
                            t.text
                        }))
                        .on_click(cx.listener(move |this, _, _| this.complete(f)))
                }))
        });
        div()
            .flex_col()
            .gap(4.)
            .child(
                text(strings::forms_template(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            )
            .child(
                div()
                    .flex_row()
                    .consume_keys(if open {
                        &["ArrowDown", "ArrowUp", "Enter", "Tab", "Escape"][..]
                    } else {
                        &[]
                    })
                    .on_key_down(cx.listener(|this, e: &KeyEvent, _| {
                        if let Some(sel) = e.selection {
                            this.template_sel = sel;
                        }
                        let Some((_, _, found)) = this.completions() else {
                            return;
                        };
                        let n = found.len();
                        match e.key.as_str() {
                            "ArrowDown" => this.completion_item = (this.completion_item + 1) % n,
                            "ArrowUp" => this.completion_item = (this.completion_item + n - 1) % n,
                            "Enter" | "Tab" => {
                                this.complete(found[this.completion_item.min(n - 1)])
                            }
                            "Escape" => this.completion_closed = true,
                            _ => {}
                        }
                    }))
                    .child(field)
                    .children(list.map(|list| {
                        // At the caret, below it (above without room), the
                        // field keeping the focus (#328).
                        popover(list)
                            .open(true)
                            .anchor_caret()
                            .take_focus(false)
                            .bare()
                            .size(200., 0.)
                            .on_dismiss(cx.listener(|this, _, _| this.completion_closed = true))
                    })),
            )
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
                    // Its visible title, for screen readers too.
                    .accessibility_label(strings::forms_shared(cx))
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
    /// The rich text editor (#294): its value kept as Markdown, ⌘K
    /// linking the selection to the guide (an app would open its own
    /// link picker).
    fn make_editor(&mut self, cx: &mut Context<Self>) {
        let sample = ui::RichText::from_markdown(&strings::forms_editor_sample(cx));
        let label = strings::forms_editor(cx);
        let placeholder = strings::forms_editor_placeholder(cx);
        let on_link = cx.listener(|this: &mut Self, e: &ui::LinkRequest, cx| {
            let Some(editor) = this.editor else { return };
            let linked = e.href.is_some();
            editor.update(cx, |ed, cx| {
                if linked {
                    ed.remove_link(cx)
                } else {
                    ed.set_link("https://prismsdk.dev", cx)
                }
            });
        });
        let on_change = cx.listener(|this: &mut Self, e: &ui::RichTextChange, _| {
            this.editor_markdown = e.value.to_markdown()
        });
        self.editor_markdown = sample.to_markdown();
        let editor = cx.new(move |_| {
            ui::rich_text_input(sample)
                .label(label)
                .placeholder(placeholder)
                .on_change(on_change)
                .on_link_request(on_link)
        });
        // The toolbar follows the selection.
        cx.observe(&editor, |_, _, _| {});
        self.editor = Some(editor);
    }

    fn editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor) = self.editor else {
            return div();
        };
        let t = Theme::of(cx);
        let f = editor.read(cx).formatting();
        let tool = |label: String, on: bool| {
            ui::button(label).small().variant(if on {
                ui::Variant::Primary
            } else {
                ui::Variant::Ghost
            })
        };
        let command = move |f: fn(&mut ui::RichTextInput, &mut Context<ui::RichTextInput>)| {
            move |_: &mut Self, _: &ClickEvent, cx: &mut Context<Self>| editor.update(cx, f)
        };
        let block = |kind: ui::BlockKind| {
            move |_: &mut Self, _: &ClickEvent, cx: &mut Context<Self>| {
                editor.update(cx, |ed, cx| ed.toggle_block(kind, cx))
            }
        };
        let toolbar = div()
            .flex_row()
            .flex_wrap()
            .gap(4.)
            .child(
                tool(strings::forms_editor_bold(cx), f.bold)
                    .on_click(cx.listener(command(|ed, cx| ed.toggle_bold(cx)))),
            )
            .child(
                tool(strings::forms_editor_italic(cx), f.italic)
                    .on_click(cx.listener(command(|ed, cx| ed.toggle_italic(cx)))),
            )
            .child(
                tool(strings::forms_editor_link(cx), f.in_link())
                    .on_click(cx.listener(command(|ed, cx| ed.request_link(cx)))),
            )
            .child(
                tool("H2".into(), f.block == ui::BlockKind::Heading(2))
                    .on_click(cx.listener(block(ui::BlockKind::Heading(2)))),
            )
            .child(
                tool("•".into(), f.block == ui::BlockKind::Bullet)
                    .on_click(cx.listener(block(ui::BlockKind::Bullet))),
            )
            .child(
                tool("1.".into(), f.block == ui::BlockKind::Numbered)
                    .on_click(cx.listener(block(ui::BlockKind::Numbered))),
            );
        div()
            .flex_col()
            .gap(6.)
            .child(text(strings::forms_editor(cx)).text_size(12.))
            .child(toolbar)
            .child(editor)
            .child(
                text(self.editor_markdown.clone())
                    .font_mono()
                    .text_size(12.)
                    .text_color(t.muted),
            )
    }
}
