use super::custom::with_error_scopes_async;
use crate::glyphs::{GlyphCache, GlyphImage, GlyphKey};
use crate::image_mips::{IMAGE_ATLAS_EDGE, ImageLevel, ImageMips, PACKED_IMAGE_MAX_LOD, full_lod, opaque, packed_footprint, packed_lod};
use crate::paths::{self, MaskKey};
use crate::plan::packing::{Box2, Guillotine, glyph_atlas_edge};
use crate::plan::{ArrayId, Region};
use octara_model::paint::{ImagePaint, PathPaint};
use rustc_hash::FxHashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

const MIB: u64 = 1024 * 1024;
const CACHE_BYTES: u64 = 64 * MIB;
const FRAME_BYTES: u64 = 256 * MIB;

/// A scene or an extension could not be recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    TextureSize { width: u32, height: u32, limit: u32 },
    TextureBudget { bytes: u64, limit: u64 },
    TextureAllocation(String),
    InstanceBufferSize { bytes: u64, limit: u64 },
    SurfaceRenderer(String),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TextureSize { width, height, limit } => write!(f, "texture {width}×{height} exceeds the GPU dimension limit of {limit}"),
            Self::TextureBudget { bytes, limit } => write!(f, "scene textures need {bytes} bytes, exceeding the {limit}-byte frame budget"),
            Self::TextureAllocation(message) => write!(f, "cannot allocate scene texture: {message}"),
            Self::InstanceBufferSize { bytes, limit } => {
                write!(f, "render instances need {bytes} bytes, exceeding the GPU storage buffer limit of {limit}")
            }
            Self::SurfaceRenderer(message) => write!(f, "cannot record surface: {message}"),
        }
    }
}

impl std::error::Error for RecordError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Mask,
    Glyph,
    Image,
}

impl Family {
    fn edge(self, limit: u32) -> u32 {
        // A power-of-two image edge keeps the packed mip coordinate grid exact.
        let edge = match self {
            Self::Mask => 2048,
            Self::Glyph => return glyph_atlas_edge(limit),
            Self::Image => IMAGE_ATLAS_EDGE,
        };
        1 << edge.min(limit).ilog2()
    }

    fn format(self) -> wgpu::TextureFormat {
        match self {
            Self::Mask => wgpu::TextureFormat::R8Unorm,
            Self::Glyph | Self::Image => wgpu::TextureFormat::Rgba8UnormSrgb,
        }
    }

    fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Mask => 1,
            Self::Glyph | Self::Image => 4,
        }
    }
}

struct Array {
    view: wgpu::TextureView,
    family: Family,
    // Empty for a dedicated texture, which always uses layer zero and has no free rectangles.
    layers: Vec<Guillotine>,
    bytes: u64,
    used: u64,
    active: usize,
    slots: usize,
}

struct Slot {
    array: ArrayId,
    layer: u16,
    footprint: Box2,
    used: u64,
}

struct Upload {
    array: ArrayId,
    layer: u16,
    level: u8,
    rect: Box2,
    pixels: Arc<[u8]>,
    stride: u32,
    offset: usize,
}

/// Packed arrays share bindings across layers. Every allocation has its own lifetime, so
/// discarding writes or reusing an old region keeps the other regions in that array resident.
pub(super) struct Atlases {
    arrays: FxHashMap<ArrayId, Array>,
    packed: [Vec<ArrayId>; 3],
    slots: FxHashMap<u64, Slot>,
    next_id: u64,
    frame: u64,
    retained_bytes: u64,
    pub frame_bytes: u64,
    pub upload_bytes: u64,
    pending_slots: Vec<u64>,
    pending: Vec<Upload>,
    invalidated: bool,
    cache_limit: u64,
    frame_limit: u64,
}

impl Default for Atlases {
    fn default() -> Self {
        Self {
            arrays: FxHashMap::default(),
            packed: std::array::from_fn(|_| Vec::new()),
            slots: FxHashMap::default(),
            next_id: 1,
            frame: 0,
            retained_bytes: 0,
            frame_bytes: 0,
            upload_bytes: 0,
            pending_slots: Vec::new(),
            pending: Vec::new(),
            invalidated: false,
            cache_limit: CACHE_BYTES,
            frame_limit: FRAME_BYTES,
        }
    }
}

