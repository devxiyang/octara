//! Rendering protocol and pixels, independent of the App capture workflow.

use crate::identity::NodeId;
use crate::image::rgba_len;
use crate::paint::DisplayList;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

/// A painted frame: RGBA8, straight alpha, sRGB, rows top to bottom.
///
/// `rgba` must contain `width × height × 4` bytes. Its public fields may be assembled directly;
/// pixel access and PNG encoding validate this layout. PNG output requires nonzero dimensions.
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
    /// The pixel at `(x, y)` as `[r, g, b, a]`.
    ///
    /// # Panics
    /// If the coordinates are out of bounds or the RGBA data does not match the dimensions.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width && y < self.height, "pixel coordinates are outside the picture");
        assert_eq!(rgba_len(self.width, self.height), Some(self.rgba.len()), "RGBA data must be width × height × 4 bytes");
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    /// Encodes an 8-bit sRGB PNG. Invalid dimensions or RGBA data return an error.
    pub fn encode_png(&self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.write_png(&mut bytes)?;
        Ok(bytes)
    }

    /// Writes an 8-bit sRGB PNG. Encoding and filesystem failures return an error.
    pub fn save_png(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let file = std::fs::File::create(path)?;
        self.write_png(io::BufWriter::new(file))
    }

    fn write_png(&self, output: impl Write) -> io::Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "PNG dimensions must be nonzero"));
        }
        if rgba_len(self.width, self.height) != Some(self.rgba.len()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "RGBA data must be width × height × 4 bytes"));
        }
        let mut enc = png::Encoder::new(output, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut w = enc.write_header().map_err(png_error)?;
        w.write_image_data(&self.rgba).map_err(png_error)?;
        w.finish().map_err(png_error)
    }
}

fn png_error(error: png::EncodingError) -> io::Error {
    match error {
        png::EncodingError::IoError(error) => error,
        error => io::Error::other(error),
    }
}

/// The immutable drawing revision and its sample time on the application clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaintSample {
    pub revision: u64,
    pub sampled_at: Duration,
}

/// The pixels actually used for an author paint site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PaintOutcome {
    Rendered,
    Retained {
        /// The drawing revision that produced this output, when supplied by the caller.
        #[serde(default, rename = "source_revision", skip_serializing_if = "Option::is_none")]
        revision: Option<u64>,
    },
    Fallback {
        reason: PaintFallback,
    },
}

/// Why the styled box supplied the pixels for a custom paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaintFallback {
    ReferenceRenderer,
    UnsupportedProgram,
    InvalidSize,
    PreparationFailed,
    EncodingFailed,
    BackdropUnavailable,
    HistoryUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintReceipt {
    pub source: NodeId,
    pub outcome: PaintOutcome,
}

/// A picture with the author paint outcomes observed while composing it.
pub struct PaintedPicture {
    pub picture: Picture,
    pub effects: Vec<PaintReceipt>,
}

/// What renders a capture's frame.
pub trait Painter {
    /// `gpu` or `reference`, for the report.
    fn renderer(&self) -> &'static str;
    /// The display list at `scale` device pixels per logical pixel.
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String>;

    /// The picture and, when supported, its author paint receipts. A host can supply the
    /// settled drawing sample used for the capture.
    fn paint_with_receipts(&mut self, list: &DisplayList, scale: f32, _sample: Option<PaintSample>) -> Result<PaintedPicture, String> {
        let picture = self.paint(list, scale)?;
        Ok(PaintedPicture { picture, effects: Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_png(picture: &Picture, bytes: &[u8]) {
        let mut reader = png::Decoder::new(io::Cursor::new(bytes)).read_info().unwrap();
        assert_eq!(reader.info().srgb, Some(png::SrgbRenderingIntent::Perceptual));
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (picture.width, picture.height));
        assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Rgba, png::BitDepth::Eight));
        assert_eq!(&decoded[..info.buffer_size()], picture.rgba);
    }

    #[test]
    fn memory_and_file_pngs_preserve_straight_alpha_pixels_and_srgb() {
        let picture = Picture { width: 2, height: 2, rgba: vec![25, 150, 250, 0, 250, 128, 64, 64, 45, 90, 180, 128, 12, 34, 56, 255] };
        assert_eq!(picture.pixel(1, 1), [12, 34, 56, 255]);
        let bytes = picture.encode_png().unwrap();
        assert_png(&picture, &bytes);

        let path = std::env::temp_dir().join(format!("octara-picture-{}.png", std::process::id()));
        picture.save_png(&path).unwrap();
        let saved = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_png(&picture, &saved);
        assert_eq!(saved, bytes);
    }

    #[test]
    fn invalid_pictures_return_encoding_errors() {
        for picture in [
            Picture { width: 0, height: 1, rgba: vec![] },
            Picture { width: 1, height: 0, rgba: vec![] },
            Picture { width: 1, height: 1, rgba: vec![0; 3] },
            Picture { width: 1, height: 1, rgba: vec![0; 5] },
            Picture { width: 65536, height: 65536, rgba: vec![] },
            Picture { width: u32::MAX, height: u32::MAX, rgba: vec![] },
        ] {
            assert_eq!(picture.encode_png().unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn pixel_access_rejects_invalid_coordinates_and_layouts() {
        let picture = Picture { width: 1, height: 2, rgba: vec![1, 2, 3, 4, 5, 6, 7, 8] };
        assert_eq!(picture.pixel(0, 1), [5, 6, 7, 8]);
        // x == width used to alias the next row; large coordinates could wrap to a valid pixel.
        for (x, y) in [(1, 0), (0, 2), (1 << 30, 0), (0, 1 << 30)] {
            assert!(std::panic::catch_unwind(|| picture.pixel(x, y)).is_err());
        }
        for picture in [
            Picture { width: 0, height: 0, rgba: vec![] },
            Picture { width: 1, height: 1, rgba: vec![0; 3] },
            Picture { width: 1, height: 1, rgba: vec![0; 5] },
            Picture { width: 65536, height: 65536, rgba: vec![0; 4] },
            Picture { width: u32::MAX, height: u32::MAX, rgba: vec![0; 4] },
        ] {
            assert!(std::panic::catch_unwind(|| picture.pixel(0, 0)).is_err());
        }
    }

    #[test]
    fn png_finalization_preserves_io_error_kinds() {
        struct FailsOnFlush;
        impl Write for FailsOnFlush {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "cannot flush PNG"))
            }
        }

        let picture = Picture { width: 1, height: 1, rgba: vec![0; 4] };
        let error = picture.write_png(FailsOnFlush).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "cannot flush PNG");
    }
}
