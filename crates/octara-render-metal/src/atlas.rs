//! Persistent texture arrays. Cache entries live as long as all their arrays do.

mod skyline;
#[cfg(all(test, target_os = "macos"))]
mod tests;
mod upload;

use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandBufferStatus, MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureType,
    MTLTextureUsage,
};
use octara_model::paint::Sample;
use octara_render::glyphs::{GlyphImage, GlyphKey};
use octara_render::paths::{self, MaskKey};
use octara_render::plan::{
    ArrayId, DrawSample, IMAGE_ATLAS_EDGE, ImageMips, PACKED_IMAGE_MAX_LOD, Region, ResourcePlan, full_lod, packed_image_layout,
};
use skyline::Skyline;
use std::{collections::HashMap, num::NonZeroU64, sync::Arc};
use upload::{Pixels, Upload, Uploads};

const CACHE_BYTES: u64 = 64 * 1024 * 1024;
const FRAME_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EDGE: u32 = 16_384;
const PATH_TILE: u32 = 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Glyph,
    Mask,
    Image,
}

impl Family {
    fn edge(self) -> u32 {
        match self {
            Self::Glyph => 1024,
            Self::Mask => 2048,
            Self::Image => IMAGE_ATLAS_EDGE,
        }
    }

    fn format(self) -> MTLPixelFormat {
        match self {
            Self::Mask => MTLPixelFormat::R8Unorm,
            Self::Glyph | Self::Image => MTLPixelFormat::RGBA8Unorm_sRGB,
        }
    }

    fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Mask => 1,
            Self::Glyph | Self::Image => 4,
        }
    }

    fn layer_bytes(self, [w, h]: [u32; 2], lod: u8) -> u64 {
        (0..=lod).map(|mip| u64::from((w >> mip).max(1)) * u64::from((h >> mip).max(1))).sum::<u64>() * u64::from(self.bytes_per_pixel())
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PathTile {
    pub(crate) region: Region,
    pub(crate) offset: [u32; 2],
}

#[derive(Clone)]
pub(crate) enum SampleBinding {
    Missing,
    Glyph(Region),
    Image(Region),
    Path(Vec<PathTile>),
}

impl SampleBinding {
    fn visit_regions(&self, mut visit: impl FnMut(Region) -> Result<(), String>) -> Result<(), String> {
        match self {
            Self::Glyph(region) | Self::Image(region) => visit(*region),
            Self::Path(tiles) => tiles.iter().try_for_each(|tile| visit(tile.region)),
            Self::Missing => Ok(()),
        }
    }

    fn is_resident(&self, arrays: &HashMap<ArrayId, Array>) -> bool {
        match self {
            Self::Glyph(region) | Self::Image(region) => arrays.contains_key(&region.array),
            Self::Path(tiles) => tiles.iter().all(|tile| arrays.contains_key(&tile.region.array)),
            Self::Missing => false,
        }
    }
}

pub(crate) struct PreparedSamples {
    pub(crate) samples: Vec<SampleBinding>,
    pub(crate) atlas_bytes: u64,
    pub(crate) upload_bytes: u64,
}

#[derive(PartialEq, Eq, Hash)]
enum SampleKey {
    Glyph(GlyphKey),
    Path(MaskKey),
    Image(u64, u64),
}

impl SampleKey {
    fn of(sample: &DrawSample) -> Self {
        match sample {
            DrawSample::Glyph(glyph) => Self::Glyph(glyph.key.clone()),
            DrawSample::Path(_, placed) => Self::Path(placed.key),
            DrawSample::Image(image) => Self::Image(image.image.source().id(), image.image.frame().version),
        }
    }
}

struct Array {
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    family: Family,
    slices: Vec<Skyline>,
    bytes: u64,
    used: u64,
}

#[derive(Default)]
pub(crate) struct Atlases {
    // Identities never repeat, including when an unsubmitted frame invalidates the cache.
    next_array: u64,
    generation: u64,
    arrays: HashMap<ArrayId, Array>,
    samples: HashMap<SampleKey, SampleBinding>,
    writers: Vec<Retained<ProtocolObject<dyn MTLCommandBuffer>>>,
    working_bytes: u64,
}

impl Atlases {
    /// The queue must preserve frame order, and command buffers must retain their resources.
    /// The renderer retires invalid writes before preparing or reusing samples.
    pub(crate) fn prepare(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        resources: &ResourcePlan,
    ) -> Result<PreparedSamples, String> {
        if !command_buffer.retainedReferences() {
            return Err("Metal atlas uploads require a command buffer with retained references".into());
        }
        self.generation = self.generation.checked_add(1).ok_or("Metal atlas generation exhausted")?;
        self.working_bytes = 0;
        let result = self.prepare_frame(device, command_buffer, resources);
        if result.is_err() {
            // Some placements can already have been cached, but their writes were never encoded.
            self.clear();
        }
        result
    }

    pub(crate) fn texture(&self, array: ArrayId) -> &ProtocolObject<dyn MTLTexture> {
        &self.arrays[&array].texture
    }

    fn prepare_frame(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        resources: &ResourcePlan,
    ) -> Result<PreparedSamples, String> {
        let keys: Vec<_> = resources.samples.iter().map(SampleKey::of).collect();
        for key in &keys {
            if let Some(binding) = self.samples.get(key) {
                binding.visit_regions(|region| Self::pin(&mut self.arrays, self.generation, &mut self.working_bytes, region.array))?;
            }
        }
        self.trim();
        let mut uploads = Uploads::default();
        let mut samples = Vec::with_capacity(keys.len());
        for (sample, key) in resources.samples.iter().zip(keys) {
            if let Some(binding) = self.samples.get(&key) {
                samples.push(binding.clone());
                continue;
            }
            let binding = match sample {
                DrawSample::Glyph(glyph) => self.place_glyph(device, &glyph.image, &mut uploads)?,
                DrawSample::Path(path, placed) => self.place_path(device, path, placed, &mut uploads)?,
                DrawSample::Image(image) => self.place_image(device, &image.image, &mut uploads)?,
            };
            if !matches!(binding, SampleBinding::Missing) {
                self.samples.insert(key, binding.clone());
            }
            samples.push(binding);
        }
        self.trim();
        let upload_bytes = uploads.encode(device, command_buffer, self)?;
        if upload_bytes != 0 && !self.writers.iter().any(|writer| std::ptr::eq(&**writer, command_buffer)) {
            // SAFETY: the borrowed command buffer is a live, retainable Objective-C object.
            let writer = unsafe { Retained::retain(std::ptr::from_ref(command_buffer).cast_mut()) }.expect("live Metal command buffer");
            self.writers.push(writer);
        }
        Ok(PreparedSamples { samples, atlas_bytes: self.working_bytes, upload_bytes })
    }

    /// Invalidates cached samples when a recorded upload never became usable.
    pub(crate) fn retire_invalid_writes(&mut self, current: &ProtocolObject<dyn MTLCommandBuffer>) -> bool {
        let mut invalid = false;
        self.writers.retain(|writer| {
            if std::ptr::eq(&**writer, current) {
                return true;
            }
            match writer.status() {
                MTLCommandBufferStatus::Committed | MTLCommandBufferStatus::Scheduled => true,
                MTLCommandBufferStatus::Completed => false,
                _ => {
                    invalid = true;
                    false
                }
            }
        });
        if invalid {
            self.clear();
        }
        invalid
    }

    fn clear(&mut self) {
        self.arrays.clear();
        self.samples.clear();
        self.writers.clear();
    }

    fn pin(arrays: &mut HashMap<ArrayId, Array>, generation: u64, working_bytes: &mut u64, id: ArrayId) -> Result<(), String> {
        let array = arrays.get_mut(&id).expect("resident atlas array");
        if array.used != generation {
            let bytes = *working_bytes + array.bytes;
            if bytes > FRAME_BYTES {
                return Err(format!("Metal atlas working set needs {bytes} bytes, exceeding the {FRAME_BYTES}-byte limit"));
            }
            *working_bytes = bytes;
            array.used = generation;
        }
        Ok(())
    }

    fn trim(&mut self) {
        let limit = CACHE_BYTES.max(self.working_bytes);
        let mut bytes = self.arrays.values().map(|array| array.bytes).sum::<u64>();
        if bytes <= limit {
            return;
        }
        let mut cold: Vec<_> =
            self.arrays.iter().filter_map(|(&id, array)| (array.used != self.generation).then_some((array.used, id))).collect();
        cold.sort_unstable();
        for (_, id) in cold {
            bytes -= self.arrays.remove(&id).expect("cold atlas array").bytes;
            if bytes <= limit {
                break;
            }
        }
        self.samples.retain(|_, binding| binding.is_resident(&self.arrays));
    }

    fn reserve(&mut self, device: &ProtocolObject<dyn MTLDevice>, family: Family, size: [u32; 2]) -> Result<Region, String> {
        let [width, height] = size;
        if size.contains(&0) || width > MAX_EDGE || height > MAX_EDGE {
            return Err(format!("Metal atlas texture {width}×{height} is outside the supported range 1–{MAX_EDGE}"));
        }
        let edge = family.edge();
        let packed = if family == Family::Image {
            packed_image_layout(size, edge).map(|(footprint, lod)| (footprint, lod, 1 << lod))
        } else {
            Some((size, 0, 0))
        };
        let available = FRAME_BYTES - self.working_bytes;
        if let Some((footprint, lod, gutter)) = packed
            && footprint.into_iter().all(|axis| axis <= edge)
        {
            let alignment = 1 << lod;
            let mut found = None;
            // Search oldest array ID first. A stable order keeps packing deterministic.
            let mut ids: Vec<_> = self
                .arrays
                .iter()
                .filter_map(|(&id, array)| {
                    (array.family == family && !array.slices.is_empty() && (array.used == self.generation || array.bytes <= available))
                        .then_some(id)
                })
                .collect();
            ids.sort_unstable();
            for id in ids {
                let array = self.arrays.get_mut(&id).expect("packed atlas array");
                if let Some((slice, origin)) =
                    array.slices.iter_mut().enumerate().find_map(|(slice, packer)| Some((slice, packer.place(footprint, alignment)?)))
                {
                    found = Some((id, slice as u16, origin));
                    break;
                }
            }
            if let Some((array, slice, origin)) = found {
                Self::pin(&mut self.arrays, self.generation, &mut self.working_bytes, array)?;
                return Ok(Region::new(array, slice, origin.map(|axis| axis + gutter), size, lod, false));
            }
            let array_lod = if family == Family::Image { PACKED_IMAGE_MAX_LOD } else { 0 };
            let slices = (available / family.layer_bytes([edge; 2], array_lod)).min(2) as usize;
            if slices != 0 {
                let array = self.new_array(device, family, [edge; 2], array_lod, slices)?;
                let origin = self.arrays.get_mut(&array).expect("new atlas array").slices[0]
                    .place(footprint, alignment)
                    .expect("reservation fits a fresh array");
                return Ok(Region::new(array, 0, origin.map(|axis| axis + gutter), size, lod, false));
            }
            if gutter != 0 && family.layer_bytes(footprint, lod) <= available {
                let array = self.new_array(device, family, footprint, lod, 0)?;
                return Ok(Region::new(array, 0, [gutter; 2], size, lod, false));
            }
        }
        let mut lod = if family == Family::Image { full_lod(size) } else { 0 };
        while lod > 0 && family.layer_bytes(size, lod) > available {
            lod -= 1;
        }
        let array = self.new_array(device, family, size, lod, 0)?;
        Ok(Region::new(array, 0, [0; 2], size, lod, false))
    }

    fn new_array(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        family: Family,
        size: [u32; 2],
        lod: u8,
        packed_slices: usize,
    ) -> Result<ArrayId, String> {
        let slices = packed_slices.max(1);
        let bytes = family.layer_bytes(size, lod) * slices as u64;
        let working = self.working_bytes + bytes;
        if working > FRAME_BYTES {
            return Err(format!("Metal atlas working set needs {working} bytes, exceeding the {FRAME_BYTES}-byte limit"));
        }
        self.next_array = self.next_array.checked_add(1).ok_or("Metal atlas array identities exhausted")?;
        let id = ArrayId(NonZeroU64::new(self.next_array).expect("nonzero atlas array identity"));
        // SAFETY: reserve bounds the extent and mip count, and uses at most two slices.
        let descriptor = unsafe {
            let descriptor = MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                family.format(),
                size[0] as usize,
                size[1] as usize,
                lod != 0,
            );
            descriptor.setTextureType(MTLTextureType::Type2DArray);
            descriptor.setArrayLength(slices);
            descriptor.setMipmapLevelCount(usize::from(lod) + 1);
            descriptor
        };
        descriptor.setStorageMode(MTLStorageMode::Private);
        descriptor.setUsage(MTLTextureUsage::ShaderRead);
        let texture = device.newTextureWithDescriptor(&descriptor).ok_or("Could not allocate Metal atlas texture array")?;
        let slices = (0..packed_slices).map(|_| Skyline::new(size[0])).collect();
        self.arrays.insert(id, Array { texture, family, slices, bytes, used: self.generation });
        self.working_bytes = working;
        Ok(id)
    }

    fn place_glyph(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        glyph: &GlyphImage,
        uploads: &mut Uploads,
    ) -> Result<SampleBinding, String> {
        if glyph.width == 0 || glyph.height == 0 {
            return Ok(SampleBinding::Missing);
        }
        let region = self.reserve(device, Family::Glyph, [glyph.width, glyph.height])?;
        let data =
            if glyph.color { glyph.data.clone() } else { glyph.data.iter().flat_map(|&alpha| [0, 0, 0, alpha]).collect::<Vec<_>>().into() };
        uploads.push(Upload::new(region, 0, Family::Glyph, Pixels { data, stride: glyph.width as usize * 4, offset: 0 }))?;
        Ok(SampleBinding::Glyph(region))
    }

    fn place_path(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        path: &octara_model::paint::PathPaint,
        at: &paths::Placed,
        uploads: &mut Uploads,
    ) -> Result<SampleBinding, String> {
        if u64::from(at.width) * u64::from(at.height) > FRAME_BYTES {
            return Err("Metal path mask exceeds the atlas frame budget".into());
        }
        let mask = paths::rasterize(path, at);
        let data: Arc<[u8]> = mask.data.into();
        let stride = mask.width as usize;
        let mut tiles = Vec::new();
        for y in (0..mask.height).step_by(PATH_TILE as usize) {
            for x in (0..mask.width).step_by(PATH_TILE as usize) {
                let size = [PATH_TILE.min(mask.width - x), PATH_TILE.min(mask.height - y)];
                let region = self.reserve(device, Family::Mask, size)?;
                let pixels = Pixels { data: data.clone(), stride, offset: y as usize * stride + x as usize };
                uploads.push(Upload::new(region, 0, Family::Mask, pixels))?;
                tiles.push(PathTile { region, offset: [x, y] });
            }
        }
        Ok(SampleBinding::Path(tiles))
    }

    fn place_image(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        image: &Sample,
        uploads: &mut Uploads,
    ) -> Result<SampleBinding, String> {
        let frame = image.frame();
        let size = [frame.width, frame.height];
        if size.contains(&0) {
            return Ok(SampleBinding::Missing);
        }
        let mut region = self.reserve(device, Family::Image, size)?;
        region.opaque = frame.rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255);
        let packed = region.origin != [0; 2];
        let levels = ImageMips::new(size, &frame.rgba, packed, region.max_lod).ok_or("Invalid Metal atlas image data")?;
        for (mip, level) in (0..=region.max_lod).zip(levels) {
            let origin = region.origin.map(|axis| axis >> mip);
            let (origin, level) = if packed { (origin.map(|axis| axis - 1), level.with_gutter()) } else { (origin, level) };
            let destination = Region::new(region.array, region.layer, origin, level.size, 0, false);
            let pixels = Pixels { data: level.pixels, stride: level.size[0] as usize * 4, offset: 0 };
            uploads.push(Upload::new(destination, mip, Family::Image, pixels))?;
        }
        Ok(SampleBinding::Image(region))
    }
}