impl Atlases {
    pub(super) fn begin_frame(&mut self) {
        debug_assert!(self.pending.is_empty());
        self.frame = self.frame.checked_add(1).expect("atlas frame identity exhausted");
        self.frame_bytes = 0;
        self.upload_bytes = 0;
    }

    pub(super) fn view(&self, array: ArrayId) -> &wgpu::TextureView {
        &self.arrays[&array].view
    }

    fn next_id(&mut self) -> NonZeroU64 {
        let id = NonZeroU64::new(self.next_id).expect("a nonzero atlas identity");
        self.next_id = self.next_id.checked_add(1).expect("atlas identity exhausted");
        id
    }

    fn contains(&self, region: Region) -> bool {
        self.slots.contains_key(&region.id)
    }

    fn touch(&mut self, region: Region) -> Result<bool, RecordError> {
        let Some(slot) = self.slots.get_mut(&region.id) else { return Ok(false) };
        if slot.used != self.frame {
            let array = self.arrays.get_mut(&slot.array).expect("an atlas array");
            if array.used != self.frame || array.active == 0 {
                self.frame_bytes = checked_bytes(self.frame_bytes, array.bytes, self.frame_limit)?;
                array.active = 0;
            }
            array.used = self.frame;
            array.active += 1;
            slot.used = self.frame;
        }
        Ok(true)
    }

    async fn stage(
        &mut self,
        device: &wgpu::Device,
        family: Family,
        size: [u32; 2],
        pixels: Arc<[u8]>,
        stride: u32,
        offset: usize,
    ) -> Result<Region, RecordError> {
        let region = self.reserve(device, family, size).await?;
        let rect = Box2 { origin: region.origin, size };
        let stride = stride.checked_mul(family.bytes_per_pixel()).expect("texture upload row exceeds 32 bits");
        self.push_upload(region, 0, rect, pixels, stride, offset);
        Ok(region)
    }

    async fn stage_image(&mut self, device: &wgpu::Device, size: [u32; 2], pixels: &Arc<[u8]>) -> Result<Region, RecordError> {
        let mut region = self.reserve(device, Family::Image, size).await?;
        region.opaque = opaque(pixels);
        // A padded image always starts one coarse texel into its reservation, even
        // when its mip chain has to spill into a dedicated array.
        let padded = region.origin != [0; 2];
        let base = ImageLevel::base(size, pixels, region.opaque);
        for (lod, level) in (0..=region.max_lod).zip(ImageMips::from_base(base, padded, region.max_lod)) {
            let origin = region.origin.map(|axis| axis >> lod);
            if padded {
                let size = level.size.map(|axis| axis + 2);
                let rect = Box2 { origin: origin.map(|axis| axis - 1), size };
                let pixels = extrude_edges(&level);
                self.push_upload(region, lod, rect, pixels, size[0] * 4, 0);
            } else {
                let size = level.size;
                self.push_upload(region, lod, Box2 { origin, size }, level.pixels, size[0] * 4, 0);
            }
        }
        Ok(region)
    }

    fn push_upload(&mut self, region: Region, level: u8, rect: Box2, pixels: Arc<[u8]>, stride: u32, offset: usize) {
        let bpp = self.arrays[&region.array].family.bytes_per_pixel();
        self.upload_bytes += u64::from(rect.size[0]) * u64::from(rect.size[1]) * u64::from(bpp);
        self.pending.push(Upload { array: region.array, layer: region.layer, level, rect, pixels, stride, offset });
    }

