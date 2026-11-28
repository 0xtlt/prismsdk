//! # prism-native
//!
//! Everything native backends have in common. A platform only implements the
//! small [`Widgets`] trait (create a view, set its text, measure it, place it);
//! [`NativeRenderer`] turns reconciler patches into those calls and runs the
//! flexbox layout with taffy, so AppKit, UIKit, GTK, Android and WinUI views
//! all lay out exactly like the web backend's CSS flexbox.

#[cfg(target_vendor = "apple")]
pub mod apple;
pub mod chips;
pub mod dnd;
mod extras;
pub mod fields;
pub mod fonts;
#[cfg(feature = "gl3d")]
pub mod gl3d;
pub mod images;
mod leftovers;
pub mod picture_cache;
pub mod remote;
pub mod save;
pub mod table;
pub mod text;

pub use extras::Sticky;

/// `init` with the screen's pixels per point set first
/// (`cx.scale_factor()`, #343): the app's first render has it, and its
/// `on_resize` handlers don't run for it. Later changes go through
/// `Runtime::set_scale_factor`.
pub fn with_scale_factor(init: &prism_core::AppInit, scale: f32) -> prism_core::AppInit {
    let init = init.clone();
    std::sync::Arc::new(move |app: &mut prism_core::App| {
        app.set_scale_factor(scale);
        init(app)
    })
}

#[cfg(test)]
mod bench;
#[cfg(test)]
mod padding_tests;
#[cfg(test)]
mod table_tests;
#[cfg(test)]
mod temp_dir;

use prism_core::{
    Accessibility, Align, Appearance, Backend, Direction, FlexDirection, IdMap, IdSet, Justify,
    Length, NodeId, NodeKind, Patch, Pose, RenderEnvironment, Style, TextAlign, Transition,
    animation::{DEFAULT_ENTER, DEFAULT_EXIT},
};
use taffy::TraversePartialTree;
use taffy::prelude::{
    AvailableSpace, Dimension, LengthPercentage, LengthPercentageAuto, Rect, Size, TaffyTree,
};

/// The platform-specific half of a native backend.
pub trait Widgets {
    /// A cheap, clonable handle to a native view.
    type View: Clone;

    fn create(&mut self, id: NodeId, kind: NodeKind) -> Self::View;
    fn set_style(&mut self, view: &Self::View, style: &Style);
    fn set_text(&mut self, view: &Self::View, text: &str);
    fn set_placeholder(&mut self, view: &Self::View, text: &str);
    fn set_clickable(&mut self, view: &Self::View, clickable: bool);
    /// Make `children` the subviews of `parent`, in order: their paint
    /// order (by `z_index`, #232).
    fn set_children(&mut self, parent: &Self::View, children: &[Self::View]);
    /// The same children in the tree's order, called right after
    /// `set_children` when their paint order differs (a `z_index`): the
    /// order screen readers read them in and the focus moves through them,
    /// which a `z_index` must not change. A `set_children` with no call
    /// after it is in the tree's order. Backends whose accessibility and
    /// focus order follow their subviews' order keep the tree's order and
    /// paint by the other.
    fn set_reading_order(&mut self, _parent: &Self::View, _children: &[Self::View]) {}
    /// A table's rows or cells changed (#284), after their
    /// `set_children` and `set_accessibility`: `rows` are its rows in
    /// order, the header row first, each with its cells. Backends whose
    /// screen readers know tables give the table its rows, columns and
    /// column headers, and each cell its row and column, so a reader
    /// announces a cell with its column's header. Default: nothing (the
    /// roles alone, from `set_accessibility`).
    fn set_table(&mut self, _table: &Self::View, _rows: &[table::TableRow<'_, Self::View>]) {}
    /// The element `view` shows, hides or changes
    /// (`accessibility_controls`, #291), after its `set_accessibility`;
    /// `None`: none any more. Backends whose screen readers know the link
    /// make it (macOS: linked elements, Linux: the controls relation,
    /// Windows: controlled peers). Default: nothing.
    fn set_controls(&mut self, _view: &Self::View, _target: Option<&Self::View>) {}
    /// Mount `view` as the root, replacing the previous one.
    fn set_root(&mut self, view: &Self::View);
    fn destroy(&mut self, view: &Self::View);
    /// Natural size of a leaf (text, button, input) given an optional width limit.
    fn measure(&mut self, view: &Self::View, max_width: Option<f32>) -> (f32, f32);
    /// Narrowest width a leaf can take (CSS min-content: the longest word of
    /// a text). Default: its natural single-line width.
    fn measure_min_content(&mut self, view: &Self::View) -> f32 {
        self.measure(view, None).0
    }
    /// Place a view in its parent's coordinate space (top-left origin).
    fn set_frame(&mut self, view: &Self::View, x: f32, y: f32, w: f32, h: f32);
    /// For platform-managed containers (stacks, tab bars): where `container`
    /// shows `child` (in the coordinates of the child's superview) and what its
    /// content must avoid (navigation bar, tab bar, safe area). Called after
    /// the container got its frame.
    fn screen_viewport(&mut self, container: &Self::View, child: &Self::View) -> Viewport;
    /// Apply the resolved accessibility description (role, label, state…).
    /// Required: every backend must expose its views to assistive technologies.
    fn set_accessibility(&mut self, view: &Self::View, a11y: &Accessibility);
    /// Toggle state (0/1) or selected tab index.
    fn set_value(&mut self, view: &Self::View, value: f64);
    /// What a shape draws, in points for its current size (arcs already
    /// turned into curves; colors may adapt to dark mode). Called when the
    /// drawing or the shape's size changes.
    fn set_drawing(&mut self, _view: &Self::View, _paths: &[prism_core::Path]) {}
    /// What a shape draws when it is more than paths (an SVG document's
    /// text, images, and groups with an opacity, a clip, a mask or
    /// shadows), in points for its current size: paths as
    /// [`set_drawing`](Self::set_drawing) gets them, text and images with
    /// their transform to points. A backend that does not draw them gets
    /// the paths alone (`prism_core::shape::flatten`).
    fn set_drawing_items(&mut self, view: &Self::View, items: &[prism_core::shape::Item]) {
        self.set_drawing(view, &prism_core::shape::flatten(items));
    }
    /// What a 3D scene shows: the update (the whole scene the first time,
    /// then what changed) and its JSON (hosts in other languages read the
    /// JSON; see `prism_core::scene3d::wire`). Apply it to the scene's
    /// player (`ScenePlayer::apply`); fetch the blobs it names by id.
    fn set_scene(
        &mut self,
        _view: &Self::View,
        _update: &prism_core::scene3d::SceneUpdate,
        _json: &str,
    ) {
    }
    /// Control configuration (see `prism_core::controls`), before the value:
    /// `SliderConfig` (slider), `PickerConfig` (picker), `DateConfig` (date
    /// picker), `InputConfig` (text input, text area). Decode it with the
    /// matching `decode`.
    fn set_config(&mut self, _view: &Self::View, _kind: NodeKind, _config: &str) {}
    /// Tab icon (SF Symbol name on Apple platforms).
    fn set_icon(&mut self, _view: &Self::View, _icon: &str) {}
    /// Context menu of a view (`None` removes it). Items report
    /// `UiEvent::MenuSelect(id)`.
    fn set_menu(&mut self, _view: &Self::View, _menu: Option<&prism_core::menu::MenuSpec>) {}
    /// Opens a pop-up menu (#247) over `view` (the window's content when
    /// `None`): at `at`, in the platform's units from the view's top-left
    /// corner, or below the view when `at` is `None`. Items report
    /// `UiEvent::MenuSelect(id)`. Default: nothing (tvOS, watchOS).
    fn show_menu(
        &mut self,
        _view: Option<&Self::View>,
        _menu: &prism_core::menu::MenuSpec,
        _at: Option<(f32, f32)>,
    ) {
    }
    /// Which horizontal swipes a view reports (`UiEvent::Swipe`).
    fn set_swipe(&mut self, _view: &Self::View, _left: bool, _right: bool) {}
    /// Whether a scroll view reports `UiEvent::Scroll`.
    fn set_scroll_events(&mut self, _view: &Self::View, _on: bool) {}
    /// Light/dark native chrome.
    fn set_appearance(&mut self, _appearance: Appearance) {}
    /// `display: none` (also hides it from assistive technologies).
    fn set_hidden(&mut self, _view: &Self::View, _hidden: bool) {}
    /// Scrollable extent of a scroll view's content.
    fn set_content_size(&mut self, _view: &Self::View, _width: f32, _height: f32) {}
    /// Like `set_frame`, animated (only called when the frame changes).
    fn set_frame_animated(
        &mut self,
        view: &Self::View,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        _transition: Transition,
    ) {
        self.set_frame(view, x, y, w, h);
    }
    /// Like `set_style`, animating opacity and colors.
    fn set_style_animated(&mut self, view: &Self::View, style: &Style, _transition: Transition) {
        self.set_style(view, style);
    }
    /// Language and layout direction of native chrome (navigation bars…).
    fn set_environment(&mut self, _env: &RenderEnvironment) {}
    /// `view` is about to leave `parent` (and be destroyed): show it animating
    /// to `to` (its style at the exit pose), e.g. with a snapshot. Default:
    /// it disappears at once.
    fn animate_exit(
        &mut self,
        _parent: &Self::View,
        _view: &Self::View,
        _to: &Style,
        _t: Transition,
    ) {
    }
    /// Loops `view` between `style` (its opacity and transform) and `pose`,
    /// back and forth, until called with `None`.
    fn set_repeat(
        &mut self,
        _view: &Self::View,
        _style: &Style,
        _repeat: Option<(Pose, Transition)>,
    ) {
    }
    /// Distance from the top of a measured leaf (`width` × `height`, view
    /// units) to the baseline of its first line of text, for
    /// `items_baseline()`. `None`: estimated from the font size (text at
    /// the top, controls centered); views without text align their bottom.
    fn baseline(&mut self, _view: &Self::View, _width: f32, _height: f32) -> Option<f32> {
        None
    }
    /// `view` is `.sticky()` in the scroll view `scroll`: as the content
    /// scrolls, move it down by [`Sticky::offset`] (without changing its
    /// layout) and draw it above its siblings. `None`: not sticky anymore
    /// (back at its frame). Frames set later are unscrolled: keep adding the
    /// current offset.
    fn set_sticky(&mut self, _view: &Self::View, _sticky: Option<(&Self::View, Sticky)>) {}
    /// The interaction states `view` has style refinements for (called when
    /// that set changes): report them with
    /// [`NativeRenderer::set_interaction`] (pointer over it, pressed,
    /// keyboard focus; focus-within is derived). Backends that resolve
    /// states themselves (GTK CSS) read `style.states` instead.
    fn set_interactions(&mut self, _view: &Self::View, _states: prism_core::Interactions) {}

    // --- Pointer, keys, focus, overlays, layers (#17, #19, #25) ---

    /// Which pointer events a view reports (`prism_core::events::pointer_mask`
    /// bits): `UiEvent::Pointer` for hover / move, `UiEvent::Drag`.
    fn set_pointer_events(&mut self, _view: &Self::View, _mask: u8) {}
    /// How long a touch must be held still before the view's drag starts
    /// (`drag_after_long_press`, #243), in milliseconds; 0 on movement as
    /// usual. Mice and trackpads still drag on movement. Backends without
    /// touch screens ignore it.
    fn set_drag_delay(&mut self, _view: &Self::View, _ms: u32) {}
    /// Whether files can be dropped on the view (#111): the types it
    /// accepts (MIME types or extensions, empty for any file), `None` for
    /// none. Report `UiEvent::DropFiles` with copies the app can read, and
    /// [`NativeRenderer::set_interaction`] with `DropHover` while accepted
    /// files are over it.
    fn set_drop_target(&mut self, _view: &Self::View, _types: Option<&[String]>) {}

    // --- Drag and drop between elements (#251, see `dnd`) ---

    /// What kind of payload `view` carries when dragged (`None`: not
    /// draggable), its preview, and the kinds it takes dropped. Backends
    /// that locate drags through [`NativeRenderer::locate_drag`] need
    /// nothing here; a host in another process that hit-tests itself does.
    fn set_drag_and_drop(
        &mut self,
        _view: &Self::View,
        _kind: Option<&str>,
        _preview: prism_core::DragPreview,
        _accepts: &[String],
    ) {
    }
    /// Where `view`'s top-left corner is in the window, as drawn (scrolling
    /// included), in view units. `None`: the backend does not locate drags
    /// ([`NativeRenderer::locate_drag`] leaves them as they are).
    fn window_origin(&mut self, _view: &Self::View) -> Option<(f32, f32)> {
        None
    }
    /// The innermost node whose view is under the window point (view
    /// units), by the platform's hit-testing; never the drag preview.
    fn node_at(&mut self, _x: f32, _y: f32) -> Option<NodeId> {
        None
    }
    /// How far a scroll view's content is scrolled (view units).
    fn scroll_offset(&mut self, _view: &Self::View) -> (f32, f32) {
        (0., 0.)
    }
    /// Shows a see-through snapshot of `view` above everything, its
    /// top-left corner at the window point `at` (view units), taken when
    /// first shown and moved after; `None` removes it. It lets the pointer
    /// through.
    fn set_drag_preview(&mut self, _view: &Self::View, _at: Option<(f32, f32)>) {}
    /// A dragged payload `view` takes is over it (or not anymore): for
    /// backends whose styles follow a platform state (GTK's `:drop(active)`);
    /// the renderer also sets [`prism_core::Interaction::DropHover`].
    fn set_drop_hover(&mut self, _view: &Self::View, _on: bool) {}
    /// Whether key presses are reported (`UiEvent::Key`): `local` while the
    /// view or a descendant has the focus (the view becomes focusable),
    /// `global` wherever the focus is.
    fn set_key_events(&mut self, _view: &Self::View, _local: bool, _global: bool) {}
    /// Keys taken from the focused control in the view (`consume_keys`,
    /// #181). Backends that dispatch keys synchronously ask
    /// `Runtime::consumes_key` instead; hosts in another process need them.
    fn set_consumed_keys(&mut self, _view: &Self::View, _keys: &[String]) {}
    /// Whether the backend edits rich text (#115): it creates
    /// [`NodeKind::RichInput`] views and gets their runs in
    /// [`set_rich_value`](Self::set_rich_value). Otherwise (the default) a
    /// rich input is created as a [`NodeKind::TextArea`] of its plain text,
    /// whose edits (`UiEvent::Input`) keep the styles of the text around
    /// them.
    fn rich_input(&self) -> bool {
        false
    }
    /// The runs of a rich input (#115, [`prism_core::decode_runs`]; a chip
    /// is [`prism_core::OBJECT_CHAR`]), after its plain text (`set_text`).
    /// Runs whose text the field already shows are an echo of the user's
    /// typing, or the app restyling it: apply the attributes in place,
    /// keeping the caret. Report edits as `UiEvent::RichInput` (the runs
    /// shown) or `UiEvent::Input` (the plain text).
    fn set_rich_value(&mut self, _view: &Self::View, _runs: &str) {}
    /// Selects `start..end` (characters, clamped to the text) in a text
    /// input or area (#86), focused or not. Sent after its text.
    fn set_selection(&mut self, _view: &Self::View, _start: usize, _end: usize) {}
    /// Whether a text input or area reports its selection and caret
    /// (`UiEvent::Select`, in characters) when they move, typing included.
    fn set_select_events(&mut self, _view: &Self::View, _on: bool) {}

    /// Draws other people's carets and selections in a text input, area
    /// or rich input (#240), above its text, offsets in characters; an
    /// empty list removes them. Never touches the field's own caret,
    /// selection or composition. Default: none drawn.
    fn set_remote_carets(&mut self, _view: &Self::View, _carets: &[prism_core::RemoteCaret]) {}
    /// Which editing events a rich input reports (#217): bits of
    /// [`prism_core::edit_events`]. With `COPY`, `CUT` or `PASTE`, the
    /// field writes and inserts nothing itself: it reports
    /// `UiEvent::Copy` / `UiEvent::Paste` (a cut still deletes the
    /// selection) and the answer follows (`write_clipboard`,
    /// `insert_text`). With `SELECTION_RECT` it reports
    /// `UiEvent::SelectionRect` as its selection moves; with `SPAN_CLICK`,
    /// clicks with ⌘, Ctrl or ⌥ (`UiEvent::SpanClick`).
    fn set_edit_events(&mut self, _view: &Self::View, _mask: u8) {}
    /// Writes `content` to the clipboard, every type as one item (#217).
    fn write_clipboard(&mut self, _view: &Self::View, _content: &prism_core::ClipboardContent) {}
    /// Inserts `text` over the field's selection like a plain-text paste
    /// (#217), then reports the edit as usual.
    fn insert_text(&mut self, _view: &Self::View, _text: &str) {}
    /// Draws `start..end` (characters) of a text field highlighted like a
    /// selection, without selecting it (#231): its part of a selection
    /// across several fields while another field has the focus.
    /// `start == end` removes it. Leaves the text, the caret and the
    /// field's own selection alone.
    fn set_selection_highlight(&mut self, _view: &Self::View, _start: usize, _end: usize) {}
    /// Whether the view is a selection group (#231): a mouse drag that
    /// starts in one of its text fields extends the selection over the
    /// others, reported as `UiEvent::SelectionAcross` (#264); the runtime
    /// answers with the selection and the highlights to draw.
    fn set_selection_group(&mut self, _view: &Self::View, _on: bool) {}
    /// Gives the view the keyboard focus (first responder, `grab_focus`).
    fn focus(&mut self, _view: &Self::View) {}
    /// Scrolls the scroll views around the view so it is visible.
    fn scroll_into_view(&mut self, _view: &Self::View) {}
    /// The app's HTML attributes (`.attr`): backends read the ones that mean
    /// something natively, such as `autocomplete` on a text field (#203).
    fn set_attributes(&mut self, _view: &Self::View, _attributes: &[(String, String)]) {}
    /// Scrolls a scroll view (#193): to (`x`, `y`) in its content, or by
    /// them from its offset when `relative`, kept within its content.
    fn scroll_to(
        &mut self,
        _view: &Self::View,
        _x: f32,
        _y: f32,
        _relative: bool,
        _animated: bool,
    ) {
    }
    /// The runtime's layers, bottom first: mount them above everything
    /// (sheets and dialogs too), covering the window, letting pointer input
    /// through where they have no child. Their frames come from `set_frame`.
    fn set_layers(&mut self, _layers: &[Self::View]) {}
    /// The field overlay `overlay` is anchored in (#327), for a config
    /// anchor naming one ([`OverlayAnchor::Text`](prism_core::OverlayAnchor::Text):
    /// characters of a rich input, a chip's one; or
    /// [`OverlayAnchor::Field`](prism_core::OverlayAnchor::Field)): the
    /// field's view and that anchor, or `None` (its parent, also once the
    /// field is gone). Called before every layout while its config names
    /// a field, then after it: store it, and place the overlay again when
    /// the anchor's rectangle moved (the text wrapped, the field or the
    /// window changed), as for the caret
    /// ([`OverlayAnchor::Caret`](prism_core::OverlayAnchor::Caret)). Find
    /// the rectangle at that time, in the field (its text layout,
    /// scrolled), with the platform's placement as for a parent anchor.
    /// Default: anchored to its parent.
    fn set_overlay_anchor(
        &mut self,
        _overlay: &Self::View,
        _anchor: Option<(Self::View, prism_core::OverlayAnchor)>,
    ) {
    }
    /// Where an `Overlay`'s content goes and how big it may get: `width` is
    /// the content's width (unless it sets a narrower one), `height` its
    /// maximum height (sheets fill it). Coordinates are those of the
    /// content's superview. Default: like a screen.
    fn overlay_viewport(&mut self, overlay: &Self::View, content: &Self::View) -> Viewport {
        self.screen_viewport(overlay, content)
    }
    /// What the runtime's layers keep clear of, given the window's safe
    /// area: also a root tab bar, so toasts sit above it. Default: the
    /// safe area.
    fn layer_insets(&mut self, safe_area: Insets) -> Insets {
        safe_area
    }
    /// Takes the focus away from whatever has it and hides the on-screen
    /// keyboard (`cx.dismiss_keyboard()`, #29).
    fn dismiss_keyboard(&mut self) {}
    /// The bar shown above the on-screen keyboard while text field `field`
    /// has the focus (#242): the keyboard `Overlay` that serves it (its
    /// content is laid out like an overlay's, in `overlay_viewport`), or
    /// `None`. Called when it changes, after a batch of patches.
    fn set_keyboard_bar(&mut self, _field: &Self::View, _bar: Option<&Self::View>) {}

    // --- Web views (#62) ---

    /// Delivers `data` (JSON) to a web view's page as a `message` event on
    /// its `window`: once the page has loaded, and only while it shows an
    /// allowed origin (`prism_core::web_view::WebViewConfig`). A web view's
    /// URL comes in `set_text`, its options in `set_config`.
    fn post_web_message(&mut self, _view: &Self::View, _data: &str) {}
    /// Reloads a web view's page.
    fn reload_web_view(&mut self, _view: &Self::View) {}
}

/// `style` at `pose` (its opacity and transform replaced).
pub fn posed(style: &Style, pose: Pose) -> Style {
    let (opacity, transform) = pose.apply(style.opacity, style.transform);
    Style {
        opacity,
        transform,
        ..style.clone()
    }
}

/// What [`prism_core::Color::current`] resolves to on a node without a text
/// color: black, white in dark mode (the platforms' label colors).
pub fn label_color() -> prism_core::Color {
    prism_core::Color::adaptive(prism_core::Color::BLACK, prism_core::Color::WHITE)
}

/// The outline of a box `w` × `h` at `(x, y)` (top-left origin, y down),
/// rounding `corners` (leading = left) by `radius` (clamped to half the
/// smaller side), clockwise from the top-left: quarter circles as cubics.
/// Backends draw per-corner backgrounds, borders, clips and shadows with it.
pub fn box_path(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    corners: prism_core::Corners,
) -> Vec<prism_core::Segment> {
    use prism_core::{Corners as C, Segment as S};
    let max = (w.min(h) / 2.).max(0.);
    let r = |c: C| {
        if corners.contains(c) {
            radius.clamp(0., max)
        } else {
            0.
        }
    };
    let (tl, tr, br, bl) = (
        r(C::TOP_LEADING),
        r(C::TOP_TRAILING),
        r(C::BOTTOM_TRAILING),
        r(C::BOTTOM_LEADING),
    );
    let k = 0.5523;
    let (x1, y1) = (x + w, y + h);
    let mut p = vec![S::Move(x + tl, y), S::Line(x1 - tr, y)];
    if tr > 0. {
        p.push(S::Cubic(
            x1 - tr + k * tr,
            y,
            x1,
            y + tr - k * tr,
            x1,
            y + tr,
        ));
    }
    p.push(S::Line(x1, y1 - br));
    if br > 0. {
        p.push(S::Cubic(
            x1,
            y1 - br + k * br,
            x1 - br + k * br,
            y1,
            x1 - br,
            y1,
        ));
    }
    p.push(S::Line(x + bl, y1));
    if bl > 0. {
        p.push(S::Cubic(
            x + bl - k * bl,
            y1,
            x,
            y1 - bl + k * bl,
            x,
            y1 - bl,
        ));
    }
    p.push(S::Line(x, y + tl));
    if tl > 0. {
        p.push(S::Cubic(x, y + tl - k * tl, x + tl - k * tl, y, x + tl, y));
    }
    p.push(S::Close);
    p
}

/// Containers whose children are positioned by the platform, not by flexbox.
fn is_managed(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Stack | NodeKind::Tabs | NodeKind::Overlay)
}

fn is_container(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::View
            | NodeKind::Stack
            | NodeKind::Screen
            | NodeKind::Scroll
            | NodeKind::Tabs
            | NodeKind::Tab
            | NodeKind::Modal
            | NodeKind::Overlay
            | NodeKind::Table
            | NodeKind::TableRow
            | NodeKind::ColumnHeader
            | NodeKind::TableCell
    )
}

