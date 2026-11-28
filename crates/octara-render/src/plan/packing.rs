//! Packed texture regions and the shared CPU rectangle allocator.

use crate::glyphs::GlyphImage;
use std::num::NonZeroU64;

pub(crate) fn glyph_atlas_edge(texture_limit: u32) -> u32 {
    1 << 1024_u32.min(texture_limit).ilog2()
}

/// Identity of the texture array bound for a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArrayId(pub NonZeroU64);

/// A sample's pixel rectangle, layer, and texture-array binding.
#[derive(Clone, Copy)]
#[non_exhaustive]
pub struct Region {
    pub array: ArrayId,
    pub layer: u16,
    pub origin: [u32; 2],
    pub size: [u32; 2],
    pub max_lod: u8,
    pub opaque: bool,
    #[cfg(feature = "wgpu")]
    pub(crate) id: u64,
}

impl Region {
    pub fn new(array: ArrayId, layer: u16, origin: [u32; 2], size: [u32; 2], max_lod: u8, opaque: bool) -> Self {
        Self {
            array,
            layer,
            origin,
            size,
            max_lod,
            opaque,
            #[cfg(feature = "wgpu")]
            id: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Box2 {
    pub(crate) origin: [u32; 2],
    pub(crate) size: [u32; 2],
}

impl Box2 {
    fn end(self) -> [u32; 2] {
        std::array::from_fn(|i| self.origin[i] + self.size[i])
    }

    #[cfg(feature = "wgpu")]
    fn join(self, other: Self) -> Option<Self> {
        for cross in 0..2 {
            let axis = 1 - cross;
            if self.origin[cross] == other.origin[cross]
                && self.size[cross] == other.size[cross]
                && (self.end()[axis] == other.origin[axis] || other.end()[axis] == self.origin[axis])
            {
                let mut output = self;
                output.origin[axis] = self.origin[axis].min(other.origin[axis]);
                output.size[axis] += other.size[axis];
                return Some(output);
            }
        }
        None
    }
}

#[derive(Clone)]
pub(crate) struct Guillotine {
    free: Vec<Box2>,
    #[cfg(feature = "wgpu")]
    size: [u32; 2],
    pub(crate) allocated: usize,
}

impl Guillotine {
    pub(crate) fn new(size: [u32; 2]) -> Self {
        Self {
            free: vec![Box2 { origin: [0; 2], size }],
            #[cfg(feature = "wgpu")]
            size,
            allocated: 0,
        }
    }

    pub(crate) fn allocate(&mut self, size: [u32; 2], alignment: u32) -> Option<Box2> {
        let (index, rect) = self
            .free
            .iter()
            .enumerate()
            .filter_map(|(index, rect)| {
                let origin = rect.origin.map(|axis| axis.div_ceil(alignment) * alignment);
                let fits = (0..2).all(|i| origin[i] + size[i] <= rect.origin[i] + rect.size[i]);
                fits.then_some((index, Box2 { origin, size }))
            })
            .min_by_key(|(index, _)| self.free[*index].size.into_iter().product::<u32>())?;
        let free = self.free.swap_remove(index);
        let [left, top] = free.origin;
        let [right, bottom] = free.end();
        let [x, y] = rect.origin;
        let [x1, y1] = rect.end();

        // Peel off any strips used for alignment, then split the remaining L shape.
        self.add([left, top], [x, bottom]);
        self.add([x, top], [right, y]);
        if right - x1 > bottom - y1 {
            self.add([x1, y], [right, bottom]);
            self.add([x, y1], [x1, bottom]);
        } else {
            self.add([x1, y], [right, y1]);
            self.add([x, y1], [right, bottom]);
        }
        self.allocated += 1;
        Some(rect)
    }

    fn add(&mut self, origin: [u32; 2], end: [u32; 2]) {
        if origin[0] < end[0] && origin[1] < end[1] {
            self.free.push(Box2 { origin, size: std::array::from_fn(|axis| end[axis] - origin[axis]) });
        }
    }

    #[cfg(feature = "wgpu")]
    pub(crate) fn release(&mut self, mut rect: Box2) {
        self.allocated -= 1;
        if self.allocated == 0 {
            self.free.clear();
            self.free.push(Box2 { origin: [0; 2], size: self.size });
            return;
        }
        while let Some((index, joined)) = self.free.iter().enumerate().find_map(|(i, &other)| rect.join(other).map(|joined| (i, joined))) {
            self.free.swap_remove(index);
            rect = joined;
        }
        self.free.push(rect);
    }
}

/// Device-free packing for text scenes that fit in one layer of a glyph atlas.
#[derive(Clone)]
pub(crate) struct TextAtlas(Guillotine);

impl TextAtlas {
    pub(crate) fn new(texture_limit: u32) -> Self {
        Self(Guillotine::new([glyph_atlas_edge(texture_limit); 2]))
    }

    pub(crate) fn place(&mut self, image: &GlyphImage) -> Option<(Region, u64)> {
        let size = [image.width, image.height];
        let footprint = self.0.allocate(size, 1)?;
        let array = ArrayId(NonZeroU64::MIN);
        let region = Region::new(array, 0, footprint.origin, size, 0, false);
        let bytes = u64::from(image.width) * u64::from(image.height) * 4;
        Some((region, bytes))
    }
}
