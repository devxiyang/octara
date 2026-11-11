//! Canvas: a drawing built by a closure, as data.
//!
//! [`canvas`](crate::view::canvas)`(move |c| ..)` is an element whose drawing a closure records
//! into a [`Canvas`]: paths filled or stroked, rectangles, images, under scoped transforms and
//! clips, in the element's own coordinates (its content box, `(0, 0)` to
//! [`size`](Canvas::size)). The closure is a binding: it reruns only when what it reads (signals,
//! the element's size) changes, and the recording is kept as plain data ([`CanvasOp`]) that tests
//! can compare. Nothing redraws per frame unless something changed (Rune's declarative canvas,
//! Exact's recorder; not GPUI's per-frame paint closures).
//!
//! ```
//! use octara::prelude::*;
//!
//! fn meter() -> View {
//!     let level = state("level", || 0.6);
//!     canvas(move |c| {
//!         let (w, h) = c.size();
//!         c.fill(&Path::rounded_rect(Rect::new(0.0, 0.0, w, h), h / 2.0), Token::Surface);
//!         c.fill(&Path::rounded_rect(Rect::new(0.0, 0.0, w * level.get(), h), h / 2.0), Token::Accent);
//!     })
//!     .w(200)
//!     .h(8)
//!     .label("Level")
//!     .into()
//! }
//! # let app = App::new(meter);
//! # assert_eq!(app.kernel().canvas(app.kernel().find_by_name("Level")[0]).unwrap().len(), 2);
//! ```
//!
//! A canvas is an image to assistive technology: give it a `label` (`check` reports a visible
//! canvas without one, `UNLABELED_CANVAS`), or `a11y_hidden()` when it is decoration.

use crate::image::Image;
use crate::kernel::Fill;
use crate::paint::Rect;
use crate::path::{FillRule, Path, PathStyle, Stroke, Transform, point};

/// One recorded drawing step, in the canvas's coordinates (`transform` maps the path's).
#[derive(Clone, PartialEq, Debug)]
pub enum CanvasOp {
    Path { path: Path, style: PathStyle, fill: Fill, transform: Transform },
    /// An image drawn into `rect` (the bounding box of the rect under the transform then).
    Image { image: Image, rect: Rect },
    /// Clips what follows to `rect` (rounded by `radius`; the bounding box under the transform
    /// then) until the matching `PopClip`.
    PushClip { rect: Rect, radius: f32 },
    PopClip,
}

/// What a canvas closure draws into. See the [module docs](self).
#[derive(Debug)]
pub struct Canvas {
    width: f32,
    height: f32,
    transform: Transform,
    ops: Vec<CanvasOp>,
}

impl Canvas {
    /// An empty recording for a content box of `width` × `height`.
    pub fn new(width: f32, height: f32) -> Canvas {
        Canvas { width, height, transform: Transform::IDENTITY, ops: Vec::new() }
    }

    /// The content box's width and height: what `(0, 0)` to `size()` covers.
    pub fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }
    pub fn width(&self) -> f32 {
        self.width
    }
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Fills `path` (non-zero rule) with a token, a color or a gradient (a gradient spans the
    /// path's box).
    pub fn fill(&mut self, path: &Path, fill: impl Into<Fill>) {
        self.fill_with(path, FillRule::NonZero, fill);
    }
    /// Fills `path` with `rule` (`EvenOdd` makes every inner subpath a hole).
    pub fn fill_with(&mut self, path: &Path, rule: FillRule, fill: impl Into<Fill>) {
        self.ops.push(CanvasOp::Path { path: path.clone(), style: PathStyle::Fill(rule), fill: fill.into(), transform: self.transform });
    }
    /// Strokes `path`; the width scales with the transform.
    pub fn stroke(&mut self, path: &Path, stroke: &Stroke, fill: impl Into<Fill>) {
        self.ops.push(CanvasOp::Path {
            path: path.clone(),
            style: PathStyle::Stroke(stroke.clone()),
            fill: fill.into(),
            transform: self.transform,
        });
    }
    /// Fills a rectangle.
    pub fn rect(&mut self, rect: Rect, fill: impl Into<Fill>) {
        self.fill(&Path::rect(rect), fill);
    }
    /// Draws `image` stretched into `rect`.
    pub fn image(&mut self, image: &Image, rect: Rect) {
        let rect = self.bounding(rect);
        self.ops.push(CanvasOp::Image { image: image.clone(), rect });
    }

    /// Draws what `f` draws under `t` (after the transform already in place).
    pub fn transform(&mut self, t: Transform, f: impl FnOnce(&mut Canvas)) {
        let saved = self.transform;
        self.transform = t.then(&saved);
        f(self);
        self.transform = saved;
    }
    /// Draws what `f` draws clipped to `rect` rounded by `radius`.
    pub fn clip(&mut self, rect: Rect, radius: f32, f: impl FnOnce(&mut Canvas)) {
        let rect = self.bounding(rect);
        self.ops.push(CanvasOp::PushClip { rect, radius });
        f(self);
        self.ops.push(CanvasOp::PopClip);
    }

    /// The recording.
    pub fn ops(&self) -> &[CanvasOp] {
        &self.ops
    }
    pub fn into_ops(self) -> Vec<CanvasOp> {
        self.ops
    }

    /// `r`'s bounding box under the current transform.
    fn bounding(&self, r: Rect) -> Rect {
        let t = self.transform;
        let pts = [point(r.x, r.y), point(r.right(), r.y), point(r.x, r.bottom()), point(r.right(), r.bottom())].map(|p| t.apply(p));
        let x0 = pts.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let y0 = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let x1 = pts.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        let y1 = pts.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}
