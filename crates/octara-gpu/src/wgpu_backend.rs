//! Native wgpu devices and surface sessions.

use crate::{
    BackendCapabilities, CapturePixels, DeviceDrawnFrame, DeviceExecutor, DeviceGeneration, DrawTarget, DrawWork, DrawableAcquisition,
    DrawableTarget, GpuBackend, GpuDevice, GpuError, GpuTask, SurfaceLease, SurfaceRendererFactory, SurfaceSession,
};
use octara_model::capture::PaintSample;
use octara_model::paint::DisplayList;
use octara_render::gpu::PendingPresentation;
use octara_render::{OutputColor, wgpu};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::sync::Arc;

#[derive(Default)]
pub struct WgpuBackend {
    instance: wgpu::Instance,
}

impl GpuBackend for WgpuBackend {
    type Device = WgpuDevice;
    type Surface = WgpuSurfaceSession;
    type RendererFactory = SurfaceRendererFactory;

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { metal: wgpu::Instance::enabled_backend_features().contains(wgpu::Backends::METAL) }
    }

    unsafe fn create_surface(&self, display: RawDisplayHandle, window: RawWindowHandle) -> Result<WgpuSurfaceSession, String> {
        // SAFETY: The caller keeps both native handles valid for the session and its drawables.
        let surface = unsafe {
            self.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(display),
                raw_window_handle: window,
            })
        }
        .map_err(|error| error.to_string())?;
        Ok(WgpuSurfaceSession { surface: Arc::new(surface) })
    }

    fn create_device(
        &self,
        compatible_surface: &WgpuSurfaceSession,
        generation: Arc<DeviceGeneration>,
        profile: bool,
        surface_renderer: Option<&SurfaceRendererFactory>,
        thread_setup: fn(),
    ) -> Result<WgpuDevice, String> {
        octara_render::block_on(async {
            let adapter = self
                .instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&compatible_surface.surface),
                    ..Default::default()
                })
                .await
                .map_err(|error| error.to_string())?;
            let required_features = if profile { adapter.features() & wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() };
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Octara native device"),
                    required_features,
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .map_err(|error| error.to_string())?;
            let executor = DeviceExecutor::spawn_for_device_with_thread_setup(device, queue, generation, surface_renderer, thread_setup)
                .map_err(|error| error.to_string())?;
            Ok(WgpuDevice { adapter, executor })
        })
    }
}

pub struct WgpuDevice {
    adapter: wgpu::Adapter,
    executor: DeviceExecutor,
}

impl GpuDevice for WgpuDevice {
    type Drawable = dyn WgpuDrawableTarget;
    type SurfaceLease = SurfaceLease;
    type Presentation = PendingPresentation;

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { metal: self.adapter.get_info().backend == wgpu::Backend::Metal }
    }

    fn generation(&self) -> &Arc<DeviceGeneration> {
        self.executor.generation()
    }

    fn draw(&self, work: DrawWork<Box<Self::Drawable>>) -> Result<GpuTask<DeviceDrawnFrame<Self>>, GpuError> {
        self.executor.draw(work)
    }

    fn drain_surfaces(&self) -> Result<GpuTask<SurfaceLease>, GpuError> {
        self.executor.drain_surfaces()
    }

    fn accept_presentation(&self, presentation: PendingPresentation) -> Result<(), GpuError> {
        self.executor.accept_presentation(presentation)
    }

    fn capture(&self, list: Arc<DisplayList>, scale: f32, sample: Option<PaintSample>) -> Result<GpuTask<CapturePixels>, GpuError> {
        self.executor.capture(list, scale, sample)
    }

    fn shutdown(self) {
        self.executor.shutdown();
    }
}

#[derive(Clone)]
pub struct WgpuSurfaceSession {
    surface: Arc<wgpu::Surface<'static>>,
}

#[cfg(target_os = "macos")]
impl WgpuSurfaceSession {
    pub fn metal_layer(&self) -> Option<objc2::rc::Retained<objc2_quartz_core::CAMetalLayer>> {
        // SAFETY: The HAL guard keeps the surface alive while we retain its layer.
        Some(unsafe { self.surface.as_hal::<wgpu::hal::api::Metal>() }?.render_layer().lock().clone())
    }
}

