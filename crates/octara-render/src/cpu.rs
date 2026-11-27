//! The CPU reference renderer: a straightforward, deterministic evaluation of the display list,
//! pixel by pixel. It is the oracle the GPU renderer is tested against, and how agents and tests
//! take screenshots without a GPU.

use crate::glyphs::{GlyphCache, GlyphKey, SUBPIXEL_STEPS};
use crate::image_mips::{IMAGE_ATLAS_EDGE, ImageLevel, full_lod, opaque, packed_lod};
use crate::paths::{self, PathCache};
use crate::shape::{ClipStack, blend, blend_premultiplied, coverage, inner_rect, sd_round_rect, shadow_alpha};
use color::{LinearSrgb, PremulColor, Rgba8};
use octara_model::capture::{PaintFallback, PaintOutcome, PaintReceipt, PaintSample, PaintedPicture};
use octara_model::geometry::{Rect, Vec2};
use octara_model::paint::{DisplayList, GlyphRun, ImagePaint, Item, PathPaint, Picture, Quad, Shadow};
use octara_model::theme::{Color, Gradient};
use std::alloc::Layout;
use std::borrow::Cow;
use std::sync::LazyLock;

/// Renders display lists to RGBA images; keeps rasterized glyphs and paths between frames.
#[derive(Default)]
pub struct CpuRenderer {
    glyphs: GlyphCache,
    paths: PathCache,
}

struct Target {
    width: u32,
    height: u32,
    /// Premultiplied linear-light RGBA with sRGB primaries.
    pixels: Vec<[f32; 4]>,
    clips: ClipStack,
    /// Pixels under open groups. A transition at full opacity passes through to its parent.
    layers: Vec<Option<(Vec<[f32; 4]>, f32)>>,
}

impl Target {
    fn clip_coverage(&self, px: f32, py: f32) -> f32 {
        self.clips.coverage(px, py)
    }
    /// Device-pixel bounds grown by one pixel, limited to the target and clip bounds.
    fn bounds(&self, x: f32, y: f32, w: f32, h: f32) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0);
        if let Some(c) = self.clips.current().bounds {
            if c.is_empty() {
                return (0, 0, 0, 0);
            }
            x0 = x0.max(c.x - 1.0);
            y0 = y0.max(c.y - 1.0);
            x1 = x1.min(c.right() + 1.0);
            y1 = y1.min(c.bottom() + 1.0);
        }
        let clampx = |v: f32| v.clamp(0.0, self.width as f32) as u32;
        let clampy = |v: f32| v.clamp(0.0, self.height as f32) as u32;
        (clampx(x0.floor()), clampy(y0.floor()), clampx(x1.ceil()), clampy(y1.ceil()))
    }
}

fn rgb(c: Color) -> ([f32; 3], f32) {
    let [r, g, b, a] = c.convert::<LinearSrgb>().components;
    ([r, g, b], a)
}

fn linear_texel(rgba: &[u8]) -> [f32; 4] {
    static LINEAR: LazyLock<[f32; 256]> =
        LazyLock::new(|| std::array::from_fn(|i| Color::from_rgb8(i as u8, 0, 0).convert::<LinearSrgb>().components[0]));
    [LINEAR[rgba[0] as usize], LINEAR[rgba[1] as usize], LINEAR[rgba[2] as usize], rgba[3] as f32 / 255.0]
}

struct LinearGradient {
    center: [f32; 2],
    line: [f32; 2],
    stops: [f32; 2],
    colors: [[f32; 4]; 2],
}

impl LinearGradient {
    fn new(gradient: Gradient, x: f32, y: f32, w: f32, h: f32) -> Self {
        let (dx, dy) = gradient.angle.to_radians().sin_cos();
        let dy = -dy;
        let len = (w * dx).abs() + (h * dy).abs();
        Self {
            center: [x + w * 0.5, y + h * 0.5],
            line: if len > 0.0 { [dx / len, dy / len] } else { [0.0; 2] },
            stops: [gradient.from.1, gradient.to.1],
            colors: [gradient.from.0, gradient.to.0].map(|color| color.convert::<LinearSrgb>().premultiply().components),
        }
    }

    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let [cx, cy] = self.center;
        let [dx, dy] = self.line;
        let t = (x - cx) * dx + (y - cy) * dy + 0.5;
        let [from, to] = self.stops;
        let k = if to > from {
            ((t - from) / (to - from)).clamp(0.0, 1.0)
        } else if t < from {
            0.0
        } else {
            1.0
        };
        let [a, b] = self.colors;
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * k)
    }
}

