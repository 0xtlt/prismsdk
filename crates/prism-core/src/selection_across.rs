//! One text selection across several fields (#231): a document made of
//! blocks, one [`rich_input`](crate::rich_input) or
//! [`text_area`](crate::text_area) per paragraph, selected like one text.
//!
//! [`text_flow()`] (or any container with
//! [`selection_group`](crate::Div::selection_group)) groups the named text
//! fields inside it (named with `.id(…)`), in the order they appear, into one
//! selection space:
//!
//! - **Extending**: ⇧↓ or ⇧→ at the end of a field, ⇧↑ or ⇧← at its
//!   start, carry the selection into the next or previous field; then ⇧→
//!   and ⇧← move it a character at a time (the step between two fields is
//!   one character, like a line break), ⇧↓ and ⇧↑ a field at a time (to the
//!   end of the next one, the start of the previous one). A mouse drag
//!   from one field into another does it too: the web client follows it,
//!   native backends report it (`UiEvent::SelectionAcross`). Touch selection
//!   handles stay inside one field (the system's, per text view).
//! - **Drawing**: the field where the selection started keeps its own
//!   selection; the others draw their part highlighted (whole for the
//!   fields in between).
//! - **Reporting**: [`on_selection_across`](crate::Div::on_selection_across)
//!   hears every change ([`SelectionAcross`]), including the one that
//!   brings it back inside a single field.
//! - **Copying**: ⌘C / ⌘X (Ctrl elsewhere) reach the
//!   [`on_copy`](crate::RichInput::on_copy) / `on_cut` of the field where
//!   the selection started, with [`CopyEvent::across`](crate::CopyEvent::across)
//!   set: the app writes its own content for the whole range. Without a
//!   handler, the fields' texts joined by line breaks are copied.
//! - **Editing**: typing (keys, an on-screen keyboard or an input method:
//!   [`Runtime::takes_text_across`](crate::Runtime::takes_text_across)),
//!   Backspace, Delete, Enter, a cut or a paste over a
//!   selection across fields is not applied by any field: it goes to
//!   [`on_edit_across`](crate::Div::on_edit_across) ([`EditAcross`]), and
//!   the app rewrites its blocks (merge the first and last, drop those in
//!   between). Without that handler, such a key first brings the selection
//!   back into the field where it started, which then acts on it.
//! - **From the app**: [`App::set_selection_across`](crate::App::set_selection_across)
//!   and [`App::clear_selection_across`](crate::App::clear_selection_across).
//!
//! Offsets are characters of each field's value, like
//! [`SelectionEvent`](crate::SelectionEvent).

use crate::Handler;
use crate::Styled;
use crate::events::KeyEvent;

/// A position in a field of a selection group (#231): the field's name
/// (its `.id(…)`) and a character offset in its value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SelectionPoint {
    /// The field's name (`.id(…)`).
    pub field: String,
    /// A character offset in its value.
    pub offset: usize,
}

impl SelectionPoint {
    /// Character `offset` of the field named `field`.
    pub fn new(field: impl Into<String>, offset: usize) -> Self {
        SelectionPoint {
            field: field.into(),
            offset,
        }
    }
}

impl<S: Into<String>> From<(S, usize)> for SelectionPoint {
    fn from((field, offset): (S, usize)) -> Self {
        SelectionPoint::new(field, offset)
    }
}

/// The part of one field inside a [`SelectionAcross`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldSelection {
    /// The field's name (`.id(…)`).
    pub field: String,
    /// First selected character.
    pub start: usize,
    /// End of its part (excluded): its length for every field but the
    /// last.
    pub end: usize,
    /// The selected text of that field (a chip as its label).
    pub text: String,
}

/// A selection in a group of text fields (#231), from where it started
/// (`anchor`) to where it was extended (`head`), which can come before it.
/// [`fields`](Self::fields) lists the part of every field it covers, in
/// document order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SelectionAcross {
    /// Where the selection started.
    pub anchor: SelectionPoint,
    /// Where it was extended to (the moving end).
    pub head: SelectionPoint,
    /// The part of each field it covers, first to last.
    pub fields: Vec<FieldSelection>,
}

