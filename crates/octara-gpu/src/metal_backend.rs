//! Native Metal surfaces and their command queue worker.

mod capture;
mod drawable;
mod worker;

pub use drawable::MetalDrawableTarget;
pub use worker::{MetalGpuDevice, MetalSurfaceLease};

use crate::{BackendCapabilities, DeviceGeneration, DrawableAcquisition, GpuBackend, SurfaceSession};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, msg_send};
use objc2_core_foundation::CGSize;
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
use objc2_metal::{MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer, CATransaction};
use octara_render::OutputColor;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::convert::Infallible;
use std::sync::Arc;

#[derive(Default)]
pub struct MetalBackend;

impl GpuBackend for MetalBackend {
    type Device = MetalGpuDevice;
    type Surface = MetalSurfaceSession;
    type RendererFactory = Infallible;

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { metal: true }
    }

    unsafe fn create_surface(&self, display: RawDisplayHandle, window: RawWindowHandle) -> Result<MetalSurfaceSession, String> {
        let _main = MainThreadMarker::new().ok_or("Metal windows must be bound on the main thread")?;
        let view = match (display, window) {
            #[cfg(target_os = "macos")]
            (RawDisplayHandle::AppKit(_), RawWindowHandle::AppKit(window)) => window.ns_view,
            #[cfg(target_os = "ios")]
            (RawDisplayHandle::UiKit(_), RawWindowHandle::UiKit(window)) => window.ui_view,
            _ => return Err("Metal requires an Apple view".into()),
        };
        // SAFETY: The handles name a live NSView or UIView on its main thread. Both expose -layer.
        let layer: Option<Retained<AnyObject>> = unsafe { msg_send![view.cast::<AnyObject>().as_ref(), layer] };
        let layer = layer.and_then(|layer| layer.downcast::<CAMetalLayer>().ok()).ok_or("the window view has no CAMetalLayer")?;
        Ok(MetalSurfaceSession::new(layer))
    }

    fn create_device(
        &self,
        _compatible_surface: &MetalSurfaceSession,
        generation: Arc<DeviceGeneration>,
        profile: bool,
        _surface_renderer: Option<&Infallible>,
        thread_setup: fn(),
    ) -> Result<MetalGpuDevice, String> {
        MetalGpuDevice::new(generation, profile, thread_setup)
    }
}

#[derive(Clone)]
pub struct MetalSurfaceSession {
    layer: Retained<CAMetalLayer>,
}

impl MetalSurfaceSession {
    pub fn new(layer: Retained<CAMetalLayer>) -> Self {
        Self { layer }
    }

    pub fn metal_layer(&self) -> Option<Retained<CAMetalLayer>> {
        Some(self.layer.clone())
    }

    fn configure_layer(&self, device: &MetalGpuDevice, [width, height]: [u32; 2], transparent: bool) -> Result<OutputColor, String> {
        let _main = MainThreadMarker::new().ok_or("Metal surfaces must be configured on the main thread")?;
        let layer = &self.layer;
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let _transaction = std::mem::DropGuard::new((), |()| CATransaction::commit());
        if !layer.device().is_some_and(|current| std::ptr::eq(&*current, device.raw_device())) {
            layer.setDevice(Some(device.raw_device()));
        }
        if layer.pixelFormat() != MTLPixelFormat::BGRA8Unorm_sRGB {
            layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm_sRGB);
        }
        if layer.framebufferOnly() {
            layer.setFramebufferOnly(false);
        }
        if layer.maximumDrawableCount() != 2 {
            layer.setMaximumDrawableCount(2);
        }
        if layer.colorspace().is_none() {
            // SAFETY: Core Graphics owns this constant for the lifetime of the process.
            let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
            layer.setColorspace(srgb.as_deref());
        }
        if layer.isOpaque() == transparent {
            layer.setOpaque(!transparent);
        }
        let size = CGSize::new(f64::from(width), f64::from(height));
        if layer.drawableSize() != size {
            layer.setDrawableSize(size);
        }
        Ok(if transparent { OutputColor::SrgbPremultiplied } else { OutputColor::SrgbOpaque })
    }
}

impl SurfaceSession for MetalSurfaceSession {
    type Device = MetalGpuDevice;

    fn supports_configure_without_drain(&self) -> bool {
        true
    }

    fn configure_without_drain(&self, device: &MetalGpuDevice, size: [u32; 2], transparent: bool) -> Option<Result<OutputColor, String>> {
        Some(self.configure_layer(device, size, transparent))
    }

    fn configure(
        &self,
        device: &MetalGpuDevice,
        lease: &MetalSurfaceLease,
        size: [u32; 2],
        transparent: bool,
    ) -> Result<OutputColor, String> {
        if !device.owns_surface_lease(lease) {
            return Err("the surface lease belongs to a different device".into());
        }
        self.configure_layer(device, size, transparent)
    }

    fn acquire(&self, color: OutputColor) -> DrawableAcquisition<MetalDrawableTarget> {
        self.acquire_with_transaction(color, self.layer.presentsWithTransaction())
    }

    fn acquire_with_transaction(&self, color: OutputColor, transactional: bool) -> DrawableAcquisition<MetalDrawableTarget> {
        let layer = &self.layer;
        if layer.device().is_none() {
            return DrawableAcquisition::Reconfigure;
        }
        let size = layer.drawableSize();
        if size.width == 0.0 || size.height == 0.0 {
            return DrawableAcquisition::Suspended;
        }
        if layer.presentsWithTransaction() != transactional {
            layer.setPresentsWithTransaction(transactional);
        }
        let Some(drawable) = layer.nextDrawable() else { return DrawableAcquisition::Retry };
        let texture = drawable.texture();
        if texture.width() != size.width as usize || texture.height() != size.height as usize {
            return DrawableAcquisition::Retry;
        }
        DrawableAcquisition::Ready(Box::new(MetalDrawableTarget::new(drawable, color, transactional)))
    }
}
