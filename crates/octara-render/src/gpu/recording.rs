//! Frame recording and submission with staged uploads and presentation changes.

use super::backing::{Backing, Raster};
use super::outputs::{CanvasCache, OutputChanges, PendingPresentation, PresentationHistory};
use super::prepare::PreparedFrame;
use super::profile::{FrameMeasurement, Instant, PendingTimestamps, TimestampPool};
use super::resolve::AttachmentInfo;
use super::retained::{CachedFrame, RetainedTarget};
use super::surface::record_surface;
use super::upload::{FrameData, FrameUploads};
use super::{FrameInput, FrameTarget, RecordError, SceneTextures, WgpuRenderer, custom};
use super::{FrameProfile, GpuTimings};
use crate::plan::SceneSnapshot;
use crate::plan::{SpatialTable, plan_frame_cached, plan_frame_spatial};
use octara_model::capture::{PaintReceipt, PaintSample};

#[derive(Default)]
pub(super) struct FrameRecording<'a> {
    /// Caller and author commands preceding the frame's current encoder.
    pub(super) before: Vec<wgpu::CommandBuffer>,
    /// Captures reuse cached canvases and accepted backdrop history without publishing outputs.
    pub(super) outputs: Option<OutputChanges>,
    pub(super) receipts: Vec<PaintReceipt>,
    pub(super) measurement: Option<&'a mut FrameMeasurement>,
}

/// A frame's recorded commands and the exclusive renderer borrow protecting its staged data.
///
/// Submit or drop this frame before using the renderer again. Dropping it discards its commands
/// and releases the atlas regions reserved for its unsubmitted uploads.
#[must_use = "submit the recorded frame to execute its commands"]
pub struct RecordedFrame<'renderer> {
    before: Vec<wgpu::CommandBuffer>,
    pub(super) encoder: wgpu::CommandEncoder,
    pub(super) device: &'renderer wgpu::Device,
    queue: &'renderer wgpu::Queue,
    uploads: Option<FrameUploads<'renderer>>,
    cache: &'renderer mut Option<CachedFrame>,
    retained: Option<CachedFrame>,
    backing_cache: &'renderer mut Option<Backing>,
    // Captures leave the presented backing alone. Ordinary submission replaces it.
    backing: Option<Option<Backing>>,
    canvas_cache: &'renderer mut CanvasCache,
    presentation_history: &'renderer PresentationHistory,
    changes: OutputChanges,
    sample: Option<PaintSample>,
    pub(super) receipts: Vec<PaintReceipt>,
    timestamp_pool: &'renderer mut TimestampPool,
    timestamps: Option<PendingTimestamps>,
}

impl RecordedFrame<'_> {
    /// Borrows the frame's encoder to append commands after the Octara draw, such as a readback
    /// copy. Leave the encoder in the frame for submission.
    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        &mut self.encoder
    }

    /// Outcomes in display-list order for the visible author draws recorded in this frame.
    pub fn paint_receipts(&self) -> &[PaintReceipt] {
        &self.receipts
    }

    /// Logical bytes of the texture and buffer writes that will accompany this submission.
    pub fn upload_bytes(&self) -> u64 {
        self.uploads.as_ref().map_or(0, FrameUploads::bytes)
    }

    /// Finishes the encoder and submits the frame with its staged uploads. Retained canvases
    /// become reusable at submission. Use [`Self::submit_for_presentation`] to retain a frame's
    /// backdrop outputs and transition regions for host acceptance.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn submit(self) -> wgpu::SubmissionIndex {
        self.submit_for_presentation().0
    }

    /// Submits the frame and returns its candidate backdrop outputs and transition regions. Give the token to
    /// [`WgpuRenderer::accept_presentation`] once the host accepts the frame.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn submit_for_presentation(self) -> (wgpu::SubmissionIndex, PendingPresentation) {
        crate::block_on(self.submit_for_presentation_async()).unwrap_or_else(|error| panic!("Octara frame submission failed: {error}"))
    }

    /// Submits with staged uploads. The host may accept the returned presentation candidate.
    pub async fn submit_for_presentation_async(self) -> Result<(wgpu::SubmissionIndex, PendingPresentation), wgpu::Error> {
        self.submit_checked(false).await
    }

    pub(super) async fn submit_checked(self, validate: bool) -> Result<(wgpu::SubmissionIndex, PendingPresentation), wgpu::Error> {
        let Self {
            mut uploads,
            before,
            encoder,
            device,
            queue,
            cache,
            retained,
            backing_cache,
            backing,
            canvas_cache,
            presentation_history,
            changes,
            sample,
            timestamp_pool,
            timestamps,
            ..
        } = self;
        let work = before.into_iter().chain(std::iter::once_with(|| encoder.finish()));
        let (submission, _) = timestamp_pool
            .submit(work, async |commands| {
                let submit = || {
                    // Frame-owned writes precede all the commands in this submission, including
                    // commands recorded by the caller before Octara.
                    if let Some(uploads) = &uploads {
                        uploads.queue(queue);
                    }
                    queue.submit(commands)
                };
                if validate || changes.has_fresh() { custom::checked_async(device, submit).await } else { Ok(submit()) }
            })
            .await?;
        if let Some(uploads) = &mut uploads {
            uploads.commit();
        }
        *cache = retained;
        if let Some(backing) = backing {
            *backing_cache = backing;
        }
        let OutputChanges { canvases, backdrops, transitions } = changes;
        canvas_cache.submit(canvases, sample.map(|sample| sample.revision));
        let pending = presentation_history.pending(backdrops, transitions, sample);
        if let Some(timestamps) = timestamps {
            timestamps.submitted(timestamp_pool, queue);
        }
        Ok((submission, pending))
    }
}

