//! Rich inputs (#115): a text area's `GtkTextView` showing the runs of a
//! [`rich_input`](prism_core::rich_input) as text tags, each chip and
//! inline icon a widget at a child anchor (one character, U+FFFC, that the
//! caret, the selection and Backspace treat as one).
//!
//! Edits reach the app as the plain text (`UiEvent::Input`, chips
//! included): prism-core gives the typed text the style of the run around
//! it and keeps the other runs. The app's runs come back on a later turn:
//! when they hold the text the view shows (the echo of the typing, or the
//! app restyling it), their tags replace the old ones in place, caret and
//! undo untouched; when the user typed further meanwhile, they wait for the
//! next echo. New text from the app comes first through `set_text` (#73),
//! its chips as plain U+FFFC characters that become anchors here. Without
//! the app's hooks, a copy is plain text with each chip's label, and a
//! paste is plain text without chips.
//!
//! GTK's undo history keeps characters, not anchors: undoing the deletion
//! of a chip puts back a plain U+FFFC. It becomes an anchor again while the
//! undo applies, when the history records nothing, so the history (and the
//! redo) survives the app's runs that show the chip again (#338).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use prism_core::{Color, NodeId, OBJECT_CHAR, Style, TextRun, TextTransform, UiEvent, decode_runs};
use prism_native::text::{StyledRun, StyledText};

thread_local! {
    static RICH: RefCell<HashMap<NodeId, Rich>> = RefCell::new(HashMap::new());
}

/// A rich input: its view, the app's runs and style, and what shows them.
struct Rich {
    view: gtk::TextView,
    runs: String,
    style: Option<Style>,
    /// The tags of the runs shown (anonymous, in the buffer's table).
    tags: Vec<gtk::TextTag>,
    chips: Vec<Chip>,
}

/// A chip (or inline icon) at its anchor: a box with its icon (a symbol,
/// or a drawing, #329) and label.
struct Chip {
    anchor: gtk::TextChildAnchor,
    widget: gtk::Box,
    icon: gtk::Label,
    vector: gtk::DrawingArea,
    /// The vector icon's paths, in its box.
    paths: Rc<RefCell<Vec<prism_core::Path>>>,
    label: gtk::Label,
    /// The `prism-chip-<state>` class it has.
    state: RefCell<Option<String>>,
    /// How far it hangs below the baseline, pixels.
    below: Cell<i32>,
    css: gtk::CssProvider,
    link: Rc<Cell<Option<u64>>>,
    /// What a copy writes for it: a chip's label, nothing for an icon.
    copied: String,
}

/// The box of characters `start..end` of `view` (a chip's own box for its
/// character, else the run on the first line it covers; a caret line for
/// an empty range), in the view's coordinates, kept inside what shows of
/// it (#327).
pub(crate) fn range_box(view: &gtk::TextView, start: usize, end: usize) -> Option<gdk::Rectangle> {
    let b = view.buffer();
    let len = b.char_count().max(0) as usize;
    if start > len {
        return None;
    }
    let at = |c: usize| b.iter_at_offset(c.min(len) as i32);
    let a = view.iter_location(&at(start));
    let (x, y) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, a.x(), a.y());
    let w = if end <= start {
        1
    } else if end > start + 1 {
        let z = view.iter_location(&at(end - 1));
        if z.y() == a.y() {
            z.x() + z.width() - a.x()
        } else {
            view.width() - x
        }
    } else {
        a.width()
    };
    let (vw, vh) = (view.width(), view.height());
    let (x0, y0) = (x.clamp(0, vw), y.clamp(0, vh));
    let (x1, y1) = ((x + w.max(1)).clamp(x0, vw), (y + a.height()).clamp(y0, vh));
    Some(gdk::Rectangle::new(
        x0,
        y0,
        (x1 - x0).max(1),
        (y1 - y0).max(1),
    ))
}

