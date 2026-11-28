//! Draws what an SVG document is made of (#342) with Core Graphics and Core
//! Text: paths painted flat or with a gradient, lines of text, bitmaps, and
//! groups with an opacity, a clip, a mask, shadows or a blur.
//!
//! The context's units are points, y down, like the items'. A group with a
//! mask, shadows or a blur is drawn into bitmaps of the view's size, at the
//! context's pixels per point, and its pixels are worked on here: Core
//! Graphics does not blur what is drawn, and its shadows take their offset
//! and radius in pixels of the base space, whichever way the context is
//! flipped.
//!
//! The macOS and iOS backends each have this file, with the same content.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::null;

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSAttributedString, NSData, NSDictionary, NSNumber, NSString};
use prism_core::Color;
use prism_core::shape::{
    Anchor, Baseline, Fit, Gradient, GradientShape, Group, Image, Item, Matrix, Path, Segment,
    Spread, Text, TextRun,
};
use prism_native::picture_cache::{PictureCache, decoded_bytes};

/// A Core Foundation or Core Graphics object.
type Ref = *mut c_void;

/// `CGPoint`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

/// `CGRect`: its origin, then its size.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// `CGAffineTransform`: `x' = a·x + c·y + tx`, `y' = b·x + d·y + ty`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    tx: f64,
    ty: f64,
}

impl Affine {
    /// A [`Matrix`] has the same order.
    fn new(m: Matrix) -> Affine {
        Affine {
            a: m[0] as f64,
            b: m[1] as f64,
            c: m[2] as f64,
            d: m[3] as f64,
            tx: m[4] as f64,
            ty: m[5] as f64,
        }
    }

    /// What it multiplies areas by.
    fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// Whether it can be drawn through: it does not squash everything onto
    /// a line, and has no number that is not one.
    fn usable(&self) -> bool {
        let all = [self.a, self.b, self.c, self.d, self.tx, self.ty];
        all.iter().all(|v| v.is_finite()) && self.det().abs() > 1e-12
    }
}

/// `kCGImageAlphaPremultipliedLast`: bytes are red, green, blue, alpha.
const PREMULTIPLIED_LAST: u32 = 1;
/// `kCGGradientDrawsBeforeStartLocation | kCGGradientDrawsAfterEndLocation`.
const EXTEND: u32 = 3;
/// `kCGInterpolationMedium`: a bitmap drawn smaller is averaged.
const SMOOTH: i32 = 4;
/// `kCGLineCapRound` and `kCGLineJoinRound`.
const ROUND: i32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: Ref;
    fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
    fn CGColorSpaceRelease(space: Ref);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits: usize,
        stride: usize,
        space: Ref,
        info: u32,
    ) -> Ref;
    fn CGBitmapContextGetData(c: Ref) -> *mut c_void;
    fn CGBitmapContextGetBytesPerRow(c: Ref) -> usize;
    fn CGBitmapContextCreateImage(c: Ref) -> Ref;
    fn CGContextRelease(c: Ref);
    fn CGImageRetain(image: Ref) -> Ref;
    fn CGImageRelease(image: Ref);
    fn CGImageGetWidth(image: Ref) -> usize;
    fn CGImageGetHeight(image: Ref) -> usize;
    fn CGContextSaveGState(c: Ref);
    fn CGContextRestoreGState(c: Ref);
    fn CGContextGetCTM(c: Ref) -> Affine;
    fn CGContextConcatCTM(c: Ref, m: Affine);
    fn CGContextTranslateCTM(c: Ref, x: f64, y: f64);
    fn CGContextScaleCTM(c: Ref, x: f64, y: f64);
    fn CGContextBeginPath(c: Ref);
    fn CGContextMoveToPoint(c: Ref, x: f64, y: f64);
    fn CGContextAddLineToPoint(c: Ref, x: f64, y: f64);
    fn CGContextAddCurveToPoint(c: Ref, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64);
    fn CGContextClosePath(c: Ref);
    fn CGContextFillPath(c: Ref);
    fn CGContextEOFillPath(c: Ref);
    fn CGContextStrokePath(c: Ref);
    fn CGContextClip(c: Ref);
    fn CGContextEOClip(c: Ref);
    fn CGContextClipToRect(c: Ref, rect: Rect);
    fn CGContextGetClipBoundingBox(c: Ref) -> Rect;
    fn CGContextReplacePathWithStrokedPath(c: Ref);
    fn CGContextFillRect(c: Ref, rect: Rect);
    fn CGContextClearRect(c: Ref, rect: Rect);
    fn CGContextSetFillColorSpace(c: Ref, space: Ref);
    fn CGContextSetFillColor(c: Ref, components: *const f64);
    fn CGContextSetStrokeColorSpace(c: Ref, space: Ref);
    fn CGContextSetStrokeColor(c: Ref, components: *const f64);
    fn CGContextSetLineWidth(c: Ref, width: f64);
    fn CGContextSetLineCap(c: Ref, cap: i32);
    fn CGContextSetLineJoin(c: Ref, join: i32);
    fn CGContextSetLineDash(c: Ref, phase: f64, lengths: *const f64, count: usize);
    fn CGContextSetAlpha(c: Ref, alpha: f64);
    fn CGContextBeginTransparencyLayer(c: Ref, info: *const c_void);
    fn CGContextEndTransparencyLayer(c: Ref);
    fn CGContextSetInterpolationQuality(c: Ref, quality: i32);
    fn CGContextDrawImage(c: Ref, rect: Rect, image: Ref);
    fn CGContextSetTextMatrix(c: Ref, m: Affine);
    fn CGContextSetTextPosition(c: Ref, x: f64, y: f64);
    fn CGGradientCreateWithColorComponents(
        space: Ref,
        components: *const f64,
        locations: *const f64,
        count: usize,
    ) -> Ref;
    fn CGGradientRelease(gradient: Ref);
    fn CGContextDrawLinearGradient(c: Ref, gradient: Ref, from: Point, to: Point, options: u32);
    fn CGContextDrawRadialGradient(
        c: Ref,
        gradient: Ref,
        from: Point,
        from_radius: f64,
        to: Point,
        to_radius: f64,
        options: u32,
    );
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(data: *const c_void, options: *const c_void) -> Ref;
    fn CGImageSourceCreateImageAtIndex(source: Ref, index: usize, options: *const c_void) -> Ref;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    static kCTFontAttributeName: &'static NSString;
    static kCTKernAttributeName: &'static NSString;
    static kCTLigatureAttributeName: &'static NSString;
    static kCTForegroundColorFromContextAttributeName: &'static NSString;
    fn CTLineCreateWithAttributedString(text: *const c_void) -> Ref;
    fn CTLineGetTypographicBounds(
        line: Ref,
        ascent: *mut f64,
        descent: *mut f64,
        leading: *mut f64,
    ) -> f64;
    fn CTLineDraw(line: Ref, c: Ref);
    fn CTFontGetAscent(font: *const c_void) -> f64;
    fn CTFontGetDescent(font: *const c_void) -> f64;
    fn CTFontGetXHeight(font: *const c_void) -> f64;
}

/// A `CGImage`, released when dropped.
struct Bitmap(Ref);

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe { CGImageRelease(self.0) };
    }
}

impl Clone for Bitmap {
    fn clone(&self) -> Bitmap {
        Bitmap(unsafe { CGImageRetain(self.0) })
    }
}

/// The most pixels one bitmap of a drawing has (64 MB): a larger view is
/// drawn with fewer pixels per point.
const MAX_PIXELS: f64 = 16_000_000.;

/// How many pixels a point of a `width` × `height` view gets in a bitmap,
/// for a context that has `scale` of them.
fn bitmap_scale(width: f64, height: f64, scale: f64) -> f64 {
    let scale = if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    };
    let pixels = width * height * scale * scale;
    if pixels > MAX_PIXELS {
        scale * (MAX_PIXELS / pixels).sqrt()
    } else {
        scale
    }
}

