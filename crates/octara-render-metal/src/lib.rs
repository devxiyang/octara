//! Apple Metal encoding for Octara display lists.

#![cfg(any(target_os = "macos", target_os = "ios"))]

mod atlas;
mod encode;
mod material;
mod pipeline;
mod prepare;
mod target;
mod transition;

pub use material::material_msl;
pub use transition::{MetalPresentation, transition_crossfade};

use encode::{LayerTargets, Render};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::*;
use octara_model::{
    capture::{PaintReceipt, PaintSample},
    paint::DisplayList,
    theme::Color,
};
use octara_render::plan::{
    FrameProfile, GlyphCache, OutputColor, SceneSnapshot, SpatialTable, TargetInfo, bytemuck, color, plan_frame_cached, plan_frame_spatial,
};
use pipeline::{Pipeline, Pipelines};
use prepare::{Buffer, PreparedFrame, buffer};
use std::time::Instant;
use target::{Attachments, Command, Device, Output, Raster, Surface};

/// Measurements and author-paint outcomes for the commands recorded into a frame.
pub struct MetalEncodedFrame {
    pub profile: FrameProfile,
    pub receipts: Vec<PaintReceipt>,
    pub presentation: MetalPresentation,
}

/// A Metal renderer intended for use on one device worker.
///
/// The caller supplies the target and command buffer and controls submission and presentation.
pub struct MetalRenderer {
    device: Retained<Device>,
    pipelines: Pipelines,
    glyphs: GlyphCache,
    atlases: atlas::Atlases,
    attachments: Attachments,
    surface: Option<Surface>,
    retained: Option<RetainedPlan>,
    previous: Option<Retained<Command>>,
    transitions: transition::History,
    dummy: Buffer,
}

struct RetainedPlan {
    extent: [u32; 2],
    frame: PreparedFrame,
    table: SpatialTable,
    spatial: Buffer,
}

impl MetalRenderer {
    /// Compiles the built-in Metal shaders and creates the device's common states.
    pub fn new(device: Retained<Device>) -> Result<Self, String> {
        let pipelines = Pipelines::new(&device)?;
        let dummy = buffer(&device, &[0; 96])?;
        Ok(Self {
            device,
            pipelines,
            glyphs: GlyphCache::default(),
            atlases: atlas::Atlases::default(),
            attachments: Attachments::default(),
            surface: None,
            retained: None,
            previous: None,
            transitions: transition::History::default(),
            dummy,
        })
    }

    /// Publishes the submitted frame's transition pixels after the host accepts the frame.
    pub fn accept_presentation(&mut self, presentation: MetalPresentation) {
        self.transitions.accept(presentation);
    }

