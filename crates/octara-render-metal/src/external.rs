//! Zero-copy Metal views of externally produced images.

use crate::target::{Command, Device, Texture};
use block2::RcBlock;
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVReturn, kCVPixelFormatType_32BGRA as PIXEL_FORMAT_BGRA8,
    kCVPixelFormatType_32RGBA as PIXEL_FORMAT_RGBA8, kCVPixelFormatType_64RGBAHalf as PIXEL_FORMAT_RGBA16_FLOAT, kCVReturnSuccess,
};
use objc2_io_surface::IOSurfaceRef;
use objc2_metal::*;
use octara_model::image::{ExternalTextureHandle, ExternalTextureKind};
use std::{
    ptr::{self, NonNull},
    sync::Mutex,
};

/// Exposes a retained, shader-readable 2D color texture to the Metal compositor.
///
/// The texture must contain premultiplied linear-sRGB samples after Metal's pixel-format
/// decoding, with the first row at the top. Synchronize producer writes before Octara
/// reads the texture and wait for the compositor to finish before reusing its pixels.
pub fn external_texture_from_metal(texture: Retained<ProtocolObject<dyn MTLTexture>>) -> Result<ExternalTextureHandle, String> {
    image(texture, None)
}

/// Creates a Metal texture view of a packed RGB IOSurface on the compositor's device.
///
/// The pixel format describes the surface's layout and must match its format if tagged.
/// The surface stays in use for the lifetime of the handle and any frame sampling it.
/// The producer synchronizes pixel writes and follows the same color convention as
/// [`external_texture_from_metal`].
pub fn external_texture_from_iosurface(
    device: &ProtocolObject<dyn MTLDevice>,
    surface: &IOSurfaceRef,
    format: MTLPixelFormat,
) -> Result<ExternalTextureHandle, String> {
    let bytes_per_pixel = packed_format(surface.pixel_format(), format)?;
    let width = surface.width();
    let height = surface.height();
    if width == 0 || height == 0 || surface.plane_count() != 0 || surface.bytes_per_element() != bytes_per_pixel {
        return Err("Octara Metal requires a nonempty packed RGB IOSurface".into());
    }
    // The surface supplies validated 2D dimensions and its compatible color format.
    let descriptor =
        unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(format, width, height, false) };
    let storage = if cfg!(target_os = "macos") && !device.supportsFamily(MTLGPUFamily::Apple1) {
        MTLStorageMode::Managed
    } else {
        MTLStorageMode::Shared
    };
    descriptor.setStorageMode(storage);
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    let texture = device
        .newTextureWithDescriptor_iosurface_plane(&descriptor, surface, 0)
        .ok_or("Could not create an Octara Metal IOSurface view")?;
    external_texture_from_metal(texture)
}

/// Reusable Core Video texture cache for one Metal device.
pub struct MetalTextureCache(CFRetained<CVMetalTextureCache>);

impl MetalTextureCache {
    /// Creates a Core Video cache on the device Octara will use to sample its textures.
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<Self, String> {
        let mut cache = ptr::null_mut();
        // All optional dictionaries use defaults and the output points to a writable slot.
        let status = unsafe { CVMetalTextureCache::create(None, None, device, None, NonNull::from(&mut cache)) };
        Ok(Self(created(status, cache, "texture cache")?))
    }

    /// Maps an IOSurface-backed packed RGB pixel buffer without uploading its pixels.
    ///
    /// BGRA8, RGBA8 and RGBA16Float buffers use the corresponding Metal pixel format.
    /// The caller chooses the format's transfer function and supplies premultiplied
    /// linear-sRGB samples after Metal decoding. Synchronize producer writes before
    /// compositing. The picture remains retained until Octara finishes sampling it.
    pub fn texture(&self, pixel_buffer: &CVPixelBuffer, format: MTLPixelFormat) -> Result<ExternalTextureHandle, String> {
        packed_format(CVPixelBufferGetPixelFormatType(pixel_buffer), format)?;
        if CVPixelBufferGetIOSurface(Some(pixel_buffer)).is_none() {
            return Err("Octara Metal requires an IOSurface-backed CVPixelBuffer".into());
        }
        let mut mapped = ptr::null_mut();
        // The color format matches the single packed plane and the output slot is valid.
        let status = unsafe {
            CVMetalTextureCache::create_texture_from_image(
                None,
                &self.0,
                pixel_buffer,
                None,
                format,
                CVPixelBufferGetWidth(pixel_buffer),
                CVPixelBufferGetHeight(pixel_buffer),
                0,
                NonNull::from(&mut mapped),
            )
        };
        let mapped = created(status, mapped, "pixel buffer view")?;
        let texture = CVMetalTextureGetTexture(&mapped).ok_or("Core Video did not return an Octara Metal texture")?;
        // The borrowed CVPixelBuffer is live. The new retained owner can outlive the caller.
        let pixels = unsafe { CFRetained::retain(NonNull::from(pixel_buffer)) };
        image(texture, Some(VideoBacking { _mapped: mapped, _pixels: pixels }))
    }
}