    /// Queue all writes immediately before their frame's submission.
    pub(super) fn queue_uploads(&self, queue: &wgpu::Queue) {
        for upload in &self.pending {
            let Box2 { origin: [x, y], size: [width, height] } = upload.rect;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self.arrays[&upload.array].view.texture(),
                    mip_level: u32::from(upload.level),
                    origin: wgpu::Origin3d { x, y, z: u32::from(upload.layer) },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.pixels[upload.offset..],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(upload.stride), rows_per_image: Some(height) },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
        }
    }

    pub(super) fn commit_uploads(&mut self) {
        self.pending.clear();
        self.pending_slots.clear();
    }

    fn discard_uploads(&mut self) {
        self.pending.clear();
        while let Some(id) = self.pending_slots.pop() {
            let array = self.release(id);
            if self.arrays[&array].slots == 0 {
                self.retire_array(array);
            }
        }
        self.upload_bytes = 0;
    }

    fn release(&mut self, id: u64) -> ArrayId {
        let slot = self.slots.remove(&id).expect("a live atlas region");
        let array = self.arrays.get_mut(&slot.array).expect("an atlas array");
        if let Some(layer) = array.layers.get_mut(usize::from(slot.layer)) {
            layer.release(slot.footprint);
        }
        if slot.used == self.frame {
            array.active -= 1;
            if array.active == 0 {
                self.frame_bytes -= array.bytes;
            }
        }
        array.slots -= 1;
        self.invalidated = true;
        slot.array
    }

    fn retire_array(&mut self, id: ArrayId) {
        let array = self.arrays.remove(&id).expect("a live atlas array");
        debug_assert!(array.used != self.frame || array.active == 0);
        self.retained_bytes -= array.bytes;
        self.packed[array.family as usize].retain(|&array| array != id);
        if array.slots != 0 {
            self.slots.retain(|_, slot| slot.array != id);
            self.invalidated = true;
        }
    }

    async fn reserve(&mut self, device: &wgpu::Device, family: Family, size: [u32; 2]) -> Result<Region, RecordError> {
        let limits = device.limits();
        texture_size(size, limits.max_texture_dimension_2d)?;
        let edge = family.edge(limits.max_texture_dimension_2d);
        let available = self.frame_limit - self.frame_bytes;
        let packed = match family {
            Family::Image => packed_lod(size, IMAGE_ATLAS_EDGE).map(|lod| (packed_footprint(size, lod), lod, 1 << lod)),
            _ => Some((size, 0, 0)),
        };
        if let Some((footprint, lod, gutter)) = packed
            && footprint.into_iter().all(|axis| axis <= edge)
        {
            let alignment = 1 << lod;
            if let Some((array, layer, rect)) = self.reserve_packed(family, footprint, alignment) {
                return self.insert(array, layer, rect, size, lod, gutter);
            }

            let array_lod = if family == Family::Image { full_lod([edge; 2]).min(PACKED_IMAGE_MAX_LOD) } else { 0 };
            let layer_bytes = array_bytes(family, [edge; 2], array_lod);
            let layers = (available / layer_bytes).min(2).min(u64::from(limits.max_texture_array_layers)) as u32;
            if layers != 0 {
                let array = self.new_array(device, family, [edge; 2], layers, array_lod, true).await?;
                let rect = self.arrays.get_mut(&array).expect("a new array").layers[0]
                    .allocate(footprint, alignment)
                    .expect("the region fits an empty layer");
                return self.insert(array, 0, rect, size, lod, gutter);
            }
        }

        if let Some((footprint, lod, gutter)) = packed
            && gutter != 0
            && footprint.into_iter().all(|axis| axis <= limits.max_texture_dimension_2d)
            && array_bytes(family, footprint, lod) <= available
        {
            // A footprint aligned to its coarsest mip retains the same coordinates and
            // sampling on a dedicated texture as it does in the shared power-of-two array.
            let array = self.new_array(device, family, footprint, 1, lod, false).await?;
            return self.insert(array, 0, Box2 { origin: [0; 2], size: footprint }, size, lod, gutter);
        }

        // A tight budget or a small device can accommodate the original pixels without
        // the padded footprint. Hardware clamp-to-edge isolates this dedicated image.
        let mut lod = if family == Family::Image { full_lod(size) } else { 0 };
        while lod > 0 && checked_bytes(self.frame_bytes, array_bytes(family, size, lod), self.frame_limit).is_err() {
            lod -= 1;
        }
        let array = self.new_array(device, family, size, 1, lod, false).await?;
        self.insert(array, 0, Box2 { origin: [0; 2], size }, size, lod, 0)
    }

    fn array_fits_frame(&self, array: &Array) -> bool {
        (array.used == self.frame && array.active != 0) || array.bytes <= self.frame_limit - self.frame_bytes
    }

    fn find_packed(&mut self, family: Family, size: [u32; 2], alignment: u32) -> Option<(ArrayId, u16, Box2)> {
        for &id in &self.packed[family as usize] {
            if !self.array_fits_frame(&self.arrays[&id]) {
                continue;
            }
            let array = self.arrays.get_mut(&id).expect("a packed atlas array");
            for (layer, capacity) in array.layers.iter_mut().enumerate() {
                if let Some(rect) = capacity.allocate(size, alignment) {
                    return Some((id, layer as u16, rect));
                }
            }
        }
        None
    }

    fn reserve_packed(&mut self, family: Family, size: [u32; 2], alignment: u32) -> Option<(ArrayId, u16, Box2)> {
        if self.packed[family as usize].is_empty() {
            return None;
        }
        if let Some(found) = self.find_packed(family, size, alignment) {
            return Some(found);
        }
        let mut cold = self
            .slots
            .iter()
            .filter_map(|(&id, slot)| {
                let array = &self.arrays[&slot.array];
                (slot.used != self.frame && array.family == family && !array.layers.is_empty() && self.array_fits_frame(array))
                    .then_some((slot.used, id))
            })
            .collect::<Vec<_>>();
        cold.sort_unstable();
        for (_, id) in cold {
            let array = self.release(id);
            if let Some(found) = self.find_packed(family, size, alignment) {
                return Some(found);
            }
            debug_assert!(self.arrays[&array].slots != 0, "the region fits an empty array");
        }
        None
    }

    fn insert(
        &mut self,
        array: ArrayId,
        layer: u16,
        footprint: Box2,
        size: [u32; 2],
        max_lod: u8,
        gutter: u32,
    ) -> Result<Region, RecordError> {
        let id = self.next_id().get();
        let region = Region { array, layer, origin: footprint.origin.map(|axis| axis + gutter), size, max_lod, opaque: false, id };
        self.slots.insert(id, Slot { array, layer, footprint, used: 0 });
        self.arrays.get_mut(&array).expect("an atlas array").slots += 1;
        let resident = self.touch(region)?;
        debug_assert!(resident, "the region was just allocated");
        self.pending_slots.push(id);
        Ok(region)
    }

    async fn new_array(
        &mut self,
        device: &wgpu::Device,
        family: Family,
        size: [u32; 2],
        layers: u32,
        lod: u8,
        packed: bool,
    ) -> Result<ArrayId, RecordError> {
        let bytes = array_bytes(family, size, lod) * u64::from(layers);
        checked_bytes(self.frame_bytes, bytes, self.frame_limit)?;
        let (view, error) = with_error_scopes_async(device, || {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("octara scene texture array"),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: layers },
                    mip_level_count: u32::from(lod) + 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: family.format(),
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() })
        })
        .await;
        if let Some(error) = error {
            return Err(RecordError::TextureAllocation(error.to_string()));
        }
        let id = ArrayId(self.next_id());
        let layers = if packed { (0..layers).map(|_| Guillotine::new(size)).collect() } else { Vec::new() };
        self.arrays.insert(id, Array { view, family, layers, bytes, used: 0, active: 0, slots: 0 });
        if packed {
            self.packed[family as usize].push(id);
        }
        self.retained_bytes += bytes;
        Ok(id)
    }

    /// Keep the current working set and at most the cache budget in total when it fits.
    fn trim(&mut self) {
        let limit = self.cache_limit.max(self.frame_bytes);
        if self.retained_bytes <= limit {
            return;
        }
        let mut cold = self
            .arrays
            .iter()
            .filter_map(|(&id, array)| (array.used != self.frame || array.active == 0).then_some((array.used, id)))
            .collect::<Vec<_>>();
        cold.sort_unstable();
        for (_, id) in cold {
            self.retire_array(id);
            if self.retained_bytes <= limit {
                break;
            }
        }
    }
}

