//! Rendering protocol and pixels, independent of the App capture workflow.

use crate::paint::DisplayList;

/// A painted frame: RGBA8, straight alpha, sRGB.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}×{})", self.width, self.height)
    }
}

impl Picture {
    /// Writes it as an 8-bit sRGB PNG.
    pub fn save_png(&self, path: &str) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut w = enc.write_header().map_err(|e| format!("{path}: {e}"))?;
        w.write_image_data(&self.rgba).map_err(|e| format!("{path}: {e}"))?;
        w.finish().map_err(|e| format!("{path}: {e}"))
    }
}

/// What renders a capture's frame.
pub trait Painter {
    /// `gpu` or `reference`, for the report.
    fn renderer(&self) -> &'static str;
    /// The display list at `scale` device pixels per logical pixel.
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String>;
}
