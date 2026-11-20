//! One node's XAML elements, placed like the other native backends place
//! their views: absolute frames from prism-native's layout, in `Canvas`es.
//!
//! ```text
//! outer: Canvas              Canvas.Left/Top, size, opacity, transform, visibility
//! ├─ shadows (Canvas)        Composition drop shadows, under the box
//! ├─ box: Border             background, border, corner radii
//! ├─ kids: Canvas            the children (a ScrollViewer's content for a scroll view)
//! ├─ content                 TextBlock, Button, TextBox, ToggleSwitch… (leaves, bars)
//! ├─ bar                     a stack's header, a tab bar
//! └─ dash: Rectangle         a dashed or dotted border
//! ```
//!
//! The box, shadows and dash exist only once a style needs them.

use prism_core::{NodeId, NodeKind};
use windows_core::{Interface, Result};

use crate::bars::{self, Header, TabBar};
use crate::controls::{self, Content};
use crate::fluent::Palette;
use crate::scene::{Node, Rect};
use crate::style::{self, Shadows};
use crate::winui::xaml::Controls::{
    Border, Button, Canvas, ScrollBarVisibility, ScrollMode, ScrollViewer, UIElementCollection,
};
use crate::winui::xaml::Media::Brush;
use crate::winui::xaml::Shapes::Rectangle;
use crate::winui::xaml::{FrameworkElement, UIElement, Visibility};
use crate::winui::{transparent, ui};

/// A container's own bar.
pub enum Bar {
    None,
    Header(Header),
    Tabs(TabBar),
}

pub struct View {
    pub kind: NodeKind,
    pub outer: Canvas,
    pub shadows: Option<Shadows>,
    pub boxed: Option<Border>,
    pub kids: Canvas,
    pub scroll: Option<ScrollViewer>,
    pub content: Content,
    pub bar: Bar,
    pub dash: Option<Rectangle>,
    /// Inset shadows and borders of several colors (`decor.rs`).
    pub decor: crate::decor::Decor,
    /// The children in `kids`, in order (what `set_children` diffs).
    pub children: Vec<NodeId>,
    /// Whether the outer element takes pointer input (clickable, hover…).
    pub hit: bool,
    /// A clickable view's button, holding `kids`.
    pub button: Option<Button>,
    /// The frame the elements were sized for.
    pub frame: Rect,
}

impl View {
    pub fn new(id: NodeId, kind: NodeKind) -> Result<View> {
        let outer = Canvas::new()?;
        let kids = Canvas::new()?;
        let content = controls::create(id, kind)?;
        let mut scroll = None;
        if kind == NodeKind::Scroll {
            let sv = ScrollViewer::new()?;
            sv.SetContent(&kids)?;
            sv.SetHorizontalScrollBarVisibility(ScrollBarVisibility::Disabled)?;
            sv.SetVerticalScrollBarVisibility(ScrollBarVisibility::Auto)?;
            sv.SetIsTabStop(false)?;
            crate::events::scrolled(&sv, id)?;
            outer.Children()?.Append(&sv)?;
            scroll = Some(sv);
        } else if kind != NodeKind::Overlay {
            // An overlay's content is drawn above the window (`Win::layer`).
            outer.Children()?.Append(&kids)?;
        }
        if let Some(e) = content.element() {
            outer.Children()?.Append(&e)?;
        }
        let bar = match kind {
            NodeKind::Stack => {
                let h = Header::new(id)?;
                bars::place(&h.root)?;
                outer.Children()?.Append(&h.root)?;
                Bar::Header(h)
            }
            NodeKind::Tabs => {
                let t = TabBar::new(id)?;
                bars::place(&t.root)?;
                outer.Children()?.Append(&t.root)?;
                Bar::Tabs(t)
            }
            _ => Bar::None,
        };
        Ok(View {
            kind,
            outer,
            shadows: None,
            boxed: None,
            kids,
            scroll,
            content,
            bar,
            dash: None,
            decor: Default::default(),
            children: Vec::new(),
            hit: false,
            button: None,
            frame: Rect::default(),
        })
    }

