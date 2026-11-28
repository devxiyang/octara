use octara::MediaProgress;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Fetches an HTTP image on a native media worker.
pub trait MediaTransport: Send + Sync + 'static {
    /// Implementations must honor the byte limit, redirect policy and cancellation.
    fn fetch(&self, request: MediaFetchRequest, cancel: &AtomicBool) -> Result<MediaFetchResponse, Arc<str>>;
}

/// An HTTP GET and the transfer limits imposed by the native loader.
pub struct MediaFetchRequest {
    pub uri: Arc<str>,
    pub headers: Vec<(Arc<str>, Arc<str>)>,
    pub max_bytes: usize,
    pub follow_redirects: bool,
    pub progress: MediaProgress,
    pub(crate) cancellation: MediaCancellation,
}

impl MediaFetchRequest {
    pub fn new(uri: impl Into<Arc<str>>, max_bytes: usize) -> Self {
        Self {
            uri: uri.into(),
            headers: Vec::new(),
            max_bytes,
            follow_redirects: true,
            progress: MediaProgress::default(),
            cancellation: MediaCancellation::default(),
        }
    }

    /// Registers an interrupt for this request. Retain the registration until the transfer exits.
    pub fn on_cancel(&self, handler: impl FnOnce() + Send + 'static) -> MediaCancelRegistration {
        self.cancellation.register(handler)
    }
}

/// The response after any allowed redirects. Return an empty body for HTTP 304.
pub struct MediaFetchResponse {
    pub status: u16,
    pub headers: Vec<(Arc<str>, Arc<str>)>,
    pub bytes: Arc<[u8]>,
}

type CancelHandler = Mutex<Option<Box<dyn FnOnce() + Send>>>;

pub struct MediaCancelRegistration {
    _handler: Arc<CancelHandler>,
}

#[derive(Clone, Default)]
pub(crate) struct MediaCancellation(Arc<Cancellation>);

#[derive(Default)]
struct Cancellation {
    cancelled: AtomicBool,
    handlers: Mutex<Vec<Weak<CancelHandler>>>,
}

impl MediaCancellation {
    pub(crate) fn flag(&self) -> &AtomicBool {
        &self.0.cancelled
    }

    pub(crate) fn cancel(&self) {
        if self.flag().swap(true, Ordering::AcqRel) {
            return;
        }
        let handlers = std::mem::take(&mut *self.0.handlers.lock().expect("media cancellation"));
        for handler in handlers.into_iter().filter_map(|weak| weak.upgrade()) {
            let interrupt = handler.lock().expect("media interrupt").take();
            if let Some(interrupt) = interrupt {
                interrupt();
            }
        }
    }

    fn register(&self, handler: impl FnOnce() + Send + 'static) -> MediaCancelRegistration {
        let mut handlers = self.0.handlers.lock().expect("media cancellation");
        if self.flag().load(Ordering::Acquire) {
            drop(handlers);
            handler();
            return MediaCancelRegistration { _handler: Arc::default() };
        }
        let handler: Arc<CancelHandler> = Arc::new(Mutex::new(Some(Box::new(handler))));
        handlers.retain(|entry| entry.strong_count() != 0);
        handlers.push(Arc::downgrade(&handler));
        MediaCancelRegistration { _handler: handler }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn cancellation_interrupts_live_and_late_registrations_once() {
        let request = MediaFetchRequest::new("https://example.test/image.png", 256);
        let counter = Arc::new(AtomicUsize::new(0));
        let interrupt = |count: Arc<AtomicUsize>| {
            move || {
                count.fetch_add(1, Ordering::Relaxed);
            }
        };
        let _guard = request.on_cancel(interrupt(counter.clone()));
        drop(request.on_cancel(interrupt(counter.clone())));
        request.cancellation.cancel();
        request.cancellation.cancel();
        assert!(request.cancellation.flag().load(Ordering::Acquire));
        assert_eq!(counter.load(Ordering::Relaxed), 1);
        let _late = request.on_cancel(interrupt(counter.clone()));
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }
}
