//! One node's XAML elements, placed like the other native backends place
//! their views: absolute frames from prism-native's layout, in `Canvas`es.
//!
//! ```text
//! outer: Canvas              Canvas.Left/Top, size, opacity, transform, visibility
//! ├─ shadows (Canvas)        Composition drop shadows, under the box
//! ├─ box: Border             background, border, corner radii
//! ├─ decor                   inset shadows, sides of several colors (`decor.rs`)
//! ├─ kids: Canvas            the children (a ScrollViewer's content for a scroll view)
//! ├─ content                 TextBlock, Button, TextBox, ToggleSwitch… (leaves, bars)
//! ├─ bar                     a stack's header, a tab bar
//! └─ dash: Rectangle         a dashed or dotted border
//! ```
//!
//! The box, shadows, decor and dash exist only once a style needs them;
//! `kids` joins the tree with the first child (a leaf has none: an empty
//! canvas in the tree still costs XAML a layout and a visual).

use prism_core::{NodeId, NodeKind};
use prism_native::text::StyledText;
use windows_core::{Interface, Result};

use crate::bars::{self, Header, TabBar};
use crate::controls::{self, Content};
use crate::fluent::Palette;
use crate::scene::{Node, Rect};
use crate::style::{self, Shadows};
use crate::winui::xaml::Controls::{
    Border, Button, Canvas, Image, ScrollBarVisibility, ScrollMode, ScrollViewer,
    UIElementCollection,
};
use crate::winui::xaml::Media::Imaging::BitmapSource;
use crate::winui::xaml::Media::{Brush, RectangleGeometry};
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
    pub id: NodeId,
    pub kind: NodeKind,
    pub outer: Canvas,
    pub shadows: Option<Shadows>,
    pub boxed: Option<Border>,
    pub kids: Canvas,
    /// `kids` is in the tree (in `outer`, the scroll viewer or the button).
    kids_in: bool,
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
    /// A text field's leading icon (`fieldicon.rs`).
    pub field_icon: Option<crate::winui::xaml::Controls::FontIcon>,
}

