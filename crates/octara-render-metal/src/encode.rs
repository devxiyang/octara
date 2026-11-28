//! Records painter-order geometry and isolated layers into the caller's command buffer.

use super::atlas::Atlases;
use super::external::MetalImage;
use super::pipeline::{Pipeline, Pipelines};
use super::prepare::{Buffer, DrawBuffers, PreparedFrame};
use super::target::{
    Attachments, Command, Device, Encoder, Raster, Texture, WORKING_FORMAT, fragment_bytes, fullscreen, render_pass, scissor, texture,
};
use super::transition::{Change, Crossfade, Region};
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use octara_model::capture::PaintSample;
use octara_render::plan::{DamageRect, Draw, FrameProfile, PassBounds, PlannedBatch};
use std::ops::Range;

pub(crate) struct LayerTargets {
    layers: Vec<Option<Texture>>,
    composites: Vec<Option<Texture>>,
    depths: Vec<Option<Texture>>,
}

impl LayerTargets {
    pub(crate) fn prepare(
        device: &Device,
        attachments: &mut Attachments,
        frame: &PreparedFrame,
        root: &ProtocolObject<dyn MTLTexture>,
        pipelines: &mut Pipelines,
        raster: Raster,
    ) -> Result<Self, String> {
        attachments.begin_frame(frame.passes.len() - 1, frame.passes.iter().map(|pass| (pass.bounds.size, pass.depth)));
        let mut layers = vec![None];
        let mut composites = Vec::with_capacity(frame.passes.len());
        let mut depths = Vec::with_capacity(frame.passes.len());
        for (index, pass) in frame.passes.iter().enumerate() {
            let composite = pass.transition.as_ref().and_then(|transition| transition.composite.as_ref());
            if composite.is_some() {
                pipelines.get(device, Pipeline::Crossfade, WORKING_FORMAT, MTLPixelFormat::Invalid)?;
            }
            composites.push(composite.map(|crossfade| layer_texture(device, crossfade.bounds)).transpose()?);
            if index > 0 {
                let layer = if pass.bounds.is_empty() {
                    None
                } else if pass.transition.as_ref().is_some_and(|transition| transition.retain) && composite.is_none() {
                    // A candidate can outlive the texture used by subsequent frames.
                    Some(layer_texture(device, pass.bounds)?)
                } else {
                    Some(attachments.layer(device, index - 1, pass.bounds.size)?)
                };
                layers.push(layer);
            }
            if index > 0 && layers[index].is_none() {
                depths.push(None);
                continue;
            }
            let depth = attachments.depth(device, pass.bounds.size, pass.depth)?;
            let format = layers[index].as_deref().unwrap_or(root).pixelFormat();
            let depth_format = depth.as_deref().map_or(MTLPixelFormat::Invalid, MTLTexture::pixelFormat);
            if index == 0 && matches!(raster, Raster::Damage(_)) {
                pipelines.get(device, Pipeline::Clear, format, depth_format)?;
            }
            for seg in &pass.segments {
                for (core, range) in [(true, seg.cores.clone()), (false, seg.blended.clone())] {
                    for batch in &frame.batches[range] {
                        if !matches!(batch.draw, Draw::Material(_)) {
                            pipelines.get(device, pipeline(frame, batch.draw, core), format, depth_format)?;
                        }
                    }
                }
            }
            depths.push(depth);
        }
        Ok(Self { layers, composites, depths })
    }

    fn composite(&self, index: usize) -> Option<&Texture> {
        self.composites[index].as_ref().or(self.layers[index].as_ref())
    }

    pub(crate) fn transition_changes(&self, frame: &PreparedFrame, sample: PaintSample, scale: f32) -> Vec<Change> {
        let mut changes: Vec<_> = frame
            .skipped_transitions
            .iter()
            .map(|(mount, revision)| Change { mount: mount.clone(), revision: *revision, image: None, pin: None })
            .collect();
        for (index, pass) in frame.passes.iter().enumerate() {
            let Some(transition) = &pass.transition else { continue };
            if !transition.publish {
                continue;
            }
            let bounds = transition.composite.as_ref().map_or(pass.bounds, |crossfade| crossfade.bounds);
            let image = if transition.retain {
                self.composite(index).map(|texture| Region { texture: texture.clone(), bounds, sample, scale })
            } else {
                None
            };
            let pin = transition.composite.as_ref().map(|crossfade| (transition.run.clone(), crossfade.old.clone()));
            changes.push(Change { mount: transition.mount.clone(), revision: transition.revision, image, pin });
        }
        changes
    }
}

