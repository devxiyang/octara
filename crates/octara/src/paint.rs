//! The display list: what a frame draws, in order, as plain data.
//!
//! The kernel produces it from the laid-out tree ([`Kernel::display_list`]); renderers consume
//! it (a deterministic CPU reference renderer for screenshots, and a GPU renderer). Coordinates
//! are logical pixels with scroll offsets applied; a renderer multiplies by the screen scale.
//! Every item names the node it comes from, so a pixel can be traced back to an element.
//!
//! [`Kernel::display_list`]: crate::kernel::Kernel::display_list

pub use parley::FontData;

use crate::kernel::{Color, NodeId};

/// An axis-aligned rectangle in logical pixels.
#[derive(Clone, Copy, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        let (r, b) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        Rect { x, y, w: (r - x).max(0.0), h: (b - y).max(0.0) }
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

/// A linear gradient between two colors at fractions of the gradient line; `angle` in degrees
/// as CSS (0 up, 90 right). Colors mix premultiplied, as CSS.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Gradient {
    pub angle: f32,
    pub from: (Color, f32),
    pub to: (Color, f32),
}

impl Gradient {
    /// The (straight) color at `(px, py)` in the box `(x, y, w, h)`: the CSS gradient line through
    /// the box's center, long enough that the corners touch the end colors.
    pub fn at(&self, px: f32, py: f32, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        let a = self.angle.to_radians();
        let (dx, dy) = (a.sin(), -a.cos());
        let len = (w * dx).abs() + (h * dy).abs();
        let t = if len > 0.0 { ((px - (x + w * 0.5)) * dx + (py - (y + h * 0.5)) * dy) / len + 0.5 } else { 0.5 };
        let (p0, p1) = (self.from.1, self.to.1);
        let k = if p1 > p0 {
            ((t - p0) / (p1 - p0)).clamp(0.0, 1.0)
        } else if t < p0 {
            0.0
        } else {
            1.0
        };
        let (c0, c1) = (self.from.0.components, self.to.0.components);
        let pre = |c: [f32; 4]| [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]];
        let (a0, a1) = (pre(c0), pre(c1));
        let m: [f32; 4] = std::array::from_fn(|i| a0[i] + (a1[i] - a0[i]) * k);
        if m[3] <= 0.0 { [0.0; 4] } else { [m[0] / m[3], m[1] / m[3], m[2] / m[3], m[3]] }
    }
}

/// A rounded rectangle with an optional fill and border (borders are drawn inside the rect).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Quad {
    pub source: NodeId,
    pub rect: Rect,
    /// Corner radius, already limited to half the shorter side.
    pub radius: f32,
    pub fill: Option<Color>,
    /// A gradient fill (instead of `fill`).
    pub gradient: Option<Gradient>,
    /// Border widths: top, right, bottom, left.
    pub border: [f32; 4],
    pub border_color: Color,
}

/// A blurred rounded rectangle under an element (a box shadow), already offset and spread.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shadow {
    pub source: NodeId,
    pub rect: Rect,
    pub radius: f32,
    pub color: Color,
    /// The CSS blur radius: twice the Gaussian's standard deviation (0: a sharp edge).
    pub blur: f32,
}

impl Shadow {
    /// How far the blur reaches beyond `rect` (three standard deviations).
    pub fn reach(&self) -> f32 {
        1.5 * self.blur
    }
}

/// One glyph of a run: its id in the font and its pen position (x, baseline y).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// Glyphs of one font at one size and color.
#[derive(Clone, Debug)]
pub struct GlyphRun {
    pub source: NodeId,
    pub font: FontData,
    pub size: f32,
    /// Normalized variation coordinates (variable fonts), as the shaper used them.
    pub coords: Vec<i16>,
    /// Synthetic bold for a family without that weight.
    pub embolden: bool,
    pub color: Color,
    pub glyphs: Vec<Glyph>,
}

