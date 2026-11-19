//! Turns entity renders into a minimal list of [`Patch`]es.
//!
//! Backends never see elements or closures: they only receive patches and
//! report events by [`NodeId`]. That boundary is what lets the same app run on
//! the DOM (over a WebSocket), AppKit, UIKit, or any future host.
//!
//! ## Entity-granular updates
//!
//! Every entity renders exactly one root node (fragments are wrapped in a
//! `div`), and the reconciler remembers which node each entity owns. When
//! entities are notified, only those are re-rendered, in place, shallowest
//! first. A clean child entity met during a parent's re-render is not rendered
//! or diffed at all: its mounted subtree is moved over as-is.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::a11y::{Accessibility, Role};
use crate::app::{AnyEntity, App, Appearance, EntityId};
use crate::element::{
    BackEvent, ChangeEvent, ClickEvent, Element, Handler, InputEvent, NavigateEvent, NodeKind,
    RichInputEvent, ScrollEvent, SelectionEvent, SwipeDirection, VNode,
};
use crate::style::Style;

/// Identifies a host node in patches and events.
pub type NodeId = u64;

/// What backends need to know about the environment to render.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RenderEnvironment {
    /// BCP 47 tag (`lang` attribute on the web).
    pub lang: String,
    /// Layout direction (right to left mirrors layouts).
    pub direction: crate::i18n::Direction,
    /// Text size multiplier (1.0 = default size).
    pub text_scale: f32,
    /// Animations are skipped.
    pub reduce_motion: bool,
    /// Header buttons drawn by backends (web, macOS) in the app's language.
    pub back: String,
    /// Label of the button closing a modal.
    pub close: String,
    /// Label of the action emptying an optional date picker (#89).
    pub clear: String,
}

/// A mutation of the host tree.
#[derive(Clone, Debug, PartialEq)]
// Styles are the big variant; patches are short-lived.
#[allow(clippy::large_enum_variant)]
pub enum Patch {
    /// Which swipe directions a node listens to.
    SetSwipe {
        /// The node.
        id: NodeId,
        /// It listens to swipes to the left.
        left: bool,
        /// It listens to swipes to the right.
        right: bool,
    },
    /// Whether a scroll view reports scroll events.
    SetScrollEvents {
        /// The scroll view.
        id: NodeId,
        /// Report scroll events.
        on: bool,
    },
    /// Context menu of a node (`None` removes it).
    SetMenu {
        /// The node.
        id: NodeId,
        /// The menu to show.
        menu: Option<crate::menu::MenuSpec>,
    },
    /// Language, layout direction, text scale and motion settings.
    SetEnvironment {
        /// The new settings.
        env: RenderEnvironment,
    },
    /// Light/dark appearance of native chrome.
    SetAppearance {
        /// The new appearance.
        appearance: Appearance,
    },
    /// Toggle state (0/1), selected tab index.
    SetValue {
        /// The node.
        id: NodeId,
        /// The new value.
        value: f64,
    },
    /// Control configuration (see [`crate::controls`]): a slider's range,
    /// a picker's options, a date picker's mode, a text input's keyboard.
    /// Sent before the node's value.
    SetConfig {
        /// The node whose config changes.
        id: NodeId,
        /// The encoded config (see [`crate::controls`]).
        config: String,
    },
    /// Icon name (SF Symbol on Apple platforms).
    SetIcon {
        /// The node.
        id: NodeId,
        /// The icon name.
        icon: String,
    },
    /// The effective accessibility description (defaults already applied).
    SetAccessibility {
        /// The node.
        id: NodeId,
        /// The full description.
        a11y: Accessibility,
    },
    /// Create a detached node.
    Create {
        /// The new node's id.
        id: NodeId,
        /// What host view to create.
        kind: NodeKind,
    },
    /// The node's complete style (layout and appearance).
    SetStyle {
        /// The node.
        id: NodeId,
        /// The new style.
        style: Style,
    },
    /// Text content, button label, or text input value.
    SetText {
        /// The node.
        id: NodeId,
        /// The new text.
        text: String,
    },
    /// Select `start..end` (characters) in a text input or area (#86):
    /// the app's `.selection(..)` changed. Sent after the node's text.
    SetSelection {
        /// The text input or area.
        id: NodeId,
        /// First selected character.
        start: usize,
        /// End of the selection (excluded), `start` for a caret.
        end: usize,
    },
    /// Other people's carets and selections in a text field (#240): the
    /// app's `.remote_carets(..)` changed (empty: none). Only the overlay
    /// changes; the field's value, caret and selection are left alone.
    SetRemoteCarets {
        /// The text input, area or rich input.
        id: NodeId,
        /// The carets, in characters.
        carets: Vec<crate::RemoteCaret>,
    },
    /// Whether a text input or area reports its selection
    /// (`UiEvent::Select`): it has an `on_select` handler (#86).
    SetSelectEvents {
        /// The text input or area.
        id: NodeId,
        /// Report selection changes.
        on: bool,
    },
    /// Placeholder of a text input.
    SetPlaceholder {
        /// The text input.
        id: NodeId,
        /// The new placeholder.
        text: String,
    },
    /// Whether a `View` reacts to clicks (buttons always do).
    SetClickable {
        /// The view.
        id: NodeId,
        /// It reacts to clicks.
        clickable: bool,
    },
    /// Replace the ordered child list. Children not listed are detached.
    SetChildren {
        /// The parent.
        id: NodeId,
        /// Its children, in order.
        children: Vec<NodeId>,
    },
    /// Mount this node as the root of the window/page.
    SetRoot {
        /// The new root.
        id: NodeId,
    },
    /// The node and its native resources can be freed.
    Destroy {
        /// The destroyed node.
        id: NodeId,
    },
    /// What a 3D scene shows: the whole scene the first time (and when the
    /// node is created again), then only what changed.
    SetScene {
        /// The scene node.
        id: NodeId,
        /// The change.
        update: crate::scene3d::SceneUpdate,
    },

    // --- Pointer, keys, focus, layers (#17, #19, #25) ---
    /// Which pointer events a view reports (bits of
    /// [`pointer_mask`](crate::events::pointer_mask): hover, move, drag).
    SetPointerEvents {
        /// The view.
        id: NodeId,
        /// `HOVER | MOVE | DRAG` bits; 0 reports none.
        mask: u8,
    },
    /// Whether files can be dropped on a view (#111): the types it accepts
    /// (MIME types or extensions, empty for any file), `None` for none.
    /// Backends report drops as [`UiEvent::DropFiles`] and the
    /// [`DropHover`](crate::Interaction::DropHover) state while accepted
    /// files are over it.
    SetDropTarget {
        /// The view.
        id: NodeId,
        /// Accepted types, `None`: not a drop target.
        types: Option<Vec<String>>,
    },
    /// Drag and drop between elements (#251): what kind of payload a view
    /// carries when dragged ([`Div::draggable`](crate::Div::draggable); it
    /// also gets the `DRAG` pointer bit) and the kinds it takes dropped.
    /// Backends report drags of such a view as [`UiEvent::PayloadDrag`]
    /// with the innermost accepting target under the pointer, show the
    /// preview, set [`DropHover`](crate::Interaction::DropHover) on the
    /// target and scroll a scroll view whose edge the pointer stays near.
    SetDragAndDrop {
        /// The view.
        id: NodeId,
        /// The kind it carries, `None`: not draggable.
        kind: Option<String>,
        /// What follows the pointer while it is dragged.
        preview: crate::events::DragPreview,
        /// The kinds dropped on it that it takes (empty: none).
        accepts: Vec<String>,
    },
    /// Whether a node reports key presses: `local` while it or something
    /// in it has the focus (a view can then take the focus), `global`
    /// whatever has the focus.
    SetKeyEvents {
        /// The node.
        id: NodeId,
        /// Keys while the focus is in the node.
        local: bool,
        /// Keys anywhere in the window.
        global: bool,
    },
    /// Whether a node reports its frame after layout (`on_layout`,
    /// `track_layout`, #192): backends send `UiEvent::Layout` when it moves
    /// or resizes.
    SetLayoutEvents {
        /// The node.
        id: NodeId,
        /// Reports its frame.
        on: bool,
    },
    /// Keys the node's `on_key_down` takes from the focused control in it
    /// (`consume_keys`, #181): backends keep the control (and the page)
    /// from acting on them. Native backends can also ask
    /// [`Runtime::consumes_key`](crate::Runtime::consumes_key).
    SetConsumedKeys {
        /// The node.
        id: NodeId,
        /// The keys, as `KeyEvent::key` names them (empty: none).
        keys: Vec<String>,
    },
    /// How long a touch must be held still before the node's drag starts
    /// (`drag_after_long_press`, #243), in milliseconds; 0 starts it on
    /// movement, and mice always do. Sent when it changes, after
    /// [`SetPointerEvents`](Patch::SetPointerEvents).
    SetDragDelay {
        /// The node.
        id: NodeId,
        /// The hold, in milliseconds.
        ms: u32,
    },
    /// Give the node the keyboard focus (`cx.focus`, `.autofocus()`). Sent
    /// after the node is mounted.
    Focus {
        /// The node.
        id: NodeId,
    },
    /// Take the keyboard focus away from whatever has it and hide the
    /// on-screen keyboard (`cx.dismiss_keyboard`).
    DismissKeyboard,
    /// Open a pop-up menu (#247): over the node (the window's content when
    /// `None`), at `at` in its coordinates, or below it when `at` is `None`.
    /// Items report `UiEvent::MenuSelect(id)`.
    ShowMenu {
        /// The node it opens over; `None`: the window.
        id: Option<NodeId>,
        /// The menu.
        menu: crate::menu::MenuSpec,
        /// Where, in points in the node; `None`: below it.
        at: Option<crate::events::Point>,
    },
    /// Scroll the scroll views around the node so it is visible
    /// (`cx.scroll_into_view`).
    ScrollIntoView {
        /// The node.
        id: NodeId,
    },
    /// Scroll a scroll view (`cx.scroll_by`, `cx.scroll_to`, #193): to
    /// (`x`, `y`) in its content, or by them from its offset when
    /// `relative`, kept within its content.
    ScrollTo {
        /// The scroll view.
        id: NodeId,
        /// Horizontal offset (or distance).
        x: f32,
        /// Vertical offset (or distance).
        y: f32,
        /// By `x`, `y` from where it is.
        relative: bool,
        /// Animated (`scroll_to` only).
        animated: bool,
    },
    /// Nodes the runtime mounts above the root, in order (the toaster):
    /// each covers the window like the root, above everything else
    /// (sheets and dialogs included), and lets pointer input through
    /// wherever it has no child.
    SetLayers {
        /// The layer roots, bottom first.
        ids: Vec<NodeId>,
    },

    // --- Islands: client-side Web Components ---
    /// The node is an island (see [`island`](crate::island())): sent right
    /// after its `Create`, and again when its source or events change.
    /// Native backends ignore it (they show the node's children, the
    /// fallback); the web turns the node into the custom element.
    SetIsland {
        /// The island's node (a `View`).
        id: NodeId,
        /// Its custom element's tag (`prism-island-chart`).
        tag: String,
        /// Where the element is defined.
        source: crate::island::IslandSource,
        /// The events it reports (`UiEvent::Island`).
        events: Vec<String>,
    },
    /// Props of an island that changed (name, JSON), `null` for a removed one.
    SetIslandProps {
        /// The island.
        id: NodeId,
        /// The changed props.
        props: Vec<(String, String)>,
    },

    // --- Links ---
    /// Where a node made a link (`Div::link`, `Text::link`) leads: an app
    /// path or a URL. Sent right after its `Create`, and again when the
    /// destination changes (`None`: no longer a link; on the web, a node
    /// that gains or loses its link is a new node instead). Activating the
    /// node is still a [`UiEvent::Click`]; native backends ignore this
    /// patch, the web renders the node as an `<a href>` (open in a new tab,
    /// copy the link, a working link before the script runs).
    SetLink {
        /// The node (a `View` or a `Text`).
        id: NodeId,
        /// Its destination, as given to `.link(…)`.
        href: Option<String>,
    },

    // --- App-defined attributes (#85) ---
    /// The node's app-defined attributes (`.attr(…)`), all of them: a name
    /// sent before and absent now is removed. Only names
    /// [`is_app_attribute`](crate::attributes::is_app_attribute) accepts.
    /// The web sets them on the element; native backends ignore them.
    SetAttributes {
        /// The node.
        id: NodeId,
        /// (name, value) pairs, in the order the app first set them.
        attributes: Vec<(String, String)>,
    },