impl SelectionAcross {
    /// A selection from `anchor` to `head`, without its fields' parts (the
    /// runtime fills them in the events it sends).
    pub fn new(anchor: impl Into<SelectionPoint>, head: impl Into<SelectionPoint>) -> Self {
        SelectionAcross {
            anchor: anchor.into(),
            head: head.into(),
            fields: Vec::new(),
        }
    }

    /// Whether it spans more than one field. `false` once the user brought
    /// it back inside one field (or collapsed it to a caret).
    pub fn is_across(&self) -> bool {
        self.anchor.field != self.head.field
    }

    /// Whether nothing is selected (a caret).
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.head
    }

    fn order(&self, p: &SelectionPoint) -> Option<(usize, usize)> {
        let i = self.fields.iter().position(|f| f.field == p.field)?;
        Some((i, p.offset))
    }

    /// The first of the two ends, in document order.
    pub fn start(&self) -> &SelectionPoint {
        match (self.order(&self.anchor), self.order(&self.head)) {
            (Some(a), Some(h)) if h < a => &self.head,
            _ => &self.anchor,
        }
    }

    /// The last of the two ends, in document order.
    pub fn end(&self) -> &SelectionPoint {
        match (self.order(&self.anchor), self.order(&self.head)) {
            (Some(a), Some(h)) if h < a => &self.anchor,
            _ => &self.head,
        }
    }

    /// The selected text: each field's part, joined by line breaks.
    pub fn text(&self) -> String {
        self.fields
            .iter()
            .map(|f| f.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What made an [`EditAcross`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EditCause {
    /// A typed character ([`EditAcross::text`]).
    Typing,
    /// Backspace.
    Backspace,
    /// Delete (forward).
    Delete,
    /// Return / Enter: split at the selection.
    Enter,
    /// A cut (⌘X), after the copy went to `on_cut`.
    Cut,
    /// A paste (⌘V) of [`EditAcross::text`].
    Paste,
}

impl EditCause {
    /// The name hosts send (`"typing"`, `"backspace"`…).
    pub fn name(self) -> &'static str {
        match self {
            EditCause::Typing => "typing",
            EditCause::Backspace => "backspace",
            EditCause::Delete => "delete",
            EditCause::Enter => "enter",
            EditCause::Cut => "cut",
            EditCause::Paste => "paste",
        }
    }

    /// Parses [`name`](Self::name).
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "typing" => EditCause::Typing,
            "backspace" => EditCause::Backspace,
            "delete" => EditCause::Delete,
            "enter" => EditCause::Enter,
            "cut" => EditCause::Cut,
            "paste" => EditCause::Paste,
            _ => return None,
        })
    }
}

/// An edit over a selection across fields (#231), which no field applies:
/// the app replaces the selected range with [`text`](Self::text) in its
/// own model (the first field keeps its start, the last its end, the
/// fields in between go), then renders the blocks and puts the caret
/// where it wants (`.selection(…)`, `cx.focus(…)`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EditAcross {
    /// The selection being replaced.
    pub selection: SelectionAcross,
    /// What replaces it: the typed character, the pasted text, `"\n"` for
    /// Enter, empty for Backspace, Delete and a cut.
    pub text: String,
    /// What made the edit.
    pub cause: EditCause,
}

impl EditAcross {
    /// An edit replacing `selection` with `text`.
    pub fn new(selection: SelectionAcross, text: impl Into<String>, cause: EditCause) -> Self {
        EditAcross {
            selection,
            text: text.into(),
            cause,
        }
    }
}

/// The handlers of a selection group.
#[derive(Clone, Default)]
pub struct GroupHandlers {
    /// [`Div::on_selection_across`](crate::Div::on_selection_across).
    pub on_selection: Option<Handler<SelectionAcross>>,
    /// [`Div::on_edit_across`](crate::Div::on_edit_across).
    pub on_edit: Option<Handler<EditAcross>>,
}

impl std::fmt::Debug for GroupHandlers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GroupHandlers")
            .field("on_selection", &self.on_selection.is_some())
            .field("on_edit", &self.on_edit.is_some())
            .finish()
    }
}

