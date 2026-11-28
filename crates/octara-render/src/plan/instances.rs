//! Storage-buffer payloads shared with the vertex-pulling shaders.

use super::batch::{NO_CLIP, Paint, Shape, straight};
use super::glyph_instance::GlyphInst;
use super::packing::Region;
use crate::shape::ClipNode;
use bytemuck::{Pod, Zeroable};
use octara_model::theme::{Color, Gradient};
use rustc_hash::FxHashMap;

pub const FRACTIONAL_CLIP: u32 = 1;
pub const SPATIAL_CLIP: u32 = 2;
pub const SPATIAL_SHIFT: u32 = 2;

/// Vertex-pulling geometry. One paint payload can back a core and up to four strips.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DrawInst {
    pub bounds: [f32; 4],
    pub clip: [f32; 4],
    pub core: [f32; 4],
    pub depth: f32,
    pub payload: u32,
    pub clip_head: u32,
    pub flags: u32,
}

const _: () = assert!(size_of::<DrawInst>() == 64);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlyphDraw {
    depth: f32,
    clip: u32,
}

const _: () = assert!(size_of::<GlyphDraw>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GlyphClip {
    rect: [f32; 4],
    head: u32,
    flags: u32,
    pad: [u32; 2],
}

const _: () = assert!(size_of::<GlyphClip>() == 32);

/// Quad payload with packed half-float corner radii and sRGBA8 colors.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct QuadInst {
    rect: [f32; 4],
    border: [f32; 4],
    radii: [u32; 2],
    fill: u32,
    border_color: u32,
    gradient_clip: u32,
    flags: u32,
    pad: [u32; 2],
}

const _: () = assert!(size_of::<QuadInst>() == 64);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GradientInst {
    start: [f32; 4],
    end: [f32; 4],
    border_color: [f32; 4],
    params: [f32; 4],
    radii: [f32; 4],
}

const _: () = assert!(size_of::<GradientInst>() == 80);

/// Shadow payload with linear-light color and Gaussian sigma in device pixels.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ShadowInst {
    rect: [f32; 4],
    radii: [f32; 4],
    color: [f32; 4],
    sigma: f32,
    pad: [f32; 3],
}

const _: () = assert!(size_of::<ShadowInst>() == 64);

/// Sampled image, path, layer, or canvas payload. Atlas coordinates are in texels.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct ImageInst {
    rect: [f32; 4],
    image_bounds: [f32; 4],
    radius: [f32; 4],
    uv: [f32; 4],
    fill: [f32; 4],
    layer: u32,
    max_lod: f32,
    kind: u32,
    gradient: u32,
}

const _: () = assert!(size_of::<ImageInst>() == 96);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MaterialInst {
    rect: [f32; 4],
    radius: [f32; 4],
    params: [f32; 4],
    opacity: f32,
    pad: [f32; 3],
}

/// Packed instance buffers populated by the scheduler.
#[derive(Default)]
pub struct Instances {
    pub draws: Vec<DrawInst>,
    pub quads: Vec<QuadInst>,
    pub shadows: Vec<ShadowInst>,
    pub images: Vec<ImageInst>,
    pub glyphs: Vec<GlyphInst>,
    pub glyph_draws: Vec<GlyphDraw>,
    pub glyph_clips: Vec<GlyphClip>,
    glyph_clip_index: FxHashMap<[u32; 6], u32>,
    pub materials: Vec<MaterialInst>,
    pub gradients: Vec<GradientInst>,
    gradient_index: FxHashMap<[u32; 20], u32>,
}

fn compact_radii(radius: [f32; 4]) -> [half::f16; 4] {
    radius.map(|r| half::f16::from_f32(r.min(f32::from(half::f16::MAX))))
}

fn pack_rgba8(color: Color) -> u32 {
    let c = color.to_rgba8();
    u32::from_le_bytes([c.r, c.g, c.b, c.a])
}

fn region_fields(region: Region) -> ([f32; 4], u32, f32) {
    let [x, y] = region.origin;
    let [w, h] = region.size;
    ([x as f32, y as f32, w as f32, h as f32], u32::from(region.layer), f32::from(region.max_lod))
}

fn append<T>(list: &mut Vec<T>, value: T) -> u32 {
    let index = list.len() as u32;
    list.push(value);
    index
}

impl Instances {
    pub fn needs_fragment_clips(&self) -> bool {
        self.draws.iter().any(|draw| draw.clip_head != 0 || draw.flags & FRACTIONAL_CLIP != 0)
            || self.glyph_clips.iter().any(|clip| clip.head != 0 || clip.flags & FRACTIONAL_CLIP != 0)
    }

    fn gradient(&mut self, base: Color, gradient: Option<Gradient>, border: Color, radii: [f32; 4]) -> u32 {
        let (start, end, params) =
            gradient.map_or((base, Color::TRANSPARENT, [0.0; 4]), |g| (g.from.0, g.to.0, [g.angle, g.from.1, g.to.1, 1.0]));
        let entry = GradientInst { start: straight(start), end: straight(end), border_color: straight(border), params, radii };
        let key: [u32; 20] = bytemuck::cast(entry);
        *self.gradient_index.entry(key).or_insert_with(|| {
            if self.gradients.is_empty() {
                self.gradients.push(GradientInst::zeroed());
            }
            append(&mut self.gradients, entry)
        })
    }