    // --- Web views (#62) ---
    /// Deliver a message to a web view's page
    /// ([`App::post_web_message`](crate::App::post_web_message)): a
    /// `message` event on its `window`, if it shows an allowed origin.
    /// Messages sent before the page finished loading wait for it.
    PostWebMessage {
        /// The web view.
        id: NodeId,
        /// The message, as JSON (checked: valid JSON).
        data: String,
    },
    /// Reload a web view's page
    /// ([`App::reload_web_view`](crate::App::reload_web_view)).
    ReloadWebView {
        /// The web view.
        id: NodeId,
    },
    // --- Editable rich text (#115) ---
    /// The runs of a [`RichInput`](NodeKind::RichInput)
    /// ([`encode_runs`](crate::encode_runs)), sent after its plain text
    /// ([`SetText`](Patch::SetText)) whenever they change. Like a text, a
    /// value equal to what the user just typed is an echo the field leaves
    /// alone (#73). Backends without rich editing ignore it and edit the
    /// plain text.
    SetRichValue {
        /// The rich input.
        id: NodeId,
        /// Its runs, encoded.
        runs: String,
    },
    // --- Rich input editing hooks (#217) ---
    /// Which editing events a rich input reports: bits of
    /// [`edit_events`](crate::edit_events) (selection rectangle, copy,
    /// cut, paste, modifier-clicks on spans). A copy, cut or paste it
    /// reports is the app's: the backend writes and inserts nothing itself
    /// (a cut still deletes the selection).
    SetEditEvents {
        /// The rich input.
        id: NodeId,
        /// `edit_events` bits; 0 reports none.
        mask: u8,
    },
    /// Write `content` to the clipboard, every type of it as one item
    /// (#217): the answer to a copy or a cut the field reported.
    WriteClipboard {
        /// The field copied from.
        id: NodeId,
        /// What to write.
        content: crate::ClipboardContent,
    },
    /// Insert `text` over the field's selection as if the user had pasted
    /// it as plain text (#217): a paste the app left to the field. The
    /// field then reports the edit (`Input` / `RichInput`) and the caret
    /// like any other.
    InsertText {
        /// The field.
        id: NodeId,
        /// The text to insert.
        text: String,
    },
    // --- One selection across several fields (#231) ---
    /// Whether a node is a selection group: its named text fields share
    /// one selection. The web client extends it across them itself.
    SetSelectionGroup {
        /// The container.
        id: NodeId,
        /// It is a group.
        on: bool,
        /// It has `on_edit_across`: edits over a selection across its
        /// fields are reported, not left to the field where it started.
        edits: bool,
    },
    /// Draw `start..end` (characters) of a text field highlighted like a
    /// selection, without selecting it: its part of a selection that spans
    /// several fields of a group while another field has the focus.
    /// `start == end` removes the highlight.
    SetSelectionHighlight {
        /// The text field.
        id: NodeId,
        /// First highlighted character.
        start: usize,
        /// End of the highlight (excluded).
        end: usize,
    },
    /// The selection across the fields of group `group` changed from the
    /// app or the runtime (not from the backend's own report): from
    /// `anchor` to `head` (field, character), `None` when it ended. The
    /// web client keeps extending it from there; native backends draw it
    /// from `SetSelection` and `SetSelectionHighlight` alone.
    SetSelectionAcross {
        /// The selection group.
        group: NodeId,
        /// Where it starts (field, offset) and where it was extended to.
        range: Option<((NodeId, usize), (NodeId, usize))>,
    },
}

/// An event reported by a backend.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// A button or clickable view was activated.
    Click(NodeId),
    /// A text input was edited: (id, new text). A rich input on a backend
    /// that edits plain text reports this too: its runs follow the text
    /// (#115).
    Input(NodeId, String),
    /// A rich input was edited (#115): (id, its runs as shown,
    /// [`encode_runs`](crate::encode_runs) without link ids), each with the
    /// style of the run it came from.
    RichInput(NodeId, String),
    /// The selection of a text input or area moved: (id, start, end) in
    /// characters (#86).
    Select(NodeId, usize, usize),
    /// A stack was popped natively down to `depth` screens.
    Back(NodeId, usize),
    /// A stack was asked to show `path` (e.g. browser forward).
    Navigate(NodeId, String),
    /// A toggle was flipped (0/1) or a tab selected (index).
    Change(NodeId, f64),
    /// A menu item was chosen (context menu, tray, app menu).
    MenuSelect(u64),
    /// An alert button was chosen: (alert id, button index).
    AlertAction(u64, usize),
    /// A scroll view moved: (id, content offset, viewport height).
    Scroll(NodeId, f32, f32),
    /// A horizontal swipe on a node.
    Swipe(NodeId, SwipeDirection),
    /// The return key was pressed in a text input.
    Submit(NodeId),

    // --- Pointer and keys (#17) ---
    /// A clickable view was clicked or tapped at a position (in the view).
    ClickAt(NodeId, crate::events::Point),
    /// The pointer entered, moved over or left a view that listens to it.
    Pointer(NodeId, crate::events::PointerEvent),
    /// A drag on a view that listens to drags.
    Drag(NodeId, crate::events::DragEvent),
    /// A drag of a [`draggable`](crate::Div::draggable) view (#251), with
    /// the drop target the backend found under the pointer (`None`: over
    /// nothing that takes its kind). A plain [`Drag`](Self::Drag) of such
    /// a view is one over nothing.
    PayloadDrag(
        NodeId,
        crate::events::DragEvent,
        Option<crate::events::DropPoint>,
    ),
    /// The user closed a sheet, popover or dialog, and how (#156). Like
    /// `Back(id, 0)` with the reason.
    Dismiss(NodeId, crate::element::DismissReason),
    /// Wheel or trackpad scrolling over a view that listens to it (#133).
    Wheel(NodeId, crate::events::WheelEvent),
    /// A click with the middle, back or forward button (#215), for
    /// `on_aux_click`; `button` says which.
    AuxClick(NodeId, crate::element::ClickEvent),
    /// A node took (`true`) or lost the keyboard focus (#224).
    FocusChange(NodeId, bool),
    /// Files dropped on a target are uploading (#199): bytes sent, total.
    DropProgress(NodeId, u64, u64),
    /// A video's playback reached the end (#207).
    Ended(NodeId),
    /// A node that reports its layout was laid out at a new frame (#192).
    Layout(NodeId, crate::events::Rect),
    /// A pinch over a view that listens to it (#133).
    Magnify(NodeId, crate::events::MagnifyEvent),
    /// A key press: the node with the focus (or `None` if nothing Prism
    /// knows has it). Handlers of that node and its ancestors run, then
    /// global ones.
    Key(Option<NodeId>, crate::events::KeyEvent),
    /// A key released, routed like [`Key`](Self::Key) (#159).
    KeyUp(Option<NodeId>, crate::events::KeyEvent),
    /// A click with its keys and count (#159), where the backend knows
    /// them; like [`ClickAt`](Self::ClickAt) otherwise.
    ClickWith(NodeId, crate::element::ClickEvent),

    // --- Islands ---
    /// An island emitted an event: (id, event name, detail as JSON).
    Island(NodeId, String, String),

    // --- 3D ---
    /// A 3D scene was tapped (not dragged): what was hit.
    SceneTap(NodeId, crate::scene3d::SceneTapEvent),
    // --- Web views (#62) ---
    /// A web view's page posted a message: (id, message as JSON, origin of
    /// the page). Backends report only messages from allowed origins.
    WebMessage(NodeId, String, String),
    /// A web view finished loading a page: (id, its URL).
    WebLoad(NodeId, String),
    // --- Drop targets (#111) ---
    /// Files were dropped on a drop target (natively their copies the app
    /// can read; on the web the uploaded files). The runtime keeps those
    /// the target accepts.
    DropFiles(NodeId, Vec<crate::PickedFile>),
    // --- Rich input editing hooks (#217) ---
    /// Where a rich input's caret or selection is: (id, start, end in
    /// characters, its rectangle in the field's coordinates).
    SelectionRect(NodeId, usize, usize, crate::events::Rect),
    /// The user copied or cut (`true`) in a rich input that reports it:
    /// (id, start, end in characters, the selection's plain text). The
    /// runtime answers with [`Patch::WriteClipboard`].
    Copy(NodeId, bool, usize, usize, String),
    /// The user pasted in a rich input that reports it: what the
    /// clipboard holds. The runtime answers with [`Patch::InsertText`]
    /// unless the app took the paste.
    Paste(NodeId, crate::PasteEvent),
    /// A click with ⌘, Ctrl or ⌥ on character `offset` of a rich input
    /// being edited: (id, offset, the keys held).
    SpanClick(NodeId, usize, crate::events::Modifiers),
    /// A click in a rich input (#266), its `text_offset` set: for its
    /// [`on_click`](crate::RichInput::on_click).
    FieldClick(NodeId, crate::element::ClickEvent),
    // --- One selection across several fields (#231) ---
    /// A backend extended (or ended, `None`) the selection across the
    /// fields of group `.0`: from (field, offset) to (field, offset). The
    /// web client already draws it; native backends (a mouse drag, #264)
    /// get `SetSelection` and `SetSelectionHighlight` back.
    SelectionAcross(NodeId, Option<((NodeId, usize), (NodeId, usize))>),
    /// The web client took an edit over the selection across group `.0`'s
    /// fields: what made it and the text replacing the range.
    EditAcross(NodeId, crate::EditCause, String),
    /// The web client took a copy (or cut, `true`) of the selection across
    /// group `.0`'s fields, already writing its plain text.
    CopyAcross(NodeId, bool),
    /// Text typed into field `.0` over a selection across its group's
    /// fields (#231) by an input method or a soft keyboard, that the field
    /// did not insert because [`Runtime::takes_text_across`](crate::Runtime::takes_text_across)
    /// said the group takes it: `""` for a deletion, `"\n"` for Return.
    TypeAcross(NodeId, String),
}

/// A selection across the fields of a group (#231), as drawn.
#[derive(Clone, Debug, Default)]
struct Across {
    group: NodeId,
    anchor: (NodeId, usize),
    head: (NodeId, usize),
    /// The highlights sent, by field.
    painted: HashMap<NodeId, (usize, usize)>,
    /// The selection set in the anchor field (to tell its echo).
    selected: (usize, usize),
}

/// What a key did to a selection group (#231), for the runtime to finish:
/// the patches to send and the handlers to call.
#[derive(Default)]
pub(crate) struct AcrossOutcome {
    pub patches: Vec<Patch>,
    /// The group's selection handler and the selection to report.
    pub report: Option<(Handler<crate::SelectionAcross>, crate::SelectionAcross)>,
    /// An edit for `on_edit_across`.
    pub edit: Option<(Handler<crate::EditAcross>, crate::EditAcross)>,
    /// A copy (or cut) to answer: the field it came from, its handler,
    /// the event.
    pub copy: Option<(NodeId, Option<Handler<crate::CopyEvent>>, crate::CopyEvent)>,
    /// A paste to read: the edit handler and the selection it replaces.
    pub paste: Option<(Handler<crate::EditAcross>, crate::SelectionAcross)>,
}

/// A mounted host node.
struct MNode {
    kind: NodeKind,
    key: Option<String>,
    /// The entity whose render produced this node as its root.
    entity: Option<AnyEntity>,
    style: Style,
    text: String,
    placeholder: String,
    icon: String,
    value: Option<f64>,
    config: String,
    a11y: Accessibility,
    menu: Option<crate::menu::MenuSpec>,
    swipe: (bool, bool),
    scroll_events: bool,
    clickable: bool,
    /// A clickable view named by its descendants' text (no label of its own).
    auto_label: bool,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// Pointer events reported (`pointer_mask` bits).
    pointer_mask: u8,
    /// Touch hold before a drag, in milliseconds (#243).
    drag_delay_ms: u32,
    /// Key events reported: (local, global).
    keys: (bool, bool),
    /// Keys taken from the focused control in it (#181).
    consume_keys: Vec<String>,
    /// It reports its frame (#192).
    layout_events: bool,
    /// The editing events it reports (#217, `edit_events` bits).
    edit_mask: u8,
    /// It is a selection group (#231), and has `on_edit_across`.
    selection_group: (bool, bool),
    /// The selection the app last rendered (`.selection`, #86).
    selection: Option<SelectionEvent>,
    /// The selection the field shows, as far as Prism knows: the last one
    /// sent to it or reported by it.
    shown_selection: Option<SelectionEvent>,
    /// It reports its selection (`on_select`).
    select_events: bool,
    /// Other people's carets, as last sent (#240).
    remote_carets: Vec<crate::RemoteCaret>,
    /// Its `.id(…)`.
    name: Option<String>,
    /// An island's tag, source, props and events.
    island: Option<MIsland>,
    /// Its `.link(…)` destination, as last sent (`Patch::SetLink`).
    link: Option<String>,
    /// Its own style animates changes (`.animate(…)`, `.transition(…)`;
    /// not an animated update's).
    animates: bool,
    /// A 3D scene as backends last got it (diffs are made against it; its
    /// blobs stay alive with it).
    scene: Option<crate::scene3d::FlatScene>,
    /// Its app-defined attributes (`.attr(…)`).
    attributes: Vec<(String, String)>,
    /// A rich input's encoded runs, as last sent (#115).
    runs: String,
    /// The file types it accepts dropped (#111), as last sent.
    drop_types: Option<Vec<String>>,
    /// The payload kind it carries and its preview, and the kinds it takes
    /// (#251), as last sent.
    dnd: (Option<(String, crate::events::DragPreview)>, Vec<String>),
}

/// What backends last got about a mounted island.
struct MIsland {
    tag: String,
    source: crate::island::IslandSource,
    props: HashMap<String, String>,
    events: Vec<String>,
}

/// A rich input's spans as last rendered (#115), with the style each run
/// was sent with: edits the backend reports are rebuilt from them.
struct RichState {
    spans: Vec<(crate::spans::TextRun, crate::spans::Span)>,
    font_size: f32,
}

/// What an edit of a rich input's plain text is (#115).
pub enum RichEdit {
    /// The new runs.
    Spans(RichInputEvent),
    /// Enter, in a backend's plain multi-line field: a lone line break was
    /// typed and the input has an `on_submit`. The field gets the app's
    /// value back.
    Submit,
}

/// A flattened child, ready to diff.
enum Item {
    Node(Box<VNode>, Option<AnyEntity>),
    /// A clean entity: reuse its mounted subtree.
    Cached(AnyEntity),
}

