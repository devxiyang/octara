//! Viewport-prioritized media loading and application media extensions.

mod probe;
mod schedule;

pub use octara_model::image::{HttpMediaEntry, VideoSurface};
pub use probe::probe_image_size;
pub(crate) use schedule::MediaScheduler;

use octara_model::image::{CachePolicy, Image, ImageSource, MediaAsset, ObjectFit, VideoSource};
use std::future::{Future, ready};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;

/// Resolves a logical URI at its authoring scope. Returning `None` uses the original URI.
pub trait MediaResolver: Send + Sync + 'static {
    fn resolve(&self, uri: &str) -> Option<ImageSource>;
}

/// Optional persistent tier for public HTTP image responses.
pub trait MediaCacheBackend: Send + Sync + 'static {
    fn get(&self, uri: &str) -> Option<HttpMediaEntry>;
    fn put(&self, uri: &str, entry: HttpMediaEntry);
    fn remove(&self, uri: &str);
}

/// Host extension for placing a video into a native slot.
pub trait VideoSurfaceProvider: Send + Sync + 'static {
    fn surface(&self, source: &VideoSource) -> Option<VideoSurface>;
}

/// A load result carries the original dimensions independently of its pixel buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedMedia {
    pub asset: MediaAsset,
    pub natural_size: (u32, u32),
    /// Whether the result may be reused from Octara's RAM caches.
    pub cacheable: bool,
    /// The host can bound decoded-cache reuse to the response freshness lifetime.
    pub cache_for: Option<Duration>,
}

impl LoadedMedia {
    pub fn pixels(image: Image) -> Self {
        let size = image.size();
        Self { asset: MediaAsset::Pixels(image), natural_size: size, cacheable: true, cache_for: None }
    }
}

/// The cancellation signal belongs to one in-flight shared request.
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// A loader can report the first usable dimensions from any worker.
#[derive(Clone, Default)]
pub struct MediaProgress(Arc<Mutex<Progress>>);

#[derive(Default)]
struct Progress {
    size: Option<(u32, u32)>,
    waker: Option<Waker>,
}

impl MediaProgress {
    pub fn probed(&self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let wake = {
            let mut progress = self.0.lock().expect("media progress");
            if progress.size.is_some() {
                return;
            }
            progress.size = Some(size);
            progress.waker.take()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }

    fn read(&self, waker: &Waker) -> Option<(u32, u32)> {
        let mut progress = self.0.lock().expect("media progress");
        if progress.size.is_none() && progress.waker.as_ref().is_none_or(|old| !old.will_wake(waker)) {
            progress.waker = Some(waker.clone());
        }
        progress.size
    }
}

#[derive(Clone)]
pub struct MediaRequest {
    pub source: ImageSource,
    pub cancel: CancellationToken,
    /// Supplied only for cache-eligible HTTP sources. Defaults to no persistent cache.
    pub disk_cache: Option<Arc<dyn MediaCacheBackend>>,
}

impl MediaRequest {
    pub fn cache_policy(&self) -> CachePolicy {
        self.source.cache_policy()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MediaVariant {
    pub width: u32,
    pub height: u32,
    pub fit: ObjectFit,
}

pub type MediaFuture = Pin<Box<dyn Future<Output = Result<LoadedMedia, Arc<str>>>>>;

/// Host transport and decoder. Dropping the future cancels its I/O.
pub trait MediaLoader: 'static {
    fn load(&self, request: MediaRequest, progress: MediaProgress) -> MediaFuture;

    fn variant(&self, original: LoadedMedia, _variant: MediaVariant, _cancel: CancellationToken) -> MediaFuture {
        Box::pin(ready(Ok(original)))
    }

    fn has_variants(&self) -> bool {
        false
    }
}

#[derive(Default)]
struct PixelsOnly;

impl MediaLoader for PixelsOnly {
    fn load(&self, request: MediaRequest, _: MediaProgress) -> MediaFuture {
        Box::pin(ready(match request.source {
            ImageSource::Pixels(image) => Ok(LoadedMedia::pixels(image)),
            _ => Err(Arc::from("This host has no encoded image loader")),
        }))
    }
}
