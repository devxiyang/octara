//! Resolves a device-free frame plan into GPU resources available to the encoder.

use super::atlas::{Bindings, PageId, Region, SceneTextures, Tile};
use super::batch::{Draw, Inst, Pass, straight};
use super::custom::{self, CanvasMode};
use super::glyph_pipeline::GlyphInst;
use super::outputs::RegionImage;
use super::pipeline::color_texture;
use super::plan::{
    DrawSample, EncodedPlan, FramePlan, GlyphRequest, PaintRequests, PassBounds, ResourcePlan, TargetInfo, TransitionRequest,
    backdrop_inputs, bound_passes, plan_frame_cached, transition_composite_bounds,
};
use super::recording::FrameRecording;
use super::resolve::AttachmentInfo;
use super::upload::FrameData;
use super::{FrameInput, GpuRenderer, RecordError, WORKING_FORMAT};
use octara_model::capture::{PaintFallback, PaintOutcome, PaintReceipt, PaintSample};
use octara_model::geometry::Rect;
use octara_model::paint::TransitionPaint;
use rustc_hash::FxHashMap;

pub(super) struct PlannedTransition {
    pub(super) request: TransitionRequest,
    pub(super) composite: Option<TransitionComposite>,
    /// The raw child bounds to draw when recording the author commands fails.
    pub(super) fallback: Option<u32>,
}

pub(super) struct TransitionComposite {
    pub(super) old: RegionImage,
    pub(super) sample: PaintSample,
    pub(super) bounds: PassBounds,
}

/// Prepared resources and pending uploads for the ordered frame plan.
pub(super) struct PreparedFrame {
    pub(super) plan: EncodedPlan,
    pub(super) background: wgpu::Color,
    pub(super) bounds: Vec<PassBounds>,
    pub(super) layer_views: Vec<wgpu::TextureView>,
    pub(super) empty: wgpu::TextureView,
    pub(super) atlas_binds: FxHashMap<Bindings, wgpu::BindGroup>,
    pub(super) glyph_binds: FxHashMap<PageId, wgpu::BindGroup>,
    pub(super) layer_binds: Vec<wgpu::BindGroup>,
    pub(super) canvas_binds: Vec<wgpu::BindGroup>,
    pub(super) transitions: FxHashMap<usize, PlannedTransition>,
    pub(super) data: FrameData,
}