/// A container whose text fields share one selection (#231): a
/// [`div()`](crate::div) laid out as a column, with
/// [`selection_group`](crate::Div::selection_group).
///
/// ```ignore
/// text_flow()
///     .children(blocks.iter().map(|b| rich_input(b.spans()).id(b.id.clone())))
///     .on_selection_across(cx.listener(|this, e: &SelectionAcross, _| this.range = e.clone()))
///     .on_edit_across(cx.listener(|this, e: &EditAcross, cx| this.replace(e, cx)))
/// ```
pub fn text_flow() -> crate::Div {
    crate::div().flex_col().selection_group()
}

// ---------------------------------------------------------------------------
// The selection logic, shared by every backend (the web client mirrors it).
// ---------------------------------------------------------------------------

/// A field of a group, in document order, as the runtime knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Field<Id> {
    pub id: Id,
    pub name: String,
    /// Its length in characters.
    pub len: usize,
}

/// A position as (field index, offset).
pub(crate) type Pos = (usize, usize);

/// What a key does to a selection group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyAction {
    /// Select from `anchor` to `head` (the same field: a selection inside
    /// it, given back to the field).
    Select(Pos, Pos),
    /// End the selection with a caret at this position.
    Caret(Pos),
    /// Copy (or cut) the selection.
    Copy { cut: bool },
    /// Paste over the selection.
    Paste,
    /// Replace the selection.
    Edit(EditCause, String),
}

/// Each field's part of the selection `anchor`..`head`, by index:
/// `(index, start, end)`, first to last.
pub(crate) fn parts<Id>(
    fields: &[Field<Id>],
    anchor: Pos,
    head: Pos,
) -> Vec<(usize, usize, usize)> {
    let (lo, hi) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    (lo.0..=hi.0.min(fields.len().saturating_sub(1)))
        .map(|i| {
            let len = fields[i].len;
            let start = if i == lo.0 { lo.1.min(len) } else { 0 };
            let end = if i == hi.0 { hi.1.min(len) } else { len };
            (i, start, end)
        })
        .collect()
}

fn arrow(key: &str) -> Option<i8> {
    // -2 up, -1 left, 1 right, 2 down.
    Some(match key {
        "ArrowUp" => -2,
        "ArrowLeft" => -1,
        "ArrowRight" => 1,
        "ArrowDown" => 2,
        _ => return None,
    })
}

/// Where `head` goes for a ⇧-arrow (`dir` from [`arrow`]).
fn step<Id>(fields: &[Field<Id>], (i, off): Pos, dir: i8) -> Pos {
    let len = fields[i].len;
    match dir {
        1 if off < len => (i, off + 1),
        1 if i + 1 < fields.len() => (i + 1, 0),
        -1 if off > 0 => (i, off - 1),
        -1 if i > 0 => (i - 1, fields[i - 1].len),
        2 if off < len => (i, len),
        2 if i + 1 < fields.len() => (i + 1, fields[i + 1].len),
        -2 if off > 0 => (i, 0),
        -2 if i > 0 => (i - 1, 0),
        _ => (i, off),
    }
}