/// A bitmap to draw in: red, green, blue and alpha bytes, the colors
/// multiplied by the alpha, rows from the top. Its units are points, y
/// down.
struct Layer {
    ctx: Ref,
    w: usize,
    h: usize,
    stride: usize,
}

impl Layer {
    /// An empty bitmap of a `width` × `height` points view, `scale` pixels
    /// a point.
    fn new(space: Ref, width: f64, height: f64, scale: f64) -> Option<Layer> {
        let (w, h) = ((width * scale).ceil(), (height * scale).ceil());
        if !(w >= 1. && h >= 1. && w * h <= MAX_PIXELS * 2.) {
            return None;
        }
        let (w, h) = (w as usize, h as usize);
        unsafe {
            let ctx = CGBitmapContextCreate(
                std::ptr::null_mut(),
                w,
                h,
                8,
                w * 4,
                space,
                PREMULTIPLIED_LAST,
            );
            if ctx.is_null() {
                return None;
            }
            let all = Rect {
                x: 0.,
                y: 0.,
                w: w as f64,
                h: h as f64,
            };
            CGContextClearRect(ctx, all);
            // Core Graphics counts rows from the bottom: turned over, the
            // first row in memory is the top one.
            CGContextTranslateCTM(ctx, 0., h as f64);
            CGContextScaleCTM(ctx, scale, -scale);
            let stride = CGBitmapContextGetBytesPerRow(ctx);
            if CGBitmapContextGetData(ctx).is_null() || stride < w * 4 {
                CGContextRelease(ctx);
                return None;
            }
            Some(Layer { ctx, w, h, stride })
        }
    }

    /// Its bytes: `h` rows of `stride` bytes.
    fn pixels(&mut self) -> &mut [u8] {
        // The context owns them for as long as it lives, and nothing else
        // reads or writes them while this borrow of the layer lasts.
        unsafe {
            std::slice::from_raw_parts_mut(
                CGBitmapContextGetData(self.ctx) as *mut u8,
                self.stride * self.h,
            )
        }
    }

    /// What it shows now, as an image.
    fn image(&self) -> Option<Bitmap> {
        let image = unsafe { CGBitmapContextCreateImage(self.ctx) };
        (!image.is_null()).then_some(Bitmap(image))
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        unsafe { CGContextRelease(self.ctx) };
    }
}

/// The sRGB color space, released when dropped.
struct Space(Ref);

impl Space {
    fn new() -> Option<Space> {
        let space = unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) };
        (!space.is_null()).then_some(Space(space))
    }
}

impl Drop for Space {
    fn drop(&mut self) {
        unsafe { CGColorSpaceRelease(self.0) };
    }
}

/// Draws `items` in `ctx`, a context whose units are the points of a
/// `width` × `height` view, y down. `dark` picks the variant of colors that
/// adapt to the appearance.
#[cfg(not(target_os = "macos"))]
pub(crate) fn draw(ctx: *mut c_void, items: &[Item], width: f64, height: f64, dark: bool) {
    if ctx.is_null() || items.is_empty() || !(width > 0. && height > 0.) {
        return;
    }
    let Some(space) = Space::new() else { return };
    unsafe {
        // The bitmaps of groups get as many pixels as the context has.
        let scale = CGContextGetCTM(ctx).det().abs().sqrt();
        CGContextSaveGState(ctx);
        Canvas {
            ctx,
            width,
            height,
            scale: bitmap_scale(width, height, scale),
            dark,
            space: space.0,
        }
        .items(items);
        CGContextRestoreGState(ctx);
    }
}

/// A picture of a drawing: a `CGImage`.
#[cfg(target_os = "macos")]
pub(crate) struct Picture(Bitmap);

#[cfg(target_os = "macos")]
impl Picture {
    /// The `CGImage`, as a layer's `contents` takes it.
    pub(crate) fn as_object(&self) -> &AnyObject {
        unsafe { &*(self.0.0 as *const AnyObject) }
    }
}

/// `items` drawn in a bitmap of a `width` × `height` points view, `scale`
/// pixels a point.
#[cfg(target_os = "macos")]
pub(crate) fn picture(
    items: &[Item],
    width: f64,
    height: f64,
    scale: f64,
    dark: bool,
) -> Option<Picture> {
    if items.is_empty() || !(width > 0. && height > 0.) {
        return None;
    }
    let space = Space::new()?;
    let scale = bitmap_scale(width, height, scale);
    let layer = Layer::new(space.0, width, height, scale)?;
    Canvas {
        ctx: layer.ctx,
        width,
        height,
        scale,
        dark,
        space: space.0,
    }
    .items(items);
    layer.image().map(Picture)
}

/// Where items are drawn: a context in points, y down.
#[derive(Clone, Copy)]
struct Canvas {
    ctx: Ref,
    /// The view's size, points.
    width: f64,
    height: f64,
    /// Pixels per point of the bitmaps groups are drawn in.
    scale: f64,
    dark: bool,
    space: Ref,
}

impl Canvas {
    fn items(&self, items: &[Item]) {
        for item in items {
            match item {
                Item::Path(p) => self.path(p),
                Item::Text(t) => self.text(t),
                Item::Image(i) => self.image(i),
                Item::Group(g) => self.group(g),
            }
        }
    }

    /// Red, green, blue and alpha of `c` in this appearance.
    fn rgba(&self, c: Color) -> [f64; 4] {
        let c = if self.dark {
            c.dark_variant()
        } else {
            c.light_variant()
        };
        [c.r, c.g, c.b, c.a].map(|v| (v as f64).clamp(0., 1.))
    }

    /// Makes `segments` the context's path. `false`: there is none.
    fn outline(&self, segments: &[Segment]) -> bool {
        unsafe {
            CGContextBeginPath(self.ctx);
            for s in segments {
                match *s {
                    Segment::Move(x, y) => CGContextMoveToPoint(self.ctx, x as f64, y as f64),
                    Segment::Line(x, y) => CGContextAddLineToPoint(self.ctx, x as f64, y as f64),
                    Segment::Cubic(x1, y1, x2, y2, x, y) => CGContextAddCurveToPoint(
                        self.ctx, x1 as f64, y1 as f64, x2 as f64, y2 as f64, x as f64, y as f64,
                    ),
                    Segment::Close => CGContextClosePath(self.ctx),
                    // Arcs come as curves once in points.
                    Segment::Arc { .. } => {}
                }
            }
        }
        !segments.is_empty()
    }

    /// The stroke's width, ends, corners and dashes.
    fn pen(&self, p: &Path) {
        unsafe {
            CGContextSetLineWidth(self.ctx, p.width as f64);
            if p.round {
                CGContextSetLineCap(self.ctx, ROUND);
                CGContextSetLineJoin(self.ctx, ROUND);
            }
            if let Some((dash, gap)) = p.dash
                && dash >= 0.
                && gap >= 0.
                && dash + gap > 0.
            {
                let lengths = [dash as f64, gap as f64];
                CGContextSetLineDash(self.ctx, 0., lengths.as_ptr(), 2);
            }
        }
    }

    fn path(&self, p: &Path) {
        if p.segments.is_empty() {
            return;
        }
        let ctx = self.ctx;
        unsafe {
            if let Some(g) = &p.fill_gradient {
                CGContextSaveGState(ctx);
                self.outline(&p.segments);
                if p.even_odd {
                    CGContextEOClip(ctx);
                } else {
                    CGContextClip(ctx);
                }
                self.gradient(g);
                CGContextRestoreGState(ctx);
            } else if let Some(c) = p.fill.map(|c| self.rgba(c))
                && c[3] > 0.
            {
                self.outline(&p.segments);
                CGContextSetFillColorSpace(ctx, self.space);
                CGContextSetFillColor(ctx, c.as_ptr());
                if p.even_odd {
                    CGContextEOFillPath(ctx);
                } else {
                    CGContextFillPath(ctx);
                }
            }
            if p.width.is_nan() || p.width <= 0. {
                return;
            }
            if let Some(g) = &p.stroke_gradient {
                // The gradient shows through the stroke's outline: it runs
                // where a fill's would, not along the line.
                CGContextSaveGState(ctx);
                self.pen(p);
                self.outline(&p.segments);
                CGContextReplacePathWithStrokedPath(ctx);
                CGContextClip(ctx);
                self.gradient(g);
                CGContextRestoreGState(ctx);
            } else if let Some(c) = p.stroke.map(|c| self.rgba(c))
                && c[3] > 0.
            {
                CGContextSaveGState(ctx);
                self.pen(p);
                self.outline(&p.segments);
                CGContextSetStrokeColorSpace(ctx, self.space);
                CGContextSetStrokeColor(ctx, c.as_ptr());
                CGContextStrokePath(ctx);
                CGContextRestoreGState(ctx);
            }
        }
    }