/// Leaves that draw their own padding: the backend gets it as
/// `style.padding` in [`Widgets::set_style`] (zoomed, leading = left) and
/// insets their content by it inside their frame: a text, an input's text,
/// a button's title. Set on a button, text input or text area, it replaces
/// the control's own content inset; `measure` then returns the bare content
/// size (taffy adds the padding). Other leaves (images, pictures, switches,
/// sliders, progress bars, spinners, checkboxes, radio groups, pickers, date
/// pickers, shapes) are placed inside their padding by the renderer: their
/// frame is the content box, their own chrome keeps its inset.
pub fn draws_padding(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Text
            | NodeKind::Timer
            | NodeKind::Button
            | NodeKind::TextInput
            | NodeKind::TextArea
            | NodeKind::RichInput
    )
}

/// The frame a node's view gets: its box, or for leaves placed inside
/// their padding (see [`draws_padding`]) the box minus the padding.
fn content_box(
    kind: NodeKind,
    style: &Style,
    zoom: f32,
    rtl: bool,
    (x, y, w, h): (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    let p = style.padding;
    if is_container(kind) || draws_padding(kind) || p.is_zero() {
        return (x, y, w, h);
    }
    let p = if rtl { p.mirrored() } else { p };
    let (l, r, t, b) = (p.left * zoom, p.right * zoom, p.top * zoom, p.bottom * zoom);
    (x + l, y + t, (w - l - r).max(0.), (h - t - b).max(0.))
}

/// The area a navigation stack gives to one of its screens.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub insets: Insets,
}

/// Safe-area or window insets applied around the root.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

struct Entry<V> {
    view: V,
    kind: NodeKind,
    style: Style,
    layout: taffy::NodeId,
    /// Last frame given to the platform (animations only run on changes).
    frame: Option<(f32, f32, f32, f32)>,
    /// What a shape draws (resolved again when its size changes).
    drawing: Option<prism_core::Drawing>,
    /// Interaction states: the style as rendered (`style` is it in the
    /// current states), the states the node is in, and those it refines.
    states: EntryStates,
    /// The `order` its parent's layout children were last sorted by.
    layout_order: i32,
}

#[derive(Default)]
struct EntryStates {
    declared: Style,
    current: prism_core::Interactions,
    used: Option<prism_core::Interactions>,
    /// The named groups around it in a group state (#212).
    groups: Vec<(std::rc::Rc<str>, prism_core::Interactions)>,
}

/// Dispatches the frames of the elements that report their layout and
/// moved or resized (#192), after a layout: the patches their handlers
/// produced, for the backend to commit (which lays out and reports again).
pub fn report_layout<W: Widgets>(
    runtime: &mut prism_core::Runtime,
    renderer: &mut NativeRenderer<W>,
) -> Vec<Patch> {
    let mut patches = Vec::new();
    for (id, frame) in renderer.take_layout_changes() {
        patches.extend(runtime.dispatch(prism_core::UiEvent::Layout(id, frame)));
    }
    patches
}

