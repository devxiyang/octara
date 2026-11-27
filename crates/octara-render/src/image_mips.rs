//! Premultiplied linear-light image levels shared by the GPU atlas and its CPU reference.

use color::{AlphaColor, LinearSrgb, Srgb};
use std::sync::{Arc, LazyLock};

pub(crate) const IMAGE_ATLAS_EDGE: u32 = 2048;
pub(crate) const PACKED_IMAGE_MAX_LOD: u8 = 6;

pub(crate) fn full_lod(size: [u32; 2]) -> u8 {
    size[0].max(size[1]).ilog2() as u8
}

pub(crate) fn packed_footprint(size: [u32; 2], lod: u8) -> [u32; 2] {
    let alignment = 1 << lod;
    size.map(|axis| (axis.div_ceil(alignment) + 2) * alignment)
}

pub(crate) fn packed_lod(size: [u32; 2], edge: u32) -> Option<u8> {
    if size.into_iter().any(|axis| axis > edge) {
        return None;
    }
    let lod = full_lod(size).min(PACKED_IMAGE_MAX_LOD);
    packed_footprint(size, lod).into_iter().all(|axis| axis <= edge).then_some(lod)
}

pub(crate) fn opaque(rgba: &[u8]) -> bool {
    rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255)
}

fn linear_srgb(channel: u8) -> f32 {
    static LINEAR: LazyLock<[f32; 256]> =
        LazyLock::new(|| std::array::from_fn(|i| AlphaColor::<Srgb>::from_rgb8(i as u8, 0, 0).convert::<LinearSrgb>().components[0]));
    LINEAR[channel as usize]
}

fn decode(pixel: [u8; 4]) -> [f32; 4] {
    let [r, g, b, a] = pixel;
    [linear_srgb(r), linear_srgb(g), linear_srgb(b), f32::from(a) / 255.0]
}

fn encode(pixel: [f32; 4]) -> [u8; 4] {
    AlphaColor::<LinearSrgb>::new(pixel).to_rgba8().to_u8_array()
}

/// An sRGB encoding of associated linear RGB and linear alpha, as stored by the GPU.
pub(crate) struct ImageLevel {
    pub(crate) size: [u32; 2],
    pub(crate) pixels: Arc<[u8]>,
}

impl ImageLevel {
    pub(crate) fn base(size: [u32; 2], rgba: &Arc<[u8]>, opaque: bool) -> Self {
        let pixels = if opaque {
            rgba.clone()
        } else {
            rgba.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&pixel| {
                    if pixel[3] == 255 {
                        return pixel;
                    }
                    let [r, g, b, a] = decode(pixel);
                    encode([r * a, g * a, b * a, a])
                })
                .collect::<Vec<_>>()
                .into()
        };
        Self { size, pixels }
    }

    pub(crate) fn downsample(&self, packed: bool) -> Self {
        // Packed levels use the power-of-two grid of the containing texture. At an odd
        // boundary the extra half texel repeats the edge. Dedicated textures use normalized
        // coordinates and their hardware-defined floor-rounded mip extents.
        let size = self.size.map(|axis| if packed { axis.div_ceil(2) } else { (axis / 2).max(1) });
        let halves = packed || self.size.into_iter().all(|axis| axis == 1 || axis.is_multiple_of(2));
        let scale = std::array::from_fn(|axis| self.size[axis] as f32 / size[axis] as f32);
        let mut pixels = Vec::with_capacity(size[0] as usize * size[1] as usize * 4);
        for y in 0..size[1] {
            for x in 0..size[0] {
                let pixel = if halves { self.half_pixel(x, y) } else { self.area_pixel(x, y, scale) };
                pixels.extend_from_slice(&pixel);
            }
        }
        Self { size, pixels: pixels.into() }
    }

    fn half_pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let (x, y) = (x * 2, y * 2);
        let texels = [self.pixel(x, y), self.pixel(x + 1, y), self.pixel(x, y + 1), self.pixel(x + 1, y + 1)];
        if texels[1..].iter().all(|&pixel| pixel == texels[0]) {
            return texels[0];
        }
        let [a, b, c, d] = texels.map(decode);
        encode(std::array::from_fn(|i| (a[i] + b[i] + c[i] + d[i]) * 0.25))
    }

    fn area_pixel(&self, x: u32, y: u32, [sx, sy]: [f32; 2]) -> [u8; 4] {
        let (l, r) = (x as f32 * sx, (x + 1) as f32 * sx);
        let (t, b) = (y as f32 * sy, (y + 1) as f32 * sy);
        let mut total = [0.0; 4];
        for row in t.floor() as u32..b.ceil() as u32 {
            let height = b.min(row as f32 + 1.0) - t.max(row as f32);
            for col in l.floor() as u32..r.ceil() as u32 {
                let width = r.min(col as f32 + 1.0) - l.max(col as f32);
                for (sum, channel) in total.iter_mut().zip(decode(self.pixel(col, row))) {
                    *sum += channel * width * height;
                }
            }
        }
        encode(total.map(|channel| channel / (sx * sy)))
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let [w, h] = self.size;
        self.pixels.as_chunks::<4>().0[y.min(h - 1) as usize * w as usize + x.min(w - 1) as usize]
    }

    pub(crate) fn sample(&self, uv: [f32; 2], extent: [f32; 2]) -> [f32; 4] {
        let [x, y] = std::array::from_fn(|axis| (uv[axis] * extent[axis] - 0.5).clamp(0.0, (self.size[axis] - 1) as f32));
        let (x0, y0) = (x as u32, y as u32);
        let (fx, fy) = (x.fract(), y.fract());
        let [a, b, c, d] = [(x0, y0), (x0 + 1, y0), (x0, y0 + 1), (x0 + 1, y0 + 1)].map(|(x, y)| decode(self.pixel(x, y)));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}
