//! Vector paths: the geometry icons and canvases draw.
//!
//! A [`Path`] is an immutable builder, like elements: each call returns the path with one more
//! command (GPUI's names). It is cheap to clone (the commands are shared) and carries a content
//! hash, so renderers cache what they rasterize by what the path is, not by which value it is.
//!
//! ```
//! use octara::path::{FillRule, LineCap, Path, Stroke, point};
//!
//! let check = Path::new().move_to(point(4.0, 12.0)).line_to(point(9.0, 17.0)).line_to(point(20.0, 6.0));
//! let ring = Path::circle(point(12.0, 12.0), 10.0).append(&Path::circle(point(12.0, 12.0), 6.0));
//! let _ = (Stroke::new(2.0).cap(LineCap::Round), FillRule::EvenOdd, check, ring);
//! ```
//!
//! Painting a path (a fill with a [`FillRule`], or a [`Stroke`]) is a display-list item
//! ([`crate::paint::PathPaint`]); both renderers rasterize it into the same coverage.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::paint::Rect;

/// A point in logical pixels.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// The point `(x, y)`.
pub fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Point {
        Point { x, y }
    }
}

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
pub enum PathCommand {
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
    /// Dash and gap lengths, alternating; empty for a solid line.
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
        self.width.to_bits().hash(h);
        (self.cap, self.join).hash(h);
        self.miter_limit.to_bits().hash(h);
        for d in self.dash.iter() {
            d.to_bits().hash(h);
        }
        self.dash_offset.to_bits().hash(h);
    }
}

/// What painting a path does: fill its inside, or stroke its outline.
#[derive(Clone, PartialEq, Hash, Debug)]
pub enum PathStyle {
    Fill(FillRule),
    Stroke(Stroke),
}

/// SVG path data that does not parse, and why.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PathError(pub String);

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid SVG path data: {}", self.0)
    }
}

impl std::error::Error for PathError {}

/// A vector path: subpaths of lines and curves. See the [module docs](self).
#[derive(Clone, Debug)]
pub struct Path {
    commands: Arc<Vec<PathCommand>>,
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

fn hash_command(h: u64, c: &PathCommand) -> u64 {
    let p = |h: u64, p: &Point| mix(mix(h, p.x.to_bits() as u64), p.y.to_bits() as u64);
    match c {
        PathCommand::MoveTo(a) => p(mix(h, 1), a),
        PathCommand::LineTo(a) => p(mix(h, 2), a),
        PathCommand::QuadTo(a, b) => p(p(mix(h, 3), a), b),
        PathCommand::CurveTo(a, b, c) => p(p(p(mix(h, 4), a), b), c),
        PathCommand::Close => mix(h, 5),
    }
}

impl Path {
    /// An empty path.
    pub fn new() -> Path {
        Path { commands: Arc::new(Vec::new()), hash: 0, start: Point::default(), current: Point::default() }
    }

    fn push(mut self, c: PathCommand) -> Path {
        self.hash = hash_command(self.hash, &c);
        match c {
            PathCommand::MoveTo(p) => {
                self.start = p;
                self.current = p;
            }
            PathCommand::LineTo(p) | PathCommand::QuadTo(_, p) | PathCommand::CurveTo(_, _, p) => self.current = p,
            PathCommand::Close => self.current = self.start,
        }
        Arc::make_mut(&mut self.commands).push(c);
        self
    }

    fn from_commands(commands: impl IntoIterator<Item = PathCommand>) -> Path {
        commands.into_iter().fold(Path::new(), Path::push)
    }