    pub(crate) fn push_draw(&mut self, draw: DrawInst) -> u32 {
        append(&mut self.draws, draw)
    }

    pub(crate) fn push_glyph(&mut self, glyph: GlyphInst, draw: DrawInst) -> u32 {
        let clip = if draw.clip == NO_CLIP && draw.clip_head == 0 && draw.flags == 0 {
            0
        } else {
            let [x, y, w, h] = draw.clip.map(f32::to_bits);
            let key = [x, y, w, h, draw.clip_head, draw.flags];
            *self.glyph_clip_index.entry(key).or_insert_with(|| {
                if self.glyph_clips.is_empty() {
                    self.glyph_clips.push(GlyphClip::zeroed());
                }
                append(&mut self.glyph_clips, GlyphClip { rect: draw.clip, head: draw.clip_head, flags: draw.flags, pad: [0; 2] })
            })
        };
        self.glyph_draws.push(GlyphDraw { depth: draw.depth, clip });
        append(&mut self.glyphs, glyph)
    }

    pub(crate) fn push_paint(&mut self, paint: Paint, clip_head: u32) -> u32 {
        let Paint { rect, radius, shape, .. } = paint;
        match shape {
            Shape::Quad { fill, gradient, border_widths, border_color } => {
                let base = fill.unwrap_or(Color::TRANSPARENT);
                let packed = compact_radii(radius);
                let exact_radii = packed.map(f32::from) != radius;
                let exact_color = |c: Color| {
                    c.components.iter().any(|v| !(0.0..=1.0).contains(v)) || f32::from(c.to_rgba8().a) / 255.0 != c.components[3]
                };
                let color_table = if gradient.is_some() || exact_radii || exact_color(base) || exact_color(border_color) {
                    self.gradient(base, gradient, border_color, if exact_radii { radius } else { [0.0; 4] })
                } else {
                    0
                };
                let [tl, tr, br, bl] = packed.map(|h| u32::from(h.to_bits()));
                append(
                    &mut self.quads,
                    QuadInst {
                        rect,
                        border: border_widths,
                        radii: [tl | tr << 16, br | bl << 16],
                        fill: pack_rgba8(base),
                        border_color: pack_rgba8(border_color),
                        gradient_clip: (color_table & 0xffff) | (clip_head.min(0xffff) << 16),
                        flags: u32::from(fill.is_some() || gradient.is_some())
                            | (u32::from(exact_radii) << 1)
                            | (color_table & 0xffff_0000),
                        pad: [0; 2],
                    },
                )
            }
            Shape::Shadow { color, sigma } => {
                append(&mut self.shadows, ShadowInst { rect, radii: radius, color: straight(color), sigma, pad: [0.0; 3] })
            }
            Shape::Material { opacity, params } => {
                append(&mut self.materials, MaterialInst { rect, radius, params, opacity, pad: [0.0; 3] })
            }
            Shape::Image { bounds, opacity, region: Some(region) } => {
                let (uv, layer, max_lod) = region_fields(region);
                append(
                    &mut self.images,
                    ImageInst {
                        rect,
                        image_bounds: bounds,
                        radius,
                        uv,
                        fill: [0.0, 0.0, 0.0, opacity],
                        layer,
                        max_lod,
                        kind: 0,
                        gradient: 0,
                    },
                )
            }
            Shape::Image { bounds, opacity, region: None } => append(
                &mut self.images,
                ImageInst {
                    rect,
                    image_bounds: bounds,
                    radius,
                    uv: [0.0, 0.0, 1.0, 1.0],
                    fill: [0.0, 0.0, 0.0, opacity],
                    kind: 4,
                    ..ImageInst::zeroed()
                },
            ),
            Shape::Path { color, gradient, bounds, region: Some(region) } => {
                let (uv, layer, max_lod) = region_fields(region);
                let gradient = gradient.map_or(0, |g| self.gradient(color, Some(g), Color::TRANSPARENT, [0.0; 4]));
                append(
                    &mut self.images,
                    ImageInst { rect, image_bounds: bounds, radius, uv, fill: straight(color), layer, max_lod, kind: 1, gradient },
                )
            }
            Shape::Layer { opacity } | Shape::Canvas { opacity } => append(
                &mut self.images,
                ImageInst {
                    rect,
                    radius,
                    fill: [0.0, 0.0, 0.0, opacity],
                    kind: if matches!(shape, Shape::Layer { .. }) { 2 } else { 3 },
                    ..ImageInst::zeroed()
                },
            ),
            _ => unreachable!("prepared paint has a resolved sample"),
        }
    }
}

pub(crate) fn payload_bytes(i: &Instances, uniforms: usize, clips: &[ClipNode]) -> u64 {
    (uniforms
        + size_of_val(i.draws.as_slice())
        + size_of_val(i.quads.as_slice())
        + size_of_val(i.shadows.as_slice())
        + size_of_val(i.images.as_slice())
        + size_of_val(i.glyphs.as_slice())
        + size_of_val(i.glyph_draws.as_slice())
        + size_of_val(i.glyph_clips.as_slice())
        + size_of_val(i.materials.as_slice())
        + size_of_val(i.gradients.as_slice())
        + size_of_val(clips)) as u64
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Globals {
    pub(crate) size: [f32; 2],
    pub(crate) origin: [f32; 2],
}