/// Makes text area `view` of node `id` a rich input.
pub(crate) fn setup(id: NodeId, view: &gtk::TextView) {
    let b = view.buffer();
    // While an undo or a redo applies (between the handlers before and
    // after the signal's own).
    let applying = Rc::new(Cell::new(false));
    for after in [false, true] {
        for signal in ["undo", "redo"] {
            let applying = applying.clone();
            b.connect_local(signal, after, move |_| {
                applying.set(!after);
                None
            });
        }
    }
    let busy = Cell::new(false);
    b.connect_changed(move |b| {
        if applying.get() && !busy.replace(true) {
            anchor_objects(b);
            busy.set(false);
        }
    });
    RICH.with(|r| {
        r.borrow_mut().insert(
            id,
            Rich {
                view: view.clone(),
                runs: String::new(),
                style: None,
                tags: Vec::new(),
                chips: Vec::new(),
            },
        )
    });
}

pub(crate) fn forget(id: NodeId) {
    RICH.with(|r| r.borrow_mut().remove(&id));
}

pub(crate) fn is_rich(id: NodeId) -> bool {
    RICH.with(|r| r.borrow().contains_key(&id))
}

/// The runs to show (`Widgets::set_rich_value`).
pub(crate) fn set_runs(id: NodeId, runs: &str) {
    RICH.with(|r| {
        if let Some(rich) = r.borrow_mut().get_mut(&id) {
            rich.runs = runs.to_string();
            show(rich);
        }
    });
}

/// The field's style (its font, size and color): the base of the runs.
pub(crate) fn set_style(id: NodeId, style: &Style) {
    RICH.with(|r| {
        if let Some(rich) = r.borrow_mut().get_mut(&id) {
            rich.style = Some(style.clone());
            show(rich);
        }
    });
}

/// The appearance changed: colors take their light or dark variant.
pub(crate) fn restyle_all() {
    RICH.with(|r| r.borrow_mut().values_mut().for_each(show));
}

/// Makes each plain U+FFFC of `b` (a chip's character an undo put back)
/// an anchor again, the characters unchanged: the app's runs then show
/// the chip in it.
fn anchor_objects(b: &gtk::TextBuffer) {
    let plain: Vec<i32> = slice(b)
        .chars()
        .enumerate()
        .filter(|&(_, c)| c == OBJECT_CHAR)
        .map(|(i, _)| i as i32)
        .filter(|&i| b.iter_at_offset(i).child_anchor().is_none())
        .collect();
    if plain.is_empty() {
        return;
    }
    // Not the user's edit: the field's text, for the app, is the same.
    crate::imp::updating(|| {
        for at in plain {
            let (mut from, mut to) = (b.iter_at_offset(at), b.iter_at_offset(at + 1));
            b.delete(&mut from, &mut to);
            b.create_child_anchor(&mut b.iter_at_offset(at));
        }
    });
}

/// The buffer's characters, a chip as its U+FFFC (unlike `text()`).
pub(crate) fn slice(b: &gtk::TextBuffer) -> String {
    b.slice(&b.start_iter(), &b.end_iter(), false).to_string()
}

/// Shows the runs: in place when the view holds their text.
fn show(rich: &mut Rich) {
    let Some(style) = rich.style.clone() else {
        return;
    };
    // The text the user edits is the value: no case transform.
    let style = Style {
        text_transform: TextTransform::None,
        ..style
    };
    let runs = decode_runs(&rich.runs);
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    let b = rich.view.buffer();
    if slice(&b) != text {
        // The user typed since: the next echo restyles it.
        return;
    }
    let styled = StyledText::from_runs(&text, &runs, &style);
    let dark = crate::imp::dark();
    let table = b.tag_table();
    for tag in rich.tags.drain(..) {
        table.remove(&tag);
    }
    let mut at = 0;
    for (s, run) in styled.runs.iter().zip(&runs) {
        let (start, len) = (at, run.text.chars().count() as i32);
        at += len;
        if len == 0 {
            continue;
        }
        if run.is_object() && len == 1 && b.iter_at_offset(start).char() == OBJECT_CHAR {
            chip_at(rich, &b, start, run, s, &style, dark);
            continue;
        }
        if let Some(tag) = tag(s, &style, dark) {
            table.add(&tag);
            b.apply_tag(&tag, &b.iter_at_offset(start), &b.iter_at_offset(at));
            rich.tags.push(tag);
        }
    }
    // Chips deleted with their text (Backspace, a new value).
    rich.chips.retain(|c| !c.anchor.is_deleted());
}