/// What says a new child is a mounted one: its key, or its entity.
enum Ident<'a> {
    Key(&'a str),
    Entity(EntityId),
}

impl Ident<'_> {
    fn matches(&self, m: &MNode) -> bool {
        match self {
            Ident::Key(key) => m.key.as_deref() == Some(*key),
            Ident::Entity(e) => m.entity.is_some_and(|o| o.id() == *e),
        }
    }
}

/// A node's children before its re-render, taken one by one by the new
/// children: the first free one with the same [`Ident`], else the one at
/// the same place.
struct Slots {
    slots: Vec<Option<NodeId>>,
    /// The first free slot: the next match while the order holds.
    first: usize,
    /// The first slot of each key (by hash) and of each entity, built on
    /// the first lookup in a long list that `first` does not answer.
    index: Option<(HashMap<u64, usize>, HashMap<EntityId, usize>)>,
}

impl Slots {
    /// Up to this many children, a scan is as fast as an index.
    const SCAN: usize = 16;

    fn new(children: &[NodeId]) -> Self {
        Slots {
            slots: children.iter().copied().map(Some).collect(),
            first: 0,
            index: None,
        }
    }

    /// Takes the first free slot whose node is `ident`.
    fn take(&mut self, nodes: &HashMap<NodeId, MNode>, ident: &Ident) -> Option<NodeId> {
        let is = |n: NodeId| nodes.get(&n).is_some_and(|m| ident.matches(m));
        if self
            .slots
            .get(self.first)
            .copied()
            .flatten()
            .is_some_and(is)
        {
            return self.take_at(self.first, |_| true);
        }
        if self.slots.len() > Self::SCAN {
            let (keys, entities) = self
                .index
                .get_or_insert_with(|| Self::index(&self.slots, nodes));
            // Not indexed: no free slot had it.
            let j = match ident {
                Ident::Key(key) => *keys.get(&key_hash(key))?,
                Ident::Entity(e) => *entities.get(e)?,
            };
            if self.slots[j].is_some_and(is) {
                return self.take_at(j, |_| true);
            }
            // Taken already (a duplicate key or entity), or another key
            // with the same hash: look through the rest.
        }
        let j = (self.first..self.slots.len()).find(|&j| self.slots[j].is_some_and(is))?;
        self.take_at(j, |_| true)
    }

    /// Takes slot `j` if it is free and its node passes `test`.
    fn take_at(&mut self, j: usize, test: impl FnOnce(NodeId) -> bool) -> Option<NodeId> {
        let n = self.slots.get(j).copied().flatten().filter(|n| test(*n))?;
        self.slots[j] = None;
        while self.slots.get(self.first).is_some_and(Option::is_none) {
            self.first += 1;
        }
        Some(n)
    }

    /// The nodes no new child took.
    fn into_rest(self) -> impl Iterator<Item = NodeId> {
        self.slots.into_iter().flatten()
    }

    fn index(
        slots: &[Option<NodeId>],
        nodes: &HashMap<NodeId, MNode>,
    ) -> (HashMap<u64, usize>, HashMap<EntityId, usize>) {
        let (mut keys, mut entities) = (HashMap::new(), HashMap::new());
        for (j, n) in slots.iter().enumerate() {
            let Some(m) = n.and_then(|n| nodes.get(&n)) else {
                continue;
            };
            if let Some(key) = &m.key {
                keys.entry(key_hash(key)).or_insert(j);
            }
            if let Some(e) = m.entity {
                entities.entry(e.id()).or_insert(j);
            }
        }
        (keys, entities)
    }
}