    pub fn outer(&self) -> UIElement {
        ui(&self.outer).expect("UIElement")
    }

    fn insert_under(&self, e: &impl Interface, before_kids: bool) -> Result<()> {
        let children = self.outer.Children()?;
        let e: UIElement = e.cast()?;
        if before_kids {
            // Under everything already there except the shadows.
            let at = u32::from(self.shadows.is_some());
            children.InsertAt(at, &e)
        } else {
            children.Append(&e)
        }
    }

    /// The node's box, shadows, dash, control look, opacity and transform.
    pub fn apply_style(&mut self, n: &Node, pal: &Palette, sticky: f32) -> Result<()> {
        let s = &n.style;
        let r = Rect::new(0., 0., n.frame.w, n.frame.h);
        let text = controls::text_color(s, pal);
        // Shadows, under the box.
        let wants_shadow = s.shadows.iter().any(|sh| !sh.inset && sh.color.a > 0.);
        if wants_shadow && self.shadows.is_none() {
            let sh = Shadows::new()?;
            self.outer.Children()?.InsertAt(0, &sh.host)?;
            self.shadows = Some(sh);
        }
        if let Some(sh) = &mut self.shadows {
            sh.apply(s, pal, r)?;
        }
        // The box.
        let wants_box = style::has_box(s);
        if wants_box && self.boxed.is_none() {
            let b = Border::new()?;
            b.SetIsHitTestVisible(false)?;
            self.insert_under(&b, true)?;
            self.boxed = Some(b);
        }
        if let Some(b) = &self.boxed {
            style::apply_box(b, s, pal, text, r)?;
            b.SetWidth(r.w as f64)?;
            b.SetHeight(r.h as f64)?;
        }
        // A dashed or dotted border.
        let dashed = !s.border_none
            && s.border_width > 0.
            && s.border_style != prism_core::decoration::BorderStyle::Solid;
        if dashed && self.dash.is_none() {
            let d = Rectangle::new()?;
            d.SetIsHitTestVisible(false)?;
            self.insert_under(&d, false)?;
            self.dash = Some(d);
        }
        if let Some(d) = &self.dash {
            if dashed {
                d.SetVisibility(Visibility::Visible)?;
                style::apply_dash(d, s, pal, text, r)?;
            } else {
                d.SetVisibility(Visibility::Collapsed)?;
            }
        }
        let under = match (&self.boxed, &self.shadows) {
            (Some(b), _) => Some(ui(b)?),
            (None, Some(sh)) => Some(ui(&sh.host)?),
            _ => None,
        };
        self.decor.apply(&self.outer, under, n, pal, text)?;
        controls::style(&self.content, n, pal)?;
        style::apply_transform(&self.outer, s, r, sticky)?;
        self.clip(n)?;
        Ok(())
    }

    /// Children are clipped to the box (`overflow: hidden`, stacks, tabs),
    /// to its rounded corners if it has some.
    fn clip(&mut self, n: &Node) -> Result<()> {
        let clips = n.clips() && n.kind != NodeKind::Scroll;
        self.decor.clip_children(&self.kids, n, clips)
    }

