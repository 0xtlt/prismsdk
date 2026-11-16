//! Text field selection and caret (#86): `on_select`, `.selection(..)`
//! applied when the app changes it (echoes of the user's own selection
//! leave the field alone), and `KeyEvent::selection`.

use prism_core::prelude::*;
use prism_core::{NodeKind, Patch, UiEvent, byte_to_char, char_to_byte};
use prism_testing::{Harness, mount};

/// A block editor: one line, its selection, split on Enter at the caret.
struct Editor {
    blocks: Vec<String>,
    sel: SelectionEvent,
    /// Stores the selection the user makes (the usual controlled field).
    tracks: bool,
    selects: Vec<(usize, usize)>,
}

impl Render for Editor {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        text_input()
            .value(self.blocks.last().cloned().unwrap_or_default())
            .on_input(cx.listener(|this: &mut Editor, e: &InputEvent, _| {
                *this.blocks.last_mut().unwrap() = e.value.clone();
            }))
            .on_select(cx.listener(|this: &mut Editor, e: &SelectionEvent, _| {
                this.selects.push((e.start, e.end));
                if this.tracks {
                    this.sel = *e;
                }
            }))
            .selection(self.sel)
            .on_key_down(cx.listener(|this: &mut Editor, e: &KeyEvent, _| {
                // Enter splits the block at the caret.
                if e.is("Enter")
                    && let Some(sel) = e.selection
                {
                    let text = this.blocks.pop().unwrap();
                    let r = sel.byte_range(&text);
                    this.blocks.push(text[..r.start].to_string());
                    this.blocks.push(text[r.end..].to_string());
                    this.sel = SelectionEvent::caret(0);
                }
            }))
    }
}

