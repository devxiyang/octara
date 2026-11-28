//! Records the prepared scene and interleaves painter-prefix and transition commands.

use super::backing::Clear;
use super::custom;
use super::outputs::{BackdropChange, RegionImage};
use super::pipeline::PipelineKind;
use super::prepare::{Attachment, PlannedBackdrop, PlannedTransition, PreparedFrame};
use super::recording::FrameRecording;
use super::{FrameInput, GpuPassKind, Transition, TransitionImage, WORKING_FORMAT, WgpuRenderer};
use crate::plan::DamageRect;
use crate::plan::batch::{Draw, PlannedBatch};
use crate::plan::{BackdropInput, CanvasMode, CanvasPaint, PassBounds, TransitionRequest};
use octara_model::capture::{PaintFallback, PaintOutcome};
use octara_model::geometry::Rect;
use octara_model::paint::PaintInput;
use std::ops::Range;

struct EncodedBackdrop {
    view: wgpu::TextureView,
    prefix_commands: Option<wgpu::CommandBuffer>,
    outcome: PaintOutcome,
}

impl FrameRecording<'_> {
    pub(super) fn pass(&mut self, kind: GpuPassKind) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.measurement.as_deref_mut()?.pass(kind)
    }

    fn continue_after(
        &mut self,
        device: &wgpu::Device,
        encoder: wgpu::CommandEncoder,
        commands: wgpu::CommandBuffer,
    ) -> wgpu::CommandEncoder {
        self.before.extend([encoder.finish(), commands]);
        drawing_encoder(device)
    }
}

fn drawing_encoder(device: &wgpu::Device) -> wgpu::CommandEncoder {
    device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara drawing") })
}

pub(super) fn color_pass<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'encoder> {
    attachment_pass(encoder, target, load, None, timestamp_writes)
}

fn scene_pass<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    depth: &wgpu::TextureView,
    clear_depth: bool,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'encoder> {
    let depth = wgpu::RenderPassDepthStencilAttachment {
        view: depth,
        depth_ops: Some(wgpu::Operations {
            load: if clear_depth { wgpu::LoadOp::Clear(0.0) } else { wgpu::LoadOp::Load },
            store: wgpu::StoreOp::Store,
        }),
        stencil_ops: None,
    };
    attachment_pass(encoder, target, load, Some(depth), timestamp_writes)
}

fn attachment_pass<'encoder>(
    encoder: &'encoder mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    depth_stencil_attachment: Option<wgpu::RenderPassDepthStencilAttachment<'_>>,
    timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
) -> wgpu::RenderPass<'encoder> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("octara"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment,
        timestamp_writes,
        ..Default::default()
    })
}

fn draw_batch(
    pass: &mut wgpu::RenderPass<'_>,
    pipeline: &wgpu::RenderPipeline,
    bind: &wgpu::BindGroup,
    projection: &[u32],
    instances: Range<u32>,
) {
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind, projection);
    pass.draw(0..4, instances);
}

fn draw_visible(
    pass: &mut wgpu::RenderPass<'_>,
    pipeline: &wgpu::RenderPipeline,
    bind: &wgpu::BindGroup,
    projection: &[u32],
    batch: &PlannedBatch,
    damage: Rect,
    offsets: &[[f32; 4]],
) -> u32 {
    let mut bind_once = true;
    batch.visible_ranges(damage, offsets, |instances| {
        if bind_once {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, projection);
            bind_once = false;
        }
        pass.draw(0..4, instances);
    })
}

impl WgpuRenderer {
    fn bind_composite(&self, empty_array: &wgpu::TextureView, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.buffers.bind(&self.device, self.pipelines.layout(PipelineKind::Image), PipelineKind::Image, Some(empty_array), Some(view))
    }

    async fn encode_backdrop(
        &mut self,
        backdrop: &PlannedBackdrop,
        target: &wgpu::TextureView,
        scale: f32,
        recording: &mut FrameRecording<'_>,
    ) -> Result<EncodedBackdrop, PaintFallback> {
        let request = &backdrop.request;
        if custom::canvas_mode(&request.paint.program) != Some(CanvasMode::Backdrop) {
            return Err(PaintFallback::UnsupportedProgram);
        }
        let region = match backdrop.input {
            BackdropInput::Prefix(region) => region,
            BackdropInput::Unavailable => return Err(PaintFallback::BackdropUnavailable),
            BackdropInput::Accepted => {
                let image = self.presentation_history.get(&request.mount).ok_or(PaintFallback::HistoryUnavailable)?;
                if let Some(p) = recording.measurement.as_deref_mut() {
                    p.profile.retained_backdrops += 1;
                }
                return Ok(EncodedBackdrop {
                    view: image.view.clone(),
                    prefix_commands: None,
                    outcome: PaintOutcome::Retained { revision: image.revision },
                });
            }
        };
        let copy = custom::BackdropCopy::new(target.texture(), region);
        let working = recording.outputs.as_ref().and_then(|_| self.presentation_history.take_working(&request.mount));
        let (view, command) = self
            .programs
            .encode_backdrop(
                &self.device,
                &request.paint,
                scale,
                working.as_ref().map(wgpu::TextureView::texture),
                copy,
                recording.measurement.as_deref_mut(),
            )
            .await?;
        if let Some(changes) = &mut recording.outputs {
            changes.backdrops.push(BackdropChange { mount: request.mount.clone(), view: view.clone() });
        }
        Ok(EncodedBackdrop { view, prefix_commands: Some(command), outcome: PaintOutcome::Rendered })
    }