    /// Paints `g` over all that the context's clip lets through. The
    /// caller saves and restores the state.
    fn gradient(&self, g: &Gradient) {
        let stops = clean_stops(g.stops.iter().map(|s| (s.offset, self.rgba(s.color))));
        let Some(&(_, last)) = stops.last() else {
            return;
        };
        let ctx = self.ctx;
        let flat = |color: [f64; 4]| unsafe {
            CGContextSetFillColorSpace(ctx, self.space);
            CGContextSetFillColor(ctx, color.as_ptr());
            CGContextFillRect(ctx, CGContextGetClipBoundingBox(ctx));
        };
        // One color, or a gradient with no length: its last color.
        let empty = match g.shape {
            GradientShape::Linear { x1, y1, x2, y2 } => x1 == x2 && y1 == y2,
            GradientShape::Radial { r, .. } => r.is_nan() || r <= 0.,
        };
        if stops.len() == 1 || empty {
            return flat(last);
        }
        let m = Affine::new(g.transform);
        if !m.usable() {
            return;
        }
        unsafe {
            // The gradient's matrix, not only its end points: a skewed
            // gradient stays skewed.
            CGContextConcatCTM(ctx, m);
            // What shows, in the gradient's coordinates: how far a
            // gradient that repeats has to.
            let b = CGContextGetClipBoundingBox(ctx);
            let corners = [
                (b.x, b.y),
                (b.x + b.w, b.y),
                (b.x, b.y + b.h),
                (b.x + b.w, b.y + b.h),
            ];
            match g.shape {
                GradientShape::Linear { x1, y1, x2, y2 } => {
                    let (from, to) = ((x1 as f64, y1 as f64), (x2 as f64, y2 as f64));
                    let (k0, k1) = match g.spread {
                        Spread::Pad => (0, 1),
                        _ => linear_cycles(from, to, &corners),
                    };
                    let Some(gradient) = self.cg_gradient(&stops, g.spread, k0, k1) else {
                        return;
                    };
                    let at = |k: i32| Point {
                        x: from.0 + (to.0 - from.0) * k as f64,
                        y: from.1 + (to.1 - from.1) * k as f64,
                    };
                    CGContextDrawLinearGradient(ctx, gradient, at(k0), at(k1), EXTEND);
                    CGGradientRelease(gradient);
                }
                GradientShape::Radial { cx, cy, r, fx, fy } => {
                    let (center, focus) = ((cx as f64, cy as f64), (fx as f64, fy as f64));
                    let k = match g.spread {
                        Spread::Pad => 1,
                        _ => radial_cycles(center, r as f64, focus, &corners),
                    };
                    let Some(gradient) = self.cg_gradient(&stops, g.spread, 0, k) else {
                        return;
                    };
                    // From the focal point to the circle, and on to the
                    // circles `k` times as far.
                    let k = k as f64;
                    CGContextDrawRadialGradient(
                        ctx,
                        gradient,
                        Point {
                            x: focus.0,
                            y: focus.1,
                        },
                        0.,
                        Point {
                            x: focus.0 + (center.0 - focus.0) * k,
                            y: focus.1 + (center.1 - focus.1) * k,
                        },
                        r as f64 * k,
                        EXTEND,
                    );
                    CGGradientRelease(gradient);
                }
            }
        }
    }

    /// A `CGGradient` of `stops` over the cycles `k0..k1`.
    fn cg_gradient(&self, stops: &[Stop], spread: Spread, k0: i32, k1: i32) -> Option<Ref> {
        let (locations, colors) = spread_stops(stops, spread, k0, k1);
        let gradient = unsafe {
            CGGradientCreateWithColorComponents(
                self.space,
                colors.as_ptr(),
                locations.as_ptr(),
                locations.len(),
            )
        };
        (!gradient.is_null()).then_some(gradient)
    }

    fn text(&self, t: &Text) {
        let m = Affine::new(t.transform);
        if t.runs.is_empty() || !m.usable() {
            return;
        }
        // Each run is a line of its own, placed after the one before.
        let lines: Vec<Option<(Ref, f64)>> = t.runs.iter().map(line).collect();
        let width: f64 = t
            .runs
            .iter()
            .zip(&lines)
            .map(|(r, l)| r.dx as f64 + l.map_or(0., |(_, w)| w))
            .sum();
        let shift = match t.runs.first().filter(|r| r.size > 0.) {
            Some(first) => {
                let font = font(first);
                let font = Retained::as_ptr(&font) as *const c_void;
                let (ascent, descent, x_height) = unsafe {
                    (
                        CTFontGetAscent(font),
                        CTFontGetDescent(font),
                        CTFontGetXHeight(font),
                    )
                };
                baseline_shift(t.baseline, ascent, descent, x_height)
            }
            None => 0.,
        };
        let ctx = self.ctx;
        unsafe {
            CGContextSaveGState(ctx);
            CGContextConcatCTM(ctx, m);
            // Glyphs are drawn y up: turned over, they stand in a context
            // whose y goes down.
            CGContextSetTextMatrix(
                ctx,
                Affine {
                    a: 1.,
                    b: 0.,
                    c: 0.,
                    d: -1.,
                    tx: 0.,
                    ty: 0.,
                },
            );
            let mut x = line_start(t.anchor, t.x as f64, width);
            for (run, line) in t.runs.iter().zip(&lines) {
                x += run.dx as f64;
                let Some((line, w)) = *line else { continue };
                let color = self.rgba(run.fill);
                // A transparent run takes its room and draws nothing.
                if color[3] > 0. {
                    CGContextSetFillColorSpace(ctx, self.space);
                    CGContextSetFillColor(ctx, color.as_ptr());
                    CGContextSetTextPosition(ctx, x, t.y as f64 + shift + run.dy as f64);
                    CTLineDraw(line, ctx);
                }
                x += w;
                CFRelease(line);
            }
            CGContextRestoreGState(ctx);
        }
    }

    fn image(&self, i: &Image) {
        let m = Affine::new(i.transform);
        if !(i.w > 0. && i.h > 0. && i.opacity > 0.) || !m.usable() {
            return;
        }
        let Some(bitmap) = bitmap(&i.src) else { return };
        let ctx = self.ctx;
        unsafe {
            let size = (
                CGImageGetWidth(bitmap.0) as f64,
                CGImageGetHeight(bitmap.0) as f64,
            );
            let frame = Rect {
                x: i.x as f64,
                y: i.y as f64,
                w: i.w as f64,
                h: i.h as f64,
            };
            let at = fit_rect(i.fit, size, frame, (i.align.0 as f64, i.align.1 as f64));
            CGContextSaveGState(ctx);
            CGContextConcatCTM(ctx, m);
            if i.fit == Fit::Slice {
                CGContextClipToRect(ctx, frame);
            }
            CGContextSetAlpha(ctx, (i.opacity as f64).min(1.));
            CGContextSetInterpolationQuality(ctx, SMOOTH);
            // How many pixels it is drawn in.
            let to = CGContextGetCTM(ctx);
            let wanted = (at.w * to.a.hypot(to.b), at.h * to.c.hypot(to.d));
            let bitmap = self.enlarged(&bitmap, size, wanted).unwrap_or(bitmap);
            self.put(&bitmap, at);
            CGContextRestoreGState(ctx);
        }
    }

