//! Offscreen capture recording, buffer mapping, and row unpacking.

use super::recording::{FrameRecording, RecordedFrame};
use super::{FrameInput, FrameTarget, GpuRenderer, OutputColor, custom};
use octara_model::capture::{PaintReceipt, PaintSample, PaintedPicture};
use octara_model::paint::{DisplayList, Picture};
use std::alloc::Layout;
use std::sync::mpsc;

struct ReadbackLayout {
    row: u32,
    bytes: u64,
    rgba_len: usize,
}

fn readback_layout(width: u32, height: u32) -> Option<ReadbackLayout> {
    if width == 0 || height == 0 {
        return None;
    }
    let count = crate::pixels::count(width, height)?;
    let rgba_len = Layout::array::<[u8; 4]>(count).ok()?.size();
    let row = (u64::from(width) * 4).div_ceil(256) * 256;
    let row = u32::try_from(row).ok()?;
    let bytes = u64::from(row).checked_mul(u64::from(height))?;
    // The mapped buffer must also fit a Rust byte slice on this host.
    Layout::array::<u8>(usize::try_from(bytes).ok()?).ok()?;
    Some(ReadbackLayout { row, bytes, rgba_len })
}

/// An offscreen recording. Dropping it before submission cancels its uploads and readback.
#[must_use = "submit the capture to start its readback"]
pub struct RecordedCapture<'renderer> {
    frame: RecordedFrame<'renderer>,
    pending: PendingReadback,
}

impl RecordedCapture<'_> {
    /// Size of the readback buffer, including row padding.
    pub fn readback_bytes(&self) -> u64 {
        self.pending.pixels.layout.bytes
    }

    /// Registers a callback for completion of this recording's submitted GPU work.
    pub fn on_submitted_work_done(&self, callback: impl FnOnce() + Send + 'static) {
        self.frame.encoder.on_submitted_work_done(callback);
    }

    /// Submits the capture and starts mapping its pixel buffer.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn submit(self) -> Result<Readback, String> {
        crate::block_on(self.submit_async())
    }

    /// Submits the capture, validates the submission and starts mapping its pixel buffer.
    pub async fn submit_async(self) -> Result<Readback, String> {
        let Self { frame, pending } = self;
        let upload_bytes = frame.upload_bytes();
        let (submission, _) = frame.submit_checked(true).await.map_err(|error| format!("capture GPU validation failed: {error}"))?;
        Ok(Readback { submission, upload_bytes, pending: Some(pending) })
    }
}

/// A submitted offscreen frame and its pending pixel readback.
#[must_use = "check the readback for the finished picture"]
pub struct Readback {
    submission: wgpu::SubmissionIndex,
    upload_bytes: u64,
    pending: Option<PendingReadback>,
}

struct PendingReadback {
    mapping: mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    pixels: MappedReadback,
}

/// A capture whose buffer is mapped and can be unpacked without polling the GPU.
pub struct MappedReadback {
    buffer: wgpu::Buffer,
    layout: ReadbackLayout,
    width: u32,
    height: u32,
    effects: Vec<PaintReceipt>,
}

impl Readback {
    /// Index of the submitted readback commands.
    pub fn submission(&self) -> &wgpu::SubmissionIndex {
        &self.submission
    }

    /// Logical bytes uploaded by this capture's GPU submission.
    pub fn upload_bytes(&self) -> u64 {
        self.upload_bytes
    }

    /// Takes the mapped buffer or error once available. The owner drives mapping callbacks by
    /// polling the device or submitting more work. Returns `None` while pending or after a
    /// previous call consumed the result.
    pub fn take_mapped(&mut self) -> Option<Result<MappedReadback, String>> {
        let mapped = match self.pending.as_ref()?.mapping.try_recv() {
            Ok(result) => result.map_err(|error| format!("capture readback mapping failed: {error}")),
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(error) => Err(format!("capture readback callback did not report completion: {error}")),
        };
        let pending = self.pending.take().expect("the readback was pending");
        Some(mapped.map(|()| pending.pixels))
    }
}

impl MappedReadback {
    pub fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    pub fn finish(self) -> Result<PaintedPicture, String> {
        let Self { buffer, layout, width, height, effects } = self;
        let data = buffer.get_mapped_range(..).map_err(|e| format!("capture readback buffer access failed: {e}"))?;
        let mut rgba = Vec::with_capacity(layout.rgba_len);
        for line in data.chunks_exact(layout.row as usize) {
            rgba.extend_from_slice(&line[..width as usize * 4]);
        }
        drop(data);
        buffer.unmap();
        Ok(PaintedPicture { picture: Picture::new(width, height, rgba).expect("checked capture dimensions"), effects })
    }
}

