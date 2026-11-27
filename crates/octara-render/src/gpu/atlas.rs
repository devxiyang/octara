use super::custom::with_error_scopes_async;
use crate::glyphs::{GlyphCache, GlyphImage, GlyphKey};
use crate::paths::{self, MaskKey};
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
    GlyphBufferSize { bytes: u64, limit: u64 },
    SurfaceRenderer(String),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TextureSize { width, height, limit } => write!(f, "texture {width}×{height} exceeds the GPU dimension limit of {limit}"),
            Self::TextureBudget { bytes, limit } => write!(f, "scene textures need {bytes} bytes, exceeding the {limit}-byte frame budget"),
            Self::TextureAllocation(message) => write!(f, "cannot allocate scene texture: {message}"),
            Self::GlyphBufferSize { bytes, limit } => {
                write!(f, "glyph instances need {bytes} bytes, exceeding the GPU storage buffer limit of {limit}")
            }
            Self::SurfaceRenderer(message) => write!(f, "cannot record surface: {message}"),
        }
    }
}

impl std::error::Error for RecordError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    Mask,
    Glyph,
    Image,
}

impl Family {
    fn edge(self, limit: u32) -> u32 {
        (match self {
            Self::Mask => 4096,
            Self::Glyph | Self::Image => 2048,
        })
        .min(limit)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct PageId(NonZeroU64);

#[derive(Clone, Copy)]
pub(super) struct Region {
    pub page: PageId,
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Bindings(pub [Option<PageId>; 2]);

impl Bindings {
    pub(super) const EMPTY: Self = Self([None; 2]);

    pub(super) fn mask(page: PageId) -> Self {
        Self([Some(page), None])
    }

    pub(super) fn image(page: PageId) -> Self {
        Self([None, Some(page)])
    }

    pub(super) fn merge(&mut self, next: Self) -> bool {
        for (current, next) in self.0.into_iter().zip(next.0) {
            if current.is_some() && next.is_some() && current != next {
                return false;
            }
        }
        for (current, next) in self.0.iter_mut().zip(next.0) {
            *current = current.or(next);
        }
        true
    }
}

#[derive(Clone, Copy)]
struct Shelf {
    edge: u32,
    x: u32,
    y: u32,
    row: u32,
}

impl Shelf {
    fn new(edge: u32) -> Self {
        Self { edge, x: 0, y: 0, row: 0 }
    }

    fn allocate(&mut self, [width, height]: [u32; 2]) -> Option<[u32; 2]> {
        if width > self.edge || height > self.edge {
            return None;
        }
        let (x, y, row) = if self.x + width <= self.edge { (self.x, self.y, self.row) } else { (0, self.y + self.row, 0) };
        if y + height > self.edge {
            return None;
        }
        self.x = x + width + 1;
        self.y = y;
        self.row = row.max(height + 1);
        Some([x, y])
    }
}

struct Page {
    view: wgpu::TextureView,
    shelf: Option<Shelf>,
    bytes: u64,
    used: u64,
}

struct Upload {
    region: Region,
    pixels: Arc<[u8]>,
    stride: u32,
    offset: usize,
}

/// Packed pages and stand-alone images share a lifetime and byte budget. Allocations append
/// to a page. Retiring a page drops its handle and lets wgpu release it after queued reads.
pub(super) struct Atlases {
    pages: FxHashMap<PageId, Page>,
    current: [Option<PageId>; 3],
    next_id: u64,
    frame: u64,
    retained_bytes: u64,
    pub frame_bytes: u64,
    pub upload_bytes: u64,
    pending: Vec<Upload>,
    cache_limit: u64,
    frame_limit: u64,
}

impl Default for Atlases {
    fn default() -> Self {
        Self {
            pages: FxHashMap::default(),
            current: [None; 3],
            next_id: 1,
            frame: 0,
            retained_bytes: 0,
            frame_bytes: 0,
            upload_bytes: 0,
            pending: Vec::new(),
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

    pub(super) fn view(&self, page: PageId) -> &wgpu::TextureView {
        &self.pages[&page].view
    }

    pub(super) fn contains(&self, page: PageId) -> bool {
        self.pages.contains_key(&page)
    }

    pub(super) fn touch(&mut self, page: PageId) -> Result<(), RecordError> {
        let entry = self.pages.get_mut(&page).expect("a cached atlas page");
        if entry.used != self.frame {
            self.frame_bytes = checked_bytes(self.frame_bytes, entry.bytes, self.frame_limit)?;
            entry.used = self.frame;
        }
        Ok(())
    }

    pub(super) async fn stage(
        &mut self,
        device: &wgpu::Device,
        family: Family,
        size: [u32; 2],
        pixels: Arc<[u8]>,
        stride: u32,
        offset: usize,
    ) -> Result<Region, RecordError> {
        let [width, height] = size;
        let limit = device.limits().max_texture_dimension_2d;
        if width > limit || height > limit {
            return Err(RecordError::TextureSize { width, height, limit });
        }
        let edge = family.edge(limit);
        let packed = width <= edge && height <= edge;
        let (page, origin) = if packed {
            self.reserve(device, family, edge, size).await?
        } else {
            (self.new_page(device, family, size, None).await?, [0, 0])
        };
        let bpp = family.bytes_per_pixel();
        let region = Region { page, origin, size };
        let stride = stride.checked_mul(bpp).expect("texture upload row exceeds 32 bits");
        self.pending.push(Upload { region, pixels, stride, offset });
        self.upload_bytes += u64::from(width) * u64::from(height) * u64::from(bpp);
        Ok(region)
    }

    /// Queue all writes immediately before their frame's submission.
    pub(super) fn queue_uploads(&self, queue: &wgpu::Queue) {
        for upload in &self.pending {
            let Region { page, origin: [x, y], size: [width, height] } = upload.region;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self.pages[&page].view.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
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
    }

    /// Drop staged bytes and retire any page with an uninitialized slot.
    pub(super) fn discard_uploads(&mut self) -> bool {
        let discarded = !self.pending.is_empty();
        for upload in self.pending.drain(..) {
            if let Some(page) = self.pages.remove(&upload.region.page) {
                self.retained_bytes -= page.bytes;
                self.frame_bytes -= page.bytes;
            }
        }
        self.upload_bytes = 0;
        discarded
    }

    async fn reserve(
        &mut self,
        device: &wgpu::Device,
        family: Family,
        edge: u32,
        size: [u32; 2],
    ) -> Result<(PageId, [u32; 2]), RecordError> {
        if let Some(id) = self.current[family as usize]
            && let Some(mut shelf) = self.pages.get(&id).and_then(|page| page.shelf)
            && let Some(origin) = shelf.allocate(size)
        {
            self.touch(id)?;
            self.pages.get_mut(&id).expect("the current atlas page").shelf = Some(shelf);
            return Ok((id, origin));
        }
        let mut shelf = Shelf::new(edge);
        let origin = shelf.allocate(size).expect("the entry fits an empty page");
        let id = self.new_page(device, family, [edge; 2], Some(shelf)).await?;
        self.current[family as usize] = Some(id);
        Ok((id, origin))
    }

    async fn new_page(
        &mut self,
        device: &wgpu::Device,
        family: Family,
        [width, height]: [u32; 2],
        shelf: Option<Shelf>,
    ) -> Result<PageId, RecordError> {
        let bytes = u64::from(width) * u64::from(height) * u64::from(family.bytes_per_pixel());
        let frame_bytes = checked_bytes(self.frame_bytes, bytes, self.frame_limit)?;
        let (view, error) = with_error_scopes_async(device, || {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("octara scene texture"),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: family.format(),
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        })
        .await;
        if let Some(error) = error {
            return Err(RecordError::TextureAllocation(error.to_string()));
        }
        let id = PageId(NonZeroU64::new(self.next_id).expect("a nonzero page identity"));
        self.next_id = self.next_id.checked_add(1).expect("atlas page identity exhausted");
        self.frame_bytes = frame_bytes;
        self.retained_bytes += bytes;
        self.pages.insert(id, Page { view, shelf, bytes, used: self.frame });
        Ok(id)
    }

    /// Keep the current working set and at most the cache budget in total when it fits.
    /// The frame's bind groups already own references to every texture it will read.
    pub(super) fn trim(&mut self) -> bool {
        let limit = self.cache_limit.max(self.frame_bytes);
        if self.retained_bytes <= limit {
            return false;
        }
        let mut cold: Vec<_> = self.pages.iter().filter(|(_, page)| page.used != self.frame).map(|(&id, page)| (page.used, id)).collect();
        cold.sort_unstable();
        for (_, id) in cold {
            self.retained_bytes -= self.pages.remove(&id).expect("a cold page").bytes;
            if self.retained_bytes <= limit {
                break;
            }
        }
        true
    }
}

fn checked_bytes(existing: u64, added: u64, limit: u64) -> Result<u64, RecordError> {
    let bytes = existing.saturating_add(added);
    if bytes > limit { Err(RecordError::TextureBudget { bytes, limit }) } else { Ok(bytes) }
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
        let discarded = self.atlases.discard_uploads();
        if self.atlases.trim() || discarded {
            self.image_slots.retain(|_, region| self.atlases.contains(region.page));
            self.glyph_slots.retain(|_, slot| self.atlases.contains(slot.page));
            self.path_slots.retain(|_, tiles| tiles.iter().all(|tile| self.atlases.contains(tile.region.page)));
        }
    }

    pub(super) async fn path_tiles(&mut self, device: &wgpu::Device, p: &PathPaint, at: &paths::Placed) -> Result<Vec<Tile>, RecordError> {
        if let Some(tiles) = self.path_slots.get(&at.key) {
            for tile in tiles {
                self.atlases.touch(tile.region.page)?;
            }
            return Ok(tiles.clone());
        }
        let mask = paths::rasterize(p, at);
        let pixels: Arc<[u8]> = mask.data.into();
        let edge = TILE.min(device.limits().max_texture_dimension_2d);
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
        if let Some(&region) = self.image_slots.get(&key) {
            self.atlases.touch(region.page)?;
            return Ok(region);
        }
        let region = self.atlases.stage(device, Family::Image, [f.width, f.height], f.rgba.clone(), f.width, 0).await?;
        self.image_slots.insert(key, region);
        Ok(region)
    }

    pub(super) async fn glyph_slot(&mut self, device: &wgpu::Device, key: GlyphKey, image: GlyphImage) -> Result<Region, RecordError> {
        if let Some(&slot) = self.glyph_slots.get(&key) {
            self.atlases.touch(slot.page)?;
            return Ok(slot);
        }
        let limit = device.limits().max_texture_dimension_2d.min(u16::MAX.into());
        if image.width > limit || image.height > limit {
            return Err(RecordError::TextureSize { width: image.width, height: image.height, limit });
        }
        let family = if image.color { Family::Glyph } else { Family::Mask };
        let region = self.atlases.stage(device, family, [image.width, image.height], image.data, image.width, 0).await?;
        self.glyph_slots.insert(key, region);
        Ok(region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusing_a_slot_leaves_space_for_a_smaller_entry() {
        let mut shelf = Shelf::new(8);
        assert_eq!(shelf.allocate([4, 4]), Some([0, 0]));
        assert_eq!(shelf.allocate([9, 1]), None);
        assert_eq!(shelf.allocate([4, 4]), None);
        assert_eq!(shelf.allocate([1, 1]), Some([5, 0]));
        assert_eq!(shelf.allocate([4, 3]), Some([0, 5]));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn frame_budget_and_cold_eviction_keep_touched_textures() {
        let Some(gpu) = super::super::GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let width = 2049;
        let bytes = u64::from(width) * 4;
        let pixels: Arc<[u8]> = vec![255; bytes as usize].into();
        let mut atlases = Atlases { cache_limit: bytes, frame_limit: 2 * bytes, ..Atlases::default() };
        let upload =
            |atlases: &mut Atlases| pollster::block_on(atlases.stage(gpu.device(), Family::Image, [width, 1], pixels.clone(), width, 0));
        atlases.begin_frame();
        let first = upload(&mut atlases).unwrap().page;
        let second = upload(&mut atlases).unwrap().page;
        assert!(!atlases.trim());
        assert_eq!(atlases.frame_bytes, 2 * bytes);
        atlases.queue_uploads(gpu.queue());
        gpu.queue().submit([]);
        atlases.commit_uploads();

        atlases.begin_frame();
        atlases.touch(second).unwrap();
        assert!(atlases.trim());
        assert!(atlases.contains(second));
        assert!(!atlases.contains(first));
        assert_eq!(atlases.upload_bytes, 0);
        let staged = upload(&mut atlases).unwrap().page;
        assert_ne!(staged, first);
        assert!(matches!(upload(&mut atlases), Err(RecordError::TextureBudget { .. })));
        assert!(atlases.discard_uploads());
        assert!(!atlases.contains(staged));
        assert!(atlases.contains(second));
        assert_eq!((atlases.frame_bytes, atlases.upload_bytes), (bytes, 0));
    }
}