/// Applies patches to native widgets and lays them out.
pub struct NativeRenderer<W: Widgets> {
    pub widgets: W,
    taffy: TaffyTree<W::View>,
    nodes: IdMap<NodeId, Entry<W::View>>,
    by_layout: IdMap<taffy::NodeId, NodeId>,
    viewport: taffy::NodeId,
    root: Option<NodeId>,
    insets: Insets,
    /// Screens of each stack. Screens are separate layout roots: their size
    /// comes from the platform's navigation container, not from flexbox.
    stack_screens: IdMap<NodeId, Vec<NodeId>>,
    /// Nodes with a zero flex basis: `0%` or `0px` depending on their
    /// parent, settled before each layout (`sync_zero_basis`, #298).
    zero_basis: IdSet<NodeId>,
    env: RenderEnvironment,
    /// Every length is multiplied by this (TVs: bigger than phone sizes).
    zoom: f32,
    /// Grids, sticky/fixed nodes, own-width rules (see `extras`).
    extras: extras::Extras,
    /// The node with the keyboard focus (its ancestors are focus-within).
    focused: Option<NodeId>,
    /// Focus changes for the app (`on_focus_change`, #224), in order.
    focus_changes: Vec<(NodeId, bool)>,
    /// Runtime-mounted layers (separate layout roots covering the window).
    layers: Vec<NodeId>,
    /// Overlay presentations (sheets fill their area, others fit content).
    overlays: IdMap<NodeId, prism_core::controls::OverlayConfig>,
    /// Size of the last layout, for the layers.
    window: (f32, f32),
    /// What layers keep clear of (`Widgets::layer_insets`).
    layer_insets: Insets,
    /// The area each screen and overlay content was last laid out in.
    areas: IdMap<NodeId, Viewport>,
    /// Set while laying out what the platform resized (the window, its safe
    /// area, a screen shown for the first time): frames are placed, not
    /// animated, as `transition` animates what the app changes (#41).
    settling: bool,
    /// Scroll views that run under a screen's bars, by node id: the platform
    /// insets their content by the bars, so it is laid out in what is
    /// visible between them (#76).
    under_bars: IdMap<NodeId, UnderBars>,
    /// Nodes that report their frame (#192), and the frame last reported.
    layout_reports: IdMap<NodeId, Option<prism_core::Rect>>,
    /// The keyboard bar serving each text field that has one (#242).
    keyboard_bars: IdMap<NodeId, NodeId>,
    /// Drag and drop between elements (#251).
    dnd: dnd::Dnd,
    /// Lists `place_children` reuses.
    child_lists: Vec<Vec<NodeId>>,
    /// Tables and their columns (#284).
    tables: table::Tables,
}

/// What the platform insets a scroll view's content by, at the top and the
/// bottom, because it runs under the bars of `screen`; and the side safe
/// area it runs under, its content inset by it (#303).
#[derive(Clone, Copy, PartialEq)]
struct UnderBars {
    screen: NodeId,
    top: f32,
    bottom: f32,
    left: f32,
    right: f32,
}

impl<W: Widgets> NativeRenderer<W> {
    pub fn new(widgets: W) -> Self {
        let mut taffy = TaffyTree::new();
        let viewport = taffy
            .new_leaf(taffy::Style::default())
            .expect("taffy viewport");
        NativeRenderer {
            widgets,
            taffy,
            nodes: IdMap::default(),
            by_layout: IdMap::default(),
            viewport,
            root: None,
            insets: Insets::default(),
            stack_screens: IdMap::default(),
            zero_basis: IdSet::default(),
            env: RenderEnvironment {
                text_scale: 1.,
                ..Default::default()
            },
            zoom: 1.,
            extras: extras::Extras::default(),
            focused: None,
            focus_changes: Vec::new(),
            layers: Vec::new(),
            overlays: IdMap::default(),
            window: (0., 0.),
            layer_insets: Insets::default(),
            areas: IdMap::default(),
            settling: false,
            under_bars: IdMap::default(),
            layout_reports: IdMap::default(),
            keyboard_bars: IdMap::default(),
            dnd: dnd::Dnd::default(),
            child_lists: Vec::new(),
            tables: table::Tables::default(),
        }
    }

    /// Zooms the whole UI: styles are in Prism points, views get
    /// `points × zoom`. The app sees a viewport of `window / zoom` (see
    /// [`zoom`](Self::zoom)).
    pub fn set_zoom(&mut self, zoom: f32) {
        if zoom == self.zoom || zoom <= 0. {
            return;
        }
        self.zoom = zoom;
        self.restyle_all();
    }

    /// Hands every node its style again and measures every leaf again: what
    /// a style resolves to changed (the zoom, a font registered while the
    /// app runs). Lay out afterwards.
    pub fn restyle_all(&mut self) {
        let ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        for id in ids {
            let e = &self.nodes[&id];
            let s = self.effective_style(&e.style, e.kind);
            self.widgets.set_style(&e.view, &s);
            self.sync_layout_style(id);
        }
        self.mark_all_dirty();
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// The style handed to the platform: Dynamic Type scaling and
    /// right-to-left text alignment applied, [`prism_core::Color::current`]
    /// resolved; the same for each interaction state's style.
    fn effective_style(&self, style: &Style, kind: NodeKind) -> Style {
        let mut s = self.effective_one(style, kind);
        if !style.states.is_empty() {
            s.states = style.states.map_resolved(|st| self.effective_one(st, kind));
        }
        s
    }

    fn effective_one(&self, style: &Style, kind: NodeKind) -> Style {
        let mut s = style.scaled(self.zoom).with_current_color(label_color());
        if kind != NodeKind::View && !kind.is_table_part() && self.env.text_scale != 1. {
            s.font_size = (s.font_size * self.env.text_scale).round();
            // Spacing in points grows with the text (fractions follow the size).
            if let Length::Px(lh) = s.line_height {
                s.line_height = Length::Px(lh * self.env.text_scale);
            }
            s.letter_spacing *= self.env.text_scale;
        }
        if self.env.direction == Direction::Rtl {
            // Borders, offsets and corners are leading/trailing, like
            // margins: backends get leading = left.
            s.border_sides = s.border_sides.mirrored();
            s.transform.x = -s.transform.x;
            s.rounded_corners = s.rounded_corners.mirrored();
            s.animations = s.animations.scaled(1., true);
            s.text_align = match s.text_align {
                TextAlign::Left => TextAlign::Right,
                TextAlign::Right => TextAlign::Left,
                TextAlign::Center => TextAlign::Center,
            };
        }
        if self.env.reduce_motion {
            s.transition = None;
            s.animations = Default::default();
        }
        // What the backend insets the content by (leading = left); the
        // layout applies everyone else's.
        s.padding = if !draws_padding(kind) {
            prism_core::Edges::default()
        } else if self.env.direction == Direction::Rtl {
            s.padding.mirrored()
        } else {
            s.padding
        };
        s
    }

    /// Places a view, animating if its style asks for it. The first time, an
    /// element with an enter pose starts there and animates to its style.
    fn set_frame(&mut self, id: NodeId, x: f32, y: f32, w: f32, h: f32) {
        let rtl = self.env.direction == Direction::Rtl;
        let zoom = self.zoom;
        let Some(e) = self.nodes.get_mut(&id) else {
            return;
        };
        let (x, y, w, h) = content_box(e.kind, &e.style, zoom, rtl, (x, y, w, h));
        let frame = (x, y, w, h);
        let previous = e.frame.replace(frame);
        // Unchanged: no native call (big trees relayout on every change).
        if previous == Some(frame) {
            return;
        }
        let transition = e
            .style
            .transition
            .filter(|_| !self.env.reduce_motion && !self.settling);
        let enter = e
            .style
            .animations
            .enter
            .filter(|_| previous.is_none() && !self.env.reduce_motion && !e.style.display_none);
        if let Some(pose) = enter {
            let (view, kind, style) = (e.view.clone(), e.kind, e.style.clone());
            self.play_enter(&view, kind, &style, pose);
            self.widgets.set_frame(&view, x, y, w, h);
            self.redraw(id);
            return;
        }
        let e = &self.nodes[&id];
        match (transition, previous) {
            (Some(t), Some(p)) if p != frame && p.2 > 0. && p.3 > 0. => {
                self.widgets.set_frame_animated(&e.view, x, y, w, h, t)
            }
            _ => self.widgets.set_frame(&e.view, x, y, w, h),
        }
        if previous.map(|p| (p.2, p.3)) != Some((w, h)) {
            self.redraw(id);
        }
    }

    fn is_shape(&self, id: NodeId) -> bool {
        self.nodes
            .get(&id)
            .is_some_and(|e| e.kind == NodeKind::Shape)
    }

    /// Hands a shape its drawing at its current size.
    fn redraw(&mut self, id: NodeId) {
        let Some(e) = self.nodes.get(&id) else {
            return;
        };
        let (Some(d), Some((_, _, w, h))) = (&e.drawing, e.frame) else {
            return;
        };
        // `Color::current()` paths (an SVG's `currentColor`) take the
        // node's text color, else the label color, like its box (#51).
        let text = e
            .style
            .text_color
            .map_or_else(label_color, |c| c.resolve_current(label_color()));
        let mut items = d.with_current_color(text).resolve_items(w, h, self.zoom);
        if self.env.direction == Direction::Rtl {
            // Like the layout around it: charts read right to left.
            for i in &mut items {
                i.mirror_x(w);
            }
        }
        // Paths alone (a chart, an icon), or everything an SVG document
        // may have.
        if items
            .iter()
            .all(|i| matches!(i, prism_core::shape::Item::Path(_)))
        {
            self.widgets
                .set_drawing(&e.view, &prism_core::shape::flatten(&items));
        } else {
            self.widgets.set_drawing_items(&e.view, &items);
        }
    }

    /// Shows `view` at its enter pose and animates it to its style.
    fn play_enter(&mut self, view: &W::View, kind: NodeKind, style: &Style, pose: Pose) {
        let s = self.effective_style(style, kind);
        let pose = pose.scaled(self.zoom, self.env.direction == Direction::Rtl);
        self.widgets.set_style(view, &posed(&s, pose));
        let t = s.transition.unwrap_or(DEFAULT_ENTER);
        self.widgets.set_style_animated(view, &s, t);
    }

    /// Children of `parent` removed by `children` and destroyed in this
    /// batch, with an exit pose: they animate out before going away.
    fn play_exits(&mut self, parent: NodeId, children: &[NodeId], destroyed: &IdSet<NodeId>) {
        if self.env.reduce_motion {
            return;
        }
        let Some(p) = self.nodes.get(&parent) else {
            return;
        };
        if is_managed(p.kind) {
            return;
        }
        let old: Vec<NodeId> = self
            .taffy
            .children(p.layout)
            .unwrap_or_default()
            .iter()
            .filter_map(|c| self.by_layout.get(c).copied())
            .collect();
        let parent_view = p.view.clone();
        for id in old {
            if children.contains(&id) || !destroyed.contains(&id) {
                continue;
            }
            let Some(e) = self.nodes.get(&id) else {
                continue;
            };
            let Some(pose) = e.style.animations.exit else {
                continue;
            };
            if e.frame.is_none() || e.style.display_none {
                continue;
            }
            let s = self.effective_style(&e.style, e.kind);
            let pose = pose.scaled(self.zoom, self.env.direction == Direction::Rtl);
            let t = s
                .transition
                .map_or(DEFAULT_EXIT, |t| Transition { delay: 0., ..t });
            let view = e.view.clone();
            self.widgets
                .animate_exit(&parent_view, &view, &posed(&s, pose), t);
        }
    }

    /// The current layout environment.
    pub fn environment(&self) -> &RenderEnvironment {
        &self.env
    }

    /// Lays the tree out in a `width` × `height` viewport and moves every view.
    ///
    /// `insets` (e.g. the iOS safe area) are added to the root's padding, so the
    /// root's background still fills the screen edge to edge.
    pub fn layout(&mut self, width: f32, height: f32, insets: Insets) {
        self.settling = (width, height) != self.window || insets != self.insets;
        // Before the layout: overlays keep an app-chosen size inside it.
        self.window = (width, height);
        if insets != self.insets {
            self.insets = insets;
            if let Some(root) = self.root {
                self.sync_layout_style(root);
            }
        }
        let viewport_style = taffy::Style {
            size: Size {
                width: Dimension::length(width),
                height: Dimension::length(height),
            },
            flex_direction: taffy::FlexDirection::Column,
            ..Default::default()
        };
        let _ = self.taffy.set_style(self.viewport, viewport_style);
        // Overlays anchored to a chip find it before they show (#327).
        self.follow_anchors();
        self.sync_zero_bases();
        // Tables size their columns from their cells first (#284).
        self.measure_tables();
        self.shift_root_fixed();
        if self.compute_with_rules(self.viewport, width, height)
            && let Some(root) = self.root
        {
            self.place(root, width, 0.);
        }
        self.update_sticky();
        self.layout_layers();
        self.settling = false;
        // ... and follow it to where this layout put it.
        self.follow_anchors();
    }

    /// Gives overlays anchored in a field (#327) the field's view: it may
    /// come or go.
    fn follow_anchors(&mut self) {
        for (id, c) in &self.overlays {
            let Some(field) = c.anchor.field() else {
                continue;
            };
            let Some(overlay) = self.nodes.get(id) else {
                continue;
            };
            let field = self.nodes.get(&field).map(|f| (f.view.clone(), c.anchor));
            self.widgets.set_overlay_anchor(&overlay.view, field);
        }
    }

    /// Runs `f`, the layout of `id` in `area`, settling if the area changed
    /// since `id` was last laid out.
    fn in_area(&mut self, id: NodeId, area: Viewport, f: impl FnOnce(&mut Self)) {
        let outer = self.settling;
        self.settling |= self.areas.insert(id, area) != Some(area);
        f(self);
        self.settling = outer;
    }

    /// Lays each layer out over the whole window, inside the safe area.
    fn layout_layers(&mut self) {
        let (width, height) = self.window;
        if !self.layers.is_empty() {
            self.layer_insets = self.widgets.layer_insets(self.insets);
        }
        for id in self.layers.clone() {
            self.sync_layout_style(id);
            let Some(layout) = self.nodes.get(&id).map(|e| e.layout) else {
                continue;
            };
            if self.compute(layout, width, height) {
                self.set_frame(id, 0., 0., width, height);
                self.place_children(id);
            }
        }
    }

    /// Lays out an overlay's content in the area the platform gives it,
    /// then tells the overlay the content's size (`set_content_size`).
    fn layout_overlay(&mut self, overlay: NodeId, content: NodeId) {
        let (Some(ov), Some(c)) = (self.nodes.get(&overlay), self.nodes.get(&content)) else {
            return;
        };
        let mut vp = self.widgets.overlay_viewport(&ov.view, &c.view);
        let config = self.overlays.get(&overlay).copied().unwrap_or_default();
        let sheet = config.kind == prism_core::OverlayKind::Sheet;
        // The app's size for a popover or dialog (#79), kept inside the
        // window; sheets keep the platform's.
        let (ww, wh) = self.window;
        let fit = |v: u32, max: f32| {
            let v = v as f32 * self.zoom;
            if max > 0. {
                v.min(max - 16.).max(1.)
            } else {
                v
            }
        };
        if !sheet && config.width > 0 {
            vp.width = fit(config.width, ww);
        }
        if !sheet && config.height > 0 {
            vp.height = fit(config.height, wh);
        }
        let fill = sheet || config.height > 0;
        let zoomed = c.style.scaled(self.zoom);
        let mut style = to_taffy(&zoomed, NodeKind::View);
        let (p, i) = (zoomed.padding, vp.insets);
        style.padding = Rect {
            top: LengthPercentage::length(p.top + i.top),
            right: LengthPercentage::length(p.right + i.right),
            bottom: LengthPercentage::length(p.bottom + i.bottom),
            left: LengthPercentage::length(p.left + i.left),
        };
        style.position = taffy::Position::Relative;
        style.margin = Rect::zero();
        if zoomed.width == Length::Auto {
            style.size.width = Dimension::length(vp.width);
        }
        style.max_size.width = LengthPercentageAuto::length(vp.width);
        if fill && zoomed.height == Length::Auto {
            style.size.height = Dimension::length(vp.height);
        }
        style.max_size.height = LengthPercentageAuto::length(vp.height);
        let (layout, view) = (c.layout, ov.view.clone());
        let _ = self.taffy.set_style(layout, style);
        if self.compute(layout, vp.width, vp.height) {
            let size = self
                .taffy
                .layout(layout)
                .map_or((0., 0.), |l| (l.size.width, l.size.height));
            // A stack or tab bar as the content lays its screens out (#298).
            self.in_area(content, vp, |r| {
                r.set_frame(content, vp.x, vp.y, size.0, size.1);
                r.place_inside(content);
            });
            self.widgets.set_content_size(&view, size.0, size.1);
        }
    }

    /// Runs flexbox on the subtree at `node`, measuring leaves natively.
    fn compute(&mut self, node: taffy::NodeId, width: f32, height: f32) -> bool {
        self.compute_in(
            node,
            Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(height),
            },
        )
    }