fn texture_size([width, height]: [u32; 2], limit: u32) -> Result<(), RecordError> {
    if width > limit || height > limit { Err(RecordError::TextureSize { width, height, limit }) } else { Ok(()) }
}

fn array_bytes(family: Family, [w, h]: [u32; 2], lod: u8) -> u64 {
    (0..=lod).map(|level| u64::from((w >> level).max(1)) * u64::from((h >> level).max(1))).sum::<u64>()
        * u64::from(family.bytes_per_pixel())
}

fn checked_bytes(existing: u64, added: u64, limit: u64) -> Result<u64, RecordError> {
    let bytes = existing.saturating_add(added);
    if bytes > limit { Err(RecordError::TextureBudget { bytes, limit }) } else { Ok(bytes) }
}

fn extrude_edges(level: &ImageLevel) -> Arc<[u8]> {
    let [width, height] = level.size.map(|axis| axis as usize);
    let stride = width * 4;
    let mut padded = Vec::with_capacity((width + 2) * (height + 2) * 4);
    for y in 0..height + 2 {
        let start = y.saturating_sub(1).min(height - 1) * stride;
        let row = &level.pixels[start..start + stride];
        padded.extend_from_slice(&row[..4]);
        padded.extend_from_slice(row);
        padded.extend_from_slice(&row[stride - 4..]);
    }
    padded.into()
}