fn packed_format(code: u32, format: MTLPixelFormat) -> Result<usize, String> {
    match (code, format) {
        (0 | PIXEL_FORMAT_BGRA8, MTLPixelFormat::BGRA8Unorm | MTLPixelFormat::BGRA8Unorm_sRGB)
        | (0 | PIXEL_FORMAT_RGBA8, MTLPixelFormat::RGBA8Unorm | MTLPixelFormat::RGBA8Unorm_sRGB) => Ok(4),
        (0 | PIXEL_FORMAT_RGBA16_FLOAT, MTLPixelFormat::RGBA16Float) => Ok(8),
        _ => Err(format!("Octara Metal cannot map packed texture format {code:#010x} as {format:?}")),
    }
}

fn created<T: objc2_core_foundation::Type>(status: CVReturn, value: *mut T, name: &str) -> Result<CFRetained<T>, String> {
    if status != kCVReturnSuccess {
        return Err(format!("Could not create Octara Metal {name} (Core Video status {status})"));
    }
    let value = NonNull::new(value).ok_or_else(|| format!("Core Video did not return an Octara Metal {name}"))?;
    // Both creation APIs transfer a +1 reference on success.
    Ok(unsafe { CFRetained::from_raw(value) })
}

struct SurfaceBacking(Retained<IOSurfaceRef>);

impl Drop for SurfaceBacking {
    fn drop(&mut self) {
        self.0.decrement_use_count();
    }
}

struct VideoBacking {
    _mapped: CFRetained<CVMetalTexture>,
    _pixels: CFRetained<CVPixelBuffer>,
}

pub(crate) struct MetalImage {
    pub(crate) texture: Texture,
    _surface: Option<SurfaceBacking>,
    _video: Option<VideoBacking>,
}

// The backing references only pin lifetime and release on drop. IOSurface use counts
// and Core Foundation reference counts are thread-safe. The Metal texture is shareable.
unsafe impl Send for MetalImage {}
unsafe impl Sync for MetalImage {}

fn image(texture: Texture, video: Option<VideoBacking>) -> Result<ExternalTextureHandle, String> {
    let usage = texture.usage();
    if texture.textureType() != MTLTextureType::Type2D
        || texture.isFramebufferOnly()
        || texture.storageMode() == MTLStorageMode::Memoryless
        || (usage != MTLTextureUsage::Unknown && !usage.contains(MTLTextureUsage::ShaderRead))
        || !matches!(
            texture.pixelFormat(),
            MTLPixelFormat::BGRA8Unorm
                | MTLPixelFormat::BGRA8Unorm_sRGB
                | MTLPixelFormat::RGBA8Unorm
                | MTLPixelFormat::RGBA8Unorm_sRGB
                | MTLPixelFormat::RGBA16Float
                | MTLPixelFormat::RGB10A2Unorm
                | MTLPixelFormat::BGR10A2Unorm
        )
    {
        return Err("Octara Metal requires a shader-readable 2D texture in a supported RGB format".into());
    }
    let extent = (texture.width() as u32, texture.height() as u32);
    let surface = texture.iosurface().map(|surface| {
        surface.increment_use_count();
        SurfaceBacking(surface)
    });
    Ok(ExternalTextureHandle::new(ExternalTextureKind::Metal, extent, MetalImage { texture, _surface: surface, _video: video }))
}

pub(crate) fn compatible(handle: &ExternalTextureHandle, device: &Device) -> bool {
    handle.kind() == ExternalTextureKind::Metal
        && handle.downcast_ref::<MetalImage>().is_some_and(|image| ptr::eq(&*image.texture.device(), device))
}

pub(crate) fn retain_until_completed(command: &Command, external: &[Option<ExternalTextureHandle>]) {
    let images: Vec<_> = external.iter().flatten().cloned().collect();
    if images.is_empty() {
        return;
    }
    // The command buffer also retains the MTLTextures. The associated owners prevent
    // an IOSurface or Core Video pool from recycling their storage during GPU sampling.
    let images = Mutex::new(Some(images));
    let completed = RcBlock::new(move |_: NonNull<Command>| {
        drop(images.lock().unwrap_or_else(|e| e.into_inner()).take());
    });
    // Metal copies the block, whose captured owners are thread-safe.
    unsafe { command.addCompletedHandler(RcBlock::as_ptr(&completed)) };
}