fn layer_texture(device: &Device, bounds: PassBounds) -> Result<Texture, String> {
    texture(device, bounds.size, WORKING_FORMAT, MTLStorageMode::Private, true)
}

pub(crate) struct Render<'a> {
    pub(crate) frame: &'a PreparedFrame,
    pub(crate) pipelines: &'a Pipelines,
    pub(crate) targets: &'a LayerTargets,
    pub(crate) atlases: &'a Atlases,
    pub(crate) dummy: &'a Buffer,
    pub(crate) spatial: Option<&'a Buffer>,
    pub(crate) offsets: &'a [[f32; 4]],
    pub(crate) scale: f32,
}

impl Render<'_> {
    pub(crate) fn encode(
        &self,
        command: &Command,
        root: &ProtocolObject<dyn MTLTexture>,
        background: [f32; 4],
        raster: Raster,
        profile: &mut FrameProfile,
    ) -> Result<(), String> {
        let damage = match raster {
            Raster::Full => None,
            Raster::Damage(rect) => Some(rect),
            Raster::Reuse => return Ok(()),
        };
        for (index, prepared) in self.frame.passes.iter().enumerate().rev() {
            let layer = self.targets.layers[index].as_deref();
            if index == 0 {
                self.paint_pass(command, root, index, background, damage, profile)?;
            } else if let Some(layer) = layer {
                profile.layer_pixels += u64::from(prepared.bounds.size[0]) * u64::from(prepared.bounds.size[1]);
                self.paint_pass(command, layer, index, [0.0; 4], None, profile)?;
            }
            if let Some(crossfade) = prepared.transition.as_ref().and_then(|transition| transition.composite.as_ref()) {
                let target = self.targets.composites[index].as_deref().expect("prepared crossfade output");
                self.crossfade(command, target, layer, prepared.bounds, crossfade, profile)?;
            }
        }
        Ok(())
    }

    fn paint_pass(
        &self,
        command: &Command,
        target: &ProtocolObject<dyn MTLTexture>,
        index: usize,
        clear: [f32; 4],
        damage: Option<DamageRect>,
        profile: &mut FrameProfile,
    ) -> Result<(), String> {
        let prepared = &self.frame.passes[index];
        let depth = self.targets.depths[index].as_deref();
        let depth_format = depth.map_or(MTLPixelFormat::Invalid, MTLTexture::pixelFormat);
        let format = target.pixelFormat();
        let mut load = damage.is_some();

        // An empty scene still clears the output. A new depth segment can discard its
        // previous depth while preserving the painter prefix already in the color attachment.
        for (segment_index, segment) in prepared.segments.iter().map(Some).chain(prepared.segments.is_empty().then_some(None)).enumerate() {
            let encoder = render_pass(command, target, depth, load, clear)?;
            profile.render_passes += 1;
            if let Some(rect) = damage {
                scissor(&encoder, rect);
                if segment_index == 0 {
                    encoder.setRenderPipelineState(self.pipelines.state(Pipeline::Clear, format, depth_format));
                    if depth.is_some() {
                        encoder.setDepthStencilState(Some(&self.pipelines.always));
                    }
                    fragment_bytes(&encoder, &clear, 0);
                    fullscreen(&encoder);
                }
            }
            if let (Some(segment), Some(globals)) = (segment, &prepared.globals) {
                self.bind_globals(&encoder, globals);
                let draws = SceneEncoder { renderer: self, encoder: &encoder, format, depth: depth_format, damage };
                profile.draw_batches += draws.batches(segment.cores.clone(), true);
                profile.draw_batches += draws.batches(segment.blended.clone(), false);
                profile.material_batches +=
                    self.frame.batches[segment.blended.clone()].iter().filter(|batch| matches!(batch.draw, Draw::Material(_))).count()
                        as u32;
            }
            encoder.endEncoding();
            load = true;
        }
        Ok(())
    }

    fn crossfade(
        &self,
        command: &Command,
        target: &ProtocolObject<dyn MTLTexture>,
        current: Option<&ProtocolObject<dyn MTLTexture>>,
        current_bounds: PassBounds,
        crossfade: &Crossfade,
        profile: &mut FrameProfile,
    ) -> Result<(), String> {
        let old = &crossfade.old;
        let [x, y, w, h] = crossfade.bounds.rect();
        let params = [[x, y, old.scale / self.scale, crossfade.progress], old.bounds.rect(), current_bounds.rect()];
        let encoder = render_pass(command, target, None, false, [0.0; 4])?;
        encoder.setRenderPipelineState(self.pipelines.state(Pipeline::Crossfade, WORKING_FORMAT, MTLPixelFormat::Invalid));
        fragment_bytes(&encoder, &params, 0);
        // Both inputs are private shader-readable colors. Zero current bounds mask an empty layer.
        unsafe {
            encoder.setFragmentTexture_atIndex(Some(&old.texture), 0);
            encoder.setFragmentTexture_atIndex(Some(current.unwrap_or(&old.texture)), 1);
            encoder.setFragmentSamplerState_atIndex(Some(&self.pipelines.sampler), 3);
        }
        fullscreen(&encoder);
        encoder.endEncoding();
        profile.render_passes += 1;
        profile.canvas_pixels += (w as u64) * (h as u64);
        Ok(())
    }

    fn bind_globals(&self, encoder: &Encoder, globals: &Buffer) {
        bind_shared(encoder, globals, 0);
        let clip = self.frame.buffers.clips.as_ref();
        bind_shared(encoder, clip.unwrap_or(self.dummy), 5);
        bind_shared(encoder, self.spatial.unwrap_or(self.dummy), 8);
    }
}

