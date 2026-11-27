//! Records the prepared scene and interleaves painter-prefix and transition commands.

use super::atlas::Bindings;
use super::batch::Draw;
use super::custom::{self, CanvasPaint};
use super::outputs::{BackdropChange, RegionImage};
use super::plan::{Attachment, BackdropInput, PassBounds, PlannedBackdrop, TransitionRequest};
use super::prepare::{PlannedTransition, PreparedFrame};
use super::recording::FrameRecording;
use super::{FrameInput, GpuPassKind, GpuRenderer, Transition, TransitionImage, WORKING_FORMAT};
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
    fn pass(&mut self, kind: GpuPassKind) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
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
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("octara"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
        })],
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
    pass.draw(0..6, instances);
}

impl GpuRenderer {
    async fn encode_backdrop(
        &mut self,
        backdrop: &PlannedBackdrop,
        target: &wgpu::TextureView,
        scale: f32,
        recording: &mut FrameRecording<'_>,
    ) -> Result<EncodedBackdrop, PaintFallback> {
        let request = &backdrop.request;
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

    pub(super) async fn encode_frame(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        frame: &mut PreparedFrame,
        output: &wgpu::TextureView,
        intermediate: Option<&wgpu::TextureView>,
        input: FrameInput<'_>,
        recording: &mut FrameRecording<'_>,
    ) -> wgpu::CommandEncoder {
        if !recording.before.is_empty() {
            recording.before.insert(0, encoder.finish());
            encoder = drawing_encoder(&self.device);
        }
        let draw_target = intermediate.unwrap_or(output);
        let output_format = draw_target.texture().format();
        let buffer = self.buffers.instances().clone();
        for target_plan in &frame.plan.targets {
            let (target, format, clear, kind, index) = match target_plan.attachment {
                Attachment::Output => (draw_target, output_format, frame.background, GpuPassKind::Ui, 0),
                Attachment::Layer(index) => {
                    (&frame.layer_views[index - 1], WORKING_FORMAT, wgpu::Color::TRANSPARENT, GpuPassKind::Layer, index)
                }
            };
            let mut pass = color_pass(&mut encoder, target, wgpu::LoadOp::Clear(clear), recording.pass(kind));
            if !target_plan.batches.is_empty() {
                pass.set_vertex_buffer(0, buffer.slice(..));
            }
            let projection = [self.buffers.projection_offset(index)];
            let plain = &frame.atlas_binds[&Bindings::EMPTY];
            for batch in &frame.plan.batches[target_plan.batches.clone()] {
                let pipelines = &self.pipelines.formats[&format];
                let mut instances = batch.instances.clone();
                let (pipeline, bind) = match batch.draw {
                    Draw::Standard(bindings) => (&pipelines.pipeline, &frame.atlas_binds[&bindings]),
                    Draw::Cutout => (&pipelines.cutout, plain),
                    Draw::Layer(layer) => {
                        if let Some(fallback) = frame.transitions.get(&layer).and_then(|transition| transition.fallback) {
                            instances = fallback..fallback + 1;
                        }
                        (&pipelines.pipeline, &frame.layer_binds[layer - 1])
                    }
                    Draw::Canvas(view) => (&pipelines.pipeline, &frame.canvas_binds[view]),
                    Draw::Glyph(page) => (&self.glyph_pipeline.formats[&format], &frame.glyph_binds[&page]),
                    Draw::Sample(_) | Draw::GlyphSample | Draw::RequestedMaterial(_) => {
                        unreachable!("draw resources prepared before encoding")
                    }
                    Draw::Material(material) => (self.programs.material_pipeline(material, format), plain),
                    Draw::Backdrop(index) => {
                        let backdrop = &frame.plan.backdrops[index];
                        let outcome = match self.encode_backdrop(backdrop, target, input.scale, recording).await {
                            Ok(EncodedBackdrop { view, prefix_commands, outcome }) => {
                                if let Some(command) = prefix_commands {
                                    // The isolated copy and author commands run after the prefix is stored.
                                    drop(pass);
                                    encoder = recording.continue_after(&self.device, encoder, command);
                                    pass = color_pass(&mut encoder, target, wgpu::LoadOp::Load, recording.pass(kind));
                                    pass.set_vertex_buffer(0, buffer.slice(..));
                                }
                                let bind = self.buffers.bind_texture(&self.device, &self.pipelines.layout, [&frame.empty; 2], &view);
                                draw_batch(&mut pass, &self.pipelines.formats[&format].pipeline, &bind, &projection, instances);
                                outcome
                            }
                            Err(reason) => {
                                let fallback = backdrop.fallback;
                                draw_batch(
                                    &mut pass,
                                    &self.pipelines.formats[&format].pipeline,
                                    plain,
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
                draw_batch(&mut pass, pipeline, bind, &projection, instances);
            }
            drop(pass);
            if let Some(transition) = frame.transitions.get_mut(&index)
                && let Some((composite, command)) = self.encode_transition(transition, target, frame.bounds[index], input, recording).await
            {
                // The new image is produced by this child pass. Its parent samples the output
                // after the author commands, without splitting the parent's render pass.
                encoder = recording.continue_after(&self.device, encoder, command);
                frame.layer_binds[index - 1] =
                    self.buffers.bind_texture(&self.device, &self.pipelines.layout, [&frame.empty; 2], &composite);
            }
        }
        if intermediate.is_some() {
            self.output_resolve.encode(&mut encoder, output, recording.pass(GpuPassKind::Output));
            if let Some(measurement) = recording.measurement.as_deref_mut() {
                let texture = output.texture();
                measurement.profile.output_pixels = u64::from(texture.width()) * u64::from(texture.height());
            }
        }
        if recording.outputs.is_none() {
            self.output_resolve.release_source();
        }
        encoder
    }
}