    /// Runs flexbox on the subtree at `node` in `available` space
    /// (min- or max-content too: table cells, #284).
    fn compute_in(&mut self, node: taffy::NodeId, available: Size<AvailableSpace>) -> bool {
        let widgets = &mut self.widgets;
        let baselines = self.extras.baselines;
        let (by_layout, nodes) = (&self.by_layout, &self.nodes);
        let font = self.zoom * self.env.text_scale;
        let zoom = self.zoom;
        // Taffy asks for a leaf's size under several constraints that come
        // down to the same width limit: within one pass a leaf's content
        // does not change, so each native measure runs once.
        let mut sizes: IdMap<(taffy::NodeId, Option<u32>), (f32, f32)> = IdMap::default();
        let mut min_widths: IdMap<taffy::NodeId, f32> = IdMap::default();
        let result =
            self.taffy
                .compute_layout_with_measure(node, available, |input, node, view, style| {
                    let view: Option<&W::View> = view.map(|v| &*v);
                    // The leaf's padding: taffy adds it around the content size
                    // we measure (known sizes include it).
                    let pad = by_layout
                        .get(&node)
                        .and_then(|id| nodes.get(id))
                        .map_or(prism_core::Edges::default(), |e| e.style.padding);
                    let (pad_x, pad_y) =
                        ((pad.left + pad.right) * zoom, (pad.top + pad.bottom) * zoom);
                    // Leaf layout (style size, min/max, padding) is taffy's; we only
                    // provide the content size of native leaves.
                    let mut out =
                        taffy::compute_leaf_layout(
                            input,
                            style,
                            |_, _| 0.0,
                            |known, available| {
                                let Some(view) = view else {
                                    return Size::ZERO;
                                };
                                let known = Size {
                                    width: known.width.map(|w| (w - pad_x).max(0.)),
                                    height: known.height.map(|h| (h - pad_y).max(0.)),
                                };
                                // Single-line text (see `to_taffy`): its natural width,
                                // cut by its frame.
                                let single_line = style.overflow.y == taffy::Overflow::Hidden;
                                let max_width = known.width.filter(|_| !single_line).or(
                                    match available.width {
                                        _ if single_line => None,
                                        AvailableSpace::Definite(w) => Some(w),
                                        AvailableSpace::MinContent => {
                                            Some(*min_widths.entry(node).or_insert_with(|| {
                                                widgets.measure_min_content(view)
                                            }))
                                        }
                                        AvailableSpace::MaxContent => None,
                                    },
                                );
                                let (w, h) = *sizes
                                    .entry((node, max_width.map(f32::to_bits)))
                                    .or_insert_with(|| widgets.measure(view, max_width));
                                // A picture given a width and no height: its height
                                // follows the image's own ratio at that width, wider
                                // than the image included (#208).
                                let picture = by_layout
                                    .get(&node)
                                    .and_then(|id| nodes.get(id))
                                    .is_some_and(|e| e.kind == NodeKind::Picture);
                                let h = match known.width {
                                    Some(kw) if picture && w > 0. => h * kw / w,
                                    _ => h,
                                };
                                Size {
                                    width: known.width.unwrap_or(w),
                                    height: known.height.unwrap_or(h),
                                }
                            },
                        );
                    // `items_baseline()`: where the first line of text sits.
                    if baselines && let Some(view) = view {
                        // Measured in the content box, below the top padding.
                        let (w, h) = (
                            (out.size.width - pad_x).max(0.),
                            (out.size.height - pad_y).max(0.),
                        );
                        out.baselines.first = widgets
                            .baseline(view, w, h)
                            .or_else(|| {
                                let e = by_layout.get(&node).and_then(|id| nodes.get(id))?;
                                extras::estimated_baseline(e.kind, e.style.font_size * font, h)
                            })
                            .map(|b| b + pad.top * zoom);
                    }
                    out
                });
        match result {
            Ok(()) => true,
            Err(err) => {
                eprintln!("prism: layout failed: {err}");
                false
            }
        }
    }

    /// Lays out one screen of a stack in the area the platform gives it.
    fn layout_screen(&mut self, stack: NodeId, screen: NodeId) {
        let (Some(st), Some(sc)) = (self.nodes.get(&stack), self.nodes.get(&screen)) else {
            return;
        };
        let vp = self.widgets.screen_viewport(&st.view, &sc.view);
        let zoomed = sc.style.scaled(self.zoom);
        let mut style = to_taffy(&zoomed, NodeKind::View);
        let (p, mut i) = (zoomed.padding, vp.insets);
        // A screen that starts with a scroll view lets it run under the bars,
        // like native apps: the platform insets its content (and collapses
        // large titles) itself.
        let leading = self
            .taffy
            .children(sc.layout)
            .ok()
            .and_then(|c| c.first().copied())
            .and_then(|c| self.by_layout.get(&c).copied())
            .filter(|c| {
                self.nodes.get(c).is_some_and(|c| {
                    c.kind == NodeKind::Scroll
                        && c.style.scroll_axis == prism_core::ScrollAxis::Vertical
                })
            });
        if leading.is_some() {
            i.top = 0.;
            i.bottom = 0.;
        }
        style.padding = Rect {
            top: LengthPercentage::length(p.top + i.top),
            right: LengthPercentage::length(p.right + i.right),
            bottom: LengthPercentage::length(p.bottom + i.bottom),
            left: LengthPercentage::length(p.left + i.left),
        };
        style.size = Size {
            width: Dimension::length(vp.width),
            height: Dimension::length(vp.height),
        };
        // Absolutely positioned children (toasts, floating buttons) are placed
        // against the screen's edges, not its padding: keep them inside the
        // safe area so bars and the keyboard don't cover them.
        let safe = vp.insets;
        let floating: Vec<(taffy::NodeId, prism_core::Inset)> = self
            .taffy
            .children(sc.layout)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| {
                let n = self.by_layout.get(&c).and_then(|id| self.nodes.get(id))?;
                matches!(
                    n.style.position,
                    prism_core::Position::Absolute | prism_core::Position::Fixed
                )
                .then(|| (c, n.style.scaled(self.zoom).inset))
            })
            .collect();
        let shift = |l: Length, by: f32| match l {
            Length::Px(v) => LengthPercentageAuto::length(v + by),
            other => lpa(other),
        };
        for (child, inset) in floating {
            if let Ok(mut st) = self.taffy.style(child).cloned() {
                st.inset = Rect {
                    top: shift(inset.top, safe.top),
                    right: shift(inset.right, safe.right),
                    bottom: shift(inset.bottom, safe.bottom),
                    left: shift(inset.left, safe.left),
                };
                let _ = self.taffy.set_style(child, st);
            }
        }
        let layout = sc.layout;
        let _ = self.taffy.set_style(layout, style);
        // Its content is laid out in what the bars leave visible, as the
        // platform insets it by them: first where the screen's padding
        // puts it, then where it really is (a row below it, a margin).
        let guess = leading.map(|s| {
            let last = self.under_bars.get(&s).filter(|u| u.screen == screen);
            let guess = last.copied().unwrap_or(UnderBars {
                screen,
                top: (vp.insets.top - p.top).max(0.),
                bottom: (vp.insets.bottom - p.bottom).max(0.),
                left: vp.insets.left,
                right: vp.insets.right,
            });
            (s, guess)
        });
        self.set_under_bars(screen, guess);
        if !self.compute_with_rules(layout, vp.width, vp.height) {
            return;
        }
        if let Some((scroll, guess)) = guess {
            let frame = self
                .nodes
                .get(&scroll)
                .and_then(|e| self.taffy.layout(e.layout).ok())
                .map(|l| (l.location.y, l.location.y + l.size.height));
            if let Some((top, bottom)) = frame {
                let real = UnderBars {
                    screen,
                    top: (vp.insets.top - top).max(0.),
                    bottom: (vp.insets.bottom - (vp.height - bottom)).max(0.),
                    ..guess
                };
                if real != guess {
                    self.set_under_bars(screen, Some((scroll, real)));
                    if !self.compute_with_rules(layout, vp.width, vp.height) {
                        return;
                    }
                }
            }
        }
        // A screen not shown yet is laid out in the platform's default
        // size, then again once shown: not an animation (#41).
        self.in_area(screen, vp, |r| {
            r.set_frame(screen, vp.x, vp.y, vp.width, vp.height);
            r.place_children(screen);
        });
    }

    /// Records the scroll view that runs under `screen`'s bars (or none):
    /// its layout padding takes them in (`sync_layout_style`).
    fn set_under_bars(&mut self, screen: NodeId, scroll: Option<(NodeId, UnderBars)>) {
        // No bar to run under (Android, macOS, a screen without a safe area).
        let scroll =
            scroll.filter(|(_, u)| u.top > 0. || u.bottom > 0. || u.left > 0. || u.right > 0.);
        let stale: Vec<NodeId> = self
            .under_bars
            .iter()
            .filter(|(id, u)| u.screen == screen && scroll.is_none_or(|(s, _)| s != **id))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.under_bars.remove(&id);
            self.sync_layout_style(id);
        }
        if let Some((id, u)) = scroll
            && self.under_bars.insert(id, u) != Some(u)
        {
            self.sync_layout_style(id);
        }
    }

    /// How far down the platform moves a scroll view's content (by the
    /// bars it runs under): its children are placed that much higher, as
    /// their layout includes it.
    fn content_shift(&self, scroll: NodeId) -> (f32, f32) {
        self.under_bars
            .get(&scroll)
            .map_or((0., 0.), |u| (u.top, u.bottom))
    }

    /// Forces leaves to be measured again (e.g. an image finished loading).
    pub fn mark_all_dirty(&mut self) {
        for e in self.nodes.values() {
            let _ = self.taffy.mark_dirty(e.layout);
        }
        self.tables.all_stale();
    }

    /// The layout nodes of `ids`, sorted by their `order` (stable: the
    /// app's order among equals). The views keep the app's order, so focus
    /// and assistive technologies follow the tree, as on the web.
    fn sorted_layouts(&mut self, ids: &[NodeId]) -> Vec<taffy::NodeId> {
        let mut kids: Vec<(i32, taffy::NodeId)> = Vec::with_capacity(ids.len());
        for c in ids {
            if let Some(e) = self.nodes.get_mut(c) {
                e.layout_order = e.style.order;
                kids.push((e.style.order, e.layout));
            }
        }
        kids.sort_by_key(|k| k.0);
        kids.into_iter().map(|k| k.1).collect()
    }

    /// A node's `order` changed: its parent sorts its layout children again.
    fn sync_order(&mut self, id: NodeId) {
        let Some(e) = self.nodes.get(&id) else { return };
        if e.style.order == e.layout_order {
            return;
        }
        if let Some(parent) = self.extras.parents.get(&id).copied() {
            self.set_layout_children(parent);
        }
    }

    fn sync_layout_style(&mut self, id: NodeId) {
        self.sync_order(id);
        let Some(e) = self.nodes.get(&id) else { return };
        let zoomed = e.style.scaled(self.zoom);
        let mut style = to_taffy(&zoomed, e.kind);
        if e.kind == NodeKind::Overlay {
            // An overlay takes no room: its content is presented on its own.
            style.position = taffy::Position::Absolute;
            style.size = Size {
                width: Dimension::length(0.),
                height: Dimension::length(0.),
            };
            style.margin = Rect::zero();
            style.inset = Rect::zero();
        }
        if self.layers.contains(&id) {
            // A layer covers the window, inside the safe area.
            let (w, h) = self.window;
            let (p, i) = (zoomed.padding, self.layer_insets);
            style.position = taffy::Position::Relative;
            style.size = Size {
                width: Dimension::length(w),
                height: Dimension::length(h),
            };
            style.padding = Rect {
                top: LengthPercentage::length(p.top + i.top),
                right: LengthPercentage::length(p.right + i.right),
                bottom: LengthPercentage::length(p.bottom + i.bottom),
                left: LengthPercentage::length(p.left + i.left),
            };
            let _ = self.taffy.set_style(e.layout, style);
            return;
        }
        if self.root == Some(id) {
            // Like `#prism-root > *` on the web: the root fills the viewport.
            style.flex_grow = 1.;
        }
        if self.in_horizontal_scroll(id) {
            // A row that scrolls: children keep their size.
            style.flex_shrink = 0.;
        }
        // A table cell takes its column's width (#284).
        if let Some(column) = self.tables.cells.get(&id) {
            column.apply(&mut style);
        }
        self.tables.apply_min(id, &mut style);
        // A scroll view under bars: its content is laid out between them.
        if let Some(u) = self.under_bars.get(&id) {
            style.padding.top = LengthPercentage::length(zoomed.padding.top + u.top);
            style.padding.bottom = LengthPercentage::length(zoomed.padding.bottom + u.bottom);
            // It reaches the screen's sides through the side safe area
            // (its indicator at the edge), its content inset by it (#303).
            style.padding.left = LengthPercentage::length(zoomed.padding.left + u.left);
            style.padding.right = LengthPercentage::length(zoomed.padding.right + u.right);
            let (m, auto) = (zoomed.margin, zoomed.margin_auto);
            if !auto[3] {
                style.margin.left = LengthPercentageAuto::length(m.left - u.left);
            }
            if !auto[1] {
                style.margin.right = LengthPercentageAuto::length(m.right - u.right);
            }
        }
        // A root stack handles the safe area itself (navigation bar, screens).
        if self.root == Some(id) && !is_managed(e.kind) {
            let (p, i) = (zoomed.padding, self.insets);
            style.padding = Rect {
                top: LengthPercentage::length(p.top + i.top),
                right: LengthPercentage::length(p.right + i.right),
                bottom: LengthPercentage::length(p.bottom + i.bottom),
                left: LengthPercentage::length(p.left + i.left),
            };
        }
        let zero = e.style.flex_basis == Length::Px(0.);
        let _ = self.taffy.set_style(e.layout, style);
        // Its parent decides what a zero flex basis is (#298).
        if zero {
            self.zero_basis.insert(id);
        } else {
            self.zero_basis.remove(&id);
        }
    }

    /// Settles every zero flex basis against the tree as it is now.
    fn sync_zero_bases(&mut self) {
        let ids: Vec<NodeId> = self.zero_basis.iter().copied().collect();
        for id in ids {
            self.sync_zero_basis(id);
        }
    }

    /// A zero flex basis (`flex_1`, `scroll()`) in a column sized by its
    /// content (no height, no growth, in a column) is `0%`, which taffy
    /// treats as the item's content size there, as browsers size such a
    /// column (#298): a `scroll().flex_1()` under a search field in a plain
    /// column shows its rows instead of getting no height. In a container
    /// of known size, `0%` is zero, as before.
    fn sync_zero_basis(&mut self, id: NodeId) {
        let Some(e) = self.nodes.get(&id) else {
            self.zero_basis.remove(&id);
            return;
        };
        let layout = e.layout;
        let by_content = self
            .taffy
            .parent(layout)
            .is_some_and(|p| self.sized_by_content(p));
        let basis = if by_content {
            Dimension::percent(0.)
        } else {
            Dimension::length(0.)
        };
        if let Ok(mut style) = self.taffy.style(layout).cloned()
            && style.flex_basis != basis
        {
            style.flex_basis = basis;
            let _ = self.taffy.set_style(layout, style);
        }
    }

    /// Whether the flex container `node` takes its height from its content:
    /// a column without a height or growth, itself in a column (in a row,
    /// stretching gives it the row's height).
    fn sized_by_content(&self, node: taffy::NodeId) -> bool {
        let column = |n: taffy::NodeId| {
            self.taffy.style(n).is_ok_and(|s| {
                s.display == taffy::Display::Flex
                    && matches!(
                        s.flex_direction,
                        taffy::FlexDirection::Column | taffy::FlexDirection::ColumnReverse
                    )
            })
        };
        let scroll = self
            .by_layout
            .get(&node)
            .and_then(|id| self.nodes.get(id))
            .is_some_and(|e| e.kind == NodeKind::Scroll);
        !scroll
            && column(node)
            && self.taffy.parent(node).is_some_and(column)
            && self.taffy.style(node).is_ok_and(|s| {
                s.size.height == Dimension::auto()
                    && s.flex_grow == 0.
                    && s.position == taffy::Position::Relative
            })
    }

    /// Places `id` inside a parent `parent_width` wide (mirrored in RTL),
    /// `shift` higher than its layout (see `content_shift`).
    fn place(&mut self, id: NodeId, parent_width: f32, shift: f32) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        let Ok(l) = self.taffy.layout(entry.layout) else {
            return;
        };
        let (mut x, mut y, mut w, h) = (
            l.location.x,
            l.location.y - shift,
            l.size.width,
            l.size.height,
        );
        if matches!(entry.kind, NodeKind::TableCell | NodeKind::ColumnHeader) {
            (x, w) = self.cell_span(entry.layout);
        }
        if self.env.direction == Direction::Rtl {
            x = parent_width - x - w;
        }
        // Offsets in percent of the node's own size: a visual shift, after
        // the layout (CSS `translate(x%, y%)`), leading like `transform.x`.
        let (tx, ty) = entry.style.translate_percent;
        if (tx, ty) != (0., 0.) {
            let dir = if self.env.direction == Direction::Rtl {
                -1.
            } else {
                1.
            };
            x += dir * w * tx / 100.;
            y += h * ty / 100.;
        }
        self.set_frame(id, x, y, w, h);
        self.place_inside(id);
    }

    /// Lays out what `id` holds once its frame is set: an overlay's content,
    /// a stack's or tab bar's screens, or its flexbox children.
    fn place_inside(&mut self, id: NodeId) {
        match self.stack_screens.get(&id).cloned() {
            Some(contents)
                if self
                    .nodes
                    .get(&id)
                    .is_some_and(|e| e.kind == NodeKind::Overlay) =>
            {
                for content in contents {
                    self.layout_overlay(id, content);
                }
            }
            Some(screens) => {
                for screen in screens {
                    self.layout_screen(id, screen);
                }
            }
            None => {
                self.place_children(id);
                self.update_content_size(id);
            }
        }
    }

    /// Tells a scroll view how far its content extends.
    fn update_content_size(&mut self, id: NodeId) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        if entry.kind != NodeKind::Scroll {
            return;
        }
        let Ok(frame) = self.taffy.layout(entry.layout).map(|l| l.size) else {
            return;
        };
        let mut bottom: f32 = 0.;
        for child in self.taffy.children(entry.layout).unwrap_or_default() {
            if let Ok(l) = self.taffy.layout(child) {
                bottom = bottom.max(l.location.y + l.size.height);
            }
        }
        // Under bars, the platform adds them around the content.
        let (top_bar, bottom_bar) = self.content_shift(id);
        let height = (bottom - top_bar + entry.style.padding.bottom)
            .max(frame.height - top_bar - bottom_bar);
        let width = self.content_width(id).unwrap_or(frame.width);
        self.widgets.set_content_size(&entry.view, width, height);
    }

    fn place_children(&mut self, id: NodeId) {
        let Some(entry) = self.nodes.get(&id) else {
            return;
        };
        let layout = entry.layout;
        let width = self.taffy.layout(layout).map_or(0., |l| l.size.width);
        // Right to left, a row that scrolls mirrors across its content.
        let width = self.content_width(id).unwrap_or(width);
        // A spare list (placing recurses through every node of the tree).
        let mut ids = self.child_lists.pop().unwrap_or_default();
        ids.extend(
            self.taffy
                .child_ids(layout)
                .filter_map(|c| self.by_layout.get(&c).copied()),
        );
        let shift = self.content_shift(id).0;
        for &child in &ids {
            self.place(child, width, shift);
        }
        ids.clear();
        self.child_lists.push(ids);
    }
}