/// What `key`, pressed in field `focused` (its index), does to the group:
/// `across` is the selection across fields shown now, if any. `None`: the
/// field handles the key as usual.
pub(crate) fn key_action<Id>(
    fields: &[Field<Id>],
    across: Option<(Pos, Pos)>,
    focused: usize,
    key: &KeyEvent,
    edits_handled: bool,
) -> Option<KeyAction> {
    let m = key.modifiers;
    let command = m.meta || m.control;
    if let Some((anchor, head)) = across {
        if let Some(dir) = arrow(&key.key) {
            if command || m.alt {
                return None;
            }
            if m.shift {
                return Some(KeyAction::Select(anchor, step(fields, head, dir)));
            }
            let (lo, hi) = if anchor <= head {
                (anchor, head)
            } else {
                (head, anchor)
            };
            return Some(KeyAction::Caret(if dir < 0 { lo } else { hi }));
        }
        if key.key == "Escape" {
            return Some(KeyAction::Caret(head));
        }
        if command {
            return match key.key.to_ascii_lowercase().as_str() {
                "c" => Some(KeyAction::Copy { cut: false }),
                "x" => Some(KeyAction::Copy { cut: true }),
                "v" if edits_handled => Some(KeyAction::Paste),
                // Back into the field it started in, which pastes.
                "v" => Some(KeyAction::Select(
                    anchor,
                    anchor_part_end(fields, anchor, head),
                )),
                _ => None,
            };
        }
        let edit = match key.key.as_str() {
            "Backspace" => Some((EditCause::Backspace, String::new())),
            "Delete" => Some((EditCause::Delete, String::new())),
            "Enter" => Some((EditCause::Enter, "\n".to_string())),
            k if key.is_character() && !m.alt => Some((EditCause::Typing, k.to_string())),
            _ => None,
        };
        return edit.map(|(cause, text)| {
            if edits_handled {
                KeyAction::Edit(cause, text)
            } else {
                KeyAction::Select(anchor, anchor_part_end(fields, anchor, head))
            }
        });
    }
    // No selection across fields yet: a ⇧-arrow at the field's edge starts
    // one.
    let dir = arrow(&key.key)?;
    if !m.shift || command || m.alt {
        return None;
    }
    let sel = key.selection?;
    let len = fields.get(focused)?.len;
    let edges = key.edges.unwrap_or_default();
    let selected = sel.start != sel.end;
    match dir {
        2 | 1 if focused + 1 < fields.len() => {
            let at_end = if dir == 2 {
                edges.last_line || (selected && sel.end >= len)
            } else {
                edges.end || sel.end >= len
            };
            at_end.then(|| {
                let head = if dir == 2 {
                    (focused + 1, fields[focused + 1].len)
                } else {
                    (focused + 1, 0)
                };
                KeyAction::Select((focused, sel.start), head)
            })
        }
        -2 | -1 if focused > 0 => {
            let at_start = if dir == -2 {
                edges.first_line || (selected && sel.start == 0)
            } else {
                edges.start || sel.start == 0
            };
            at_start.then(|| {
                let head = if dir == -2 {
                    (focused - 1, 0)
                } else {
                    (focused - 1, fields[focused - 1].len)
                };
                KeyAction::Select((focused, sel.end), head)
            })
        }
        _ => None,
    }
}

