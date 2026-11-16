//! Pointer, drag and keyboard events (see [`Div::on_hover`](crate::Div::on_hover),
//! [`Div::on_pointer_move`](crate::Div::on_pointer_move),
//! [`Div::on_drag`](crate::Div::on_drag) and
//! [`Div::on_key_down`](crate::Div::on_key_down)).
//!
//! Positions are in points, in the element's own coordinates: from its
//! top-left corner, whatever the layout direction.

/// A position in an element, in points from its top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    /// Distance from the element's left edge.
    pub x: f32,
    /// Distance from the element's top edge.
    pub y: f32,
}

impl Point {
    /// The point (`x`, `y`).
    pub fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }
}

/// What happened to the pointer over an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerPhase {
    /// It came over the element.
    Enter,
    /// It moved over the element.
    Move,
    /// It left the element.
    Leave,
}

impl PointerPhase {
    /// `enter`, `move` or `leave` (the name backends report).
    pub fn name(self) -> &'static str {
        match self {
            PointerPhase::Enter => "enter",
            PointerPhase::Move => "move",
            PointerPhase::Leave => "leave",
        }
    }

    /// Parses [`name`](Self::name)'s output.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "enter" => Some(PointerPhase::Enter),
            "move" => Some(PointerPhase::Move),
            "leave" => Some(PointerPhase::Leave),
            _ => None,
        }
    }
}

/// The pointer (mouse, trackpad, pen, a finger while it touches) over an
/// element: [`Div::on_pointer_move`](crate::Div::on_pointer_move).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    /// Entered, moved or left.
    pub phase: PointerPhase,
    /// Where the pointer is, in the element.
    pub position: Point,
}

/// The pointer came over an element or left it:
/// [`Div::on_hover`](crate::Div::on_hover).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoverEvent {
    /// `true` when it came over the element, `false` when it left.
    pub hovered: bool,
}

/// Files dropped on a drop target:
/// [`Div::on_drop_files`](crate::Div::on_drop_files) (#111).
#[derive(Clone, Debug, PartialEq)]
pub struct FileDropEvent {
    /// The dropped files the target accepts, readable like picked ones
    /// ([`PickedFile::read`](crate::PickedFile::read)): on the web the
    /// browser uploaded them over the app's connection first.
    pub files: Vec<crate::PickedFile>,
}

/// Where a drag is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DragPhase {
    /// The pointer went down on the element and moved past the platform's
    /// slop: the drag starts.
    Start,
    /// The pointer moved.
    Move,
    /// The pointer was released.
    End,
    /// The platform took the gesture over (a scroll, a system gesture, the
    /// window lost focus): undo what the drag previewed.
    Cancel,
}

impl DragPhase {
    /// `start`, `move`, `end` or `cancel` (the name backends report).
    pub fn name(self) -> &'static str {
        match self {
            DragPhase::Start => "start",
            DragPhase::Move => "move",
            DragPhase::End => "end",
            DragPhase::Cancel => "cancel",
        }
    }

    /// Parses [`name`](Self::name)'s output.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "start" => Some(DragPhase::Start),
            "move" => Some(DragPhase::Move),
            "end" => Some(DragPhase::End),
            "cancel" => Some(DragPhase::Cancel),
            _ => None,
        }
    }
}

/// A drag on an element: [`Div::on_drag`](crate::Div::on_drag).
///
/// `start` is where the pointer went down, in the element's coordinates at
/// that moment; `translation` is how far it moved since. Moving the
/// element while it is dragged (drag to reorder) does not change them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragEvent {
    /// Started, moved, ended or cancelled.
    pub phase: DragPhase,
    /// Where the pointer went down, in the element.
    pub start: Point,
    /// How far the pointer moved since it went down, in points.
    pub translation: Point,
}

impl DragEvent {
    /// Where the pointer is now, in the element's coordinates at the start
    /// of the drag (`start + translation`).
    pub fn position(&self) -> Point {
        Point::new(
            self.start.x + self.translation.x,
            self.start.y + self.translation.y,
        )
    }
}