    /// `bitmap` (`size` pixels) with the `wanted` pixels it is drawn in, when
    /// they are many more: each one between its four nearest, as a browser
    /// enlarges. Core Graphics enlarges along a curve that keeps the edges
    /// of large pixels sharper.
    fn enlarged(&self, bitmap: &Bitmap, size: (f64, f64), wanted: (f64, f64)) -> Option<Bitmap> {
        let (w, h) = enlarged_size(size, wanted)?;
        let mut from = Layer::new(self.space, size.0, size.1, 1.)?;
        let all = Rect {
            x: 0.,
            y: 0.,
            w: size.0,
            h: size.1,
        };
        Canvas {
            ctx: from.ctx,
            ..*self
        }
        .put(bitmap, all);
        let mut to = Layer::new(self.space, w as f64, h as f64, 1.)?;
        let (from_size, to_size) = ((from.w, from.h, from.stride), (to.w, to.h, to.stride));
        enlarge(from.pixels(), from_size, to.pixels(), to_size);
        to.image()
    }

    /// Draws `image` in `at`, its top at the top.
    fn put(&self, image: &Bitmap, at: Rect) {
        unsafe {
            CGContextSaveGState(self.ctx);
            // Images are drawn y up.
            CGContextTranslateCTM(self.ctx, at.x, at.y + at.h);
            CGContextScaleCTM(self.ctx, 1., -1.);
            let rect = Rect {
                x: 0.,
                y: 0.,
                w: at.w,
                h: at.h,
            };
            CGContextDrawImage(self.ctx, rect, image.0);
            CGContextRestoreGState(self.ctx);
        }
    }

    /// A bitmap of the view, and the canvas that draws in it.
    fn layer(&self) -> Option<(Layer, Canvas)> {
        let layer = Layer::new(self.space, self.width, self.height, self.scale)?;
        let canvas = Canvas {
            ctx: layer.ctx,
            ..*self
        };
        Some((layer, canvas))
    }

    /// Draws what `layer` shows, moved by `(dx, dy)` points.
    fn put_layer(&self, layer: &Layer, dx: f64, dy: f64) {
        if let Some(image) = layer.image() {
            let at = Rect {
                x: dx,
                y: dy,
                w: layer.w as f64 / self.scale,
                h: layer.h as f64 / self.scale,
            };
            self.put(&image, at);
        }
    }

    fn group(&self, g: &Group) {
        // An outline with nothing in it lets nothing through.
        if g.opacity.is_nan()
            || g.opacity <= 0.
            || g.clip.as_ref().is_some_and(|c| c.segments.is_empty())
        {
            return;
        }
        let ctx = self.ctx;
        let opacity = (g.opacity as f64).min(1.);
        unsafe {
            CGContextSaveGState(ctx);
            if let Some(c) = &g.clip {
                self.outline(&c.segments);
                if c.even_odd {
                    CGContextEOClip(ctx);
                } else {
                    CGContextClip(ctx);
                }
            }
            if g.blur > 0. || !g.shadows.is_empty() || g.mask.is_some() {
                if let Some(picture) = self.effects(g) {
                    CGContextSetAlpha(ctx, opacity);
                    self.put_layer(&picture, 0., 0.);
                }
            } else if opacity < 1. {
                // Drawn together, then shown at the opacity: what overlaps
                // inside does not show through.
                CGContextSetAlpha(ctx, opacity);
                CGContextBeginTransparencyLayer(ctx, null());
                self.items(&g.items);
                CGContextEndTransparencyLayer(ctx);
            } else {
                self.items(&g.items);
            }
            CGContextRestoreGState(ctx);
        }
    }

    /// The picture of a group that is blurred, over shadows or seen through
    /// a mask: its items, blurred, over its shadows, times its mask.
    fn effects(&self, g: &Group) -> Option<Layer> {
        let (mut content, canvas) = self.layer()?;
        canvas.items(&g.items);
        let (w, h, stride) = (content.w, content.h, content.stride);
        if g.blur > 0. {
            blur(
                content.pixels(),
                w,
                h,
                stride,
                4,
                g.blur as f64 * self.scale,
            );
        }
        let mut picture = if g.shadows.is_empty() {
            content
        } else {
            let (under, canvas) = self.layer()?;
            let (mut tinted, _) = self.layer()?;
            for s in &g.shadows {
                let color = self.rgba(s.color);
                if color[3] <= 0. {
                    continue;
                }
                // The content's alpha, blurred, in the shadow's color.
                let mut alpha: Vec<u8> = content
                    .pixels()
                    .chunks_exact(stride)
                    .flat_map(|row| row[..w * 4].as_chunks::<4>().0.iter().map(|p| p[3]))
                    .collect();
                blur(&mut alpha, w, h, w, 1, (s.blur as f64).max(0.) * self.scale);
                tint(tinted.pixels(), stride, &alpha, w, color);
                canvas.put_layer(&tinted, s.dx as f64, s.dy as f64);
            }
            canvas.put_layer(&content, 0., 0.);
            under
        };
        if let Some(mask) = &g.mask {
            let (mut seen, canvas) = self.layer()?;
            canvas.items(&mask.items);
            mask_pixels(picture.pixels(), seen.pixels(), mask.luminance);
        }
        Some(picture)
    }
}

/// One color of a gradient: its offset, then red, green, blue, alpha.
type Stop = (f64, [f64; 4]);

/// Stops as they are painted: offsets within `0..=1`, none before the one
/// it follows.
fn clean_stops(stops: impl Iterator<Item = (f32, [f64; 4])>) -> Vec<Stop> {
    let mut out: Vec<Stop> = Vec::new();
    for (offset, color) in stops {
        let offset = if offset.is_finite() {
            (offset as f64).clamp(0., 1.)
        } else {
            0.
        };
        let offset = out.last().map_or(offset, |&(before, _)| offset.max(before));
        out.push((offset, color));
    }
    out
}

/// The most times a gradient that repeats is laid out: past them, its end
/// colors go on.
const MAX_CYCLES: i32 = 512;

/// The cycles `k0..k1` of a gradient along `from` → `to` that cover
/// `corners`: cycle 0 is the gradient itself, 1 the one after its end.
fn linear_cycles(from: (f64, f64), to: (f64, f64), corners: &[(f64, f64)]) -> (i32, i32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len2 = dx * dx + dy * dy;
    let (mut low, mut high) = (0f64, 1f64);
    for &(x, y) in corners {
        let t = ((x - from.0) * dx + (y - from.1) * dy) / len2;
        if t.is_finite() {
            low = low.min(t);
            high = high.max(t);
        }
    }
    let half = (MAX_CYCLES / 2) as f64;
    (
        low.floor().max(-half) as i32,
        high.ceil().min(half).max(1.) as i32,
    )
}

/// How many cycles of a radial gradient (`focus` to the circle of `r`
/// around `center`) cover `corners`.
fn radial_cycles(center: (f64, f64), r: f64, focus: (f64, f64), corners: &[(f64, f64)]) -> i32 {
    let (ex, ey) = (center.0 - focus.0, center.1 - focus.1);
    // The circles grow away from the focal point only if it is inside the
    // first one.
    let a = r * r - (ex * ex + ey * ey);
    if a.is_nan() || a <= r * r * 1e-6 {
        return 1;
    }
    let mut far = 1f64;
    for &(x, y) in corners {
        // The circle through (x, y): its center is `t` of the way from the
        // focal point to the center, its radius `t · r`.
        let (qx, qy) = (x - focus.0, y - focus.1);
        let along = qx * ex + qy * ey;
        let t = (-along + (along * along + a * (qx * qx + qy * qy)).sqrt()) / a;
        if t.is_finite() {
            far = far.max(t);
        }
    }
    far.ceil().min(MAX_CYCLES as f64) as i32
}

