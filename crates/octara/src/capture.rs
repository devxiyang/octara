//! Captures: an agent's picture of the app, native views included, with a report of what the
//! picture can be trusted for ([design](../../../docs/design/capture.md)).
//!
//! The `capture` request of the [inspector protocol](crate::inspect::Request) settles the app,
//! asks the platform window for each shown view's snapshot, and paints
//! the frame with the snapshots in place through a [`Painter`]: the window's GPU renderer in a
//! shell, the reference renderer headless (`octara-render` implements both). The reply is a
//! [`Capture`] report.

use crate::native::NativeId;
use crate::paint::DisplayList;
use serde::{Deserialize, Serialize};

/// A painted frame: RGBA8, straight alpha, sRGB.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}×{})", self.width, self.height)
    }
}

impl Picture {
    /// Draws `top` (the same size, straight alpha) over this picture.
    pub fn over(&mut self, top: &Picture) {
        if (top.width, top.height) != (self.width, self.height) {
            return;
        }
        for (d, s) in self.rgba.as_chunks_mut::<4>().0.iter_mut().zip(top.rgba.as_chunks::<4>().0) {
            let a = s[3] as f32 / 255.0;
            if a == 0.0 {
                continue;
            }
            let da = d[3] as f32 / 255.0;
            let out = a + da * (1.0 - a);
            for i in 0..3 {
                let c = (s[i] as f32 * a + d[i] as f32 * da * (1.0 - a)) / out.max(f32::EPSILON);
                d[i] = c.round().clamp(0.0, 255.0) as u8;
            }
            d[3] = (out * 255.0).round() as u8;
        }
    }

    /// Writes it as an 8-bit sRGB PNG.
    pub fn save_png(&self, path: &str) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut w = enc.write_header().map_err(|e| format!("{path}: {e}"))?;
        w.write_image_data(&self.rgba).map_err(|e| format!("{path}: {e}"))?;
        w.finish().map_err(|e| format!("{path}: {e}"))
    }
}

/// What renders a capture's frame.
pub trait Painter {
    /// `gpu` or `reference`, for the report.
    fn renderer(&self) -> &'static str;
    /// The display list at `scale` device pixels per logical pixel.
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String>;
}

/// Where a capture's pixels come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pixels {
    /// Octara composed it: its frame with each native view's snapshot in place.
    #[default]
    Painted,
    /// The window server's picture of the window: what the user sees.
    Display,
}

/// What a native view shows in a capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shown {
    /// Its own snapshot, drawn in its place.
    Snapshot,
    /// The placeholder box: its host takes no snapshots (`reason` says why).
    Placeholder,
    /// The snapshot was asked for and did not come (`reason` says why).
    Failed,
    /// The view itself (the window server's picture).
    Live,
}

/// One native view in a capture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeShot {
    /// Its test id, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: String,
    /// `[x, y, width, height]` in logical pixels.
    pub rect: [f32; 4],
    pub shown: Shown,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What the snapshot cannot show (WebKit: hardware video and WebGL come out black).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub misses: Vec<String>,
}

/// A capture's report: the picture, and what it can be trusted for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Capture {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub pixels: Pixels,
    /// Why `display` was asked and `painted` given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// `gpu`, `reference` or `display`.
    pub renderer: String,
    /// Nothing was running when the picture was taken.
    pub settled: bool,
    /// What was still running when the wait ended: tasks, motion, native views loading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native: Vec<NativeShot>,
    /// The numbered elements drawn over a marked capture, in number order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
}

/// One numbered element of a marked capture: `#n` in the picture, `mark: n` in the snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    pub mark: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `[x, y, width, height]` in logical pixels.
    pub rect: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Capture {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a capture serializes")
    }
}

/// A capture in progress: the snapshots asked for, and what each host answered.
#[derive(Default)]
pub(crate) struct Capturing {
    /// Each shown view: `Ok(misses)` when its snapshot is coming, `Err(reason)` when not.
    pub asked: Vec<(NativeId, Result<Vec<String>, String>)>,
}

/// What marks are drawn in: interactive elements, and where a component starts.
const INTERACTIVE: crate::kernel::Color = crate::kernel::rgb(0xe11d48);
const COMPONENT: crate::kernel::Color = crate::kernel::rgb(0x7c3aed);

/// What a marked capture draws over the frame: each numbered element's outline and a badge
/// with its number, and the marks for the report. Elements outside the window are left out of
/// the picture, not of the numbering.
pub(crate) fn marks(k: &crate::kernel::Kernel) -> (Vec<crate::paint::Item>, Vec<Mark>) {
    let mut items = Vec::new();
    let mut report = Vec::new();
    let mut badges = Vec::new();
    for (i, id) in k.marks().into_iter().enumerate() {
        let n = i as u32 + 1;
        let one = k.inspect_one(id, Some(n));
        report.push(Mark { mark: n, id: one.id, role: one.role, name: one.name, rect: one.bounds, source: one.source });
        items.extend(outline(k, id, Some(n), false, &mut badges));
    }
    (items, report)
}

