//! Custom window chrome (`WindowChrome::Custom`, #67), as on macOS: the
//! title bar goes and the content reaches the window's top edge; the
//! caption buttons stay, drawn by the system over the content's top right
//! corner (`titlebar.rs`). What moves the window is a caption region: a
//! press drags it, a double click maximizes it and a right click opens its
//! system menu. The caption regions are:
//!
//! - the window's top bar, when Prism draws it: the menu bar's row, else a
//!   tab bar or a navigation header along the window's top edge. Its empty
//!   part moves the window, as a Windows 11 title bar with menus or tabs
//!   does; its menus, tabs and buttons keep their clicks;
//! - `.window_drag()` views, in the app's own content.
//!
//! What keeps its own presses inside them (buttons, controls, `on_click`,
//! draggable views) is a passthrough region. No Win32 types: tested on
//! every host.

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

impl Regions {
    /// Adds `other`'s regions.
    pub fn extend(&mut self, other: Regions) {
        self.caption.extend(other.caption);
        self.passthrough.extend(other.passthrough);
    }

    /// The regions moved down by `dy`.
    pub fn moved(mut self, dy: f32) -> Regions {
        for r in self.caption.iter_mut().chain(&mut self.passthrough) {
            r.y += dy;
        }
        self
    }
}

/// A bar Prism draws (the menu bar, a tab bar, a navigation header): its
/// frame and what takes its own clicks in it (menus, tabs, buttons), in
/// the window's units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopBar {
    pub frame: Rect,
    pub controls: Vec<Rect>,
}

/// A bar along the window's top edge: its empty part moves the window (a
/// caption region, around the caption buttons' corner `buttons`), what
/// takes clicks in it keeps them (passthrough).
pub fn top_bar(bar: &TopBar, buttons: Option<&WindowControls>) -> Regions {
    let caption = match buttons {
        Some(b) => around(&bar.frame, b),
        None => vec![bar.frame],
    };
    Regions {
        caption: caption
            .into_iter()
            .filter(|r| r.w > 0. && r.h > 0.)
            .collect(),
        passthrough: bar
            .controls
            .iter()
            .copied()
            .filter(|r| r.w > 0. && r.h > 0.)
            .collect(),
    }
}

/// `r` less the caption buttons' corner: the part beside them, full
/// height, and the part under them.
fn around(r: &Rect, b: &WindowControls) -> Vec<Rect> {
    let (left, right) = (r.x.max(b.x), (r.x + r.w).min(b.right()));
    let bottom = b.bottom();
    if left >= right || r.y >= bottom {
        // Not under them.
        return vec![*r];
    }
    let mut out = Vec::new();
    // Beside them: the side with more room (the other is the window's
    // edge, or a sliver).
    if left > r.x {
        out.push(Rect::new(r.x, r.y, left - r.x, r.h));
    }
    if right < r.x + r.w {
        out.push(Rect::new(right, r.y, r.x + r.w - right, r.h));
    }
    // Under them.
    if bottom < r.y + r.h {
        out.push(Rect::new(left, bottom, right - left, r.y + r.h - bottom));
    }
    out
}

/// How much of `r`'s left and right ends the caption buttons `b` cover.
pub fn overlap(r: &Rect, b: &WindowControls) -> (f32, f32) {
    if b.bottom() <= r.y || b.y >= r.y + r.h {
        return (0., 0.);
    }
    let left = (b.right() - r.x).clamp(0., r.w);
    let right = (r.x + r.w - b.x).clamp(0., r.w);
    if left >= r.w && right >= r.w {
        // Wider than the bar: all of it.
        return (r.w, 0.);
    }
    // The end they are at.
    if b.x + b.width / 2. <= r.x + r.w / 2. {
        (left, 0.)
    } else {
        (0., right)
    }
}

/// The drag regions of the views shown: none while an overlay is open
/// (its smoke or light dismiss takes the presses). `keeps(id, node)` says
/// whether a view keeps its own presses; a `.window_drag()` view that
/// does is no drag region (its click wins, as on macOS). `bar(id, node)`
/// is the bar a view shows at its top (a tab bar, a navigation header),
/// if any: those along the window's top edge move the window, around the
/// caption buttons (`buttons`).
pub fn regions(
    scene: &Scene,
    keeps: impl Fn(NodeId, &Node) -> bool,
    bar: impl Fn(NodeId, &Node) -> Option<TopBar>,
    buttons: Option<&WindowControls>,
) -> Regions {
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
        if !inside
            && rect.y.abs() < 0.5
            && let Some(b) = bar(id, n)
        {
            r.extend(top_bar(&b, buttons));
        }
        for &c in scene.shown_children(id).iter() {
            stack.push((c, inside || drag));
        }
    }
    r
}

