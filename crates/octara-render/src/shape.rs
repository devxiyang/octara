//! The geometry both renderers share. The GPU shaders implement exactly these formulas, and a
//! differential test keeps the two within a level or two per channel.

use octara_primitives::paint::Rect;

/// Signed distance from `(px, py)` to a rounded rectangle at `(x, y, w, h)` with corner radius
/// `r`: negative inside, positive outside.
pub fn sd_round_rect(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (hw, hh) = (w * 0.5, h * 0.5);
    let r = r.min(hw).min(hh).max(0.0);
    let qx = (px - (x + hw)).abs() - (hw - r);
    let qy = (py - (y + hh)).abs() - (hh - r);
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - r
}

/// Coverage of a pixel whose center is `d` from an edge: a one-pixel ramp.
pub fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// The inner edge of a border: the rect shrunk by the widths (top, right, bottom, left), with
/// the radius reduced by the widest side.
pub fn inner_rect(x: f32, y: f32, w: f32, h: f32, r: f32, b: [f32; 4]) -> (f32, f32, f32, f32, f32) {
    let (t, rt, bt, l) = (b[0], b[1], b[2], b[3]);
    let widest = t.max(rt).max(bt).max(l);
    (x + l, y + t, (w - l - rt).max(0.0), (h - t - bt).max(0.0), (r - widest).max(0.0))
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ClipState {
    pub(crate) bounds: Option<Rect>,
    /// One-based index into the clip nodes. Zero uses only the rectangular bounds.
    pub(crate) head: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[cfg_attr(feature = "gpu", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub(crate) struct ClipNode {
    rect: [f32; 4],
    radius: f32,
    parent: u32,
    _pad: [u32; 2],
}

#[derive(Default)]
pub(crate) struct ClipStack {
    stack: Vec<ClipState>,
    nodes: Vec<ClipNode>,
}

impl ClipStack {
    pub(crate) fn current(&self) -> ClipState {
        self.stack.last().copied().unwrap_or_default()
    }

    pub(crate) fn push(&mut self, rect: Rect, radius: f32, scale: f32) {
        let previous = self.current();
        let rect = Rect::new(rect.x * scale, rect.y * scale, rect.w * scale, rect.h * scale);
        let radius = (radius * scale).max(0.0);
        let bounds = previous.bounds.map_or(rect, |outer| outer.intersect(&rect));
        let mut head = previous.head;
        if !bounds.is_empty() && (head != 0 || radius > 0.0) {
            if head == 0
                && let Some(outer) = previous.bounds
            {
                head = self.add_node(outer, 0.0, 0);
            }
            head = self.add_node(rect, radius, head);
        }
        self.stack.push(ClipState { bounds: Some(bounds), head });
    }

    pub(crate) fn pop(&mut self) {
        self.stack.pop();
    }

    fn add_node(&mut self, rect: Rect, radius: f32, parent: u32) -> u32 {
        self.nodes.push(ClipNode { rect: [rect.x, rect.y, rect.w, rect.h], radius, parent, _pad: [0; 2] });
        u32::try_from(self.nodes.len()).expect("clip node index")
    }

    #[cfg(feature = "gpu")]
    pub(crate) fn nodes(&self) -> &[ClipNode] {
        &self.nodes
    }

    pub(crate) fn coverage(&self, x: f32, y: f32) -> f32 {
        let ClipState { bounds: Some(bounds), mut head } = self.current() else {
            return 1.0;
        };
        if bounds.is_empty() {
            return 0.0;
        }
        if head == 0 {
            return coverage(sd_round_rect(x, y, bounds.x, bounds.y, bounds.w, bounds.h, 0.0));
        }
        // Intersect the original shapes. The most restrictive edge determines the coverage,
        // including when two clips share the same antialiased edge.
        let mut alpha = 1.0_f32;
        while head != 0 && alpha > 0.0 {
            let node = self.nodes[head as usize - 1];
            let [left, top, width, height] = node.rect;
            alpha = alpha.min(coverage(sd_round_rect(x, y, left, top, width, height, node.radius)));
            head = node.parent;
        }
        alpha
    }
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2.0 * sigma * sigma)).exp() / ((2.0 * std::f32::consts::PI).sqrt() * sigma)
}

/// The error function (Abramowitz–Stegun, as GPUI's shader).
fn erf(v: f32) -> f32 {
    let (s, a) = (v.signum(), v.abs());
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    s - s / (r2 * r2)
}

/// A rounded rect's row at height `y` (from its center), blurred along x at `x`.
fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half: (f32, f32)) -> f32 {
    let delta = (half.1 - corner - y.abs()).min(0.0);
    let curved = half.0 - corner + (corner * corner - delta * delta).max(0.0).sqrt();
    let k = 0.5f32.sqrt() / sigma;
    let (lo, hi) = (0.5 + 0.5 * erf((x - curved) * k), 0.5 + 0.5 * erf((x + curved) * k));
    hi - lo
}

/// How much of a box shadow covers the pixel at `(px, py)`: the rounded rect `(x, y, w, h)` with
/// radius `r` blurred by a Gaussian of standard deviation `sigma` (Evan Wallace's closed form
/// along x, four samples along y, as GPUI and Rune). `sigma` 0 is a sharp edge.
pub fn shadow_alpha((px, py): (f32, f32), (x, y, w, h): (f32, f32, f32, f32), r: f32, sigma: f32) -> f32 {
    if sigma <= 0.0 {
        return coverage(sd_round_rect(px, py, x, y, w, h, r));
    }
    let half = (w * 0.5, h * 0.5);
    let (cx, cy) = (px - (x + half.0), py - (y + half.1));
    let corner = r.min(half.0).min(half.1).max(0.0);
    let (low, high) = (cy - half.1, cy + half.1);
    let start = (-3.0 * sigma).clamp(low, high);
    let end = (3.0 * sigma).clamp(low, high);
    let step = (end - start) / 4.0;
    let mut s = start + step * 0.5;
    let mut alpha = 0.0;
    for _ in 0..4 {
        alpha += blur_along_x(cx, cy - s, sigma, corner, half) * gaussian(s, sigma) * step;
        s += step;
    }
    alpha.clamp(0.0, 1.0)
}

/// An image's premultiplied color at the pixel centered on `p`: bilinear between its texels
/// (premultiplied, so transparent texels do not darken edges), clamped at its edges, with
/// antialiased edges where it ends inside the clip. `rect` is where the image is drawn,
/// `bounds` (rounded by `radius`) clips it. `texel(x, y)` reads a straight RGBA texel (0–1).
pub fn image_color(
    p: (f32, f32),
    rect: (f32, f32, f32, f32),
    bounds: (f32, f32, f32, f32, f32),
    size: (u32, u32),
    texel: impl Fn(u32, u32) -> [f32; 4],
) -> [f32; 4] {
    let (x, y, w, h) = rect;
    let edge = coverage(sd_round_rect(p.0, p.1, x, y, w, h, 0.0));
    let clip = coverage(sd_round_rect(p.0, p.1, bounds.0, bounds.1, bounds.2, bounds.3, bounds.4));
    let cov = edge * clip;
    if cov <= 0.0 || w <= 0.0 || h <= 0.0 {
        return [0.0; 4];
    }
    let (iw, ih) = size;
    let u = (p.0 - x) / w * iw as f32 - 0.5;
    let v = (p.1 - y) / h * ih as f32 - 0.5;
    let (fu, fv) = (u.floor(), v.floor());
    let (tx, ty) = (u - fu, v - fv);
    let cx = |i: f32| i.clamp(0.0, iw as f32 - 1.0) as u32;
    let cy = |i: f32| i.clamp(0.0, ih as f32 - 1.0) as u32;
    let pre = |t: [f32; 4]| [t[0] * t[3], t[1] * t[3], t[2] * t[3], t[3]];
    let (a, b) = (pre(texel(cx(fu), cy(fv))), pre(texel(cx(fu + 1.0), cy(fv))));
    let (c, d) = (pre(texel(cx(fu), cy(fv + 1.0))), pre(texel(cx(fu + 1.0), cy(fv + 1.0))));
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bottom = c[i] + (d[i] - c[i]) * tx;
        (top + (bottom - top) * ty) * cov
    })
}

