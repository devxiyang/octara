//! The CPU reference renderer: a straightforward, deterministic evaluation of the display list,
//! pixel by pixel. It is the oracle the GPU renderer is tested against, and how agents and tests
//! take screenshots without a GPU.

use crate::Bitmap;
use crate::glyphs::{GlyphCache, SUBPIXEL_STEPS};
use crate::paths::{self, PathCache};
use crate::shape::{blend, blend_premultiplied, coverage, image_color, inner_rect, sd_round_rect, shadow_alpha};
use octara::kernel::Color;
use octara::paint::{DisplayList, GlyphRun, ImagePaint, Item, PathPaint, Quad, Rect, Shadow};

/// A clip in device pixels.
#[derive(Clone, Copy)]
struct Clip {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
}

impl Clip {
    fn coverage(&self, px: f32, py: f32) -> f32 {
        coverage(sd_round_rect(px, py, self.x, self.y, self.w, self.h, self.r))
    }
}

/// Renders display lists to RGBA images; keeps rasterized glyphs and paths between frames.
#[derive(Default)]
pub struct CpuRenderer {
    glyphs: GlyphCache,
    paths: PathCache,
}

struct Target {
    width: u32,
    height: u32,
    /// Premultiplied RGBA, sRGB-encoded (blending happens in that space, as browsers and Rune).
    pixels: Vec<[f32; 4]>,
    clips: Vec<Clip>,
    /// Pixels under open layers, with each layer's opacity.
    layers: Vec<(Vec<[f32; 4]>, f32)>,
}

impl Target {
    /// Clips nest by intersection; only the innermost one's corners are rounded (the GPU renderer
    /// carries one clip per instance, and both renderers follow the same rule).
    fn clip_coverage(&self, px: f32, py: f32) -> f32 {
        self.clips.last().map_or(1.0, |c| c.coverage(px, py))
    }
    /// Device-pixel bounds of `x..x+w`, `y..y+h` grown by one pixel, limited to the target and the
    /// innermost clip.
    fn bounds(&self, x: f32, y: f32, w: f32, h: f32) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0);
        if let Some(c) = self.clips.last() {
            x0 = x0.max(c.x - 1.0);
            y0 = y0.max(c.y - 1.0);
            x1 = x1.min(c.x + c.w + 1.0);
            y1 = y1.min(c.y + c.h + 1.0);
        }
        let clampx = |v: f32| v.clamp(0.0, self.width as f32) as u32;
        let clampy = |v: f32| v.clamp(0.0, self.height as f32) as u32;
        (clampx(x0.floor()), clampy(y0.floor()), clampx(x1.ceil()), clampy(y1.ceil()))
    }
}

fn rgb(c: Color) -> ([f32; 3], f32) {
    let [r, g, b, a] = c.components;
    ([r, g, b], a)
}

