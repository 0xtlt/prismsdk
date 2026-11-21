//! The Interaction card of the Components tab: checkbox, radio group, a time
//! of day, a search field with an icon and suggestions, a file input, a
//! drawn level bar (drag and tap position), a pointer read-out that also
//! zooms with a pinch (`on_magnify`), an
//! arrow-key pad, a row of cards that move after a long press (#243), a
//! board whose cards are dragged between two columns (#251), and a sheet, a popover and a dialog opened without a route. Its own entity, so dragging the level only re-renders this card.

use prism::Div;
use prism::prelude::*;
use prism::ui::{self, Theme};

use crate::strings;

/// Cells per side of the arrow-key pad.
const PAD: i32 = 5;

pub struct Interact {
    updates: bool,
    every: Option<usize>,
    alarm: u32,
    search: String,
    file: Option<String>,
    level: f32,
    /// The level when the current drag started.
    drag_from: f32,
    /// Where the pointer last was over the read-out area.
    pointer: Option<Point>,
    hovered: bool,
    /// The read-out's zoom, from pinches.
    zoom: f32,
    dot: (i32, i32),
    sheet: bool,
    popover: bool,
    dialog: bool,
    confirmed: u32,
    /// The animated tile sits alone, not in a row beside its caption.
    moved: bool,
    /// The tile was tapped: a stronger color.
    lit: bool,
    /// The cards of the sideways row, in order (#243).
    cards: Vec<u32>,
    /// The card being dragged, by its number.
    lifted: Option<u32>,
    /// The board's columns (#251): card numbers, in order.
    columns: [Vec<usize>; 2],
    /// Where a dragged card would land: its column and slot.
    slot: Option<(usize, usize)>,
}

impl Default for Interact {
    fn default() -> Self {
        Interact {
            updates: true,
            every: Some(1),
            alarm: 7 * 60 + 30,
            search: String::new(),
            file: None,
            level: 0.6,
            drag_from: 0.6,
            pointer: None,
            hovered: false,
            zoom: 1.,
            dot: (2, 2),
            sheet: false,
            popover: false,
            dialog: false,
            confirmed: 0,
            moved: false,
            lit: false,
            cards: (1..=8).collect(),
            lifted: None,
            columns: [vec![0, 1, 2, 3], vec![4, 5]],
            slot: None,
        }
    }
}

/// Width of the level bar, in points.
const BAR: f32 = 240.;
/// A card of the sideways row and the gap after it, in points.
const CARD: f32 = 88.;
const CARD_GAP: f32 = 8.;
/// Height of a board card and the gap under it, in points.
const BOARD_CARD: f32 = 40.;
const BOARD_GAP: f32 = 6.;

impl Interact {
    fn set_level(&mut self, x: f32) {
        self.level = (x / BAR).clamp(0., 1.);
    }

    fn level_bar(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        let pct = (self.level * 100.).round() as i64;
        div()
            .w(px(BAR))
            .h(px(28.))
            .rounded(14.)
            .bg(t.border)
            .accessibility_role(Role::Slider)
            .accessibility_label(strings::interact_level(cx))
            .accessibility_value(format!("{pct} %"))
            // Dragged sideways: left-right arrows, not the clickable hand.
            .cursor(Cursor::ResizeHorizontal)
            .on_click(cx.listener(|this, e: &ClickEvent, _| {
                if let Some(p) = e.position {
                    this.set_level(p.x);
                }
            }))
            .on_drag(cx.listener(|this, e: &DragEvent, _| match e.phase {
                DragPhase::Start => {
                    this.drag_from = this.level;
                    this.set_level(e.position().x);
                }
                DragPhase::Move | DragPhase::End => this.set_level(e.position().x),
                DragPhase::Cancel => this.level = this.drag_from,
            }))
            .on_key_down(cx.listener(|this, e: &KeyEvent, _| {
                if e.is("ArrowRight") || e.is("ArrowUp") {
                    this.level = (this.level + 0.05).min(1.);
                } else if e.is("ArrowLeft") || e.is("ArrowDown") {
                    this.level = (this.level - 0.05).max(0.);
                }
            }))
            .child(
                div()
                    .absolute()
                    .left(0.)
                    .top(0.)
                    .bottom(0.)
                    .w(px(BAR * self.level))
                    .rounded(14.)
                    .bg(t.accent)
                    .hit_test(false),
            )
            .child(
                // Drawn over the bar: clicks go through to it.
                text(format!("{pct} %"))
                    .absolute()
                    .left(12.)
                    .top(4.)
                    .text_size(14.)
                    .semibold()
                    .text_color(t.on_accent)
                    .hit_test(false)
                    .accessibility_hidden(),
            )
    }