/// The tag of a text run: what differs from the field's own font and color
/// (its CSS rule), `None` when nothing does.
fn tag(s: &StyledRun, style: &Style, dark: bool) -> Option<gtk::TextTag> {
    let tag = gtk::TextTag::new(None);
    let mut any = false;
    if s.font.family != style.font_family
        && let Some(name) = crate::text::family_name(&s.font.family)
    {
        tag.set_family(Some(&name));
        any = true;
    }
    if s.font.weight != style.font_weight {
        tag.set_weight(s.font.weight.css() as i32);
        any = true;
    }
    if s.font.italic && !style.italic {
        tag.set_style(pango::Style::Italic);
        any = true;
    }
    if s.font.size != style.font_size && style.font_size > 0. {
        tag.set_scale((s.font.size / style.font_size) as f64);
        any = true;
    }
    if let Some(c) = s
        .color
        .filter(|_| s.color != style.text_color || s.link.is_some())
    {
        tag.set_foreground_rgba(Some(&rgba(c, dark)));
        any = true;
    }
    if let Some(c) = s.background {
        tag.set_background_rgba(Some(&rgba(c, dark)));
        any = true;
    }
    if s.underline {
        tag.set_underline(crate::text::underline(s.underline_style));
        if let Some(c) = s.underline_color {
            tag.set_underline_rgba(Some(&rgba(c, dark)));
        }
        any = true;
    }
    if s.strikethrough {
        tag.set_strikethrough(true);
        any = true;
    }
    any.then_some(tag)
}

fn rgba(c: Color, dark: bool) -> gdk::RGBA {
    let c = if dark {
        c.dark_variant()
    } else {
        c.light_variant()
    };
    gdk::RGBA::new(c.r, c.g, c.b, c.a)
}

/// The chip of `run` at character `at`: its anchor (a plain U+FFFC becomes
/// one) and its widget, updated in place.
fn chip_at(
    rich: &mut Rich,
    b: &gtk::TextBuffer,
    at: i32,
    run: &TextRun,
    s: &StyledRun,
    style: &Style,
    dark: bool,
) {
    let anchor = match b.iter_at_offset(at).child_anchor() {
        Some(a) => a,
        None => {
            // Not an edit to undo: the app's new value, whose `set_text`
            // started the history over already (an undo's characters are
            // anchors by now, `anchor_objects`).
            b.begin_irreversible_action();
            let (mut from, mut to) = (b.iter_at_offset(at), b.iter_at_offset(at + 1));
            b.delete(&mut from, &mut to);
            let a = b.create_child_anchor(&mut b.iter_at_offset(at));
            b.end_irreversible_action();
            a
        }
    };
    let i = match rich.chips.iter().position(|c| c.anchor == anchor) {
        Some(i) => i,
        None => {
            let c = new_chip(anchor);
            rich.view.add_child_at_anchor(&c.widget, &c.anchor);
            rich.chips.push(c);
            rich.chips.len() - 1
        }
    };
    configure(&rich.chips[i], run, s, style, dark);
    rich.chips[i].copied = copied(run).to_string();
}