    /// The node's frame (in its parent's coordinates) and size.
    pub fn place(&mut self, n: &Node, pal: &Palette, sticky: f32) -> Result<()> {
        let f = n.frame;
        let outer = self.outer();
        Canvas::SetLeft(&outer, f.x as f64)?;
        Canvas::SetTop(&outer, f.y as f64)?;
        self.outer.SetWidth(f.w.max(0.) as f64)?;
        self.outer.SetHeight(f.h.max(0.) as f64)?;
        let resized = (self.frame.w, self.frame.h) != (f.w, f.h);
        self.frame = f;
        size(&self.kids, f.w, f.h)?;
        if let Some(b) = &self.button {
            size(b, f.w, f.h)?;
        }
        if let Some(sv) = &self.scroll {
            size(sv, f.w, f.h)?;
            size(&self.kids, n.content.0.max(f.w), n.content.1.max(f.h))?;
        }
        if let Some(e) = self.content.element() {
            let fe: FrameworkElement = e.cast()?;
            let (x, y, w, h) = content_rect(n);
            Canvas::SetLeft(&e, x as f64)?;
            Canvas::SetTop(&e, y as f64)?;
            if matches!(self.content, Content::Radios(..)) {
                // Its own size: wider options overflow like CSS, not cut.
                fe.SetWidth(f64::NAN)?;
                fe.SetHeight(f64::NAN)?;
            } else {
                fe.SetWidth(w.max(0.) as f64)?;
                fe.SetHeight(h.max(0.) as f64)?;
            }
        }
        if resized {
            // Radii, gradients, shadows and clips depend on the size.
            self.apply_style(n, pal, sticky)?;
        }
        Ok(())
    }

    /// A scroll view's content size.
    pub fn content_size(&self, n: &Node) -> Result<()> {
        if self.scroll.is_some() {
            size(
                &self.kids,
                n.content.0.max(n.frame.w),
                n.content.1.max(n.frame.h),
            )?;
        }
        Ok(())
    }

    /// Takes pointer input (a clickable view, hover and press states, a
    /// context menu): a panel is hit only with a background.
    pub fn set_hit(&mut self, on: bool) -> Result<()> {
        if on == self.hit {
            return Ok(());
        }
        self.hit = on;
        if on {
            self.outer.SetBackground(&transparent()?)?;
        } else {
            self.outer.SetBackground(None::<&Brush>)?;
        }
        Ok(())
    }

    /// Makes the view a button (`set_clickable`): its children go inside a
    /// chrome-less WinUI `Button` in their place.
    pub fn make_button(&mut self, id: NodeId) -> Result<Option<Button>> {
        if self.button.is_some() || self.scroll.is_some() || self.kind == NodeKind::Overlay {
            return Ok(None);
        }
        let b = controls::clickable_host(id)?;
        let coll = self.outer.Children()?;
        let kids: UIElement = self.kids.cast()?;
        let mut at = 0u32;
        let found = coll.IndexOf(&kids, &mut at)?;
        if found {
            coll.RemoveAt(at)?;
        }
        b.SetContent(&self.kids)?;
        let e: UIElement = b.cast()?;
        if found {
            coll.InsertAt(at, &e)?;
        } else {
            coll.Append(&e)?;
        }
        size(&b, self.frame.w, self.frame.h)?;
        self.button = Some(b.clone());
        Ok(Some(b))
    }

    pub fn set_visible(&self, on: bool) -> Result<()> {
        let v = if on {
            Visibility::Visible
        } else {
            Visibility::Collapsed
        };
        if self.outer.Visibility()? != v {
            self.outer.SetVisibility(v)?;
        }
        Ok(())
    }

    /// Takes child `id` (element `e`) out (it moves to another parent).
    pub fn remove_child(&mut self, id: NodeId, e: &UIElement) -> Result<()> {
        if let Some(i) = self.children.iter().position(|c| *c == id) {
            self.children.remove(i);
        }
        let coll = self.kids.Children()?;
        let mut at = 0u32;
        if coll.IndexOf(e, &mut at)? {
            coll.RemoveAt(at)?;
        }
        Ok(())
    }

