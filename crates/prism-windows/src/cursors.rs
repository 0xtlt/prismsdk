//! The pointer's shape over a view (`.cursor(..)`): one of WinUI's system
//! cursors (`InputSystemCursor`, set as the view's `ProtectedCursor`).
//! The style's cursor, else a pointing hand over a clickable view, as on
//! macOS and Linux; other views show the cursor of what is around them.
//! Windows has no open or closed hand and no zoom cursor: grabbing shows
//! the move arrows, zooming the arrow. No Win32 types: tested on every
//! host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::{Cursor, IdMap, NodeId};

/// A system cursor; its value is `InputSystemCursorShape`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Shape {
    Arrow = 0,
    Cross = 1,
    Hand = 3,
    Help = 4,
    IBeam = 5,
    SizeAll = 6,
    SizeNortheastSouthwest = 7,
    SizeNorthSouth = 8,
    SizeNorthwestSoutheast = 9,
    SizeWestEast = 10,
    UniversalNo = 11,
    AppStarting = 16,
}

/// The system cursor for a Prism one.
pub fn system(c: Cursor) -> Shape {
    match c {
        Cursor::Default | Cursor::ZoomIn | Cursor::ZoomOut => Shape::Arrow,
        Cursor::Pointer => Shape::Hand,
        Cursor::Text => Shape::IBeam,
        Cursor::Crosshair => Shape::Cross,
        Cursor::Move | Cursor::Grab | Cursor::Grabbing => Shape::SizeAll,
        Cursor::NotAllowed => Shape::UniversalNo,
        Cursor::ResizeHorizontal | Cursor::ResizeE | Cursor::ResizeW => Shape::SizeWestEast,
        Cursor::ResizeVertical | Cursor::ResizeN | Cursor::ResizeS => Shape::SizeNorthSouth,
        Cursor::ResizeNeSw | Cursor::ResizeNe | Cursor::ResizeSw => Shape::SizeNortheastSouthwest,
        Cursor::ResizeNwSe | Cursor::ResizeNw | Cursor::ResizeSe => Shape::SizeNorthwestSoutheast,
        Cursor::Help => Shape::Help,
        Cursor::Progress => Shape::AppStarting,
    }
}

/// A view's cursor: its style's, else a hand when it is clickable, else
/// none (the cursor of what is around it).
pub fn shape(style: Option<Cursor>, clickable: bool) -> Option<Shape> {
    style.map(system).or(clickable.then_some(Shape::Hand))
}

/// The cursor each view shows.
#[derive(Default)]
pub struct Shapes {
    shown: IdMap<NodeId, Shape>,
}

impl Shapes {
    /// View `id` shows `shape` now; returns whether that changed.
    pub fn set(&mut self, id: NodeId, shape: Option<Shape>) -> bool {
        match shape {
            Some(s) => self.shown.insert(id, s) != Some(s),
            None => self.shown.remove(&id).is_some(),
        }
    }

    /// View `id` is gone.
    pub fn forget(&mut self, id: NodeId) {
        self.shown.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_style_wins_then_clickable_views_get_a_hand() {
        assert_eq!(shape(None, false), None);
        assert_eq!(shape(None, true), Some(Shape::Hand));
        assert_eq!(shape(Some(Cursor::Text), true), Some(Shape::IBeam));
        // An explicit arrow over a clickable view.
        assert_eq!(shape(Some(Cursor::Default), true), Some(Shape::Arrow));
    }

    #[test]
    fn every_cursor_has_the_nearest_system_one() {
        assert_eq!(system(Cursor::ResizeE), Shape::SizeWestEast);
        assert_eq!(system(Cursor::ResizeN), Shape::SizeNorthSouth);
        assert_eq!(system(Cursor::ResizeNe), Shape::SizeNortheastSouthwest);
        assert_eq!(system(Cursor::ResizeSe), Shape::SizeNorthwestSoutheast);
        assert_eq!(system(Cursor::Grabbing), Shape::SizeAll);
        assert_eq!(system(Cursor::NotAllowed), Shape::UniversalNo);
        assert_eq!(system(Cursor::Progress), Shape::AppStarting);
        assert_eq!(system(Cursor::ZoomIn), Shape::Arrow);
        // InputSystemCursorShape's values.
        assert_eq!(Shape::Hand as i32, 3);
        assert_eq!(Shape::AppStarting as i32, 16);
    }

    #[test]
    fn a_shape_is_set_once() {
        let mut s = Shapes::default();
        assert!(s.set(1, Some(Shape::Hand)));
        assert!(!s.set(1, Some(Shape::Hand)));
        assert!(s.set(1, Some(Shape::IBeam)));
        assert!(s.set(1, None));
        assert!(!s.set(1, None));
    }
}