// Interaction states (`.hover(|s| …)`…): the platform reports them, the
// node is restyled in its new states.
impl<W: Widgets> NativeRenderer<W> {
    /// Stores the style `id` was given and returns it in the node's current
    /// interaction states; tells the platform which states to report.
    fn declare(&mut self, id: NodeId, style: &Style) -> Style {
        let Some(e) = self.nodes.get_mut(&id) else {
            return style.clone();
        };
        e.states.declared = style.clone();
        let mut used = style.states.used();
        if style.is_group() {
            // A group reports its states for its descendants' refinements
            // (a drop target its drop hover, #290).
            use prism_core::Interaction as I;
            for i in [I::Hover, I::Active, I::FocusWithin, I::DropHover] {
                used = used.with(i, true);
            }
        }
        if e.states.used != Some(used) && (e.states.used.is_some() || !used.is_empty()) {
            e.states.used = Some(used);
            let view = e.view.clone();
            self.widgets.set_interactions(&view, used);
        }
        let e = &self.nodes[&id];
        style.interacted_in(e.states.current, &e.states.groups)
    }

    /// The platform saw `id` enter or leave `state` (pointer over it,
    /// pressed, keyboard focus): restyles it, and its ancestors for
    /// focus-within. Returns whether a style changed (the backend lays out
    /// again: a refinement may change sizes).
    pub fn set_interaction(
        &mut self,
        id: NodeId,
        state: prism_core::Interaction,
        on: bool,
    ) -> bool {
        use prism_core::Interaction;
        if state != Interaction::Focus {
            let changed = self.set_state_bit(id, state, on);
            self.flush_restack();
            return changed;
        }
        let mut changed = false;
        if on {
            if let Some(previous) = self.focused.replace(id)
                && previous != id
            {
                changed |= self.set_state_bit(previous, Interaction::Focus, false);
                changed |= self.set_focus_within(previous, false);
                self.focus_changes.push((previous, false));
            }
        } else if self.focused == Some(id) {
            self.focused = None;
        }
        // For the app (#224): only real changes.
        let was = self.interactions(id).contains(Interaction::Focus);
        if was != on && self.nodes.contains_key(&id) {
            self.focus_changes.push((id, on));
        }
        changed |= self.set_state_bit(id, Interaction::Focus, on);
        changed |= self.set_focus_within(id, on);
        self.flush_restack();
        changed
    }

    /// Focus changes since the last call, as events for the runtime
    /// (`on_focus_change`, #224). Backends dispatch them after reporting a
    /// focus state with [`set_interaction`](Self::set_interaction).
    pub fn take_focus_changes(&mut self) -> Vec<prism_core::UiEvent> {
        self.focus_changes
            .drain(..)
            .map(|(id, on)| prism_core::UiEvent::FocusChange(id, on))
            .collect()
    }

    /// The interaction states `id` is in.
    pub fn interactions(&self, id: NodeId) -> prism_core::Interactions {
        self.nodes
            .get(&id)
            .map_or_else(Default::default, |e| e.states.current)
    }

    fn set_focus_within(&mut self, id: NodeId, on: bool) -> bool {
        let mut changed = false;
        let mut node = Some(id);
        while let Some(n) = node {
            changed |= self.set_state_bit(n, prism_core::Interaction::FocusWithin, on);
            node = self.parent_of(n);
        }
        changed
    }

    /// The frames of the nodes that report their layout (#192) and moved or
    /// resized since they were last reported: in the window's layout
    /// coordinates before scrolling (the frames of their ancestors added
    /// up), in points. Backends dispatch them as `UiEvent::Layout` after a
    /// layout (see [`report_layout`]).
    pub fn take_layout_changes(&mut self) -> Vec<(NodeId, prism_core::Rect)> {
        let ids: Vec<NodeId> = self.layout_reports.keys().copied().collect();
        let mut out = Vec::new();
        for id in ids {
            let Some(frame) = self.layout_frame(id) else {
                continue;
            };
            if self.layout_reports.insert(id, Some(frame)) != Some(Some(frame)) {
                out.push((id, frame));
            }
        }
        out.sort_by_key(|(id, _)| *id);
        out
    }

    /// `id`'s laid-out frame in the window's layout coordinates (#192):
    /// `None` until it and its ancestors have been placed.
    fn layout_frame(&self, id: NodeId) -> Option<prism_core::Rect> {
        let (_, _, w, h) = self.nodes.get(&id)?.frame?;
        let (mut x, mut y) = (0., 0.);
        let mut node = Some(id);
        while let Some(n) = node {
            let (fx, fy, _, _) = self.nodes.get(&n)?.frame?;
            x += fx;
            y += fy;
            node = self.parent_of(n);
        }
        let z = self.zoom.max(0.01);
        Some(prism_core::Rect::new(x / z, y / z, w / z, h / z))
    }

    fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        let e = self.nodes.get(&id)?;
        match self.taffy.parent(e.layout) {
            Some(p) => self.by_layout.get(&p).copied(),
            // Screens are laid out on their own: their stack is the parent.
            None => self
                .stack_screens
                .iter()
                .find(|(_, screens)| screens.contains(&id))
                .map(|(stack, _)| *stack),
        }
    }

    fn set_state_bit(&mut self, id: NodeId, state: prism_core::Interaction, on: bool) -> bool {
        let Some(e) = self.nodes.get_mut(&id) else {
            return false;
        };
        let current = e.states.current.with(state, on);
        if current == e.states.current {
            return false;
        }
        e.states.current = current;
        let mut changed = false;
        // A group's hover, press and focus-within reach its descendants: a
        // named group's only their refinements of that name (#212).
        if let Some(group_state) = state.for_group() {
            let group = e.states.declared.group;
            let name = e.states.declared.group_name.clone();
            // An anonymous group only reaches the nodes it is the nearest
            // group of (#273): not past a nested `group()`.
            if group {
                for d in self.group_members(id) {
                    changed |= self.set_state_bit(d, group_state, on);
                }
            }
            if let Some(name) = &name {
                for d in self.descendants(id) {
                    changed |= self.set_group_bit(d, name, group_state, on);
                }
            }
        }
        let Some(e) = self.nodes.get_mut(&id) else {
            return changed;
        };
        if !e.states.declared.states.used().contains(state) {
            return changed;
        }
        changed | self.restyle_states(id)
    }

    /// Node `id`'s named group `name` (an ancestor) entered or left `state`
    /// (#212): restyled if a refinement follows it.
    fn set_group_bit(
        &mut self,
        id: NodeId,
        name: &std::rc::Rc<str>,
        state: prism_core::Interaction,
        on: bool,
    ) -> bool {
        let Some(e) = self.nodes.get_mut(&id) else {
            return false;
        };
        let groups = &mut e.states.groups;
        let slot = match groups.iter().position(|(g, _)| g == name) {
            Some(i) => i,
            None => {
                groups.push((name.clone(), prism_core::Interactions::NONE));
                groups.len() - 1
            }
        };
        let now = groups[slot].1.with(state, on);
        if now == groups[slot].1 {
            return false;
        }
        groups[slot].1 = now;
        if now.is_empty() {
            groups.remove(slot);
        }
        if !e.states.declared.states.uses_named(state, name) {
            return false;
        }
        self.restyle_states(id)
    }

    /// Restyles `id` in its current states and named groups' states.
    fn restyle_states(&mut self, id: NodeId) -> bool {
        let Some(e) = self.nodes.get_mut(&id) else {
            return false;
        };
        let style = e
            .states
            .declared
            .interacted_in(e.states.current, &e.states.groups);
        if style == e.style {
            return false;
        }
        let was_hidden = e.style.display_none;
        let repaint = e.kind == NodeKind::Shape
            && e.style.text_color != style.text_color
            && e.drawing.as_ref().is_some_and(|d| d.uses_current_color());
        let z = (e.style.z_index != style.z_index).then_some(style.z_index);
        e.style = style;
        // A refinement changed its paint order (#232).
        if let Some(z) = z {
            self.z_changed(id, z);
        }
        let e = &self.nodes[&id];
        let s = self.effective_style(&e.style, e.kind);
        let view = e.view.clone();
        if was_hidden != s.display_none {
            self.widgets.set_hidden(&view, s.display_none);
        }
        match s.transition {
            Some(t) if e.frame.is_some() => self.widgets.set_style_animated(&view, &s, t),
            _ => self.widgets.set_style(&view, &s),
        }
        if repaint {
            self.redraw(id);
        }
        self.sync_layout_style(id);
        true
    }

    /// Every node under `id` (screens of stacks included).
    fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        self.nodes_under(id, false)
    }

    /// The nodes whose nearest anonymous [`group`](prism_core::Styled::group)
    /// ancestor is `id` (#273): its descendants, a nested group included but
    /// not what is under it.
    fn group_members(&self, id: NodeId) -> Vec<NodeId> {
        self.nodes_under(id, true)
    }

    fn nodes_under(&self, id: NodeId, stop_at_groups: bool) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut todo = vec![id];
        while let Some(n) = todo.pop() {
            let Some(e) = self.nodes.get(&n) else {
                continue;
            };
            let mut kids: Vec<NodeId> = self
                .taffy
                .children(e.layout)
                .unwrap_or_default()
                .iter()
                .filter_map(|c| self.by_layout.get(c).copied())
                .collect();
            if let Some(screens) = self.stack_screens.get(&n) {
                kids.extend(screens.iter().copied());
            }
            out.extend(kids.iter().copied());
            todo.extend(kids.into_iter().filter(|k| {
                !stop_at_groups || !self.nodes.get(k).is_some_and(|e| e.states.declared.group)
            }));
        }
        out
    }
}

impl<W: Widgets> Backend for NativeRenderer<W> {
    fn apply(&mut self, patches: &[Patch]) {
        let mut destroyed = IdSet::default();
        let mut creates = 0;
        for patch in patches {
            match patch {
                Patch::Destroy { id } => {
                    destroyed.insert(*id);
                }
                Patch::Create { .. } => creates += 1,
                _ => {}
            }
        }
        // Room for the new nodes at once: entries are large to move when
        // the map grows (a first render creates the whole tree).
        self.nodes.reserve(creates);
        self.by_layout.reserve(creates);
        for patch in patches {
            match patch {
                Patch::Create { id, kind } => {
                    // A rich input is a text area of its plain text where
                    // the backend cannot edit rich text (#115).
                    let kind = &match kind {
                        NodeKind::RichInput if !self.widgets.rich_input() => NodeKind::TextArea,
                        k => *k,
                    };
                    let view = self.widgets.create(*id, *kind);
                    // Shapes, scenes and web views have no content size:
                    // only their style sizes them.
                    let container = is_container(*kind)
                        || matches!(
                            *kind,
                            NodeKind::Shape | NodeKind::Scene3d | NodeKind::WebView
                        );
                    let layout = if container {
                        self.taffy.new_leaf(taffy::Style::default())
                    } else {
                        self.taffy
                            .new_leaf_with_context(taffy::Style::default(), view.clone())
                    }
                    .expect("taffy node");
                    self.by_layout.insert(layout, *id);
                    if *kind == NodeKind::Table {
                        self.tables.ids.insert(*id);
                    }
                    self.nodes.insert(
                        *id,
                        Entry {
                            view,
                            kind: *kind,
                            style: Style::default(),
                            layout,
                            frame: None,
                            drawing: None,
                            states: EntryStates::default(),
                            layout_order: 0,
                        },
                    );
                }
                Patch::SetStyle { id, style } => {
                    // Rules on its own width: resolved at its last width.
                    let own = self.own_width_style(*id, style);
                    let style = own.as_ref().unwrap_or(style);
                    self.note_style(*id, style);
                    // The style in the node's current interaction states.
                    let style = self.declare(*id, style);
                    if let Some(e) = self.nodes.get(id) {
                        let s = self.effective_style(&style, e.kind);
                        let was_hidden = e.style.display_none;
                        if e.style.display_none != s.display_none || s.display_none {
                            self.widgets.set_hidden(&e.view, s.display_none);
                        }
                        let (view, kind) = (e.view.clone(), e.kind);
                        let enter = s
                            .animations
                            .enter
                            .filter(|_| was_hidden && !s.display_none && e.frame.is_some());
                        // The previous style as drawn, for a loop (now or
                        // before) only.
                        let loops = |a: &prism_core::Animations| a.repeat.is_some() || a.restart;
                        let old = (loops(&e.style.animations) || loops(&s.animations))
                            .then(|| self.effective_style(&e.style, e.kind));
                        match (enter, s.transition) {
                            // Shown again: enters like a new element.
                            (Some(pose), _) => self.play_enter(&view, kind, &style, pose),
                            (None, Some(t)) if e.frame.is_some() => {
                                self.widgets.set_style_animated(&view, &s, t)
                            }
                            _ => self.widgets.set_style(&view, &s),
                        }
                        // A loop restarts when it, or what it loops from, changes.
                        if let Some(old) = old
                            && (old.animations.repeat != s.animations.repeat
                                || old.animations.restart != s.animations.restart
                                || (s.animations.repeat.is_some()
                                    && (old.opacity != s.opacity || old.transform != s.transform)))
                        {
                            self.widgets.set_repeat(&view, &s, s.animations.repeat);
                        }
                    }
                    let z = style.z_index;
                    let mut repaint = false;
                    let z_before = self.nodes.get(id).map(|e| e.style.z_index);
                    if let Some(e) = self.nodes.get_mut(id) {
                        // A shape painted in `Color::current()` follows its
                        // text color.
                        repaint = e.kind == NodeKind::Shape
                            && e.style.text_color != style.text_color
                            && e.drawing.as_ref().is_some_and(|d| d.uses_current_color());
                        e.style = style;
                    }
                    if repaint {
                        self.redraw(*id);
                    }
                    // Paint order (#232).
                    if z_before.is_some_and(|before| before != z)
                        || (z != 0 && !self.extras.z_nodes.contains(id))
                    {
                        self.z_changed(*id, z);
                    }
                    self.sync_layout_style(*id);
                    self.style_changed(*id);
                }
                Patch::SetEnvironment { env } => {
                    let changed_text = env.text_scale != self.env.text_scale
                        || env.direction != self.env.direction;
                    self.env = env.clone();
                    self.widgets.set_environment(env);
                    if changed_text {
                        // Restyle every node at the new scale/direction.
                        let ids: Vec<NodeId> = self.nodes.keys().copied().collect();
                        for id in ids {
                            let e = &self.nodes[&id];
                            let s = self.effective_style(&e.style, e.kind);
                            self.widgets.set_style(&e.view, &s);
                            if e.kind == NodeKind::Shape {
                                self.redraw(id);
                            }
                        }
                        self.mark_all_dirty();
                    }
                }
                Patch::SetScene { id, update } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_scene(&e.view, update, &update.to_json());
                    }
                }
                Patch::SetText { id, text } if self.is_shape(*id) => {
                    let drawing = prism_core::Drawing::decode(text);
                    if let Some(e) = self.nodes.get_mut(id) {
                        e.drawing = Some(drawing);
                    }
                    self.redraw(*id);
                }
                Patch::SetText { id, text } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_text(&e.view, text);
                        let _ = self.taffy.mark_dirty(e.layout);
                    }
                }
                Patch::SetRichValue { id, runs } => {
                    if let Some(e) = self.nodes.get(id)
                        && e.kind == NodeKind::RichInput
                    {
                        self.widgets.set_rich_value(&e.view, runs);
                        let _ = self.taffy.mark_dirty(e.layout);
                    }
                }
                Patch::SetSelection { id, start, end } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_selection(&e.view, *start, *end);
                    }
                }
                Patch::SetSelectEvents { id, on } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_select_events(&e.view, *on);
                    }
                }
                Patch::SetRemoteCarets { id, carets } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_remote_carets(&e.view, carets);
                    }
                }
                Patch::SetEditEvents { id, mask } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_edit_events(&e.view, *mask);
                    }
                }
                Patch::WriteClipboard { id, content } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.write_clipboard(&e.view, content);
                    }
                }
                Patch::InsertText { id, text } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.insert_text(&e.view, text);
                    }
                }
                Patch::SetSelectionHighlight { id, start, end } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_selection_highlight(&e.view, *start, *end);
                    }
                }
                // The runtime extends a selection across fields natively
                // from the fields' keys (#231): nothing to set up.
                Patch::SetSelectionGroup { id, on, .. } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_selection_group(&e.view, *on);
                    }
                }
                Patch::SetSelectionAcross { .. } => {}
                Patch::SetPlaceholder { id, text } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_placeholder(&e.view, text);
                        let _ = self.taffy.mark_dirty(e.layout);
                    }
                }
                Patch::SetClickable { id, clickable } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_clickable(&e.view, *clickable);
                    }
                }
                Patch::SetChildren { id, children } => {
                    self.play_exits(*id, children, &destroyed);
                    if !self.nodes.contains_key(id) {
                        continue;
                    }
                    // Drawn in their `z_index` order (#232); a stack's screens
                    // and a tab bar's tabs keep theirs.
                    let managed = self.nodes.get(id).is_some_and(|e| is_managed(e.kind));
                    let painted = if managed {
                        children.clone()
                    } else {
                        self.paint_order(children)
                    };
                    let views: Vec<W::View> = painted
                        .iter()
                        .filter_map(|c| self.nodes.get(c).map(|e| e.view.clone()))
                        .collect();
                    let reorder = painted != *children;
                    self.extras.painted.insert(*id, painted);
                    let layouts = self.sorted_layouts(children);
                    let parent = &self.nodes[id];
                    self.widgets.set_children(&parent.view, &views);
                    if reorder {
                        let tree = self.views_of(children);
                        let parent = &self.nodes[id];
                        self.widgets.set_reading_order(&parent.view, &tree);
                    }
                    if is_managed(parent.kind) {
                        let _ = self.taffy.set_children(parent.layout, &[]);
                        self.stack_screens.insert(*id, children.clone());
                    } else {
                        let _ = self.taffy.set_children(parent.layout, &layouts);
                    }
                    self.children_changed(*id, children);
                    // A `z_index` inside lifts this node among its siblings.
                    if !self.extras.z_nodes.is_empty() {
                        self.restack(*id);
                    }
                }
                Patch::SetAccessibility { id, a11y } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_accessibility(&e.view, a11y);
                        let target = a11y
                            .controls_node
                            .and_then(|t| self.nodes.get(&t))
                            .map(|t| &t.view);
                        if a11y.controls.is_some() || self.extras.controllers.remove(id) {
                            self.widgets.set_controls(&e.view, target);
                        }
                        if a11y.controls.is_some() {
                            self.extras.controllers.insert(*id);
                        }
                    }
                }
                Patch::SetAppearance { appearance } => self.widgets.set_appearance(*appearance),
                Patch::SetSwipe { id, left, right } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_swipe(&e.view, *left, *right);
                    }
                }
                Patch::SetScrollEvents { id, on } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_scroll_events(&e.view, *on);
                    }
                }
                Patch::SetMenu { id, menu } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_menu(&e.view, menu.as_ref());
                    }
                }
                Patch::SetValue { id, value } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_value(&e.view, *value);
                        // An optional date picker is as wide as its
                        // placeholder while empty (#89).
                        if e.kind == NodeKind::DatePicker {
                            let _ = self.taffy.mark_dirty(e.layout);
                        }
                    }
                }
                Patch::SetConfig { id, config } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_config(&e.view, e.kind, config);
                        let _ = self.taffy.mark_dirty(e.layout);
                        if e.kind == NodeKind::Overlay {
                            let c = prism_core::controls::OverlayConfig::decode(config);
                            // No longer anchored in a field (#327).
                            if c.anchor.field().is_none()
                                && self.overlays.get(id).is_some_and(|o| o.anchor.field().is_some())
                            {
                                self.widgets.set_overlay_anchor(&e.view, None);
                            }
                            self.overlays.insert(*id, c);
                        }
                    }
                }
                Patch::SetIcon { id, icon } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_icon(&e.view, icon);
                        let _ = self.taffy.mark_dirty(e.layout);
                    }
                }
                Patch::SetRoot { id } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_root(&e.view);
                        let _ = self.taffy.set_children(self.viewport, &[e.layout]);
                        let previous = self.root.replace(*id);
                        if let Some(previous) = previous {
                            self.sync_layout_style(previous);
                        }
                        self.sync_layout_style(*id);
                    }
                }
                Patch::Destroy { id } => {
                    self.tables.forget(*id);
                    self.forget_extras(*id);
                    self.dnd.forget(*id);
                    self.layout_reports.remove(id);
                    if self.focused == Some(*id) {
                        self.focused = None;
                        self.set_focus_within(*id, false);
                    }
                    if let Some(e) = self.nodes.remove(id) {
                        self.widgets.destroy(&e.view);
                        self.by_layout.remove(&e.layout);
                        self.stack_screens.remove(id);
                        self.zero_basis.remove(id);
                        self.overlays.remove(id);
                        self.areas.remove(id);
                        self.under_bars.remove(id);
                        let _ = self.taffy.remove(e.layout);
                    }
                }
                Patch::SetPointerEvents { id, mask } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_pointer_events(&e.view, *mask);
                    }
                }
                Patch::SetDragDelay { id, ms } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_drag_delay(&e.view, *ms);
                    }
                }
                Patch::SetDropTarget { id, types } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_drop_target(&e.view, types.as_deref());
                    }
                }
                Patch::SetDragAndDrop {
                    id,
                    kind,
                    preview,
                    accepts,
                } => self.set_drag_and_drop(*id, kind.as_deref(), *preview, accepts),
                Patch::SetKeyEvents { id, local, global } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_key_events(&e.view, *local, *global);
                    }
                }
                Patch::SetConsumedKeys { id, keys } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_consumed_keys(&e.view, keys);
                    }
                }
                Patch::SetLayoutEvents { id, on } => {
                    if *on {
                        self.layout_reports.entry(*id).or_insert(None);
                    } else {
                        self.layout_reports.remove(id);
                    }
                }
                Patch::Focus { id } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.focus(&e.view);
                    }
                }
                Patch::ScrollIntoView { id } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.scroll_into_view(&e.view);
                    }
                }
                Patch::ShowMenu { id, menu, at } => {
                    let z = self.zoom;
                    let at = at.map(|p| (p.x * z, p.y * z));
                    match id {
                        Some(id) => {
                            if let Some(e) = self.nodes.get(id) {
                                self.widgets.show_menu(Some(&e.view), menu, at);
                            }
                        }
                        None => self.widgets.show_menu(None, menu, at),
                    }
                }
                Patch::ScrollTo {
                    id,
                    x,
                    y,
                    relative,
                    animated,
                } => {
                    if let Some(e) = self.nodes.get(id) {
                        // In the platform's units; `f32::MAX` (the end) may
                        // become infinite: backends clamp to the content.
                        let z = self.zoom;
                        self.widgets
                            .scroll_to(&e.view, x * z, y * z, *relative, *animated);
                    }
                }
                Patch::DismissKeyboard => self.widgets.dismiss_keyboard(),
                Patch::SetLayers { ids } => {
                    let views: Vec<W::View> = ids
                        .iter()
                        .filter_map(|i| self.nodes.get(i).map(|e| e.view.clone()))
                        .collect();
                    let previous = std::mem::replace(&mut self.layers, ids.clone());
                    for old in previous {
                        self.sync_layout_style(old);
                    }
                    self.widgets.set_layers(&views);
                }
                // Islands run in browsers: native apps show the node's
                // children (the island's fallback) as a plain view.
                // A link is activated like any clickable view (`SetClickable`).
                Patch::SetIsland { .. }
                | Patch::SetIslandProps { .. }
                | Patch::SetLink { .. }
                // The app copies when the click arrives (#323).
                | Patch::SetCopyOnClick { .. }
                // Native Back gestures pop as before (#324).
                | Patch::SetBackRequests { .. } => {}
                // HTML attributes (`.attr(…)`): browser integrations, and
                // `autocomplete` for a text field's content type (#203).
                Patch::SetAttributes { id, attributes } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.set_attributes(&e.view, attributes);
                    }
                }
                // Web views (#62).
                Patch::PostWebMessage { id, data } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.post_web_message(&e.view, data);
                    }
                }
                Patch::ReloadWebView { id } => {
                    if let Some(e) = self.nodes.get(id) {
                        self.widgets.reload_web_view(&e.view);
                    }
                }
            }
        }
        if self.extras.rehoist {
            self.rehoist();
        }
        self.flush_restack();
        // Tables whose cells changed size their columns again; their
        // structure goes to the platform's accessibility (#284).
        if !self.tables.ids.is_empty() {
            self.note_table_patches(patches);
        }
        // Which bar serves each field changes with the tree and the bars.
        if patches.iter().any(|p| {
            matches!(
                p,
                Patch::SetChildren { .. } | Patch::Destroy { .. } | Patch::SetConfig { .. }
            )
        }) {
            self.sync_keyboard_bars();
        }
    }
}

