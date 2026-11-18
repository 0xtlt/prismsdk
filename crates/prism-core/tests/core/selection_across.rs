//! One selection across several text fields (#231): a group extends it
//! from the fields' keys, draws it, and hands copies and edits to the app.

use prism_core::prelude::*;
use prism_core::{
    CaretEdges, ClipboardContent, CopyEvent, Modifiers, NodeId, NodeKind, Patch, Platform, Runtime,
    RuntimeOptions, UiEvent,
};
use prism_testing::{Harness, mount};

#[derive(Default)]
struct Doc {
    blocks: Vec<(String, String)>,
    seen: Vec<SelectionAcross>,
    edits: Vec<EditAcross>,
    copies: Vec<(String, Option<usize>, bool)>,
    handle_edits: bool,
    /// An edit merges the blocks into the first one (#265).
    merge: bool,
}

impl Render for Doc {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let flow = text_flow()
            .children(self.blocks.iter().map(|(id, text)| {
                rich_input([span(text.clone())])
                    .id(id.clone())
                    .on_copy(cx.listener(|this: &mut Doc, e: &CopyEvent, _| {
                        this.copies.push((
                            e.text.clone(),
                            e.across.as_ref().map(|a| a.fields.len()),
                            e.cut,
                        ));
                        if let Some(a) = &e.across {
                            e.write(ClipboardContent::text(a.text()).markdown("blocks"));
                        }
                    }))
            }))
            .on_selection_across(
                cx.listener(|this: &mut Doc, e: &SelectionAcross, _| this.seen.push(e.clone())),
            );
        if self.handle_edits {
            flow.on_edit_across(cx.listener(|this: &mut Doc, e: &EditAcross, cx| {
                this.edits.push(e.clone());
                if this.merge {
                    this.blocks.truncate(1);
                    cx.notify();
                }
            }))
        } else {
            flow
        }
    }
}

fn doc_on(platform: Platform, handle_edits: bool) -> (Harness, Vec<NodeId>) {
    let h = Harness::with_runtime(
        move |init| {
            Runtime::with_options(
                init,
                RuntimeOptions {
                    platform: Some(platform),
                    ..Default::default()
                },
            )
        },
        move |cx| {
            let root = cx.new_entity(|_| Doc {
                blocks: vec![
                    ("p1".into(), "Hello".into()),
                    ("p2".into(), "big".into()),
                    ("p3".into(), "world".into()),
                ],
                handle_edits,
                ..Default::default()
            });
            mount(cx, root);
        },
    );
    let mut fields: Vec<NodeId> = h
        .host
        .nodes
        .iter()
        .filter(|(_, n)| n.kind == NodeKind::RichInput)
        .map(|(id, _)| *id)
        .collect();
    fields.sort_unstable();
    (h, fields)
}

fn doc(handle_edits: bool) -> (Harness, Vec<NodeId>) {
    doc_on(Platform::Macos, handle_edits)
}

fn key(name: &str, shift: bool) -> KeyEvent {
    let mut k = KeyEvent::new(name);
    k.modifiers = Modifiers {
        shift,
        ..Default::default()
    };
    k
}

/// ⇧↓ with the caret at `at` (of `len`), on the last line.
fn shift_down(at: usize, len: usize) -> KeyEvent {
    key("ArrowDown", true)
        .with_selection(SelectionEvent::caret(at))
        .with_edges(CaretEdges::caret(at, len, 0, 1))
}

/// A key pressed in `field` as backends do: asked first, then dispatched.
fn press(h: &mut Harness, field: NodeId, key: KeyEvent) -> Vec<Patch> {
    h.rt.consumes_key_event(Some(field), &key);
    h.dispatch(UiEvent::Key(Some(field), key)).to_vec()
}

fn seen(h: &Harness) -> Vec<SelectionAcross> {
    let d = h.root::<Doc>();
    h.rt.app.read(&d).seen.clone()
}

