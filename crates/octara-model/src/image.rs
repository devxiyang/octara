//! RGBA8 image sources and their immutable samples.

use crate::CacheLine;
use crate::geometry::{Rect, Size};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Which caches may retain this source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CachePolicy {
    #[default]
    Standard,
    MemoryOnly,
    NoCache,
}

/// Authored media. Encoded bytes and URIs are decoded by the host on demand.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ImageSource {
    Pixels(Image),
    Uri(Arc<str>),
    Encoded { bytes: Arc<[u8]>, mime: Option<Arc<str>> },
    Resolved { uri: Arc<str>, headers: Arc<[(Arc<str>, Arc<str>)]>, cache: CachePolicy },
}

impl ImageSource {
    pub fn cache_policy(&self) -> CachePolicy {
        match self {
            Self::Resolved { headers, cache: CachePolicy::Standard, .. } if !headers.is_empty() => CachePolicy::MemoryOnly,
            Self::Resolved { cache, .. } => *cache,
            _ => CachePolicy::Standard,
        }
    }

    /// Drop the disk-cache permission at the first boundary that sees headers.
    pub fn normalize_cache_policy(mut self) -> Self {
        let policy = self.cache_policy();
        if let Self::Resolved { cache, .. } = &mut self {
            *cache = policy;
        }
        self
    }
}

impl std::fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pixels(image) => f.debug_tuple("Pixels").field(image).finish(),
            Self::Uri(_) => f.write_str("Uri(..)"),
            Self::Encoded { bytes, .. } => f.debug_struct("Encoded").field("len", &bytes.len()).finish_non_exhaustive(),
            Self::Resolved { .. } => f.debug_struct("Resolved").field("cache", &self.cache_policy()).finish_non_exhaustive(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ImageState {
    #[default]
    Idle,
    Loading {
        probed_size: Option<(u32, u32)>,
    },
    Ready {
        natural_size: (u32, u32),
    },
    Failed(Arc<str>),
}

impl ImageState {
    pub fn natural_size(&self) -> Option<(u32, u32)> {
        match *self {
            Self::Loading { probed_size } => probed_size,
            Self::Ready { natural_size } => Some(natural_size),
            _ => None,
        }
    }
}

/// A decoded image, or a URL already loaded by a browser's native image decoder.
#[derive(Clone, PartialEq, Eq)]
pub enum MediaAsset {
    Pixels(Image),
    BrowserUri(Arc<str>),
}

impl std::fmt::Debug for MediaAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pixels(image) => f.debug_tuple("Pixels").field(image).finish(),
            Self::BrowserUri(_) => f.write_str("BrowserUri(..)"),
        }
    }
}

/// An authored video and its optional fallback artwork.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VideoSource {
    pub uri: String,
    pub poster: Option<String>,
    #[serde(default = "video_controls")]
    pub controls: bool,
}

fn video_controls() -> bool {
    true
}

impl VideoSource {
    pub fn new(uri: impl Into<String>) -> Self {
        Self { uri: uri.into(), poster: None, controls: true }
    }
}

/// Cached HTTP response data. Freshness times are Unix seconds.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct HttpMediaEntry {
    pub bytes: Arc<[u8]>,
    pub mime: Option<Arc<str>>,
    pub etag: Option<Arc<str>>,
    pub fresh_until: u64,
    pub stale_until: u64,
    pub max_age: Option<u64>,
    pub stale_while_revalidate: u64,
}

/// A video provider's native-slot kind and authored properties.
pub struct VideoSurface {
    pub kind: &'static str,
    pub props: serde_json::Value,
}

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
    /// Places an image of the given pixel dimensions inside the element's bounds.
    pub fn place(self, image: Size, bounds: Rect) -> Rect {
        let Size { width, height } = image;
        if width <= 0.0 || height <= 0.0 || bounds.is_empty() {
            return Rect::new(bounds.x, bounds.y, 0.0, 0.0);
        }
        if self == ObjectFit::Fill {
            return bounds;
        }
        let sx = bounds.w / width;
        let sy = bounds.h / height;
        let scale = match self {
            ObjectFit::Fill | ObjectFit::None => 1.0,
            ObjectFit::Contain => sx.min(sy),
            ObjectFit::Cover => sx.max(sy),
            ObjectFit::ScaleDown => sx.min(sy).min(1.0),
        };
        let w = width * scale;
        let h = height * scale;
        Rect::new(bounds.x + (bounds.w - w) / 2.0, bounds.y + (bounds.h - h) / 2.0, w, h)
    }
}