impl CpuRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many rasterized path masks are cached.
    pub fn cached_paths(&self) -> usize {
        self.paths.len()
    }

    /// Renders at `scale` device pixels per logical pixel, with a one-pixel minimum in each axis.
    ///
    /// # Panics
    /// If the scale or dimensions are invalid, or the target exceeds addressable memory.
    /// [`Self::paint`] returns an error for these inputs instead.
    pub fn render(&mut self, dl: &DisplayList, scale: f32) -> Picture {
        self.try_render(dl, scale).expect("CPU rendering failed").picture
    }

    fn try_render(&mut self, dl: &DisplayList, scale: f32) -> Result<PaintedPicture, String> {
        let (width, height) = crate::pixels::dimensions((dl.size.width, dl.size.height), scale)?;
        let count = target_len(width, height).ok_or("CPU target exceeds addressable memory")?;
        let (bg, bga) = rgb(dl.background);
        let base = [bg[0] * bga, bg[1] * bga, bg[2] * bga, bga];
        let mut t = Target { width, height, pixels: vec![base; count], clips: ClipStack::default(), layers: Vec::new() };
        let mut effects = Vec::new();
        for placed in dl.placed_items() {
            let item = if matches!(placed.item, Item::Glyphs(_)) { Cow::Borrowed(placed.item) } else { placed.resolve() };
            let item = item.as_ref();
            match item {
                Item::Shadow(s) => shadow(&mut t, s, scale),
                Item::Quad(q) => quad(&mut t, q, scale),
                Item::Custom(p) => {
                    quad(&mut t, &p.fallback, scale);
                    effects.push(PaintReceipt {
                        source: p.fallback.source,
                        outcome: PaintOutcome::Fallback { reason: PaintFallback::ReferenceRenderer },
                    });
                }
                Item::Image(i) => image(&mut t, i, scale),
                Item::Glyphs(g) => self.glyph_run(&mut t, g, scale, placed.offset, placed.fade),
                Item::Path(p) => self.path(&mut t, p, scale),
                // An underlay view the platform shows: a hole down to transparent.
                Item::Native { rect, radius, cut: true, .. } => cutout(&mut t, *rect, *radius, scale),
                // Otherwise its placeholder or snapshot, painted before, shows.
                Item::Native { .. } => {}
                Item::SkipTransition { source, .. } => {
                    effects.push(PaintReceipt {
                        source: *source,
                        outcome: PaintOutcome::Fallback { reason: PaintFallback::UnsupportedRegion },
                    });
                }
                Item::PushClip { rect, radius, .. } => t.clips.push(*rect, *radius, scale),
                Item::PopClip => t.clips.pop(),
                Item::PushLayer { opacity, .. } | Item::PushTransition(octara_model::paint::TransitionPaint { opacity, .. }) => {
                    if let Item::PushTransition(paint) = item {
                        if paint.progress < 1.0 {
                            effects.push(PaintReceipt {
                                source: paint.source,
                                outcome: PaintOutcome::Fallback { reason: PaintFallback::ReferenceRenderer },
                            });
                        }
                        if *opacity == 1.0 {
                            t.layers.push(None);
                            continue;
                        }
                    }
                    let below = std::mem::replace(&mut t.pixels, vec![[0.0; 4]; count]);
                    t.layers.push(Some((below, *opacity)));
                }
                Item::PopLayer => {
                    let Some(Some((below, opacity))) = t.layers.pop() else { continue };
                    let layer = std::mem::replace(&mut t.pixels, below);
                    for (dst, src) in t.pixels.iter_mut().zip(layer) {
                        blend_premultiplied(dst, src.map(|channel| channel * opacity));
                    }
                }
            }
        }
        self.paths.end_frame();
        Ok(PaintedPicture { picture: from_premultiplied(width, height, &t.pixels), effects })
    }

    fn path(&mut self, t: &mut Target, p: &PathPaint, scale: f32) {
        let window = paths::window((t.width, t.height), t.clips.current().bounds.map(|c| (c.x, c.y, c.w, c.h)));
        let Some(at) = paths::place(p, scale, window) else { return };
        let mask = self.paths.get(p, &at);
        let (color, alpha) = rgb(p.color);
        let b = p.bounds;
        let gradient = p.gradient.map(|g| LinearGradient::new(g, b.x * scale, b.y * scale, b.w * scale, b.h * scale));
        for row in 0..mask.height {
            let py = at.y + row as i32;
            for col in 0..mask.width {
                let px = at.x + col as i32;
                let cov = mask.data[row as usize * mask.width as usize + col as usize];
                if cov == 0 || px < 0 || py < 0 || px >= t.width as i32 || py >= t.height as i32 {
                    continue;
                }
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                let k = cov as f32 / 255.0 * t.clip_coverage(cx, cy);
                let dst = &mut t.pixels[py as usize * t.width as usize + px as usize];
                if let Some(g) = &gradient {
                    blend_premultiplied(dst, g.sample(cx, cy).map(|v| v * k));
                } else {
                    blend(dst, color, alpha * k);
                }
            }
        }
    }

    fn glyph_run(&mut self, t: &mut Target, run: &GlyphRun, scale: f32, offset: Vec2, fade: f32) {
        let size = run.size * scale;
        for (g, color) in run.placed_colored_at(offset, fade) {
            let (color, alpha) = rgb(color);
            if alpha <= 0.0 {
                continue;
            }
            let (gx, gy) = (g.x * scale, g.y * scale);
            let (ix, fx) = (gx.floor(), gx - gx.floor());
            let subpixel = ((fx * SUBPIXEL_STEPS).floor() as u8).min(SUBPIXEL_STEPS as u8 - 1);
            let iy = gy.round();
            let key = GlyphKey::new(run, g.id, size, subpixel);
            let Some(img) = self.glyphs.rasterize(&run.font, key) else { continue };
            let (ox, oy) = (ix as i32 + img.left, iy as i32 - img.top);
            let (x0, y0, x1, y1) = t.bounds(ox as f32, oy as f32, img.width as f32, img.height as f32);
            for py in y0..y1 {
                for px in x0..x1 {
                    let (sx, sy) = (px as i32 - ox, py as i32 - oy);
                    if sx < 0 || sy < 0 || sx >= img.width as i32 || sy >= img.height as i32 {
                        continue;
                    }
                    let clip = t.clip_coverage(px as f32 + 0.5, py as f32 + 0.5);
                    let i = sy as usize * img.width as usize + sx as usize;
                    let dst = &mut t.pixels[py as usize * t.width as usize + px as usize];
                    if img.color {
                        let [r, g, b, a] = linear_texel(&img.data[i * 4..i * 4 + 4]);
                        blend(dst, [r, g, b], a * alpha * clip);
                    } else {
                        let a = img.data[i] as f32 / 255.0;
                        blend(dst, color, a * alpha * clip);
                    }
                }
            }
        }
    }
}