/// The stops of a gradient laid out over the cycles `k0..k1`, as locations
/// within `0..=1` and their colors' components: every other cycle runs
/// backwards when it reflects.
fn spread_stops(stops: &[Stop], spread: Spread, k0: i32, k1: i32) -> (Vec<f64>, Vec<f64>) {
    // Each cycle has its end colors up to its ends, so that one does not
    // run into the next.
    let mut whole = Vec::with_capacity(stops.len() + 2);
    if let Some(&(offset, color)) = stops.first()
        && offset > 0.
    {
        whole.push((0., color));
    }
    whole.extend_from_slice(stops);
    if let Some(&(offset, color)) = stops.last()
        && offset < 1.
    {
        whole.push((1., color));
    }
    let stops = &whole;
    let cycles = (k1 - k0).max(1);
    let mut locations = Vec::with_capacity(stops.len() * cycles as usize);
    let mut colors = Vec::with_capacity(locations.capacity() * 4);
    for k in k0..k0 + cycles {
        let back = spread == Spread::Reflect && k.rem_euclid(2) == 1;
        let mut push = |&(offset, color): &Stop| {
            let offset = if back { 1. - offset } else { offset };
            locations.push(((k - k0) as f64 + offset) / cycles as f64);
            colors.extend_from_slice(&color);
        };
        if back {
            stops.iter().rev().for_each(&mut push);
        } else {
            stops.iter().for_each(&mut push);
        }
    }
    (locations, colors)
}

/// Where a line `width` wide starts, anchored at `x`.
fn line_start(anchor: Anchor, x: f64, width: f64) -> f64 {
    match anchor {
        Anchor::Start => x,
        Anchor::Middle => x - width / 2.,
        Anchor::End => x - width,
    }
}

/// How far below a text's `y` its baseline is.
fn baseline_shift(baseline: Baseline, ascent: f64, descent: f64, x_height: f64) -> f64 {
    match baseline {
        Baseline::Alphabetic => 0.,
        Baseline::Middle => x_height / 2.,
        Baseline::Central => (ascent - descent) / 2.,
        Baseline::Hanging => 0.8 * ascent,
    }
}

/// The most pixels a bitmap is enlarged from, and to, here.
const MAX_ENLARGED: f64 = 4_000_000.;

/// The pixels a bitmap of `size` pixels is enlarged to before it is drawn
/// in `wanted` pixels; `None`: it is drawn as it is (about its size, or
/// smaller).
fn enlarged_size(size: (f64, f64), wanted: (f64, f64)) -> Option<(usize, usize)> {
    let (kx, ky) = (wanted.0 / size.0, wanted.1 / size.1);
    if !(kx.is_finite() && ky.is_finite()) || kx.max(ky) < 1.25 || size.0 * size.1 > MAX_ENLARGED {
        return None;
    }
    // A side that is not enlarged keeps its pixels.
    let (mut w, mut h) = (wanted.0.max(size.0), wanted.1.max(size.1));
    let over = (w * h / MAX_ENLARGED).sqrt();
    if over > 1. {
        (w, h) = (w / over, h / over);
    }
    Some((w.round().max(1.) as usize, h.round().max(1.) as usize))
}

/// Fills `to` with `from` at another size: both are rows of four bytes a
/// pixel, given with their width, height and bytes a row. Each pixel is
/// between the four nearest of `from`, by how near their centers are; past
/// the centers of the outermost ones, it has their color.
fn enlarge(
    from: &[u8],
    (from_w, from_h, from_stride): (usize, usize, usize),
    to: &mut [u8],
    (to_w, to_h, to_stride): (usize, usize, usize),
) {
    // For each pixel along a side: the two it is between, and how far from
    // the first.
    let between = |from: usize, to: usize| -> Vec<(usize, usize, f32)> {
        (0..to)
            .map(|i| {
                let at = (i as f32 + 0.5) * from as f32 / to as f32 - 0.5;
                let at = at.clamp(0., (from - 1) as f32);
                let first = at.floor() as usize;
                (first, (first + 1).min(from - 1), at - first as f32)
            })
            .collect()
    };
    let (columns, rows) = (between(from_w, to_w), between(from_h, to_h));
    for (y, &(y0, y1, down)) in rows.iter().enumerate() {
        let (above, below) = (&from[y0 * from_stride..], &from[y1 * from_stride..]);
        let out = &mut to[y * to_stride..y * to_stride + to_w * 4];
        for (pixel, &(x0, x1, along)) in out.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
            for (c, byte) in pixel.iter_mut().enumerate() {
                let mix = |row: &[u8]| {
                    row[x0 * 4 + c] as f32 * (1. - along) + row[x1 * 4 + c] as f32 * along
                };
                *byte = (mix(above) * (1. - down) + mix(below) * down).round() as u8;
            }
        }
    }
}

/// Where a bitmap of `size` pixels is drawn in `frame` (the caller clips a
/// sliced one to the frame).
fn fit_rect(fit: Fit, size: (f64, f64), frame: Rect, align: (f64, f64)) -> Rect {
    let k = match fit {
        Fit::Stretch => return frame,
        Fit::Meet => (frame.w / size.0).min(frame.h / size.1),
        Fit::Slice => (frame.w / size.0).max(frame.h / size.1),
    };
    if !(k.is_finite() && k > 0.) {
        return frame;
    }
    let (w, h) = (size.0 * k, size.1 * k);
    Rect {
        x: frame.x + (frame.w - w) * align.0,
        y: frame.y + (frame.h - h) * align.1,
        w,
        h,
    }
}

/// The font of a run.
fn spec(run: &TextRun) -> prism_native::text::FontSpec {
    prism_native::text::FontSpec {
        family: run.family.clone(),
        size: run.size,
        weight: prism_core::FontWeight::from_css(run.weight),
        italic: run.italic,
        tabular: false,
    }
}

/// The font of a run: an `NSFont`, which is a `CTFont` too.
#[cfg(target_os = "macos")]
fn font(run: &TextRun) -> Retained<AnyObject> {
    Retained::into_super(Retained::into_super(crate::label::ns_font(&spec(run))))
}

/// The font of a run: a `UIFont`, which is a `CTFont` too.
#[cfg(not(target_os = "macos"))]
fn font(run: &TextRun) -> Retained<AnyObject> {
    Retained::into_super(Retained::into_super(crate::label::ui_font(&spec(run))))
}

/// A run as a `CTLine` (the caller releases it) and its width; `None`: it
/// has nothing to draw.
fn line(run: &TextRun) -> Option<(Ref, f64)> {
    if run.text.is_empty() || run.size.is_nan() || run.size <= 0. {
        return None;
    }
    let font = font(run);
    let spacing = NSNumber::new_f64(run.spacing as f64);
    let (yes, no) = (NSNumber::new_bool(true), NSNumber::new_i32(0));
    let (font, spacing, yes, no): (&AnyObject, &AnyObject, &AnyObject, &AnyObject) =
        (&font, &spacing, &yes, &no);
    // The color is the context's fill color.
    let (mut keys, mut values) = unsafe {
        (
            vec![
                kCTFontAttributeName,
                kCTForegroundColorFromContextAttributeName,
            ],
            vec![font, yes],
        )
    };
    if run.spacing != 0. {
        // Spaced out letters do not join.
        keys.extend(unsafe { [kCTKernAttributeName, kCTLigatureAttributeName] });
        values.extend([spacing, no]);
    }
    let attributes = NSDictionary::from_slices(&keys, &values);
    let text = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(&run.text),
            Some(&attributes),
        )
    };
    unsafe {
        let line = CTLineCreateWithAttributedString(Retained::as_ptr(&text) as *const c_void);
        if line.is_null() {
            return None;
        }
        let width = CTLineGetTypographicBounds(
            line,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        Some((line, width))
    }
}