/// The far end of the anchor field's part: where a selection brought back
/// into it ends.
fn anchor_part_end<Id>(fields: &[Field<Id>], anchor: Pos, head: Pos) -> Pos {
    if head > anchor {
        (anchor.0, fields[anchor.0].len)
    } else {
        (anchor.0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SelectionEvent;
    use crate::events::{CaretEdges, Modifiers};

    fn fields() -> Vec<Field<u64>> {
        [("a", 5), ("b", 3), ("c", 4)]
            .into_iter()
            .enumerate()
            .map(|(i, (n, len))| Field {
                id: i as u64,
                name: n.into(),
                len,
            })
            .collect()
    }

    fn shift(key: &str) -> KeyEvent {
        let mut k = KeyEvent::new(key);
        k.modifiers = Modifiers {
            shift: true,
            ..Default::default()
        };
        k
    }

    #[test]
    fn parts_cover_the_fields_between() {
        let f = fields();
        assert_eq!(parts(&f, (0, 2), (2, 1)), [(0, 2, 5), (1, 0, 3), (2, 0, 1)]);
        // Backwards: the same parts.
        assert_eq!(parts(&f, (2, 1), (0, 2)), [(0, 2, 5), (1, 0, 3), (2, 0, 1)]);
        assert_eq!(parts(&f, (1, 1), (1, 2)), [(1, 1, 2)]);
    }

    #[test]
    fn shift_arrows_at_an_edge_start_a_selection() {
        let f = fields();
        // ⇧↓ on the last line with a caret at 2: into the next field.
        let k = shift("ArrowDown")
            .with_selection(SelectionEvent::caret(2))
            .with_edges(CaretEdges::caret(2, 5, 0, 1));
        assert_eq!(
            key_action(&f, None, 0, &k, true),
            Some(KeyAction::Select((0, 2), (1, 3)))
        );
        // ⇧→ in the middle: the field's own.
        let k = shift("ArrowRight")
            .with_selection(SelectionEvent::caret(2))
            .with_edges(CaretEdges::caret(2, 5, 0, 1));
        assert_eq!(key_action(&f, None, 0, &k, true), None);
        // ⇧→ with the selection reaching the end.
        let k = shift("ArrowRight").with_selection(SelectionEvent::new(1, 5));
        assert_eq!(
            key_action(&f, None, 0, &k, true),
            Some(KeyAction::Select((0, 1), (1, 0)))
        );
        // ⇧↑ at the start of the second field.
        let k = shift("ArrowUp")
            .with_selection(SelectionEvent::caret(0))
            .with_edges(CaretEdges::caret(0, 3, 0, 1));
        assert_eq!(
            key_action(&f, None, 1, &k, true),
            Some(KeyAction::Select((1, 0), (0, 0)))
        );
        // ⇧↓ in the last field: nowhere to go.
        let k = shift("ArrowDown")
            .with_selection(SelectionEvent::caret(4))
            .with_edges(CaretEdges::caret(4, 4, 0, 1));
        assert_eq!(key_action(&f, None, 2, &k, true), None);
        // Without ⇧: the field's.
        let k = KeyEvent::new("ArrowDown")
            .with_selection(SelectionEvent::caret(5))
            .with_edges(CaretEdges::caret(5, 5, 0, 1));
        assert_eq!(key_action(&f, None, 0, &k, true), None);
    }

    #[test]
    fn keys_move_and_end_a_selection_across() {
        let f = fields();
        let across = Some(((0, 2), (1, 0)));
        let act = |k: &KeyEvent, handled| key_action(&f, across, 0, k, handled);
        assert_eq!(
            act(&shift("ArrowRight"), true),
            Some(KeyAction::Select((0, 2), (1, 1)))
        );
        assert_eq!(
            act(&shift("ArrowLeft"), true),
            Some(KeyAction::Select((0, 2), (0, 5)))
        );
        assert_eq!(
            act(&shift("ArrowDown"), true),
            Some(KeyAction::Select((0, 2), (1, 3)))
        );
        assert_eq!(
            act(&KeyEvent::new("ArrowLeft"), true),
            Some(KeyAction::Caret((0, 2)))
        );
        assert_eq!(
            act(&KeyEvent::new("ArrowDown"), true),
            Some(KeyAction::Caret((1, 0)))
        );
        assert_eq!(
            act(&KeyEvent::new("x"), true),
            Some(KeyAction::Edit(EditCause::Typing, "x".into()))
        );
        assert_eq!(
            act(&KeyEvent::new("Backspace"), true),
            Some(KeyAction::Edit(EditCause::Backspace, String::new()))
        );
        // Without on_edit_across: back into the first field, which acts.
        assert_eq!(
            act(&KeyEvent::new("Backspace"), false),
            Some(KeyAction::Select((0, 2), (0, 5)))
        );
        let mut copy = KeyEvent::new("c");
        copy.modifiers.meta = true;
        assert_eq!(act(&copy, true), Some(KeyAction::Copy { cut: false }));
        copy.key = "x".into();
        assert_eq!(act(&copy, true), Some(KeyAction::Copy { cut: true }));
        copy.key = "a".into();
        assert_eq!(act(&copy, true), None);
        assert_eq!(act(&KeyEvent::new("Tab"), true), None);
    }

    #[test]
    fn selection_across_ends() {
        let mut s = SelectionAcross::new(("b", 1), ("a", 3));
        s.fields = vec![
            FieldSelection {
                field: "a".into(),
                start: 3,
                end: 5,
                text: "lo".into(),
            },
            FieldSelection {
                field: "b".into(),
                start: 0,
                end: 1,
                text: "w".into(),
            },
        ];
        assert!(s.is_across());
        assert_eq!(s.start(), &SelectionPoint::new("a", 3));
        assert_eq!(s.end(), &SelectionPoint::new("b", 1));
        assert_eq!(s.text(), "lo\nw");
        assert_eq!(EditCause::from_name("paste"), Some(EditCause::Paste));
    }
}
