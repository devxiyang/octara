use super::RecordError;
use octara_model::image::{ExternalTextureHandle, ExternalTextureKind};
use std::sync::Arc;

struct WgpuTexture {
    owner: usize,
    _device: wgpu::Device,
    texture: Arc<wgpu::Texture>,
    view: wgpu::TextureView,
}

/// Wraps a single-sampled, filterable 2D texture for direct sampling by the renderer.
///
/// Native surface renderer factories receive the device to pass here.
/// Create it on the renderer's device with `TEXTURE_BINDING` usage. Its texels must
/// represent premultiplied color in linear light after the texture format's decode.
/// Submit producer commands before submitting a frame that samples the texture.
/// Keep the texture available until sampling frames finish. Octara samples mip
/// level zero and retains the texture through the handle.
///
/// # Safety
///
/// `texture` must have been created by `device`. wgpu does not expose the
/// device owning a texture, and resource IDs from distinct instances can alias.
pub unsafe fn external_texture(device: &wgpu::Device, texture: Arc<wgpu::Texture>) -> Result<ExternalTextureHandle, RecordError> {
    let size = (texture.width(), texture.height());
    if size.0 == 0
        || size.1 == 0
        || texture.dimension() != wgpu::TextureDimension::D2
        || texture.depth_or_array_layers() != 1
        || texture.sample_count() != 1
    {
        return Err(RecordError::ExternalTexture("expected a single-sampled 2D texture with one layer"));
    }
    if !texture.usage().contains(wgpu::TextureUsages::TEXTURE_BINDING) {
        return Err(RecordError::ExternalTexture("missing TEXTURE_BINDING usage"));
    }
    if !filterable(&texture, device.features()) {
        return Err(RecordError::ExternalTexture("expected a filterable color texture format"));
    }
    let owner = device_address(device).ok_or(RecordError::ExternalTexture("unsupported wgpu device backend"))?;
    let (view, error) = super::with_error_scopes(device, || {
        texture.create_view(&wgpu::TextureViewDescriptor { mip_level_count: Some(1), ..Default::default() })
    });
    if error.is_some() {
        return Err(RecordError::ExternalTexture("texture is unavailable"));
    }
    Ok(ExternalTextureHandle::new(ExternalTextureKind::Wgpu, size, WgpuTexture { owner, _device: device.clone(), texture, view }))
}

pub(super) fn view(owner: usize, handle: &ExternalTextureHandle) -> Option<&wgpu::TextureView> {
    if handle.kind() != ExternalTextureKind::Wgpu {
        return None;
    }
    let source = handle.downcast_ref::<WgpuTexture>()?;
    (source.owner == owner && handle.size() == (source.texture.width(), source.texture.height())).then_some(&source.view)
}

fn filterable(texture: &wgpu::Texture, features: wgpu::Features) -> bool {
    texture.format().sample_type(None, Some(features)) == Some(wgpu::TextureSampleType::Float { filterable: true })
}

pub(super) fn device_address(device: &wgpu::Device) -> Option<usize> {
    fn address<A: wgpu::hal::Api>(device: &wgpu::Device) -> Option<usize> {
        // Only the address is used as an identity. The device owns the HAL object.
        unsafe { device.as_hal::<A>() }.map(|guard| std::ptr::from_ref(&*guard).addr())
    }

    use wgpu::hal::api;
    match device.adapter_info().backend {
        #[cfg(target_vendor = "apple")]
        wgpu::Backend::Metal => address::<api::Metal>(device),
        #[cfg(target_os = "windows")]
        wgpu::Backend::Dx12 => address::<api::Dx12>(device),
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "android", target_os = "freebsd"))]
        wgpu::Backend::Vulkan => address::<api::Vulkan>(device),
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "android", target_os = "freebsd"))]
        wgpu::Backend::Gl => address::<api::Gles>(device),
        _ => None,
    }
}
