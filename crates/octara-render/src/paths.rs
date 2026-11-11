//! Path rasterization, shared by both renderers: strokes become outlines (kurbo), and a path's
//! coverage is computed on the CPU by zeno into an 8-bit mask, which the reference renderer blends directly and the GPU renderer
//! samples from its mask atlas. Both therefore draw the same coverage.
//!
//! A mask is keyed by the path's content, the transform's linear part, the translation's
//! subpixel phase (quarter pixels, as glyphs), the style and the visible region: a path that
//! only moves by whole pixels reuses its mask, and a path larger than what is visible is
//! rasterized only where it shows.

use crate::glyphs::SUBPIXEL_STEPS;
use octara::paint::PathPaint;
use octara::path::{FillRule, LineCap, LineJoin, PathCommand, PathStyle, Transform};
use rustc_hash::{FxHashMap, FxHasher};
use std::hash::{Hash, Hasher};
use swash::zeno;

/// What identifies a mask.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MaskKey {
    path: u64,
    linear: [u32; 4],
    phase: (u8, u8),
    style: u64,
    /// The rasterized region relative to the transform's whole-pixel origin.
    region: [i32; 4],
}

/// Where a path's mask goes this frame.
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub key: MaskKey,
    /// The device pixel of the mask's first texel.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    transform: zeno::Transform,
}

/// A rasterized path: coverage, one byte per pixel, rows of `width`.
#[derive(Clone, Debug)]
pub struct PathMask {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

fn zpoint(v: octara::path::Point) -> zeno::Point {
    zeno::Point::new(v.x, v.y)
}

fn kpoint(v: octara::path::Point) -> kurbo::Point {
    kurbo::Point::new(v.x as f64, v.y as f64)
}

/// How much the transform stretches lengths at most (a bound on its largest singular value).
fn stretch(t: &Transform) -> f32 {
    (t.xx.abs() + t.yx.abs()).max(t.xy.abs() + t.yy.abs())
}

/// The path to fill and its rule: the path itself, or a stroke's outline (computed by kurbo in
/// the path's coordinates, so the transform shapes the pen as it does the path; zeno's own
/// stroker bevels every join sharper than a right angle).
fn outline(p: &PathPaint, t: &Transform) -> (Vec<zeno::Command>, zeno::Fill) {
    let cmds = p.path.commands();
    match &p.style {
        PathStyle::Fill(rule) => {
            let z = cmds
                .iter()
                .map(|c| match *c {
                    PathCommand::MoveTo(a) => zeno::Command::MoveTo(zpoint(a)),
                    PathCommand::LineTo(a) => zeno::Command::LineTo(zpoint(a)),
                    PathCommand::QuadTo(a, b) => zeno::Command::QuadTo(zpoint(a), zpoint(b)),
                    PathCommand::CurveTo(a, b, c) => zeno::Command::CurveTo(zpoint(a), zpoint(b), zpoint(c)),
                    PathCommand::Close => zeno::Command::Close,
                })
                .collect();
            (z, if *rule == FillRule::EvenOdd { zeno::Fill::EvenOdd } else { zeno::Fill::NonZero })
        }
        PathStyle::Stroke(s) => {
            let els = cmds.iter().map(|c| match *c {
                PathCommand::MoveTo(a) => kurbo::PathEl::MoveTo(kpoint(a)),
                PathCommand::LineTo(a) => kurbo::PathEl::LineTo(kpoint(a)),
                PathCommand::QuadTo(a, b) => kurbo::PathEl::QuadTo(kpoint(a), kpoint(b)),
                PathCommand::CurveTo(a, b, c) => kurbo::PathEl::CurveTo(kpoint(a), kpoint(b), kpoint(c)),
                PathCommand::Close => kurbo::PathEl::ClosePath,
            });
            let mut style = kurbo::Stroke::new(s.width as f64)
                .with_join(match s.join {
                    LineJoin::Miter => kurbo::Join::Miter,
                    LineJoin::Round => kurbo::Join::Round,
                    LineJoin::Bevel => kurbo::Join::Bevel,
                })
                .with_miter_limit(s.miter_limit as f64)
                .with_caps(match s.cap {
                    LineCap::Butt => kurbo::Cap::Butt,
                    LineCap::Round => kurbo::Cap::Round,
                    LineCap::Square => kurbo::Cap::Square,
                });
            if !s.dash.is_empty() {
                style = style.with_dashes(s.dash_offset as f64, s.dash.iter().map(|d| *d as f64));
            }
            // A tenth of a device pixel.
            let tolerance = 0.1 / stretch(t).max(1e-3) as f64;
            let out = kurbo::stroke(els, &style, &kurbo::StrokeOpts::default(), tolerance);
            let zp = |p: kurbo::Point| zeno::Point::new(p.x as f32, p.y as f32);
            let z = out
                .elements()
                .iter()
                .map(|e| match *e {
                    kurbo::PathEl::MoveTo(a) => zeno::Command::MoveTo(zp(a)),
                    kurbo::PathEl::LineTo(a) => zeno::Command::LineTo(zp(a)),
                    kurbo::PathEl::QuadTo(a, b) => zeno::Command::QuadTo(zp(a), zp(b)),
                    kurbo::PathEl::CurveTo(a, b, c) => zeno::Command::CurveTo(zp(a), zp(b), zp(c)),
                    kurbo::PathEl::ClosePath => zeno::Command::Close,
                })
                .collect();
            (z, zeno::Fill::NonZero)
        }
    }
}

/// The visible window in device pixels: the target, limited to the innermost clip's rect.
pub fn window(target: (u32, u32), clip: Option<(f32, f32, f32, f32)>) -> (f32, f32, f32, f32) {
    let (mut x0, mut y0, mut x1, mut y1) = (0.0f32, 0.0f32, target.0 as f32, target.1 as f32);
    if let Some((x, y, w, h)) = clip {
        x0 = x0.max(x.floor());
        y0 = y0.max(y.floor());
        x1 = x1.min((x + w).ceil());
        y1 = y1.min((y + h).ceil());
    }
    (x0, y0, x1, y1)
}

/// Where `p`'s mask goes at `scale` within `window` (device pixels `x0, y0, x1, y1`), or `None`
/// when none of it shows.
pub fn place(p: &PathPaint, scale: f32, window: (f32, f32, f32, f32)) -> Option<Placed> {
    if p.path.is_empty() {
        return None;
    }
    let t = p.transform.then(&Transform::scale(scale, scale));
    let (ix, iy) = (t.x.floor(), t.y.floor());
    let quantize = |f: f32| ((f * SUBPIXEL_STEPS).floor() as u8).min(SUBPIXEL_STEPS as u8 - 1);
    let phase = (quantize(t.x - ix), quantize(t.y - iy));
    let local = zeno::Transform::new(t.xx, t.xy, t.yx, t.yy, phase.0 as f32 / SUBPIXEL_STEPS, phase.1 as f32 / SUBPIXEL_STEPS);
    // The control points' box (curves stay inside it), grown by a stroke's reach; a pixel more
    // for antialiasing.
    let b = p.path.bounds()?;
    let corners = [(b.x, b.y), (b.right(), b.y), (b.x, b.bottom()), (b.right(), b.bottom())];
    let lt = Transform { x: local.x, y: local.y, ..t };
    let pts = corners.map(|(x, y)| lt.apply(octara::path::point(x, y)));
    let grow = match &p.style {
        PathStyle::Fill(_) => 0.0,
        PathStyle::Stroke(s) => s.reach() * stretch(&t),
    } + 1.0;
    let min_x = pts.iter().map(|p| p.x).fold(f32::INFINITY, f32::min) - grow;
    let min_y = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min) - grow;
    let max_x = pts.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max) + grow;
    let max_y = pts.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max) + grow;
    if !(min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite()) {
        return None;
    }
    let x0 = (min_x.floor() as i32).max((window.0 - ix).floor() as i32);
    let y0 = (min_y.floor() as i32).max((window.1 - iy).floor() as i32);
    let x1 = (max_x.ceil() as i32).min((window.2 - ix).ceil() as i32);
    let y1 = (max_y.ceil() as i32).min((window.3 - iy).ceil() as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut h = FxHasher::default();
    p.style.hash(&mut h);
    let key = MaskKey {
        path: p.path.content_hash(),
        linear: [t.xx.to_bits(), t.xy.to_bits(), t.yx.to_bits(), t.yy.to_bits()],
        phase,
        style: h.finish(),
        region: [x0, y0, x1, y1],
    };
    let (width, height) = ((x1 - x0) as u32, (y1 - y0) as u32);
    Some(Placed { key, x: ix as i32 + x0, y: iy as i32 + y0, width, height, transform: local })
}