impl CpuRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many rasterized path masks are cached.
    pub fn cached_paths(&self) -> usize {
        self.paths.len()
    }

    /// Renders at `scale` device pixels per logical pixel.
    pub fn render(&mut self, dl: &DisplayList, scale: f32) -> Bitmap {
        let width = (dl.size.0 * scale).ceil().max(1.0) as u32;
        let height = (dl.size.1 * scale).ceil().max(1.0) as u32;
        let (bg, bga) = rgb(dl.background);
        let base = [bg[0] * bga, bg[1] * bga, bg[2] * bga, bga];
        let mut t = Target { width, height, pixels: vec![base; (width * height) as usize], clips: Vec::new(), layers: Vec::new() };
        for item in &dl.items {
            match item {
                Item::Shadow(s) => shadow(&mut t, s, scale),
                Item::Quad(q) => quad(&mut t, q, scale),
                Item::Image(i) => image(&mut t, i, scale),
                Item::Glyphs(g) => self.glyph_run(&mut t, g, scale),
                Item::Path(p) => self.path(&mut t, p, scale),
                // An underlay view the platform shows: a hole down to transparent.
                Item::Native { rect, radius, cut: true, .. } => cutout(&mut t, *rect, *radius, scale),
                // Otherwise its placeholder or snapshot, painted before, shows.
                Item::Native { .. } => {}
                Item::PushClip { rect, radius, .. } => {
                    let c = crate::shape::push_clip(t.clips.last().map(|c| (c.x, c.y, c.w, c.h)), *rect, *radius, scale);
                    t.clips.push(Clip { x: c.0, y: c.1, w: c.2, h: c.3, r: c.4 });
                }
                Item::PopClip => {
                    t.clips.pop();
                }
                Item::PushLayer { opacity, .. } => {
                    let below = std::mem::replace(&mut t.pixels, vec![[0.0; 4]; (width * height) as usize]);
                    t.layers.push((below, *opacity));
                }
                Item::PopLayer => {
                    let Some((below, opacity)) = t.layers.pop() else { continue };
                    let layer = std::mem::replace(&mut t.pixels, below);
                    // The layer is 8-bit, as the GPU renderer's layer texture.
                    for (dst, src) in t.pixels.iter_mut().zip(layer) {
                        let q = src.map(|v| (v * 255.0).round() / 255.0 * opacity);
                        blend_premultiplied(dst, q);
                    }
                }
            }
        }
        self.paths.end_frame();
        Bitmap::from_premultiplied(width, height, &t.pixels)
    }

    fn path(&mut self, t: &mut Target, p: &PathPaint, scale: f32) {
        let window = paths::window((t.width, t.height), t.clips.last().map(|c| (c.x, c.y, c.w, c.h)));
        let Some(at) = paths::place(p, scale, window) else { return };
        let mask = self.paths.get(p, &at);
        let (color, alpha) = rgb(p.color);
        let b = p.bounds;
        let gbox = (b.x * scale, b.y * scale, b.w * scale, b.h * scale);
        for row in 0..mask.height {
            let py = at.y + row as i32;
            for col in 0..mask.width {
                let px = at.x + col as i32;
                let cov = mask.data[(row * mask.width + col) as usize];
                if cov == 0 || px < 0 || py < 0 || px >= t.width as i32 || py >= t.height as i32 {
                    continue;
                }
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                let k = cov as f32 / 255.0 * t.clip_coverage(cx, cy);
                let dst = &mut t.pixels[(py as u32 * t.width + px as u32) as usize];
                if let Some(g) = &p.gradient {
                    let [r, gr, bl, a] = g.at(cx, cy, gbox.0, gbox.1, gbox.2, gbox.3);
                    blend(dst, [r, gr, bl], a * k);
                } else {
                    blend(dst, color, alpha * k);
                }
            }
        }
    }

    fn glyph_run(&mut self, t: &mut Target, run: &GlyphRun, scale: f32) {
        let (color, alpha) = rgb(run.color);
        let size = run.size * scale;
        for g in &run.glyphs {
            let (gx, gy) = (g.x * scale, g.y * scale);
            let (ix, fx) = (gx.floor(), gx - gx.floor());
            let subpixel = ((fx * SUBPIXEL_STEPS).floor() as u8).min(SUBPIXEL_STEPS as u8 - 1);
            let iy = gy.round();
            let Some(img) = self.glyphs.get(&run.font, g.id, size, subpixel, run.embolden, &run.coords) else { continue };
            let (ox, oy) = (ix as i32 + img.left, iy as i32 - img.top);
            let (x0, y0, x1, y1) = t.bounds(ox as f32, oy as f32, img.width as f32, img.height as f32);
            for py in y0..y1 {
                for px in x0..x1 {
                    let (sx, sy) = (px as i32 - ox, py as i32 - oy);
                    if sx < 0 || sy < 0 || sx >= img.width as i32 || sy >= img.height as i32 {
                        continue;
                    }
                    let clip = t.clip_coverage(px as f32 + 0.5, py as f32 + 0.5);
                    let i = (sy as u32 * img.width + sx as u32) as usize;
                    let dst = &mut t.pixels[(py * t.width + px) as usize];
                    if img.color {
                        let p = &img.data[i * 4..i * 4 + 4];
                        let a = p[3] as f32 / 255.0;
                        blend(dst, [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0], a * alpha * clip);
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
    let (x, y, w, h, r) = (x * scale, y * scale, w * scale, h * scale, s.radius * scale);
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
            blend(&mut t.pixels[(py * t.width + px) as usize], c, a * alpha * clip);
        }
    }
}

fn image(t: &mut Target, i: &ImagePaint, scale: f32) {
    let f = i.image.frame();
    if f.width == 0 || f.height == 0 {
        return;
    }
    let s = |r: Rect| (r.x * scale, r.y * scale, r.w * scale, r.h * scale);
    let rect = s(i.rect);
    let b = s(i.bounds);
    let bounds = (b.0, b.1, b.2, b.3, i.radius * scale);
    let texel = |x: u32, y: u32| {
        let k = ((y * f.width + x) * 4) as usize;
        let p = &f.rgba[k..k + 4];
        [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]
    };
    // Pixels in both the drawn rect and the clipping box.
    let x0 = rect.0.max(b.0);
    let y0 = rect.1.max(b.1);
    let x1 = (rect.0 + rect.2).min(b.0 + b.2);
    let y1 = (rect.1 + rect.3).min(b.1 + b.3);
    let (px0, py0, px1, py1) = t.bounds(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0));
    for py in py0..py1 {
        for px in px0..px1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let c = image_color((cx, cy), rect, bounds, (f.width, f.height), texel);
            let k = i.opacity * t.clip_coverage(cx, cy);
            if c[3] * k <= 0.0 {
                continue;
            }
            blend_premultiplied(&mut t.pixels[(py * t.width + px) as usize], c.map(|v| v * k));
        }
    }
}