impl View {
    pub fn new(id: NodeId, kind: NodeKind) -> Result<View> {
        let content = controls::create(id, kind)?;
        // A container stands for its node in UI Automation: its custom
        // peer gives it the node's role (`peers.rs`).
        let outer = if content.element().is_none() {
            crate::peers::canvas(crate::peers::Key::Node(id))?
        } else {
            Canvas::new()?
        };
        let kids = Canvas::new()?;
        let mut scroll = None;
        // An overlay's content is drawn above the window (`Win::layer`);
        // other nodes' children canvas joins the tree with the first child.
        let kids_in = matches!(kind, NodeKind::Scroll | NodeKind::Overlay);
        if kind == NodeKind::Scroll {
            let sv = ScrollViewer::new()?;
            sv.SetContent(&kids)?;
            sv.SetHorizontalScrollBarVisibility(ScrollBarVisibility::Disabled)?;
            sv.SetVerticalScrollBarVisibility(ScrollBarVisibility::Auto)?;
            sv.SetIsTabStop(false)?;
            crate::events::scrolled(&sv, id)?;
            outer.Children()?.Append(&sv)?;
            scroll = Some(sv);
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
            id,
            kind,
            outer,
            shadows: None,
            boxed: None,
            kids,
            kids_in,
            scroll,
            content,
            bar,
            dash: None,
            decor: Default::default(),
            children: Vec::new(),
            hit: false,
            button: None,
            frame: Rect::default(),
            field_icon: None,
        })
    }

    pub fn outer(&self) -> UIElement {
        ui(&self.outer).expect("UIElement")
    }

    /// Where `kids` goes in `outer`: under the content, the bar and the
    /// dash.
    fn kids_index(&self) -> Result<u32> {
        let coll = self.outer.Children()?;
        let above: Vec<UIElement> = [
            self.content.element(),
            match &self.bar {
                Bar::Header(h) => ui(&h.root).ok(),
                Bar::Tabs(t) => ui(&t.root).ok(),
                Bar::None => None,
            },
            self.dash.as_ref().and_then(|d| ui(d).ok()),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut first = coll.Size()?;
        for e in above {
            let mut at = 0u32;
            if coll.IndexOf(&e, &mut at)? {
                first = first.min(at);
            }
        }
        Ok(first)
    }

    /// Puts `kids` in the tree (the node has children now).
    fn attach_kids(&mut self) -> Result<()> {
        if self.kids_in {
            return Ok(());
        }
        let at = self.kids_index()?;
        self.outer.Children()?.InsertAt(at, &self.kids)?;
        self.kids_in = true;
        Ok(())
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
        // Underlines XAML cannot draw (a color, a thick or dotted line).
        if let Content::Text(t) = &self.content {
            let st = StyledText::new(&n.text, &n.config, s);
            crate::underlay::sync(self.id, &self.outer, t, &st, pal, text)?;
        }
        crate::fieldicon::sync(&mut self.field_icon, &self.outer, n, pal)?;
        style::apply_transform(&self.outer, s, r, sticky)?;
        self.clip(n)?;
        Ok(())
    }

    /// Children are clipped to the box (`overflow: hidden`, stacks, tabs),
    /// to its rounded corners if it has some; a picture to its rounded
    /// corners.
    fn clip(&mut self, n: &Node) -> Result<()> {
        let clips = n.clips() && n.kind != NodeKind::Scroll;
        self.decor.clip_children(&self.kids, n, clips)?;
        if let Content::Picture(img) = &self.content {
            let (x, y, _, _) = picture_rect(img, n).0;
            self.decor.clip_picture(&img.cast()?, n, (x, y))?;
        }
        Ok(())
    }

    /// A picture's image placed for its fit and focal point, and clipped
    /// (after its bitmap, its mode or its focal point changed).
    pub fn fit_picture(&mut self, n: &Node) -> Result<()> {
        let Content::Picture(img) = &self.content else {
            return Ok(());
        };
        let ((x, y, w, h), clip) = picture_rect(img, n);
        Canvas::SetLeft(img, x as f64)?;
        Canvas::SetTop(img, y as f64)?;
        img.SetWidth(w.max(0.) as f64)?;
        img.SetHeight(h.max(0.) as f64)?;
        match clip {
            Some(r) => {
                let g = RectangleGeometry::new()?;
                g.SetRect(r)?;
                img.SetClip(&g)?;
            }
            None if img.Clip().is_ok() => img.SetClip(None::<&RectangleGeometry>)?,
            None => {}
        }
        self.clip(n)
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
        if matches!(self.content, Content::Picture(_)) {
            self.fit_picture(n)?;
        } else if let Some(e) = self.content.element() {
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
        // A native button is clicked through itself: another button around
        // it would be an empty Tab stop, nameless for screen readers.
        if self.button.is_some()
            || self.scroll.is_some()
            || self.kind == NodeKind::Overlay
            || matches!(self.content, Content::Button(..))
        {
            return Ok(None);
        }
        let b = controls::clickable_host(id)?;
        let coll = self.outer.Children()?;
        let kids: UIElement = self.kids.cast()?;
        let mut at = 0u32;
        if coll.IndexOf(&kids, &mut at)? {
            coll.RemoveAt(at)?;
        } else {
            at = self.kids_index()?;
        }
        b.SetContent(&self.kids)?;
        let e: UIElement = b.cast()?;
        coll.InsertAt(at, &e)?;
        self.kids_in = true;
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
        if !children.is_empty() {
            self.attach_kids()?;
        }
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

/// Where a picture's image goes in its node, and the clip that keeps it
/// to its box. A filled picture with a focal point (`object_position`,
/// #218) is as large as covering the box and placed so the box shows the
/// part around that point (`cover_crop`, CSS `object-position`); WinUI
/// would crop around the center. Otherwise it fills the content box.
fn picture_rect(
    img: &Image,
    n: &Node,
) -> ((f32, f32, f32, f32), Option<windows::Foundation::Rect>) {
    let (x, y, w, h) = content_rect(n);
    let at = prism_core::picture_position(&n.config);
    let size = img
        .Source()
        .ok()
        .and_then(|s| s.cast::<BitmapSource>().ok())
        .and_then(|b| Some((b.PixelWidth().ok()? as f32, b.PixelHeight().ok()? as f32)));
    let image = size.filter(|&(iw, ih)| {
        n.value != 1. && at != (0.5, 0.5) && iw > 0. && ih > 0. && w > 0. && h > 0.
    });
    let Some(image) = image else {
        return ((x, y, w, h), None);
    };
    let (cx, cy, cw, ch) = prism_core::cover_crop(image, (w, h), at);
    let (iw, ih) = (w / cw, h / ch);
    let (ox, oy) = (cx * iw, cy * ih);
    let clip = windows::Foundation::Rect {
        X: ox,
        Y: oy,
        Width: w,
        Height: h,
    };
    ((x - ox, y - oy, iw, ih), Some(clip))
}

/// The scroll view's axis and its bar: WinUI's (it shows over the content
/// while scrolling or under the pointer), or none (`Scrollbar::Hidden`,
/// #132), the view still scrolling with the wheel, touch and the keyboard.
pub fn scroll_axis(sv: &ScrollViewer, vertical: bool, bar: prism_core::Scrollbar) -> Result<()> {
    let shown = if bar == prism_core::Scrollbar::Hidden {
        ScrollBarVisibility::Hidden
    } else {
        ScrollBarVisibility::Auto
    };
    if vertical {
        sv.SetHorizontalScrollMode(ScrollMode::Disabled)?;
        sv.SetHorizontalScrollBarVisibility(ScrollBarVisibility::Disabled)?;
        sv.SetVerticalScrollMode(ScrollMode::Enabled)?;
        sv.SetVerticalScrollBarVisibility(shown)?;
    } else {
        sv.SetVerticalScrollMode(ScrollMode::Disabled)?;
        sv.SetVerticalScrollBarVisibility(ScrollBarVisibility::Disabled)?;
        sv.SetHorizontalScrollMode(ScrollMode::Enabled)?;
        sv.SetHorizontalScrollBarVisibility(shown)?;
    }
    Ok(())
}
