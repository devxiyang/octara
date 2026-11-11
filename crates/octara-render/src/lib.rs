//! Renderers for Octara's display list ([`octara::paint`]).
//!
//! - [`CpuRenderer`]: the reference renderer. Deterministic (bundled fonts, plain float math), it
//!   gives agents and tests screenshots without a GPU and is the oracle the GPU renderer is
//!   compared with.
//! - [`GpuRenderer`] (feature `gpu`): wgpu, one WGSL shader on every platform, one instance per
//!   item and one draw call per frame. It implements the formulas of [`shape`] and uses the same
//!   [`glyphs`] rasterization, and is tested against the reference renderer.

pub mod cpu;
pub mod glyphs;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod shape;

pub use cpu::CpuRenderer;
#[cfg(feature = "gpu")]
pub use gpu::GpuRenderer;
#[cfg(feature = "gpu")]
pub use wgpu;

use std::io;
use std::path::Path;

/// A rendered frame: RGBA8 (straight alpha, sRGB).
#[derive(Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bitmap({}×{})", self.width, self.height)
    }
}

impl Bitmap {
    fn from_premultiplied(width: u32, height: u32, pixels: &[[f32; 4]]) -> Bitmap {
        let mut rgba = Vec::with_capacity(pixels.len() * 4);
        for p in pixels {
            let a = p[3];
            let un = |v: f32| if a > 0.0 { (v / a).clamp(0.0, 1.0) } else { 0.0 };
            for v in [un(p[0]), un(p[1]), un(p[2]), a.clamp(0.0, 1.0)] {
                rgba.push((v * 255.0 + 0.5) as u8);
            }
        }
        Bitmap { width, height, rgba }
    }

    /// The pixel at `(x, y)` as `[r, g, b, a]`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    /// PNG bytes.
    pub fn encode_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(&self.rgba).expect("png data");
        w.finish().expect("png finish");
        out
    }

    pub fn save_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        std::fs::write(path, self.encode_png())
    }
}

/// A screenshot of `app`'s current frame at `scale` device pixels per logical pixel, rendered
/// by the reference renderer.
pub fn screenshot(app: &octara::app::App, scale: f32) -> Bitmap {
    CpuRenderer::new().render(&app.display_list(), scale)
}
