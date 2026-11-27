//! Imports and presents an AppKit drawable on the renderer’s Metal queue.

use crate::os::Drawable;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDrawable, MTLHazardTrackingMode, MTLLoadAction, MTLPixelFormat,
    MTLRenderPassDescriptor, MTLResource, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureType, MTLTextureUsage,
};
use objc2_quartz_core::CAMetalDrawable;
use octara_gpu::DrawTarget;
use octara_render::{gpu::OutputColor, wgpu};
use wgpu::hal::api::Metal;

pub(super) struct MetalFrame {
    drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
    queue: Option<wgpu::Queue>,
    size: (u32, u32),
    color: OutputColor,
    transactional: bool,
}

// SAFETY: One retained drawable is handed from the acquirer to the presenter by value. Only
// its owner can use it. This matches the lifetime of wgpu-hal's retained Metal SurfaceTexture.
unsafe impl Send for MetalFrame {}

impl MetalFrame {
    pub(super) fn new(
        drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
        size: (u32, u32),
        color: OutputColor,
        transactional: bool,
    ) -> Self {
        Self { drawable, queue: None, size, color, transactional }
    }
}

impl DrawTarget for MetalFrame {
    fn texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(wgpu::Texture, OutputColor), String> {
        let expected_size = self.size;
        if self.queue.is_some() {
            return Err("the drawable was already imported".into());
        }

        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        autoreleasepool(|_| {
            let raw = self.drawable.texture();
            let usage = raw.usage();
            if raw.pixelFormat() != MTLPixelFormat::BGRA8Unorm_sRGB
                || raw.textureType() != MTLTextureType::Type2D
                || raw.width() != expected_size.0 as usize
                || raw.height() != expected_size.1 as usize
                || raw.depth() != 1
                || raw.arrayLength() != 1
                || raw.mipmapLevelCount() != 1
                || raw.sampleCount() != 1
                || raw.isFramebufferOnly()
                || (usage != MTLTextureUsage::Unknown && !usage.contains(MTLTextureUsage::RenderTarget))
                || raw.storageMode() == MTLStorageMode::Memoryless
                || raw.hazardTrackingMode() != MTLHazardTrackingMode::Tracked
            {
                return Err("the Metal drawable texture does not match the window target".into());
            }

            {
                // SAFETY: Both guards are used to inspect retained resources and enqueue a clear.
                let hal_device = unsafe { device.as_hal::<Metal>() }.ok_or("the wgpu device is not Metal")?;
                // SAFETY: Commands use this Metal queue while its wgpu guard keeps it alive.
                let hal_queue = unsafe { queue.as_hal::<Metal>() }.ok_or("the wgpu queue is not Metal")?;
                let native_queue = hal_queue.as_raw();
                let texture_device = raw.device();
                if !std::ptr::eq(&*texture_device, &**hal_device.raw_device()) || !std::ptr::eq(&*texture_device, &*native_queue.device()) {
                    return Err("the drawable, renderer and queue use different Metal devices".into());
                }
                clear(native_queue, &raw)?;
            }

            let size = wgpu::Extent3d { width: expected_size.0, height: expected_size.1, depth_or_array_layers: 1 };
            let descriptor = wgpu::TextureDescriptor {
                label: Some("Octara Metal drawable"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            };
            // SAFETY: The drawable owns this texture, whose device and full descriptor were
            // checked above. Metal orders accesses to the bound, hazard-tracked texture on
            // this queue after the stored clear. wgpu's first and later uses stay on that queue.
            let texture = unsafe {
                let raw = wgpu::hal::metal::Device::texture_from_raw(raw, format, MTLTextureType::Type2D, 1, 1, size.into(), None);
                device.create_texture_from_hal::<Metal>(raw, &descriptor, wgpu::TextureUses::COLOR_TARGET)
            };
            self.queue = Some(queue.clone());
            Ok((texture, self.color))
        })
    }
}

impl Drawable for MetalFrame {
    fn present(self: Box<Self>, queue: &wgpu::Queue) -> Result<(), String> {
        if self.queue.as_ref() != Some(queue) {
            return Err("the Metal drawable must be presented on its rendering queue".into());
        }
        autoreleasepool(|_| {
            // SAFETY: This is the same live queue used to import and render the drawable.
            let queue = unsafe { queue.as_hal::<Metal>() }.ok_or("the wgpu queue is not Metal")?;
            let command = queue.as_raw().commandBuffer().ok_or("Metal could not create the present command buffer")?;
            if !self.transactional {
                command.presentDrawable(ProtocolObject::from_ref(&*self.drawable));
            }
            command.commit();
            if self.transactional {
                // Earlier rendering must be scheduled before the drawable joins the open transaction.
                command.waitUntilScheduled();
                self.drawable.present();
            }
            Ok(())
        })
    }
}

fn clear(queue: &ProtocolObject<dyn MTLCommandQueue>, texture: &ProtocolObject<dyn MTLTexture>) -> Result<(), String> {
    let descriptor = MTLRenderPassDescriptor::renderPassDescriptor();
    // SAFETY: A render pass descriptor always has color attachment zero.
    let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    attachment.setTexture(Some(texture));
    attachment.setLoadAction(MTLLoadAction::Clear);
    attachment.setStoreAction(MTLStoreAction::Store);
    attachment.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 });
    let command = queue.commandBuffer().ok_or("Metal could not create the drawable clear command buffer")?;
    let encoder = command.renderCommandEncoderWithDescriptor(&descriptor).ok_or("Metal could not clear the drawable")?;
    encoder.endEncoding();
    command.commit();
    Ok(())
}
