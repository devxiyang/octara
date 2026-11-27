//! Vector geometry for icons and canvases.
//!
//! A [`Path`] shares its commands across clones. Renderers can cache the rasterized
//! result by its content hash.
//!
//! ```
//! use octara_model::geometry::point;
//! use octara_model::paint::{FillRule, LineCap, Path, Stroke};
//!
//! let check = Path::new().move_to(point(4.0, 12.0)).line_to(point(9.0, 17.0)).line_to(point(20.0, 6.0));
//! let ring = Path::circle(point(12.0, 12.0), 10.0).append(&Path::circle(point(12.0, 12.0), 6.0));
//! let _ = (Stroke::new(2.0).cap(LineCap::Round), FillRule::EvenOdd, check, ring);
//! ```
//!
//! A [`crate::paint::PathPaint`] tells the renderer how to fill or stroke the path.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::geometry::{Point, Rect, point};

/// An affine transform: `x' = xx·x + yx·y + x`, `y' = xy·x + yy·y + y`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Transform {
    pub xx: f32,
    pub xy: f32,
    pub yx: f32,
    pub yy: f32,
    pub x: f32,
    pub y: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform { xx: 1.0, xy: 0.0, yx: 0.0, yy: 1.0, x: 0.0, y: 0.0 };

    pub fn translate(x: f32, y: f32) -> Transform {
        Transform { x, y, ..Transform::IDENTITY }
    }
    pub fn scale(sx: f32, sy: f32) -> Transform {
        Transform { xx: sx, yy: sy, ..Transform::IDENTITY }
    }
    /// A rotation by `degrees`, clockwise on screen (y points down).
    pub fn rotate(degrees: f32) -> Transform {
        let (s, c) = degrees.to_radians().sin_cos();
        Transform { xx: c, xy: s, yx: -s, yy: c, x: 0.0, y: 0.0 }
    }

    /// This transform followed by `next`.
    pub fn then(&self, next: &Transform) -> Transform {
        let a = self;
        let b = next;
        Transform {
            xx: a.xx * b.xx + a.xy * b.yx,
            xy: a.xx * b.xy + a.xy * b.yy,
            yx: a.yx * b.xx + a.yy * b.yx,
            yy: a.yx * b.xy + a.yy * b.yy,
            x: a.x * b.xx + a.y * b.yx + b.x,
            y: a.x * b.xy + a.y * b.yy + b.y,
        }
    }

    pub fn apply(&self, p: Point) -> Point {
        Point { x: self.xx * p.x + self.yx * p.y + self.x, y: self.xy * p.x + self.yy * p.y + self.y }
    }

    pub fn invert(&self) -> Option<Transform> {
        let det = self.xx * self.yy - self.xy * self.yx;
        if det.abs() < f32::EPSILON {
            return None;
        }
        let inv = 1.0 / det;
        let (xx, xy, yx, yy) = (self.yy * inv, -self.xy * inv, -self.yx * inv, self.xx * inv);
        Some(Transform { xx, xy, yx, yy, x: -(self.x * xx + self.y * yx), y: -(self.x * xy + self.y * yy) })
    }
}

/// One path command (cubic and quadratic curves keep their control points first).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PathCmd {
    MoveTo(Point),
    LineTo(Point),
    /// A quadratic curve: control point, end point.
    QuadTo(Point, Point),
    /// A cubic curve: two control points, end point.
    CurveTo(Point, Point, Point),
    Close,
}

/// How a fill decides what is inside where subpaths overlap.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum FillRule {
    /// Inside where the winding number is not zero (a reversed inner subpath makes a hole).
    #[default]
    NonZero,
    /// Inside where an odd number of edges is crossed (any inner subpath makes a hole).
    EvenOdd,
}

/// The ends of open subpaths.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum LineCap {
    #[default]
    Butt,
    Round,
    Square,
}

