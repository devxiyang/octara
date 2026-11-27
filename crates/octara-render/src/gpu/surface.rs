//! The per-device extension boundary for recording a frozen display list.

use super::{FrameInput, FrameTarget, RecordError, custom};
use octara_model::capture::PaintReceipt;
use std::sync::Arc;

/// Creates a fresh renderer for each device, including replacements after device loss.
/// On native hosts, the resulting renderer can move to the device's worker thread.
pub type SurfaceRendererFactory = Arc<dyn Fn(&wgpu::Device) -> Box<dyn SurfaceRenderer> + Send + Sync>;

/// Arguments for recording one frame. The host owns the queue and submits the recorded commands.
pub struct SurfaceFrame<'a> {
    /// The same device that created the renderer and owns the output texture.
    pub device: &'a wgpu::Device,
    /// Records uploads and draws for this frame, subject to host submission.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The output texture and its required color representation.
    pub target: FrameTarget<'a>,
    /// The frozen display list at the host's drawing scale and sample time.
    pub input: FrameInput<'a>,
}

/// Records a display list into the host's output attachment using the requested color encoding.
pub trait SurfaceRenderer: wgpu::WasmNotSendSync {
    /// Return paint receipts in display-list order for the author draws this frame records.
    /// `Ok(None)` discards this encoder and asks the built-in renderer to record the frame.
    /// `Err` reports a recording failure to the host.
    /// The host may record the same input again or discard commands before submission.
    /// Record uploads and rendering in the supplied encoder. Keep its resources usable through
    /// submission and avoid depending on a previously recorded frame having been submitted.
    fn record(&mut self, frame: SurfaceFrame<'_>) -> Result<Option<Vec<PaintReceipt>>, String>;
}

pub(super) async fn record_surface(
    renderer: &mut dyn SurfaceRenderer,
    device: &wgpu::Device,
    target: FrameTarget<'_>,
    input: FrameInput<'_>,
) -> Result<Option<(wgpu::CommandBuffer, Vec<PaintReceipt>)>, RecordError> {
    custom::checked_async(device, || {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("octara surface renderer") });
        let receipts = renderer.record(SurfaceFrame { device, encoder: &mut encoder, target, input })?;
        Ok(receipts.map(|receipts| (encoder.finish(), receipts)))
    })
    .await
    .map_err(|error| RecordError::SurfaceRenderer(error.to_string()))?
    .map_err(RecordError::SurfaceRenderer)
}
