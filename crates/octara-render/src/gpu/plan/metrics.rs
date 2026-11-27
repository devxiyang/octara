use super::{DrawSample, FramePlan, TargetInfo, plan_frame_cached};
use crate::glyphs::{GlyphCache, GlyphKey};
use crate::gpu::atlas::{Region, TextAtlas};
use crate::gpu::batch::{Draw, Paint, Shape, rect};
use crate::gpu::glyph_pipeline::GlyphInst;
use crate::gpu::instances::Instances;
use crate::gpu::pipeline::Globals;
use crate::gpu::schedule::schedule_pass;
use crate::gpu::upload::payload_bytes;
use octara_model::paint::DisplayList;
use rustc_hash::FxHashMap;

/// Structural work of a text frame on the device-free planner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextFrameMetrics {
    pub draw_calls: usize,
    pub uploaded_bytes: u64,
}

/// Runs the GPU frame scheduler without a device for text, geometry, and opacity layers.
/// Glyphs are retained across samples and must fit in one atlas layer.
pub struct TextFramePlanner {
    target: TargetInfo,
    glyphs: GlyphCache,
    atlas: TextAtlas,
    slots: FxHashMap<GlyphKey, Region>,
}

impl TextFramePlanner {
    pub fn new(target: TargetInfo) -> Self {
        Self {
            target,
            glyphs: GlyphCache::default(),
            atlas: TextAtlas::new(target.max_texture_dimension.max(1)),
            slots: FxHashMap::default(),
        }
    }

    /// Counts scheduled draws and logical buffer and texel uploads using the default GPU
    /// uniform alignment. Returns `None` for images, paths, custom effects, or atlas overflow.
    pub fn measure(&mut self, list: &DisplayList) -> Option<TextFrameMetrics> {
        let FramePlan { mut passes, requests, clips, resources, bounds, resolve_output, .. } =
            plan_frame_cached(list, self.target, None, &mut self.glyphs);
        if !resources.materials.is_empty() || !requests.receipts.is_empty() {
            return None;
        }

        let mut packing = None;
        let mut new_slots = FxHashMap::default();
        let mut texel_bytes = 0;
        let mut glyphs = Vec::with_capacity(resources.samples.len());
        for sample in resources.samples {
            let DrawSample::Glyph(glyph) = sample else { return None };
            let region = if let Some(&region) = self.slots.get(&glyph.key).or_else(|| new_slots.get(&glyph.key)) {
                region
            } else {
                let (region, bytes) = packing.get_or_insert_with(|| self.atlas.clone()).place(&glyph.image)?;
                new_slots.insert(glyph.key, region);
                texel_bytes += bytes;
                region
            };
            glyphs.push((GlyphInst::new(glyph.pos, region, glyph.color, glyph.image.color), region.array));
        }
        if let Some(packing) = packing {
            self.atlas = packing;
            self.slots.extend(new_slots);
        }

        let mut instances = Instances::default();
        let mut batches = Vec::new();
        for (pass, bound) in passes.iter_mut().zip(&bounds).rev() {
            for batch in &mut pass.batches {
                if let Draw::Sample(index) = batch.draw {
                    let (glyph, array) = glyphs[index];
                    batch.draw = Draw::Glyph(array);
                    for paint in &mut pass.paints[batch.instances.clone()] {
                        *paint = Paint { shape: Shape::Glyph(Some(glyph)), ..*paint };
                    }
                }
            }
            schedule_pass(pass, rect(bound.rect()), &clips, &mut instances, &mut batches);
        }
        let alignment = wgpu::Limits::default().min_uniform_buffer_offset_alignment as usize;
        let globals = size_of::<Globals>();
        let uniforms = (passes.len() - 1) * globals.next_multiple_of(alignment) + globals;
        let draw_calls = batches.len() + usize::from(resolve_output);
        let uploaded_bytes = texel_bytes + payload_bytes(&instances, uniforms, &clips.into_nodes());
        Some(TextFrameMetrics { draw_calls, uploaded_bytes })
    }
}