    /// Cards in a row that scrolls sideways (#243): on a touch screen a
    /// swipe scrolls it, a long press picks a card up to move it.
    fn card_row(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        let row = scroll_x()
            .gap(CARD_GAP)
            .py(4.)
            .children(self.cards.iter().map(|&n| {
                let lifted = self.lifted == Some(n);
                div()
                    .w(px(CARD))
                    .h(px(64.))
                    .flex_shrink_0()
                    .rounded(t.radius)
                    .border(if lifted { 2. } else { 1. })
                    .border_color(if lifted { t.accent } else { t.border })
                    .bg(t.surface)
                    .items_center()
                    .justify_center()
                    .accessibility_label(strings::interact_card(cx, n as i64))
                    .on_drag(cx.listener(move |this, e: &DragEvent, _| match e.phase {
                        DragPhase::Start | DragPhase::Move => this.lifted = Some(n),
                        DragPhase::End => {
                            this.lifted = None;
                            // Dropped where the finger went: whole cards away.
                            let Some(from) = this.cards.iter().position(|&c| c == n) else {
                                return;
                            };
                            let steps = (e.translation.x / (CARD + CARD_GAP)).round() as i64;
                            let to = (from as i64 + steps).clamp(0, this.cards.len() as i64 - 1);
                            let card = this.cards.remove(from);
                            this.cards.insert(to as usize, card);
                        }
                        DragPhase::Cancel => this.lifted = None,
                    }))
                    .drag_after_long_press(std::time::Duration::from_millis(400))
                    .child(
                        text(n.to_string())
                            .text_size(22.)
                            .semibold()
                            .text_color(t.text),
                    )
            }));
        div()
            .gap(4.)
            .child(text(strings::interact_cards(cx)).semibold())
            .child(row)
            .child(
                text(strings::interact_cards_hint(cx))
                    .text_size(13.)
                    .text_color(t.muted),
            )
    }

    /// Where a card dropped at `y` in a column goes (#251).
    fn slot_at(&self, column: usize, y: f32) -> usize {
        let slot = ((y - 8.) / (BOARD_CARD + BOARD_GAP) + 0.5).max(0.) as usize;
        slot.min(self.columns[column].len())
    }