/// Where stroke segments meet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum LineJoin {
    /// A sharp corner, beveled beyond the miter limit.
    #[default]
    Miter,
    Round,
    Bevel,
}

/// How a path is stroked: width (logical pixels), caps, joins and dashes.
#[derive(Clone, PartialEq, Debug)]
pub struct Stroke {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    /// The miter length (in widths) beyond which a miter join is beveled.
    pub miter_limit: f32,
    /// Alternating dash and gap lengths. An empty pattern makes a solid line.
    pub dash: Arc<[f32]>,
    pub dash_offset: f32,
}

impl Stroke {
    pub fn new(width: f32) -> Stroke {
        Stroke { width, cap: LineCap::Butt, join: LineJoin::Miter, miter_limit: 4.0, dash: Arc::from([]), dash_offset: 0.0 }
    }
    pub fn cap(mut self, cap: LineCap) -> Stroke {
        self.cap = cap;
        self
    }
    pub fn join(mut self, join: LineJoin) -> Stroke {
        self.join = join;
        self
    }
    pub fn miter_limit(mut self, limit: f32) -> Stroke {
        self.miter_limit = limit;
        self
    }
    /// Dash and gap lengths, alternating (`&[4.0, 2.0]`).
    pub fn dash(mut self, lengths: &[f32]) -> Stroke {
        self.dash = Arc::from(lengths);
        self
    }
    pub fn dash_offset(mut self, offset: f32) -> Stroke {
        self.dash_offset = offset;
        self
    }

    /// How far the stroke reaches beyond the path's geometry, in the stroke's own units.
    pub fn reach(&self) -> f32 {
        let half = self.width * 0.5;
        let join = if self.join == LineJoin::Miter { half * self.miter_limit.max(1.0) } else { half };
        let cap = if self.cap == LineCap::Square { half * std::f32::consts::SQRT_2 } else { half };
        join.max(cap)
    }
}

impl Hash for Stroke {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let bits = |value: f32| if value == 0.0 { 0 } else { value.to_bits() };
        bits(self.width).hash(h);
        (self.cap, self.join).hash(h);
        bits(self.miter_limit).hash(h);
        for d in self.dash.iter() {
            bits(*d).hash(h);
        }
        bits(self.dash_offset).hash(h);
    }
}

/// What painting a path does: fill its inside, or stroke its outline.
#[derive(Clone, PartialEq, Hash, Debug)]
pub enum PathStyle {
    Fill(FillRule),
    Stroke(Stroke),
}

/// A vector path: subpaths of lines and curves. See the [module docs](self).
#[derive(Clone, Debug)]
pub struct Path {
    commands: Arc<Vec<PathCmd>>,
    hash: u64,
    start: Point,
    current: Point,
}

impl Default for Path {
    fn default() -> Self {
        Path::new()
    }
}

impl PartialEq for Path {
    fn eq(&self, o: &Path) -> bool {
        self.hash == o.hash && (Arc::ptr_eq(&self.commands, &o.commands) || self.commands == o.commands)
    }
}

/// Kappa for a quarter circle drawn as one cubic.
const KAPPA: f32 = 0.552_284_8;

fn mix(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95)
}

fn hash_command(h: u64, c: &PathCmd) -> u64 {
    let p = |h: u64, p: &Point| mix(mix(h, p.x.to_bits() as u64), p.y.to_bits() as u64);
    match c {
        PathCmd::MoveTo(a) => p(mix(h, 1), a),
        PathCmd::LineTo(a) => p(mix(h, 2), a),
        PathCmd::QuadTo(a, b) => p(p(mix(h, 3), a), b),
        PathCmd::CurveTo(a, b, c) => p(p(p(mix(h, 4), a), b), c),
        PathCmd::Close => mix(h, 5),
    }
}

impl Path {
    /// An empty path.
    pub fn new() -> Path {
        Path { commands: Arc::new(Vec::new()), hash: 0, start: Point::default(), current: Point::default() }
    }