/// Source-over blend of a premultiplied color onto a premultiplied pixel.
pub fn blend_premultiplied(dst: &mut [f32; 4], c: [f32; 4]) {
    let k = 1.0 - c[3];
    for i in 0..4 {
        dst[i] = c[i] + dst[i] * k;
    }
}

/// Source-over blend of a straight color with `alpha` onto a premultiplied pixel.
pub fn blend(dst: &mut [f32; 4], rgb: [f32; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let k = 1.0 - alpha;
    dst[0] = rgb[0] * alpha + dst[0] * k;
    dst[1] = rgb[1] * alpha + dst[1] * k;
    dst[2] = rgb[2] * alpha + dst[2] * k;
    dst[3] = alpha + dst[3] * k;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangular_ancestor_preserves_the_round_clip_center() {
        let mut clips = ClipStack::default();
        clips.push(Rect::new(8.0, 0.0, 8.0, 16.0), 0.0, 1.0);
        clips.push(Rect::new(0.0, 0.0, 16.0, 16.0), 8.0, 1.0);

        assert_eq!(clips.coverage(8.5, 1.5), 1.0);
        assert_eq!(clips.coverage(7.5, 1.5), 0.0);
        assert_eq!(clips.coverage(15.5, 0.5), 0.0);

        clips.pop();
        assert_eq!(clips.coverage(15.5, 0.5), 1.0);
    }

    #[test]
    fn distances() {
        // A 10×10 square at the origin with radius 2.
        assert_eq!(sd_round_rect(5.0, 5.0, 0.0, 0.0, 10.0, 10.0, 2.0), -5.0);
        assert_eq!(sd_round_rect(12.0, 5.0, 0.0, 0.0, 10.0, 10.0, 2.0), 2.0);
        // The corner is rounded: (0, 0) lies outside the arc centered at (2, 2).
        let d = sd_round_rect(0.0, 0.0, 0.0, 0.0, 10.0, 10.0, 2.0);
        assert!((d - (8f32.sqrt() - 2.0)).abs() < 1e-6);
        assert_eq!((coverage(-1.0), coverage(0.0), coverage(1.0)), (1.0, 0.5, 0.0));
    }

    #[test]
    fn shadows() {
        // A 100×100 square blurred with σ 4: solid deep inside, half at the edge, nothing far out.
        let a = |px: f32, py: f32| shadow_alpha((px, py), (0.0, 0.0, 100.0, 100.0), 0.0, 4.0);
        assert!((a(50.0, 50.0) - 1.0).abs() < 0.01);
        assert!((a(100.0, 50.0) - 0.5).abs() < 0.02);
        assert!(a(120.0, 50.0) < 0.001);
        assert!(a(96.0, 50.0) > a(100.0, 50.0) && a(100.0, 50.0) > a(104.0, 50.0));
    }
}