    /// Records one display list using an open command buffer with retained references.
    ///
    /// Commit a successful recording before encoding another frame on the same queue to
    /// reuse its backing pixels and prepared resources. Discard the command buffer on error.
    /// The texture must belong to the renderer's device and support render targets. A
    /// drawable with framebuffer-only access uses a render transfer when needed.
    /// Unsupported author programs draw their ordinary fallback and report the outcome.
    pub fn encode(
        &mut self,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
        drawable: &ProtocolObject<dyn MTLTexture>,
        list: &DisplayList,
        scale: f32,
        color: OutputColor,
        sample: Option<PaintSample>,
    ) -> Result<MetalEncodedFrame, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("Octara Metal requires a finite positive scale".into());
        }
        if !std::ptr::eq(&*command.device(), &*self.device) || !std::ptr::eq(&*drawable.device(), &*self.device) {
            return Err("Octara Metal command buffer and target must use the renderer's device".into());
        }
        if !command.retainedReferences() || command.status().0 > MTLCommandBufferStatus::Enqueued.0 {
            return Err("Octara Metal requires an open command buffer with retained references".into());
        }
        let output = Output::new(drawable, color)?;
        self.check_previous(command);
        // The borrowed Objective-C command is live and retainable for the current call.
        let command = unsafe { Retained::retain(std::ptr::from_ref(command).cast_mut()) }.expect("live Metal command buffer");
        let result = self.record(&command, drawable, list, scale, output, sample);
        if result.is_ok() {
            self.previous = Some(command);
        } else {
            self.retained = None;
            if let Some(surface) = &mut self.surface {
                surface.snapshot = None;
            }
        }
        result
    }

    fn check_previous(&mut self, command: &Command) {
        if let Some(previous) = &self.previous {
            if !std::ptr::eq(&*command.commandQueue(), &*previous.commandQueue()) {
                self.surface = None;
                self.attachments = Attachments::default();
                self.atlases = atlas::Atlases::default();
                self.retained = None;
                self.previous = None;
                self.transitions = transition::History::default();
                return;
            }
            // Start a full raster if the backing's last write could still fail. Immutable
            // instance data can be reused while a committed command runs on the same queue.
            let status = previous.status();
            if !matches!(status, MTLCommandBufferStatus::Committed | MTLCommandBufferStatus::Scheduled | MTLCommandBufferStatus::Completed)
            {
                self.retained = None;
            }
            if status != MTLCommandBufferStatus::Completed
                && let Some(surface) = &mut self.surface
            {
                surface.snapshot = None;
            }
        }
        if self.atlases.retire_invalid_writes(command) {
            self.retained = None;
            if let Some(surface) = &mut self.surface {
                surface.snapshot = None;
            }
        }
    }

    fn record(
        &mut self,
        command: &Retained<Command>,
        drawable: &ProtocolObject<dyn MTLTexture>,
        list: &DisplayList,
        scale: f32,
        output: Output,
        sample: Option<PaintSample>,
    ) -> Result<MetalEncodedFrame, String> {
        let started = Instant::now();
        self.transitions.prune();
        let mut profile = FrameProfile { damage_area_ratio: 1.0, ..FrameProfile::default() };
        let target = TargetInfo {
            extent: output.extent,
            scale,
            max_texture_dimension: 16384,
            copyable: output.blit,
            encode_output: output.requires_resolve(),
        };
        let (frame, table, spatial) = self.prepare(command, list, target, output.format, sample, &mut profile)?;
        let retain = sample.is_some() && frame.supports_damage;
        if self.surface.as_ref().is_none_or(|surface| !surface.matches(output, frame.resolve_output, retain)) {
            self.surface = Some(Surface::new(&self.device, output, frame.resolve_output, retain)?);
        }
        let surface = self.surface.as_mut().expect("the surface has an attachment");
        let is_spatial = table.is_some();
        let (next_snapshot, raster) = if surface.tracks_damage() {
            let mut snapshot = SceneSnapshot::new(list, scale, output.extent);
            let damage = surface.snapshot.take().and_then(|(previous, spatial)| {
                (spatial == is_spatial).then(|| snapshot.damage_since(&previous, &mut self.glyphs, spatial)).flatten()
            });
            let raster = match damage {
                Some(rect) if rect.is_empty() => Raster::Reuse,
                Some(rect) if rect.is_partial(output.extent) => Raster::Damage(rect),
                _ => Raster::Full,
            };
            (snapshot.can_retain().then_some((snapshot, is_spatial)), raster)
        } else {
            (None, Raster::Full)
        };
        profile.partial_raster = !matches!(raster, Raster::Full);
        profile.damage_area_ratio = match raster {
            Raster::Full => 1.0,
            Raster::Damage(rect) => rect.area_ratio(output.extent),
            Raster::Reuse => 0.0,
        };
        let root = surface.draw_texture(drawable);
        let layers = if matches!(raster, Raster::Reuse) {
            None
        } else {
            Some(LayerTargets::prepare(&self.device, &mut self.attachments, &frame, root, &mut self.pipelines, raster)?)
        };
        if surface.resolves() {
            self.pipelines.get(&self.device, Pipeline::Resolve, output.format, MTLPixelFormat::Invalid)?;
        }
        profile.prepare = started.elapsed();
        let encoding = Instant::now();
        if let Some(layers) = &layers {
            let render = Render {
                frame: &frame,
                pipelines: &self.pipelines,
                targets: layers,
                atlases: &self.atlases,
                dummy: &self.dummy,
                spatial: spatial.as_ref(),
                offsets: table.as_ref().map_or(&[], SpatialTable::offsets),
                scale,
            };
            render.encode(command, root, premultiplied(list.background), raster, &mut profile)?;
        }
        surface.transfer(command, drawable, &self.pipelines, raster, &mut profile)?;
        surface.snapshot = next_snapshot;
        profile.encode = encoding.elapsed();
        profile.atlas_bytes = frame.atlas_bytes;
        let changes =
            sample.zip(layers.as_ref()).map_or_else(Vec::new, |(sample, layers)| layers.transition_changes(&frame, sample, scale));
        let presentation = self.transitions.pending(command, changes);
        let receipts = if let (Some(table), Some(spatial)) = (table, spatial) {
            let receipts = frame.receipts.clone();
            self.retained = Some(RetainedPlan { extent: output.extent, frame, table, spatial });
            receipts
        } else {
            frame.receipts
        };
        Ok(MetalEncodedFrame { profile, receipts, presentation })
    }

    fn prepare(
        &mut self,
        command: &Command,
        list: &DisplayList,
        target: TargetInfo,
        output_format: MTLPixelFormat,
        sample: Option<PaintSample>,
        profile: &mut FrameProfile,
    ) -> Result<(PreparedFrame, Option<SpatialTable>, Option<Buffer>), String> {
        if let Some(mut retained) = self.retained.take()
            && retained.extent == target.extent
            && retained.frame.resolve_output == target.encode_output
            && let Some(writes) = retained.table.update(list, target.scale)
        {
            profile.retained_prepare = true;
            if !writes.is_empty() {
                let bytes = bytemuck::cast_slice(retained.table.offsets());
                // An immutable replacement keeps previous in-flight frames' offsets intact.
                retained.spatial = buffer(&self.device, bytes)?;
                profile.spatial_upload_bytes = bytes.len() as u64;
            }
            return Ok((retained.frame, Some(retained.table), Some(retained.spatial)));
        }
        let spatial = plan_frame_spatial(list, target, &mut self.glyphs);
        let (frame, table, spatial, instance_bytes, atlas_upload_bytes) = if let Some((plan, table)) = spatial
            && let Ok((frame, instances, uploads)) =
                PreparedFrame::new(&self.device, command, &mut self.atlases, plan, output_format, &mut self.pipelines, Vec::new())
        {
            let bytes = bytemuck::cast_slice(table.offsets());
            let spatial = buffer(&self.device, bytes)?;
            profile.spatial_upload_bytes = bytes.len() as u64;
            (frame, Some(table), Some(spatial), instances, uploads)
        } else {
            let mut plan = plan_frame_cached(list, target, sample, &mut self.glyphs);
            let transitions = self.transitions.plan(&mut plan, target.extent, target.scale, sample.is_some());
            let (frame, instances, uploads) =
                PreparedFrame::new(&self.device, command, &mut self.atlases, plan, output_format, &mut self.pipelines, transitions)?;
            (frame, None, None, instances, uploads)
        };
        profile.instance_upload_bytes = instance_bytes;
        profile.atlas_upload_bytes = atlas_upload_bytes;
        Ok((frame, table, spatial))
    }
}

fn premultiplied(color: Color) -> [f32; 4] {
    let [r, g, b, a] = color.convert::<color::LinearSrgb>().components;
    [r * a, g * a, b * a, a]
}
