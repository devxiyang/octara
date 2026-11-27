//! Frame recording and submission with staged uploads and presentation changes.

use super::outputs::{CanvasCache, OutputChanges, PendingPresentation, PresentationHistory};
use super::profile::{FrameMeasurement, FrameProfile, GpuTimings, Instant, PendingTimestamps, TimestampPool};
use super::resolve::AttachmentInfo;
use super::surface::record_surface;
use super::upload::FrameUploads;
use super::{FrameInput, FrameTarget, GpuRenderer, RecordError, custom};
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
    /// [`GpuRenderer::accept_presentation`] once the host accepts the frame.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn submit_for_presentation(self) -> (wgpu::SubmissionIndex, PendingPresentation) {
        pollster::block_on(self.submit_for_presentation_async()).unwrap_or_else(|error| panic!("Octara frame submission failed: {error}"))
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
        let OutputChanges { canvases, backdrops, transitions } = changes;
        canvas_cache.submit(canvases, sample.map(|sample| sample.revision));
        let pending = presentation_history.pending(backdrops, transitions, sample);
        if let Some(timestamps) = timestamps {
            timestamps.submitted(timestamp_pool, queue);
        }
        Ok((submission, pending))
    }
}

impl GpuRenderer {
    /// Records a display list into the given encoder for this frame's color attachment.
    ///
    /// The target is a single-sample, single-level 2D texture with `RENDER_ATTACHMENT` usage.
    /// Use `Rgba16Float` for [`OutputColor::Linear`](super::OutputColor::Linear) or an RGBA8/BGRA8 format for sRGB output.
    /// Opaque sRGB attachments can be drawn directly. Other sRGB outputs are encoded after
    /// composition in a linear-light attachment.
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
        pollster::block_on(self.record_async(encoder, target, input, report))
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
            recording.before.extend([encoder.finish(), commands]);
            return Ok(RecordedFrame {
                before: recording.before,
                encoder: self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara surface output") }),
                device: &self.device,
                queue: &self.queue,
                uploads: None,
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
        // Keep the texture transaction local until recording succeeds. A panic during recording
        // drops the uncommitted cache along with the rest of the frame.
        let mut textures = self.textures.take().unwrap_or_default();
        textures.atlases.begin_frame();
        let mut frame = match self.prepare_frame(input, attachment, &mut textures, &mut recording).await {
            Ok(frame) => frame,
            Err(error) => {
                textures.finish_frame();
                self.textures = Some(textures);
                return Err(error);
            }
        };
        if let Some(measurement) = recording.measurement.as_deref_mut() {
            measurement.profile.atlas_bytes = textures.atlases.frame_bytes;
            measurement.profile.atlas_upload_bytes = textures.atlases.upload_bytes;
        }
        let intermediate = self.output_resolve.prepare(&self.device, frame.plan.resolve_output.then_some(attachment), target);
        let prepared = preparing.map(|start| {
            let end = Instant::now();
            (end, end.duration_since(start))
        });
        let encoder = self.encode_frame(encoder, &mut frame, &view, intermediate.as_ref(), input, &mut recording).await;
        if let Some((encoding, prepare)) = prepared
            && let Some(measurement) = recording.measurement
        {
            measurement.record_frame(prepare, encoding.elapsed(), &frame.plan, &frame.layer_views);
        }
        let textures = self.textures.insert(textures);
        let uploads = self.buffers.stage(textures, frame.data);
        Ok(RecordedFrame {
            before: recording.before,
            encoder,
            device: &self.device,
            queue: &self.queue,
            uploads: Some(uploads),
            canvas_cache: &mut self.canvas_cache,
            presentation_history: &self.presentation_history,
            changes: recording.outputs.unwrap_or_default(),
            sample: input.sample,
            receipts: recording.receipts,
            timestamp_pool: &mut self.timestamps,
            timestamps: None,
        })
    }
}