/// What a chip run shows: its symbol's glyph, its label, and whether it is
/// boxed (a chip) or alone (an inline icon, its glyph as the label).
fn parts(run: &TextRun) -> (Option<&'static str>, &str, bool) {
    match &run.chip {
        Some(_) => (
            run.chip_icon.as_deref().map(prism_core::glyphs::glyph),
            run.chip_label.as_deref().unwrap_or(""),
            true,
        ),
        None => (
            None,
            run.icon.as_deref().map_or("", prism_core::glyphs::glyph),
            false,
        ),
    }
}

/// What a copy writes for a chip run: its label (an icon: nothing).
fn copied(run: &TextRun) -> &str {
    match (&run.chip, &run.chip_label) {
        (Some(_), Some(label)) => label,
        _ => "",
    }
}

fn new_chip(anchor: gtk::TextChildAnchor) -> Chip {
    let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    widget.add_css_class("prism-chip");
    widget.set_focusable(false);
    widget.set_can_focus(false);
    let icon = gtk::Label::new(None);
    let vector = gtk::DrawingArea::new();
    vector.set_valign(gtk::Align::Center);
    let paths: Rc<RefCell<Vec<prism_core::Path>>> = Rc::default();
    let p = paths.clone();
    vector.set_draw_func(move |_, cr, _, _| crate::imp::draw_paths(cr, &p.borrow()));
    let label = gtk::Label::new(None);
    label.set_xalign(0.);
    widget.append(&icon);
    widget.append(&vector);
    widget.append(&label);
    let css = gtk::CssProvider::new();
    #[allow(deprecated)]
    widget
        .style_context()
        .add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
    let link = Rc::new(Cell::new(None));
    // A click on a chip with `on_click`.
    let click = gtk::GestureClick::new();
    let l = link.clone();
    click.connect_released(move |g, _, _, _| {
        if let Some(link) = l.get() {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(w) = g.widget() {
                clicked(&w, link);
            }
        }
    });
    widget.add_controller(click);
    Chip {
        anchor,
        widget,
        icon,
        vector,
        paths,
        label,
        state: RefCell::new(None),
        below: Cell::new(0),
        css,
        link,
        copied: String::new(),
    }
}

/// Chip widget `w` with link `link` clicked: reported with the chip's box
/// in the field (its root overlay, #327).
fn clicked(w: &gtk::Widget, link: u64) {
    let rect = w
        .ancestor(gtk::Overlay::static_type())
        .and_then(|field| w.compute_bounds(&field));
    crate::imp::dispatch(match rect {
        Some(r) => UiEvent::RunClick(
            link,
            prism_core::Rect::new(r.x(), r.y(), r.width(), r.height()),
        ),
        None => UiEvent::MenuSelect(link),
    });
}

/// Clicks the shown chip labelled `label` that has `on_click` (the
/// `chip:` development hook): its window bounds, `None` without one.
pub(crate) fn click_chip(label: &str) -> Option<gtk::graphene::Rect> {
    let (w, link) = RICH.with(|r| {
        r.borrow().values().find_map(|rich| {
            rich.chips.iter().find_map(|c| {
                let link = c.link.get()?;
                (c.label.text() == label && c.widget.is_mapped())
                    .then(|| (c.widget.clone().upcast::<gtk::Widget>(), link))
            })
        })
    })?;
    let bounds = w.root().and_then(|root| w.compute_bounds(&root));
    clicked(&w, link);
    bounds
}

