//! Metal attachments and the persistent output backing.

use super::pipeline::{Pipeline, Pipelines};
use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::*;
use octara_render::{DamageRect, FrameProfile, OutputColor, plan::SceneSnapshot};
use std::collections::HashMap;

pub(crate) const WORKING_FORMAT: MTLPixelFormat = MTLPixelFormat::RGBA16Float;
pub(crate) type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
pub(crate) type Encoder = ProtocolObject<dyn MTLRenderCommandEncoder>;
pub(crate) type Command = ProtocolObject<dyn MTLCommandBuffer>;
pub(crate) type Device = ProtocolObject<dyn MTLDevice>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Output {
    pub(crate) extent: [u32; 2],
    pub(crate) format: MTLPixelFormat,
    pub(crate) color: OutputColor,
    pub(crate) blit: bool,
}

impl Output {
    pub(crate) fn new(drawable: &ProtocolObject<dyn MTLTexture>, color: OutputColor) -> Result<Self, String> {
        let extent = [drawable.width(), drawable.height()];
        if extent.iter().any(|&n| n == 0 || n > 16384)
            || drawable.textureType() != MTLTextureType::Type2D
            || drawable.sampleCount() != 1
            || drawable.storageMode() == MTLStorageMode::Memoryless
        {
            return Err("Octara Metal requires a single-sample 2D output texture".into());
        }
        let usage = drawable.usage();
        if usage != MTLTextureUsage::Unknown && !usage.contains(MTLTextureUsage::RenderTarget) {
            return Err("Octara Metal requires output with render-target usage".into());
        }
        let format = drawable.pixelFormat();
        let valid = match color {
            OutputColor::Linear => format == WORKING_FORMAT,
            _ => matches!(format, MTLPixelFormat::RGBA8Unorm | MTLPixelFormat::BGRA8Unorm) || is_srgb(format),
        };
        if !valid {
            return Err(format!("Unsupported Metal output format {format:?} for {color:?}"));
        }
        Ok(Self { extent: extent.map(|n| n as u32), format, color, blit: !drawable.isFramebufferOnly() })
    }

    pub(crate) fn requires_resolve(self) -> bool {
        self.color != OutputColor::Linear && !(self.color == OutputColor::SrgbOpaque && is_srgb(self.format))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Raster {
    Full,
    Damage(DamageRect),
    Reuse,
}

pub(crate) struct Surface {
    output: Output,
    resolve: bool,
    retain: bool,
    resolved: Option<Texture>,
    working: Option<Texture>,
    pub(crate) snapshot: Option<(SceneSnapshot, bool)>,
}

impl Surface {
    pub(crate) fn new(device: &Device, output: Output, resolve: bool, retain: bool) -> Result<Self, String> {
        let resolved = if output.blit || (retain && !resolve) {
            Some(texture(device, output.extent, output.format, MTLStorageMode::Private, true)?)
        } else {
            None
        };
        let working = if resolve { Some(texture(device, output.extent, WORKING_FORMAT, MTLStorageMode::Private, true)?) } else { None };
        Ok(Self { output, retain, resolve, resolved, working, snapshot: None })
    }

    pub(crate) fn matches(&self, output: Output, resolve: bool, retain: bool) -> bool {
        self.output == output && self.resolve == resolve && self.retain == retain
    }

    pub(crate) fn draw_texture<'a>(&'a self, drawable: &'a ProtocolObject<dyn MTLTexture>) -> &'a ProtocolObject<dyn MTLTexture> {
        self.working.as_deref().or(self.resolved.as_deref()).unwrap_or(drawable)
    }

    pub(crate) fn tracks_damage(&self) -> bool {
        self.retain && (self.resolved.is_some() || self.working.is_some())
    }

    pub(crate) fn resolves(&self) -> bool {
        self.working.is_some() || (!self.output.blit && self.resolved.is_some())
    }

    pub(crate) fn transfer(
        &self,
        command: &Command,
        drawable: &ProtocolObject<dyn MTLTexture>,
        pipelines: &Pipelines,
        raster: Raster,
        profile: &mut FrameProfile,
    ) -> Result<(), String> {
        if self.resolves() && (!self.output.blit || !matches!(raster, Raster::Reuse)) {
            let target = if self.output.blit { self.resolved.as_deref().expect("blit backing") } else { drawable };
            let damage = match raster {
                Raster::Damage(rect) if self.output.blit => Some(rect),
                _ => None,
            };
            let pass = render_pass(command, target, None, damage.is_some(), [0.0; 4])?;
            pass.setRenderPipelineState(pipelines.state(Pipeline::Resolve, self.output.format, MTLPixelFormat::Invalid));
            if let Some(rect) = damage {
                scissor(&pass, rect);
            }
            let encoding = if self.working.is_some() { self.output.color } else { OutputColor::Linear };
            let params = [encoding as u32, u32::from(is_srgb(self.output.format))];
            fragment_bytes(&pass, &params, 0);
            // The source and destination have the same extent, and the shader reads level zero.
            unsafe { pass.setFragmentTexture_atIndex(Some(self.draw_texture(drawable)), 0) };
            fullscreen(&pass);
            pass.endEncoding();
            profile.render_passes += 1;
        }
        if self.output.blit {
            let blit = command.blitCommandEncoder().ok_or("Could not begin the Octara Metal backing blit")?;
            let origin = MTLOrigin { x: 0, y: 0, z: 0 };
            let [width, height] = self.output.extent.map(|n| n as usize);
            let size = MTLSize { width, height, depth: 1 };
            let source = self.resolved.as_deref().expect("blit backing");
            // Both textures have the same format, sample count and extent, and are distinct.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    source, 0, 0, origin, size, drawable, 0, 0, origin,
                );
            }
            blit.endEncoding();
        }
        if self.output.blit || self.resolves() {
            let [w, h] = self.output.extent;
            profile.output_pixels = u64::from(w) * u64::from(h);
        }
        Ok(())
    }
}