/// Rasterizes the placed region of `p`.
pub fn rasterize(p: &PathPaint, at: &Placed) -> PathMask {
    let t = Transform { xx: at.transform.xx, xy: at.transform.xy, yx: at.transform.yx, yy: at.transform.yy, x: 0.0, y: 0.0 };
    let (cmds, fill) = outline(p, &t);
    let [x0, y0, ..] = at.key.region;
    let (data, _) = zeno::Mask::new(&cmds[..])
        .style(fill)
        .transform(Some(at.transform))
        .offset(zeno::Vector::new(-x0 as f32, -y0 as f32))
        .size(at.width, at.height)
        .render();
    PathMask { width: at.width, height: at.height, data }
}

/// How many frames a mask stays cached after its last use.
const KEEP_FRAMES: u64 = 120;

/// Rasterized masks, kept while they are drawn (the reference renderer's cache).
#[derive(Default)]
pub struct PathCache {
    masks: FxHashMap<MaskKey, (PathMask, u64)>,
    frame: u64,
}

impl PathCache {
    /// The mask of `p` placed at `at`, rasterized on first use.
    pub fn get(&mut self, p: &PathPaint, at: &Placed) -> &PathMask {
        let frame = self.frame;
        let entry = self.masks.entry(at.key).or_insert_with(|| (rasterize(p, at), frame));
        entry.1 = frame;
        &entry.0
    }

    /// Ends a frame: masks unused for a while are dropped (a canvas redrawn every frame does
    /// not accumulate them).
    pub fn end_frame(&mut self) {
        let frame = self.frame;
        self.masks.retain(|_, (_, used)| frame - *used < KEEP_FRAMES);
        self.frame += 1;
    }

    pub fn len(&self) -> usize {
        self.masks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.masks.is_empty()
    }
}