/// The largest piece of a path's mask in the atlas.
const TILE: u32 = 1024;

/// A piece of a path's mask and its offset within the full raster.
#[derive(Clone, Copy)]
pub(super) struct Tile {
    pub(super) region: Region,
    pub(super) offset: [u32; 2],
}

#[derive(Default)]
pub(super) struct SceneTextures {
    pub(super) atlases: Atlases,
    image_slots: FxHashMap<(u64, u64), Region>,
    glyph_slots: FxHashMap<GlyphKey, Region>,
    path_slots: FxHashMap<MaskKey, Vec<Tile>>,
    pub(super) glyphs: GlyphCache,
}

impl SceneTextures {
    pub(super) fn finish_frame(&mut self) {
        self.atlases.discard_uploads();
        self.atlases.trim();
        if std::mem::take(&mut self.atlases.invalidated) {
            self.image_slots.retain(|_, &mut region| self.atlases.contains(region));
            self.glyph_slots.retain(|_, &mut region| self.atlases.contains(region));
            self.path_slots.retain(|_, tiles| tiles.iter().all(|tile| self.atlases.contains(tile.region)));
        }
    }

    pub(super) fn pin_image(&mut self, i: &ImagePaint) -> Result<(), RecordError> {
        let key = (i.image.source().id(), i.image.frame().version);
        if let Some(&region) = self.image_slots.get(&key) {
            self.atlases.touch(region)?;
        }
        Ok(())
    }

    pub(super) fn pin_glyph(&mut self, key: &GlyphKey) -> Result<(), RecordError> {
        if let Some(&region) = self.glyph_slots.get(key) {
            self.atlases.touch(region)?;
        }
        Ok(())
    }

    pub(super) fn pin_path(&mut self, at: &paths::Placed) -> Result<(), RecordError> {
        if let Some(tiles) = self.path_slots.get(&at.key) {
            for tile in tiles {
                self.atlases.touch(tile.region)?;
            }
        }
        Ok(())
    }