    /// Two columns of cards (#251): a card is dragged onto the other column
    /// (or within its own), a line shows where it would land, and the
    /// first column scrolls when the pointer stays near its edge.
    fn board(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        let names = strings::interact_board_cards(cx);
        let names: Vec<&str> = names.split(',').collect();
        let titles = [
            strings::interact_board_todo(cx),
            strings::interact_board_done(cx),
        ];
        let line = || div().h(px(3.)).rounded(1.5).bg(t.accent).flex_shrink_0();
        let mut columns = div().flex_row().gap(12.);
        for (c, title) in titles.into_iter().enumerate() {
            let mut cards = Vec::new();
            for (i, card) in self.columns[c].iter().enumerate() {
                if self.slot == Some((c, i)) {
                    cards.push(line().into_element());
                }
                cards.push(
                    div()
                        .key(format!("card-{card}"))
                        .id(format!("card-{card}"))
                        .h(px(BOARD_CARD))
                        .flex_shrink_0()
                        .px(12.)
                        .justify_center()
                        .rounded(t.radius)
                        .bg(t.surface)
                        .border(1.)
                        .border_color(t.border)
                        .draggable(DragPayload::new("card", card.to_string()))
                        .child(text(names.get(*card).copied().unwrap_or("?")).text_size(14.))
                        .into_element(),
                );
            }
            if self.slot == Some((c, self.columns[c].len())) {
                cards.push(line().into_element());
            }
            let column = scroll()
                .id(format!("board-{c}"))
                .fit()
                .h(px(if c == 0 { 150. } else { 200. }))
                .flex_shrink_0()
                .p(8.)
                .gap(BOARD_GAP)
                .rounded(t.radius)
                .bg(t.border.with_alpha(0.35))
                .drop_hover(move |s| s.bg(t.accent.with_alpha(0.12)))
                .accessibility_label(title.clone())
                .children(cards)
                .on_drag_over(
                    "card",
                    cx.listener(move |this, e: &DragOverEvent, _| {
                        this.slot = match e.phase {
                            DragOverPhase::Leave => None,
                            _ => Some((c, this.slot_at(c, e.position.y))),
                        };
                    }),
                )
                .on_drop_payload(
                    "card",
                    cx.listener(move |this, e: &DropEvent, _| {
                        let Ok(card) = e.payload.data.parse::<usize>() else {
                            return;
                        };
                        let mut slot = this.slot_at(c, e.position.y);
                        for (k, col) in this.columns.iter_mut().enumerate() {
                            if let Some(i) = col.iter().position(|x| *x == card) {
                                col.remove(i);
                                if k == c && i < slot {
                                    slot -= 1;
                                }
                            }
                        }
                        this.columns[c].insert(slot.min(this.columns[c].len()), card);
                    }),
                );
            columns = columns.child(
                div()
                    .flex_1()
                    .gap(6.)
                    .child(text(title).text_size(13.).semibold().text_color(t.muted))
                    .child(column),
            );
        }
        div()
            .gap(8.)
            .child(text(strings::interact_board(cx)).semibold())
            .child(columns)
    }

    fn pointer_area(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        let readout = match (self.hovered, self.pointer) {
            (true, Some(p)) => {
                strings::interact_pointer_at(cx, p.x.round() as i64, p.y.round() as i64)
            }
            _ => strings::interact_pointer_hint(cx),
        };
        let readout = if (self.zoom - 1.).abs() > 0.005 {
            let percent = (self.zoom * 100.).round() as i64;
            format!("{readout} · {}", strings::interact_zoom(cx, percent))
        } else {
            readout
        };
        div()
            .h(px(72.))
            .rounded(t.radius)
            .border(1.)
            .border_color(if self.hovered { t.accent } else { t.border })
            .items_center()
            .justify_center()
            .on_hover(cx.listener(|this, e: &HoverEvent, _| this.hovered = e.hovered))
            .on_pointer_move(cx.listener(|this, e: &PointerEvent, _| {
                this.hovered = true;
                this.pointer = Some(e.position);
            }))
            .on_magnify(cx.listener(|this, e: &MagnifyEvent, _| {
                this.zoom = (this.zoom * e.scale).clamp(0.25, 8.);
            }))
            .child(text(readout).text_size(14.).text_color(t.muted))
    }

    fn key_pad(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        let cell = 18.;
        let (x, y) = self.dot;
        div()
            .id("pad")
            .flex_shrink_0()
            .w(px(cell * PAD as f32 + 8.))
            .h(px(cell * PAD as f32 + 8.))
            .p(4.)
            .rounded(t.radius)
            .bg(t.surface)
            .border(1.)
            .border_color(t.border)
            .accessibility_role(Role::Group)
            .accessibility_label(strings::interact_pad(cx))
            .accessibility_value(format!("{}, {}", x + 1, y + 1))
            .on_click(cx.listener(|_, _, cx| cx.focus("pad")))
            .on_key_down(cx.listener(|this, e: &KeyEvent, cx| {
                let (dx, dy) = match e.key.as_str() {
                    "ArrowLeft" => (-1, 0),
                    "ArrowRight" => (1, 0),
                    "ArrowUp" => (0, -1),
                    "ArrowDown" => (0, 1),
                    _ => return,
                };
                this.dot = (
                    (this.dot.0 + dx).clamp(0, PAD - 1),
                    (this.dot.1 + dy).clamp(0, PAD - 1),
                );
                cx.stop_propagation();
            }))
            .child(
                div()
                    .absolute()
                    .left(4. + cell * x as f32 + 3.)
                    .top(4. + cell * y as f32 + 3.)
                    .size(px(cell - 6.))
                    .rounded((cell - 6.) / 2.)
                    .bg(t.accent)
                    .transition(Transition::spring(220)),
            )
    }