#[test]
fn shift_down_at_the_end_of_a_field_extends_into_the_next() {
    let (mut h, f) = doc(true);
    // The group hears its fields' keys.
    assert!(h.host.get(f[0]).kind == NodeKind::RichInput);
    let down = shift_down(2, 5);
    assert!(h.rt.consumes_key_event(Some(f[0]), &down));
    // In the middle of a field, ⇧→ is the field's.
    let right = key("ArrowRight", true)
        .with_selection(SelectionEvent::caret(2))
        .with_edges(CaretEdges::caret(2, 5, 0, 1));
    assert!(!h.rt.consumes_key_event(Some(f[0]), &right));

    let patches = press(&mut h, f[0], down);
    // The first field keeps its part selected, the next is highlighted.
    assert!(patches.contains(&Patch::SetSelection {
        id: f[0],
        start: 2,
        end: 5
    }));
    assert_eq!(h.host.get(f[1]).highlight, Some((0, 3)));
    assert_eq!(h.host.get(f[2]).highlight, None);
    assert_eq!(h.host.across, Some(((f[0], 2), (f[1], 3))));
    let e = seen(&h).pop().unwrap();
    assert!(e.is_across());
    assert_eq!(e.anchor, SelectionPoint::new("p1", 2));
    assert_eq!(e.head, SelectionPoint::new("p2", 3));
    assert_eq!(e.text(), "llo\nbig");

    // ⇧→ goes on into the third field, one character at a time.
    press(&mut h, f[0], key("ArrowRight", true));
    press(&mut h, f[0], key("ArrowRight", true));
    assert_eq!(h.host.get(f[1]).highlight, Some((0, 3)));
    assert_eq!(h.host.get(f[2]).highlight, Some((0, 1)));
    assert_eq!(seen(&h).pop().unwrap().text(), "llo\nbig\nw");

    // ⇧↑ goes a field at a time: the start of the third, of the second,
    // then of the first: back inside it, given back to it.
    press(&mut h, f[0], key("ArrowUp", true));
    press(&mut h, f[0], key("ArrowUp", true));
    assert_eq!(h.host.get(f[2]).highlight, None);
    let patches = press(&mut h, f[0], key("ArrowUp", true));
    assert_eq!(h.host.get(f[1]).highlight, None);
    assert!(patches.contains(&Patch::SetSelection {
        id: f[0],
        start: 0,
        end: 2
    }));
    assert_eq!(h.host.across, None);
    assert!(!seen(&h).pop().unwrap().is_across());
}

#[test]
fn shift_up_at_the_start_extends_backwards() {
    let (mut h, f) = doc(true);
    let up = key("ArrowUp", true)
        .with_selection(SelectionEvent::caret(0))
        .with_edges(CaretEdges::caret(0, 5, 0, 1));
    press(&mut h, f[2], up);
    assert_eq!(h.host.get(f[1]).highlight, Some((0, 3)));
    let e = seen(&h).pop().unwrap();
    assert_eq!(e.start(), &SelectionPoint::new("p2", 0));
    assert_eq!(e.end(), &SelectionPoint::new("p3", 0));
}

#[test]
fn a_copy_across_fields_goes_to_the_first_fields_handler() {
    let (mut h, f) = doc(true);
    press(&mut h, f[0], shift_down(2, 5));
    let mut copy = KeyEvent::new("c");
    copy.modifiers.meta = true;
    assert!(h.rt.consumes_key_event(Some(f[0]), &copy));
    press(&mut h, f[0], copy);
    let d = h.root::<Doc>();
    assert_eq!(
        h.rt.app.read(&d).copies,
        [("llo\nbig".to_string(), Some(2), false)]
    );
    assert_eq!(
        h.host.clipboard,
        [(f[0], ClipboardContent::text("llo\nbig").markdown("blocks"))]
    );
    // A copy leaves the selection.
    assert!(h.host.across.is_some());

    // A cut: copied, then an edit removing the range.
    let mut cut = KeyEvent::new("x");
    cut.modifiers.meta = true;
    press(&mut h, f[0], cut);
    let edits = h.rt.app.read(&d).edits.clone();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].cause, EditCause::Cut);
    assert_eq!(edits[0].text, "");
    assert_eq!(h.host.across, None);
    assert_eq!(h.host.get(f[1]).highlight, None);
}