impl<W: Widgets> NativeRenderer<W> {
    /// The native view of node `id`, if mounted.
    pub fn view(&self, id: NodeId) -> Option<&W::View> {
        self.nodes.get(&id).map(|e| &e.view)
    }

    /// The keyboard bar serving text field `field` (#242): its own (the
    /// keyboard overlay right after it, `field_only`), else the nearest
    /// container's shared one.
    pub fn keyboard_bar_of(&self, field: NodeId) -> Option<NodeId> {
        let keyboard = |id: &NodeId| {
            self.overlays
                .get(id)
                .filter(|c| c.kind == prism_core::OverlayKind::Keyboard)
        };
        let mut child = field;
        let mut own = true;
        while let Some(&parent) = self.extras.parents.get(&child) {
            let siblings = self.extras.children.get(&parent)?;
            if std::mem::take(&mut own)
                && let Some(next) = siblings
                    .iter()
                    .position(|c| *c == child)
                    .and_then(|i| siblings.get(i + 1))
                && keyboard(next).is_some_and(|c| c.field_only)
            {
                return Some(*next);
            }
            if let Some(bar) = siblings
                .iter()
                .find(|c| keyboard(c).is_some_and(|c| !c.field_only))
            {
                return Some(*bar);
            }
            child = parent;
        }
        None
    }

    /// Tells the platform which keyboard bar each text field has (#242),
    /// where it changed.
    fn sync_keyboard_bars(&mut self) {
        let any = self
            .overlays
            .values()
            .any(|c| c.kind == prism_core::OverlayKind::Keyboard);
        if !any && self.keyboard_bars.is_empty() {
            return;
        }
        let mut next = IdMap::default();
        if any {
            // What `keyboard_bar_of` answers for every field, in one walk
            // down from the roots with the nearest container's shared bar.
            let bar = |id: NodeId, field_only: bool| {
                self.overlays.get(&id).is_some_and(|c| {
                    c.kind == prism_core::OverlayKind::Keyboard && c.field_only == field_only
                })
            };
            let is_field = |id: NodeId| {
                self.nodes.get(&id).is_some_and(|e| {
                    matches!(
                        e.kind,
                        NodeKind::TextInput | NodeKind::TextArea | NodeKind::RichInput
                    )
                })
            };
            let mut todo: Vec<(NodeId, Option<NodeId>)> = self
                .nodes
                .keys()
                .filter(|id| !self.extras.parents.contains_key(id))
                .map(|id| (*id, None))
                .collect();
            while let Some((id, outer)) = todo.pop() {
                let Some(children) = self.extras.children.get(&id) else {
                    continue;
                };
                let shared = children.iter().copied().find(|c| bar(*c, false)).or(outer);
                for (i, &child) in children.iter().enumerate() {
                    if is_field(child) {
                        let own = children.get(i + 1).copied().filter(|n| bar(*n, true));
                        if let Some(b) = own.or(shared) {
                            next.insert(child, b);
                        }
                    }
                    todo.push((child, shared));
                }
            }
        }
        let previous = std::mem::take(&mut self.keyboard_bars);
        for (field, bar) in &next {
            if previous.get(field) != Some(bar)
                && let (Some(f), Some(b)) = (self.nodes.get(field), self.nodes.get(bar))
            {
                self.widgets.set_keyboard_bar(&f.view, Some(&b.view));
            }
        }
        for field in previous.keys() {
            if !next.contains_key(field)
                && let Some(f) = self.nodes.get(field)
            {
                self.widgets.set_keyboard_bar(&f.view, None);
            }
        }
        self.keyboard_bars = next;
    }
}

fn dim(l: Length) -> Dimension {
    match l {
        Length::Auto => Dimension::auto(),
        Length::Px(v) => Dimension::length(v),
        Length::Fraction(f) => Dimension::percent(f),
    }
}

fn lpa(l: Length) -> LengthPercentageAuto {
    match l {
        Length::Auto => LengthPercentageAuto::auto(),
        Length::Px(v) => LengthPercentageAuto::length(v),
        Length::Fraction(f) => LengthPercentageAuto::percent(f),
    }
}

fn align(a: Align) -> taffy::AlignItems {
    match a {
        Align::Start => taffy::AlignItems::FLEX_START,
        Align::Center => taffy::AlignItems::CENTER,
        Align::End => taffy::AlignItems::FLEX_END,
        Align::Stretch => taffy::AlignItems::STRETCH,
        Align::Baseline => taffy::AlignItems::BASELINE,
    }
}

/// A flex item's minimum width when the style sets none: `0`, like the web
/// backend's `min-width:0` on every node, so a row shrinks its items below
/// their content (a text wraps, then breaks its words) the same way on both.
/// Switches and spinners keep their automatic minimum (their content size),
/// as on the web (`min-width:auto`): the platform control can't draw smaller.
/// The height keeps CSS's automatic minimum on both backends.
fn min_width(s: &Style, kind: NodeKind) -> LengthPercentageAuto {
    match s.min_width {
        Length::Auto if !matches!(kind, NodeKind::Toggle | NodeKind::Spinner) => {
            LengthPercentageAuto::length(0.)
        }
        other => lpa(other),
    }
}