    async fn encode_transition(
        &mut self,
        transition: &mut PlannedTransition,
        target: &wgpu::TextureView,
        current: PassBounds,
        input: FrameInput<'_>,
        recording: &mut FrameRecording<'_>,
    ) -> Option<(wgpu::TextureView, wgpu::CommandBuffer)> {
        let TransitionRequest { paint, receipt, .. } = &transition.request;
        let product = if let Some(composite) = transition.composite.take() {
            let scale = input.scale;
            let [x, y, w, h] = composite.bounds.rect().map(|px| px / scale);
            let receipt = receipt.expect("an active transition has a receipt");
            let paint_inputs = CanvasPaint {
                program: paint.program.clone(),
                source: paint.source,
                size: composite.bounds.size,
                input: PaintInput::default(),
                rect: Rect::new(x, y, w, h),
                radius: [0.0; 4],
                sampled_at: input.sample.map(|sample| sample.sampled_at),
            };
            let new = RegionImage::new(target, current.origin, scale);
            let sources = Transition {
                old: composite.old.input(),
                new: TransitionImage { size: current.size, ..new.input() },
                progress: paint.progress.max(0.0),
            };
            match self.programs.encode_transition(&self.device, &paint_inputs, scale, sources, recording.measurement.as_deref_mut()).await {
                Ok((view, command)) => {
                    transition.fallback = None;
                    recording.receipts[receipt].outcome = PaintOutcome::Transition { old: composite.sample };
                    Some((view, command, composite.bounds))
                }
                Err(reason) => {
                    recording.receipts[receipt].outcome = PaintOutcome::Fallback { reason };
                    None
                }
            }
        } else {
            None
        };
        if let Some(changes) = &mut recording.outputs {
            let (view, bounds) = product.as_ref().map_or((target, current), |(view, _, bounds)| (view, *bounds));
            let retain = input.sample.is_some() && paint.opacity > 0.0 && !bounds.is_empty() && RegionImage::can_retain(bounds.size);
            let image = retain.then(|| RegionImage::new(view, bounds.origin, input.scale));
            changes.transitions.record(paint.mount.downgrade(), paint.revision, image);
        }
        product.map(|(view, command, _)| (view, command))
    }

