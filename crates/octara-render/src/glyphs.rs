//! Glyph rasterization with swash, cached per font, glyph, size and subpixel offset. Shared by
//! the CPU renderer and (next) the GPU glyph atlas.

use octara::paint::FontData;
use rustc_hash::FxHashMap;
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};

/// Horizontal positions are quantized to this many steps per pixel (as GPUI).
pub const SUBPIXEL_STEPS: f32 = 4.0;

/// A rasterized glyph: an alpha mask or a color (RGBA) bitmap, placed relative to the pen.
#[derive(Clone, Debug)]
pub struct GlyphImage {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Color glyphs (emoji) carry their own RGBA; masks are tinted by the run's color.
    pub color: bool,
    pub data: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    font: u64,
    index: u32,
    glyph: u32,
    size_bits: u32,
    subpixel: u8,
    embolden: bool,
    coords: Vec<i16>,
}

/// Rasterizes and caches glyphs.
#[derive(Default)]
pub struct GlyphCache {
    context: ScaleContext,
    cache: FxHashMap<Key, Option<GlyphImage>>,
}

impl GlyphCache {
    /// The image of `glyph` at `size` device pixels with a horizontal offset of `subpixel` /
    /// `SUBPIXEL_STEPS` of a pixel. `None` for blank glyphs (spaces).
    pub fn get(&mut self, font: &FontData, glyph: u32, size: f32, subpixel: u8, embolden: bool, coords: &[i16]) -> Option<&GlyphImage> {
        let key =
            Key { font: font.data.id(), index: font.index, glyph, size_bits: size.to_bits(), subpixel, embolden, coords: coords.to_vec() };
        let context = &mut self.context;
        self.cache
            .entry(key)
            .or_insert_with(|| {
                let font_ref = FontRef::from_index(font.data.data(), font.index as usize)?;
                let mut scaler = context.builder(font_ref).size(size).hint(false).normalized_coords(coords).build();
                let mut render = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline]);
                render.format(Format::Alpha).offset(Vector::new(subpixel as f32 / SUBPIXEL_STEPS, 0.0));
                if embolden {
                    render.embolden(size / 24.0);
                }
                let image = render.render(&mut scaler, glyph as u16)?;
                if image.placement.width == 0 || image.placement.height == 0 {
                    return None;
                }
                Some(GlyphImage {
                    left: image.placement.left,
                    top: image.placement.top,
                    width: image.placement.width,
                    height: image.placement.height,
                    color: image.content == Content::Color,
                    data: image.data,
                })
            })
            .as_ref()
    }
}
