//! A plain Metal frame acquired from the configured layer and rendered on Octara's wgpu queue.

use crate::gpu::Gpu;
use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, available};
use objc2_metal::{
    MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDrawable, MTLHazardTrackingMode, MTLLoadAction, MTLPixelFormat,
    MTLRenderPassDescriptor, MTLResource, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureType, MTLTextureUsage,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use octara_render::wgpu;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Mutex;
use wgpu::hal::api::Metal;

pub(crate) struct MetalLayer(Retained<CAMetalLayer>);

// SAFETY: The worker only calls nextDrawable and releases this retained Core Animation layer.
// The host owns attachment and waits for the worker to release the target before reconfiguring it.
unsafe impl Send for MetalLayer {}

impl MetalLayer {
    /// Retains the configured layer on the window thread before acquisition starts.
    pub(crate) fn from_surface(surface: &wgpu::Surface<'_>) -> Result<Self, String> {
        if MainThreadMarker::new().is_none() || !available!(macos = 10.15) {
            return Err("Metal drawable acquisition requires macOS 10.15 and the window thread".into());
        }
        // SAFETY: The guard protects the HAL surface while we retain its layer.
        let layer = unsafe { surface.as_hal::<Metal>() }
            .map(|surface| surface.render_layer().lock().clone())
            .ok_or("the wgpu surface has no Metal layer")?;
        if layer.pixelFormat() != MTLPixelFormat::BGRA8Unorm || layer.presentsWithTransaction() || layer.framebufferOnly() {
            return Err("the Metal layer is not configured for plain BGRA8 rendering and copies".into());
        }
        if !layer.allowsNextDrawableTimeout() {
            layer.setAllowsNextDrawableTimeout(true);
        }
        Ok(Self(layer))
    }

    pub(crate) fn acquire(&self) -> Option<MetalFrame> {
        autoreleasepool(|_| self.0.nextDrawable().map(|drawable| MetalFrame { drawable, queue: None }))
    }
}

pub(crate) struct MetalFrame {
    drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
    queue: Option<wgpu::Queue>,
}

// SAFETY: One retained drawable is handed from the acquirer to the presenter by value. Only
// its owner can use it. This matches the lifetime of wgpu-hal's retained Metal SurfaceTexture.
unsafe impl Send for MetalFrame {}

impl MetalFrame {
    pub(crate) fn texture(&mut self, gpu: &Gpu, format: wgpu::TextureFormat, expected_size: (u32, u32)) -> Result<wgpu::Texture, String> {
        if self.queue.is_some() || format != wgpu::TextureFormat::Bgra8Unorm {
            return Err("the drawable may be imported once as BGRA8Unorm".into());
        }

        autoreleasepool(|_| {
            let device = gpu.device();
            let queue = gpu.queue();
            let raw = self.drawable.texture();
            let usage = raw.usage();
            if raw.pixelFormat() != MTLPixelFormat::BGRA8Unorm
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
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
            Ok(texture)
        })
    }

    /// Reports Metal's presentation time in host seconds. Register before presenting the frame.
    pub(crate) fn on_presented(&self, callback: impl FnOnce(f64) + Send + 'static) {
        if !available!(macos = 10.15.4) {
            return;
        }

        let callback = Mutex::new(Some(callback));
        let handler = RcBlock::new(move |drawable: NonNull<ProtocolObject<dyn MTLDrawable>>| {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let callback = callback.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
                // SAFETY: Metal supplies a drawable that lives through the callback.
                let time = unsafe { drawable.as_ref() }.presentedTime();
                if let Some(callback) = callback
                    && time.is_finite()
                    && time > 0.0
                {
                    callback(time);
                }
            }));
        });
        // SAFETY: Metal retains its escaping block. The mutex protects the take on any thread,
        // and the block holds no drawable reference that could extend the drawable's lifetime.
        unsafe { self.drawable.addPresentedHandler(RcBlock::as_ptr(&handler)) };
    }

    /// Schedules the drawable after the renderer has submitted on its import queue.
    pub(crate) fn present(self, queue: &wgpu::Queue) -> Result<(), String> {
        if self.queue.as_ref() != Some(queue) {
            return Err("the Metal drawable must be presented on its rendering queue".into());
        }
        autoreleasepool(|_| {
            // SAFETY: The wgpu guard keeps the Metal queue alive through the ordered present.
            let queue = unsafe { queue.as_hal::<Metal>() }.ok_or("the wgpu queue is not Metal")?;
            let command = queue.as_raw().commandBuffer().ok_or("Metal could not create the present command buffer")?;
            command.presentDrawable(ProtocolObject::from_ref(&*self.drawable));
            command.commit();
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