impl SurfaceSession for WgpuSurfaceSession {
    type Device = WgpuDevice;

    fn configure(
        &self,
        device: &WgpuDevice,
        lease: &SurfaceLease,
        [width, height]: [u32; 2],
        transparent: bool,
    ) -> Result<OutputColor, String> {
        if !device.executor.owns_surface_lease(lease) {
            return Err("the surface lease belongs to a different device".into());
        }
        let mut config = self.surface.get_default_config(&device.adapter, width, height).ok_or("no compatible surface format")?;
        let supported = self.surface.get_capabilities(&device.adapter);
        if let Some(format) = supported.formats.iter().find(|format| format.is_srgb()) {
            config.format = *format;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.usage |= supported.usages & (wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST);
        if let Some(alpha) = preferred_alpha_mode(transparent, &supported.alpha_modes) {
            config.alpha_mode = alpha;
        }
        let color = match config.alpha_mode {
            wgpu::CompositeAlphaMode::PreMultiplied => OutputColor::SrgbPremultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied => OutputColor::SrgbStraight,
            _ => OutputColor::SrgbOpaque,
        };
        self.surface.configure(lease.device(), &config);
        Ok(color)
    }

    fn acquire(&self, color: OutputColor) -> DrawableAcquisition<dyn WgpuDrawableTarget> {
        use wgpu::CurrentSurfaceTexture as Status;

        let (frame, suboptimal) = match self.surface.get_current_texture() {
            Status::Success(frame) => (frame, false),
            Status::Suboptimal(frame) => (frame, true),
            Status::Timeout | Status::Occluded => return DrawableAcquisition::Retry,
            Status::Outdated => return DrawableAcquisition::Reconfigure,
            Status::Lost | Status::Validation => return DrawableAcquisition::Lost,
        };
        DrawableAcquisition::Ready(Box::new(SurfaceDrawable { frame, _surface: self.clone(), queue: None, color, suboptimal }))
    }
}

/// A native drawable the wgpu worker can import or render into directly.
pub trait WgpuDrawableTarget: DrawableTarget + DrawTarget {}
impl<T: DrawableTarget + DrawTarget> WgpuDrawableTarget for T {}

impl DrawTarget for Box<dyn WgpuDrawableTarget> {
    fn texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(wgpu::Texture, OutputColor), String> {
        self.as_mut().texture(device, queue)
    }
}

struct SurfaceDrawable {
    frame: wgpu::SurfaceTexture,
    _surface: WgpuSurfaceSession,
    queue: Option<wgpu::Queue>,
    color: OutputColor,
    suboptimal: bool,
}

impl DrawTarget for SurfaceDrawable {
    fn texture(&mut self, _: &wgpu::Device, queue: &wgpu::Queue) -> Result<(wgpu::Texture, OutputColor), String> {
        self.queue = Some(queue.clone());
        Ok((self.frame.texture.clone(), self.color))
    }
}

impl DrawableTarget for SurfaceDrawable {
    fn present(self: Box<Self>) -> Result<(), String> {
        let queue = self.queue.ok_or("the drawable was not rendered")?;
        queue.present(self.frame);
        Ok(())
    }

    fn suboptimal(&self) -> bool {
        self.suboptimal
    }
}

fn preferred_alpha_mode(transparent: bool, modes: &[wgpu::CompositeAlphaMode]) -> Option<wgpu::CompositeAlphaMode> {
    use wgpu::CompositeAlphaMode::*;

    let preference = if transparent { &[PreMultiplied, PostMultiplied][..] } else { &[Opaque] };
    preference.iter().copied().find(|mode| modes.contains(mode))
}

#[cfg(test)]
mod tests {
    use super::preferred_alpha_mode;
    use octara_render::wgpu::CompositeAlphaMode::*;

    #[test]
    fn surface_alpha_prefers_premultiplied_for_transparent_windows() {
        let supported = [PostMultiplied, Opaque, PreMultiplied];
        assert_eq!(preferred_alpha_mode(false, &supported), Some(Opaque));
        assert_eq!(preferred_alpha_mode(true, &supported), Some(PreMultiplied));
        assert_eq!(preferred_alpha_mode(true, &[Opaque, PostMultiplied]), Some(PostMultiplied));
        assert_eq!(preferred_alpha_mode(true, &[Opaque]), None);
    }
}
