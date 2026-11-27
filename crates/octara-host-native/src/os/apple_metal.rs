//! Access to the renderer's Core Animation presentation layer.

use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
use octara_render::wgpu;

pub(super) fn metal_layer(surface: &wgpu::Surface<'static>) -> Option<Retained<CAMetalLayer>> {
    // SAFETY: The HAL guard keeps the surface alive while we retain its layer.
    Some(unsafe { surface.as_hal::<wgpu::hal::api::Metal>() }?.render_layer().lock().clone())
}

pub(super) fn set_transactional(layer: &CAMetalLayer, transactional: bool) {
    if layer.presentsWithTransaction() != transactional {
        layer.setPresentsWithTransaction(transactional);
    }
}