    /// A tile with `.animate(…)` that changes place in the tree: where it
    /// lands it is a new node, which appears with its style at once, its
    /// own-width padding included (#50). Tapping it animates its color.
    fn moving_tile(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        let tile = div()
            .p(16.)
            .rounded(t.radius)
            .bg(t.accent.with_alpha(if self.lit { 0.32 } else { 0.16 }))
            .when_width_at_least(200., |s| s.p(28.))
            .animate(420)
            .on_click(cx.listener(|this, _, _| this.lit ^= true))
            .child(text(strings::interact_tile(cx)).text_color(t.accent));
        let place = if self.moved {
            div().child(tile)
        } else {
            div().child(
                div()
                    .flex_row()
                    .items_center()
                    .gap(12.)
                    .child(tile)
                    .child(text(strings::interact_tile_beside(cx)).text_color(t.muted)),
            )
        };
        div().gap(8.).child(place).child(
            ui::button(strings::interact_tile_move(cx))
                .secondary()
                .on_click(cx.listener(|this, _, _| this.moved ^= true)),
        )
    }

    fn overlays(&self, cx: &mut Context<Self>) -> Div {
        let t = Theme::of(cx);
        let close_sheet = cx.listener(|this: &mut Self, _: &ClickEvent, _| this.sheet = false);
        let gallery = div()
            .p(20.)
            .gap(12.)
            .child(ui::heading(strings::interact_sheet_title(cx)))
            .child(text(strings::interact_sheet_body(cx)).text_color(t.muted))
            .child(div().flex_row().flex_wrap().gap(8.).children(
                ["🌅", "🏔️", "🌊", "🌲", "🏜️", "🌌"].map(|e| {
                    div()
                        .size(px(64.))
                        .rounded(t.radius)
                        .bg(t.accent.with_alpha(0.12))
                        .items_center()
                        .justify_center()
                        .child(text(e).text_size(28.))
                }),
            ))
            .child(ui::button(strings::interact_close(cx)).on_click(close_sheet));
        let filters = div()
            .p(16.)
            .gap(10.)
            .w(px(240.))
            .child(text(strings::interact_filters(cx)).semibold())
            .child(
                checkbox(self.updates)
                    .label(strings::interact_updates(cx))
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.updates = e.checked())),
            )
            .child(
                radio_group(strings::interact_every(cx).split(','), self.every)
                    .accessibility_label(strings::interact_frequency(cx))
                    .on_change(
                        cx.listener(|this, e: &ChangeEvent, _| this.every = Some(e.index())),
                    ),
            );
        let confirm = div()
            .p(20.)
            .gap(14.)
            .w(px(300.))
            .child(ui::heading(strings::interact_dialog_title(cx)).level(3))
            .child(text(strings::interact_dialog_body(cx)).text_color(t.muted))
            .child(
                div()
                    .flex_row()
                    .justify_end()
                    .gap(8.)
                    .child(
                        ui::button(strings::interact_cancel(cx))
                            .secondary()
                            .on_click(cx.listener(|this, _, _| this.dialog = false)),
                    )
                    .child(
                        ui::button(strings::interact_confirm(cx))
                            .on_click(cx.listener(|this, _, _| {
                                this.dialog = false;
                                this.confirmed += 1;
                            }))
                            .style(|s| s.flex_grow = 0.),
                    ),
            );
        div()
            .flex_row()
            .flex_wrap()
            .gap(8.)
            .child(
                ui::button(strings::interact_open_sheet(cx))
                    .on_click(cx.listener(|this, _, _| this.sheet = true)),
            )
            .child(
                // The popover is anchored to this wrapper (its parent).
                div()
                    .child(
                        ui::button(strings::interact_open_popover(cx))
                            .secondary()
                            .on_click(cx.listener(|this, _, _| this.popover = !this.popover)),
                    )
                    .child(
                        popover(filters)
                            .open(self.popover)
                            .on_dismiss(cx.listener(|this, _, _| this.popover = false)),
                    ),
            )
            .child(
                ui::button(strings::interact_open_dialog(cx))
                    .secondary()
                    .on_click(cx.listener(|this, _, _| this.dialog = true)),
            )
            .child(
                sheet(gallery)
                    .open(self.sheet)
                    .on_dismiss(cx.listener(|this, _, _| this.sheet = false)),
            )
            .child(
                dialog(confirm)
                    .open(self.dialog)
                    .on_dismiss(cx.listener(|this, _, _| this.dialog = false)),
            )
    }
}