/// Modifier keys held during a key press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Shift.
    pub shift: bool,
    /// Control.
    pub control: bool,
    /// Option / Alt.
    pub alt: bool,
    /// Command (⌘) on Apple platforms, the Windows / Super key elsewhere.
    pub meta: bool,
}

impl Modifiers {
    /// Bit mask as backends report it: 1 shift, 2 control, 4 alt, 8 meta.
    pub fn bits(self) -> u8 {
        self.shift as u8
            | (self.control as u8) << 1
            | (self.alt as u8) << 2
            | (self.meta as u8) << 3
    }

    /// Parses [`bits`](Self::bits).
    pub fn from_bits(bits: u8) -> Self {
        Modifiers {
            shift: bits & 1 != 0,
            control: bits & 2 != 0,
            alt: bits & 4 != 0,
            meta: bits & 8 != 0,
        }
    }

    /// The platform's shortcut modifier is held: Command on Apple
    /// platforms, Control elsewhere.
    pub fn command(self) -> bool {
        if cfg!(target_vendor = "apple") {
            self.meta
        } else {
            self.control
        }
    }

    /// No modifier is held.
    pub fn is_empty(self) -> bool {
        self.bits() == 0
    }
}

/// A key was pressed: [`Div::on_key_down`](crate::Div::on_key_down) and
/// [`on_global_key_down`](crate::Div::on_global_key_down).
///
/// `key` uses the web's `KeyboardEvent.key` names on every platform: a
/// printed character (`"a"`, `"A"`, `"7"`, `" "`), or one of
/// `"Escape"`, `"Enter"`, `"Tab"`, `"Backspace"`, `"Delete"`,
/// `"ArrowUp"`, `"ArrowDown"`, `"ArrowLeft"`, `"ArrowRight"`, `"Home"`,
/// `"End"`, `"PageUp"`, `"PageDown"`, `"F1"`…`"F12"`, `"MediaPlayPause"`,
/// `"ContextMenu"`. A TV remote reports its arrows and `"Enter"` (select),
/// `"MediaPlayPause"`, and `"Escape"` for Menu / Back when the app listens
/// for it; an Android D-pad its arrows and `"Enter"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key's name (see above).
    pub key: String,
    /// Modifiers held.
    pub modifiers: Modifiers,
    /// Sent again because the key is held down.
    pub repeat: bool,
    /// While a text input or area has the focus: its selection or caret
    /// when the key was pressed, before the key acts (#86), in characters
    /// like [`on_select`](crate::TextInput::on_select). So Enter can split
    /// a block at the caret, Backspace at `0..0` merge it with the previous
    /// one, ↑ on the first line move to the previous block. `None` when no
    /// field has the focus (or the platform cannot tell: watchOS).
    pub selection: Option<crate::SelectionEvent>,
}

impl KeyEvent {
    /// A press of `key` without modifiers.
    pub fn new(key: impl Into<String>) -> Self {
        KeyEvent {
            key: key.into(),
            modifiers: Modifiers::default(),
            repeat: false,
            selection: None,
        }
    }

    /// The same press, made while a text field had `selection`.
    pub fn with_selection(mut self, selection: impl Into<crate::SelectionEvent>) -> Self {
        self.selection = Some(selection.into());
        self
    }

    /// Whether this is `key` (`e.is("Escape")`). Single letters compare
    /// without case.
    pub fn is(&self, key: &str) -> bool {
        self.key == key || (key.chars().count() == 1 && self.key.eq_ignore_ascii_case(key))
    }

    /// Whether the key prints a character (as opposed to Escape, arrows…).
    pub fn is_character(&self) -> bool {
        self.key.chars().count() == 1
    }
}

/// Which pointer events a node reports ([`Patch::SetPointerEvents`](crate::Patch::SetPointerEvents)).
pub mod pointer_mask {
    /// Enter and leave.
    pub const HOVER: u8 = 1;
    /// Moves over the node.
    pub const MOVE: u8 = 2;
    /// Drags starting on the node.
    pub const DRAG: u8 = 4;
    /// Mouse wheel and trackpad scrolling over the node (#133).
    pub const WHEEL: u8 = 8;
    /// Pinches over the node (#133).
    pub const MAGNIFY: u8 = 16;
}

