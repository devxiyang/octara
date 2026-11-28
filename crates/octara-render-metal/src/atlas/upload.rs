use super::{Atlases, Family};
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLDevice, MTLOrigin, MTLResourceOptions, MTLSize,
};
use octara_render::plan::Region;
use std::sync::Arc;

// The buffer blit uses a four-byte-aligned pitch even for R8 masks.
const ALIGNMENT: usize = 4;

pub(super) struct Pixels {
    pub(super) data: Arc<[u8]>,
    pub(super) stride: usize,
    pub(super) offset: usize,
}

pub(super) struct Upload {
    region: Region,
    mip: u8,
    pixels: Pixels,
    row_bytes: usize,
}

impl Upload {
    pub(super) fn new(region: Region, mip: u8, family: Family, pixels: Pixels) -> Self {
        Self { region, mip, pixels, row_bytes: region.size[0] as usize * family.bytes_per_pixel() as usize }
    }
}

#[derive(Default)]
pub(super) struct Uploads {
    blits: Vec<(usize, Upload)>,
    length: usize,
    bytes: u64,
}

impl Uploads {
    pub(super) fn push(&mut self, upload: Upload) -> Result<(), String> {
        let height = upload.region.size[1] as usize;
        let row_bytes = upload.row_bytes;
        let Pixels { data, stride, offset } = &upload.pixels;
        let needed = offset
            .checked_add((height - 1).checked_mul(*stride).ok_or("Metal atlas source stride overflow")?)
            .and_then(|start| start.checked_add(row_bytes))
            .ok_or("Metal atlas source offset overflow")?;
        if row_bytes > *stride || needed > data.len() {
            return Err("Metal atlas upload exceeded its source bytes".into());
        }
        let offset = self.length;
        self.length = offset.checked_add(row_bytes.next_multiple_of(ALIGNMENT) * height).ok_or("Metal atlas staging size overflow")?;
        self.bytes += (row_bytes * height) as u64;
        self.blits.push((offset, upload));
        Ok(())
    }

    pub(super) fn encode(
        self,
        device: &ProtocolObject<dyn MTLDevice>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        atlas: &Atlases,
    ) -> Result<u64, String> {
        if self.blits.is_empty() {
            return Ok(0);
        }
        if self.length > device.maxBufferLength() {
            return Err(format!("Metal atlas staging buffer needs {} bytes, exceeding the device limit", self.length));
        }
        let buffer = device
            .newBufferWithLength_options(self.length, MTLResourceOptions::StorageModeShared | MTLResourceOptions::CPUCacheModeWriteCombined)
            .ok_or("Could not allocate Metal atlas staging buffer")?;
        let blit = command_buffer.blitCommandEncoder().ok_or("Could not encode Metal atlas blit")?;
        let base = buffer.contents().cast::<u8>().as_ptr();
        for (start, upload) in self.blits {
            let [width, height] = upload.region.size.map(|axis| axis as usize);
            let pitch = upload.row_bytes.next_multiple_of(ALIGNMENT);
            let Pixels { data, stride, offset } = upload.pixels;
            for row in 0..height {
                let src = offset + row * stride;
                let dst = start + row * pitch;
                // SAFETY: push validated the source span and reserved this disjoint
                // destination span. The shared buffer is new and the GPU has yet to read it.
                unsafe { base.add(dst).copy_from_nonoverlapping(data.as_ptr().add(src), upload.row_bytes) };
            }
            let [x, y] = upload.region.origin.map(|axis| axis as usize);
            // SAFETY: the reservation contains the destination at this mip, and the
            // aligned source image is entirely inside the staging buffer.
            unsafe {
                blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &buffer,
                    start,
                    pitch,
                    0,
                    MTLSize { width, height, depth: 1 },
                    atlas.texture(upload.region.array),
                    usize::from(upload.region.layer),
                    usize::from(upload.mip),
                    MTLOrigin { x, y, z: 0 },
                );
            }
        }
        blit.endEncoding();
        Ok(self.bytes)
    }
}