fn shadow(t: &mut Target, s: &Shadow, scale: f32) {
    let Rect { x, y, w, h } = s.rect;
    let (x, y, w, h, r) = (x * scale, y * scale, w * scale, h * scale, s.rect.corner_radii(s.radius).map(|radius| radius * scale));
    let sigma = s.blur * 0.5 * scale;
    let reach = 3.0 * sigma;
    let (c, a) = rgb(s.color);
    let (x0, y0, x1, y1) = t.bounds(x - reach, y - reach, w + 2.0 * reach, h + 2.0 * reach);
    for py in y0..y1 {
        for px in x0..x1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let alpha = shadow_alpha((cx, cy), (x, y, w, h), r, sigma);
            if alpha <= 0.0 {
                continue;
            }
            let clip = t.clip_coverage(cx, cy);
            blend(&mut t.pixels[py as usize * t.width as usize + px as usize], c, a * alpha * clip);
        }
    }
}

fn image(t: &mut Target, i: &ImagePaint, scale: f32) {
    let f = i.image.frame();
    if f.width == 0 || f.height == 0 || i.rect.is_empty() || i.opacity <= 0.0 {
        return;
    }
    let s = |r: Rect| (r.x * scale, r.y * scale, r.w * scale, r.h * scale);
    let (x, y, w, h) = s(i.rect);
    let b = s(i.bounds);
    let radius = i.bounds.corner_radii(i.radius).map(|radius| radius * scale);
    // Pixels in both the drawn rect and the clipping box.
    let x0 = x.max(b.0);
    let y0 = y.max(b.1);
    let x1 = (x + w).min(b.0 + b.2);
    let y1 = (y + h).min(b.1 + b.3);
    let (px0, py0, px1, py1) = t.bounds(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
    if px0 >= px1 || py0 >= py1 {
        return;
    }
    let size = [f.width, f.height];
    let packed_lod = packed_lod(size, IMAGE_ATLAS_EDGE);
    let packed = packed_lod.is_some();
    let max_lod = packed_lod.unwrap_or_else(|| full_lod(size));
    let lod = (f.width as f32 / w).max(f.height as f32 / h).log2().clamp(0.0, f32::from(max_lod));
    let lo = lod.floor() as u8;
    let mut level = ImageLevel::base(size, &f.rgba, opaque(&f.rgba));
    for _ in 0..lo {
        level = level.downsample(packed);
    }
    let high = (lod.fract() != 0.0).then(|| level.downsample(packed));
    let level_extent = |level: &ImageLevel, lod| {
        if packed { size.map(|axis| axis as f32 / (1_u32 << lod) as f32) } else { level.size.map(|axis| axis as f32) }
    };
    let extent = level_extent(&level, lo);
    let high = high.as_ref().map(|high| (high, level_extent(high, lo + 1)));
    for py in py0..py1 {
        for px in px0..px1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let edge = coverage(sd_round_rect(cx, cy, x, y, w, h, [0.0; 4]));
            let inside = coverage(sd_round_rect(cx, cy, b.0, b.1, b.2, b.3, radius));
            let k = i.opacity * edge * inside * t.clip_coverage(cx, cy);
            if k <= 0.0 {
                continue;
            }
            let uv = [((cx - x) / w.max(0.000001)).clamp(0.0, 1.0), ((cy - y) / h.max(0.000001)).clamp(0.0, 1.0)];
            let mut c = level.sample(uv, extent);
            if let Some((high, extent)) = high {
                for (low, high) in c.iter_mut().zip(high.sample(uv, extent)) {
                    *low += (high - *low) * lod.fract();
                }
            }
            blend_premultiplied(&mut t.pixels[py as usize * t.width as usize + px as usize], c.map(|v| v * k));
        }
    }
}