thread_local! {
    /// Decoded bitmaps by their `data:` URL (`None`: it does not decode),
    /// [`UNSHOWN_BYTES`](prism_native::picture_cache::UNSHOWN_BYTES) of them
    /// at most, the least recently drawn going first (#343).
    static BITMAPS: RefCell<PictureCache<Option<Bitmap>>> = RefCell::default();
}

/// The bitmap of a `data:` URL, decoded once.
fn bitmap(src: &str) -> Option<Bitmap> {
    BITMAPS.with(|all| cached(&mut all.borrow_mut(), src, decode))
}

/// The bitmap of `src` from `all`, or decoded and kept there.
fn cached(
    all: &mut PictureCache<Option<Bitmap>>,
    src: &str,
    decode: impl FnOnce(&str) -> Option<Bitmap>,
) -> Option<Bitmap> {
    if let Some(bitmap) = all.touch(src) {
        return bitmap.clone();
    }
    let bitmap = decode(src);
    let bytes = bitmap.as_ref().map_or(0, |b| unsafe {
        decoded_bytes(CGImageGetWidth(b.0) as f64, CGImageGetHeight(b.0) as f64)
    });
    all.insert(src, bitmap.clone(), bytes);
    bitmap
}

fn decode(src: &str) -> Option<Bitmap> {
    let data = NSData::from_vec(prism_native::images::data_url(src)?);
    unsafe {
        // An `NSData` is a `CFData`.
        let source = CGImageSourceCreateWithData(Retained::as_ptr(&data) as *const c_void, null());
        if source.is_null() {
            return None;
        }
        let image = CGImageSourceCreateImageAtIndex(source, 0, null());
        CFRelease(source);
        (!image.is_null()).then_some(Bitmap(image))
    }
}

/// How one line of pixels is blurred.
enum Passes {
    /// Weights of the pixel itself, then of its neighbors one by one
    /// further on each side.
    Kernel(Vec<f32>),
    /// Three averages in a row, each over this many pixels before and
    /// after: together, close to a Gaussian blur.
    Boxes([(usize, usize); 3]),
}

impl Passes {
    /// A Gaussian blur of `sigma` pixels.
    fn new(sigma: f64) -> Passes {
        if sigma < 2. {
            let radius = (sigma * 3.).ceil().max(1.) as usize;
            let mut weights: Vec<f32> = (0..=radius)
                .map(|i| (-((i * i) as f64) / (2. * sigma * sigma)).exp() as f32)
                .collect();
            let sum: f32 = weights[0] + 2. * weights[1..].iter().sum::<f32>();
            weights.iter_mut().for_each(|w| *w /= sum);
            return Passes::Kernel(weights);
        }
        // The size of the averages a browser blurs with (SVG's
        // `feGaussianBlur`).
        let d = (sigma * 3. * (2. * std::f64::consts::PI).sqrt() / 4. + 0.5).floor() as usize;
        Passes::Boxes(if d % 2 == 1 {
            [(d / 2, d / 2); 3]
        } else {
            [(d / 2, d / 2 - 1), (d / 2 - 1, d / 2), (d / 2, d / 2)]
        })
    }

    /// How many pixels away a pixel spreads.
    fn reach(&self) -> usize {
        match self {
            Passes::Kernel(weights) => weights.len() - 1,
            Passes::Boxes(boxes) => boxes.iter().map(|&(a, b)| a.max(b)).sum(),
        }
    }

    /// Blurs `line`; past its ends there is nothing. `work` is room for
    /// `line.len() + 1` numbers.
    fn run(&self, line: &mut [f32], work: &mut [f32]) {
        let n = line.len();
        match self {
            Passes::Kernel(weights) => {
                for (i, out) in work[..n].iter_mut().enumerate() {
                    let mut sum = line[i] * weights[0];
                    for (k, weight) in weights.iter().enumerate().skip(1) {
                        if i >= k {
                            sum += line[i - k] * weight;
                        }
                        if i + k < n {
                            sum += line[i + k] * weight;
                        }
                    }
                    *out = sum;
                }
                line.copy_from_slice(&work[..n]);
            }
            Passes::Boxes(boxes) => {
                for &(before, after) in boxes {
                    // Sums up to each pixel: an average is two of them.
                    work[0] = 0.;
                    for i in 0..n {
                        work[i + 1] = work[i] + line[i];
                    }
                    let size = (before + after + 1) as f32;
                    for (i, out) in line.iter_mut().enumerate() {
                        let (from, to) = (i.saturating_sub(before), (i + after + 1).min(n));
                        *out = (work[to] - work[from]) / size;
                    }
                }
            }
        }
    }
}

/// A Gaussian blur of `sigma` pixels, in place. `pixels` holds `h` rows of
/// `stride` bytes, each of `w` pixels of `channels` bytes, the last one the
/// alpha (the others multiplied by it).
fn blur(pixels: &mut [u8], w: usize, h: usize, stride: usize, channels: usize, sigma: f64) {
    if !sigma.is_finite() || sigma <= 0. {
        return;
    }
    // Only where something is drawn, and as far around as it spreads.
    let Some((x0, y0, x1, y1)) = drawn_box(pixels, w, h, stride, channels) else {
        return;
    };
    let passes = Passes::new(sigma);
    let reach = passes.reach();
    let (bx0, bx1) = (x0.saturating_sub(reach), (x1 + reach).min(w));
    let (by0, by1) = (y0.saturating_sub(reach), (y1 + reach).min(h));
    let longest = (bx1 - bx0).max(by1 - by0);
    let mut line = vec![0f32; longest];
    let mut work = vec![0f32; longest + 1];
    let byte = |v: f32| v.round().clamp(0., 255.) as u8;
    for y in y0..y1 {
        let row = &mut pixels[y * stride..];
        for c in 0..channels {
            let line = &mut line[..bx1 - bx0];
            for (v, x) in line.iter_mut().zip(bx0..bx1) {
                *v = row[x * channels + c] as f32;
            }
            passes.run(line, &mut work);
            for (v, x) in line.iter().zip(bx0..bx1) {
                row[x * channels + c] = byte(*v);
            }
        }
    }
    for x in bx0..bx1 {
        for c in 0..channels {
            let line = &mut line[..by1 - by0];
            for (v, y) in line.iter_mut().zip(by0..by1) {
                *v = pixels[y * stride + x * channels + c] as f32;
            }
            passes.run(line, &mut work);
            for (v, y) in line.iter().zip(by0..by1) {
                pixels[y * stride + x * channels + c] = byte(*v);
            }
        }
    }
}

/// The columns `x0..x1` and rows `y0..y1` that hold every pixel with some
/// alpha; `None`: there is none.
fn drawn_box(
    pixels: &[u8],
    w: usize,
    h: usize,
    stride: usize,
    channels: usize,
) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let row = &pixels[y * stride..y * stride + w * channels];
        let alpha = |x: usize| row[x * channels + channels - 1] != 0;
        let Some(first) = (0..w).find(|&x| alpha(x)) else {
            continue;
        };
        let last = (0..w).rev().find(|&x| alpha(x)).unwrap_or(first);
        x0 = x0.min(first);
        x1 = x1.max(last + 1);
        y0 = y0.min(y);
        y1 = y + 1;
    }
    (x1 > x0).then_some((x0, y0, x1, y1))
}

/// Fills `pixels` (rows of `stride` bytes, four a pixel) with `color` as
/// much as `alpha` (rows of `w` bytes) has: a shadow.
fn tint(pixels: &mut [u8], stride: usize, alpha: &[u8], w: usize, color: [f64; 4]) {
    let [r, g, b, a] = color.map(|v| v as f32);
    for (row, alphas) in pixels.chunks_exact_mut(stride).zip(alpha.chunks_exact(w)) {
        for (p, &v) in row.as_chunks_mut::<4>().0.iter_mut().zip(alphas) {
            let v = v as f32 * a;
            p.copy_from_slice(&[v * r, v * g, v * b, v].map(|c| c.round() as u8));
        }
    }
}