fn key_hash(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

#[derive(Default)]
/// Diffs entity renders against the mounted tree and keeps the event handlers of mounted nodes.
pub struct Reconciler {
    nodes: HashMap<NodeId, MNode>,
    root: Option<NodeId>,
    entity_roots: HashMap<EntityId, NodeId>,
    next_id: NodeId,
    click: HashMap<NodeId, Handler<ClickEvent>>,
    input: HashMap<NodeId, Handler<InputEvent>>,
    rich_input: HashMap<NodeId, Handler<RichInputEvent>>,
    rich: HashMap<NodeId, RichState>,
    select: HashMap<NodeId, Handler<SelectionEvent>>,
    back: HashMap<NodeId, Handler<BackEvent>>,
    navigate: HashMap<NodeId, Handler<NavigateEvent>>,
    change: HashMap<NodeId, Handler<ChangeEvent>>,
    scene_tap: HashMap<NodeId, Handler<crate::scene3d::SceneTapEvent>>,
    scroll: HashMap<NodeId, Handler<ScrollEvent>>,
    swipe: HashMap<(NodeId, SwipeDirection), Handler<ClickEvent>>,
    submit: HashMap<NodeId, Handler<ClickEvent>>,
    /// Context menu items and clickable runs of each node, by item id
    /// (node id << 16 | n).
    menu_items: HashMap<NodeId, Vec<(u64, Handler<ClickEvent>)>>,
    // Per-frame scratch state.
    dirty: HashSet<EntityId>,
    patches: Vec<Patch>,
    destroyed: Vec<NodeId>,
    rendered: usize,
    appearance: Appearance,
    environment: Option<RenderEnvironment>,
    /// `with_animation` of this render: given to nodes without a transition.
    animation: Option<crate::style::Transition>,
    /// Nodes that got it, reset on the next render.
    animated: Vec<NodeId>,
    // Pointer, keys, focus, layers (#17, #25).
    hover: HashMap<NodeId, Handler<crate::events::HoverEvent>>,
    pointer_move: HashMap<NodeId, Handler<crate::events::PointerEvent>>,
    drag: HashMap<NodeId, Handler<crate::events::DragEvent>>,
    wheel: HashMap<NodeId, Handler<crate::events::WheelEvent>>,
    aux_click: HashMap<NodeId, Handler<crate::element::ClickEvent>>,
    focus_change: HashMap<NodeId, Handler<crate::element::FocusEvent>>,
    /// Rich inputs' editing hooks (#217).
    edit: HashMap<NodeId, crate::text_edit::EditHandlers>,
    /// Selection groups (#231) and their handlers.
    groups: HashMap<NodeId, crate::selection_across::GroupHandlers>,
    /// The selection across fields shown now (#231).
    across: Option<Across>,
    layout: HashMap<NodeId, Handler<crate::events::LayoutEvent>>,
    drop_progress: HashMap<NodeId, Handler<crate::events::DropProgress>>,
    ended: HashMap<NodeId, Handler<crate::video::EndedEvent>>,
    magnify: HashMap<NodeId, Handler<crate::events::MagnifyEvent>>,
    key_down: HashMap<NodeId, Handler<crate::events::KeyEvent>>,
    global_key: HashMap<NodeId, Handler<crate::events::KeyEvent>>,
    key_up: HashMap<NodeId, Handler<crate::events::KeyEvent>>,
    global_key_up: HashMap<NodeId, Handler<crate::events::KeyEvent>>,
    /// `.id(…)` names of mounted nodes.
    names: HashMap<String, NodeId>,
    /// Nodes created with `.autofocus()` in this render.
    autofocus: Vec<NodeId>,
    /// Runtime-mounted layers: (entity, its root node), bottom first.
    layers: Vec<(EntityId, NodeId)>,
    /// The layer roots backends last got.
    sent_layers: Vec<NodeId>,
    /// Islands' event handlers: (node, event name) → handler.
    island_events: HashMap<(NodeId, String), Handler<crate::island::IslandEvent>>,
    /// Web views' handlers (#62).
    web_message: HashMap<NodeId, Handler<crate::web_view::WebMessageEvent>>,
    web_load: HashMap<NodeId, Handler<crate::web_view::WebLoadEvent>>,
    /// What draggable nodes carry (#251).
    drag_payloads: HashMap<NodeId, crate::events::DragPayload>,
    /// Payload drop targets' handlers, by kind (#251).
    drop_payload: HashMap<NodeId, Vec<(String, Handler<crate::events::DropEvent>)>>,
    drag_over: HashMap<NodeId, Vec<(String, Handler<crate::events::DragOverEvent>)>>,
    /// Drop targets' accepted types and handlers (#111).
    drop_files: HashMap<
        NodeId,
        (
            crate::device::PickFiles,
            Handler<crate::events::FileDropEvent>,
        ),
    >,
}

impl Reconciler {
    /// An empty reconciler (nothing mounted).
    pub fn new() -> Self {
        Self::default()
    }

    /// The click handler of node `id`, if any.
    pub fn click_handler(&self, id: NodeId) -> Option<Handler<ClickEvent>> {
        self.click.get(&id).cloned()
    }

    /// The input handler of node `id`, if any.
    pub fn input_handler(&self, id: NodeId) -> Option<Handler<InputEvent>> {
        self.input.get(&id).cloned()
    }

    /// The input handler of rich input `id`, if any (#115).
    pub fn rich_input_handler(&self, id: NodeId) -> Option<Handler<RichInputEvent>> {
        self.rich_input.get(&id).cloned()
    }

    /// Rich input `id`'s plain text became `value` (#115): a backend that
    /// edits plain text. The runs around the change keep their style (see
    /// [`RichInputEvent`]). `None`: `id` is no rich input.
    pub fn rich_text_edit(&mut self, id: NodeId, value: &str) -> Option<RichEdit> {
        let state = self.rich.get(&id)?;
        let old: Vec<crate::spans::Span> = state.spans.iter().map(|(_, s)| s.clone()).collect();
        let before = crate::spans::spans_text(&old);
        if self.submit.contains_key(&id) && is_line_break_typed(&before, value) {
            // The field shows a line break the app does not have: send it
            // the app's value again at the next render.
            if let Some(m) = self.nodes.get_mut(&id) {
                m.text = value.to_string();
                m.runs.clear();
            }
            return Some(RichEdit::Submit);
        }
        let spans = crate::spans::edit_text(&old, value);
        Some(RichEdit::Spans(RichInputEvent::new(spans)))
    }

    /// Rich input `id` reported `runs` (#115): the web client and AppKit,
    /// which keep each run's style. `None`: `id` is no rich input or has
    /// no `on_input`.
    pub fn rich_runs_edit(
        &self,
        id: NodeId,
        runs: &str,
    ) -> Option<(Handler<RichInputEvent>, RichInputEvent)> {
        let h = self.rich_input.get(&id)?.clone();
        let state = self.rich.get(&id)?;
        let spans =
            crate::spans::edit_runs(&state.spans, state.font_size, crate::decode_runs(runs));
        Some((h, RichInputEvent::new(spans)))
    }

    /// The selection handler of text field `id`, if any (#86).
    pub fn select_handler(&self, id: NodeId) -> Option<Handler<SelectionEvent>> {
        self.select.get(&id).cloned()
    }

    /// Text field `id` reported `selection` (#86): the field shows it, so
    /// the app rendering it back is an echo, not a change to apply.
    pub fn selection_reported(&mut self, id: NodeId, selection: SelectionEvent) {
        if let Some(m) = self.nodes.get_mut(&id) {
            m.shown_selection = Some(selection);
        }
    }

    /// The back handler of stack `id`, if any.
    pub fn back_handler(&self, id: NodeId) -> Option<Handler<BackEvent>> {
        self.back.get(&id).cloned()
    }

    /// The navigate handler of stack `id`, if any.
    pub fn navigate_handler(&self, id: NodeId) -> Option<Handler<NavigateEvent>> {
        self.navigate.get(&id).cloned()
    }

    /// The scroll handler of node `id`, if any.
    pub fn scroll_handler(&self, id: NodeId) -> Option<Handler<ScrollEvent>> {
        self.scroll.get(&id).cloned()
    }

    /// The handler of node `id` for swipes in `dir`, if any.
    pub fn swipe_handler(&self, id: NodeId, dir: SwipeDirection) -> Option<Handler<ClickEvent>> {
        self.swipe.get(&(id, dir)).cloned()
    }

    /// The handler of node `id` for the return key (`on_submit`), if any.
    pub fn submit_handler(&self, id: NodeId) -> Option<Handler<ClickEvent>> {
        self.submit.get(&id).cloned()
    }

    /// The handler of context menu item `item`, if any.
    pub fn menu_handler(&self, item: u64) -> Option<Handler<ClickEvent>> {
        self.menu_items
            .get(&(item >> 16))?
            .iter()
            .find(|(k, _)| *k == item)
            .map(|(_, h)| h.clone())
    }

    /// The change handler of node `id`, if any.
    pub fn change_handler(&self, id: NodeId) -> Option<Handler<ChangeEvent>> {
        self.change.get(&id).cloned()
    }

    /// The tap handler of 3D scene `id`, if any.
    pub fn scene_tap_handler(&self, id: NodeId) -> Option<Handler<crate::scene3d::SceneTapEvent>> {
        self.scene_tap.get(&id).cloned()
    }

    /// Whether node `id` takes an empty value (`NaN`): an optional date
    /// picker (#89). Hosts report a cleared picker as `NaN`; no other
    /// control has an empty value.
    pub fn accepts_empty(&self, id: NodeId) -> bool {
        self.nodes.get(&id).is_some_and(|m| {
            m.kind == NodeKind::DatePicker
                && crate::controls::DateConfig::decode(&m.config).optional
        })
    }

    /// The hover handler of node `id`, if any.
    pub fn hover_handler(&self, id: NodeId) -> Option<Handler<crate::events::HoverEvent>> {
        self.hover.get(&id).cloned()
    }

    /// The pointer-move handler of node `id`, if any.
    pub fn pointer_handler(&self, id: NodeId) -> Option<Handler<crate::events::PointerEvent>> {
        self.pointer_move.get(&id).cloned()
    }

    /// The drag handler of node `id`, if any.
    pub fn drag_handler(&self, id: NodeId) -> Option<Handler<crate::events::DragEvent>> {
        self.drag.get(&id).cloned()
    }

    /// The wheel handler of node `id`, if any (#133).
    pub fn wheel_handler(&self, id: NodeId) -> Option<Handler<crate::events::WheelEvent>> {
        self.wheel.get(&id).cloned()
    }

    /// Rich input `id`'s editing hooks (#217), if it has any.
    pub fn edit_handlers(&self, id: NodeId) -> Option<&crate::text_edit::EditHandlers> {
        self.edit.get(&id)
    }

    /// The spans rich input `id` showed at the last render (#217), as the
    /// app gave them (empty for another node).
    pub fn rich_spans(&self, id: NodeId) -> Vec<crate::spans::Span> {
        self.rich
            .get(&id)
            .map(|r| r.spans.iter().map(|(_, s)| s.clone()).collect())
            .unwrap_or_default()
    }

    /// Node `id`'s `on_focus_change` handler (#224).
    pub fn focus_change_handler(&self, id: NodeId) -> Option<Handler<crate::element::FocusEvent>> {
        self.focus_change.get(&id).cloned()
    }

    /// Node `id`'s `on_aux_click` handler (#215).
    pub fn aux_click_handler(&self, id: NodeId) -> Option<Handler<crate::element::ClickEvent>> {
        self.aux_click.get(&id).cloned()
    }

    /// The pinch handler of node `id`, if any (#133).
    pub fn magnify_handler(&self, id: NodeId) -> Option<Handler<crate::events::MagnifyEvent>> {
        self.magnify.get(&id).cloned()
    }

    /// What node `id` carries when dragged (#251), if it is draggable.
    pub fn drag_payload(&self, id: NodeId) -> Option<&crate::events::DragPayload> {
        self.drag_payloads.get(&id)
    }

    /// Whether node `id` takes dropped payloads of `kind` (#251).
    pub fn accepts_payload(&self, id: NodeId, kind: &str) -> bool {
        let drops = self.drop_payload.get(&id);
        let overs = self.drag_over.get(&id);
        drops.is_some_and(|v| v.iter().any(|(k, _)| k == kind))
            || overs.is_some_and(|v| v.iter().any(|(k, _)| k == kind))
    }

    /// Node `id`'s `on_drop_payload` handler for `kind` (#251).
    pub fn drop_payload_handler(
        &self,
        id: NodeId,
        kind: &str,
    ) -> Option<Handler<crate::events::DropEvent>> {
        let v = self.drop_payload.get(&id)?;
        v.iter().find(|(k, _)| k == kind).map(|(_, h)| h.clone())
    }

    /// Node `id`'s `on_drag_over` handler for `kind` (#251).
    pub fn drag_over_handler(
        &self,
        id: NodeId,
        kind: &str,
    ) -> Option<Handler<crate::events::DragOverEvent>> {
        let v = self.drag_over.get(&id)?;
        v.iter().find(|(k, _)| k == kind).map(|(_, h)| h.clone())
    }

    /// The accepted types and the handler of drop target `id` (#111).
    pub fn drop_files_handler(
        &self,
        id: NodeId,
    ) -> Option<(
        crate::device::PickFiles,
        Handler<crate::events::FileDropEvent>,
    )> {
        self.drop_files.get(&id).cloned()
    }

    /// The handlers a key press in node `focused` reaches, in order: the
    /// node's and its ancestors' `on_key_down`, then every
    /// `on_global_key_down`, most recently mounted first.
    pub fn key_handlers(&self, focused: Option<NodeId>) -> Vec<Handler<crate::events::KeyEvent>> {
        self.key_handlers_of(focused, &self.key_down, &self.global_key)
    }

    /// Like [`key_handlers`](Self::key_handlers), for a released key
    /// (`on_key_up`, then `on_global_key_up`, #159).
    pub fn key_up_handlers(
        &self,
        focused: Option<NodeId>,
    ) -> Vec<Handler<crate::events::KeyEvent>> {
        self.key_handlers_of(focused, &self.key_up, &self.global_key_up)
    }

    /// The `on_ended` handler of video `id` (#207).
    pub fn ended_handler(&self, id: NodeId) -> Option<Handler<crate::video::EndedEvent>> {
        self.ended.get(&id).cloned()
    }

    /// The `on_drop_progress` handler of node `id` (#199).
    pub fn drop_progress_handler(
        &self,
        id: NodeId,
    ) -> Option<Handler<crate::events::DropProgress>> {
        self.drop_progress.get(&id).cloned()
    }

    /// The `on_layout` handler of node `id` (#192).
    pub fn layout_handler(&self, id: NodeId) -> Option<Handler<crate::events::LayoutEvent>> {
        self.layout.get(&id).cloned()
    }

    /// Whether node `id` is mounted and reports its frame (#192).
    pub fn reports_layout(&self, id: NodeId) -> bool {
        self.nodes.get(&id).is_some_and(|n| n.layout_events)
    }

    /// Whether node `id` is mounted.
    pub fn is_mounted(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// The `.id()` of node `id`, if it has one.
    pub fn name_of(&self, id: NodeId) -> Option<&str> {
        self.nodes.get(&id).and_then(|n| n.name.as_deref())
    }

    /// Whether `key` pressed in node `focused` is taken from the focused
    /// control by it or an ancestor (`consume_keys`, #181).
    /// `edges`: where the focused text field's caret is (#211), for
    /// entries of [`consume_key_at`](crate::Div::consume_key_at).
    pub fn consumes_key(
        &self,
        focused: Option<NodeId>,
        key: &str,
        edges: Option<crate::events::CaretEdges>,
    ) -> bool {
        let mut next = focused;
        while let Some(id) = next {
            let Some(n) = self.nodes.get(&id) else {
                return false;
            };
            if n.consume_keys
                .iter()
                .any(|k| crate::events::consume_entry_matches(k, key, edges))
            {
                return true;
            }
            next = n.parent;
        }
        false
    }

    fn key_handlers_of(
        &self,
        focused: Option<NodeId>,
        local: &HashMap<NodeId, Handler<crate::events::KeyEvent>>,
        global: &HashMap<NodeId, Handler<crate::events::KeyEvent>>,
    ) -> Vec<Handler<crate::events::KeyEvent>> {
        let mut out = Vec::new();
        let mut next = focused;
        while let Some(id) = next {
            if let Some(h) = local.get(&id) {
                out.push(h.clone());
            }
            next = self.nodes.get(&id).and_then(|n| n.parent);
        }
        let mut global: Vec<(&NodeId, &Handler<crate::events::KeyEvent>)> = global.iter().collect();
        global.sort_by(|a, b| b.0.cmp(a.0));
        out.extend(global.into_iter().map(|(_, h)| h.clone()));
        out
    }

    /// The handler of island `id` for its event `name`, if any.
    pub fn island_handler(
        &self,
        id: NodeId,
        name: &str,
    ) -> Option<Handler<crate::island::IslandEvent>> {
        self.island_events.get(&(id, name.to_string())).cloned()
    }

    /// The message handler of web view `id`, if any.
    pub fn web_message_handler(
        &self,
        id: NodeId,
    ) -> Option<Handler<crate::web_view::WebMessageEvent>> {
        self.web_message.get(&id).cloned()
    }

    /// The load handler of web view `id`, if any.
    pub fn web_load_handler(&self, id: NodeId) -> Option<Handler<crate::web_view::WebLoadEvent>> {
        self.web_load.get(&id).cloned()
    }

    /// The mounted node named `name` (`.id(name)`), if any.
    pub fn node_named(&self, name: &str) -> Option<NodeId> {
        self.names.get(name).copied()
    }

    /// Whether node `id` is a sheet, popover or dialog (an overlay or a
    /// modal stack), whose dismissal has a reason.
    pub fn is_overlay(&self, id: NodeId) -> bool {
        self.nodes
            .get(&id)
            .is_some_and(|m| matches!(m.kind, NodeKind::Overlay | NodeKind::Modal))
    }

    /// The user closed overlay `id` natively: remember it is closed, so
    /// opening it again shows it even if the app never set it to closed.
    pub fn overlay_dismissed(&mut self, id: NodeId) {
        if let Some(m) = self.nodes.get_mut(&id)
            && m.kind == NodeKind::Overlay
        {
            m.value = Some(0.);
        }
    }

    /// Number of entity renders performed by the last [`render`](Self::render).
    pub fn last_render_count(&self) -> usize {
        self.rendered
    }

    /// Re-renders dirty entities (or everything, on first mount) and returns
    /// the patches to apply.
    pub fn render(&mut self, app: &mut App) -> Vec<Patch> {
        self.dirty = app.take_dirty();
        self.rendered = 0;
        // Last render's animated update is over: later changes are instant.
        for id in std::mem::take(&mut self.animated) {
            if let Some(m) = self.nodes.get_mut(&id)
                && m.style.transition.is_some()
            {
                m.style.transition = None;
                self.patches.push(Patch::SetStyle {
                    id,
                    style: m.style.clone(),
                });
            }
        }
        self.animation = app.take_animation();
        let Some(root_entity) = app.root() else {
            return Vec::new();
        };
        // The environment comes before any node patch (text is created at
        // the right scale and direction). The first render, the biggest,
        // keeps it a place now, so its patches need not move to make room.
        let first = self.environment.is_none();
        if first {
            self.patches.insert(
                0,
                Patch::SetEnvironment {
                    env: RenderEnvironment::default(),
                },
            );
        }

        let root_owner = self
            .root
            .and_then(|r| self.nodes.get(&r))
            .and_then(|n| n.entity)
            .map(|e| e.id());
        if root_owner != Some(root_entity.id()) {
            // First mount, or the root entity was swapped.
            let vnode = self.render_entity(app, root_entity);
            let id = self.diff(app, self.root, None, vnode, Some(root_entity));
            if self.root != Some(id) {
                self.root = Some(id);
                self.patches.push(Patch::SetRoot { id });
            }
        }
        self.mount_layers(app);

        // Re-render the remaining dirty entities in place, shallowest first, so
        // an ancestor's re-render handles its dirty descendants in one pass.
        // Sorted once: a re-render only moves the nodes under it, and renders
        // the dirty entities there.
        let mut order: Vec<(usize, EntityId)> = self
            .dirty
            .iter()
            .filter_map(|e| self.entity_roots.get(e).map(|n| (self.depth(*n), *e)))
            .collect();
        order.sort_unstable();
        for (_, e) in order {
            // Rendered with an ancestor, or unmounted by it.
            if !self.dirty.contains(&e) {
                continue;
            }
            let Some(&node) = self.entity_roots.get(&e) else {
                continue;
            };
            let entity = self.nodes[&node].entity.expect("entity root is tagged");
            let parent = self.nodes[&node].parent;
            let vnode = self.render_entity(app, entity);
            let new = self.diff(app, Some(node), parent, vnode, Some(entity));
            if new != node {
                self.replace_child(parent, node, new);
            }
            // Clickable ancestors read this entity's text in their label.
            self.refresh_auto_labels(parent);
        }
        self.dirty.clear();

        let mut patches = std::mem::take(&mut self.patches);
        patches.extend(self.destroyed.drain(..).map(|id| Patch::Destroy { id }));
        let env = RenderEnvironment {
            lang: app.locale().tag().to_string(),
            direction: app.direction(),
            text_scale: app.text_scale(),
            reduce_motion: app.reduce_motion(),
            back: app.chrome_text("back"),
            close: app.chrome_text("close"),
            clear: app.chrome_text("clear"),
        };
        if self.environment.as_ref() != Some(&env) {
            let patch = Patch::SetEnvironment { env: env.clone() };
            if first {
                patches[0] = patch;
            } else {
                patches.insert(0, patch);
            }
            self.environment = Some(env);
        }
        if app.appearance() != self.appearance {
            self.appearance = app.appearance();
            patches.push(Patch::SetAppearance {
                appearance: self.appearance,
            });
        }
        self.push_commands(app, &mut patches);
        patches
    }

    /// Mounts the app's layers (`App::add_layer`) that are not mounted yet
    /// and unmounts removed ones.
    fn mount_layers(&mut self, app: &mut App) {
        let wanted = app.layers.clone();
        let mut next = Vec::with_capacity(wanted.len());
        for entity in wanted {
            let mounted = self
                .layers
                .iter()
                .find(|(e, _)| *e == entity.id())
                .map(|(_, n)| *n);
            let id = match mounted {
                Some(n) => n,
                None => {
                    let vnode = self.render_entity(app, entity);
                    self.diff(app, None, None, vnode, Some(entity))
                }
            };
            next.push((entity.id(), id));
        }
        for (e, n) in std::mem::take(&mut self.layers) {
            if !next.iter().any(|(k, _)| *k == e) {
                self.destroy(n);
            }
        }
        self.layers = next;
        self.push_layers();
    }

    /// Tells backends the layer roots, if they changed.
    fn push_layers(&mut self) {
        let ids: Vec<NodeId> = self.layers.iter().map(|(_, n)| *n).collect();
        if ids != self.sent_layers {
            self.sent_layers = ids.clone();
            self.patches.push(Patch::SetLayers { ids });
        }
    }

    /// Focus and scroll requests, after every node is mounted.
    fn push_commands(&mut self, app: &mut App, patches: &mut Vec<Patch>) {
        for id in std::mem::take(&mut self.autofocus) {
            if self.nodes.contains_key(&id) {
                patches.push(Patch::Focus { id });
            }
        }
        for command in std::mem::take(&mut app.ui_commands) {
            use crate::app::UiCommand as C;
            let name = match &command {
                C::ShowMenu(..) => {
                    let C::ShowMenu(target, menu, at) = command else {
                        unreachable!()
                    };
                    use crate::app::MenuTarget as T;
                    let id = match target {
                        T::Window => None,
                        T::Named(name) => match self.node_named(&name) {
                            Some(id) => Some(id),
                            None => continue,
                        },
                        T::Node(id) if self.nodes.contains_key(&id) => Some(id),
                        T::Node(_) => continue,
                    };
                    patches.push(Patch::ShowMenu { id, menu, at });
                    continue;
                }
                C::Focus(n) | C::ScrollIntoView(n) | C::ReloadWebView(n) => n,
                C::Scroll(n, ..) => n,
                C::PostWebMessage(n, _) => n,
                C::SelectionAcross(range) => {
                    patches.extend(self.command_across(range.clone()));
                    continue;
                }
                C::DismissKeyboard => {
                    patches.push(Patch::DismissKeyboard);
                    continue;
                }
            };
            let Some(id) = self.node_named(name) else {
                continue;
            };
            let web_view = self.nodes[&id].kind == NodeKind::WebView;
            let scroll = self.nodes[&id].kind == NodeKind::Scroll;
            patches.push(match command {
                C::Focus(_) => Patch::Focus { id },
                C::ScrollIntoView(_) => Patch::ScrollIntoView { id },
                C::Scroll(_, x, y, relative, animated) if scroll => Patch::ScrollTo {
                    id,
                    x,
                    y,
                    relative,
                    animated,
                },
                C::PostWebMessage(_, data) if web_view => Patch::PostWebMessage { id, data },
                C::ReloadWebView(_) if web_view => Patch::ReloadWebView { id },
                _ => continue,
            });
        }
    }

    fn replace_child(&mut self, parent: Option<NodeId>, old: NodeId, new: NodeId) {
        match parent {
            Some(p) => {
                let node = self.nodes.get_mut(&p).expect("parent is mounted");
                for c in node.children.iter_mut() {
                    if *c == old {
                        *c = new;
                    }
                }
                self.patches.push(Patch::SetChildren {
                    id: p,
                    children: node.children.clone(),
                });
            }
            None => {
                if let Some(layer) = self.layers.iter_mut().find(|(_, n)| *n == old) {
                    layer.1 = new;
                    self.push_layers();
                    return;
                }
                self.root = Some(new);
                self.patches.push(Patch::SetRoot { id: new });
            }
        }
    }

    fn depth(&self, mut id: NodeId) -> usize {
        let mut depth = 0;
        while let Some(parent) = self.nodes.get(&id).and_then(|n| n.parent) {
            depth += 1;
            id = parent;
        }
        depth
    }

    /// Renders an entity to exactly one root node.
    fn render_entity(&mut self, app: &mut App, entity: AnyEntity) -> Box<VNode> {
        self.dirty.remove(&entity.id());
        self.rendered += 1;
        let mut element = entity.render(app);
        while let Element::Lazy(build) = element {
            element = build(app);
        }
        match element {
            // A component whose root is a field with its own keyboard bar
            // (#242): the bar needs a sibling's place, in a wrapper.
            Element::Node(mut vnode) if vnode.keyboard_toolbar.is_some() => {
                let bar = vnode
                    .keyboard_toolbar
                    .take()
                    .map(|bar| own_keyboard_bar(&vnode, *bar));
                let mut wrapper = VNode::new(NodeKind::View);
                wrapper.children.push(Element::Node(vnode));
                wrapper.children.extend(bar);
                Box::new(wrapper)
            }
            Element::Node(vnode) => vnode,
            other => {
                let mut wrapper = VNode::new(NodeKind::View);
                wrapper.children.push(other);
                Box::new(wrapper)
            }
        }
    }

    fn flatten(&mut self, app: &mut App, element: Element, out: &mut Vec<Item>) {
        match element {
            Element::Empty => {}
            // `.only(…)` / `.except(…)`: absent from other platforms' trees.
            Element::Node(vnode) if !vnode.style.exists_on(app.platform()) => {}
            Element::Node(mut vnode) => {
                // A field's own keyboard bar (#242) goes right after it.
                let bar = vnode
                    .keyboard_toolbar
                    .take()
                    .map(|bar| own_keyboard_bar(&vnode, *bar));
                out.push(Item::Node(vnode, None));
                if let Some(bar) = bar {
                    self.flatten(app, bar, out);
                }
            }
            Element::Lazy(build) => {
                let element = build(app);
                self.flatten(app, element, out);
            }
            Element::Fragment(items) => {
                for item in items {
                    self.flatten(app, item, out);
                }
            }
            Element::Entity(entity) => {
                let clean = !self.dirty.contains(&entity.id())
                    && self.entity_roots.contains_key(&entity.id());
                if clean {
                    out.push(Item::Cached(entity));
                } else {
                    let vnode = self.render_entity(app, entity);
                    if vnode.style.exists_on(app.platform()) {
                        out.push(Item::Node(vnode, Some(entity)));
                    }
                }
            }
        }
    }

    fn owner(&self, id: NodeId) -> Option<EntityId> {
        self.nodes.get(&id).and_then(|n| n.entity).map(|e| e.id())
    }

    fn diff(
        &mut self,
        app: &mut App,
        old: Option<NodeId>,
        parent: Option<NodeId>,
        vnode: Box<VNode>,
        entity: Option<AnyEntity>,
    ) -> NodeId {
        let mut vnode = vnode;
        let island = vnode.island.take();
        let resolved = vnode.style.resolve(app.style_context());
        let animates = resolved.transition.is_some();
        // On the web a link is an `<a>` element, anything else a `<div>`:
        // a node that gains or loses its link is a new element there.
        let web = app.platform() == crate::Platform::Web;
        let reusable = old.filter(|o| {
            let m = &self.nodes[o];
            // An island keeps its element only while its tag is the same.
            let tag = |i: Option<&str>| i.map(str::to_string);
            m.kind == vnode.kind
                && m.key == vnode.key
                && tag(m.island.as_ref().map(|i| i.tag.as_str()))
                    == tag(island.as_ref().map(|i| i.tag.as_str()))
                && (!web || m.link.is_some() == vnode.link.is_some())
                && self.keeps_identity(m, animates, entity.as_ref())
        });
        if let (Some(old), None) = (old, reusable) {
            self.destroy(old);
        }
        let id = match reusable {
            Some(id) => id,
            None => self.create(vnode.kind, vnode.key.clone()),
        };
        // Right after its `Create`: the web swaps in the custom element, or
        // the `<a>` of a link.
        self.diff_island(id, island);
        {
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.link != vnode.link {
                self.patches.push(Patch::SetLink {
                    id,
                    href: vnode.link.clone(),
                });
                m.link.clone_from(&vnode.link);
            }
        }

        // Ownership: which entity's root this node is.
        let previous_owner = self.owner(id);
        let new_owner = entity.map(|e| e.id());
        if let Some(prev) = previous_owner.filter(|p| Some(*p) != new_owner)
            && self.entity_roots.get(&prev) == Some(&id)
        {
            self.entity_roots.remove(&prev);
        }
        if let Some(e) = new_owner {
            self.entity_roots.insert(e, id);
        }

        let VNode {
            kind,
            style,
            text,
            placeholder,
            icon,
            value,
            config,
            a11y,
            children,
            on_click,
            on_input,
            on_rich_input,
            on_select,
            selection,
            remote_carets,
            on_back,
            on_navigate,
            on_change,
            link,
            context_menu,
            click_menu,
            on_scroll,
            on_swipe_left,
            on_swipe_right,
            on_submit,
            spans,
            id: name,
            autofocus,
            on_hover,
            on_pointer_move,
            on_drag,
            on_key_down,
            on_global_key_down,
            on_key_up,
            on_global_key_up,
            on_scene_tap,
            scene,
            attributes,
            on_web_message,
            on_web_load,
            on_drop_files,
            on_drop_progress,
            drag_payload,
            on_drop_payload,
            on_drag_over,
            on_ended,
            on_wheel,
            on_magnify,
            on_aux_click,
            on_focus_change,
            drag_buttons,
            drag_delay_ms,
            consume_keys,
            on_layout,
            track_layout,
            edit,
            selection_group,
            across,
            ..
        } = *vnode;
        app.window.watch_widths(&style);
        {
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.attributes != attributes {
                self.patches.push(Patch::SetAttributes {
                    id,
                    attributes: attributes.clone(),
                });
                m.attributes = attributes;
            }
        }
        if autofocus && reusable.is_none() {
            self.autofocus.push(id);
        }
        {
            let m = self.nodes.get_mut(&id).expect("node exists");
            // A selection's rectangle is also given in the window (#217).
            let edit_mask = if m.kind == NodeKind::RichInput {
                edit.mask()
            } else {
                0
            };
            let reports = on_layout.is_some()
                || track_layout
                || edit_mask & crate::edit_events::SELECTION_RECT != 0;
            if m.edit_mask != edit_mask {
                m.edit_mask = edit_mask;
                self.patches.push(Patch::SetEditEvents {
                    id,
                    mask: edit_mask,
                });
            }
            if m.layout_events != reports {
                m.layout_events = reports;
                self.patches
                    .push(Patch::SetLayoutEvents { id, on: reports });
            }
            match on_layout {
                Some(h) => self.layout.insert(id, h),
                None => self.layout.remove(&id),
            };
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.consume_keys != consume_keys {
                self.patches.push(Patch::SetConsumedKeys {
                    id,
                    keys: consume_keys.clone(),
                });
                m.consume_keys = consume_keys;
            }
        }
        match on_focus_change {
            Some(h) => self.focus_change.insert(id, h),
            None => self.focus_change.remove(&id),
        };
        if edit.is_empty() {
            self.edit.remove(&id);
        } else {
            self.edit.insert(id, edit);
        }
        // A selection group (#231) hears its fields' keys: the runtime
        // extends the selection across them.
        {
            let m = self.nodes.get_mut(&id).expect("node exists");
            let flags = (selection_group, selection_group && across.on_edit.is_some());
            if m.selection_group != flags {
                m.selection_group = flags;
                self.patches.push(Patch::SetSelectionGroup {
                    id,
                    on: flags.0,
                    edits: flags.1,
                });
            }
        }
        let on_key_down = if selection_group {
            self.groups.insert(id, across);
            on_key_down.or_else(|| Some(Rc::new(|_: &crate::events::KeyEvent, _: &mut App| {})))
        } else {
            self.groups.remove(&id);
            on_key_down
        };
        // Drag and drop between elements (#251), before the pointer bits: a
        // draggable node reports its drags.
        {
            let mut accepts: Vec<String> = Vec::new();
            for k in on_drop_payload
                .iter()
                .map(|(k, _)| k)
                .chain(on_drag_over.iter().map(|(k, _)| k))
            {
                if !accepts.contains(k) {
                    accepts.push(k.clone());
                }
            }
            let dnd = (
                drag_payload.as_ref().map(|p| (p.kind.clone(), p.preview)),
                accepts,
            );
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.dnd != dnd {
                self.patches.push(Patch::SetDragAndDrop {
                    id,
                    kind: dnd.0.as_ref().map(|(k, _)| k.clone()),
                    preview: dnd.0.as_ref().map(|(_, p)| *p).unwrap_or_default(),
                    accepts: dnd.1.clone(),
                });
                m.dnd = dnd;
            }
            match drag_payload {
                Some(p) => self.drag_payloads.insert(id, p),
                None => self.drag_payloads.remove(&id),
            };
            if on_drop_payload.is_empty() {
                self.drop_payload.remove(&id);
            } else {
                self.drop_payload.insert(id, on_drop_payload);
            }
            if on_drag_over.is_empty() {
                self.drag_over.remove(&id);
            } else {
                self.drag_over.insert(id, on_drag_over);
            }
        }
        self.diff_events(
            id,
            name,
            (on_hover, on_pointer_move, on_drag, drag_buttons),
            (on_wheel, on_magnify, on_aux_click),
            (on_key_down, on_global_key_down, on_key_up, on_global_key_up),
        );
        {
            // Only for a node that drags (#243).
            let m = self.nodes.get_mut(&id).expect("node exists");
            let ms = if m.pointer_mask & crate::events::pointer_mask::DRAG_ANY != 0 {
                drag_delay_ms
            } else {
                0
            };
            if m.drag_delay_ms != ms {
                m.drag_delay_ms = ms;
                self.patches.push(Patch::SetDragDelay { id, ms });
            }
        }
        let mut style = resolved;
        if let Some(t) = self.animation
            && style.transition.is_none()
        {
            style.transition = Some(t);
            self.animated.push(id);
        }
        {
            let swipe = (on_swipe_left.is_some(), on_swipe_right.is_some());
            let scroll_events = on_scroll.is_some();
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.swipe != swipe {
                self.patches.push(Patch::SetSwipe {
                    id,
                    left: swipe.0,
                    right: swipe.1,
                });
                m.swipe = swipe;
            }
            if m.scroll_events != scroll_events {
                self.patches.push(Patch::SetScrollEvents {
                    id,
                    on: scroll_events,
                });
                m.scroll_events = scroll_events;
            }
        }
        match on_scroll {
            Some(h) => self.scroll.insert(id, h),
            None => self.scroll.remove(&id),
        };
        for (dir, h) in [
            (SwipeDirection::Left, on_swipe_left),
            (SwipeDirection::Right, on_swipe_right),
        ] {
            match h {
                Some(h) => self.swipe.insert((id, dir), h),
                None => self.swipe.remove(&(id, dir)),
            };
        }
        // Context menu: ids derived from the node id stay stable across renders.
        self.menu_items.remove(&id);
        let menu_spec = context_menu.map(|m| {
            let mut handlers = Vec::new();
            let spec = m.compile(id << 16, &mut handlers);
            if !handlers.is_empty() {
                self.menu_items.insert(id, handlers);
            }
            spec
        });
        {
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.menu != menu_spec {
                self.patches.push(Patch::SetMenu {
                    id,
                    menu: menu_spec.clone(),
                });
                m.menu = menu_spec;
            }
        }
        // Rich text: the runs are the config; clickable runs get item ids
        // (after the menu above, which drops this node's old items).
        let (config, rich_label, rich_runs) = if kind == NodeKind::RichInput {
            let runs = self.mount_rich(id, style.font_size, spans);
            (config, None, Some(runs))
        } else if spans.is_empty() {
            (config, None, None)
        } else {
            let (config, label) = self.mount_spans(id, style.font_size, spans);
            (config, label, None)
        };
        // A link navigates when activated in the app.
        let on_click = on_click.or_else(|| link.clone().map(link_handler));
        // A click menu (#247) opens below the node, in place of `on_click`.
        let on_click = match click_menu {
            Some(menu) => {
                let h: Handler<ClickEvent> =
                    std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
                        app.show_menu_below(id, &menu)
                    });
                Some(h)
            }
            None => on_click,
        };
        let a11y = {
            let mut a = a11y;
            if a.label.is_none() {
                a.label = rich_label;
            }
            if link.is_some() && a.role.is_none() {
                a.role = Some(Role::Link);
            }
            a
        };
        let clickable = kind == NodeKind::Button || on_click.is_some();
        // Disabled: its `disabled` refinement, and no hover, press or focus.
        if a11y.disabled && !style.states.is_empty() {
            style = style.disabled_applied();
        }
        {
            let m = self.nodes.get_mut(&id).expect("node exists");
            m.entity = entity;
            m.parent = parent;
            m.animates = animates;
            if m.style != style {
                self.patches.push(Patch::SetStyle {
                    id,
                    style: style.clone(),
                });
                m.style = style;
            }
            if m.text != text {
                self.patches.push(Patch::SetText {
                    id,
                    text: text.clone(),
                });
                m.text = text;
                // The field's caret may have moved with its text: what it
                // last reported no longer tells an echo (#86).
                m.shown_selection = None;
            }
            if let Some(scene) = scene {
                let flat = scene.flatten();
                let update = match &m.scene {
                    Some(old) => old.diff(&flat),
                    None => Some(crate::scene3d::SceneUpdate::Full(flat.clone())),
                };
                if let Some(update) = update {
                    self.patches.push(Patch::SetScene { id, update });
                }
                m.scene = Some(flat);
            }
            // A rich input's runs, after its text (#115).
            if let Some(runs) = rich_runs
                && m.runs != runs
            {
                self.patches.push(Patch::SetRichValue {
                    id,
                    runs: runs.clone(),
                });
                m.runs = runs;
                m.shown_selection = None;
            }
            if m.placeholder != placeholder {
                self.patches.push(Patch::SetPlaceholder {
                    id,
                    text: placeholder.clone(),
                });
                m.placeholder = placeholder;
            }
            if m.clickable != clickable {
                self.patches.push(Patch::SetClickable { id, clickable });
                m.clickable = clickable;
            }
            if m.icon != icon {
                self.patches.push(Patch::SetIcon {
                    id,
                    icon: icon.clone(),
                });
                m.icon = icon;
            }
            // A secure field hides its text: no one's caret shows in it (#240).
            let remote_carets = if kind == NodeKind::TextInput
                && !remote_carets.is_empty()
                && crate::controls::InputConfig::decode(&config).secure
            {
                Vec::new()
            } else {
                remote_carets
            };
            // Before the value: a slider clamps it to its range.
            if m.config != config {
                self.patches.push(Patch::SetConfig {
                    id,
                    config: config.clone(),
                });
                m.config = config;
            }
            // `NaN` (an empty optional date picker) equals itself here.
            if let Some(v) = value
                && !m
                    .value
                    .is_some_and(|old| old == v || (old.is_nan() && v.is_nan()))
            {
                self.patches.push(Patch::SetValue { id, value: v });
                m.value = Some(v);
            }
            // After the text (#86): applied when the app's value changes,
            // unless the field already shows it (the user made it).
            if m.selection != selection {
                if let Some(sel) = selection
                    && m.shown_selection != Some(sel)
                {
                    self.patches.push(Patch::SetSelection {
                        id,
                        start: sel.start,
                        end: sel.end,
                    });
                    m.shown_selection = Some(sel);
                }
                m.selection = selection;
            }
            if m.remote_carets != remote_carets {
                self.patches.push(Patch::SetRemoteCarets {
                    id,
                    carets: remote_carets.clone(),
                });
                m.remote_carets = remote_carets;
            }
            let reports = on_select.is_some();
            if m.select_events != reports {
                self.patches
                    .push(Patch::SetSelectEvents { id, on: reports });
                m.select_events = reports;
            }
        }
        match on_click {
            Some(h) => self.click.insert(id, h),
            None => self.click.remove(&id),
        };
        match on_input {
            Some(h) => self.input.insert(id, h),
            None => self.input.remove(&id),
        };
        match on_select {
            Some(h) => self.select.insert(id, h),
            None => self.select.remove(&id),
        };
        match on_rich_input {
            Some(h) => self.rich_input.insert(id, h),
            None => self.rich_input.remove(&id),
        };
        match on_back {
            Some(h) => self.back.insert(id, h),
            None => self.back.remove(&id),
        };
        match on_navigate {
            Some(h) => self.navigate.insert(id, h),
            None => self.navigate.remove(&id),
        };
        match on_change {
            Some(h) => self.change.insert(id, h),
            None => self.change.remove(&id),
        };
        match on_submit {
            Some(h) => self.submit.insert(id, h),
            None => self.submit.remove(&id),
        };
        match on_scene_tap {
            Some(h) => self.scene_tap.insert(id, h),
            None => self.scene_tap.remove(&id),
        };
        match on_web_message {
            Some(h) => self.web_message.insert(id, h),
            None => self.web_message.remove(&id),
        };
        match on_web_load {
            Some(h) => self.web_load.insert(id, h),
            None => self.web_load.remove(&id),
        };
        {
            let types = on_drop_files
                .as_ref()
                .map(|(accept, _)| accept.types.clone());
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.drop_types != types {
                m.drop_types = types.clone();
                self.patches.push(Patch::SetDropTarget { id, types });
            }
        }
        match on_drop_files {
            Some(d) => self.drop_files.insert(id, d),
            None => self.drop_files.remove(&id),
        };
        match on_drop_progress {
            Some(h) => self.drop_progress.insert(id, h),
            None => self.drop_progress.remove(&id),
        };
        match on_ended {
            Some(h) => self.ended.insert(id, h),
            None => self.ended.remove(&id),
        };

        // Children.
        let mut items = Vec::new();
        for child in children {
            self.flatten(app, child, &mut items);
        }
        let old_children = std::mem::take(&mut self.nodes.get_mut(&id).unwrap().children);
        let mut slots = Slots::new(&old_children);
        let mut new_children = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            let child = match item {
                Item::Cached(e) => match slots.take(&self.nodes, &Ident::Entity(e.id())) {
                    Some(n) => n,
                    None => {
                        // Moved to another parent: render it fresh here.
                        let v = self.render_entity(app, e);
                        self.diff(app, None, Some(id), v, Some(e))
                    }
                },
                Item::Node(v, e) => {
                    let found = match (&v.key, e) {
                        (Some(key), _) => slots.take(&self.nodes, &Ident::Key(key)),
                        (None, Some(e)) => slots.take(&self.nodes, &Ident::Entity(e.id())),
                        (None, None) => None,
                    }
                    .or_else(|| {
                        // Positional match among unkeyed, unowned nodes.
                        slots.take_at(i, |n| {
                            let m = &self.nodes[&n];
                            m.key.is_none() && m.entity.is_none()
                        })
                    });
                    self.diff(app, found, Some(id), v, e)
                }
            };
            new_children.push(child);
        }
        for leftover in slots.into_rest() {
            self.destroy(leftover);
        }
        if new_children != old_children {
            self.patches.push(Patch::SetChildren {
                id,
                children: new_children.clone(),
            });
        }
        self.nodes.get_mut(&id).unwrap().children = new_children;

        // Accessibility, with defaults resolved here once for every backend.
        let auto_label = kind == NodeKind::View && clickable && a11y.label.is_none();
        let effective = self.effective_a11y(id, a11y, clickable, value);
        let m = self.nodes.get_mut(&id).unwrap();
        m.auto_label = auto_label;
        if m.a11y != effective {
            self.patches.push(Patch::SetAccessibility {
                id,
                a11y: effective.clone(),
            });
            m.a11y = effective;
        }
        id
    }

    /// Island `id`'s element, changed props and event handlers.
    fn diff_island(&mut self, id: NodeId, spec: Option<Box<crate::island::IslandSpec>>) {
        let Some(spec) = spec else {
            return;
        };
        let crate::island::IslandSpec {
            tag,
            source,
            props,
            events,
        } = *spec;
        let names: Vec<String> = events.iter().map(|(n, _)| n.clone()).collect();
        self.island_events.retain(|(n, _), _| *n != id);
        for (name, h) in events {
            self.island_events.insert((id, name), h);
        }
        let m = self.nodes.get_mut(&id).expect("node exists");
        let mounted = m.island.get_or_insert_with(|| MIsland {
            tag: String::new(),
            source: crate::island::IslandSource::Page,
            props: HashMap::new(),
            events: Vec::new(),
        });
        if mounted.tag != tag || mounted.source != source || mounted.events != names {
            self.patches.push(Patch::SetIsland {
                id,
                tag: tag.clone(),
                source: source.clone(),
                events: names.clone(),
            });
            mounted.tag = tag;
            mounted.source = source;
            mounted.events = names;
        }
        let mut changed: Vec<(String, String)> = props
            .iter()
            .filter(|(k, v)| mounted.props.get(k) != Some(v))
            .cloned()
            .collect();
        let mut removed: Vec<String> = mounted
            .props
            .keys()
            .filter(|k| !props.iter().any(|(n, _)| n == *k))
            .cloned()
            .collect();
        removed.sort();
        for k in removed {
            mounted.props.remove(&k);
            changed.push((k, "null".into()));
        }
        for (k, v) in &props {
            mounted.props.insert(k.clone(), v.clone());
        }
        if !changed.is_empty() {
            self.patches
                .push(Patch::SetIslandProps { id, props: changed });
        }
    }

    /// Names, pointer and key handlers of node `id` (#17).
    #[allow(clippy::type_complexity)]
    fn diff_events(
        &mut self,
        id: NodeId,
        name: Option<String>,
        pointer: (
            Option<Handler<crate::events::HoverEvent>>,
            Option<Handler<crate::events::PointerEvent>>,
            Option<Handler<crate::events::DragEvent>>,
            u8,
        ),
        gestures: (
            Option<Handler<crate::events::WheelEvent>>,
            Option<Handler<crate::events::MagnifyEvent>>,
            Option<Handler<crate::element::ClickEvent>>,
        ),
        keys: (
            Option<Handler<crate::events::KeyEvent>>,
            Option<Handler<crate::events::KeyEvent>>,
            Option<Handler<crate::events::KeyEvent>>,
            Option<Handler<crate::events::KeyEvent>>,
        ),
    ) {
        use crate::events::pointer_mask::{AUX_CLICK, DRAG_ANY, HOVER, MAGNIFY, MOVE, WHEEL};
        let m = self.nodes.get_mut(&id).expect("node exists");
        if m.name != name {
            if let Some(old) = m.name.take()
                && self.names.get(&old) == Some(&id)
            {
                self.names.remove(&old);
            }
            m.name = name.clone();
        }
        if let Some(n) = name {
            self.names.insert(n, id);
        }
        let mask = if pointer.0.is_some() { HOVER } else { 0 }
            | if pointer.1.is_some() { MOVE } else { 0 }
            | if pointer.2.is_some() || self.drag_payloads.contains_key(&id) {
                pointer.3 & DRAG_ANY
            } else {
                0
            }
            | if gestures.0.is_some() { WHEEL } else { 0 }
            | if gestures.1.is_some() { MAGNIFY } else { 0 }
            | if gestures.2.is_some() { AUX_CLICK } else { 0 };
        if m.pointer_mask != mask {
            m.pointer_mask = mask;
            self.patches.push(Patch::SetPointerEvents { id, mask });
        }
        // Key ups (#159) come with the same reports as key downs.
        let flags = (
            keys.0.is_some() || keys.2.is_some(),
            keys.1.is_some() || keys.3.is_some(),
        );
        if m.keys != flags {
            m.keys = flags;
            self.patches.push(Patch::SetKeyEvents {
                id,
                local: flags.0,
                global: flags.1,
            });
        }
        fn set<E>(map: &mut HashMap<NodeId, Handler<E>>, id: NodeId, h: Option<Handler<E>>) {
            match h {
                Some(h) => map.insert(id, h),
                None => map.remove(&id),
            };
        }
        set(&mut self.hover, id, pointer.0);
        set(&mut self.pointer_move, id, pointer.1);
        set(&mut self.drag, id, pointer.2);
        set(&mut self.wheel, id, gestures.0);
        set(&mut self.magnify, id, gestures.1);
        set(&mut self.aux_click, id, gestures.2);
        set(&mut self.key_down, id, keys.0);
        set(&mut self.global_key, id, keys.1);
        set(&mut self.key_up, id, keys.2);
        set(&mut self.global_key_up, id, keys.3);
    }

    /// Recomputes the automatic labels of `from` and its ancestors (after a
    /// descendant entity re-rendered on its own).
    fn refresh_auto_labels(&mut self, from: Option<NodeId>) {
        let mut next = from;
        while let Some(id) = next {
            let Some(m) = self.nodes.get(&id) else { return };
            next = m.parent;
            if !m.auto_label {
                continue;
            }
            let label = Some(self.collect_text(id)).filter(|t| !t.is_empty());
            let m = self.nodes.get_mut(&id).expect("node exists");
            if m.a11y.label != label {
                m.a11y.label = label;
                self.patches.push(Patch::SetAccessibility {
                    id,
                    a11y: m.a11y.clone(),
                });
            }
        }
    }

    fn effective_a11y(
        &self,
        id: NodeId,
        mut a: Accessibility,
        clickable: bool,
        value: Option<f64>,
    ) -> Accessibility {
        let m = &self.nodes[&id];
        if a.role.is_none() {
            a.role = match m.kind {
                NodeKind::View if clickable => Some(Role::Button),
                NodeKind::Text if clickable => Some(Role::Button),
                kind => kind.default_role(),
            };
        }
        if matches!(m.kind, NodeKind::Toggle | NodeKind::Checkbox) && a.checked.is_none() {
            a.checked = value.map(|v| v != 0.);
        }
        if a.value.is_none() {
            a.value = match (m.kind, value) {
                (NodeKind::Slider, Some(v)) => Some(format_value(v)),
                (NodeKind::Picker, Some(v)) if v >= 0. => {
                    crate::controls::PickerConfig::decode(&m.config)
                        .options
                        .get(v as usize)
                        .cloned()
                }
                (NodeKind::RadioGroup, Some(v)) if v >= 0. => {
                    crate::controls::RadioConfig::decode(&m.config)
                        .options
                        .get(v as usize)
                        .cloned()
                }
                _ => None,
            };
        }
        if a.label.is_none() {
            a.label = match m.kind {
                // A clickable container reads as one button named by its text.
                NodeKind::View if clickable => {
                    Some(self.collect_text(id)).filter(|t| !t.is_empty())
                }
                NodeKind::TextInput
                | NodeKind::TextArea
                | NodeKind::RichInput
                | NodeKind::Picker
                | NodeKind::DatePicker
                    if !m.placeholder.is_empty() =>
                {
                    Some(m.placeholder.clone())
                }
                NodeKind::Tab | NodeKind::Screen | NodeKind::Checkbox if !m.text.is_empty() => {
                    Some(m.text.clone())
                }
                NodeKind::Overlay => None,
                _ => None,
            };
        }
        a
    }

    /// The visible text of a subtree, for automatic labels.
    fn collect_text(&self, id: NodeId) -> String {
        fn walk(r: &Reconciler, id: NodeId, out: &mut Vec<String>) {
            let Some(m) = r.nodes.get(&id) else { return };
            if m.a11y.hidden {
                return;
            }
            match m.kind {
                NodeKind::Text | NodeKind::Button if !m.text.trim().is_empty() => out.push(
                    m.a11y
                        .label
                        .clone()
                        .unwrap_or_else(|| m.text.trim().to_string()),
                ),
                _ => m.children.iter().for_each(|c| walk(r, *c, out)),
            }
        }
        let mut parts = Vec::new();
        for c in &self.nodes[&id].children {
            walk(self, *c, &mut parts);
        }
        parts.join(", ")
    }

    /// Registers the handlers of a rich text's clickable runs and returns
    /// its encoded runs, plus an accessibility label when some runs read
    /// differently than they show (icons, labelled runs).
    fn mount_spans(
        &mut self,
        id: NodeId,
        font_size: f32,
        spans: Vec<crate::spans::Span>,
    ) -> (String, Option<String>) {
        let mut runs = Vec::with_capacity(spans.len());
        let mut spoken = String::new();
        let mut relabelled = false;
        for (i, span) in spans.into_iter().enumerate() {
            let mut run = span.run;
            // Read-only, a chip shows its label (#115), an icon its
            // fallback glyph (#221).
            run.text = run.display_text().to_string();
            if let Some(size) = span.size.filter(|_| font_size > 0.) {
                run.size = Some(size / font_size);
            }
            let handler = span.on_click.or_else(|| run.href.clone().map(link_handler));
            if let Some(h) = handler {
                let item = crate::spans::link_id(id, i);
                self.menu_items.entry(id).or_default().push((item, h));
                run.link = Some(item);
            }
            match (&span.label, &run.icon) {
                (Some(label), _) => {
                    spoken.push_str(label);
                    relabelled = true;
                }
                (None, Some(_)) => relabelled = true,
                (None, None) => spoken.push_str(&run.text),
            }
            runs.push(run);
        }
        let label = relabelled
            .then(|| spoken.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty());
        (crate::spans::encode_runs(&runs), label)
    }

    /// Registers the handlers of a rich input's clickable runs (links,
    /// chips with `on_click`), remembers its spans to rebuild edits from,
    /// and returns its encoded runs (#115). Unlike a read-only text, a chip
    /// or an icon is [`OBJECT_CHAR`](crate::OBJECT_CHAR) there.
    fn mount_rich(&mut self, id: NodeId, font_size: f32, spans: Vec<crate::spans::Span>) -> String {
        let mut runs = Vec::with_capacity(spans.len());
        let mut keyed = Vec::with_capacity(spans.len());
        for (i, span) in spans.into_iter().enumerate() {
            let mut run = span.run.clone();
            if let Some(size) = span.size.filter(|_| font_size > 0.) {
                run.size = Some(size / font_size);
            }
            let handler = span
                .on_click
                .clone()
                .or_else(|| run.href.clone().map(link_handler));
            if let Some(h) = handler {
                let item = crate::spans::link_id(id, i);
                self.menu_items.entry(id).or_default().push((item, h));
                run.link = Some(item);
            }
            keyed.push((crate::spans::run_key(&run), span));
            runs.push(run);
        }
        self.rich.insert(
            id,
            RichState {
                spans: keyed,
                font_size,
            },
        );
        crate::spans::encode_runs(&runs)
    }

    fn create(&mut self, kind: NodeKind, key: Option<String>) -> NodeId {
        self.next_id += 1;
        let id = self.next_id;
        self.patches.push(Patch::Create { id, kind });
        self.nodes.insert(
            id,
            MNode {
                kind,
                key,
                entity: None,
                // NaN never compares equal, which forces the first SetStyle.
                style: Style {
                    opacity: f32::NAN,
                    ..Style::default()
                },
                text: String::new(),
                placeholder: String::new(),
                icon: String::new(),
                value: None,
                config: String::new(),
                a11y: Accessibility::default(),
                menu: None,
                swipe: (false, false),
                scroll_events: false,
                clickable: false,
                auto_label: false,
                parent: None,
                children: Vec::new(),
                pointer_mask: 0,
                drag_delay_ms: 0,
                drop_types: None,
                dnd: (None, Vec::new()),
                keys: (false, false),
                consume_keys: Vec::new(),
                layout_events: false,
                edit_mask: 0,
                selection_group: (false, false),
                selection: None,
                shown_selection: None,
                remote_carets: Vec::new(),
                select_events: false,
                name: None,
                island: None,
                link: None,
                animates: false,
                scene: None,
                attributes: Vec::new(),
                runs: String::new(),
            },
        );
        id
    }

    /// Whether `m`, matched by its place among its siblings, stays the node
    /// a vnode whose own style `animates` describes. An element that starts
    /// animating at an update, where an element that did not animate was,
    /// is a new element there (#50): it appears with its style at once, and
    /// only changes made to it later animate. Without this, it would
    /// animate from the other element's style (no background, no padding).
    /// A key or an entity says the node is the same one: it is kept.
    fn keeps_identity(&self, m: &MNode, animates: bool, entity: Option<&AnyEntity>) -> bool {
        let same_entity = matches!((&m.entity, entity), (Some(a), Some(b)) if a.id() == b.id());
        !animates || m.animates || m.key.is_some() || same_entity
    }

    fn destroy(&mut self, id: NodeId) {
        let Some(node) = self.nodes.remove(&id) else {
            return;
        };
        for child in node.children {
            self.destroy(child);
        }
        if let Some(e) = node.entity
            && self.entity_roots.get(&e.id()) == Some(&id)
        {
            self.entity_roots.remove(&e.id());
        }
        self.click.remove(&id);
        self.input.remove(&id);
        self.rich_input.remove(&id);
        self.rich.remove(&id);
        self.select.remove(&id);
        self.back.remove(&id);
        self.navigate.remove(&id);
        self.change.remove(&id);
        self.scene_tap.remove(&id);
        self.submit.remove(&id);
        self.web_message.remove(&id);
        self.web_load.remove(&id);
        self.drop_files.remove(&id);
        self.drop_progress.remove(&id);
        self.drag_payloads.remove(&id);
        self.drop_payload.remove(&id);
        self.drag_over.remove(&id);
        self.ended.remove(&id);
        self.menu_items.remove(&id);
        self.scroll.remove(&id);
        self.swipe.remove(&(id, SwipeDirection::Left));
        self.swipe.remove(&(id, SwipeDirection::Right));
        self.hover.remove(&id);
        self.pointer_move.remove(&id);
        self.drag.remove(&id);
        self.wheel.remove(&id);
        self.magnify.remove(&id);
        self.aux_click.remove(&id);
        self.focus_change.remove(&id);
        self.edit.remove(&id);
        self.groups.remove(&id);
        if self
            .across
            .as_ref()
            .is_some_and(|a| a.group == id || a.anchor.0 == id || a.head.0 == id)
        {
            self.across = None;
        }
        self.layout.remove(&id);
        self.key_down.remove(&id);
        self.global_key.remove(&id);
        self.key_up.remove(&id);
        self.global_key_up.remove(&id);
        if node.island.is_some() {
            self.island_events.retain(|(n, _), _| *n != id);
        }
        if let Some(name) = node.name
            && self.names.get(&name) == Some(&id)
        {
            self.names.remove(&name);
        }
        self.destroyed.push(id);
    }
}