    fn push(mut self, c: PathCmd) -> Path {
        if self.commands.is_empty() {
            match c {
                PathCmd::MoveTo(_) => {}
                PathCmd::Close => return self,
                _ => self = self.move_to(Point::default()),
            }
        }
        self.hash = hash_command(self.hash, &c);
        match c {
            PathCmd::MoveTo(p) => {
                self.start = p;
                self.current = p;
            }
            PathCmd::LineTo(p) | PathCmd::QuadTo(_, p) | PathCmd::CurveTo(_, _, p) => self.current = p,
            PathCmd::Close => self.current = self.start,
        }
        Arc::make_mut(&mut self.commands).push(c);
        self
    }

    /// Builds a path from its drawing commands.
    pub fn from_commands(commands: impl IntoIterator<Item = PathCmd>) -> Path {
        commands.into_iter().fold(Path::new(), Path::push)
    }

    /// Begins a subpath at `to`.
    pub fn move_to(self, to: impl Into<Point>) -> Path {
        self.push(PathCmd::MoveTo(to.into()))
    }
    /// A straight line to `to`.
    pub fn line_to(self, to: impl Into<Point>) -> Path {
        self.push(PathCmd::LineTo(to.into()))
    }
    /// A quadratic curve to `to` with the control point `ctrl` (GPUI's order).
    pub fn curve_to(self, to: impl Into<Point>, ctrl: impl Into<Point>) -> Path {
        self.push(PathCmd::QuadTo(ctrl.into(), to.into()))
    }
    /// A cubic curve to `to` with control points `control_a` and `control_b` (GPUI's order).
    pub fn cubic_bezier_to(self, to: impl Into<Point>, control_a: impl Into<Point>, control_b: impl Into<Point>) -> Path {
        self.push(PathCmd::CurveTo(control_a.into(), control_b.into(), to.into()))
    }
    /// An elliptical arc to `to` (SVG's `A`): radii, the ellipse's rotation in degrees, and which
    /// of the four candidate arcs (`large_arc`, `sweep` clockwise).
    pub fn arc_to(self, radii: impl Into<Point>, x_rotation: f32, large_arc: bool, sweep: bool, to: impl Into<Point>) -> Path {
        let (r, to, from) = (radii.into(), to.into(), self.current);
        let k = |p: Point| kurbo::Point::new(p.x as f64, p.y as f64);
        let arc = kurbo::SvgArc {
            from: k(from),
            to: k(to),
            radii: kurbo::Vec2::new(r.x as f64, r.y as f64),
            x_rotation: (x_rotation as f64).to_radians(),
            large_arc,
            sweep,
        };
        match kurbo::Arc::from_svg_arc(&arc) {
            Some(a) => a.append_iter(0.01).map(from_kurbo).fold(self, Path::push),
            None => self.line_to(to),
        }
    }
    /// A polygon through `points`, closed or not.
    pub fn add_polygon(self, points: &[Point], closed: bool) -> Path {
        let Some((first, rest)) = points.split_first() else { return self };
        let p = rest.iter().fold(self.move_to(*first), |p, q| p.line_to(*q));
        if closed { p.close() } else { p }
    }
    /// Closes the subpath with a line back to its start.
    pub fn close(self) -> Path {
        self.push(PathCmd::Close)
    }
    /// This path followed by the subpaths of `other`.
    pub fn append(self, other: &Path) -> Path {
        other.commands.iter().copied().fold(self, Path::push)
    }
    /// The path so far, transformed.
    pub fn transform(self, t: Transform) -> Path {
        let f = |p: Point| t.apply(p);
        Path::from_commands(self.commands.iter().map(|c| match *c {
            PathCmd::MoveTo(a) => PathCmd::MoveTo(f(a)),
            PathCmd::LineTo(a) => PathCmd::LineTo(f(a)),
            PathCmd::QuadTo(a, b) => PathCmd::QuadTo(f(a), f(b)),
            PathCmd::CurveTo(a, b, c) => PathCmd::CurveTo(f(a), f(b), f(c)),
            PathCmd::Close => PathCmd::Close,
        }))
    }