#[test]
fn typing_over_a_selection_across_fields_is_the_apps() {
    let (mut h, f) = doc(true);
    press(&mut h, f[0], shift_down(2, 5));
    let x = KeyEvent::new("x");
    assert!(h.rt.consumes_key_event(Some(f[0]), &x));
    press(&mut h, f[0], x);
    let d = h.root::<Doc>();
    let e = h.rt.app.read(&d).edits[0].clone();
    assert_eq!(e.cause, EditCause::Typing);
    assert_eq!(e.text, "x");
    assert_eq!(e.selection.fields.len(), 2);
    assert_eq!(e.selection.fields[0].start, 2);
    assert_eq!(h.host.get(f[1]).highlight, None);
}

#[test]
fn without_on_edit_across_a_key_goes_back_to_the_first_field() {
    let (mut h, f) = doc(false);
    press(&mut h, f[0], shift_down(2, 5));
    let patches = press(&mut h, f[0], KeyEvent::new("Backspace"));
    assert!(patches.contains(&Patch::SetSelection {
        id: f[0],
        start: 2,
        end: 5
    }));
    assert_eq!(h.host.get(f[1]).highlight, None);
    // Now the field's own: not taken.
    assert!(
        !h.rt
            .consumes_key_event(Some(f[0]), &KeyEvent::new("Backspace").with_selection(2..5))
    );
}

#[test]
fn an_arrow_or_a_new_selection_ends_it() {
    let (mut h, f) = doc(true);
    press(&mut h, f[0], shift_down(2, 5));
    // Prism's own selection echoed by the field: nothing changes.
    h.dispatch(UiEvent::Select(f[0], 2, 5));
    assert!(h.host.across.is_some());
    // → puts the caret at the end, in the second field.
    let patches = press(&mut h, f[0], key("ArrowRight", false));
    assert!(patches.contains(&Patch::Focus { id: f[1] }));
    assert!(patches.contains(&Patch::SetSelection {
        id: f[1],
        start: 3,
        end: 3
    }));
    assert_eq!(h.host.across, None);

    // A click elsewhere in a field.
    press(&mut h, f[0], shift_down(2, 5));
    h.dispatch(UiEvent::Select(f[2], 1, 1));
    assert_eq!(h.host.get(f[1]).highlight, None);
    let e = seen(&h).pop().unwrap();
    assert_eq!(e.anchor, SelectionPoint::new("p3", 1));
    assert!(e.is_collapsed());
}

#[test]
fn the_app_sets_and_clears_a_selection_across_fields() {
    let (mut h, f) = doc(true);
    h.rt.app.set_selection_across(("p1", 1), ("p3", 2));
    let patches = h.render().to_vec();
    assert!(patches.contains(&Patch::Focus { id: f[0] }));
    assert!(patches.contains(&Patch::SetSelection {
        id: f[0],
        start: 1,
        end: 5
    }));
    assert_eq!(h.host.get(f[1]).highlight, Some((0, 3)));
    assert_eq!(h.host.get(f[2]).highlight, Some((0, 2)));
    // Not reported: the app set it.
    assert!(seen(&h).is_empty());

    h.rt.app.clear_selection_across();
    h.render();
    assert_eq!(h.host.get(f[1]).highlight, None);
    assert_eq!(h.host.get(f[2]).highlight, None);
    assert_eq!(h.host.across, None);
}