/// Clears `rect` (rounded by `radius`, inside the clip) toward transparent by its coverage: the
/// cutout an underlay view shows through (destination-out).
fn cutout(t: &mut Target, rect: Rect, radius: f32, scale: f32) {
    let (x, y, w, h, r) = (rect.x * scale, rect.y * scale, rect.w * scale, rect.h * scale, radius * scale);
    let x0 = (x.floor().max(0.0)) as u32;
    let y0 = (y.floor().max(0.0)) as u32;
    let x1 = ((x + w).ceil().max(0.0) as u32).min(t.width);
    let y1 = ((y + h).ceil().max(0.0) as u32).min(t.height);
    for py in y0..y1 {
        for px in x0..x1 {
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let k = coverage(sd_round_rect(cx, cy, x, y, w, h, r)) * t.clip_coverage(cx, cy);
            if k > 0.0 {
                let p = &mut t.pixels[(py * t.width + px) as usize];
                *p = p.map(|v| v * (1.0 - k));
            }
        }
    }
}

fn quad(t: &mut Target, q: &Quad, scale: f32) {
    let Rect { x, y, w, h } = q.rect;
    let (x, y, w, h, r) = (x * scale, y * scale, w * scale, h * scale, q.radius * scale);
    let border = q.border.map(|b| b * scale);
    let has_border = border.iter().any(|b| *b > 0.0);
    let inner = inner_rect(x, y, w, h, r, border);
    let fill = q.fill.map(rgb);
    let gradient = q.gradient;
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
            let dst = &mut t.pixels[(py * t.width + px) as usize];
            if let Some(g) = &gradient {
                let [r, gr, b, a] = g.at(cx, cy, x, y, w, h);
                blend(dst, [r, gr, b], a * outer * clip);
            } else if let Some((c, a)) = fill {
                blend(dst, c, a * outer * clip);
            }
            if has_border {
                let (ix, iy, iw, ih, ir) = inner;
                let inside = if iw > 0.0 && ih > 0.0 { coverage(sd_round_rect(cx, cy, ix, iy, iw, ih, ir)) } else { 0.0 };
                blend(dst, bc, ba * (outer - inside).max(0.0) * clip);
            }
        }
    }
}

impl octara::capture::Painter for CpuRenderer {
    fn renderer(&self) -> &'static str {
        "reference"
    }
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<octara::capture::Picture, String> {
        let b = self.render(list, scale);
        Ok(octara::capture::Picture { width: b.width, height: b.height, rgba: b.rgba })
    }
}