/// Whether a bar Prism draws along the window's top edge (a tab bar, a
/// navigation header: `bar(id, node)` is its frame) is under the caption
/// buttons `b`. That bar makes room for them, so the app's own views do
/// not reach them: `cx.window_controls()` is then `None`, and an app's own
/// title bar would be a second one under the bar.
pub fn bar_under(
    scene: &Scene,
    bar: impl Fn(NodeId, &Node) -> Option<Rect>,
    b: &WindowControls,
) -> bool {
    let mut stack: Vec<NodeId> = scene.root.into_iter().collect();
    while let Some(id) = stack.pop() {
        let Some(n) = scene.get(id) else { continue };
        if n.hidden {
            continue;
        }
        let rect = scene.window_rect(id);
        // Lower down: nothing inside reaches the top edge.
        if rect.y >= 0.5 {
            continue;
        }
        if let Some(frame) = bar(id, n)
            && frame.y.abs() < 0.5
            && overlap(&frame, b) != (0., 0.)
        {
            return true;
        }
        stack.extend(scene.shown_children(id).iter().copied());
    }
    false
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

    /// 138 × 32 caption buttons at the right of an 800-wide window.
    const BUTTONS: WindowControls = WindowControls {
        x: 662.,
        y: 0.,
        width: 138.,
        height: 32.,
    };

    #[test]
    fn a_menu_rows_empty_part_moves_the_window() {
        // File and View at the left of a 40-high row.
        let row = TopBar {
            frame: Rect::new(0., 0., 800., 40.),
            controls: vec![Rect::new(0., 4., 40., 32.), Rect::new(40., 4., 48., 32.)],
        };
        let r = top_bar(&row, Some(&BUTTONS));
        // The row beside the buttons, and the strip under them.
        assert_eq!(
            r.caption,
            vec![Rect::new(0., 0., 662., 40.), Rect::new(662., 32., 138., 8.)]
        );
        // The menus keep their clicks.
        assert_eq!(r.passthrough, row.controls);
        // Without buttons (none reported yet): the whole row.
        assert_eq!(
            top_bar(&row, None).caption,
            vec![Rect::new(0., 0., 800., 40.)]
        );
    }

    #[test]
    fn captions_go_around_the_buttons_on_either_side() {
        let row = Rect::new(0., 0., 800., 32.);
        // As high as the row: nothing under them.
        assert_eq!(around(&row, &BUTTONS), vec![Rect::new(0., 0., 662., 32.)]);
        // Right to left: at the left.
        let rtl = WindowControls { x: 0., ..BUTTONS };
        assert_eq!(around(&row, &rtl), vec![Rect::new(138., 0., 662., 32.)]);
        // A bar they do not reach.
        let low = Rect::new(0., 40., 800., 48.);
        assert_eq!(around(&low, &BUTTONS), vec![low]);
        let left_pane = Rect::new(0., 0., 300., 48.);
        assert_eq!(around(&left_pane, &BUTTONS), vec![left_pane]);
    }

    #[test]
    fn bars_make_room_for_the_buttons_over_their_end() {
        let bar = Rect::new(0., 0., 800., 48.);
        assert_eq!(overlap(&bar, &BUTTONS), (0., 138.));
        let rtl = WindowControls { x: 0., ..BUTTONS };
        assert_eq!(overlap(&bar, &rtl), (138., 0.));
        // A split view's panes: only the one under them.
        assert_eq!(overlap(&Rect::new(0., 0., 300., 48.), &BUTTONS), (0., 0.));
        assert_eq!(
            overlap(&Rect::new(300., 0., 500., 48.), &BUTTONS),
            (0., 138.)
        );
        // Partly under them.
        assert_eq!(overlap(&Rect::new(0., 0., 700., 48.), &BUTTONS), (0., 38.));
        // Lower down.
        assert_eq!(overlap(&Rect::new(0., 40., 800., 48.), &BUTTONS), (0., 0.));
    }

    /// root(800×600, a tab bar's view) ⊃ page(0,48 800×552) ⊃
    /// stack(0,0 800×552, a header).
    fn tabs() -> Scene {
        let mut s = Scene::default();
        node(&mut s, 1, Rect::new(0., 0., 800., 600.));
        node(&mut s, 2, Rect::new(0., 48., 800., 552.));
        node(&mut s, 3, Rect::new(0., 0., 800., 552.));
        s.set_children(1, &[2]);
        s.set_children(2, &[3]);
        s.root = Some(1);
        s
    }

    /// Views 1 (tabs: two at the left) and 3 (a header: Back) have bars.
    fn bars(id: NodeId, _: &Node) -> Option<TopBar> {
        match id {
            1 => Some(TopBar {
                frame: Rect::new(0., 0., 800., 48.),
                controls: vec![Rect::new(4., 8., 80., 32.), Rect::new(84., 8., 80., 32.)],
            }),
            3 => Some(TopBar {
                frame: Rect::new(0., 48., 800., 48.),
                controls: vec![Rect::new(8., 56., 40., 32.)],
            }),
            _ => None,
        }
    }

    #[test]
    fn only_the_bar_along_the_top_edge_moves_the_window() {
        let s = tabs();
        let r = regions(&s, keeps, bars, Some(&BUTTONS));
        // The tab bar, around the buttons; not the header under it.
        assert_eq!(
            r.caption,
            vec![
                Rect::new(0., 0., 662., 48.),
                Rect::new(662., 32., 138., 16.)
            ]
        );
        assert_eq!(r.passthrough.len(), 2);
        // Without tabs, the header is the top bar.
        let mut s = tabs();
        s.set_children(1, &[]);
        s.root = Some(3);
        s.nodes.get_mut(&3).unwrap().parent = None;
        let header = |id: NodeId, n: &Node| {
            bars(id, n).map(|b| TopBar {
                frame: Rect::new(0., 0., 800., 48.),
                controls: b.controls,
            })
        };
        let r = regions(&s, keeps, header, Some(&BUTTONS));
        assert_eq!(r.caption[0], Rect::new(0., 0., 662., 48.));
        assert_eq!(r.passthrough, vec![Rect::new(8., 56., 40., 32.)]);
    }

    #[test]
    fn a_bar_under_the_buttons_takes_their_room_from_the_app() {
        let frame = |id: NodeId, n: &Node| bars(id, n).map(|b| b.frame);
        // The tab bar along the top edge is under them.
        assert!(bar_under(&tabs(), frame, &BUTTONS));
        // A header lower down (under the tab bar) is not.
        let header_only = |id: NodeId, n: &Node| (id == 3).then(|| bars(id, n).unwrap().frame);
        assert!(!bar_under(&tabs(), header_only, &BUTTONS));
        // Without tabs, the header is the top bar.
        let mut s = tabs();
        s.set_children(1, &[]);
        s.root = Some(3);
        s.nodes.get_mut(&3).unwrap().parent = None;
        let top = |id: NodeId, _: &Node| (id == 3).then(|| Rect::new(0., 0., 800., 48.));
        assert!(bar_under(&s, top, &BUTTONS));
        // Hidden (a bare screen, no header): the app's views reach them.
        s.nodes.get_mut(&3).unwrap().hidden = true;
        assert!(!bar_under(&s, top, &BUTTONS));
        // A split view's left pane has a header, the right one none.
        let left = |id: NodeId, _: &Node| (id == 1).then(|| Rect::new(0., 0., 300., 48.));
        assert!(!bar_under(&tabs(), left, &BUTTONS));
        // No bars at all.
        assert!(!bar_under(&window(), |_, _| None, &BUTTONS));
    }

    #[test]
    fn regions_move_under_the_menu_bar() {
        let r = regions(&window(), keeps, |_, _| None, None).moved(40.);
        assert_eq!(r.caption, vec![Rect::new(0., 40., 800., 40.)]);
        assert_eq!(r.passthrough, vec![Rect::new(100., 40., 80., 40.)]);
    }

    #[test]
    fn drag_views_are_captions_with_holes_for_what_keeps_its_presses() {
        let s = window();
        let r = regions(&s, keeps, |_, _| None, None);
        assert_eq!(r.caption, vec![Rect::new(0., 0., 800., 40.)]);
        // The tab keeps its clicks; the title moves the window.
        assert_eq!(r.passthrough, vec![Rect::new(100., 0., 80., 40.)]);
    }

    #[test]
    fn no_regions_without_drag_views_hidden_ones_or_clickable_ones() {
        let mut s = window();
        s.nodes.get_mut(&2).unwrap().style.window_drag = false;
        assert_eq!(regions(&s, keeps, |_, _| None, None), Regions::default());
        // A clickable drag view: its click wins.
        s.nodes.get_mut(&3).unwrap().style.window_drag = true;
        assert_eq!(regions(&s, keeps, |_, _| None, None), Regions::default());
        // Hidden.
        let mut s = window();
        s.nodes.get_mut(&2).unwrap().hidden = true;
        assert_eq!(regions(&s, keeps, |_, _| None, None), Regions::default());
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