/// An image drawn into `rect` (where its fit put it), clipped to `bounds` rounded by `radius`
/// (its element's box).
#[derive(Clone, Debug)]
pub struct ImagePaint {
    pub source: NodeId,
    pub image: crate::image::Image,
    pub rect: Rect,
    pub bounds: Rect,
    pub radius: f32,
    pub opacity: f32,
}

/// A path filled or stroked with a color or a gradient. `transform` maps the path's
/// coordinates to the window's (logical pixels); a stroke's width scales with it.
#[derive(Clone, Debug)]
pub struct PathPaint {
    pub source: NodeId,
    pub path: crate::path::Path,
    pub transform: crate::path::Transform,
    pub style: crate::path::PathStyle,
    pub color: Color,
    /// A gradient (instead of `color`), spanning `bounds`.
    pub gradient: Option<Gradient>,
    /// The box a gradient spans (the element's), in window coordinates.
    pub bounds: Rect,
}

/// One drawing step.
#[derive(Clone, Debug)]
pub enum Item {
    Shadow(Shadow),
    Quad(Quad),
    Image(ImagePaint),
    Glyphs(GlyphRun),
    Path(PathPaint),
    /// Where a native view is in paint order (it draws nothing here): renderers with platform
    /// views clear an underlay view's cutout; `check` finds content painted over an overlay one.
    Native {
        source: NodeId,
        rect: Rect,
        radius: f32,
        slot: crate::native::Slot,
        /// The platform shows an underlay view here: the renderer clears the rect (rounded by
        /// `radius`) to transparent, so the view behind the surface shows through.
        cut: bool,
    },
    /// Clip what follows to `rect` (rounded by `radius`) until the matching `PopClip`.
    PushClip {
        source: NodeId,
        rect: Rect,
        radius: f32,
    },
    PopClip,
    /// Draw what follows into a transparent layer until the matching `PopLayer`, then composite
    /// the layer at `opacity` (group opacity: overlapping parts do not show through each other).
    PushLayer {
        source: NodeId,
        opacity: f32,
    },
    PopLayer,
}

impl Item {
    /// Whether the item draws something (rather than clipping or grouping).
    pub fn draws(&self) -> bool {
        matches!(self, Item::Shadow(_) | Item::Quad(_) | Item::Image(_) | Item::Glyphs(_) | Item::Path(_))
    }

    /// Multiplies the item's colors' alpha by `k`.
    pub fn fade(&mut self, k: f32) {
        let f = |c: &mut Color| *c = c.multiply_alpha(k);
        match self {
            Item::Shadow(s) => f(&mut s.color),
            Item::Quad(q) => {
                if let Some(c) = &mut q.fill {
                    f(c);
                }
                f(&mut q.border_color);
                if let Some(g) = &mut q.gradient {
                    f(&mut g.from.0);
                    f(&mut g.to.0);
                }
            }
            Item::Image(i) => i.opacity *= k,
            Item::Glyphs(g) => f(&mut g.color),
            Item::Path(p) => {
                f(&mut p.color);
                if let Some(g) = &mut p.gradient {
                    f(&mut g.from.0);
                    f(&mut g.to.0);
                }
            }
            Item::PushClip { .. } | Item::PopClip | Item::PopLayer | Item::Native { .. } => {}
            Item::PushLayer { opacity, .. } => *opacity *= k,
        }
    }
}

/// A frame: the window size, its background and the items in drawing order.
#[derive(Clone, Debug)]
pub struct DisplayList {
    pub size: (f32, f32),
    pub background: Color,
    pub items: Vec<Item>,
}

impl DisplayList {
    pub fn quads(&self) -> impl Iterator<Item = &Quad> {
        self.items.iter().filter_map(|i| if let Item::Quad(q) = i { Some(q) } else { None })
    }
    pub fn paths(&self) -> impl Iterator<Item = &PathPaint> {
        self.items.iter().filter_map(|i| if let Item::Path(p) = i { Some(p) } else { None })
    }
    pub fn glyph_runs(&self) -> impl Iterator<Item = &GlyphRun> {
        self.items.iter().filter_map(|i| if let Item::Glyphs(g) = i { Some(g) } else { None })
    }
}