    /// Children, in order, keeping the elements that stay (a focused text
    /// box keeps its focus).
    pub fn set_children(&mut self, children: &[(NodeId, UIElement)]) -> Result<()> {
        let coll: UIElementCollection = self.kids.Children()?;
        let want: Vec<NodeId> = children.iter().map(|(id, _)| *id).collect();
        // Those that leave.
        let mut i = 0;
        while i < self.children.len() {
            if want.contains(&self.children[i]) {
                i += 1;
            } else {
                coll.RemoveAt(i as u32)?;
                self.children.remove(i);
            }
        }
        // In order: moved within, or inserted.
        for (at, (id, e)) in children.iter().enumerate() {
            if self.children.get(at) == Some(id) {
                continue;
            }
            if let Some(from) = self.children.iter().position(|c| c == id) {
                coll.Move(from as u32, at as u32)?;
                let c = self.children.remove(from);
                self.children.insert(at, c);
            } else {
                detach(e)?;
                coll.InsertAt(at as u32, e)?;
                self.children.insert(at, *id);
            }
        }
        Ok(())
    }
}

/// Where a leaf's content goes in its box: text inside its padding and
/// border; controls fill the box.
pub fn content_rect(n: &Node) -> (f32, f32, f32, f32) {
    let s = &n.style;
    let (w, h) = (n.frame.w, n.frame.h);
    match n.kind {
        NodeKind::Text | NodeKind::Timer | NodeKind::Image | NodeKind::Picture => {
            let b = if s.border_none {
                [0.; 4]
            } else {
                s.borders().map(|b| b.width)
            };
            let p = s.padding;
            let (l, t) = (p.left + b[3], p.top + b[0]);
            let (r, bt) = (p.right + b[1], p.bottom + b[2]);
            let cw = (w - l - r).max(0.);
            let ch = (h - t - bt).max(0.);
            if n.kind == NodeKind::Text || n.kind == NodeKind::Timer {
                // One DIP of slack: layout rounding must not wrap a line
                // measured to fit exactly.
                let shift = match s.text_align {
                    prism_core::TextAlign::Right => 1.,
                    prism_core::TextAlign::Center => 0.5,
                    _ => 0.,
                };
                (l - shift, t, cw + 1., ch)
            } else {
                (l, t, cw, ch)
            }
        }
        NodeKind::Toggle => {
            // WinUI's switch is 40 × 20 in a row 32 high.
            let ch = 32f32.max(h);
            (0., (h - ch) / 2., w, ch)
        }
        _ => (0., 0., w, h),
    }
}

fn size(e: &impl Interface, w: f32, h: f32) -> Result<()> {
    let fe: FrameworkElement = e.cast()?;
    fe.SetWidth(w.max(0.) as f64)?;
    fe.SetHeight(h.max(0.) as f64)
}

/// Takes an element out of the panel holding it (it moves elsewhere).
pub fn detach(e: &UIElement) -> Result<()> {
    let fe: FrameworkElement = e.cast()?;
    if let Ok(parent) = fe.Parent()
        && let Ok(panel) = parent.cast::<crate::winui::xaml::Controls::Panel>()
    {
        let coll = panel.Children()?;
        let mut index = 0u32;
        if coll.IndexOf(e, &mut index)? {
            coll.RemoveAt(index)?;
        }
    }
    Ok(())
}

/// The scroll view's axis.
pub fn scroll_axis(sv: &ScrollViewer, vertical: bool) -> Result<()> {
    if vertical {
        sv.SetHorizontalScrollMode(ScrollMode::Disabled)?;
        sv.SetHorizontalScrollBarVisibility(ScrollBarVisibility::Disabled)?;
        sv.SetVerticalScrollMode(ScrollMode::Enabled)?;
        sv.SetVerticalScrollBarVisibility(ScrollBarVisibility::Auto)?;
    } else {
        sv.SetVerticalScrollMode(ScrollMode::Disabled)?;
        sv.SetVerticalScrollBarVisibility(ScrollBarVisibility::Disabled)?;
        sv.SetHorizontalScrollMode(ScrollMode::Enabled)?;
        sv.SetHorizontalScrollBarVisibility(ScrollBarVisibility::Auto)?;
    }
    Ok(())
}