impl WgpuRenderer {
    /// Records a display list into the given encoder for this frame's color attachment.
    ///
    /// The target is a single-sample, single-level 2D texture with `RENDER_ATTACHMENT` usage.
    /// Use `Rgba16Float` for [`OutputColor::Linear`](super::OutputColor::Linear) or an RGBA8/BGRA8 format for sRGB output.
    /// Opaque sRGB attachments can be drawn directly. Other sRGB outputs are encoded after
    /// composition in a linear-light attachment. Sampled frames can retain scene pixels
    /// across recordings, including when the host supplies a different drawable on each frame.
    ///
    /// The caller can record earlier commands into the encoder before passing it here. The
    /// recording owns its uploads until submission. Their queue writes execute before all
    /// commands in that submission, including earlier encoder commands. Coordinate access to
    /// the shared queue at submission. Finish any open debug groups first. Author canvases
    /// use separate command buffers in the same submission. A backdrop runs after its group's
    /// preceding draws. Accept the frame's [`PendingPresentation`] to keep backdrop outputs
    /// and rendered transition regions for their mounts.
    /// Returns an error before submitting if required scene textures exceed device or frame limits.
    ///
    /// Giving a profiling callback measures CPU time and, when available, GPU timestamps.
    /// Enable `TIMESTAMP_QUERY` when creating the device for render and compute pass timestamps.
    /// Author passes opt in through [`CanvasFrame::begin_render_pass`](super::CanvasFrame::begin_render_pass) or
    /// [`CanvasFrame::begin_compute_pass`](super::CanvasFrame::begin_compute_pass). `TIMESTAMP_QUERY_INSIDE_ENCODERS` adds the frame's
    /// command span, full canvas and backdrop timings, and backdrop copies when supported.
    /// If the device lacks timestamps or all readback slots are occupied, CPU measurements
    /// still describe the frame. Dropping the frame discards its GPU sample.
    ///
    /// The callback receives the timing results after submission and a successful asynchronous
    /// buffer map. Call [`Self::poll_profile`] to advance pending samples. A failed map or a
    /// dropped renderer can discard a sample. Keep the callback short so it can run during a
    /// device poll or queue call.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn record(
        &mut self,
        encoder: wgpu::CommandEncoder,
        target: FrameTarget<'_>,
        input: FrameInput<'_>,
        report: Option<Box<dyn FnOnce(GpuTimings) + Send>>,
    ) -> Result<(RecordedFrame<'_>, Option<FrameProfile>), RecordError> {
        crate::block_on(self.record_async(encoder, target, input, report))
    }

    /// Records a display list and asynchronously validates any new resources and author programs.
    pub async fn record_async(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        target: FrameTarget<'_>,
        input: FrameInput<'_>,
        report: Option<Box<dyn FnOnce(GpuTimings) + Send>>,
    ) -> Result<(RecordedFrame<'_>, Option<FrameProfile>), RecordError> {
        let Some(report) = report else {
            let recording = FrameRecording { outputs: Some(OutputChanges::default()), ..Default::default() };
            return self.record_frame(encoder, target, input, recording).await.map(|frame| (frame, None));
        };
        let mut measurement = FrameMeasurement::begin(&mut self.timestamps, &self.device, &self.queue, &mut encoder);
        let recording =
            FrameRecording { outputs: Some(OutputChanges::default()), measurement: Some(&mut measurement), ..Default::default() };
        let mut frame = self.record_frame(encoder, target, input, recording).await?;
        frame.timestamps = measurement.timestamps.and_then(|timestamps| timestamps.resolve(frame.device, &mut frame.encoder, report));
        measurement.profile.gpu_sampled = frame.timestamps.is_some();
        Ok((frame, Some(measurement.profile)))
    }

    pub(super) async fn record_frame(
        &mut self,
        encoder: wgpu::CommandEncoder,
        target: FrameTarget<'_>,
        input: FrameInput<'_>,
        mut recording: FrameRecording<'_>,
    ) -> Result<RecordedFrame<'_>, RecordError> {
        let preparing = recording.measurement.is_some().then(Instant::now);
        self.canvas_cache.prune();
        self.presentation_history.prune();
        if let Some(renderer) = &mut self.surface_renderer
            && let Some((commands, receipts)) = record_surface(renderer.as_mut(), &self.device, target, input).await?
        {
            self.prepared = None;
            recording.before.extend([encoder.finish(), commands]);
            return Ok(RecordedFrame {
                before: recording.before,
                encoder: self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara surface output") }),
                device: &self.device,
                queue: &self.queue,
                uploads: None,
                cache: &mut self.prepared,
                retained: None,
                backing_cache: &mut self.backing,
                backing: recording.outputs.is_some().then_some(None),
                canvas_cache: &mut self.canvas_cache,
                presentation_history: &self.presentation_history,
                changes: recording.outputs.unwrap_or_default(),
                sample: input.sample,
                receipts,
                timestamp_pool: &mut self.timestamps,
                timestamps: None,
            });
        }
        let attachment = AttachmentInfo::for_target(target, input.scale, self.device.limits().max_texture_dimension_2d);
        let view = target.texture.create_view(&Default::default());
        let retained_target = RetainedTarget::from(target);
        // Any preparation that may replace buffers or evict atlas slots first drops the plan
        // referring to them. A reused plan keeps the last submission's atlas working set intact.
        let cached = self.prepared.take().and_then(|mut cached| {
            if cached.target != retained_target {
                return None;
            }
            let writes = cached.scroll.update(input.list, input.scale)?;
            Some((cached, writes))
        });
        let reused = cached.is_some();
        let mut full_upload = None;
        let mut spatial_writes = Vec::new();
        let (mut frame, scroll, atlas_bytes) = if let Some((CachedFrame { scroll, frame, atlas_bytes, .. }, writes)) = cached {
            spatial_writes = writes;
            (frame, Some(scroll), atlas_bytes)
        } else {
            // A panic drops any uncommitted atlas slots with the local texture transaction.
            let mut textures = self.textures.take().unwrap_or_default();
            textures.atlases.begin_frame();
            let (frame, data, scroll) = match self.prepare_miss(input, attachment, &mut textures, &mut recording).await {
                Ok(frame) => frame,
                Err(error) => {
                    textures.finish_frame();
                    self.textures = Some(textures);
                    return Err(error);
                }
            };
            let atlas_bytes = textures.atlases.frame_bytes;
            full_upload = Some((textures, data));
            (frame, scroll, atlas_bytes)
        };
        let presenting = recording.outputs.is_some();
        let (mut backing, raster) = if presenting && input.sample.is_some() && frame.plan.supports_damage() && recording.receipts.is_empty()
        {
            let spatial = scroll.is_some();
            let compatible_target = self.backing.as_ref().filter(|backing| backing.target == retained_target);
            let previous = compatible_target.filter(|backing| backing.spatial == spatial);
            let mut scene = SceneSnapshot::new(input.list, input.scale, attachment.requirements.extent);
            let damage = previous.and_then(|backing| {
                let textures = full_upload.as_mut().map(|(textures, _)| textures).or(self.textures.as_mut())?;
                scene.damage_since(&backing.scene, &mut textures.glyphs, spatial)
            });
            let raster = match damage {
                Some(damage) if damage.is_empty() => Raster::Reuse,
                Some(damage) if damage.is_partial(attachment.requirements.extent) => Raster::Damage(damage),
                _ => Raster::Full,
            };
            // A large edit is cheap to draw directly. The next compatible scene can seed a
            // fresh backing; the older snapshot cannot describe the newly submitted pixels.
            let backing = if damage.map_or_else(|| !scene.can_retain(), |damage| !damage.is_partial(attachment.requirements.extent)) {
                None
            } else {
                Some(self.backing_pipelines.prepare(&self.device, attachment, target, scene, compatible_target, spatial))
            };
            (backing, raster)
        } else {
            (None, Raster::Full)
        };
        if let Some(backing) = &mut backing {
            match raster {
                Raster::Full => backing.clear = None,
                Raster::Damage(_) if backing.clear.is_none() => {
                    backing.clear = Some(self.backing_pipelines.prepare_clear(&self.device, attachment.format, frame.background));
                }
                _ => {}
            }
        }
        let intermediate =
            self.output_resolve.prepare(&self.device, (backing.is_none() && frame.plan.resolve_output).then_some(attachment), target);
        let prepared = preparing.map(|start| {
            let end = Instant::now();
            (end, end.duration_since(start))
        });
        let mut encoder = encoder;
        if !matches!(raster, Raster::Reuse) {
            let draw_target = backing.as_ref().map_or_else(|| intermediate.as_ref().unwrap_or(&view), |backing| &backing.view);
            let partial = if let Raster::Damage(damage) = raster {
                Some((damage, backing.as_ref().and_then(|backing| backing.clear.as_ref()).expect("damage clear")))
            } else {
                None
            };
            let offsets = scroll.as_ref().map_or(&[][..], |scroll| scroll.offsets());
            encoder = self.encode_frame(encoder, &mut frame, draw_target, partial, offsets, input, &mut recording).await;
        }
        if let Some(backing) = &backing {
            self.backing_pipelines.transfer(&mut encoder, backing, &view, &mut recording);
        } else if intermediate.is_some() {
            self.output_resolve.encode(&mut encoder, &view, recording.pass(super::GpuPassKind::Output));
            if let Some(measurement) = recording.measurement.as_deref_mut() {
                let [width, height] = attachment.requirements.extent;
                measurement.profile.output_pixels = u64::from(width) * u64::from(height);
            }
        }
        if !presenting {
            self.output_resolve.release_source();
        }
        let atlas_upload_bytes = full_upload.as_ref().map_or(0, |(textures, _)| textures.atlases.upload_bytes);
        let uploads = if let Some((textures, data)) = full_upload {
            self.buffers.stage(self.textures.insert(textures), data)
        } else {
            self.buffers.stage_spatials(spatial_writes)
        };
        if let Some((encoding, prepare)) = prepared
            && let Some(measurement) = recording.measurement
        {
            measurement.profile.retained_prepare = reused;
            measurement.profile.partial_raster = !matches!(raster, Raster::Full);
            measurement.profile.damage_area_ratio = match raster {
                Raster::Full => 1.0,
                Raster::Damage(rect) => rect.area_ratio(attachment.requirements.extent),
                Raster::Reuse => 0.0,
            };
            measurement.profile.instance_upload_bytes = uploads.instance_bytes();
            measurement.profile.spatial_upload_bytes = uploads.spatial_bytes();
            measurement.profile.atlas_bytes = atlas_bytes;
            measurement.profile.atlas_upload_bytes = atlas_upload_bytes;
            measurement.record_frame(prepare, encoding.elapsed(), &frame.plan, &frame.layer_views);
        }
        let retained = scroll.map(|scroll| CachedFrame { scroll, target: retained_target, frame, atlas_bytes });
        Ok(RecordedFrame {
            before: recording.before,
            encoder,
            device: &self.device,
            queue: &self.queue,
            uploads: Some(uploads),
            cache: &mut self.prepared,
            retained,
            backing_cache: &mut self.backing,
            backing: presenting.then_some(backing),
            canvas_cache: &mut self.canvas_cache,
            presentation_history: &self.presentation_history,
            changes: recording.outputs.unwrap_or_default(),
            sample: input.sample,
            receipts: recording.receipts,
            timestamp_pool: &mut self.timestamps,
            timestamps: None,
        })
    }

    async fn prepare_miss(
        &mut self,
        input: FrameInput<'_>,
        attachment: AttachmentInfo,
        textures: &mut SceneTextures,
        recording: &mut FrameRecording<'_>,
    ) -> Result<(PreparedFrame, FrameData, Option<SpatialTable>), RecordError> {
        if let Some((plan, scroll)) = plan_frame_spatial(input.list, attachment.requirements, &mut textures.glyphs) {
            match self.prepare_frame(input, attachment, textures, recording, plan, scroll.offsets().to_vec()).await {
                Ok((frame, data)) => return Ok((frame, data, Some(scroll))),
                Err(_) => {
                    // Unculled geometry can exceed the storage or atlas budget even when the
                    // visible working set fits. Retry with the conventional visible plan.
                    textures.finish_frame();
                    textures.atlases.begin_frame();
                }
            }
        }
        let plan = plan_frame_cached(input.list, attachment.requirements, input.sample, &mut textures.glyphs);
        let (frame, data) = self.prepare_frame(input, attachment, textures, recording, plan, Vec::new()).await?;
        Ok((frame, data, None))
    }
}