    /// Begins a subpath at `to`.
    pub fn move_to(self, to: impl Into<Point>) -> Path {
        self.push(PathCommand::MoveTo(to.into()))
    }
    /// A straight line to `to`.
    pub fn line_to(self, to: impl Into<Point>) -> Path {
        self.push(PathCommand::LineTo(to.into()))
    }
    /// A quadratic curve to `to` with the control point `ctrl` (GPUI's order).
    pub fn curve_to(self, to: impl Into<Point>, ctrl: impl Into<Point>) -> Path {
        self.push(PathCommand::QuadTo(ctrl.into(), to.into()))
    }
    /// A cubic curve to `to` with control points `control_a` and `control_b` (GPUI's order).
    pub fn cubic_bezier_to(self, to: impl Into<Point>, control_a: impl Into<Point>, control_b: impl Into<Point>) -> Path {
        self.push(PathCommand::CurveTo(control_a.into(), control_b.into(), to.into()))
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
        self.push(PathCommand::Close)
    }
    /// This path followed by the subpaths of `other`.
    pub fn append(self, other: &Path) -> Path {
        other.commands.iter().copied().fold(self, Path::push)
    }
    /// The path so far, transformed.
    pub fn transform(self, t: Transform) -> Path {
        let f = |p: Point| t.apply(p);
        Path::from_commands(self.commands.iter().map(|c| match *c {
            PathCommand::MoveTo(a) => PathCommand::MoveTo(f(a)),
            PathCommand::LineTo(a) => PathCommand::LineTo(f(a)),
            PathCommand::QuadTo(a, b) => PathCommand::QuadTo(f(a), f(b)),
            PathCommand::CurveTo(a, b, c) => PathCommand::CurveTo(f(a), f(b), f(c)),
            PathCommand::Close => PathCommand::Close,
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
    /// A path from SVG path data (`"M3 12h18"`): every command, relative and absolute, arcs
    /// included.
    pub fn svg(data: &str) -> Result<Path, PathError> {
        let p = kurbo::BezPath::from_svg(data).map_err(|e| PathError(e.to_string()))?;
        Ok(Path::from_commands(p.elements().iter().copied().map(from_kurbo)))
    }

    /// The path as SVG path data (absolute commands, numbers to a thousandth).
    pub fn to_svg(&self) -> String {
        use std::fmt::Write;
        fn num(out: &mut String, v: f32) {
            let r = (v * 1000.0).round() / 1000.0;
            let s = format!("{}", if r == 0.0 { 0.0 } else { r });
            // A space only where the number could run into the previous one.
            if !(out.is_empty() || out.ends_with(|c: char| c.is_ascii_alphabetic()) || s.starts_with('-')) {
                out.push(' ');
            }
            let s = s.strip_prefix("0.").map(|t| format!(".{t}")).or_else(|| s.strip_prefix("-0.").map(|t| format!("-.{t}"))).unwrap_or(s);
            let _ = write!(out, "{s}");
        }
        let mut out = String::new();
        for c in self.commands.iter() {
            let (verb, pts): (char, Vec<Point>) = match *c {
                PathCommand::MoveTo(a) => ('M', vec![a]),
                PathCommand::LineTo(a) => ('L', vec![a]),
                PathCommand::QuadTo(a, b) => ('Q', vec![a, b]),
                PathCommand::CurveTo(a, b, c) => ('C', vec![a, b, c]),
                PathCommand::Close => ('Z', Vec::new()),
            };
            out.push(verb);
            for p in pts {
                num(&mut out, p.x);
                num(&mut out, p.y);
            }
        }
        out
    }

    /// The commands, in order.
    pub fn commands(&self) -> &[PathCommand] {
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
            PathCommand::MoveTo(a) | PathCommand::LineTo(a) => [Some(a), None, None],
            PathCommand::QuadTo(a, b) => [Some(a), Some(b), None],
            PathCommand::CurveTo(a, b, c) => [Some(a), Some(b), Some(c)],
            PathCommand::Close => [None, None, None],
        });
        let first = pts.find_map(|p| p)?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in pts.flatten() {
            (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
        }
        Some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// Whether `p` is inside the path filled with `rule`.
    pub fn contains(&self, p: impl Into<Point>, rule: FillRule) -> bool {
        let p = p.into();
        let fill = match rule {
            FillRule::NonZero => zeno::Fill::NonZero,
            FillRule::EvenOdd => zeno::Fill::EvenOdd,
        };
        let cmds: Vec<zeno::Command> = self.commands.iter().map(to_zeno).collect();
        zeno::HitTest::new(&cmds[..]).style(fill).test(zeno::Point::new(p.x, p.y))
    }
}

fn from_kurbo(e: kurbo::PathEl) -> PathCommand {
    let p = |v: kurbo::Point| point(v.x as f32, v.y as f32);
    match e {
        kurbo::PathEl::MoveTo(a) => PathCommand::MoveTo(p(a)),
        kurbo::PathEl::LineTo(a) => PathCommand::LineTo(p(a)),
        kurbo::PathEl::QuadTo(a, b) => PathCommand::QuadTo(p(a), p(b)),
        kurbo::PathEl::CurveTo(a, b, c) => PathCommand::CurveTo(p(a), p(b), p(c)),
        kurbo::PathEl::ClosePath => PathCommand::Close,
    }
}

fn to_zeno(c: &PathCommand) -> zeno::Command {
    let p = |v: Point| zeno::Point::new(v.x, v.y);
    match *c {
        PathCommand::MoveTo(a) => zeno::Command::MoveTo(p(a)),
        PathCommand::LineTo(a) => zeno::Command::LineTo(p(a)),
        PathCommand::QuadTo(a, b) => zeno::Command::QuadTo(p(a), p(b)),
        PathCommand::CurveTo(a, b, c) => zeno::Command::CurveTo(p(a), p(b), p(c)),
        PathCommand::Close => zeno::Command::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_hash_and_shapes() {
        let a = Path::new().move_to(point(0.0, 0.0)).line_to(point(10.0, 0.0)).line_to(point(10.0, 10.0)).close();
        let b = Path::svg("M0 0 H10 V10 Z").unwrap();
        assert_eq!(a, b, "the same commands, however built");
        assert_eq!(a.content_hash(), b.content_hash());
        assert_ne!(a.content_hash(), a.clone().line_to(point(1.0, 1.0)).content_hash());
        assert_eq!(a.to_svg(), "M0 0L10 0L10 10Z");
        let c = Path::svg("M.5-1.25C1 2 3 4 5 6").unwrap();
        assert_eq!(c.to_svg(), "M.5-1.25C1 2 3 4 5 6");
        assert_eq!(Path::svg(&c.to_svg()).unwrap(), c);
        assert!(Path::svg("M0 0 K1 1").is_err());
        let implicit = Path::svg("M7 10H6a4 4 0 0 1-4-4 1 1 0 0 1 1-1h4").unwrap();
        assert!(implicit.commands().len() > 4, "arcs repeat without their letter");

        let ring = Path::circle(point(12.0, 12.0), 10.0).append(&Path::circle(point(12.0, 12.0), 6.0));
        assert!(ring.contains(point(12.0, 3.0), FillRule::EvenOdd));
        assert!(!ring.contains(point(12.0, 12.0), FillRule::EvenOdd), "a hole");
        assert!(ring.contains(point(12.0, 12.0), FillRule::NonZero), "same winding: no hole");
        let r = Path::rounded_rect(Rect { x: 0.0, y: 0.0, w: 20.0, h: 10.0 }, 4.0).bounds().unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 20.0, 10.0));

        let arc = Path::new().move_to(point(0.0, 0.0)).arc_to(point(5.0, 5.0), 0.0, false, true, point(10.0, 0.0));
        assert!(matches!(arc.commands().last(), Some(PathCommand::CurveTo(_, _, e)) if (e.x - 10.0).abs() < 1e-4));
        let t = Transform::rotate(90.0).then(&Transform::translate(5.0, 0.0));
        let q = t.apply(point(1.0, 0.0));
        assert!((q.x - 5.0).abs() < 1e-5 && (q.y - 1.0).abs() < 1e-5, "{q:?}");
        let back = t.invert().unwrap().apply(q);
        assert!((back.x - 1.0).abs() < 1e-5 && back.y.abs() < 1e-5);
    }
}
