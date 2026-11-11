//! Images: a shared handle to pixels (Rune's `Pixels`), shown on any element with `.img(..)`.
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

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Image {
    /// An image from straight RGBA bytes (`width × height × 4`).
    pub fn rgba(width: u32, height: u32, rgba: impl Into<Arc<[u8]>>) -> Image {
        let rgba = rgba.into();
        assert_eq!(rgba.len(), (width * height * 4) as usize, "RGBA data must be width × height × 4 bytes");
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

    /// Width and height in pixels.
    pub fn size(&self) -> (u32, u32) {
        let f = self.0.frame.lock().expect("image");
        (f.width, f.height)
    }

    /// Replaces the pixels (a video frame, a decoded download): every element showing the image
    /// draws them on its next frame.
    pub fn set(&self, width: u32, height: u32, rgba: impl Into<Arc<[u8]>>) {
        let rgba = rgba.into();
        assert_eq!(rgba.len(), (width * height * 4) as usize, "RGBA data must be width × height × 4 bytes");
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
