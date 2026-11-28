//! Offscreen Metal readback, with pixel unpacking deferred to the capture writer.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLOrigin, MTLPixelFormat, MTLResourceOptions,
    MTLSize, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use octara_model::capture::{PaintReceipt, PaintedPicture};
use octara_model::paint::{DisplayList, Picture};

pub(super) struct Capture {
    pub(super) texture: Retained<ProtocolObject<dyn MTLTexture>>,
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    size: [u32; 2],
    stride: usize,
}

pub(super) struct Readback {
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    size: [u32; 2],
    stride: usize,
    effects: Vec<PaintReceipt>,
}

// SAFETY: The buffer is retained and has shared storage. Its owner only reads it after
// Metal completes the copy and is the only CPU user.
unsafe impl Send for Readback {}

impl Capture {
    pub(super) fn new(device: &ProtocolObject<dyn MTLDevice>, list: &DisplayList, scale: f32) -> Result<Self, String> {
        if !scale.is_finite() || scale <= 0.0 {
            return Err("capture scale must be finite and positive".into());
        }
        let dimensions = [list.size.width, list.size.height];
        if dimensions.iter().any(|size| !size.is_finite() || *size < 0.0) {
            return Err("capture dimensions must be finite and non-negative".into());
        }
        let size = dimensions.map(|size| (size * scale).ceil().max(1.0) as u32);
        let [width, height] = size.map(|pixels| pixels as usize);
        const MAX_TEXTURE_DIMENSION: usize = 16_384;
        if width > MAX_TEXTURE_DIMENSION || height > MAX_TEXTURE_DIMENSION {
            return Err(format!("capture {width}×{height} exceeds the Metal texture dimension limit {MAX_TEXTURE_DIMENSION}"));
        }
        let stride = (width * 4).next_multiple_of(256);
        let bytes = stride * height;
        if bytes > device.maxBufferLength() {
            return Err(format!("capture readback exceeds the Metal buffer limit of {} bytes", device.maxBufferLength()));
        }
        // SAFETY: The 2D dimensions are positive and within Metal's texture limit.
        let descriptor = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm_sRGB,
                width,
                height,
                false,
            )
        };
        descriptor.setStorageMode(MTLStorageMode::Private);
        descriptor.setUsage(MTLTextureUsage::RenderTarget);
        let texture = device.newTextureWithDescriptor(&descriptor).ok_or("Metal could not allocate the capture texture")?;
        let buffer = device
            .newBufferWithLength_options(bytes, MTLResourceOptions::StorageModeShared)
            .ok_or("Metal could not allocate the capture buffer")?;
        Ok(Self { texture, buffer, size, stride })
    }

    pub(super) fn encode_readback(
        self,
        command: &ProtocolObject<dyn MTLCommandBuffer>,
        effects: Vec<PaintReceipt>,
    ) -> Result<Readback, String> {
        let Self { texture, buffer, size, stride } = self;
        let [width, height] = size.map(|pixels| pixels as usize);
        let blit = command.blitCommandEncoder().ok_or("Metal could not create the capture blit encoder")?;
        // SAFETY: The source and destination belong to this command queue. The copy covers
        // exactly the texture and writes at most stride * height bytes into the buffer.
        unsafe {
            blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                &texture, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, MTLSize { width, height, depth: 1 },
                &buffer, 0, stride, 0,
            );
        }
        blit.endEncoding();
        Ok(Readback { buffer, size, stride, effects })
    }
}

impl Readback {
    pub(super) fn bytes(&self) -> usize {
        self.stride * self.size[1] as usize
    }

    pub(super) fn size(&self) -> [u32; 2] {
        self.size
    }

    pub(super) fn unpack(self) -> Result<PaintedPicture, String> {
        let [width, height] = self.size;
        let row_bytes = width as usize * 4;
        let mut rgba = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            // SAFETY: Metal completed the blit into this retained shared buffer. Each
            // row's pixels were initialized by the blit. Leave alignment padding unread.
            let bgra =
                unsafe { std::slice::from_raw_parts(self.buffer.contents().cast::<u8>().as_ptr().add(row * self.stride), row_bytes) };
            for &[b, g, r, a] in bgra.as_chunks::<4>().0 {
                rgba.extend_from_slice(&[r, g, b, a]);
            }
        }
        let picture = Picture::new(width, height, rgba).expect("checked Metal capture dimensions");
        Ok(PaintedPicture { picture, effects: self.effects })
    }
}
