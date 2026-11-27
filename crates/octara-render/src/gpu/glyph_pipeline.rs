//! Compact packed glyph payload used by the array-backed glyph pipeline.

use super::atlas::Region;
use bytemuck::{Pod, Zeroable};

const COLOR_GLYPH: u32 = 1 << 31;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct GlyphInst {
    pos: [f32; 2],
    size: u32,
    atlas_origin: u32,
    color_rgba8: u32,
    layer_flags: u32,
}

const _: () = assert!(size_of::<GlyphInst>() == 24);

impl GlyphInst {
    pub(super) fn new(pos: [f32; 2], region: Region, color_rgba8: u32, color_glyph: bool) -> Self {
        let pack = |[x, y]: [u32; 2]| {
            debug_assert!(x <= u16::MAX.into() && y <= u16::MAX.into());
            x | (y << 16)
        };
        Self {
            pos,
            size: pack(region.size),
            atlas_origin: pack(region.origin),
            color_rgba8,
            layer_flags: u32::from(region.layer) | if color_glyph { COLOR_GLYPH } else { 0 },
        }
    }
}