/// Shows `run` in `chip`: a rounded box with its icon and label (its
/// styled runs, cut with an ellipsis at the chip's largest width) in the
/// run's font, on the system's gray at 18% unless it sets a background,
/// with its ring, radius and height (#329); an inline icon is its glyph
/// alone. Its tool tip is the widget's, its state a `prism-chip-<state>`
/// class, its test id the widget's name (`gtk_widget_get_name`).
fn configure(chip: &Chip, run: &TextRun, s: &StyledRun, style: &Style, dark: bool) {
    let (icon, label, boxed) = parts(run);
    let attrs = font_attributes(s);
    chip.icon.set_attributes(Some(&attrs));
    chip.icon.set_text(icon.unwrap_or(""));
    chip.icon.set_visible(icon.is_some());
    let em = s.font.size.max(1.);
    let text = s.color.or(style.text_color).unwrap_or(Color::BLACK);
    // The label: its runs, each its weight, slant and color.
    let label_attrs = font_attributes(s);
    let mut shown = String::new();
    for l in run.label_runs() {
        let start = shown.len() as u32;
        shown += &l.text;
        let end = shown.len() as u32;
        let add = |mut a: pango::Attribute| {
            a.set_start_index(start);
            a.set_end_index(end);
            label_attrs.insert(a);
        };
        if let Some(w) = l.weight {
            let w: pango::Weight = unsafe { glib::translate::from_glib(w.css() as i32) };
            add(pango::AttrInt::new_weight(w).upcast());
        }
        if l.italic {
            add(pango::AttrInt::new_style(pango::Style::Italic).upcast());
        }
        if let Some(c) = l.color {
            let c = rgba(c, dark);
            let u = |v: f32| (v.clamp(0., 1.) * 65535.).round() as u16;
            add(pango::AttrColor::new_foreground(u(c.red()), u(c.green()), u(c.blue())).upcast());
            add(pango::AttrInt::new_foreground_alpha(u(c.alpha())).upcast());
        }
    }
    let shown = if boxed { shown } else { label.to_string() };
    chip.label.set_attributes(Some(&label_attrs));
    chip.label.set_text(&shown);
    chip.label.set_visible(!shown.is_empty());
    // Its box: the shared numbers (prism-native `chips`).
    chip.label.set_ellipsize(pango::EllipsizeMode::None);
    chip.label.set_max_width_chars(-1);
    chip.label.set_size_request(-1, -1);
    let (_, label_w, _, _) = chip.label.measure(gtk::Orientation::Horizontal, -1);
    let probe = if shown.is_empty() {
        &chip.icon
    } else {
        &chip.label
    };
    let (_, text_h, _, baseline) = probe.measure(gtk::Orientation::Vertical, -1);
    let geometry = prism_native::chips::chip_box(
        run,
        em,
        text_h as f32,
        if shown.is_empty() { 0. } else { label_w as f32 },
    );
    if boxed && geometry.cut {
        // Exactly its room, both its least and natural width.
        chip.label.set_ellipsize(pango::EllipsizeMode::End);
        chip.label.set_max_width_chars(0);
        chip.label
            .set_size_request(geometry.label_w.floor() as i32, -1);
    }
    let vector = boxed && run.chip_drawing.is_some();
    if vector {
        let (w, h) = (geometry.icon_w, geometry.icon_h);
        *chip.paths.borrow_mut() = prism_native::chips::icon_paths(run, w, h, text);
        chip.vector.set_content_width(w.round() as i32);
        chip.vector.set_content_height(h.round() as i32);
        chip.vector.queue_draw();
    }
    chip.vector.set_visible(vector);
    chip.widget
        .set_spacing(if boxed { geometry.gap as i32 } else { 0 });
    // GTK sets an anchored widget on the baseline: the box hangs below it
    // by its text's descent (and half of what it is taller than its
    // text), so its text sits on the line's baseline.
    let descent = if baseline >= 0 { text_h - baseline } else { 0 };
    let extra = if boxed {
        ((geometry.height - text_h as f32).max(0.) / 2.).round() as i32
    } else {
        0
    };
    chip.below.set(descent + extra);
    let mut css = format!("box{{margin-bottom:-{}px;", descent + extra);
    if boxed {
        let bg = s.background.map_or_else(
            || "alpha(#8e8e93,0.18)".to_string(),
            |c| crate::decor::color(c, dark),
        );
        css += &format!(
            "padding:0 {}px;border-radius:{}px;background-color:{bg};min-height:{}px;",
            geometry.pad, geometry.radius, geometry.height,
        );
        if let Some((w, c)) = run.chip_ring {
            css += &format!(
                "box-shadow:inset 0 0 0 {w}px {};",
                crate::decor::color(c, dark)
            );
        }
    }
    if let Some(c) = s.color.filter(|_| s.color != style.text_color) {
        css += &format!("color:{};", crate::decor::color(c, dark));
    }
    css.push('}');
    #[allow(deprecated)]
    chip.css.load_from_data(&css);
    chip.link.set(run.link);
    chip.widget
        .set_cursor_from_name(run.link.map(|_| "pointer"));
    chip.widget.set_tooltip_text(run.tooltip.as_deref());
    let state = run.chip_state.as_ref().map(|s| format!("prism-chip-{s}"));
    let mut had = chip.state.borrow_mut();
    if *had != state {
        if let Some(old) = had.as_deref() {
            chip.widget.remove_css_class(old);
        }
        if let Some(new) = state.as_deref() {
            chip.widget.add_css_class(new);
        }
        *had = state;
    }
    chip.widget
        .set_widget_name(run.chip_test_id.as_deref().unwrap_or(""));
    crate::imp::update_string(&chip.widget, crate::imp::a11y_property::LABEL, &shown);
}

