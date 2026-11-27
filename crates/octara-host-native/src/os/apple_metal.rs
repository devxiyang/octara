//! Access to the renderer's Core Animation presentation layer.

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
#[cfg(target_os = "macos")]
use octara_render::wgpu;

#[cfg(target_os = "macos")]
pub(super) fn metal_layer(surface: &wgpu::Surface<'static>) -> Option<Retained<CAMetalLayer>> {
    // SAFETY: The HAL guard keeps the surface alive while we retain its layer.
    Some(unsafe { surface.as_hal::<wgpu::hal::api::Metal>() }?.render_layer().lock().clone())
}

pub(super) fn set_transactional(layer: &CAMetalLayer, transactional: bool) {
    if layer.presentsWithTransaction() != transactional {
        layer.setPresentsWithTransaction(transactional);
    }
}
