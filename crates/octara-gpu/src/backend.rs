//! Device and surface lifecycle used by the native host.

use crate::{CapturePixels, DeviceGeneration, DrawWork, DrawnFrame, GpuError, GpuTask};
use octara_model::capture::PaintSample;
use octara_model::paint::DisplayList;
use octara_render::OutputColor;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::sync::Arc;
use std::time::Instant;

/// Platform presentation support for the selected adapter, or for the backend before device selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendCapabilities {
    pub metal: bool,
}

/// Creates a device and its compatible native surface sessions.
pub trait GpuBackend {
    type Device: GpuDevice;
    type Surface: SurfaceSession<Device = Self::Device> + Clone;
    type RendererFactory;

    fn capabilities(&self) -> BackendCapabilities;

    /// Open a session on a native window.
    ///
    /// # Safety
    ///
    /// Both handles must be valid and remain valid until every clone of the surface and
    /// every drawable acquired from it have been released.
    unsafe fn create_surface(&self, display: RawDisplayHandle, window: RawWindowHandle) -> Result<Self::Surface, String>;

    fn create_device(
        &self,
        compatible_surface: &Self::Surface,
        generation: Arc<DeviceGeneration>,
        profile: bool,
        surface_renderer: Option<&Self::RendererFactory>,
        thread_setup: fn(),
    ) -> Result<Self::Device, String>;
}

pub type DeviceDrawnFrame<D> = DrawnFrame<Box<<D as GpuDevice>::Drawable>, <D as GpuDevice>::Presentation>;

pub trait GpuDevice: Sized {
    type Drawable: DrawableTarget + ?Sized;
    type SurfaceLease: Send + 'static;
    type Presentation: Send + 'static;

    fn capabilities(&self) -> BackendCapabilities;
    fn generation(&self) -> &Arc<DeviceGeneration>;
    fn draw(&self, work: DrawWork<Box<Self::Drawable>>) -> Result<GpuTask<DeviceDrawnFrame<Self>>, GpuError>;
    fn drain_surfaces(&self) -> Result<GpuTask<Self::SurfaceLease>, GpuError>;
    /// Make an accepted frame's retained output available to subsequent draws.
    fn accept_presentation(&self, presentation: Self::Presentation) -> Result<(), GpuError>;
    fn capture(&self, list: Arc<DisplayList>, scale: f32, sample: Option<PaintSample>) -> Result<GpuTask<CapturePixels>, GpuError>;

    /// Wait for the worker to release its drawables before the host drops the native windows.
    fn shutdown(self);
}

pub trait SurfaceSession {
    type Device: GpuDevice;

    /// Whether the surface can change its drawable configuration while submitted work finishes.
    fn supports_configure_without_drain(&self) -> bool {
        false
    }

    /// Reconfigure in place if the backend manages drawable allocation independently of the queue.
    fn configure_without_drain(&self, _device: &Self::Device, _size: [u32; 2], _transparent: bool) -> Option<Result<OutputColor, String>> {
        None
    }

    /// Configure the surface using the lease returned by the device's drain operation.
    fn configure(
        &self,
        device: &Self::Device,
        lease: &<Self::Device as GpuDevice>::SurfaceLease,
        size: [u32; 2],
        transparent: bool,
    ) -> Result<OutputColor, String>;

    fn acquire(&self, color: OutputColor) -> DrawableAcquisition<<Self::Device as GpuDevice>::Drawable>;

    fn acquire_with_transaction(
        &self,
        color: OutputColor,
        _transactional: bool,
    ) -> DrawableAcquisition<<Self::Device as GpuDevice>::Drawable> {
        self.acquire(color)
    }
}

/// A drawable travels to the worker for rendering and returns to the host for frame acceptance.
pub trait DrawableTarget: Send + 'static {
    /// Complete any remaining presentation work on the host.
    fn present(self: Box<Self>) -> Result<(), String>;

    fn report_stages(&mut self, _tick_at: Option<Instant>, _cpu_done_at: Instant, _report: FrameStageReport) {}

    fn suboptimal(&self) -> bool {
        false
    }
}

/// CPU and hardware timestamps for a presented frame. Hardware times use the platform media clock.
#[derive(Clone, Copy, Debug)]
pub struct FrameStageTimings {
    pub tick_at: Option<Instant>,
    pub cpu_done_at: Instant,
    pub encode_done_at: Instant,
    pub clock_at: Instant,
    pub clock_media_s: f64,
    pub gpu_start_s: f64,
    pub gpu_end_s: f64,
    /// Actual display time, when the drawable reports one.
    pub presented_at_s: Option<f64>,
}

/// Report CPU, GPU, and display timestamps once rendering and presentation have finished.
pub type FrameStageReport = Box<dyn FnOnce(FrameStageTimings) + Send>;

pub enum DrawableAcquisition<T: DrawableTarget + ?Sized> {
    Ready(Box<T>),
    WaitingForClock,
    Retry,
    Reconfigure,
    Lost,
    Suspended,
}
