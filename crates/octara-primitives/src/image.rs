//! Images: a shared handle to pixels (Rune's `Pixels`), shown by the UI’s `image(..)` constructor.
//! Setting new pixels on a handle redraws whatever shows it on the next frame, without
//! rerunning the view.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// How an image fills its element (GPUI's `ObjectFit`, CSS `object-fit`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ObjectFit {
    /// Stretched to the element (the default, as Rune's `pixels`).
    #[default]
    Fill,
    /// Whole, at its aspect ratio, centered (letterboxed).
    Contain,
    /// Covering the element at its aspect ratio, centered and cropped.
    Cover,
    /// `Contain`, but never larger than the image's own size.
    ScaleDown,
    /// At its own size, centered (cropped if larger).
    None,
}

impl ObjectFit {
    /// Where an image of `(iw, ih)` pixels goes in the box `(x, y, w, h)`.
    pub fn place(self, (iw, ih): (f32, f32), (x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        if iw <= 0.0 || ih <= 0.0 {
            return (x, y, 0.0, 0.0);
        }
        let center = |dw: f32, dh: f32| (x + (w - dw) / 2.0, y + (h - dh) / 2.0, dw, dh);
        let contain = (w / iw).min(h / ih);
        match self {
            ObjectFit::Fill => (x, y, w, h),
            ObjectFit::Contain => center(iw * contain, ih * contain),
            ObjectFit::Cover => {
                let k = (w / iw).max(h / ih);
                center(iw * k, ih * k)
            }
            ObjectFit::ScaleDown => {
                let k = contain.min(1.0);
                center(iw * k, ih * k)
            }
            ObjectFit::None => center(iw, ih),
        }
    }
}

/// Pixels: straight (not premultiplied) sRGB RGBA, 8 bits per channel, rows top to bottom.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    /// Bumped by every [`Image::set`], so renderers know to upload again.
    pub version: u64,
}

struct Inner {
    id: u64,
    frame: Mutex<Frame>,
}

/// A shared handle to an image's pixels. Clones show the same pixels; [`Image::set`] replaces
/// them for every element showing the image.
#[derive(Clone)]
pub struct Image(Arc<Inner>);

/// The pixels and source identity recorded together for a prepared frame.
#[derive(Clone)]
pub struct Sample {
    source: Image,
    frame: Frame,
}

impl Sample {
    /// The shared source for identity and freshness checks.
    pub fn source(&self) -> &Image {
        &self.source
    }

    /// The sampled pixels, even after the source changes.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }
}

impl std::fmt::Debug for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Frame { width, height, version, .. } = self.frame;
        write!(f, "Image#{}@{version}({width}×{height})", self.source.id())
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The byte length of a tightly packed RGBA8 frame, if it fits the address space.
pub(crate) fn rgba_len(width: u32, height: u32) -> Option<usize> {
    (width as usize).checked_mul(height as usize)?.checked_mul(4)
}

fn assert_rgba_len(width: u32, height: u32, len: usize) {
    let expected = rgba_len(width, height).expect("RGBA dimensions exceed the address space");
    assert_eq!(len, expected, "RGBA data must be width × height × 4 bytes");
}