fn is_srgb(format: MTLPixelFormat) -> bool {
    matches!(format, MTLPixelFormat::RGBA8Unorm_sRGB | MTLPixelFormat::BGRA8Unorm_sRGB)
}

#[derive(Default)]
pub(crate) struct Attachments {
    layers: Vec<Option<Texture>>,
    depth: HashMap<([u32; 2], MTLPixelFormat), Texture>,
}

impl Attachments {
    pub(crate) fn begin_frame(&mut self, layer_count: usize, targets: impl Iterator<Item = ([u32; 2], MTLPixelFormat)>) {
        self.layers.resize_with(layer_count, || None);
        let compatible = targets.collect::<Vec<_>>();
        self.depth.retain(|key, _| compatible.contains(key));
    }

    pub(crate) fn layer(&mut self, device: &Device, index: usize, extent: [u32; 2]) -> Result<Texture, String> {
        let entry = &mut self.layers[index];
        if let Some(found) = entry
            && found.width() == extent[0] as usize
            && found.height() == extent[1] as usize
        {
            return Ok(found.clone());
        }
        let layer = texture(device, extent, WORKING_FORMAT, MTLStorageMode::Private, true)?;
        *entry = Some(layer.clone());
        Ok(layer)
    }

    pub(crate) fn depth(&mut self, device: &Device, extent: [u32; 2], format: MTLPixelFormat) -> Result<Option<Texture>, String> {
        if format == MTLPixelFormat::Invalid {
            return Ok(None);
        }
        let entry = match self.depth.entry((extent, format)) {
            std::collections::hash_map::Entry::Occupied(entry) => return Ok(Some(entry.get().clone())),
            std::collections::hash_map::Entry::Vacant(entry) => entry,
        };
        // Apple GPU families support tile-only depth. Simulator and non-Apple GPUs use private depth.
        let memoryless = !cfg!(target_abi = "sim") && device.supportsFamily(MTLGPUFamily::Apple1);
        let depth = if memoryless {
            texture(device, extent, format, MTLStorageMode::Memoryless, false)
                .or_else(|_| texture(device, extent, format, MTLStorageMode::Private, false))?
        } else {
            texture(device, extent, format, MTLStorageMode::Private, false)?
        };
        Ok(Some(entry.insert(depth).clone()))
    }
}

pub(crate) fn texture(
    device: &Device,
    [width, height]: [u32; 2],
    format: MTLPixelFormat,
    storage: MTLStorageMode,
    sampleable: bool,
) -> Result<Texture, String> {
    // All callers supply a supported attachment format and an extent within device limits.
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(format, width as usize, height as usize, false)
    };
    desc.setStorageMode(storage);
    let mut usage = MTLTextureUsage::RenderTarget;
    if sampleable {
        usage |= MTLTextureUsage::ShaderRead;
    }
    desc.setUsage(usage);
    device.newTextureWithDescriptor(&desc).ok_or_else(|| format!("Could not allocate an Octara Metal {width}x{height} {format:?} texture"))
}

pub(crate) fn render_pass(
    command: &Command,
    target: &ProtocolObject<dyn MTLTexture>,
    depth: Option<&ProtocolObject<dyn MTLTexture>>,
    load: bool,
    [r, g, b, a]: [f32; 4],
) -> Result<Retained<Encoder>, String> {
    let desc = MTLRenderPassDescriptor::renderPassDescriptor();
    // The descriptor has a fixed array of color attachments, starting at zero.
    let color = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    color.setTexture(Some(target));
    color.setLoadAction(if load { MTLLoadAction::Load } else { MTLLoadAction::Clear });
    color.setStoreAction(MTLStoreAction::Store);
    color.setClearColor(MTLClearColor { red: r.into(), green: g.into(), blue: b.into(), alpha: a.into() });
    if let Some(depth) = depth {
        let attachment = desc.depthAttachment();
        attachment.setTexture(Some(depth));
        attachment.setLoadAction(MTLLoadAction::Clear);
        attachment.setClearDepth(0.0);
        attachment.setStoreAction(MTLStoreAction::DontCare);
    }
    command.renderCommandEncoderWithDescriptor(&desc).ok_or_else(|| "Could not begin an Octara Metal render pass".into())
}

pub(crate) fn scissor(encoder: &Encoder, rect: DamageRect) {
    let [x, y, width, height] = rect.pixels_rect().map(|n| n as usize);
    // Damage is clamped to the attachment dimensions by the scene planner.
    encoder.setScissorRect(MTLScissorRect { x, y, width, height })
}

pub(crate) fn fragment_bytes<T: octara_render::plan::bytemuck::Pod>(encoder: &Encoder, value: &T, slot: usize) {
    let bytes = octara_render::plan::bytemuck::bytes_of(value);
    let ptr = std::ptr::NonNull::from(bytes).cast();
    // Metal copies these initialized uniform bytes immediately.
    unsafe { encoder.setFragmentBytes_length_atIndex(ptr, bytes.len(), slot) }
}

pub(crate) fn fullscreen(encoder: &Encoder) {
    // The full-screen vertex function uses three vertices and no vertex buffers.
    unsafe { encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) }
}