/// Clears `rect` (rounded by `radius`, inside the clip) toward transparent by its coverage: the
/// cutout an underlay view shows through (destination-out).
fn cutout(t: &mut Target, rect: Rect, radius: [f32; 4], scale: f32) {
    let (x, y, w, h, r) =
        (rect.x * scale, rect.y * scale, rect.w * scale, rect.h * scale, rect.corner_radii(radius).map(|radius| radius * scale));
    let x0 = (x.floor().max(0.0)) as u32;
    let y0 = (y.floor().max(0.0)) as u32;
    let x1 = ((x + w).ceil().max(0.0) as u32).min(t.width);
    let y1 = ((y + h).ceil().max(0.0) as u32).min(t.height);
    for py in y0..y1 {
        for px in x0..x1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let k = coverage(sd_round_rect(cx, cy, x, y, w, h, r)) * t.clip_coverage(cx, cy);
            if k > 0.0 {
                let p = &mut t.pixels[py as usize * t.width as usize + px as usize];
                *p = p.map(|v| v * (1.0 - k));
            }
        }
    }
}

fn quad(t: &mut Target, q: &Quad, scale: f32) {
    let Rect { x, y, w, h } = q.rect;
    let (x, y, w, h, r) = (x * scale, y * scale, w * scale, h * scale, q.rect.corner_radii(q.radius).map(|radius| radius * scale));
    let border = q.border.map(|b| b * scale);
    let has_border = border.iter().any(|b| *b > 0.0);
    let inner = inner_rect(x, y, w, h, r, border);
    let fill = q.fill.map(rgb);
    let gradient = q.gradient.map(|g| LinearGradient::new(g, x, y, w, h));
    let (bc, ba) = rgb(q.border_color);
    let (x0, y0, x1, y1) = t.bounds(x, y, w, h);
    for py in y0..y1 {
        for px in x0..x1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let outer = coverage(sd_round_rect(cx, cy, x, y, w, h, r));
            if outer <= 0.0 {
                continue;
            }
            let clip = t.clip_coverage(cx, cy);
            let dst = &mut t.pixels[py as usize * t.width as usize + px as usize];
            if let Some(g) = &gradient {
                blend_premultiplied(dst, g.sample(cx, cy).map(|v| v * outer * clip));
            } else if let Some((c, a)) = fill {
                blend(dst, c, a * outer * clip);
            }
            if has_border {
                let (rect, radii) = inner;
                let inside = if rect.is_empty() { 0.0 } else { coverage(sd_round_rect(cx, cy, rect.x, rect.y, rect.w, rect.h, radii)) };
                blend(dst, bc, ba * (outer - inside).max(0.0) * clip);
            }
        }
    }
}