    /// A rectangle.
    pub fn rect(r: Rect) -> Path {
        Path::new().add_polygon(&[point(r.x, r.y), point(r.right(), r.y), point(r.right(), r.bottom()), point(r.x, r.bottom())], true)
    }
    /// A rectangle with rounded corners (the radius is limited to half the shorter side).
    pub fn rounded_rect(r: Rect, radius: f32) -> Path {
        let k = radius.min(r.w * 0.5).min(r.h * 0.5).max(0.0);
        if k <= 0.0 {
            return Path::rect(r);
        }
        let c = k * (1.0 - KAPPA);
        let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
        Path::new()
            .move_to(point(x0 + k, y0))
            .line_to(point(x1 - k, y0))
            .cubic_bezier_to(point(x1, y0 + k), point(x1 - c, y0), point(x1, y0 + c))
            .line_to(point(x1, y1 - k))
            .cubic_bezier_to(point(x1 - k, y1), point(x1, y1 - c), point(x1 - c, y1))
            .line_to(point(x0 + k, y1))
            .cubic_bezier_to(point(x0, y1 - k), point(x0 + c, y1), point(x0, y1 - c))
            .line_to(point(x0, y0 + k))
            .cubic_bezier_to(point(x0 + k, y0), point(x0, y0 + c), point(x0 + c, y0))
            .close()
    }
    /// An ellipse around `center`.
    pub fn ellipse(center: impl Into<Point>, rx: f32, ry: f32) -> Path {
        let c = center.into();
        let (kx, ky) = (rx * KAPPA, ry * KAPPA);
        Path::new()
            .move_to(point(c.x + rx, c.y))
            .cubic_bezier_to(point(c.x, c.y + ry), point(c.x + rx, c.y + ky), point(c.x + kx, c.y + ry))
            .cubic_bezier_to(point(c.x - rx, c.y), point(c.x - kx, c.y + ry), point(c.x - rx, c.y + ky))
            .cubic_bezier_to(point(c.x, c.y - ry), point(c.x - rx, c.y - ky), point(c.x - kx, c.y - ry))
            .cubic_bezier_to(point(c.x + rx, c.y), point(c.x + kx, c.y - ry), point(c.x + rx, c.y - ky))
            .close()
    }
    /// A circle around `center`.
    pub fn circle(center: impl Into<Point>, r: f32) -> Path {
        Path::ellipse(center, r, r)
    }
    /// The commands, in order.
    pub fn commands(&self) -> &[PathCmd] {
        &self.commands
    }
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
    /// A hash of the commands: equal paths hash equally, however they were built.
    pub fn content_hash(&self) -> u64 {
        self.hash
    }