    pub(super) async fn path_tiles(&mut self, device: &wgpu::Device, p: &PathPaint, at: &paths::Placed) -> Result<Vec<Tile>, RecordError> {
        if let Some(tiles) = self.path_slots.get(&at.key)
            && tiles.iter().all(|tile| self.atlases.contains(tile.region))
        {
            self.pin_path(at)?;
            return Ok(self.path_slots[&at.key].clone());
        }
        let mask = paths::rasterize(p, at);
        let pixels: Arc<[u8]> = mask.data.into();
        let edge = TILE.min(Family::Mask.edge(device.limits().max_texture_dimension_2d));
        let mut tiles = Vec::new();
        for y in (0..mask.height).step_by(edge as usize) {
            for x in (0..mask.width).step_by(edge as usize) {
                let size = [edge.min(mask.width - x), edge.min(mask.height - y)];
                let start = y as usize * mask.width as usize + x as usize;
                let region = self.atlases.stage(device, Family::Mask, size, pixels.clone(), mask.width, start).await?;
                tiles.push(Tile { region, offset: [x, y] });
            }
        }
        self.path_slots.insert(at.key, tiles.clone());
        Ok(tiles)
    }

    pub(super) async fn image_slot(&mut self, device: &wgpu::Device, i: &ImagePaint) -> Result<Region, RecordError> {
        let f = i.image.frame();
        let key = (i.image.source().id(), f.version);
        if let Some(&region) = self.image_slots.get(&key)
            && self.atlases.touch(region)?
        {
            return Ok(region);
        }
        let region = self.atlases.stage_image(device, [f.width, f.height], &f.rgba).await?;
        self.image_slots.insert(key, region);
        Ok(region)
    }