/// Multiplies `pixels` by how much `mask` shows of them, both red, green,
/// blue and alpha bytes with the colors multiplied by the alpha: its
/// brightness (times its alpha) if `luminance`, else its alpha.
fn mask_pixels(pixels: &mut [u8], mask: &[u8], luminance: bool) {
    for (p, m) in pixels
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(mask.as_chunks::<4>().0)
    {
        let shown = if luminance {
            // 0.2126 red + 0.7152 green + 0.0722 blue, in 256ths.
            (54 * m[0] as u32 + 183 * m[1] as u32 + 19 * m[2] as u32 + 128) >> 8
        } else {
            m[3] as u32
        };
        for c in p {
            *c = ((*c as u32 * shown + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [f64; 4] = [1., 0., 0., 1.];
    const BLUE: [f64; 4] = [0., 0., 1., 1.];

    fn frame() -> Rect {
        Rect {
            x: 10.,
            y: 20.,
            w: 100.,
            h: 160.,
        }
    }

    /// A `w` × `h` pixels bitmap.
    fn blank(w: f64, h: f64) -> Option<Bitmap> {
        let space = Space::new()?;
        Layer::new(space.0, w, h, 1.)?.image()
    }

    #[test]
    fn drawn_bitmaps_are_kept_within_a_byte_budget() {
        // 1000 × 1000 pixels: 4 MB each, three fit in 13 MB.
        let mut all = PictureCache::new(13 << 20);
        let mut decoded = 0;
        let mut get = |all: &mut PictureCache<Option<Bitmap>>, src: &str| {
            cached(all, src, |_| {
                decoded += 1;
                blank(1000., 1000.)
            })
            .is_some()
        };
        assert!(get(&mut all, "a"));
        assert!(get(&mut all, "a"));
        assert!(get(&mut all, "b"));
        assert!(get(&mut all, "c"));
        // `a` drawn again: `b` is now the least recently drawn.
        assert!(get(&mut all, "a"));
        assert!(get(&mut all, "d"));
        assert_eq!(all.len(), 3);
        assert!(all.get("b").is_none(), "the least recently drawn went");
        assert!(all.unshown_bytes() <= 13 << 20);
        drop(get);
        assert_eq!(decoded, 4, "`a` was decoded once");
    }

    #[test]
    fn a_stretched_bitmap_fills_its_frame() {
        assert_eq!(
            fit_rect(Fit::Stretch, (8., 8.), frame(), (0.5, 0.5)),
            frame()
        );
    }

    #[test]
    fn a_whole_bitmap_is_placed_in_the_room_left() {
        let at = |align| fit_rect(Fit::Meet, (8., 8.), frame(), align);
        let rect = |y| Rect {
            x: 10.,
            y,
            w: 100.,
            h: 100.,
        };
        assert_eq!(at((0.5, 0.5)), rect(50.));
        assert_eq!(at((0., 0.)), rect(20.));
        assert_eq!(at((1., 1.)), rect(80.));
    }

    #[test]
    fn a_covering_bitmap_overflows_by_its_alignment() {
        let at = |align| fit_rect(Fit::Slice, (8., 8.), frame(), align);
        let rect = |x| Rect {
            x,
            y: 20.,
            w: 160.,
            h: 160.,
        };
        assert_eq!(at((0.5, 0.5)), rect(-20.));
        assert_eq!(at((0., 0.)), rect(10.));
        assert_eq!(at((1., 1.)), rect(-50.));
    }

    #[test]
    fn a_line_starts_by_its_anchor() {
        assert_eq!(line_start(Anchor::Start, 60., 40.), 60.);
        assert_eq!(line_start(Anchor::Middle, 60., 40.), 40.);
        assert_eq!(line_start(Anchor::End, 60., 40.), 20.);
    }

    #[test]
    fn a_baseline_is_below_y_by_the_font() {
        let shift = |b| baseline_shift(b, 10., 2., 5.);
        assert_eq!(shift(Baseline::Alphabetic), 0.);
        assert_eq!(shift(Baseline::Middle), 2.5);
        assert_eq!(shift(Baseline::Central), 4.);
        assert_eq!(shift(Baseline::Hanging), 8.);
    }

    #[test]
    fn a_matrix_keeps_its_order() {
        let m = Affine::new([1., 2., 3., 4., 5., 6.]);
        assert_eq!((m.a, m.b, m.c, m.d, m.tx, m.ty), (1., 2., 3., 4., 5., 6.));
        assert_eq!(m.det(), -2.);
        assert!(m.usable());
        assert!(!Affine::new([1., 2., 2., 4., 0., 0.]).usable());
        assert!(!Affine::new([f32::NAN, 0., 0., 1., 0., 0.]).usable());
    }

    #[test]
    fn stops_stay_in_order_within_the_gradient() {
        let stops = clean_stops([(-1., RED), (0.6, BLUE), (0.4, RED), (2., BLUE)].into_iter());
        let offsets: Vec<f64> = stops.iter().map(|s| s.0).collect();
        assert_eq!(offsets, [0., 0.6f32 as f64, 0.6f32 as f64, 1.]);
    }

    #[test]
    fn a_gradient_that_stops_is_laid_out_once() {
        let (locations, colors) = spread_stops(&[(0., RED), (1., BLUE)], Spread::Pad, 0, 1);
        assert_eq!(locations, [0., 1.]);
        assert_eq!(colors, [RED, BLUE].concat());
    }

    #[test]
    fn a_gradient_that_repeats_starts_over() {
        let stops = [(0., RED), (1., BLUE)];
        let (locations, colors) = spread_stops(&stops, Spread::Repeat, -1, 1);
        assert_eq!(locations, [0., 0.5, 0.5, 1.]);
        assert_eq!(colors, [RED, BLUE, RED, BLUE].concat());
    }

    #[test]
    fn a_cycle_keeps_its_end_colors_up_to_its_ends() {
        let stops = [(0.25, RED), (0.5, BLUE)];
        let (locations, colors) = spread_stops(&stops, Spread::Repeat, 0, 2);
        assert_eq!(locations, [0., 0.125, 0.25, 0.5, 0.5, 0.625, 0.75, 1.]);
        assert_eq!(
            colors,
            [RED, RED, BLUE, BLUE, RED, RED, BLUE, BLUE].concat()
        );
    }

    #[test]
    fn a_gradient_that_reflects_runs_back_every_other_time() {
        let stops = [(0., RED), (1., BLUE)];
        let (locations, colors) = spread_stops(&stops, Spread::Reflect, -1, 2);
        // Cycle -1 runs backwards, 0 forwards, 1 backwards.
        let third = 1. / 3.;
        let expected = [0., 1., 1., 2., 2., 3.].map(|v: f64| v * third);
        assert_eq!(locations.len(), expected.len());
        for (got, want) in locations.iter().zip(expected) {
            assert!((got - want).abs() < 1e-12, "{locations:?}");
        }
        assert_eq!(colors, [BLUE, RED, RED, BLUE, BLUE, RED].concat());
    }

    #[test]
    fn a_line_repeats_as_far_as_what_shows() {
        let corners = [(-25., 0.), (35., 0.), (-25., 50.), (35., 50.)];
        // Along x, 10 long: from 2.5 cycles before to 2.5 after its end.
        assert_eq!(linear_cycles((0., 0.), (10., 0.), &corners), (-3, 4));
        // What shows is inside the gradient: the gradient alone.
        let inside = [(2., 0.), (8., 0.)];
        assert_eq!(linear_cycles((0., 0.), (10., 0.), &inside), (0, 1));
    }

    #[test]
    fn circles_repeat_as_far_as_what_shows() {
        let corners = [(0., 0.), (35., 0.)];
        assert_eq!(radial_cycles((0., 0.), 10., (0., 0.), &corners), 4);
        // An off-center focal point: the circle through (35, 0) has its
        // center 2.33 × 5 away and a radius of 2.33 × 10.
        assert_eq!(radial_cycles((5., 0.), 10., (0., 0.), &corners), 3);
        // A focal point on the circle: no circles to count.
        assert_eq!(radial_cycles((10., 0.), 10., (0., 0.), &corners), 1);
    }

    #[test]
    fn a_large_view_gets_fewer_pixels() {
        assert_eq!(bitmap_scale(320., 200., 2.), 2.);
        assert_eq!(bitmap_scale(320., 200., f64::NAN), 1.);
        let scale = bitmap_scale(8000., 8000., 2.);
        assert!((8000. * 8000. * scale * scale - MAX_PIXELS).abs() < 1.);
    }

    #[test]
    fn a_blur_spreads_a_pixel_evenly_and_keeps_its_weight() {
        for sigma in [0.5, 1.5, 2., 3.3, 6.] {
            let (w, h) = (64, 64);
            let mut plane = vec![0u8; w * h];
            // A 4 × 4 square of full pixels in the middle.
            for y in 30..34 {
                plane[y * w + 30..y * w + 34].fill(255);
            }
            blur(&mut plane, w, h, w, 1, sigma);
            let sum: u32 = plane.iter().map(|&v| v as u32).sum();
            let expected = 16. * 255.;
            assert!(
                (sum as f64 - expected).abs() < expected * 0.06,
                "sigma {sigma}: {sum}"
            );
            // The same on both sides, across and down.
            for d in 1..20 {
                let at = |x: usize, y: usize| plane[y * w + x] as i32;
                assert!(
                    (at(31 - d, 31) - at(32 + d, 32)).abs() <= 1,
                    "sigma {sigma}"
                );
                assert!(
                    (at(31, 31 - d) - at(32, 32 + d)).abs() <= 1,
                    "sigma {sigma}"
                );
                assert!(
                    (at(31 - d, 31) - at(31, 31 - d)).abs() <= 2,
                    "sigma {sigma}"
                );
            }
            // Fainter further away, and gone past three deviations.
            assert!(plane[31 * w + 31] > plane[31 * w + 27]);
            assert_eq!(plane[31 * w + 31 - 4 * sigma.ceil() as usize - 4], 0);
        }
    }

    #[test]
    fn a_blur_is_as_wide_as_its_deviation() {
        // An edge between nothing and full pixels: blurred, it goes from
        // 16% to 84% over two deviations.
        for sigma in [1., 1.9, 4., 8.] {
            let (w, h) = (200, 80);
            let mut plane = vec![0u8; w * h];
            for row in plane.chunks_exact_mut(w) {
                row[100..].fill(255);
            }
            blur(&mut plane, w, h, w, 1, sigma);
            let row = &plane[40 * w..41 * w];
            let reaches = |part: f64| {
                let i = row.iter().position(|&v| v as f64 >= part * 255.).unwrap();
                let (a, b) = (row[i - 1] as f64, row[i] as f64);
                (i - 1) as f64 + (part * 255. - a) / (b - a)
            };
            let width = reaches(0.8413) - reaches(0.1587);
            assert!(
                (width - 2. * sigma).abs() < 0.3 * sigma,
                "sigma {sigma}: {width}"
            );
        }
    }

    #[test]
    fn a_blur_keeps_every_channel_and_skips_the_padding() {
        // Two pixels a row of red at half alpha, with 3 bytes of padding.
        let (w, h, stride) = (2, 2, 11);
        let mut pixels = vec![9u8; stride * h];
        for y in 0..h {
            pixels[y * stride..y * stride + 8].copy_from_slice(&[128, 0, 0, 128, 128, 0, 0, 128]);
        }
        blur(&mut pixels, w, h, stride, 4, 0.5);
        for y in 0..h {
            let row = &pixels[y * stride..(y + 1) * stride];
            assert_eq!(row[8..], [9, 9, 9]);
            for p in row[..8].as_chunks::<4>().0 {
                assert_eq!((p[1], p[2]), (0, 0));
                assert_eq!(p[0], p[3]);
                assert!(p[3] < 128 && p[3] > 60, "{p:?}");
            }
        }
    }

    #[test]
    fn nothing_drawn_is_not_blurred() {
        let mut plane = vec![0u8; 16];
        blur(&mut plane, 4, 4, 4, 1, 3.);
        assert_eq!(plane, [0; 16]);
        assert_eq!(drawn_box(&plane, 4, 4, 4, 1), None);
        plane[4 + 2] = 1;
        plane[2 * 4 + 1] = 1;
        assert_eq!(drawn_box(&plane, 4, 4, 4, 1), Some((1, 1, 3, 3)));
    }

    #[test]
    fn a_shadow_has_its_color_where_the_content_is() {
        let mut pixels = vec![7u8; 2 * 9];
        tint(&mut pixels, 9, &[255, 0, 128, 255], 2, [1., 0.5, 0., 0.5]);
        assert_eq!(pixels[..8], [128, 64, 0, 128, 0, 0, 0, 0]);
        assert_eq!(pixels[9..17], [64, 32, 0, 64, 128, 64, 0, 128]);
        // The padding of each row is left alone.
        assert_eq!((pixels[8], pixels[17]), (7, 7));
    }

    #[test]
    fn a_bitmap_drawn_much_larger_is_enlarged_first() {
        // About its size, or smaller: drawn as it is.
        assert_eq!(enlarged_size((100., 100.), (110., 90.)), None);
        assert_eq!(enlarged_size((100., 100.), (50., 20.)), None);
        assert_eq!(enlarged_size((8., 8.), (320., 320.)), Some((320, 320)));
        // Only the side that grows.
        assert_eq!(enlarged_size((8., 100.), (320., 50.)), Some((320, 100)));
        // Not past what is kept in memory.
        let (w, h) = enlarged_size((100., 50.), (8000., 4000.)).unwrap();
        assert!(
            w * h <= MAX_ENLARGED as usize + 4000 && w == 2 * h,
            "{w} {h}"
        );
        assert_eq!(enlarged_size((4000., 4000.), (8000., 8000.)), None);
        assert_eq!(enlarged_size((0., 8.), (320., 320.)), None);
    }

    #[test]
    fn an_enlarged_pixel_is_between_its_nearest() {
        // Black then white, opaque, with a byte of padding a row.
        let from = [0, 0, 0, 255, 255, 255, 255, 255, 9];
        let mut to = [7u8; 8 * 4 + 3];
        enlarge(&from, (2, 1, 9), &mut to, (8, 1, 35));
        let reds: Vec<u8> = to[..32].as_chunks::<4>().0.iter().map(|p| p[0]).collect();
        // The centers of the two pixels are at 2 and 6 of 8: black before,
        // white after, even steps between.
        assert_eq!(reds, [0, 0, 32, 96, 159, 223, 255, 255]);
        assert!(to[..32].as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        assert_eq!(to[32..], [7, 7, 7]);
        // Down as across.
        let from = [0, 0, 0, 255, 255, 255, 255, 255];
        let mut to = [0u8; 16];
        enlarge(&from, (1, 2, 4), &mut to, (1, 4, 4));
        assert_eq!([to[0], to[4], to[8], to[12]], [0, 64, 191, 255]);
    }

    #[test]
    fn a_mask_shows_by_its_brightness_or_its_alpha() {
        // White, black, half-transparent white, nothing.
        let mask = [
            255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 128, 0, 0, 0, 0,
        ];
        let content = [200u8, 100, 50, 255].repeat(4);
        let mut by_luminance = content.clone();
        mask_pixels(&mut by_luminance, &mask, true);
        assert_eq!(
            by_luminance,
            [200, 100, 50, 255, 0, 0, 0, 0, 100, 50, 25, 128, 0, 0, 0, 0]
        );
        let mut by_alpha = content.clone();
        mask_pixels(&mut by_alpha, &mask, false);
        assert_eq!(
            by_alpha,
            [
                200, 100, 50, 255, 200, 100, 50, 255, 100, 50, 25, 128, 0, 0, 0, 0
            ]
        );
    }
}
