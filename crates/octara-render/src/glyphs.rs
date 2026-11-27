//! Glyph rasterization with swash, cached per font, glyph, size and subpixel offset. Shared by
//! the CPU renderer and the GPU glyph atlas.

use octara_model::paint::{FontFace, GlyphRun};
use rustc_hash::FxHashMap;
use std::sync::Arc;
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};

/// Horizontal positions are quantized to this many steps per pixel (as GPUI).
pub const SUBPIXEL_STEPS: f32 = 4.0;

/// A rasterized glyph: an alpha mask or a color (RGBA) bitmap, placed relative to the pen.
#[derive(Clone, Debug)]
pub struct GlyphImage {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Color glyphs carry straight sRGB RGBA. Masks are linear coverage tinted by the run's color.
    pub color: bool,
    pub data: Arc<[u8]>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u32,
    size_bits: u32,
    subpixel: u8,
    embolden: bool,
    skew_bits: u32,
    coords: Vec<i16>,
}

impl GlyphKey {
    pub(crate) fn new(run: &GlyphRun, glyph: u32, size: f32, subpixel: u8) -> Self {
        Self {
            font: run.font.id.0,
            index: run.font.index,
            glyph,
            size_bits: size.to_bits(),
            subpixel,
            embolden: run.embolden,
            skew_bits: run.skew.to_bits(),
            coords: run.coords.clone(),
        }
    }
}

/// Rasterizes and caches glyphs.
#[derive(Default)]
pub struct GlyphCache {
    context: ScaleContext,
    cache: FxHashMap<GlyphKey, Option<GlyphImage>>,
}

impl GlyphCache {
    #[cfg(all(test, feature = "gpu", not(target_arch = "wasm32")))]
    pub(crate) fn insert_test_glyph(&mut self, key: GlyphKey, image: GlyphImage) {
        self.cache.insert(key, Some(image));
    }

    /// The image of `glyph` at `size` device pixels with a horizontal offset of `subpixel` /
    /// `SUBPIXEL_STEPS` of a pixel. `None` for blank glyphs (spaces).
    pub fn get(&mut self, font: &FontFace, glyph: u32, size: f32, subpixel: u8, embolden: bool, coords: &[i16]) -> Option<&GlyphImage> {
        let key = GlyphKey {
            font: font.id.0,
            index: font.index,
            glyph,
            size_bits: size.to_bits(),
            subpixel,
            embolden,
            skew_bits: 0,
            coords: coords.to_vec(),
        };
        self.rasterize(font, key)
    }

    pub(crate) fn rasterize(&mut self, font: &FontFace, key: GlyphKey) -> Option<&GlyphImage> {
        let context = &mut self.context;
        self.cache
            .entry(key)
            .or_insert_with_key(|key| {
                let font_ref = FontRef::from_index(&font.data, font.index as usize)?;
                let size = f32::from_bits(key.size_bits);
                let mut scaler = context.builder(font_ref).size(size).hint(false).normalized_coords(&key.coords).build();
                let mut render = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline]);
                render.format(Format::Alpha).offset(Vector::new(key.subpixel as f32 / SUBPIXEL_STEPS, 0.0));
                if key.embolden {
                    render.embolden(size / 24.0);
                }
                let skew = f32::from_bits(key.skew_bits);
                if skew.is_finite() && skew != 0.0 {
                    render.transform(Some(Transform::skew(Angle::from_degrees(skew), Angle::from_degrees(0.0))));
                }
                let mut image = render.render(&mut scaler, key.glyph as u16)?;
                if image.placement.width == 0 || image.placement.height == 0 {
                    return None;
                }
                if matches!(image.source, Source::ColorOutline(_)) {
                    // Swash composites color outlines into associated sRGB bytes. Recover the
                    // straight color before the atlas or the CPU renderer decodes sRGB.
                    for pixel in image.data.as_chunks_mut::<4>().0 {
                        let alpha = u32::from(pixel[3]);
                        for channel in &mut pixel[..3] {
                            let associated = u32::from(*channel) * 255 + alpha / 2;
                            *channel = associated.checked_div(alpha).unwrap_or(0).min(255) as u8;
                        }
                    }
                }
                Some(GlyphImage {
                    left: image.placement.left,
                    top: image.placement.top,
                    width: image.placement.width,
                    height: image.placement.height,
                    color: image.content == Content::Color,
                    data: image.data.into(),
                })
            })
            .as_ref()
    }
}
