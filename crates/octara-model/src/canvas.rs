//! Recorded drawing commands in local coordinates.
//!
//! A Canvas records paths, images and text without a window or reactive runtime.
//! The UI layer supplies its size and decides when to record it again.

use super::{FillRule, Image, Path, PathStyle, Stroke, Transform};
use crate::geometry::{Align, Point, Rect, Size, point};
use crate::theme::{ColorValue, Fill};

/// One recorded drawing step, in the canvas's coordinates (`transform` maps the path's).
#[derive(Clone, PartialEq, Debug)]
pub enum CanvasOp {
    Path {
        path: Path,
        style: PathStyle,
        fill: Fill,
        transform: Transform,
    },
    /// An image drawn into `rect` (the bounding box of the rect under the transform then).
    Image {
        image: Image,
        rect: Rect,
    },
    Text(CanvasText),
    /// Clips to the transformed bounding box `rect` with the given corner radius until `PopClip`.
    PushClip {
        rect: Rect,
        radius: f32,
    },
    PopClip,
}

/// A text on a canvas. Options left unset take the canvas element's own text style and color
/// (so `text_size(FontSizeToken::Sm)` or `text_color(..)` on the canvas styles its labels).
#[derive(Clone, PartialEq, Debug)]
pub struct CanvasText {
    pub text: String,
    /// Where the text is anchored, as `align` and `valign` say (under the transform then).
    pub at: Point,
    /// Which side of the text `at` is horizontally: start (the default), center or end.
    pub align: Align,
    /// Which side of the line `at` is vertically: top (the default), middle or bottom.
    pub valign: Align,
    pub color: Option<ColorValue>,
    pub size: Option<f32>,
    pub weight: Option<u16>,
    /// Wraps at this width (unbounded: one line per paragraph).
    pub max_width: Option<f32>,
    /// The scale applied to the text's size.
    pub scale: f32,
}

impl CanvasText {
    pub fn align(&mut self, a: Align) -> &mut Self {
        self.align = a;
        self
    }
    pub fn valign(&mut self, a: Align) -> &mut Self {
        self.valign = a;
        self
    }
    /// A token or a color.
    pub fn color(&mut self, c: impl Into<ColorValue>) -> &mut Self {
        self.color = Some(c.into());
        self
    }
    /// The font size in pixels.
    pub fn size(&mut self, px: f32) -> &mut Self {
        self.size = Some(px);
        self
    }
    /// The font weight (400 regular, 600 semibold).
    pub fn weight(&mut self, w: u16) -> &mut Self {
        self.weight = Some(w);
        self
    }
    pub fn max_width(&mut self, w: f32) -> &mut Self {
        self.max_width = Some(w);
        self
    }
}

/// What a canvas closure draws into. See the [module docs](self).
#[derive(Clone, Debug, PartialEq)]
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
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
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
    /// Strokes `path` at a width scaled by the transform.
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

    /// Draws `text` anchored at `at` (its top-left corner unless `align` / `valign` say
    /// otherwise), in the canvas's text style. Transforms apply to the anchor and scale.
    pub fn text(&mut self, text: impl Into<String>, at: impl Into<Point>) -> &mut CanvasText {
        let t = self.transform;
        let scale = (t.xx * t.yy - t.xy * t.yx).abs().sqrt();
        self.ops.push(CanvasOp::Text(CanvasText {
            text: text.into(),
            at: t.apply(at.into()),
            align: Align::Start,
            valign: Align::Start,
            color: None,
            size: None,
            weight: None,
            max_width: None,
            scale,
        }));
        let Some(CanvasOp::Text(t)) = self.ops.last_mut() else { unreachable!() };
        t
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