    pub(super) async fn glyph_slot(&mut self, device: &wgpu::Device, key: GlyphKey, image: GlyphImage) -> Result<Region, RecordError> {
        if let Some(&region) = self.glyph_slots.get(&key)
            && self.atlases.touch(region)?
        {
            return Ok(region);
        }
        let limit = device.limits().max_texture_dimension_2d.min(u16::MAX.into());
        texture_size([image.width, image.height], limit)?;
        let pixels =
            if image.color { image.data } else { image.data.iter().flat_map(|&alpha| [0, 0, 0, alpha]).collect::<Vec<_>>().into() };
        let region = self.atlases.stage(device, Family::Glyph, [image.width, image.height], pixels, image.width, 0).await?;
        self.glyph_slots.insert(key, region);
        Ok(region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guillotine_reuses_freed_space_and_honors_mip_alignment() {
        let mut allocator = Guillotine::new([8, 8]);
        let first = allocator.allocate([3, 4], 1).unwrap();
        assert_eq!(first.origin, [0, 0]);
        assert!(allocator.allocate([9, 1], 1).is_none());
        let aligned = allocator.allocate([4, 4], 4).unwrap();
        assert_eq!(aligned.origin, [4, 0]);

        allocator.release(first);
        let reused = allocator.allocate([3, 8], 1).unwrap();
        assert_eq!(reused.origin, first.origin);
        allocator.release(aligned);
        allocator.release(reused);
        assert_eq!(allocator.allocate([8, 8], 1).unwrap().origin, [0, 0]);
    }

    #[test]
    fn image_mips_filter_premultiplied_light_and_repeat_their_edges() {
        let rgba: Arc<[u8]> = [255, 0, 0, 255, 0, 0, 255, 0].into();
        let mut levels = ImageMips::new([2, 1], &rgba, true, 1).unwrap();
        let base = levels.next().unwrap();
        assert_eq!(&*base.pixels, &[255, 0, 0, 255, 0, 0, 0, 0]);
        let level = levels.next().unwrap();
        assert_eq!(level.size, [1, 1]);
        assert_eq!(&*level.pixels, &[188, 0, 0, 128]);
        assert_eq!(&*extrude_edges(&level), &[188, 0, 0, 128].repeat(9));
        assert!(levels.next().is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn padded_images_keep_their_mip_geometry_when_a_shared_array_does_not_fit() {
        let Some(gpu) = super::super::WgpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let size = [17, 9];
        let texels = (size[0] * size[1]) as usize;
        let mut rgba = [255, 0, 0, 255, 0, 0, 255, 128].repeat(texels.div_ceil(2));
        rgba.truncate(texels * 4);
        let pixels = rgba.into();
        let lod = packed_lod(size, IMAGE_ATLAS_EDGE).unwrap();
        let footprint = packed_footprint(size, lod);
        let mut spill = Atlases { frame_limit: array_bytes(Family::Image, footprint, lod), ..Atlases::default() };
        let mut shared = Atlases::default();
        for atlas in [&mut shared, &mut spill] {
            atlas.begin_frame();
            let image = crate::block_on(atlas.stage_image(gpu.device(), size, &pixels)).unwrap();
            assert_eq!((image.origin, image.max_lod), ([1 << lod; 2], lod));
            assert!(!image.opaque);
            atlas.queue_uploads(gpu.queue());
            gpu.queue().submit([]);
            atlas.commit_uploads();
        }
        assert!(spill.frame_bytes < shared.frame_bytes);
        assert_eq!(spill.upload_bytes, shared.upload_bytes);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn regions_share_array_layers_and_discard_preserves_committed_neighbors() {
        let Some(gpu) = super::super::WgpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let edge = Family::Mask.edge(gpu.device().limits().max_texture_dimension_2d);
        let pixel_count = edge as usize * edge as usize;
        let pixels: Arc<[u8]> = vec![255; pixel_count].into();
        let mut atlases = Atlases::default();
        let stage = |atlases: &mut Atlases, size| {
            crate::block_on(atlases.stage(gpu.device(), Family::Mask, size, pixels.clone(), edge, 0)).unwrap()
        };

        atlases.begin_frame();
        let keep = stage(&mut atlases, [edge / 2, edge]);
        let cold = stage(&mut atlases, [edge / 2, edge]);
        let other_layer = stage(&mut atlases, [edge, edge]);
        assert_eq!((keep.array, keep.layer), (cold.array, cold.layer));
        assert_eq!(other_layer.array, cold.array);
        assert_ne!(other_layer.layer, keep.layer);
        atlases.queue_uploads(gpu.queue());
        gpu.queue().submit([]);
        atlases.commit_uploads();

        atlases.begin_frame();
        atlases.touch(keep).unwrap();
        atlases.touch(other_layer).unwrap();
        let replacement = stage(&mut atlases, [edge / 2, edge]);
        assert_eq!((replacement.array, replacement.layer, replacement.origin), (cold.array, cold.layer, cold.origin));
        assert!(!atlases.contains(cold));
        assert_ne!(replacement.id, cold.id);
        atlases.discard_uploads();
        assert!(!atlases.contains(replacement));
        assert!(atlases.contains(keep));
        assert!(atlases.contains(other_layer));
        assert_eq!(atlases.frame_bytes, 2 * pixel_count as u64);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn frame_budget_and_cold_eviction_keep_touched_arrays() {
        let Some(gpu) = super::super::WgpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let width = 1025;
        let bytes = u64::from(width) * 4;
        let pixels: Arc<[u8]> = vec![255; bytes as usize].into();
        let mut atlases = Atlases { cache_limit: bytes, frame_limit: 2 * bytes, ..Atlases::default() };
        let upload =
            |atlases: &mut Atlases| crate::block_on(atlases.stage(gpu.device(), Family::Glyph, [width, 1], pixels.clone(), width, 0));
        atlases.begin_frame();
        let first = upload(&mut atlases).unwrap();
        let second = upload(&mut atlases).unwrap();
        atlases.trim();
        assert!(atlases.contains(first));
        assert_eq!(atlases.frame_bytes, 2 * bytes);
        atlases.queue_uploads(gpu.queue());
        gpu.queue().submit([]);
        atlases.commit_uploads();

        atlases.begin_frame();
        atlases.touch(second).unwrap();
        atlases.trim();
        assert!(atlases.contains(second));
        assert!(!atlases.contains(first));
        let staged = upload(&mut atlases).unwrap();
        assert_ne!(staged.array, first.array);
        assert!(matches!(upload(&mut atlases), Err(RecordError::TextureBudget { .. })));
        atlases.discard_uploads();
        assert!(!atlases.contains(staged));
        assert!(atlases.contains(second));
        assert_eq!((atlases.frame_bytes, atlases.upload_bytes), (bytes, 0));
    }
}