    /// The box around every point of the path (control points included), or `None` for an
    /// empty path.
    pub fn bounds(&self) -> Option<Rect> {
        let mut pts = self.commands.iter().flat_map(|c| match *c {
            PathCmd::MoveTo(a) | PathCmd::LineTo(a) => [Some(a), None, None],
            PathCmd::QuadTo(a, b) => [Some(a), Some(b), None],
            PathCmd::CurveTo(a, b, c) => [Some(a), Some(b), Some(c)],
            PathCmd::Close => [None, None, None],
        });
        let first = pts.find_map(|p| p)?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in pts.flatten() {
            (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
        }
        Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// Whether the point is inside the path filled with the given rule.
    pub fn contains(&self, p: impl Into<Point>, rule: FillRule) -> bool {
        use kurbo::Shape;
        let point = |p: Point| kurbo::Point::new(p.x as f64, p.y as f64);
        let mut path = kurbo::BezPath::new();
        let mut open = false;
        for command in self.commands() {
            match *command {
                PathCmd::MoveTo(a) => {
                    if open {
                        path.close_path();
                    }
                    path.move_to(point(a));
                }
                PathCmd::LineTo(a) => path.line_to(point(a)),
                PathCmd::QuadTo(a, b) => path.quad_to(point(a), point(b)),
                PathCmd::CurveTo(a, b, c) => path.curve_to(point(a), point(b), point(c)),
                PathCmd::Close => path.close_path(),
            }
            open = !matches!(command, PathCmd::Close);
        }
        if open {
            path.close_path();
        }
        let p = point(p.into());
        if !p.is_finite() || !path.is_finite() {
            return false;
        }
        let winding = path.winding(p);
        match rule {
            FillRule::NonZero => winding != 0,
            FillRule::EvenOdd => winding % 2 != 0,
        }
    }
}

fn from_kurbo(e: kurbo::PathEl) -> PathCmd {
    let p = |v: kurbo::Point| point(v.x as f32, v.y as f32);
    match e {
        kurbo::PathEl::MoveTo(a) => PathCmd::MoveTo(p(a)),
        kurbo::PathEl::LineTo(a) => PathCmd::LineTo(p(a)),
        kurbo::PathEl::QuadTo(a, b) => PathCmd::QuadTo(p(a), p(b)),
        kurbo::PathEl::CurveTo(a, b, c) => PathCmd::CurveTo(p(a), p(b), p(c)),
        kurbo::PathEl::ClosePath => PathCmd::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_hash_and_shapes() {
        let a = Path::new().move_to(point(0.0, 0.0)).line_to(point(10.0, 0.0)).line_to(point(10.0, 10.0)).close();
        let b = Path::from_commands([
            PathCmd::MoveTo(point(0.0, 0.0)),
            PathCmd::LineTo(point(10.0, 0.0)),
            PathCmd::LineTo(point(10.0, 10.0)),
            PathCmd::Close,
        ]);
        assert_eq!(a, b);
        assert_eq!(a.content_hash(), b.content_hash());
        assert_ne!(a.content_hash(), a.clone().line_to(point(1.0, 1.0)).content_hash());

        let ring = Path::circle(point(12.0, 12.0), 10.0).append(&Path::circle(point(12.0, 12.0), 6.0));
        assert!(ring.contains(point(12.0, 3.0), FillRule::EvenOdd));
        assert!(!ring.contains(point(12.0, 12.0), FillRule::EvenOdd), "a hole");
        assert!(ring.contains(point(12.0, 12.0), FillRule::NonZero), "same winding: no hole");
        let open_triangle = Path::new().add_polygon(&[point(0.0, 0.0), point(10.0, 0.0), point(0.0, 10.0)], false);
        assert!(open_triangle.contains(point(2.0, 2.0), FillRule::NonZero));
        assert!(!open_triangle.contains(point(-2.0, 2.0), FillRule::NonZero));
        let r = Path::rounded_rect(Rect { x: 0.0, y: 0.0, w: 20.0, h: 10.0 }, 4.0).bounds().unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 20.0, 10.0));

        let arc = Path::new().move_to(point(0.0, 0.0)).arc_to(point(5.0, 5.0), 0.0, false, true, point(10.0, 0.0));
        assert!(matches!(arc.commands().last(), Some(PathCmd::CurveTo(_, _, e)) if (e.x - 10.0).abs() < 1e-4));
        let t = Transform::rotate(90.0).then(&Transform::translate(5.0, 0.0));
        let q = t.apply(point(1.0, 0.0));
        assert!((q.x - 5.0).abs() < 1e-5 && (q.y - 1.0).abs() < 1e-5, "{q:?}");
        let back = t.invert().unwrap().apply(q);
        assert!((back.x - 1.0).abs() < 1e-5 && back.y.abs() < 1e-5);
    }
}