struct SceneEncoder<'a, 'frame> {
    renderer: &'a Render<'frame>,
    encoder: &'a Encoder,
    format: MTLPixelFormat,
    depth: MTLPixelFormat,
    damage: Option<DamageRect>,
}

impl SceneEncoder<'_, '_> {
    fn batches(&self, batches: Range<usize>, core: bool) -> u32 {
        let renderer = self.renderer;
        if self.depth != MTLPixelFormat::Invalid {
            let state = if core { &renderer.pipelines.core } else { &renderer.pipelines.blend };
            self.encoder.setDepthStencilState(Some(state));
        }
        renderer.frame.batches[batches].iter().map(|batch| self.batch(batch, core)).sum()
    }

    fn batch(&self, batch: &PlannedBatch, core: bool) -> u32 {
        if let Draw::Layer(index) = batch.draw
            && self.renderer.targets.composite(index).is_none()
        {
            return 0;
        }
        let mut bind = true;
        let mut draw = |instances: Range<u32>| {
            if bind {
                self.bind_batch(batch.draw, core);
                bind = false;
            }
            // The selected buffers cover exactly the planner's instance range.
            unsafe {
                self.encoder.drawPrimitives_vertexStart_vertexCount_instanceCount_baseInstance(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    4,
                    instances.len(),
                    instances.start as usize,
                );
            }
        };
        if let Some(rect) = self.damage {
            batch.visible_ranges(rect.rect(), self.renderer.offsets, draw)
        } else {
            draw(batch.instances.clone());
            1
        }
    }

