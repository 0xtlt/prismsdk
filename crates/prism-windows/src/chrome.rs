//! Custom window chrome (`WindowChrome::Custom`, #67), as on macOS: the
//! title bar goes and the content reaches the window's top edge; the
//! caption buttons stay, drawn by the system over the content's top right
//! corner (`titlebar.rs`). Only `.window_drag()` regions move the window:
//! they are its caption regions, so a press drags it, a double click
//! maximizes it and a right click opens its system menu. What keeps its
//! own presses inside them (buttons, controls, `on_click`, draggable views)
//! is a passthrough region. No Win32 types: tested on every host.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use prism_core::{NodeId, WindowControls};

use crate::scene::{Node, Rect, Scene};

/// The window's drag regions and the holes in them, in the window's units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Regions {
    /// `.window_drag()` views.
    pub caption: Vec<Rect>,
    /// What keeps its presses inside them.
    pub passthrough: Vec<Rect>,
}

/// The drag regions of the views shown: none while an overlay is open
/// (its smoke or light dismiss takes the presses). `keeps(id, node)` says
/// whether a view keeps its own presses; a `.window_drag()` view that
/// does is no drag region (its click wins, as on macOS).
pub fn regions(scene: &Scene, keeps: impl Fn(NodeId, &Node) -> bool) -> Regions {
    let mut r = Regions::default();
    if !scene.open_overlays().is_empty() {
        return r;
    }
    // (view, inside a drag region)
    let mut stack: Vec<(NodeId, bool)> = scene.roots().map(|id| (id, false)).collect();
    while let Some((id, inside)) = stack.pop() {
        let Some(n) = scene.get(id) else { continue };
        if n.hidden || n.style.inert || !n.style.hit_test {
            continue;
        }
        let rect = scene.window_rect(id);
        let area = rect.w > 0. && rect.h > 0.;
        let keeps = keeps(id, n);
        if inside && keeps {
            // Its whole frame keeps its presses.
            if area {
                r.passthrough.push(rect);
            }
            continue;
        }
        let drag = n.style.window_drag && !keeps;
        if drag && area {
            r.caption.push(rect);
        }
        for &c in scene.shown_children(id).iter() {
            stack.push((c, inside || drag));
        }
    }
    r
}

/// A rectangle in physical pixels (`[x, y, width, height]`) at scale `k`:
/// rounded to the nearest pixel, or grown to whole pixels (`grow`).
pub fn px(r: &Rect, k: f32, grow: bool) -> [i32; 4] {
    let (x0, y0, x1, y1) = (r.x * k, r.y * k, (r.x + r.w) * k, (r.y + r.h) * k);
    let (x0, y0, x1, y1) = if grow {
        (x0.floor(), y0.floor(), x1.ceil(), y1.ceil())
    } else {
        (x0.round(), y0.round(), x1.round(), y1.round())
    };
    [x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32]
}

/// Where the caption buttons are, from the system's reserved corners
/// (`AppWindowTitleBar`'s `LeftInset` / `RightInset` and `Height`, pixels)
/// in a client area `client_width` pixels wide, at scale `k`: on the
/// right, or on the left in a right-to-left window.
pub fn controls(
    client_width: i32,
    left: i32,
    right: i32,
    height: i32,
    k: f32,
) -> Option<WindowControls> {
    if height <= 0 || k <= 0. {
        return None;
    }
    let (x, width) = if right > 0 {
        (client_width - right, right)
    } else if left > 0 {
        (0, left)
    } else {
        return None;
    };
    Some(WindowControls {
        x: x as f32 / k,
        y: 0.,
        width: width as f32 / k,
        height: height as f32 / k,
    })
}

#[cfg(test)]
mod tests {
    use prism_core::NodeKind;

    use super::*;

    fn node(s: &mut Scene, id: NodeId, frame: Rect) {
        s.create(id, NodeKind::View);
        s.nodes.get_mut(&id).unwrap().frame = frame;
    }

    /// root(800×600) ⊃ bar(0,0 800×40, drag) ⊃ tab(100,0 80×40, clickable),
    /// title(200,10 100×20); root ⊃ body(0,40 800×560).
    fn window() -> Scene {
        let mut s = Scene::default();
        node(&mut s, 1, Rect::new(0., 0., 800., 600.));
        node(&mut s, 2, Rect::new(0., 0., 800., 40.));
        node(&mut s, 3, Rect::new(100., 0., 80., 40.));
        node(&mut s, 4, Rect::new(200., 10., 100., 20.));
        node(&mut s, 5, Rect::new(0., 40., 800., 560.));
        s.set_children(1, &[2, 5]);
        s.set_children(2, &[3, 4]);
        s.root = Some(1);
        s.nodes.get_mut(&2).unwrap().style.window_drag = true;
        s.nodes.get_mut(&3).unwrap().clickable = true;
        s
    }

    fn keeps(_: NodeId, n: &Node) -> bool {
        n.clickable
    }

    #[test]
    fn drag_views_are_captions_with_holes_for_what_keeps_its_presses() {
        let s = window();
        let r = regions(&s, keeps);
        assert_eq!(r.caption, vec![Rect::new(0., 0., 800., 40.)]);
        // The tab keeps its clicks; the title moves the window.
        assert_eq!(r.passthrough, vec![Rect::new(100., 0., 80., 40.)]);
    }

    #[test]
    fn no_regions_without_drag_views_hidden_ones_or_clickable_ones() {
        let mut s = window();
        s.nodes.get_mut(&2).unwrap().style.window_drag = false;
        assert_eq!(regions(&s, keeps), Regions::default());
        // A clickable drag view: its click wins.
        s.nodes.get_mut(&3).unwrap().style.window_drag = true;
        assert_eq!(regions(&s, keeps), Regions::default());
        // Hidden.
        let mut s = window();
        s.nodes.get_mut(&2).unwrap().hidden = true;
        assert_eq!(regions(&s, keeps), Regions::default());
    }

    #[test]
    fn pixels_round_or_grow() {
        let r = Rect::new(10.2, 0., 20.5, 40.);
        assert_eq!(px(&r, 1.5, false), [15, 0, 31, 60]);
        assert_eq!(px(&r, 1.5, true), [15, 0, 32, 60]);
    }

    #[test]
    fn the_caption_buttons_sit_in_the_reserved_corner() {
        // 1200 px wide at 150 %: 138 DIPs of buttons at the right.
        let c = controls(1200, 0, 207, 48, 1.5).unwrap();
        assert_eq!((c.x, c.y, c.width, c.height), (662., 0., 138., 32.));
        assert_eq!(c.insets(800.), (0., 138.));
        // Right to left: on the left.
        let c = controls(1200, 207, 0, 48, 1.5).unwrap();
        assert_eq!((c.x, c.width), (0., 138.));
        assert_eq!(c.insets(800.), (138., 0.));
        assert_eq!(controls(1200, 0, 0, 48, 1.5), None);
    }
}