/// Pango attributes for text in `s`'s font.
fn font_attributes(s: &StyledRun) -> pango::AttrList {
    let attrs = pango::AttrList::new();
    if let Some(name) = crate::text::family_name(&s.font.family) {
        attrs.insert(pango::AttrString::new_family(&name));
    }
    let w: pango::Weight = unsafe { glib::translate::from_glib(s.font.weight.css() as i32) };
    attrs.insert(pango::AttrInt::new_weight(w));
    if s.font.italic {
        attrs.insert(pango::AttrInt::new_style(pango::Style::Italic));
    }
    attrs.insert(pango::AttrSize::new_size_absolute(
        crate::text::pango_units(s.font.size),
    ));
    attrs
}

/// Characters `start..end` of rich input `id` as copied text: each chip as
/// its label, icons and other objects dropped.
pub(crate) fn labelled(id: NodeId, b: &gtk::TextBuffer, start: i32, end: i32) -> String {
    RICH.with(|r| {
        let r = r.borrow();
        let chips = r.get(&id).map(|r| &r.chips[..]).unwrap_or_default();
        let mut out = String::new();
        let mut it = b.iter_at_offset(start);
        while it.offset() < end && !it.is_end() {
            match it.child_anchor() {
                Some(a) => {
                    if let Some(c) = chips.iter().find(|c| c.anchor == a) {
                        out += &c.copied;
                    }
                }
                None if it.char() == OBJECT_CHAR => {}
                None => out.push(it.char()),
            }
            it.forward_char();
        }
        out
    })
}