    fn bind_batch(&self, draw: Draw, core: bool) {
        let encoder = self.encoder;
        let renderer = self.renderer;
        let state = match draw {
            Draw::Material(material) => renderer.pipelines.material_state(material, self.format, self.depth),
            _ => renderer.pipelines.state(pipeline(renderer.frame, draw, core), self.format, self.depth),
        };
        encoder.setRenderPipelineState(state);
        let DrawBuffers { draws, quads, shadows, images, glyphs, glyph_draws, glyph_clips, gradients, materials, .. } =
            &renderer.frame.buffers;
        let buffer = |slot| bound_buffer(slot, renderer.dummy);
        let payload = match draw {
            Draw::Quad | Draw::Cutout => quads,
            Draw::Shadow => shadows,
            Draw::Image(_) | Draw::Layer(_) | Draw::External(_) => images,
            Draw::Glyph(_) => glyphs,
            Draw::Material(_) => materials,
            _ => unreachable!("paint resolved before recording"),
        };
        if let Draw::Glyph(id) = draw {
            bind_shared(encoder, buffer(payload), 2);
            bind_shared(encoder, buffer(glyph_draws), 6);
            bind_shared(encoder, buffer(glyph_clips), 7);
            // Glyph lookups use the array and slice assigned by atlas preparation.
            unsafe { encoder.setFragmentTexture_atIndex(Some(renderer.atlases.texture(id)), 1) };
        } else {
            bind_shared(encoder, buffer(draws), 6);
            // Indices and payload layouts are fixed by the frame-plan and Metal shader ABI.
            unsafe {
                encoder.setFragmentBuffer_offset_atIndex(Some(buffer(payload)), 0, 2);
                encoder.setFragmentBuffer_offset_atIndex(Some(buffer(gradients)), 0, 7);
                match draw {
                    Draw::Image(id) => {
                        encoder.setFragmentTexture_atIndex(Some(renderer.atlases.texture(id)), 1);
                        encoder.setFragmentSamplerState_atIndex(Some(&renderer.pipelines.sampler), 3);
                    }
                    Draw::Layer(index) => {
                        encoder.setFragmentTexture_atIndex(renderer.targets.composite(index).map(|texture| &**texture), 4);
                        let pass = &renderer.frame.passes[index];
                        if let Some(matrix) = pass.transform {
                            let bounds = pass
                                .transition
                                .as_ref()
                                .and_then(|transition| transition.composite.as_ref())
                                .map_or(pass.bounds, |crossfade| crossfade.bounds);
                            let [x, y, z, w] = matrix;
                            let transform = [x, y, z, w, bounds.rect()];
                            let bytes = octara_render::plan::bytemuck::bytes_of(&transform);
                            encoder.setVertexBytes_length_atIndex(std::ptr::NonNull::from(bytes).cast(), bytes.len(), 9);
                            encoder.setFragmentSamplerState_atIndex(Some(&renderer.pipelines.sampler), 3);
                        }
                    }
                    Draw::External(index) => {
                        encoder.setFragmentTexture_atIndex(Some(&self.external(index).texture), 4);
                        encoder.setFragmentSamplerState_atIndex(Some(&renderer.pipelines.sampler), 3);
                    }
                    _ => {}
                }
            }
        }
    }

    fn external(&self, index: usize) -> &MetalImage {
        self.renderer.frame.external[index].as_ref().and_then(|handle| handle.downcast_ref()).expect("prepared Metal texture")
    }
}

fn pipeline(frame: &PreparedFrame, draw: Draw, core: bool) -> Pipeline {
    match (draw, core) {
        (Draw::Quad, true) => Pipeline::QuadCore,
        (Draw::Image(_), true) => Pipeline::ImageCore,
        (Draw::Quad, false) => Pipeline::Quad,
        (Draw::Image(_), false) => Pipeline::Image,
        (Draw::Cutout, false) => Pipeline::Cutout,
        (Draw::Glyph(_), false) => Pipeline::Glyph,
        (Draw::Shadow, false) => Pipeline::Shadow,
        (Draw::Layer(index), false) if frame.passes[index].transform.is_some() => Pipeline::ProjectedLayer,
        (Draw::Layer(_), false) => Pipeline::Layer,
        (Draw::External(_), false) => Pipeline::External,
        _ => unreachable!("draw scheduled before encoding"),
    }
}

fn bind_shared(encoder: &Encoder, buffer: &Buffer, slot: usize) {
    // Both stages use the same ABI and read-only buffer; offset zero satisfies alignment.
    unsafe {
        encoder.setVertexBuffer_offset_atIndex(Some(buffer), 0, slot);
        encoder.setFragmentBuffer_offset_atIndex(Some(buffer), 0, slot);
    }
}

fn bound_buffer<'a>(slot: &'a Option<Buffer>, dummy: &'a Buffer) -> &'a Buffer {
    slot.as_ref().unwrap_or(dummy)
}
