//! Bounded edit history with independent, disposable reader cursors.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

pub const DEFAULT_DELTA_LOG_CAP: usize = 4096;
const DEFAULT_DELTA_LOG_BYTES: usize = 1024 * 1024;
static NEXT_LOG: AtomicU64 = AtomicU64::new(1);

/// The next entry to read from one particular log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    log: u64,
    next: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Changes<D> {
    Delta(Vec<D>),
    /// History was lost. Take an authoritative snapshot before reading further.
    Reset,
}

pub struct DeltaLog<D> {
    id: u64,
    next: u64,
    entries: VecDeque<(D, usize)>,
    bytes: usize,
    capacity: usize,
    byte_capacity: usize,
}

impl<D> Default for DeltaLog<D> {
    fn default() -> Self {
        Self::new(DEFAULT_DELTA_LOG_CAP)
    }
}

impl<D> DeltaLog<D> {
    pub fn new(capacity: usize) -> Self {
        Self {
            id: NEXT_LOG.fetch_add(1, Ordering::Relaxed),
            next: 0,
            entries: VecDeque::new(),
            bytes: 0,
            capacity,
            byte_capacity: DEFAULT_DELTA_LOG_BYTES,
        }
    }

    /// Starts reading with the next edit. A default cursor can request a first snapshot.
    pub fn cursor(&self) -> Cursor {
        Cursor { log: self.id, next: self.next }
    }

    pub fn push(&mut self, delta: D) {
        self.push_sized(delta, 0);
    }

    /// Includes externally allocated memory owned by this entry in the byte budget.
    pub fn push_sized(&mut self, delta: D, owned_bytes: usize) {
        self.next += 1;
        let bytes = size_of::<D>().saturating_add(owned_bytes);
        if bytes > self.byte_capacity || self.capacity == 0 {
            self.entries.clear();
            self.bytes = 0;
            return;
        }
        while self.entries.len() >= self.capacity || self.bytes + bytes > self.byte_capacity {
            let (_, removed_bytes) = self.entries.pop_front().expect("log has entries to evict");
            self.bytes -= removed_bytes;
        }
        self.entries.push_back((delta, bytes));
        self.bytes += bytes;
    }

    /// Invalidates all existing cursors without retaining an edit.
    pub fn reset(&mut self) {
        self.next += 1;
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn take(&self, cursor: &mut Cursor) -> Changes<D>
    where
        D: Clone,
    {
        let first = self.next - self.entries.len() as u64;
        let changes = if cursor.log == self.id && (first..=self.next).contains(&cursor.next) {
            Changes::Delta(self.entries.range((cursor.next - first) as usize..).map(|(delta, _)| delta.clone()).collect())
        } else {
            Changes::Reset
        };
        *cursor = self.cursor();
        changes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_cursors_reset_after_overflow_without_partial_replay() {
        let mut log = DeltaLog::new(2);
        let mut stalled = log.cursor();
        let mut active = stalled;
        for i in 0..10 {
            log.push(i);
            assert_eq!(log.take(&mut active), Changes::Delta(vec![i]));
        }
        assert_eq!(log.len(), 2);
        assert_eq!(log.take(&mut stalled), Changes::Reset);
        log.push(10);
        assert_eq!(log.take(&mut stalled), Changes::Delta(vec![10]));
        assert_eq!(DeltaLog::<i32>::default().take(&mut stalled), Changes::Reset);
    }

    #[test]
    fn oversized_edits_discard_history() {
        let mut log = DeltaLog::default();
        let mut reader = log.cursor();
        log.push(1);
        log.push_sized(2, DEFAULT_DELTA_LOG_BYTES);
        assert!(log.is_empty());
        assert_eq!(log.take(&mut reader), Changes::Reset);
    }
}