/// What activating a link to `path` does: navigate in the app, or open a URL.
fn link_handler(path: String) -> Handler<ClickEvent> {
    std::rc::Rc::new(move |_: &ClickEvent, app: &mut App| {
        if crate::spans::is_external(&path) {
            app.open_url(path.clone());
        } else {
            app.navigate(path.clone());
        }
    })
}

/// A slider's value as read out: `3`, `0.25`, `12.5`.
fn format_value(v: f64) -> String {
    if v.fract() == 0. && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Whether `after` is `before` with one line break typed in it: Enter in a
/// plain multi-line field standing in for a rich input (#115).
fn is_line_break_typed(before: &str, after: &str) -> bool {
    let (b, a): (Vec<char>, Vec<char>) = (before.chars().collect(), after.chars().collect());
    if a.len() != b.len() + 1 {
        return false;
    }
    let p = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
    a[p] == '\n' && a[p + 1..] == b[p..]
}

/// A field's own keyboard bar (#242): a [`keyboard_toolbar`](crate::keyboard_toolbar)
/// serving only the field it follows, keyed after it.
fn own_keyboard_bar(field: &VNode, bar: Element) -> Element {
    use crate::IntoElement;
    let mut o = crate::keyboard_toolbar(bar).into_element();
    if let Element::Node(n) = &mut o {
        let mut c = crate::controls::OverlayConfig::decode(&n.config);
        c.field_only = true;
        n.config = c.encode();
        n.key = field.key.as_ref().map(|k| format!("{k}\u{1}keyboard"));
    }
    o
}

// --- One selection across several fields (#231) ----------------------------

type Field = crate::selection_across::Field<NodeId>;
type Pos = crate::selection_across::Pos;

impl Reconciler {
    /// The selection group text field `field` belongs to, if any.
    pub(crate) fn group_of(&self, field: NodeId) -> Option<NodeId> {
        let n = self.nodes.get(&field)?;
        if !matches!(
            n.kind,
            NodeKind::RichInput | NodeKind::TextArea | NodeKind::TextInput
        ) {
            return None;
        }
        let mut next = n.parent;
        while let Some(id) = next {
            if self.groups.contains_key(&id) {
                return Some(id);
            }
            next = self.nodes.get(&id)?.parent;
        }
        None
    }

    /// The value of text field `id`, as last rendered.
    fn field_value(&self, id: NodeId) -> String {
        match self.rich.get(&id) {
            Some(r) => r.spans.iter().map(|(_, s)| s.text()).collect(),
            None => self
                .nodes
                .get(&id)
                .map(|n| n.text.clone())
                .unwrap_or_default(),
        }
    }

    /// Characters `start..end` of field `id` as copied text (a chip as its
    /// label).
    fn field_part(&self, id: NodeId, start: usize, end: usize) -> String {
        if self.rich.contains_key(&id) {
            let spans = self.rich_spans(id);
            let (_, rest) = crate::spans::split_spans(&spans, start);
            let (mid, _) = crate::spans::split_spans(&rest, end.saturating_sub(start));
            mid.iter()
                .map(|s| s.chip_label().unwrap_or(s.text()))
                .collect()
        } else {
            self.field_value(id)
                .chars()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect()
        }
    }

    /// The text fields of group `group`, in document order.
    pub(crate) fn group_fields(&self, group: NodeId) -> Vec<Field> {
        fn walk(r: &Reconciler, id: NodeId, out: &mut Vec<Field>) {
            let Some(n) = r.nodes.get(&id) else { return };
            if matches!(
                n.kind,
                NodeKind::RichInput | NodeKind::TextArea | NodeKind::TextInput
            ) {
                // Every text field takes part (the web client cannot tell
                // the named ones); an unnamed one is `""` to the app.
                out.push(Field {
                    id,
                    name: n.name.clone().unwrap_or_default(),
                    len: r.field_value(id).chars().count(),
                });
                return;
            }
            for c in &n.children {
                walk(r, *c, out);
            }
        }
        let mut out = Vec::new();
        if let Some(n) = self.nodes.get(&group) {
            for c in &n.children {
                walk(self, *c, &mut out);
            }
        }
        out
    }

    fn pos_of(fields: &[Field], (id, offset): (NodeId, usize)) -> Option<Pos> {
        let i = fields.iter().position(|f| f.id == id)?;
        Some((i, offset.min(fields[i].len)))
    }

    /// The selection `anchor`..`head` as the app sees it.
    fn across_event(&self, fields: &[Field], anchor: Pos, head: Pos) -> crate::SelectionAcross {
        use crate::selection_across::{FieldSelection, SelectionPoint, parts};
        let point = |(i, o): Pos| SelectionPoint::new(fields[i].name.clone(), o);
        let mut e = crate::SelectionAcross::new(point(anchor), point(head));
        e.fields = parts(fields, anchor, head)
            .into_iter()
            .map(|(i, start, end)| FieldSelection {
                field: fields[i].name.clone(),
                start,
                end,
                text: self.field_part(fields[i].id, start, end),
            })
            .collect();
        e
    }

    /// The selection across group `group`'s fields now, as positions.
    fn across_range(&self, group: NodeId, fields: &[Field]) -> Option<(Pos, Pos)> {
        let a = self.across.as_ref().filter(|a| a.group == group)?;
        Some((
            Self::pos_of(fields, a.anchor)?,
            Self::pos_of(fields, a.head)?,
        ))
    }

    /// Shows `range` as the selection of group `group` (`None`: ends it;
    /// both ends in one field: a selection given back to that field).
    /// With `emit`, returns the patches that draw it; without, the backend
    /// drew it itself (the web client).
    fn set_across(
        &mut self,
        group: NodeId,
        range: Option<(Pos, Pos)>,
        fields: &[Field],
        emit: bool,
    ) -> Vec<Patch> {
        let mut patches = Vec::new();
        let old = self.across.take().map(|a| a.painted).unwrap_or_default();
        let mut painted = HashMap::new();
        let mut shown = None;
        match range {
            Some((a, h)) if a.0 != h.0 => {
                let mut selected = (0, 0);
                for (i, start, end) in crate::selection_across::parts(fields, a, h) {
                    if i == a.0 {
                        selected = (start, end);
                    } else {
                        painted.insert(fields[i].id, (start, end));
                    }
                }
                shown = Some(((fields[a.0].id, a.1), (fields[h.0].id, h.1)));
                self.across = Some(Across {
                    group,
                    anchor: (fields[a.0].id, a.1),
                    head: (fields[h.0].id, h.1),
                    painted: painted.clone(),
                    selected,
                });
                if emit {
                    patches.push(Patch::SetSelection {
                        id: fields[a.0].id,
                        start: selected.0,
                        end: selected.1,
                    });
                }
            }
            Some((a, h)) if emit => {
                let (start, end) = (a.1.min(h.1), a.1.max(h.1));
                patches.push(Patch::SetSelection {
                    id: fields[a.0].id,
                    start,
                    end,
                });
            }
            _ => {}
        }
        if emit {
            let mut ids: Vec<NodeId> = old.keys().chain(painted.keys()).copied().collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                let now = painted.get(&id).copied();
                if old.get(&id).copied() != now {
                    let (start, end) = now.unwrap_or((0, 0));
                    patches.push(Patch::SetSelectionHighlight { id, start, end });
                }
            }
            patches.push(Patch::SetSelectionAcross {
                group,
                range: shown,
            });
        }
        patches
    }

    /// What `key`, pressed in text field `focused`, does to its selection
    /// group (#231), if anything: the key is then the group's.
    pub(crate) fn across_key(
        &self,
        focused: Option<NodeId>,
        key: &crate::events::KeyEvent,
    ) -> Option<(NodeId, Vec<Field>, crate::selection_across::KeyAction)> {
        let f = focused?;
        let group = self.group_of(f)?;
        let fields = self.group_fields(group);
        let idx = fields.iter().position(|x| x.id == f)?;
        let across = self.across_range(group, &fields);
        let edits = self.groups.get(&group)?.on_edit.is_some();
        let action = crate::selection_across::key_action(&fields, across, idx, key, edits)?;
        Some((group, fields, action))
    }

    /// Carries out `action` (from [`across_key`](Self::across_key)) in
    /// group `group`, pressed in field `focused`.
    pub(crate) fn apply_across(
        &mut self,
        group: NodeId,
        fields: &[Field],
        focused: NodeId,
        action: crate::selection_across::KeyAction,
    ) -> AcrossOutcome {
        use crate::selection_across::KeyAction as K;
        let mut out = AcrossOutcome::default();
        let handlers = self.groups.get(&group).cloned().unwrap_or_default();
        match action {
            K::Select(a, h) => {
                out.patches = self.set_across(group, Some((a, h)), fields, true);
                out.report = handlers
                    .on_selection
                    .map(|hd| (hd, self.across_event(fields, a, h)));
            }
            K::Caret(p) => {
                let id = fields[p.0].id;
                if id != focused {
                    out.patches.push(Patch::Focus { id });
                }
                out.patches
                    .extend(self.set_across(group, Some((p, p)), fields, true));
                out.report = handlers
                    .on_selection
                    .map(|hd| (hd, self.across_event(fields, p, p)));
            }
            K::Copy { cut } => {
                let Some((a, h)) = self.across_range(group, fields) else {
                    return out;
                };
                out.copy = Some(self.copy_across(fields, a, h, cut));
                if cut {
                    out.edit = handlers.on_edit.map(|hd| {
                        let e = crate::EditAcross::new(
                            self.across_event(fields, a, h),
                            "",
                            crate::EditCause::Cut,
                        );
                        (hd, e)
                    });
                    if out.edit.is_some() {
                        out.patches = self.set_across(group, None, fields, true);
                    }
                }
            }
            K::Paste => {
                let Some((a, h)) = self.across_range(group, fields) else {
                    return out;
                };
                out.paste = handlers
                    .on_edit
                    .map(|hd| (hd, self.across_event(fields, a, h)));
                out.patches = self.set_across(group, None, fields, true);
            }
            K::Edit(cause, text) => {
                let Some((a, h)) = self.across_range(group, fields) else {
                    return out;
                };
                out.edit = handlers.on_edit.map(|hd| {
                    (
                        hd,
                        crate::EditAcross::new(self.across_event(fields, a, h), text, cause),
                    )
                });
                out.patches = self.set_across(group, None, fields, true);
            }
        }
        out
    }

    /// The copy of `anchor`..`head`, for the field where it started.
    fn copy_across(
        &self,
        fields: &[Field],
        anchor: Pos,
        head: Pos,
        cut: bool,
    ) -> (NodeId, Option<Handler<crate::CopyEvent>>, crate::CopyEvent) {
        let event = self.across_event(fields, anchor, head);
        let id = fields[anchor.0].id;
        let (start, end) = event
            .fields
            .iter()
            .find(|f| f.field == fields[anchor.0].name)
            .map_or((anchor.1, anchor.1), |f| (f.start, f.end));
        let part = self.field_part(id, start, end);
        let copy = crate::CopyEvent::new(
            SelectionEvent::new(start, end),
            part,
            &self.rich_spans(id),
            cut,
        )
        .with_across(event);
        let handler = self.edit.get(&id).and_then(|h| {
            if cut {
                h.on_cut.clone()
            } else {
                h.on_copy.clone()
            }
        });
        (id, handler, copy)
    }

    /// Text field `id` reported `start..end` (#231): unless it echoes the
    /// selection Prism gave it, a selection across its group ends there.
    pub(crate) fn across_select(&mut self, id: NodeId, start: usize, end: usize) -> AcrossOutcome {
        let mut out = AcrossOutcome::default();
        let Some(a) = &self.across else { return out };
        let group = a.group;
        if (a.anchor.0 == id && a.selected == (start, end)) || self.group_of(id) != Some(group) {
            return out;
        }
        let fields = self.group_fields(group);
        out.patches = self.set_across(group, None, &fields, true);
        if let (Some(hd), Some(i)) = (
            self.groups.get(&group).and_then(|g| g.on_selection.clone()),
            fields.iter().position(|f| f.id == id),
        ) {
            out.report = Some((hd, self.across_event(&fields, (i, start), (i, end))));
        }
        out
    }

    /// What `text`, typed into field `field` by an input method (#231,
    /// `UiEvent::TypeAcross`), does: an edit of the selection across its
    /// group's fields, when there is one and the group has
    /// `on_edit_across`. `None`: the field inserts it.
    pub(crate) fn across_text(
        &self,
        field: NodeId,
        text: &str,
    ) -> Option<(NodeId, Vec<Field>, crate::selection_across::KeyAction)> {
        use crate::EditCause as C;
        let group = self.group_of(field)?;
        self.groups.get(&group)?.on_edit.as_ref()?;
        let fields = self.group_fields(group);
        self.across_range(group, &fields)?;
        let (cause, text) = match text {
            "" => (C::Backspace, String::new()),
            "\n" | "\r" | "\r\n" => (C::Enter, "\n".to_string()),
            t => (C::Typing, t.to_string()),
        };
        Some((
            group,
            fields,
            crate::selection_across::KeyAction::Edit(cause, text),
        ))
    }

    /// A backend extended the selection across group `group` to `range`
    /// (#231): the web client draws it itself; native backends (a mouse
    /// drag, #264) get the selection and the highlights to draw (`draw`).
    pub(crate) fn across_reported(
        &mut self,
        group: NodeId,
        range: Option<((NodeId, usize), (NodeId, usize))>,
        draw: bool,
    ) -> AcrossOutcome {
        let fields = self.group_fields(group);
        let pos =
            range.and_then(|(a, h)| Some((Self::pos_of(&fields, a)?, Self::pos_of(&fields, h)?)));
        let patches = self.set_across(group, pos, &fields, draw);
        let report = pos.and_then(|(a, h)| {
            let hd = self.groups.get(&group)?.on_selection.clone()?;
            Some((hd, self.across_event(&fields, a, h)))
        });
        AcrossOutcome {
            patches,
            report,
            ..Default::default()
        }
    }

    /// The web client took an edit (or a copy, `copy = Some(cut)`) over the
    /// selection across group `group` (#231).
    pub(crate) fn across_client_edit(
        &mut self,
        group: NodeId,
        edit: Option<(crate::EditCause, String)>,
        copy: Option<bool>,
    ) -> AcrossOutcome {
        let fields = self.group_fields(group);
        let mut out = AcrossOutcome::default();
        let Some((a, h)) = self.across_range(group, &fields) else {
            return out;
        };
        let on_edit = self.groups.get(&group).and_then(|g| g.on_edit.clone());
        if let Some(cut) = copy {
            out.copy = Some(self.copy_across(&fields, a, h, cut));
            if cut {
                out.edit = on_edit.map(|hd| {
                    let e = crate::EditAcross::new(
                        self.across_event(&fields, a, h),
                        "",
                        crate::EditCause::Cut,
                    );
                    (hd, e)
                });
                self.set_across(group, None, &fields, false);
            }
        } else if let Some((cause, text)) = edit {
            out.edit = on_edit.map(|hd| {
                (
                    hd,
                    crate::EditAcross::new(self.across_event(&fields, a, h), text, cause),
                )
            });
            self.set_across(group, None, &fields, false);
        }
        out
    }

    /// `cx.set_selection_across` / `clear_selection_across` (#231).
    fn command_across(
        &mut self,
        range: Option<(crate::SelectionPoint, crate::SelectionPoint)>,
    ) -> Vec<Patch> {
        match range {
            None => {
                let Some(group) = self.across.as_ref().map(|a| a.group) else {
                    return Vec::new();
                };
                let fields = self.group_fields(group);
                self.set_across(group, None, &fields, true)
            }
            Some((anchor, head)) => {
                let (Some(a), Some(h)) =
                    (self.node_named(&anchor.field), self.node_named(&head.field))
                else {
                    return Vec::new();
                };
                let Some(group) = self.group_of(a).filter(|g| self.group_of(h) == Some(*g)) else {
                    return Vec::new();
                };
                let fields = self.group_fields(group);
                let (Some(pa), Some(ph)) = (
                    Self::pos_of(&fields, (a, anchor.offset)),
                    Self::pos_of(&fields, (h, head.offset)),
                ) else {
                    return Vec::new();
                };
                let mut patches = vec![Patch::Focus { id: a }];
                patches.extend(self.set_across(group, Some((pa, ph)), &fields, true));
                patches
            }
        }
    }
}