#[test]
fn on_the_web_the_client_extends_it() {
    let (mut h, f) = doc_on(Platform::Web, true);
    // The client does it: the runtime takes no key.
    assert!(!h.rt.consumes_key_event(Some(f[0]), &shift_down(2, 5)));
    let group = h
        .host
        .nodes
        .iter()
        .find(|(_, n)| n.children.contains(&f[0]))
        .map(|(id, _)| *id)
        .unwrap();
    let patches = h
        .dispatch(UiEvent::SelectionAcross(
            group,
            Some(((f[0], 2), (f[2], 1))),
        ))
        .to_vec();
    // Already drawn by the client.
    assert!(patches.is_empty());
    assert_eq!(seen(&h).pop().unwrap().text(), "llo\nbig\nw");

    h.dispatch(UiEvent::CopyAcross(group, false));
    assert_eq!(h.host.clipboard[0].1.markdown_text(), Some("blocks"));
    h.dispatch(UiEvent::EditAcross(
        group,
        EditCause::Paste,
        "pasted".into(),
    ));
    let d = h.root::<Doc>();
    let e = h.rt.app.read(&d).edits[0].clone();
    assert_eq!((e.cause, e.text.as_str()), (EditCause::Paste, "pasted"));
    assert_eq!(e.selection.fields.len(), 3);
}

#[test]
fn a_key_the_backend_left_to_the_field_does_not_act() {
    let (mut h, f) = doc(true);
    press(&mut h, f[0], shift_down(2, 5));
    // iOS lets typed letters reach the field without asking: not the group's.
    h.dispatch(UiEvent::Key(Some(f[0]), KeyEvent::new("y")));
    let d = h.root::<Doc>();
    assert!(h.rt.app.read(&d).edits.is_empty());
    assert!(h.host.across.is_some());
}

/// #265: an edit that merges the fields destroys them in the re-render; the
/// answers aimed at them (their highlight, the selection) are dropped
/// instead of reaching destroyed nodes.
#[test]
fn answers_skip_the_fields_an_edit_destroyed() {
    let (mut h, f) = doc(true);
    let d = h.root::<Doc>();
    h.rt.app.update(&d, |doc, _| doc.merge = true);
    press(&mut h, f[0], shift_down(2, 5));
    // Typing over the selection: the app merges, the host must not panic.
    let patches = press(&mut h, f[0], KeyEvent::new("x"));
    let gone: Vec<NodeId> = patches
        .iter()
        .filter_map(|p| match p {
            Patch::Destroy { id } => Some(*id),
            _ => None,
        })
        .collect();
    assert!(gone.contains(&f[1]) && gone.contains(&f[2]), "{patches:?}");
    for p in &patches {
        if let Patch::SetSelectionHighlight { id, .. } | Patch::SetSelection { id, .. } = p {
            assert!(!gone.contains(id), "{p:?}");
        }
    }
    assert_eq!(h.rt.app.read(&d).blocks.len(), 1);
}

/// #264: a native backend reports a mouse drag across fields; the runtime
/// selects the anchor field's part, highlights the others and tells the app.
#[test]
fn a_native_drag_across_fields_is_drawn_by_the_runtime() {
    let (mut h, f) = doc(true);
    let group = h
        .host
        .nodes
        .iter()
        .find(|(_, n)| n.children.contains(&f[0]))
        .map(|(id, _)| *id)
        .unwrap();
    let patches = h
        .dispatch(UiEvent::SelectionAcross(
            group,
            Some(((f[0], 2), (f[2], 3))),
        ))
        .to_vec();
    assert!(patches.contains(&Patch::SetSelection {
        id: f[0],
        start: 2,
        end: 5
    }));
    assert_eq!(h.host.get(f[1]).highlight, Some((0, 3)));
    assert_eq!(h.host.get(f[2]).highlight, Some((0, 3)));
    let e = seen(&h).pop().unwrap();
    assert_eq!(e.fields.len(), 3);
    // Back inside the first field: one field again, highlights gone.
    h.dispatch(UiEvent::SelectionAcross(
        group,
        Some(((f[0], 2), (f[0], 4))),
    ));
    assert_eq!(h.host.get(f[1]).highlight, None);
    assert_eq!(h.host.get(f[2]).highlight, None);
}
