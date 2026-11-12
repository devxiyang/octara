//! `TextBuffer`: reactive text with a change structure (ILC): edits are first-class `TextChange`s.
//!
//! A `TextBuffer` holds its string, a version signal and a bounded log of edits. Readers that need
//! the whole value (`get`, `with`) track the version as usual. The UI's `text(text_buffer)` applies
//! the log to the kernel: an append costs O(appended), not O(length). `TextBuffer` is a cheap,
//! reference-counted handle; it can live inside a list item and is reclaimed with it.

use crate::reactive::graph::{Signal, state};
use crate::reactive::list::{Cursor, Log, writable};
use std::cell::RefCell;
use std::ops::Range;
use std::panic::Location;
use std::rc::Rc;

/// An edit. Offsets are byte offsets into the text before the edit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TextChange {
    Append(String),
    Splice { start: usize, end: usize, insert: String },
}

struct Inner {
    value: RefCell<String>,
    log: RefCell<Log<TextChange>>,
    version: Signal<u64>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.version.dispose();
    }
}

#[derive(Clone)]
pub struct TextBuffer(Rc<Inner>);

/// Identity: two handles are equal when they are the same text.
impl PartialEq for TextBuffer {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for TextBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TextBuffer({:?})", self.0.value.borrow())
    }
}

/// Named text state for the current component (like `state`).
#[track_caller]
pub fn text_buffer(key: &'static str, init: impl FnOnce() -> String) -> TextBuffer {
    state(key, || TextBuffer::new(init())).get_untracked()
}

impl TextBuffer {
    /// A text owned by its handles (for example inside a list item).
    pub fn new(init: impl Into<String>) -> TextBuffer {
        TextBuffer(Rc::new(Inner { value: RefCell::new(init.into()), log: RefCell::new(Log::new()), version: Signal::detached(0) }))
    }

    /// The whole text (O(length)); tracks it.
    #[track_caller]
    pub fn get(&self) -> String {
        self.0.version.get();
        self.0.value.borrow().clone()
    }

    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&str) -> R) -> R {
        self.0.version.get();
        f(&self.0.value.borrow())
    }

    /// Which edit the text is at; tracks it (to follow a text without copying it).
    #[track_caller]
    pub fn version(&self) -> u64 {
        self.0.version.get()
    }

    pub fn get_untracked(&self) -> String {
        self.0.value.borrow().clone()
    }

    #[track_caller]
    pub fn len(&self) -> usize {
        self.0.version.get();
        self.0.value.borrow().len()
    }

    #[track_caller]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn edit(&self, at: &'static Location<'static>, c: TextChange) {
        if !writable(at) {
            return;
        }
        {
            let mut v = self.0.value.borrow_mut();
            match &c {
                TextChange::Append(s) => v.push_str(s),
                TextChange::Splice { start, end, insert } => v.replace_range(*start..*end, insert),
            }
        }
        self.0.log.borrow_mut().push(c);
        self.0.version.set_internal(self.0.version.get_untracked() + 1);
    }

    #[track_caller]
    pub fn append(&self, s: &str) {
        if !s.is_empty() {
            self.edit(Location::caller(), TextChange::Append(s.to_owned()));
        }
    }

    /// Replaces the byte range with `insert`.
    #[track_caller]
    pub fn splice(&self, range: Range<usize>, insert: &str) {
        self.edit(Location::caller(), TextChange::Splice { start: range.start, end: range.end, insert: insert.to_owned() });
    }

    #[track_caller]
    pub fn set(&self, s: &str) {
        let n = self.0.value.borrow().len();
        self.edit(Location::caller(), TextChange::Splice { start: 0, end: n, insert: s.to_owned() });
    }

    // ---- for the UI layer ----

    pub(crate) fn len_untracked(&self) -> usize {
        self.0.value.borrow().len()
    }
    pub(crate) fn slice_untracked(&self, range: Range<usize>) -> String {
        self.0.value.borrow()[range].to_owned()
    }
    /// The nearest char boundary at or after byte `i`.
    pub(crate) fn char_boundary_at(&self, mut i: usize) -> usize {
        let v = self.0.value.borrow();
        while i < v.len() && !v.is_char_boundary(i) {
            i += 1;
        }
        i.min(v.len())
    }

    pub(crate) fn subscribe(&self) -> Cursor {
        self.0.log.borrow_mut().subscribe()
    }
    pub(crate) fn track(&self) {
        self.0.version.get();
    }
    pub(crate) fn take(&self, c: &Cursor) -> Vec<TextChange> {
        self.0.log.borrow_mut().take(c)
    }
    /// Entries held in the edit log (to check it stays bounded).
    pub fn log_len(&self) -> usize {
        self.0.log.borrow().len()
    }
}
