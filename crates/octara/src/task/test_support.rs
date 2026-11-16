use super::*;
use std::sync::atomic::AtomicUsize;

pub(super) struct WakeCounter(pub(super) AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) struct DroppedResult(pub(super) Arc<AtomicUsize>);

impl Drop for DroppedResult {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
