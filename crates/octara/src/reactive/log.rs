//! Bounded, disposable edit history. Authoritative state lives in the collection/text owner;
//! a reader that misses history must synchronize from that state, never replay a partial tail.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

pub(crate) type Cursor = Rc<Cell<u64>>;
pub(crate) const MAX_ENTRIES: usize = 4096;
pub(crate) const MAX_BYTES: usize = 1024 * 1024;

pub(crate) enum Changes<E> {
    Delta(Vec<E>),
    Reset,
}

pub(crate) struct Log<E> {
    first: u64,
    entries: VecDeque<(E, usize)>,
    bytes: usize,
    cursors: Vec<Weak<Cell<u64>>>,
}

impl<E: Clone> Log<E> {
    pub(crate) fn new() -> Self {
        Self { first: 0, entries: VecDeque::new(), bytes: 0, cursors: Vec::new() }
    }

    fn end(&self) -> u64 {
        self.first + self.entries.len() as u64
    }

    pub(crate) fn subscribe(&mut self) -> Cursor {
        // Retiring readers must not leave accumulating weak handles even without edits.
        self.trim();
        let cursor = Rc::new(Cell::new(self.end()));
        self.cursors.push(Rc::downgrade(&cursor));
        cursor
    }

    pub(crate) fn push(&mut self, event: E) {
        self.push_sized(event, 0);
    }

    /// `owned_bytes` counts variable payload storage where its size is known (text strings).
    /// Generic collection keys have no universal deep-size operation; their count is bounded.
    pub(crate) fn push_sized(&mut self, event: E, owned_bytes: usize) {
        let bytes = size_of::<E>().saturating_add(owned_bytes);
        self.entries.push_back((event, bytes));
        self.bytes = self.bytes.saturating_add(bytes);
        self.trim();
    }

    pub(crate) fn take(&mut self, cursor: &Cursor) -> Changes<E> {
        let out = if cursor.get() < self.first {
            Changes::Reset
        } else {
            let from = (cursor.get() - self.first) as usize;
            Changes::Delta(self.entries.iter().skip(from).map(|(event, _)| event.clone()).collect())
        };
        cursor.set(self.end());
        self.trim();
        out
    }

    fn trim(&mut self) {
        self.cursors.retain(|cursor| cursor.strong_count() > 0);
        let end = self.end();
        let oldest = self.cursors.iter().filter_map(Weak::upgrade).map(|cursor| cursor.get()).min().unwrap_or(end);
        while !self.entries.is_empty() && (self.first < oldest || self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES) {
            let (_, bytes) = self.entries.pop_front().expect("nonempty log");
            self.bytes -= bytes;
            self.first += 1;
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stalled_reader_cannot_pin_history_and_active_readers_keep_deltas() {
        let mut log = Log::new();
        let stalled = log.subscribe();
        let active = log.subscribe();
        for value in 0..MAX_ENTRIES * 3 {
            log.push(value);
            let Changes::Delta(changes) = log.take(&active) else { panic!("active reader fell behind") };
            assert_eq!(changes, vec![value]);
            assert!(log.len() <= MAX_ENTRIES);
        }
        assert!(matches!(log.take(&stalled), Changes::Reset));
        assert_eq!(log.len(), 0);
        log.push(42);
        assert!(matches!(log.take(&stalled), Changes::Delta(changes) if changes == [42]));
    }

    #[test]
    fn byte_limit_and_single_oversized_payload_require_reset_without_a_partial_tail() {
        let mut log = Log::new();
        let reader = log.subscribe();
        for _ in 0..4 {
            log.push_sized("payload", MAX_BYTES / 3);
        }
        assert!(log.bytes <= MAX_BYTES);
        assert!(matches!(log.take(&reader), Changes::Reset));
        log.push_sized("oversized", MAX_BYTES + 1);
        assert_eq!(log.len(), 0);
        assert_eq!(log.bytes, 0);
        assert!(matches!(log.take(&reader), Changes::Reset));
        log.push("next");
        assert!(matches!(log.take(&reader), Changes::Delta(changes) if changes == ["next"]));
    }

    #[test]
    fn subscribing_at_the_retained_boundary_and_dropping_readers_release_history() {
        let mut log = Log::new();
        let old = log.subscribe();
        for i in 0..MAX_ENTRIES {
            log.push(i);
        }
        assert!(matches!(log.take(&old), Changes::Delta(changes) if changes.len() == MAX_ENTRIES));
        log.push(1);
        drop(old);
        let fresh = log.subscribe();
        assert_eq!(log.len(), 0);
        assert_eq!(log.cursors.len(), 1);
        assert!(matches!(log.take(&fresh), Changes::Delta(changes) if changes.is_empty()));
        drop(fresh);
        for _ in 0..MAX_ENTRIES {
            drop(log.subscribe());
        }
        assert_eq!(log.cursors.len(), 1, "weak readers do not accumulate without writes");
    }
}