/// Pango attributes of rich input `id`'s text (as `slice` gives it), to
/// measure its height: the runs' fonts and sizes, each chip its box.
pub(crate) fn attributes(id: NodeId) -> Option<pango::AttrList> {
    RICH.with(|r| {
        let r = r.borrow();
        let rich = r.get(&id)?;
        let style = rich.style.clone()?;
        let style = Style {
            text_transform: TextTransform::None,
            ..style
        };
        let runs = decode_runs(&rich.runs);
        let text: String = runs.iter().map(|r| r.text.as_str()).collect();
        let b = rich.view.buffer();
        if slice(&b) != text {
            return None;
        }
        let styled = StyledText::from_runs(&text, &runs, &style);
        let attrs = pango::AttrList::new();
        for s in &styled.runs {
            let (start, end) = (s.byte_start as u32, (s.byte_start + s.text.len()) as u32);
            let add = |mut a: pango::Attribute| {
                a.set_start_index(start);
                a.set_end_index(end);
                attrs.insert(a);
            };
            let char_at = text[..s.byte_start].chars().count() as i32;
            let anchor = b.iter_at_offset(char_at).child_anchor();
            if let Some(chip) = anchor.and_then(|a| rich.chips.iter().find(|c| c.anchor == a)) {
                // As GTK lays it out: its minimum size, on the baseline,
                // and what hangs below it.
                let (w, _, _, _) = chip.widget.measure(gtk::Orientation::Horizontal, -1);
                let (h, _, _, _) = chip.widget.measure(gtk::Orientation::Vertical, w);
                let below = chip.below.get();
                let rect = pango::Rectangle::new(
                    0,
                    -h * pango::SCALE,
                    w * pango::SCALE,
                    (h + below) * pango::SCALE,
                );
                add(pango::AttrShape::new(&rect, &rect).upcast());
                continue;
            }
            if let Some(name) = crate::text::family_name(&s.font.family) {
                add(pango::AttrString::new_family(&name).upcast());
            }
            if s.font.size != style.font_size && style.font_size > 0. {
                add(pango::AttrFloat::new_scale((s.font.size / style.font_size) as f64).upcast());
            }
            if s.font.weight != style.font_weight {
                let w: pango::Weight =
                    unsafe { glib::translate::from_glib(s.font.weight.css() as i32) };
                add(pango::AttrInt::new_weight(w).upcast());
            }
        }
        Some(attrs)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undoing_a_chips_deletion_keeps_the_history() {
        // GTK needs a display (Xvfb in the Linux image): skipped without.
        if gtk::init().is_err() {
            return;
        }
        let view = gtk::TextView::new();
        setup(1, &view);
        let b = view.buffer();
        b.set_text("ab");
        b.create_child_anchor(&mut b.iter_at_offset(1));
        assert_eq!(slice(&b), "a\u{fffc}b");
        // Backspace after the chip.
        b.place_cursor(&b.iter_at_offset(2));
        b.begin_user_action();
        let (mut from, mut to) = (b.iter_at_offset(1), b.iter_at_offset(2));
        b.delete_interactive(&mut from, &mut to, true);
        b.end_user_action();
        assert_eq!(slice(&b), "ab");
        b.undo();
        assert_eq!(slice(&b), "a\u{fffc}b");
        assert!(
            b.iter_at_offset(1).child_anchor().is_some(),
            "an anchor again, for the chip's widget"
        );
        assert!(b.can_redo(), "the redo survives");
        b.redo();
        assert_eq!(slice(&b), "ab");
        assert!(b.can_undo());
        b.undo();
        assert!(b.iter_at_offset(1).child_anchor().is_some());
        forget(1);
    }

    #[test]
    fn a_chip_shows_its_symbol_and_label_and_copies_its_label() {
        let runs = decode_runs(&prism_core::encode_runs(&[
            TextRun {
                text: "See ".into(),
                ..TextRun::default()
            },
            TextRun {
                text: OBJECT_CHAR.to_string(),
                chip: Some("page:1".into()),
                chip_label: Some("Roadmap".into()),
                chip_icon: Some("doc.text".into()),
                link: Some(7),
                ..TextRun::default()
            },
            TextRun {
                text: OBJECT_CHAR.to_string(),
                icon: Some("star.fill".into()),
                ..TextRun::default()
            },
        ]));
        let (icon, label, boxed) = parts(&runs[1]);
        assert!(boxed);
        assert_eq!(label, "Roadmap");
        assert_eq!(icon, Some(prism_core::glyphs::glyph("doc.text")));
        assert_eq!(copied(&runs[1]), "Roadmap");
        assert_eq!(runs[1].link, Some(7));
        // An inline icon: its glyph, unboxed, copied as nothing.
        let (icon, label, boxed) = parts(&runs[2]);
        assert!(!boxed);
        assert_eq!(icon, None);
        assert_eq!(label, prism_core::glyphs::glyph("star.fill"));
        assert_eq!(copied(&runs[2]), "");
        // One character each, like the buffer's anchors.
        let text: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text.chars().count(), 6);
    }
}
