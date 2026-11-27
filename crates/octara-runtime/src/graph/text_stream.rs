//! A bounded single-producer queue drained at the start of an application turn.

use octara_model::CacheLine;
use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Queue {
    read: CacheLine<AtomicUsize>,
    written: CacheLine<AtomicUsize>,
    notified: AtomicBool,
    producer: AtomicBool,
    consumer: AtomicBool,
    slots: Box<[UnsafeCell<Option<String>>]>,
}

const _: () = {
    assert!(std::mem::offset_of!(Queue, read).is_multiple_of(128));
    assert!(std::mem::offset_of!(Queue, written).is_multiple_of(128));
};

// SAFETY: Only the exclusive producer writes an unpublished slot. Only the
// exclusive consumer takes a published slot. Release/acquire cursor updates
// transfer ownership before either endpoint can revisit the slot.
unsafe impl Sync for Queue {}

/// A single producer for a scope-owned text buffer. Transfer it to a task or
/// thread to append chunks without taking a lock. A full queue returns the
/// original chunk so the producer can retry without losing data.
pub struct TextStreamSink {
    queue: Arc<Queue>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

pub(crate) struct TextStreamDrain(Arc<Queue>);

#[derive(Debug, PartialEq, Eq)]
pub enum TextStreamError {
    Full(String),
    Closed(String),
}

impl TextStreamSink {
    pub(crate) fn pair(capacity: usize, wake: Option<Arc<dyn Fn() + Send + Sync>>) -> (Self, TextStreamDrain) {
        let capacity = capacity.max(1).checked_next_power_of_two().expect("text stream capacity is too large");
        let queue = Arc::new(Queue {
            read: CacheLine(AtomicUsize::new(0)),
            written: CacheLine(AtomicUsize::new(0)),
            notified: AtomicBool::new(false),
            producer: AtomicBool::new(true),
            consumer: AtomicBool::new(true),
            slots: (0..capacity).map(|_| UnsafeCell::new(None)).collect(),
        });
        (Self { queue: queue.clone(), wake }, TextStreamDrain(queue))
    }

    pub fn try_send(&mut self, text: String) -> Result<(), TextStreamError> {
        let queue = &self.queue;
        if !queue.consumer.load(Ordering::Acquire) {
            return Err(TextStreamError::Closed(text));
        }
        if text.is_empty() {
            return Ok(());
        }
        let written = queue.written.load(Ordering::Relaxed);
        let read = queue.read.load(Ordering::Acquire);
        if written.wrapping_sub(read) == queue.slots.len() {
            return Err(TextStreamError::Full(text));
        }
        let slot = &queue.slots[written & (queue.slots.len() - 1)];
        // SAFETY: This slot is outside the consumer's published interval.
        unsafe { *slot.get() = Some(text) };
        queue.written.store(written.wrapping_add(1), Ordering::Release);
        self.notify();
        Ok(())
    }

    fn notify(&self) {
        if !self.queue.notified.swap(true, Ordering::AcqRel)
            && let Some(wake) = &self.wake
        {
            wake();
        }
    }
}

impl Drop for TextStreamSink {
    fn drop(&mut self) {
        self.queue.producer.store(false, Ordering::Release);
        if self.queue.consumer.load(Ordering::Acquire) {
            self.notify();
        }
    }
}

impl TextStreamDrain {
    pub(crate) fn pending(&self) -> bool {
        self.0.notified.load(Ordering::Acquire)
    }

    pub(crate) fn take(&mut self) -> (Option<String>, bool) {
        let queue = &self.0;
        queue.notified.swap(false, Ordering::AcqRel);
        let producer = queue.producer.load(Ordering::Acquire);
        let mut read = queue.read.load(Ordering::Relaxed);
        let written = queue.written.load(Ordering::Acquire);
        let mut combined: Option<String> = None;
        while read != written {
            let slot = &queue.slots[read & (queue.slots.len() - 1)];
            // SAFETY: The producer published the slot and cannot reuse it until
            // the read cursor advances.
            let text = unsafe { (*slot.get()).take() }.expect("published stream chunk");
            if let Some(combined) = combined.as_mut() {
                combined.push_str(&text);
            } else {
                combined = Some(text);
            }
            read = read.wrapping_add(1);
        }
        queue.read.store(read, Ordering::Release);
        (combined, producer)
    }
}

impl Drop for TextStreamDrain {
    fn drop(&mut self) {
        self.0.consumer.store(false, Ordering::Release);
    }
}