impl GpuRenderer {
    pub(super) async fn prepare_resources(
        &mut self,
        passes: &mut [Pass],
        resources: ResourcePlan,
        textures: &mut SceneTextures,
        root_format: wgpu::TextureFormat,
        receipts: &mut [PaintReceipt],
    ) -> Result<(), RecordError> {
        enum SampleBinding {
            Image(Region),
            Glyph(GlyphInst, PageId),
            Path(Vec<Tile>),
        }

        let mut samples = Vec::with_capacity(resources.samples.len());
        for sample in resources.samples {
            let binding = match sample {
                DrawSample::Image(image) => SampleBinding::Image(textures.image_slot(&self.device, &image).await?),
                DrawSample::Glyph(GlyphRequest { key, image, pos, color, clip }) => {
                    let color_glyph = image.color;
                    let region = textures.glyph_slot(&self.device, key, image).await?;
                    SampleBinding::Glyph(GlyphInst::new(pos, region, color, clip, color_glyph), region.page)
                }
                DrawSample::Path(paint, placement) => SampleBinding::Path(textures.path_tiles(&self.device, &paint, &placement).await?),
            };
            samples.push(binding);
        }
        for (index, pass) in passes.iter_mut().enumerate() {
            if !pass.batches.iter().any(|batch| matches!(batch.draw, Draw::Sample(_) | Draw::GlyphSample | Draw::RequestedMaterial(_))) {
                continue;
            }
            let format = if index == 0 { root_format } else { WORKING_FORMAT };
            let batches = std::mem::take(&mut pass.batches);
            let instances = std::mem::take(&mut pass.insts);
            for batch in batches {
                let draw = batch.draw;
                if draw == Draw::GlyphSample {
                    for sample in &samples[batch.instances] {
                        let SampleBinding::Glyph(instance, page) = *sample else { unreachable!("a prepared glyph sample") };
                        pass.push_glyph(instance, page);
                    }
                    continue;
                }
                for &instance in &instances[batch.instances] {
                    match draw {
                        Draw::Sample(index) => match &samples[index] {
                            SampleBinding::Glyph(..) => unreachable!("glyphs have a dedicated draw"),
                            SampleBinding::Image(region) => {
                                let [x, y] = region.origin;
                                let [w, h] = region.size;
                                let instance = Inst { uv: [x as f32, y as f32, w as f32, h as f32], ..instance };
                                pass.push(instance, Draw::Standard(Bindings::image(region.page)));
                            }
                            SampleBinding::Path(tiles) => {
                                for tile in tiles {
                                    let [x, y] = tile.region.origin;
                                    let [dx, dy] = tile.offset;
                                    let [w, h] = tile.region.size;
                                    let instance = Inst {
                                        rect: [instance.rect[0] + dx as f32, instance.rect[1] + dy as f32, w as f32, h as f32],
                                        uv: [x as f32, y as f32, 0.0, 0.0],
                                        ..instance
                                    };
                                    pass.push(instance, Draw::Standard(Bindings::mask(tile.region.page)));
                                }
                            }
                        },
                        Draw::RequestedMaterial(index) => {
                            let request = &resources.materials[index];
                            let receipt = &mut receipts[request.receipt];
                            if self
                                .programs
                                .prepare_material(&self.device, &self.pipelines.pipeline_layout, format, request.material, receipt.source)
                                .await
                            {
                                receipt.outcome = PaintOutcome::Rendered;
                                pass.push(instance, Draw::Material(request.material));
                            } else {
                                receipt.outcome = PaintOutcome::Fallback { reason: PaintFallback::PreparationFailed };
                                pass.push(request.fallback, Draw::PLAIN);
                            }
                        }
                        _ => pass.push(instance, draw),
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) async fn prepare_frame(
        &mut self,
        input: FrameInput<'_>,
        output: AttachmentInfo,
        textures: &mut SceneTextures,
        recording: &mut FrameRecording<'_>,
    ) -> Result<PreparedFrame, RecordError> {
        let target =
            TargetInfo { scale: input.scale, max_texture_dimension: self.device.limits().max_texture_dimension_2d, ..output.requirements };
        let FramePlan { mut passes, requests, clips, resources, bounds, resolve_output, backdrop_inputs: backdrop_reads } =
            plan_frame_cached(input.list, target, input.sample, &mut textures.glyphs);

        let PaintRequests { canvases, backdrops, skipped_transitions, receipts } = requests;
        recording.receipts = receipts;
        if let Some(changes) = &mut recording.outputs {
            for (mount, revision) in skipped_transitions {
                changes.transitions.record(mount, revision, None);
            }
        }
        self.prepare_resources(&mut passes, resources, textures, output.format, &mut recording.receipts).await?;
        let canvas_views = self.prepare_canvases(&mut passes, canvases, input.scale, recording).await;
        let window = output.requirements.extent;
        let mut transitions = FxHashMap::default();
        let has_transitions = passes.iter().any(|pass| pass.transition.is_some());
        let bounds = if has_transitions {
            bound_passes(&mut passes, window, |index, request, bounds, visible| {
                let mut composite = None;
                if let Some(receipt) = request.receipt {
                    match self.plan_transition(&request.paint, bounds, visible, input.scale, recording) {
                        Ok(plan) => composite = Some(plan),
                        Err(reason) => recording.receipts[receipt].outcome = PaintOutcome::Fallback { reason },
                    }
                }
                let output = composite.as_ref().map_or(bounds, |plan| plan.bounds);
                transitions.insert(index, PlannedTransition { request: request.clone(), composite, fallback: None });
                output
            })
        } else {
            bounds
        };
        // Each instance buffer holds every pass's instances of its draw type.
        let mut all: Vec<Inst> = Vec::new();
        let mut glyphs = Vec::new();
        let mut instance_offsets = Vec::with_capacity(passes.len());
        for p in &passes {
            instance_offsets.push([all.len() as u32, glyphs.len() as u32]);
            all.extend_from_slice(&p.insts);
            glyphs.extend_from_slice(&p.glyphs);
            if !transitions.is_empty() {
                for batch in &p.batches {
                    if let Draw::Layer(layer) = batch.draw
                        && let Some(transition) = transitions.get_mut(&layer)
                        && transition.composite.as_ref().is_some_and(|plan| plan.bounds != bounds[layer])
                    {
                        transition.fallback = Some(all.len() as u32);
                        all.push(Inst { rect: bounds[layer].rect(), ..p.insts[batch.instances.start] });
                    }
                }
            }
        }
        let backdrop_fallback_offset = all.len() as u32;
        all.extend(backdrops.iter().map(|request| request.fallback));
        let (reads, resolve) =
            if has_transitions { backdrop_inputs(&backdrops, &bounds, target) } else { (backdrop_reads, resolve_output) };
        let plan = EncodedPlan::new(&passes, &instance_offsets, backdrops, backdrop_fallback_offset, reads, resolve);
        self.buffers.prepare_instances(&self.device, &all);
        self.buffers.prepare_glyphs(&self.device, &glyphs)?;
        let layer_count = passes.len() - 1;
        for format in std::iter::once(output.format).chain((layer_count > 0).then_some(WORKING_FORMAT)) {
            self.pipelines.prepare(&self.device, format);
            if !glyphs.is_empty() {
                self.glyph_pipeline.prepare(&self.device, format);
            }
        }
        self.layers.resize_with(layer_count, || None);
        let mut layer_views = Vec::with_capacity(layer_count);
        let can_publish = input.sample.is_some() && recording.outputs.is_some();
        for (index, (bounds, slot)) in bounds[1..].iter().zip(&mut self.layers).enumerate() {
            let immutable = transitions.get(&(index + 1)).is_some_and(|transition| {
                transition.composite.is_some()
                    || (can_publish && transition.request.paint.opacity > 0.0 && !bounds.is_empty() && RegionImage::can_retain(bounds.size))
            });
            let texture = if immutable {
                // This target can become accepted history. Its allocation is never used as a
                // working target while an accepted frame or a run can still read from it.
                *slot = None;
                color_texture(&self.device, "octara layer", WORKING_FORMAT, bounds.size.map(|axis| axis.max(1)))
            } else {
                let [width, height] = bounds.allocation(window);
                if slot.as_ref().is_none_or(|t| t.width() != width || t.height() != height) {
                    *slot = Some(color_texture(&self.device, "octara layer", WORKING_FORMAT, [width, height]));
                }
                slot.as_ref().expect("a working layer").clone()
            };
            layer_views.push(texture.create_view(&Default::default()));
        }
        let uniforms = self.buffers.prepare_projections(&self.device, &bounds, window, &layer_views);
        let clips = clips.into_nodes();
        self.buffers.prepare_clips(&self.device, &clips);
        let empty = self.no_layer.create_view(&Default::default());
        let bind_with = |view| self.buffers.bind_texture(&self.device, &self.pipelines.layout, [&empty; 2], view);
        let mut atlas_binds = FxHashMap::default();
        let mut glyph_binds = FxHashMap::default();
        atlas_binds.insert(Bindings::EMPTY, bind_with(&empty));
        for batch in &plan.batches {
            match batch.draw {
                Draw::Standard(bindings) => {
                    atlas_binds.entry(bindings).or_insert_with(|| {
                        let views = bindings.0.map(|page| page.map_or(&empty, |page| textures.atlases.view(page)));
                        self.buffers.bind_texture(&self.device, &self.pipelines.layout, views, &empty)
                    });
                }
                Draw::Glyph(page) => {
                    glyph_binds.entry(page).or_insert_with(|| {
                        let view = textures.atlases.view(page);
                        self.buffers.bind_glyphs(&self.device, &self.glyph_pipeline.layout, view, &glyphs)
                    });
                }
                _ => {}
            }
        }
        let layer_binds: Vec<wgpu::BindGroup> = layer_views.iter().map(bind_with).collect();
        let canvas_binds = canvas_views.iter().map(bind_with).collect();
        let [r, g, b, a] = straight(input.list.background);
        let background = wgpu::Color { r: (r * a) as f64, g: (g * a) as f64, b: (b * a) as f64, a: a as f64 };
        let data = FrameData { instances: all, glyphs, uniforms, clips };
        Ok(PreparedFrame {
            plan,
            background,
            bounds,
            layer_views,
            empty,
            atlas_binds,
            glyph_binds,
            layer_binds,
            canvas_binds,
            transitions,
            data,
        })
    }

    fn plan_transition(
        &self,
        paint: &TransitionPaint,
        current: PassBounds,
        visible: Rect,
        scale: f32,
        recording: &mut FrameRecording<'_>,
    ) -> Result<TransitionComposite, PaintFallback> {
        if paint.program.downcast_ref::<custom::Canvas>().is_none_or(|canvas| canvas.mode != CanvasMode::Transition) {
            return Err(PaintFallback::UnsupportedProgram);
        }
        if !RegionImage::can_retain(current.size) || !scale.is_finite() || scale <= 0.0 {
            return Err(PaintFallback::InvalidSize);
        }
        let old = self.presentation_history.transition(
            &paint.mount.downgrade(),
            &paint.run.downgrade(),
            paint.revision,
            recording.outputs.as_mut().map(|changes| &mut changes.transitions),
        );
        let (old, sample) = old.ok_or(PaintFallback::HistoryUnavailable)?;
        let previous = PassBounds { origin: old.origin, size: old.size() };
        let bounds = transition_composite_bounds(current, visible, previous, old.scale, scale);
        if bounds.is_empty() || !RegionImage::can_retain(bounds.size) {
            return Err(PaintFallback::InvalidSize);
        }
        Ok(TransitionComposite { old, sample, bounds })
    }
}