impl GpuRenderer {
    /// Renders offscreen and reads the pixels back (tests, screenshots, a window's captures), in
    /// straight-alpha sRGB RGBA8. Each axis has a one-pixel minimum.
    ///
    /// # Panics
    /// If the dimensions or scale are invalid, or the frame cannot be recorded or read back.
    /// [`Self::paint`] returns an error in these cases.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render(&mut self, dl: &DisplayList, scale: f32) -> Picture {
        self.try_render(dl, scale, None).expect("GPU rendering failed").picture
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn try_render(&mut self, dl: &DisplayList, scale: f32, sample: Option<PaintSample>) -> Result<PaintedPicture, String> {
        let mut readback = self.record_capture(dl, scale, sample)?.submit()?;
        self.device
            .poll(wgpu::PollType::Wait { submission_index: Some(readback.submission().clone()), timeout: None })
            .map_err(|e| format!("capture GPU poll failed: {e}"))?;
        readback.take_mapped().ok_or("capture readback callback did not report completion")??.finish()
    }

    /// Records an offscreen frame and its copy to a pixel buffer. The caller can cancel the
    /// recording while it is still on the CPU. A submitted readback owns its pixel buffer.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn record_capture(&mut self, list: &DisplayList, scale: f32, sample: Option<PaintSample>) -> Result<RecordedCapture<'_>, String> {
        self.record_capture_with_wake(list, scale, sample, || {})
    }

    /// Records a capture and calls `wake` when the pixel mapping reports completion.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn record_capture_with_wake(
        &mut self,
        list: &DisplayList,
        scale: f32,
        sample: Option<PaintSample>,
        wake: impl FnOnce() + Send + 'static,
    ) -> Result<RecordedCapture<'_>, String> {
        crate::block_on(self.record_capture_async(list, scale, sample, wake))
    }

    /// Records an offscreen drawing and schedules a signal once its pixels are mapped.
    pub async fn record_capture_async(
        &mut self,
        list: &DisplayList,
        scale: f32,
        sample: Option<PaintSample>,
        wake: impl FnOnce() + Send + 'static,
    ) -> Result<RecordedCapture<'_>, String> {
        let (width, height) = crate::pixels::dimensions((list.size.width, list.size.height), scale)?;
        let limits = self.device.limits();
        if width > limits.max_texture_dimension_2d || height > limits.max_texture_dimension_2d {
            return Err(format!("capture {width}×{height} exceeds the GPU texture dimension limit {}", limits.max_texture_dimension_2d));
        }
        let layout = readback_layout(width, height).ok_or("capture readback exceeds addressable memory or row limits")?;
        if layout.bytes > limits.max_buffer_size {
            return Err(format!("capture readback exceeds the GPU buffer limit of {} bytes", limits.max_buffer_size));
        }
        // Validate the allocation before recording. Keep scopes local to immediate device
        // operations so another task's WebGPU commands cannot enter this capture's scope.
        let (target, buffer, encoder) = custom::checked_async(&self.device, || {
            let target = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("octara offscreen"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("octara readback"),
                size: layout.bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let encoder = self.device.create_command_encoder(&Default::default());
            (target, buffer, encoder)
        })
        .await
        .map_err(|error| format!("capture GPU allocation failed: {error}"))?;
        let input = FrameInput { list, scale, sample };
        let output = FrameTarget { texture: &target, color: OutputColor::SrgbStraight };
        let mut frame = self.record_frame(encoder, output, input, FrameRecording::default()).await.map_err(|error| error.to_string())?;
        let device = frame.device;
        let (mapped, mapping) = mpsc::channel();
        custom::checked_async(device, || {
            frame.encoder().copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(layout.row), rows_per_image: Some(height) },
                },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
            frame.encoder().map_buffer_on_submit(&buffer, wgpu::MapMode::Read, .., move |result| {
                let _ = mapped.send(result);
                wake();
            });
        })
        .await
        .map_err(|error| format!("capture GPU copy failed: {error}"))?;
        let effects = std::mem::take(&mut frame.receipts);
        let pixels = MappedReadback { buffer, layout, width, height, effects };
        Ok(RecordedCapture { frame, pending: PendingReadback { mapping, pixels } })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String> {
        self.try_render(list, scale, None).map(|painted| painted.picture)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn paint_with_receipts(&mut self, list: &DisplayList, scale: f32, sample: Option<PaintSample>) -> Result<PaintedPicture, String> {
        self.try_render(list, scale, sample)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::readback_layout;

    #[test]
    fn readback_layout_aligns_rows_and_checks_allocation_without_allocating() {
        assert!(readback_layout(0, 1).is_none());
        assert!(readback_layout(1, 0).is_none());
        let layout = readback_layout(65, 3).unwrap();
        assert_eq!((layout.row, layout.bytes, layout.rgba_len), (512, 1536, 65 * 3 * 4));
        assert!(readback_layout(u32::MAX / 4, 1).is_none(), "row padding must fit bytes_per_row");
        assert!(readback_layout(u32::MAX, u32::MAX).is_none());
        if usize::BITS == 64 {
            let layout = readback_layout(65536, 65536).unwrap();
            assert_eq!(layout.row, 262144);
            assert_eq!(layout.bytes, 1_u64 << 34);
            assert_eq!(layout.rgba_len, usize::try_from(1_u64 << 34).unwrap());
        } else {
            assert!(readback_layout(65536, 65536).is_none());
        }
    }
}