fn editor(text: &str, tracks: bool) -> (Harness, prism_core::NodeId) {
    let text = text.to_string();
    let h = Harness::new(move |cx| {
        let root = cx.new_entity(|_| Editor {
            blocks: vec![text.clone()],
            sel: SelectionEvent::default(),
            tracks,
            selects: Vec::new(),
        });
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == NodeKind::TextInput).unwrap();
    (h, id)
}

fn selection_patches(patches: &[Patch]) -> Vec<(usize, usize)> {
    patches
        .iter()
        .filter_map(|p| match p {
            Patch::SetSelection { start, end, .. } => Some((*start, *end)),
            _ => None,
        })
        .collect()
}

#[test]
fn on_select_subscribes_the_field_and_receives_ordered_characters() {
    let (mut h, id) = editor("héllo", true);
    assert!(h.host.get(id).select_events, "SetSelectEvents on");
    assert_eq!(h.host.get(id).selection, Some((0, 0)));
    // A selection made backwards arrives in order.
    h.dispatch(UiEvent::Select(id, 4, 1));
    let editor = h.root::<Editor>();
    let selects = h.rt.app.read(&editor).selects.clone();
    assert_eq!(selects, [(1, 4)]);
}

#[test]
fn the_users_selection_rendered_back_is_an_echo() {
    let (mut h, id) = editor("hello world", true);
    for (s, e) in [(5, 5), (6, 11), (11, 11)] {
        let patches = h.dispatch(UiEvent::Select(id, s, e)).to_vec();
        assert_eq!(
            selection_patches(&patches),
            [],
            "{s}..{e} is the field's own"
        );
    }
    // The app changes it: applied, after the text.
    let patches = h
        .update::<Editor>(|e| {
            e.blocks = vec!["**hello** world".into()];
            e.sel = (2..7).into();
        })
        .to_vec();
    assert_eq!(selection_patches(&patches), [(2, 7)]);
    let text = patches
        .iter()
        .position(|p| matches!(p, Patch::SetText { .. }))
        .unwrap();
    let sel = patches
        .iter()
        .position(|p| matches!(p, Patch::SetSelection { .. }))
        .unwrap();
    assert!(text < sel, "the selection applies to the new text");
    assert_eq!(h.host.get(id).selection, Some((2, 7)));
    // The user selects again, then the app rewrites the text and keeps the
    // same range: the text moved under it, so it is applied.
    h.dispatch(UiEvent::Select(id, 2, 7));
    let patches = h
        .update::<Editor>(|e| e.blocks = vec!["__hello__ world".into()])
        .to_vec();
    assert_eq!(selection_patches(&patches), [], "the range did not change");
    let patches = h.update::<Editor>(|e| e.sel = (0..5).into()).to_vec();
    assert_eq!(selection_patches(&patches), [(0, 5)]);
}

#[test]
fn an_unchanged_app_selection_is_not_applied_again() {
    // The app sets a caret once and does not follow the user's moves.
    let (mut h, id) = editor("hello", false);
    let patches = h
        .update::<Editor>(|e| e.sel = SelectionEvent::caret(5))
        .to_vec();
    assert_eq!(selection_patches(&patches), [(5, 5)]);
    h.dispatch(UiEvent::Select(id, 1, 1));
    // Anything else re-renders: the caret the user moved stays.
    let patches = h
        .update::<Editor>(|e| e.blocks = vec!["hello!".into()])
        .to_vec();
    assert_eq!(selection_patches(&patches), []);
    // Setting it to where the field reported it is sends nothing either.
    h.dispatch(UiEvent::Select(id, 1, 1));
    let patches = h
        .update::<Editor>(|e| e.sel = SelectionEvent::caret(1))
        .to_vec();
    assert_eq!(selection_patches(&patches), []);
    let patches = h.update::<Editor>(|e| e.sel = (0..6).into()).to_vec();
    assert_eq!(selection_patches(&patches), [(0, 6)]);
}

#[test]
fn key_events_carry_the_caret_so_enter_splits_the_block() {
    let (mut h, id) = editor("hello wörld", true);
    let key = KeyEvent::new("Enter").with_selection(SelectionEvent::caret(6));
    h.dispatch(UiEvent::Key(Some(id), key));
    let editor = h.root::<Editor>();
    let blocks = h.rt.app.read(&editor).blocks.clone();
    assert_eq!(blocks, ["hello ", "wörld"]);
    assert_eq!(h.host.get(id).text, "wörld");
    // Without a field focused (or a platform that cannot tell): `None`.
    assert_eq!(KeyEvent::new("Enter").selection, None);
}

#[test]
fn removing_on_select_unsubscribes() {
    struct Plain(bool);
    impl Render for Plain {
        fn render(&mut self, _: &mut Context<Self>) -> impl IntoElement {
            text_area().when(self.0, |t| {
                t.on_select(std::rc::Rc::new(|_: &SelectionEvent, _: &mut App| {}))
            })
        }
    }
    let mut h = Harness::new(|cx| {
        let root = cx.new_entity(|_| Plain(true));
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == NodeKind::TextArea).unwrap();
    assert!(h.host.get(id).select_events);
    assert_eq!(h.host.get(id).selection, None, "no `.selection`: never set");
    h.update::<Plain>(|p| p.0 = false);
    assert!(!h.host.get(id).select_events);
}

#[test]
fn character_and_byte_offsets() {
    let t = "a😀é b";
    assert_eq!(char_to_byte(t, 0), 0);
    assert_eq!(char_to_byte(t, 1), 1);
    assert_eq!(char_to_byte(t, 2), 5);
    assert_eq!(char_to_byte(t, 3), 7);
    assert_eq!(char_to_byte(t, 99), t.len(), "clamped");
    assert_eq!(byte_to_char(t, 5), 2);
    assert_eq!(byte_to_char(t, 3), 2, "inside 😀: after it");
    assert_eq!(byte_to_char(t, 99), 5);
    let e = SelectionEvent::new(3, 1);
    assert_eq!((e.start, e.end), (1, 3));
    assert_eq!(&t[e.byte_range(t)], "😀é");
    assert!(!e.is_caret() && SelectionEvent::caret(2).is_caret());
    assert_eq!(e.range(), 1..3);
}