    #[expect(clippy::too_many_arguments)]
    pub(super) async fn encode_frame(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        frame: &mut PreparedFrame,
        draw_target: &wgpu::TextureView,
        partial: Option<(DamageRect, &Clear)>,
        offsets: &[[f32; 4]],
        input: FrameInput<'_>,
        recording: &mut FrameRecording<'_>,
    ) -> wgpu::CommandEncoder {
        if !recording.before.is_empty() {
            recording.before.insert(0, encoder.finish());
            encoder = drawing_encoder(&self.device);
        }

        let output_format = draw_target.texture().format();
        let mut partial_batches = 0;
        for target_plan in &frame.plan.targets {
            let (target, format, clear, kind, index) = match target_plan.attachment {
                Attachment::Output => (draw_target, output_format, frame.background, GpuPassKind::Ui, 0),
                Attachment::Layer(index) => {
                    (&frame.layer_views[index - 1], WORKING_FORMAT, wgpu::Color::TRANSPARENT, GpuPassKind::Layer, index)
                }
            };
            let mut load = if partial.is_some() { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(clear) };
            let mut needs_damage_clear = partial.is_some();
            let depth = &frame.depth[index];
            let projection = [self.buffers.projection_offset(index)];
            let pipeline_key = (format, frame.fragment_clipping);
            for segment in &target_plan.segments {
                if !segment.cores.is_empty() {
                    let mut pass = scene_pass(&mut encoder, target, load, depth, partial.is_none(), recording.pass(kind));
                    if let Some((damage, clear)) = partial {
                        let [x, y, w, h] = damage.pixels_rect();
                        pass.set_scissor_rect(x, y, w, h);
                        clear.encode(&mut pass);
                        needs_damage_clear = false;
                    }
                    let pipelines = &self.pipelines.formats[&pipeline_key];
                    for batch in &frame.plan.batches[segment.cores.clone()] {
                        let (pipeline, bind) = match batch.draw {
                            Draw::Quad => (&pipelines.quad_core, &frame.quad_bind),
                            Draw::Image(array) => (&pipelines.image_core, &frame.image_binds[&array]),
                            _ => unreachable!("only quads and images have opaque cores"),
                        };
                        if let Some((damage, _)) = partial {
                            partial_batches += draw_visible(&mut pass, pipeline, bind, &projection, batch, damage.rect(), offsets);
                        } else {
                            draw_batch(&mut pass, pipeline, bind, &projection, batch.instances.clone());
                        }
                    }
                    load = wgpu::LoadOp::Load;
                }
                if segment.blended.is_empty() {
                    continue;
                }
                let mut pass =
                    scene_pass(&mut encoder, target, load, depth, partial.is_none() && segment.cores.is_empty(), recording.pass(kind));
                if let Some((damage, clear)) = partial {
                    let [x, y, w, h] = damage.pixels_rect();
                    pass.set_scissor_rect(x, y, w, h);
                    if needs_damage_clear {
                        clear.encode(&mut pass);
                        needs_damage_clear = false;
                    }
                }
                load = wgpu::LoadOp::Load;
                for batch in &frame.plan.batches[segment.blended.clone()] {
                    let pipelines = &self.pipelines.formats[&pipeline_key];
                    let mut instances = batch.instances.clone();
                    let (pipeline, bind) = match batch.draw {
                        Draw::Quad => (&pipelines.quad, &frame.quad_bind),
                        Draw::Shadow => (&pipelines.shadow, &frame.shadow_bind),
                        Draw::Image(array) => (&pipelines.image, &frame.image_binds[&array]),
                        Draw::Cutout => (&pipelines.cutout, &frame.quad_bind),
                        Draw::Layer(layer) => {
                            if let Some(fallback) = frame.transitions.get(&layer).and_then(|transition| transition.fallback) {
                                instances = fallback..fallback + 1;
                            }
                            (&pipelines.image, &frame.layer_binds[layer - 1])
                        }
                        Draw::Canvas(view) => (&pipelines.image, &frame.canvas_binds[view]),
                        Draw::Glyph(array) => (&pipelines.glyph, &frame.glyph_binds[&array]),
                        Draw::Sample(_) | Draw::RequestedMaterial(_) => unreachable!("draw resources prepared before encoding"),
                        Draw::Material(material) => (self.programs.material_pipeline(material, format), &frame.material_bind),
                        Draw::Backdrop(index) => {
                            let backdrop = &frame.plan.backdrops[index];
                            let outcome = match self.encode_backdrop(backdrop, target, input.scale, recording).await {
                                Ok(EncodedBackdrop { view, prefix_commands, outcome }) => {
                                    if let Some(command) = prefix_commands {
                                        // The copy observes only this segment's prefix.
                                        drop(pass);
                                        encoder = recording.continue_after(&self.device, encoder, command);
                                        pass = scene_pass(&mut encoder, target, load, depth, false, recording.pass(kind));
                                    }
                                    let bind = self.bind_composite(&frame.empty_array, &view);
                                    draw_batch(&mut pass, &self.pipelines.formats[&pipeline_key].image, &bind, &projection, instances);
                                    outcome
                                }
                                Err(reason) => {
                                    let fallback = backdrop.fallback;
                                    draw_batch(
                                        &mut pass,
                                        &self.pipelines.formats[&pipeline_key].quad,
                                        &frame.quad_bind,
                                        &projection,
                                        fallback..fallback + 1,
                                    );
                                    PaintOutcome::Fallback { reason }
                                }
                            };
                            recording.receipts[backdrop.request.receipt].outcome = outcome;
                            continue;
                        }
                    };
                    if let Some((damage, _)) = partial {
                        partial_batches += draw_visible(&mut pass, pipeline, bind, &projection, batch, damage.rect(), offsets);
                    } else {
                        draw_batch(&mut pass, pipeline, bind, &projection, instances);
                    }
                }
            }
            if let Some((damage, clear)) = partial.filter(|_| needs_damage_clear) {
                let mut pass = scene_pass(&mut encoder, target, load, depth, false, recording.pass(kind));
                let [x, y, w, h] = damage.pixels_rect();
                pass.set_scissor_rect(x, y, w, h);
                clear.encode(&mut pass);
            } else if matches!(load, wgpu::LoadOp::Clear(_)) {
                drop(color_pass(&mut encoder, target, load, recording.pass(kind)));
            }
            if let Some(transition) = frame.transitions.get_mut(&index)
                && let Some((composite, command)) = self.encode_transition(transition, target, frame.bounds[index], input, recording).await
            {
                // The new image is produced by this child pass. Its parent samples the output
                // after the author commands, without splitting the parent's render pass.
                encoder = recording.continue_after(&self.device, encoder, command);
                frame.layer_binds[index - 1] = self.bind_composite(&frame.empty_array, &composite);
            }
        }
        if partial.is_some()
            && let Some(measurement) = recording.measurement.as_deref_mut()
        {
            measurement.profile.draw_batches = partial_batches;
        }
        encoder
    }
}
