//! Resolves a device-free frame plan into GPU resources available to the encoder.

use super::atlas::{ArrayId, Region, SceneTextures, Tile};
use super::batch::{Draw, Paint, Pass, Shape, rect, straight};
use super::custom::{self, CanvasMode};
use super::glyph_pipeline::GlyphInst;
use super::instances::Instances;
use super::outputs::RegionImage;
use super::pipeline::{PipelineKind, color_texture, depth_texture};
use super::plan::{
    Attachment, DrawSample, EncodedPlan, FramePlan, GlyphRequest, PaintRequests, PassBounds, PlannedBackdrop, ResourcePlan, TargetPlan,
    TransitionRequest, backdrop_inputs, bound_passes, transition_composite_bounds,
};
use super::recording::FrameRecording;
use super::resolve::AttachmentInfo;
use super::schedule::{fallback_draw, schedule_pass};
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

/// Prepared resources for the ordered frame plan.
pub(super) struct PreparedFrame {
    pub(super) plan: EncodedPlan,
    pub(super) background: wgpu::Color,
    pub(super) bounds: Vec<PassBounds>,
    pub(super) layer_views: Vec<wgpu::TextureView>,
    pub(super) empty_array: wgpu::TextureView,
    pub(super) depth: Vec<wgpu::TextureView>,
    pub(super) fragment_clipping: bool,
    pub(super) quad_bind: wgpu::BindGroup,
    pub(super) shadow_bind: wgpu::BindGroup,
    pub(super) material_bind: wgpu::BindGroup,
    pub(super) image_binds: FxHashMap<ArrayId, wgpu::BindGroup>,
    pub(super) glyph_binds: FxHashMap<ArrayId, wgpu::BindGroup>,
    pub(super) layer_binds: Vec<wgpu::BindGroup>,
    pub(super) canvas_binds: Vec<wgpu::BindGroup>,
    pub(super) transitions: FxHashMap<usize, PlannedTransition>,
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
            Glyph(GlyphInst, ArrayId),
            Path(Vec<Tile>),
        }

        // Reserve the frame's cached working set before staging misses that may evict cold slots.
        for sample in &resources.samples {
            match sample {
                DrawSample::Image(image) => textures.pin_image(image)?,
                DrawSample::Glyph(glyph) => textures.pin_glyph(&glyph.key)?,
                DrawSample::Path(_, placement) => textures.pin_path(placement)?,
            }
        }
        let mut samples = Vec::with_capacity(resources.samples.len());
        for sample in resources.samples {
            let binding = match sample {
                DrawSample::Image(image) => SampleBinding::Image(textures.image_slot(&self.device, &image).await?),
                DrawSample::Glyph(GlyphRequest { key, image, pos, color }) => {
                    let color_glyph = image.color;
                    let region = textures.glyph_slot(&self.device, key, image).await?;
                    SampleBinding::Glyph(GlyphInst::new(pos, region, color, color_glyph), region.array)
                }
                DrawSample::Path(paint, placement) => SampleBinding::Path(textures.path_tiles(&self.device, &paint, &placement).await?),
            };
            samples.push(binding);
        }
        for (index, pass) in passes.iter_mut().enumerate() {
            if !pass.batches.iter().any(|batch| matches!(batch.draw, Draw::Sample(_) | Draw::RequestedMaterial(_))) {
                continue;
            }
            let format = if index == 0 { root_format } else { WORKING_FORMAT };
            let batches = std::mem::take(&mut pass.batches);
            let instances = std::mem::take(&mut pass.paints);
            for batch in batches {
                let draw = batch.draw;
                for &instance in &instances[batch.instances] {
                    match draw {
                        Draw::Sample(index) => match &samples[index] {
                            SampleBinding::Glyph(glyph, array) => {
                                pass.push(Paint { shape: Shape::Glyph(Some(*glyph)), ..instance }, Draw::Glyph(*array));
                            }
                            SampleBinding::Image(region) => {
                                let Shape::Image { bounds, opacity, .. } = instance.shape else { unreachable!("image paint") };
                                let shape = Shape::Image { bounds, opacity, region: Some(*region) };
                                pass.push(Paint { shape, ..instance }, Draw::Image(region.array));
                            }
                            SampleBinding::Path(tiles) => {
                                let Shape::Path { bounds, color, gradient, .. } = instance.shape else { unreachable!("path paint") };
                                for tile in tiles {
                                    let [dx, dy] = tile.offset;
                                    let [w, h] = tile.region.size;
                                    let instance = Paint {
                                        rect: [instance.rect[0] + dx as f32, instance.rect[1] + dy as f32, w as f32, h as f32],
                                        shape: Shape::Path { bounds, color, gradient, region: Some(tile.region) },
                                        ..instance
                                    };
                                    pass.push(instance, Draw::Image(tile.region.array));
                                }
                            }
                        },
                        Draw::RequestedMaterial(index) => {
                            let request = &resources.materials[index];
                            let receipt = &mut receipts[request.receipt];
                            if self
                                .programs
                                .prepare_material(&self.device, &self.pipelines.material.pipeline, format, request.material, receipt.source)
                                .await
                            {
                                receipt.outcome = PaintOutcome::Rendered;
                                pass.push(instance, Draw::Material(request.material));
                            } else {
                                receipt.outcome = PaintOutcome::Fallback { reason: PaintFallback::PreparationFailed };
                                pass.push(request.fallback, Draw::Quad);
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
        plan: FramePlan,
        spatials: Vec<[f32; 4]>,
    ) -> Result<(PreparedFrame, FrameData), RecordError> {
        let target = output.requirements;
        let FramePlan { mut passes, requests, clips, resources, bounds, resolve_output, backdrop_inputs: backdrop_reads } = plan;

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
        let (reads, resolve_output) =
            if has_transitions { backdrop_inputs(&backdrops, &bounds, target) } else { (backdrop_reads, resolve_output) };
        let mut backdrops: Vec<_> =
            backdrops.into_iter().zip(reads).map(|(request, input)| PlannedBackdrop { request, input, fallback: 0 }).collect();
        let mut instances = Instances::default();
        let mut batches = Vec::new();
        let mut targets = Vec::with_capacity(passes.len());
        for (index, pass) in passes.iter().enumerate().rev() {
            let target = rect(bounds[index].rect());
            let attachment = if index == 0 { Attachment::Output } else { Attachment::Layer(index) };
            let start = batches.len();
            let segments = schedule_pass(pass, target, &clips, &mut instances, &mut batches);
            let mut layers = pass
                .batches
                .iter()
                .filter_map(|batch| {
                    let Draw::Layer(layer) = batch.draw else { return None };
                    Some((layer, pass.paints[batch.instances.start]))
                })
                .collect::<FxHashMap<_, _>>();
            for batch in &batches[start..] {
                match batch.draw {
                    Draw::Backdrop(id) => {
                        let depth = instances.draws[batch.instances.start as usize].depth;
                        backdrops[id].fallback = fallback_draw(backdrops[id].request.fallback, target, depth, &clips, &mut instances);
                    }
                    Draw::Layer(layer) => {
                        if let Some(transition) = transitions.get_mut(&layer)
                            && transition.composite.as_ref().is_some_and(|plan| plan.bounds != bounds[layer])
                        {
                            let depth = instances.draws[batch.instances.start as usize].depth;
                            let paint = Paint { rect: bounds[layer].rect(), ..layers.remove(&layer).expect("layer paint") };
                            transition.fallback = Some(fallback_draw(paint, target, depth, &clips, &mut instances));
                        }
                    }
                    _ => {}
                }
            }
            targets.push(TargetPlan { attachment, segments });
        }
        let plan = EncodedPlan { targets, batches, backdrops, resolve_output };
        let fragment_clipping = instances.needs_fragment_clips();
        let layer_count = passes.len() - 1;
        for format in std::iter::once(output.format).chain((layer_count > 0).then_some(WORKING_FORMAT)) {
            self.pipelines.prepare(&self.device, format, fragment_clipping);
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
        let data = FrameData { instances, uniforms, clips: clips.into_nodes(), spatials };
        self.buffers.prepare(&self.device, &data)?;
        let extents: Vec<_> =
            std::iter::once(window).chain(layer_views.iter().map(|view| [view.texture().width(), view.texture().height()])).collect();
        self.depth.retain(|extent, _| extents.contains(extent));
        let depth = extents
            .into_iter()
            .map(|extent| self.depth.entry(extent).or_insert_with(|| depth_texture(&self.device, extent)).clone())
            .collect();
        let empty = self.no_layer.create_view(&Default::default());
        let empty_array = self
            .no_layer
            .create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let bind = |kind, atlas, layer| self.buffers.bind(&self.device, self.pipelines.layout(kind), kind, atlas, layer);
        let bind_with = |view| bind(PipelineKind::Image, Some(&empty_array), Some(view));
        let mut image_binds = FxHashMap::default();
        let mut glyph_binds = FxHashMap::default();
        for batch in &plan.batches {
            match batch.draw {
                Draw::Image(array) => {
                    image_binds.entry(array).or_insert_with(|| bind(PipelineKind::Image, Some(textures.atlases.view(array)), Some(&empty)));
                }
                Draw::Glyph(array) => {
                    glyph_binds.entry(array).or_insert_with(|| bind(PipelineKind::Glyph, Some(textures.atlases.view(array)), None));
                }
                _ => {}
            }
        }
        let layer_binds = layer_views.iter().map(bind_with).collect();
        let canvas_binds = canvas_views.iter().map(bind_with).collect();
        let quad_bind = bind(PipelineKind::Quad, None, None);
        let shadow_bind = bind(PipelineKind::Shadow, None, None);
        let material_bind = bind(PipelineKind::Material, None, None);
        let [r, g, b, a] = straight(input.list.background);
        let background = wgpu::Color { r: (r * a) as f64, g: (g * a) as f64, b: (b * a) as f64, a: a as f64 };
        let frame = PreparedFrame {
            plan,
            background,
            bounds,
            layer_views,
            empty_array,
            depth,
            fragment_clipping,
            quad_bind,
            shadow_bind,
            material_bind,
            image_binds,
            glyph_binds,
            layer_binds,
            canvas_binds,
            transitions,
        };
        Ok((frame, data))
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