/// Straight-alpha sRGB RGBA8 pixels, ordered from top row to bottom row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImageFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    /// Bumped by every [`Image::set`], so renderers know to upload again.
    pub version: u64,
}

struct Inner {
    id: u64,
    frame: Mutex<ImageFrame>,
}

/// A shared handle to an image's pixels. [`Image::set`] publishes the new pixels to all clones.
#[derive(Clone)]
pub struct Image(Arc<Inner>);

static PIXEL_GENERATION: CacheLine<AtomicU64> = CacheLine(AtomicU64::new(0));

/// The pixels and source identity recorded together for a prepared frame.
#[derive(Clone)]
pub struct Sample {
    source: Image,
    frame: ImageFrame,
}

impl Sample {
    /// The shared source for identity and freshness checks.
    pub fn source(&self) -> &Image {
        &self.source
    }

    /// The sampled pixels, even after the source changes.
    pub fn frame(&self) -> &ImageFrame {
        &self.frame
    }
}

impl PartialEq for Sample {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source && self.frame.version == other.frame.version
    }
}

impl std::fmt::Debug for Sample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ImageFrame { width, height, version, .. } = self.frame;
        write!(f, "Image#{}@{version}({width}×{height})", self.source.id())
    }
}

fn next_id() -> u64 {
    static NEXT_ID: CacheLine<AtomicU64> = CacheLine(AtomicU64::new(1));
    NEXT_ID.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("image identities exhausted")
}

/// The byte length of a tightly packed RGBA8 frame, if it fits the address space.
pub(super) fn rgba_len(width: u32, height: u32) -> Option<usize> {
    (width as usize).checked_mul(height as usize)?.checked_mul(4)
}

fn assert_rgba_len(width: u32, height: u32, len: usize) {
    let expected = rgba_len(width, height).expect("RGBA dimensions exceed the address space");
    assert_eq!(len, expected, "RGBA data must be width × height × 4 bytes");
}

impl Image {
    /// A process-wide change hint for consumers tracking mounted image handles.
    #[doc(hidden)]
    pub fn pixel_generation() -> u64 {
        PIXEL_GENERATION.load(Ordering::Acquire)
    }

    /// An image from straight RGBA bytes (`width × height × 4`).
    /// Either dimension may be zero for an empty image.
    ///
    /// # Panics
    /// If the byte length does not match the dimensions or the required length overflows.
    pub fn rgba(width: u32, height: u32, rgba: impl Into<Arc<[u8]>>) -> Image {
        let rgba = rgba.into();
        assert_rgba_len(width, height, rgba.len());
        Image(Arc::new(Inner { id: next_id(), frame: Mutex::new(ImageFrame { width, height, rgba, version: 0 }) }))
    }

    /// Identifies the image (and its clones).
    pub fn id(&self) -> u64 {
        self.0.id
    }

    /// The current pixels.
    pub fn frame(&self) -> ImageFrame {
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

    /// Replaces the pixels sampled the next time a window prepares a frame. Either dimension may be zero
    /// for an empty image.
    ///
    /// # Panics
    /// If the byte length does not match the dimensions or the required length overflows.
    /// Invalid replacement data leaves the current frame and version unchanged.
    pub fn set(&self, width: u32, height: u32, rgba: impl Into<Arc<[u8]>>) {
        let rgba = rgba.into();
        assert_rgba_len(width, height, rgba.len());
        let mut f = self.0.frame.lock().expect("image");
        let version = f.version.checked_add(1);
        let Some(version) = version else {
            drop(f);
            panic!("image versions exhausted");
        };
        *f = ImageFrame { width, height, rgba, version };
        PIXEL_GENERATION.fetch_add(1, Ordering::Release);
    }
}

impl PartialEq for Image {
    fn eq(&self, other: &Image) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Image {}

impl Hash for Image {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state);
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
