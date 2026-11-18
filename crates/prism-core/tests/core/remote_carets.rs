//! Other people's carets and selections in text fields (#240): sent when
//! the app changes them, never with the field's value or selection, and
//! never in a secure field.

use prism_core::prelude::*;
use prism_core::{NodeKind, Patch, UiEvent};
use prism_testing::{Harness, mount};

struct Doc {
    text: String,
    carets: Vec<RemoteCaret>,
    secure: bool,
    kind: NodeKind,
}

impl Render for Doc {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let input = cx.listener(|this: &mut Doc, e: &InputEvent, _| this.text = e.value.clone());
        match self.kind {
            NodeKind::TextArea => text_area()
                .value(self.text.clone())
                .on_input(input)
                .remote_carets(self.carets.clone())
                .into_element(),
            NodeKind::RichInput => rich_input(vec![span(self.text.clone())])
                .remote_carets(self.carets.clone())
                .into_element(),
            _ => text_input()
                .value(self.text.clone())
                .secure(self.secure)
                .on_input(input)
                .remote_carets(self.carets.clone())
                .into_element(),
        }
    }
}

fn doc(kind: NodeKind, carets: Vec<RemoteCaret>, secure: bool) -> (Harness, prism_core::NodeId) {
    let h = Harness::new(move |cx| {
        let root = cx.new_entity(|_| Doc {
            text: "Hello, world".into(),
            carets: carets.clone(),
            secure,
            kind,
        });
        mount(cx, root);
    });
    let id = h.host.find(|n| n.kind == kind).unwrap();
    (h, id)
}

fn caret_patches(patches: &[Patch]) -> Vec<Vec<RemoteCaret>> {
    patches
        .iter()
        .filter_map(|p| match p {
            Patch::SetRemoteCarets { carets, .. } => Some(carets.clone()),
            _ => None,
        })
        .collect()
}

fn camille(at: usize) -> RemoteCaret {
    RemoteCaret::caret(at, rgb(0xE8590C)).label("Camille")
}

#[test]
fn carets_reach_every_kind_of_text_field_after_its_text() {
    for kind in [NodeKind::TextInput, NodeKind::TextArea, NodeKind::RichInput] {
        let (h, id) = doc(kind, vec![camille(5)], false);
        assert_eq!(h.host.get(id).remote_carets, [camille(5)], "{kind:?}");
    }
    let (mut h, _) = doc(NodeKind::TextArea, Vec::new(), false);
    let patches = h
        .update::<Doc>(|d| {
            d.text = "Hello there".into();
            d.carets = vec![camille(11)];
        })
        .to_vec();
    let text = patches
        .iter()
        .position(|p| matches!(p, Patch::SetText { .. }))
        .unwrap();
    let carets = patches
        .iter()
        .position(|p| matches!(p, Patch::SetRemoteCarets { .. }))
        .unwrap();
    assert!(text < carets, "carets are laid out on the new text");
}

#[test]
fn only_a_change_of_the_carets_is_sent_and_it_never_touches_the_field() {
    let (mut h, id) = doc(NodeKind::TextArea, vec![camille(5)], false);
    // The same carets rendered again: nothing.
    let patches = h.update::<Doc>(|_| {}).to_vec();
    assert_eq!(caret_patches(&patches), Vec::<Vec<RemoteCaret>>::new());
    // The user types: the value is theirs, the carets stay.
    let patches = h
        .dispatch(UiEvent::Input(id, "Hello, world!".into()))
        .to_vec();
    assert!(caret_patches(&patches).is_empty());
    // Another person moves, selects, goes idle: only the overlay changes.
    let noor = RemoteCaret::new(0, 5, rgb(0x1C7ED6)).label("Noor");
    let patches = h
        .update::<Doc>(|d| d.carets = vec![camille(7).idle(true), noor.clone()])
        .to_vec();
    assert_eq!(caret_patches(&patches), [vec![camille(7).idle(true), noor]]);
    assert!(
        patches
            .iter()
            .all(|p| matches!(p, Patch::SetRemoteCarets { .. })),
        "no text, value or selection: {patches:?}"
    );
    // Gone: an empty list.
    let patches = h.update::<Doc>(|d| d.carets.clear()).to_vec();
    assert_eq!(caret_patches(&patches), [Vec::new()]);
    assert!(h.host.get(id).remote_carets.is_empty());
}

#[test]
fn a_secure_field_shows_no_ones_caret() {
    let (mut h, id) = doc(NodeKind::TextInput, vec![camille(3)], true);
    assert!(h.host.get(id).remote_carets.is_empty());
    h.update::<Doc>(|d| d.secure = false);
    assert_eq!(h.host.get(id).remote_carets, [camille(3)]);
    h.update::<Doc>(|d| d.secure = true);
    assert!(h.host.get(id).remote_carets.is_empty());
}

#[test]
fn a_caret_knows_its_range() {
    let c = RemoteCaret::new(9, 3, rgb(0x1C7ED6));
    assert_eq!(c.range(), 3..9);
    assert!(!c.is_caret());
    assert_eq!(camille(4).range(), 4..4);
    assert!(camille(4).is_caret() && !camille(4).idle);
}