/// An element's outline (violet where a component starts, rose otherwise), tinted inside when
/// `fill`, with a badge holding `number` in its top-left corner (above it when the element is
/// too small), moved right past the `badges` already placed; nothing when it is outside the
/// window.
pub(crate) fn outline(
    k: &crate::kernel::Kernel,
    id: crate::kernel::NodeId,
    number: Option<u32>,
    fill: bool,
    badges: &mut Vec<crate::paint::Rect>,
) -> Vec<crate::paint::Item> {
    use crate::paint::{Item, PathPaint, Quad, Rect};
    use crate::path::{LineCap, LineJoin, Path, PathStyle, Stroke, Transform, point};
    let (vw, vh) = k.viewport();
    let (x, y, w, h) = k.shown_rect(id);
    if w <= 0.0 || h <= 0.0 || x >= vw || y >= vh || x + w <= 0.0 || y + h <= 0.0 {
        return Vec::new();
    }
    let color = if k.node_actions(id).is_empty() { COMPONENT } else { INTERACTIVE };
    let mut items = vec![Item::Quad(Quad {
        source: id,
        rect: Rect::new(x, y, w, h),
        radius: 2.0,
        fill: fill.then(|| color.with_alpha(0.12)),
        gradient: None,
        border: [1.0; 4],
        border_color: color.with_alpha(0.8),
    })];
    let Some(n) = number else { return items };
    // The badge: digits 5 × 9 with 2 between, 3 around.
    let digits = n.to_string();
    let bw = digits.len() as f32 * 7.0 + 4.0;
    let bh = 15.0;
    let inside = w >= bw + 2.0 && h >= bh + 2.0;
    let (mut bx, by) = if inside { (x + 1.0, y + 1.0) } else { (x, if y >= bh { y - bh } else { y.max(0.0) }) };
    bx = bx.clamp(0.0, (vw - bw).max(0.0));
    let mut badge = Rect::new(bx, by, bw, bh);
    for _ in 0..8 {
        let Some(hit) = badges.iter().find(|b| !b.intersect(&badge).is_empty()) else { break };
        badge.x = (hit.right() + 1.0).min((vw - bw).max(0.0));
    }
    badges.push(badge);
    let (bx, by) = (badge.x, badge.y);
    items.push(Item::Quad(Quad {
        source: id,
        rect: badge,
        radius: 3.0,
        fill: Some(color),
        gradient: None,
        border: [0.0; 4],
        border_color: color,
    }));
    let mut path = Path::new();
    for (j, c) in digits.bytes().enumerate() {
        let (ox, oy) = (bx + 3.5 + j as f32 * 7.0, by + 3.0);
        for &(a, b) in segments(c - b'0') {
            path = path.move_to(point(ox + a.0, oy + a.1)).line_to(point(ox + b.0, oy + b.1));
        }
    }
    items.push(Item::Path(PathPaint {
        source: id,
        path,
        transform: Transform::IDENTITY,
        style: PathStyle::Stroke(Stroke::new(1.5).cap(LineCap::Round).join(LineJoin::Round)),
        color: crate::kernel::rgb(0xffffff),
        gradient: None,
        bounds: badge,
    }));
    items
}

/// A stroke from one point to another.
type Segment = ((f32, f32), (f32, f32));

/// A digit's seven-segment strokes in a 5 × 9 box.
fn segments(d: u8) -> &'static [Segment] {
    const A: Segment = ((0.0, 0.0), (5.0, 0.0));
    const B: Segment = ((5.0, 0.0), (5.0, 4.5));
    const C: Segment = ((5.0, 4.5), (5.0, 9.0));
    const D: Segment = ((0.0, 9.0), (5.0, 9.0));
    const E: Segment = ((0.0, 4.5), (0.0, 9.0));
    const F: Segment = ((0.0, 0.0), (0.0, 4.5));
    const G: Segment = ((0.0, 4.5), (5.0, 4.5));
    match d {
        0 => &[A, B, C, D, E, F],
        1 => &[B, C],
        2 => &[A, B, G, E, D],
        3 => &[A, B, G, C, D],
        4 => &[F, G, B, C],
        5 => &[A, F, G, C, D],
        6 => &[A, F, G, E, C, D],
        7 => &[A, B, C],
        8 => &[A, B, C, D, E, F, G],
        _ => &[A, B, C, D, F, G],
    }
}