impl Image {
    /// An image from straight RGBA bytes (`width × height × 4`).
    /// Either dimension may be zero for an empty image.
    ///
    /// # Panics
    /// If the byte length does not match the dimensions or the required length overflows.
    pub fn rgba(width: u32, height: u32, rgba: impl Into<Arc<[u8]>>) -> Image {
        let rgba = rgba.into();
        assert_rgba_len(width, height, rgba.len());
        Image(Arc::new(Inner { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), frame: Mutex::new(Frame { width, height, rgba, version: 0 }) }))
    }

    /// Decodes a PNG (any bit depth and color type; 16-bit channels are reduced to 8).
    pub fn decode(bytes: &[u8]) -> Result<Image, String> {
        let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
        d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
        let mut reader = d.read_info().map_err(|e| format!("PNG: {e}"))?;
        let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG: image too large")?];
        let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG: {e}"))?;
        let (w, h) = (info.width, info.height);
        let px = &buf[..info.buffer_size()];
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::GrayscaleAlpha => px.as_chunks::<2>().0.iter().flat_map(|[g, a]| [*g, *g, *g, *a]).collect(),
            png::ColorType::Rgb => px.as_chunks::<3>().0.iter().flat_map(|[r, g, b]| [*r, *g, *b, 255]).collect(),
            png::ColorType::Grayscale => px.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
            png::ColorType::Indexed => return Err("PNG: palette not expanded".into()),
        };
        Ok(Image::rgba(w, h, rgba))
    }

    /// Identifies the image (and its clones).
    pub fn id(&self) -> u64 {
        self.0.id
    }

    /// The current pixels.
    pub fn frame(&self) -> Frame {
        self.0.frame.lock().expect("image").clone()
    }

    /// Pins the current pixels together with their source identity.
    pub fn sample(&self) -> Sample {
        Sample { source: self.clone(), frame: self.frame() }
    }

    /// Width and height in pixels.
    pub fn size(&self) -> (u32, u32) {
        let f = self.0.frame.lock().expect("image");
        (f.width, f.height)
    }

    /// Replaces the pixels (a video frame, a decoded download): every element showing the image
    /// samples them when its window next prepares a frame. Either dimension may be zero
    /// for an empty image.
    ///
    /// # Panics
    /// If the byte length does not match the dimensions or the required length overflows.
    /// Invalid replacement data leaves the current frame and version unchanged.
    pub fn set(&self, width: u32, height: u32, rgba: impl Into<Arc<[u8]>>) {
        let rgba = rgba.into();
        assert_rgba_len(width, height, rgba.len());
        let mut f = self.0.frame.lock().expect("image");
        let version = f.version + 1;
        *f = Frame { width, height, rgba, version };
    }
}

impl PartialEq for Image {
    fn eq(&self, other: &Image) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, h) = self.size();
        write!(f, "Image#{}({w}×{h})", self.0.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::catch_unwind;

    #[test]
    fn shared_image_replaces_a_complete_frame() {
        let image = Image::rgba(1, 1, [1, 2, 3, 4]);
        let clone = image.clone();
        let first = image.frame();
        assert_eq!((first.width, first.height, first.version), (1, 1, 0));
        image.set(2, 1, [5, 6, 7, 8, 9, 10, 11, 12]);
        let frame = clone.frame();
        assert_eq!((frame.width, frame.height, frame.version), (2, 1, 1));
        assert_eq!(frame.rgba.as_ref(), &[5, 6, 7, 8, 9, 10, 11, 12]);
        assert_eq!(first.rgba.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn empty_images_allow_either_zero_dimension() {
        for (width, height) in [(0, 0), (0, u32::MAX), (u32::MAX, 0)] {
            let image = Image::rgba(width, height, []);
            assert_eq!(image.size(), (width, height));
            assert!(image.frame().rgba.is_empty());
            image.set(1, 1, [0; 4]);
            image.set(width, height, []);
            assert_eq!(image.size(), (width, height));
            assert_eq!(image.frame().version, 2);
        }
    }

    #[test]
    fn invalid_rgba_is_rejected_before_replacing_the_frame() {
        let image = Image::rgba(1, 1, [1, 2, 3, 4]);
        let first = image.frame();
        for (width, height, rgba) in
            [(1, 1, vec![0; 3]), (1, 1, vec![0; 5]), (0, 1, vec![0; 4]), (65536, 65536, vec![]), (u32::MAX, u32::MAX, vec![])]
        {
            assert!(catch_unwind(|| Image::rgba(width, height, rgba.clone())).is_err());
            assert!(catch_unwind(|| image.set(width, height, rgba)).is_err());
            assert_eq!(image.frame(), first);
        }
        image.set(1, 1, [5, 6, 7, 8]);
        assert_eq!(image.frame().version, 1);
    }
}