/// Maps a Prism style to taffy. Kept in sync with `prism-web`'s CSS output.
pub fn to_taffy(s: &Style, kind: NodeKind) -> taffy::Style {
    let edges = |e: prism_core::Edges| Rect {
        top: LengthPercentage::length(e.top),
        right: LengthPercentage::length(e.right),
        bottom: LengthPercentage::length(e.bottom),
        left: LengthPercentage::length(e.left),
    };
    // `auto` sides take the free space (#228).
    let side = |v: f32, auto: bool| {
        if auto {
            LengthPercentageAuto::auto()
        } else {
            LengthPercentageAuto::length(v)
        }
    };
    let margin = Rect {
        top: side(s.margin.top, s.margin_auto[0]),
        right: side(s.margin.right, s.margin_auto[1]),
        bottom: side(s.margin.bottom, s.margin_auto[2]),
        left: side(s.margin.left, s.margin_auto[3]),
    };
    let mut style = taffy::Style {
        display: if s.display_none {
            taffy::Display::None
        } else {
            taffy::Display::Flex
        },
        flex_direction: match s.direction {
            FlexDirection::Row => taffy::FlexDirection::Row,
            FlexDirection::Column => taffy::FlexDirection::Column,
            FlexDirection::RowReverse => taffy::FlexDirection::RowReverse,
            FlexDirection::ColumnReverse => taffy::FlexDirection::ColumnReverse,
        },
        flex_wrap: if s.wrap {
            taffy::FlexWrap::Wrap
        } else {
            taffy::FlexWrap::NoWrap
        },
        align_items: Some(align(s.align_items)),
        align_self: s.align_self.map(align),
        justify_content: Some(match s.justify_content {
            Justify::Start => taffy::JustifyContent::FLEX_START,
            Justify::Center => taffy::JustifyContent::CENTER,
            Justify::End => taffy::JustifyContent::FLEX_END,
            Justify::SpaceBetween => taffy::JustifyContent::SPACE_BETWEEN,
            Justify::SpaceAround => taffy::JustifyContent::SPACE_AROUND,
            Justify::SpaceEvenly => taffy::JustifyContent::SPACE_EVENLY,
            // A grid's `auto` tracks fill it, like CSS (#284); a flex
            // container packs at the start (taffy, like CSS).
            Justify::Stretch => taffy::JustifyContent::STRETCH,
        }),
        gap: Size {
            width: LengthPercentage::length(s.gap),
            height: LengthPercentage::length(s.gap),
        },
        // Leaves too: taffy adds it around their measured content, and the
        // content is drawn inset by it (see `draws_padding`).
        padding: edges(s.padding),
        border: Rect::zero(),
        margin,
        position: match s.position {
            prism_core::Position::Relative => taffy::Position::Relative,
            prism_core::Position::Absolute => taffy::Position::Absolute,
            // Fixed nodes are hoisted into their screen (see `extras`).
            prism_core::Position::Sticky => taffy::Position::Relative,
            prism_core::Position::Fixed => taffy::Position::Absolute,
        },
        inset: Rect {
            top: lpa(s.inset.top),
            right: lpa(s.inset.right),
            bottom: lpa(s.inset.bottom),
            left: lpa(s.inset.left),
        },
        size: Size {
            width: dim(s.width),
            height: dim(s.height),
        },
        min_size: Size {
            width: min_width(s, kind),
            height: lpa(s.min_height),
        },
        max_size: Size {
            width: lpa(s.max_width),
            height: lpa(s.max_height),
        },
        flex_grow: s.flex_grow,
        flex_shrink: s.flex_shrink,
        flex_basis: dim(s.flex_basis),
        // A scroll view's content may be taller than the view itself.
        overflow: if kind == NodeKind::Scroll {
            taffy::Point {
                x: taffy::Overflow::Hidden,
                y: taffy::Overflow::Scroll,
            }
        } else if kind == NodeKind::Text && s.truncates() {
            // Cut text (ellipsis, no wrapping) can be narrower than its
            // words, like CSS `overflow: hidden`. Single-line text (both axes
            // hidden, like the web) is measured on one line.
            taffy::Point {
                x: taffy::Overflow::Hidden,
                y: if s.no_wrap || s.max_lines == Some(1) {
                    taffy::Overflow::Hidden
                } else {
                    taffy::Overflow::Visible
                },
            }
        } else {
            taffy::Point {
                x: taffy::Overflow::Visible,
                y: taffy::Overflow::Visible,
            }
        },
        scrollbar_width: 0.,
        // `aspect_ratio` (and its caps): `extras::apply`.
        ..Default::default()
    };
    extras::apply(&mut style, s, kind);
    table::apply(&mut style, kind);
    style
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_core::{
        AppInit, IntoElement, ParentElement, Render, Router, Runtime, Styled, div, screen, scroll,
        text,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    /// Records frames; screens get the stack's area minus a 44pt header.
    #[derive(Default)]
    struct Fake {
        frames: HashMap<NodeId, (f32, f32, f32, f32)>,
        content: HashMap<NodeId, (f32, f32)>,
        drawings: HashMap<NodeId, Vec<prism_core::Path>>,
        /// What a backend that draws text, images and groups gets
        /// (`None`: this one draws paths only).
        items: Option<HashMap<NodeId, Vec<prism_core::shape::Item>>>,
    }

    impl Widgets for Fake {
        type View = NodeId;
        fn create(&mut self, id: NodeId, _: NodeKind) -> NodeId {
            id
        }
        fn set_style(&mut self, _: &NodeId, _: &Style) {}
        fn set_text(&mut self, _: &NodeId, _: &str) {}
        fn set_placeholder(&mut self, _: &NodeId, _: &str) {}
        fn set_clickable(&mut self, _: &NodeId, _: bool) {}
        fn set_children(&mut self, _: &NodeId, _: &[NodeId]) {}
        fn set_root(&mut self, _: &NodeId) {}
        fn destroy(&mut self, _: &NodeId) {}
        fn measure(&mut self, _: &NodeId, _: Option<f32>) -> (f32, f32) {
            (50., 20.)
        }
        fn set_frame(&mut self, v: &NodeId, x: f32, y: f32, w: f32, h: f32) {
            self.frames.insert(*v, (x, y, w, h));
        }
        fn set_accessibility(&mut self, _: &NodeId, _: &Accessibility) {}
        fn set_value(&mut self, _: &NodeId, _: f64) {}
        fn set_content_size(&mut self, v: &NodeId, w: f32, h: f32) {
            self.content.insert(*v, (w, h));
        }
        fn set_drawing(&mut self, v: &NodeId, paths: &[prism_core::Path]) {
            self.drawings.insert(*v, paths.to_vec());
        }
        fn set_drawing_items(&mut self, v: &NodeId, items: &[prism_core::shape::Item]) {
            match &mut self.items {
                Some(all) => {
                    all.insert(*v, items.to_vec());
                }
                None => self.set_drawing(v, &prism_core::shape::flatten(items)),
            }
        }
        fn screen_viewport(&mut self, stack: &NodeId, _: &NodeId) -> Viewport {
            let (_, _, w, h) = self.frames[stack];
            Viewport {
                x: 0.,
                y: 44.,
                width: w,
                height: h - 44.,
                insets: Insets::default(),
            }
        }
    }

    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            screen("Page").child(text("hello"))
        }
    }

    #[test]
    fn root_stack_fills_viewport_and_screens_get_their_area() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = Router::new()
                .route("/", |cx, _| cx.new_entity(|_| Page))
                .build(cx);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 800., Insets::default());
        let f = &renderer.widgets.frames;
        assert_eq!(f[&1], (0., 0., 400., 800.), "stack");
        assert_eq!(f[&2], (0., 44., 400., 756.), "screen");
        assert_eq!(f[&3], (0., 0., 400., 20.), "text in screen");
    }

    struct Long;
    impl Render for Long {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div().child(
                scroll()
                    .p(10.)
                    .children((0..50).map(|i| text(format!("row {i}")))),
            )
        }
    }

    #[test]
    fn scroll_views_report_their_content_height() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Long);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        let w = &renderer.widgets;
        assert_eq!(
            w.frames[&2].3, 300.,
            "scroll view keeps the viewport height"
        );
        // 50 rows × 20pt + 10pt padding top and bottom.
        assert_eq!(w.content[&2], (400., 1020.));
    }

    /// A picture and a text given a width (#208): the picture's height
    /// follows its natural ratio (the fake's 50 × 20), the text's does not.
    struct Pictured;
    impl Render for Pictured {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div()
                .child(prism_core::picture("cat.png").w(prism_core::px(250.)))
                .child(text("caption").w(prism_core::px(250.)))
                .child(prism_core::picture("dog.png").w_full())
        }
    }

    #[test]
    fn a_picture_with_a_width_takes_its_height_from_its_ratio() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Pictured);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 600., Insets::default());
        let f = &renderer.widgets.frames;
        // Ids: 1 div, 2 picture, 3 text, 4 picture.
        assert_eq!((f[&2].2, f[&2].3), (250., 100.), "250 wide: 250 × 20 / 50");
        assert_eq!((f[&3].2, f[&3].3), (250., 20.), "a text keeps its height");
        assert_eq!(
            (f[&4].2, f[&4].3),
            (400., 160.),
            "the full width: 400 × 20 / 50"
        );
    }

    /// Rows with their frames reported (#192), inside a padded scroll.
    struct Tracked;
    impl Render for Tracked {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            let row = |i: usize| {
                div()
                    .id(format!("row{i}"))
                    .h(prism_core::px(30.))
                    .flex_shrink_0()
                    .track_layout()
            };
            div()
                .p(10.)
                .child(scroll().h(prism_core::px(100.)).children((0..20).map(row)))
        }
    }

    #[test]
    fn tracked_frames_add_up_their_ancestors_and_ignore_scrolling() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Tracked);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 600., Insets::default());
        let changes = renderer.take_layout_changes();
        assert_eq!(changes.len(), 20, "every tracked row, once laid out");
        // Rows below the 100pt viewport still have their content frame.
        assert!(report_layout(&mut rt, &mut renderer).is_empty());
        let mut rt2 = Runtime::new(&init);
        let mut r2 = NativeRenderer::new(Fake::default());
        r2.apply(&rt2.render());
        r2.layout(400., 600., Insets::default());
        let _ = report_layout(&mut rt2, &mut r2);
        let f0 = rt2.app.frame_of("row0").unwrap();
        let f1 = rt2.app.frame_of("row1").unwrap();
        let f9 = rt2.app.frame_of("row9").unwrap();
        assert_eq!((f0.x, f0.y, f0.width, f0.height), (10., 10., 380., 30.));
        assert_eq!(f1.y, 40.);
        assert_eq!(f9.y, 10. + 9. * 30., "below the viewport: unscrolled");
        // Nothing moved: nothing to report.
        assert!(r2.take_layout_changes().is_empty());
        // A narrower window resizes every row: reported again.
        r2.layout(300., 600., Insets::default());
        let changed = r2.take_layout_changes();
        assert_eq!(changed.len(), 20);
        assert_eq!(changed[0].1.width, 280.);
    }

    /// Two fitted scroll views (3 rows, then 50) and a line of text below.
    struct Fitted;
    impl Render for Fitted {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            let rows = |n: usize| (0..n).map(|i| text(format!("row {i}")));
            div()
                .child(scroll().fit().max_h(prism_core::px(100.)).children(rows(3)))
                .child(
                    scroll()
                        .fit()
                        .max_h(prism_core::px(100.))
                        .children(rows(50)),
                )
                .child(text("below"))
        }
    }

    #[test]
    fn fitted_scroll_views_are_as_tall_as_their_content_up_to_max_h() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Fitted);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 600., Insets::default());
        let w = &renderer.widgets;
        // Ids: 1 div, 2 short scroll (3..5 rows), 6 long scroll (7..56), 57 text.
        assert_eq!(w.frames[&2], (0., 0., 400., 60.), "short: its content");
        assert_eq!(w.frames[&6], (0., 60., 400., 100.), "long: capped at max_h");
        assert_eq!(w.content[&6], (400., 1000.), "long: scrolls its content");
        assert_eq!(w.frames[&57].1, 160., "text follows, nothing stretched");
    }

    struct Row;
    impl Render for Row {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div().flex_row().child(text("a")).child(text("b"))
        }
    }

    #[test]
    fn right_to_left_mirrors_rows_and_text_scale_restyles() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Row);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(
            renderer.widgets.frames[&2].0, 0.,
            "LTR: first item on the left"
        );

        rt.app.set_locale(Some(prism_core::Locale::new("ar")));
        rt.app.set_environment(prism_core::Environment {
            text_scale: 1.5,
            ..Default::default()
        });
        renderer.apply(&rt.render());
        assert_eq!(renderer.environment().direction, prism_core::Direction::Rtl);
        assert_eq!(renderer.environment().text_scale, 1.5);
        renderer.layout(400., 300., Insets::default());
        let f = &renderer.widgets.frames;
        assert_eq!(f[&2].0, 350., "RTL: first item on the right");
        assert_eq!(f[&3].0, 300.);
    }

    struct Overlay;
    impl Render for Overlay {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div()
                .flex_1()
                .child(text("in flow"))
                .child(div().absolute().left(10.).bottom(20.).w(100.).h(30.))
        }
    }

    #[test]
    fn absolute_nodes_sit_at_their_insets_mirrored_right_to_left() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Overlay);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(renderer.widgets.frames[&3], (10., 250., 100., 30.));
        assert_eq!(renderer.widgets.frames[&2].1, 0., "out of the flow");
        rt.app.set_locale(Some(prism_core::Locale::new("ar")));
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(renderer.widgets.frames[&3], (290., 250., 100., 30.));
    }

    struct Chart;
    impl Render for Chart {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            div().child(
                prism_core::shape()
                    .path(prism_core::Path::line([(0., 1.), (1., 0.)]))
                    .w_full()
                    .h(100.),
            )
        }
    }

    #[test]
    fn shapes_are_drawn_at_their_size_and_mirrored_right_to_left() {
        use prism_core::Segment;
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Chart);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        let first = |r: &NativeRenderer<Fake>| r.widgets.drawings[&2][0].segments.clone();
        assert_eq!(
            first(&renderer),
            [Segment::Move(0., 100.), Segment::Line(400., 0.)]
        );
        // Resized: drawn again at the new width.
        renderer.layout(200., 300., Insets::default());
        assert_eq!(first(&renderer)[1], Segment::Line(200., 0.));
        rt.app.set_locale(Some(prism_core::Locale::new("ar")));
        renderer.apply(&rt.render());
        renderer.layout(200., 300., Insets::default());
        assert_eq!(
            first(&renderer),
            [Segment::Move(200., 100.), Segment::Line(0., 0.)]
        );
    }

    struct Icon {
        tint: Option<prism_core::Color>,
    }
    impl Render for Icon {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            let icon = prism_core::svg(
                r#"<svg width="10" height="10"><path d="M0 0H10V10Z" fill="currentColor"/></svg>"#,
            );
            div().child(match self.tint {
                Some(c) => icon.text_color(c),
                None => icon,
            })
        }
    }

    #[test]
    fn svg_current_color_is_the_shape_text_color() {
        // #51: `currentColor` follows `.text_color(…)`, else the label color.
        let icon: Arc<std::sync::Mutex<Option<prism_core::Entity<Icon>>>> = Default::default();
        let out = icon.clone();
        let init: AppInit = Arc::new(move |cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Icon { tint: None });
            *out.lock().unwrap() = Some(r);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        let fill = |r: &NativeRenderer<Fake>| r.widgets.drawings[&2][0].fill;
        assert_eq!(fill(&renderer), Some(label_color()));
        let red = prism_core::rgb(0xff0000);
        let icon = icon.lock().unwrap().unwrap();
        icon.update(&mut rt.app, |i, cx| {
            i.tint = Some(red);
            cx.notify();
        });
        renderer.apply(&rt.poll());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(fill(&renderer), Some(red));
    }

    struct Card;
    impl Render for Card {
        fn render(&mut self, _: &mut prism_core::Context<Self>) -> impl IntoElement {
            // 100 × 50 points, shown twice as large.
            div().child(
                prism_core::svg(
                    r##"<svg width="100" height="50" viewBox="0 0 100 50">
                      <g opacity="0.5">
                        <rect width="60" height="50" fill="#f00" stroke="#000" stroke-width="2"/>
                        <rect x="40" width="60" height="50" fill="currentColor"/>
                      </g>
                      <text x="50" y="25" font-size="10" text-anchor="middle" fill="currentColor">Hi</text>
                    </svg>"##,
                )
                .w(prism_core::px(200.)),
            )
        }
    }

    #[test]
    fn an_svg_document_reaches_the_backend_whole_or_as_paths() {
        use prism_core::shape::Item;
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = cx.new_entity(|_| Card);
            cx.set_root(r);
        });
        // #342: a backend that draws them gets the group and the text, in
        // points at the size shown, `currentColor` resolved.
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake {
            items: Some(HashMap::new()),
            ..Fake::default()
        });
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        assert_eq!(renderer.widgets.frames[&2].2, 200.);
        assert_eq!(renderer.widgets.frames[&2].3, 100.);
        let items = &renderer.widgets.items.as_ref().unwrap()[&2];
        let [Item::Group(g), Item::Text(t)] = &items[..] else {
            panic!("{items:?}")
        };
        assert_eq!((g.opacity, g.items.len()), (0.5, 2));
        let Item::Path(first) = &g.items[0] else {
            panic!()
        };
        let prism_core::Segment::Line(x, y) = first.segments[2] else {
            panic!()
        };
        assert!((x - 120.).abs() < 0.01 && (y - 100.).abs() < 0.01);
        // Strokes follow the document's size (2 units, shown twice as large).
        assert_eq!(first.width, 4.);
        assert_eq!((t.x, t.y, t.runs[0].size), (100., 50., 20.));
        assert_eq!(t.runs[0].fill, label_color());
        assert!(renderer.widgets.drawings.is_empty());
        // One that draws paths gets them alone, the group's opacity in
        // each.
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 300., Insets::default());
        let paths = &renderer.widgets.drawings[&2];
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].fill, Some(prism_core::rgba(0xff0000, 0.5)));
        assert_eq!(paths[1].fill.map(|c| c.a), Some(label_color().a * 0.5));
    }

    #[test]
    fn tab_contents_are_laid_out_in_their_viewport() {
        let init: AppInit = Arc::new(|cx: &mut prism_core::App| {
            let r = Router::new()
                .tab("A", "a", "/")
                .tab("B", "b", "/b")
                .route("/", |cx, _| cx.new_entity(|_| Page))
                .route("/b", |cx, _| cx.new_entity(|_| Page))
                .build(cx);
            cx.set_root(r);
        });
        let mut rt = Runtime::new(&init);
        let mut renderer = NativeRenderer::new(Fake::default());
        renderer.apply(&rt.render());
        renderer.layout(400., 800., Insets::default());
        let f = &renderer.widgets.frames;
        assert_eq!(f[&1], (0., 0., 400., 800.), "tabs");
        // tab (viewport below a fake 44pt bar) → stack fills it → screen.
        assert_eq!(f[&2], (0., 44., 400., 756.), "tab");
        assert_eq!(f[&3], (0., 0., 400., 756.), "stack inside tab");
    }
}