/// Where a trackpad gesture is: [`WheelEvent::phase`],
/// [`MagnifyEvent::phase`] (#133).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GesturePhase {
    /// No gesture: a mouse wheel's notch, or a platform that does not say
    /// (the web).
    #[default]
    None,
    /// The fingers started the gesture.
    Start,
    /// The fingers moved.
    Move,
    /// The fingers left (scrolling may go on with [`Momentum`](Self::Momentum)),
    /// or the momentum stopped.
    End,
    /// Scrolling goes on by inertia after the fingers left (trackpads).
    Momentum,
}

impl GesturePhase {
    /// `none`, `start`, `move`, `end` or `momentum` (as backends report it).
    pub fn name(self) -> &'static str {
        match self {
            GesturePhase::None => "none",
            GesturePhase::Start => "start",
            GesturePhase::Move => "move",
            GesturePhase::End => "end",
            GesturePhase::Momentum => "momentum",
        }
    }

    /// Parses [`name`](Self::name)'s output.
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::None,
            Self::Start,
            Self::Move,
            Self::End,
            Self::Momentum,
        ]
        .into_iter()
        .find(|p| p.name() == name)
    }
}

/// The mouse wheel turned or two fingers scrolled on a trackpad over an
/// element: [`Div::on_wheel`](crate::Div::on_wheel) (#133).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelEvent {
    /// How far to scroll, in points, like the web's `deltaX` / `deltaY`:
    /// positive `y` scrolls down (toward the end), positive `x` toward the
    /// trailing edge, after the user's « natural scrolling » setting. To
    /// pan a canvas, move its content by `-delta`.
    pub delta: Point,
    /// Where the pointer is, in the element.
    pub position: Point,
    /// Keys held (⌘ / Ctrl + wheel commonly zooms, Shift scrolls sideways).
    pub modifiers: Modifiers,
    /// The trackpad gesture's phase; [`GesturePhase::None`] for a wheel.
    pub phase: GesturePhase,
    /// Point-precise deltas (a trackpad, a smooth wheel) rather than coarse
    /// wheel steps.
    pub precise: bool,
}

/// Two fingers pinched over an element: [`Div::on_magnify`](crate::Div::on_magnify)
/// (#133). Browsers report a trackpad pinch as Ctrl + wheel: it arrives here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MagnifyEvent {
    /// How much to scale by since the previous event: above 1 zooms in.
    /// Multiply the zoom by it, around [`position`](Self::position).
    pub scale: f32,
    /// Where the pointer (or the pinch's center) is, in the element.
    pub position: Point,
    /// The gesture's phase ([`GesturePhase::None`] where unknown).
    pub phase: GesturePhase,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_bits_round_trip() {
        for p in [PointerPhase::Enter, PointerPhase::Move, PointerPhase::Leave] {
            assert_eq!(PointerPhase::from_name(p.name()), Some(p));
        }
        for p in [
            DragPhase::Start,
            DragPhase::Move,
            DragPhase::End,
            DragPhase::Cancel,
        ] {
            assert_eq!(DragPhase::from_name(p.name()), Some(p));
        }
        let m = Modifiers {
            shift: true,
            meta: true,
            ..Default::default()
        };
        assert_eq!(m.bits(), 9);
        assert_eq!(Modifiers::from_bits(9), m);
        assert!(Modifiers::default().is_empty());
        let k = KeyEvent::new("A");
        assert!(k.is("a") && k.is("A") && k.is_character());
        assert!(!KeyEvent::new("Escape").is_character());
        let d = DragEvent {
            phase: DragPhase::Move,
            start: Point::new(10., 5.),
            translation: Point::new(-4., 20.),
        };
        assert_eq!(d.position(), Point::new(6., 25.));
    }
}
