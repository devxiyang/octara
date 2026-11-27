//! The per-device extension boundary for recording a frozen display list.

use super::{FrameInput, FrameTarget, RecordError, custom};
use octara_model::capture::PaintReceipt;
use std::sync::Arc;

/// Creates a fresh renderer for the initial device and for each replacement device.
pub type SurfaceRendererFactory = Arc<dyn Fn(&wgpu::Device) -> Box<dyn SurfaceRenderer> + Send + Sync>;

/// Arguments for recording one frame. The host owns the queue and submits the recorded commands.
pub struct SurfaceFrame<'a> {
    pub device: &'a wgpu::Device,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target: FrameTarget<'a>,
    pub input: FrameInput<'a>,
}

/// Records a display list into the host's output attachment using the requested color encoding.
pub trait SurfaceRenderer: wgpu::WasmNotSendSync {
    /// Return the paint receipts for a recorded frame or `Ok(None)` to use the built-in renderer.
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