fn target_len(width: u32, height: u32) -> Option<usize> {
    let count = crate::pixels::count(width, height)?;
    Layout::array::<[f32; 4]>(count).ok()?;
    Some(count)
}

fn from_premultiplied(width: u32, height: u32, pixels: &[[f32; 4]]) -> Picture {
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for &pixel in pixels {
        let Rgba8 { r, g, b, a } = PremulColor::<LinearSrgb>::new(pixel).un_premultiply().to_rgba8();
        rgba.extend_from_slice(&if a == 0 { [0; 4] } else { [r, g, b, a] });
    }
    Picture::new(width, height, rgba).expect("checked capture dimensions")
}

impl CpuRenderer {
    pub fn renderer(&self) -> &'static str {
        "reference"
    }
    pub fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String> {
        self.try_render(list, scale).map(|painted| painted.picture)
    }
    pub fn paint_with_receipts(&mut self, list: &DisplayList, scale: f32, _sample: Option<PaintSample>) -> Result<PaintedPicture, String> {
        self.try_render(list, scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_allocation_checks_the_full_size_without_allocating() {
        assert_eq!(target_len(7, 5), Some(35));
        assert_eq!(target_len(u32::MAX, u32::MAX), None);
        if usize::BITS == 64 {
            assert_eq!(target_len(65536, 65536), Some(usize::try_from(1_u64 << 32).unwrap()));
        } else {
            assert_eq!(target_len(65536, 65536), None);
        }
    }

    #[test]
    fn invalid_capture_keeps_the_cpu_renderer_usable() {
        let mut cpu = CpuRenderer::new();
        let mut list = DisplayList { size: octara_model::geometry::Size::new(1.0, 1.0), background: Color::WHITE, parts: vec![] };
        assert!(cpu.paint(&list, f32::NAN).is_err());
        list.size = octara_model::geometry::Size::new(4_000_000_000.0, 4_000_000_000.0);
        assert!(cpu.paint(&list, 1.0).is_err());
        list.size = octara_model::geometry::Size::new(0.0, 0.0);
        let picture = cpu.paint(&list, 1.0).unwrap();
        assert_eq!((picture.width(), picture.height()), (1, 1));
        assert_eq!(picture.rgba(), [255; 4]);
    }
}