impl Render for Interact {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx);
        let cities = strings::interact_cities(cx);
        let alarm = format!("{:02}:{:02}", self.alarm / 60, self.alarm % 60);
        let level_bar = self.level_bar(cx);
        let pointer_area = self.pointer_area(cx);
        let card_row = self.card_row(cx);
        let key_pad = self.key_pad(cx);
        let overlays = self.overlays(cx);
        let moving_tile = self.moving_tile(cx);
        let board = self.board(cx);
        let this = cx.entity();
        // The bytes reach the app: `read()` them (here, just counted).
        let on_pick = move |files: Vec<PickedFile>, app: &mut App| {
            let Some(f) = files.into_iter().next() else {
                return;
            };
            let bytes = f.read().map_or(f.size, |b| b.len() as u64);
            let label = format!("{} · {} B", f.name, bytes);
            this.update(app, |this, cx| {
                this.file = Some(label);
                cx.notify();
            });
        };
        ui::card()
            .child(ui::heading(strings::interact_heading(cx)))
            .child(
                checkbox(self.updates)
                    .label(strings::interact_updates(cx))
                    .on_change(cx.listener(|this, e: &ChangeEvent, _| this.updates = e.checked())),
            )
            .child(
                radio_group(strings::interact_every(cx).split(','), self.every)
                    .horizontal()
                    .accessibility_label(strings::interact_frequency(cx))
                    .on_change(
                        cx.listener(|this, e: &ChangeEvent, _| this.every = Some(e.index())),
                    ),
            )
            .child(
                div()
                    .flex_row()
                    .items_center()
                    .gap(12.)
                    .child(text(strings::interact_alarm(cx, &alarm)).flex_1())
                    .child(
                        time_picker(self.alarm)
                            .accessibility_label(strings::interact_alarm_label(cx))
                            .on_change(
                                cx.listener(|this, e: &ChangeEvent, _| this.alarm = e.minutes()),
                            ),
                    ),
            )
            .child(
                text_input()
                    .id("search")
                    .value(self.search.clone())
                    .placeholder(strings::interact_search(cx))
                    .leading_icon("magnifyingglass")
                    .suggestions(cities.split(','))
                    .max_length(24)
                    .on_input(cx.listener(|this, e: &InputEvent, _| this.search = e.value.clone())),
            )
            .child({
                let input = ui::file_input(strings::interact_photo(cx))
                    .accept(["image/*"])
                    .button(strings::interact_choose(cx))
                    .on_pick(on_pick);
                match &self.file {
                    Some(f) => input.selected(f.clone()),
                    None => input,
                }
            })
            .child(text(strings::interact_level(cx)).semibold())
            .child(level_bar)
            .child(pointer_area)
            .child(card_row)
            .child(
                div()
                    .flex_row()
                    .items_center()
                    .gap(12.)
                    .child(key_pad)
                    .child(
                        text(strings::interact_pad_hint(cx))
                            .text_size(13.)
                            .text_color(t.muted)
                            .flex_1()
                            .min_w(px(0.)),
                    ),
            )
            .child(board)
            .child(overlays)
            .child(moving_tile)
            .when(self.confirmed > 0, |d| {
                d.child(
                    text(strings::interact_confirmed(cx, self.confirmed as i64))
                        .text_size(13.)
                        .text_color(t.muted),
                )
            })
            .child(
                div()
                    .self_start()
                    .accessibility_role(Role::Link)
                    .on_click(cx.listener(|_, _, cx| {
                        cx.scroll_into_view("search");
                        cx.focus("search");
                    }))
                    .child(text(strings::interact_jump_button(cx)).text_color(t.accent)),
            )
    }
}
