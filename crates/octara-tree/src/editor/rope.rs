//! Persistent UTF-8 storage with bounded leaves and cached coordinate metrics.

use super::{EditorText, wipe};
use std::{
    fmt,
    ops::Range,
    sync::{Arc, OnceLock},
};

use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

const CHUNK_BYTES: usize = 1024;
type Link = Option<Arc<Node>>;

#[derive(Clone, Copy, Default)]
struct Metrics {
    bytes: usize,
    utf16: usize,
    breaks: usize,
}

impl Metrics {
    fn of(text: &str) -> Self {
        Self { bytes: text.len(), utf16: text.encode_utf16().count(), breaks: text.bytes().filter(|&byte| byte == b'\n').count() }
    }

    fn plus(self, right: Self) -> Self {
        Self { bytes: self.bytes + right.bytes, utf16: self.utf16 + right.utf16, breaks: self.breaks + right.breaks }
    }
}

enum Children {
    Leaf(EditorText),
    Branch(Arc<Node>, Arc<Node>),
}

struct Node {
    metrics: Metrics,
    height: usize,
    children: Children,
}

impl Node {
    fn leaf(text: String) -> Arc<Self> {
        Arc::new(Self { metrics: Metrics::of(&text), height: 1, children: Children::Leaf(EditorText(text)) })
    }

    fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        let metrics = left.metrics.plus(right.metrics);
        let height = 1 + left.height.max(right.height);
        Arc::new(Self { metrics, height, children: Children::Branch(left, right) })
    }

    fn branches(&self) -> (&Arc<Self>, &Arc<Self>) {
        let Children::Branch(left, right) = &self.children else { unreachable!("a taller subtree has children") };
        (left, right)
    }

    fn find(&self, index: usize, measure: fn(Metrics) -> usize) -> (Metrics, &str, usize) {
        let mut node = self;
        let mut index = index;
        let mut prefix = Metrics::default();
        loop {
            match &node.children {
                Children::Leaf(text) => return (prefix, &text.0, index),
                Children::Branch(left, right) => {
                    let left_len = measure(left.metrics);
                    if index < left_len {
                        node = left;
                    } else {
                        prefix = prefix.plus(left.metrics);
                        index -= left_len;
                        node = right;
                    }
                }
            }
        }
    }

    fn visit(&self, range: Range<usize>, visit: &mut impl FnMut(&str)) {
        if range.is_empty() {
            return;
        }
        match &self.children {
            Children::Leaf(text) => visit(&text.0[range]),
            Children::Branch(left, right) => {
                let split = left.metrics.bytes;
                if range.start < split {
                    left.visit(range.start..range.end.min(split), visit);
                }
                if range.end > split {
                    right.visit(range.start.saturating_sub(split)..range.end - split, visit);
                }
            }
        }
    }
}

fn build(text: &str) -> Arc<Node> {
    if text.len() <= CHUNK_BYTES {
        return Node::leaf(text.to_owned());
    }
    let middle = text.floor_char_boundary(text.len() / 2);
    Node::branch(build(&text[..middle]), build(&text[middle..]))
}

fn concat(left: Link, right: Link) -> Link {
    match (left, right) {
        (Some(left), Some(right)) => Some(join(left, right)),
        (left, right) => left.or(right),
    }
}

fn join(left: Arc<Node>, right: Arc<Node>) -> Arc<Node> {
    if left.height > right.height + 1 {
        let (outer, inner) = left.branches();
        return rebalance(outer.clone(), join(inner.clone(), right));
    }
    if right.height > left.height + 1 {
        let (inner, outer) = right.branches();
        return rebalance(join(left, inner.clone()), outer.clone());
    }
    if let (Children::Leaf(a), Children::Leaf(b)) = (&left.children, &right.children)
        && a.0.len() + b.0.len() <= CHUNK_BYTES
    {
        let mut text = String::with_capacity(a.0.len() + b.0.len());
        text.push_str(&a.0);
        text.push_str(&b.0);
        return Node::leaf(text);
    }
    Node::branch(left, right)
}

fn rebalance(left: Arc<Node>, right: Arc<Node>) -> Arc<Node> {
    if left.height > right.height + 1 {
        let (outer, inner) = left.branches();
        if outer.height < inner.height {
            let (near, far) = inner.branches();
            return Node::branch(Node::branch(outer.clone(), near.clone()), Node::branch(far.clone(), right));
        }
        return Node::branch(outer.clone(), Node::branch(inner.clone(), right));
    }
    if right.height > left.height + 1 {
        let (inner, outer) = right.branches();
        if outer.height < inner.height {
            let (near, far) = inner.branches();
            return Node::branch(Node::branch(left, near.clone()), Node::branch(far.clone(), outer.clone()));
        }
        return Node::branch(Node::branch(left, inner.clone()), outer.clone());
    }
    Node::branch(left, right)
}

fn split(node: Link, byte: usize) -> (Link, Link) {
    let Some(node) = node else { return (None, None) };
    if byte == 0 {
        return (None, Some(node));
    }
    if byte == node.metrics.bytes {
        return (Some(node), None);
    }
    match &node.children {
        Children::Leaf(text) => (Some(Node::leaf(text.0[..byte].into())), Some(Node::leaf(text.0[byte..].into()))),
        Children::Branch(left, right) => {
            let middle = left.metrics.bytes;
            if byte < middle {
                let (before, after) = split(Some(left.clone()), byte);
                (before, concat(after, Some(right.clone())))
            } else {
                let (before, after) = split(Some(right.clone()), byte - middle);
                (concat(Some(left.clone()), before), after)
            }
        }
    }
}

struct Chunks<'a>(Vec<&'a Node>);

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        let mut node = self.0.pop()?;
        loop {
            match &node.children {
                Children::Leaf(text) => return Some(text.0.as_bytes()),
                Children::Branch(left, right) => {
                    self.0.push(right);
                    node = left;
                }
            }
        }
    }
}

#[derive(Default)]
struct Buffer {
    root: Link,
    flattened: OnceLock<EditorText>,
}

/// UTF-8 editor text with logarithmic range edits and coordinate queries.
/// Clones share storage and are independent of subsequent edits. Text storage is
/// wiped as its last owner releases it.
#[derive(Clone, Default)]
pub struct EditorRope {
    buffer: Arc<Buffer>,
}

impl EditorRope {
    pub fn new(text: impl Into<String>) -> Self {
        let mut text = text.into();
        if text.is_empty() {
            return Self::default();
        }
        let node = if text.len() <= CHUNK_BYTES {
            Node::leaf(text)
        } else {
            let node = build(&text);
            wipe(&mut text);
            node
        };
        Self::from_root(Some(node))
    }

    fn from_root(root: Link) -> Self {
        Self { buffer: Arc::new(Buffer { root, flattened: OnceLock::new() }) }
    }

    fn metrics(&self) -> Metrics {
        self.buffer.root.as_ref().map_or_else(Metrics::default, |node| node.metrics)
    }

    /// Length in UTF-8 bytes.
    pub fn len(&self) -> usize {
        self.metrics().bytes
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.root.is_none()
    }

    pub fn len_utf16(&self) -> usize {
        self.metrics().utf16
    }

    /// Number of LF-terminated lines. A CRLF sequence counts once.
    pub fn line_breaks(&self) -> usize {
        self.metrics().breaks
    }

    pub fn is_char_boundary(&self, byte: usize) -> bool {
        if byte >= self.len() {
            return byte == self.len();
        }
        let (_, leaf, index) = self.buffer.root.as_ref().unwrap().find(byte, |metrics| metrics.bytes);
        leaf.is_char_boundary(index)
    }

    /// The next extended grapheme boundary in the requested direction.
    pub fn grapheme_edge(&self, byte: usize, backward: bool) -> Option<usize> {
        if !self.is_char_boundary(byte) {
            return None;
        }
        if (byte == 0 && backward) || (byte == self.len() && !backward) {
            return Some(byte);
        }
        let root = self.buffer.root.as_ref()?;
        let chunk_at = |byte| {
            let (prefix, chunk, _) = root.find(byte, |metrics| metrics.bytes);
            (prefix.bytes, chunk)
        };
        let mut cursor = GraphemeCursor::new(byte, self.len(), true);
        let (mut start, mut chunk) = chunk_at(byte - usize::from(backward));
        loop {
            let result = if backward { cursor.prev_boundary(chunk, start) } else { cursor.next_boundary(chunk, start) };
            match result {
                Ok(edge) => return edge.or(Some(byte)),
                Err(GraphemeIncomplete::NextChunk) => (start, chunk) = chunk_at(start + chunk.len()),
                Err(GraphemeIncomplete::PrevChunk) => (start, chunk) = chunk_at(start - 1),
                Err(GraphemeIncomplete::PreContext(end)) => {
                    let (context_start, context) = chunk_at(end - 1);
                    cursor.provide_context(&context[..end - context_start], context_start);
                }
                Err(GraphemeIncomplete::InvalidOffset) => return None,
            }
        }
    }

    /// Returns the UTF-16 offset of a UTF-8 character boundary.
    pub fn byte_to_utf16(&self, byte: usize) -> Option<usize> {
        if byte >= self.len() {
            return (byte == self.len()).then(|| self.len_utf16());
        }
        let (prefix, leaf, index) = self.buffer.root.as_ref()?.find(byte, |metrics| metrics.bytes);
        Some(prefix.utf16 + leaf.get(..index)?.encode_utf16().count())
    }

    /// Returns the UTF-8 byte offset of a UTF-16 character boundary.
    pub fn utf16_to_byte(&self, offset: usize) -> Option<usize> {
        let (floor, ceil) = self.utf16_byte_bounds(offset)?;
        (floor == ceil).then_some(floor)
    }

    /// Character boundaries around a UTF-16 offset. They differ only inside a surrogate pair.
    pub fn utf16_byte_bounds(&self, offset: usize) -> Option<(usize, usize)> {
        if offset >= self.len_utf16() {
            return (offset == self.len_utf16()).then(|| (self.len(), self.len()));
        }
        let (prefix, leaf, mut index) = self.buffer.root.as_ref()?.find(offset, |metrics| metrics.utf16);
        for (byte, ch) in leaf.char_indices() {
            let byte = prefix.bytes + byte;
            if index == 0 {
                return Some((byte, byte));
            }
            if index < ch.len_utf16() {
                return Some((byte, byte + ch.len_utf8()));
            }
            index -= ch.len_utf16();
        }
        unreachable!("offset within leaf")
    }

    /// Byte offset of the start of a zero-based logical line.
    pub fn line_start(&self, line: usize) -> Option<usize> {
        if line == 0 {
            return Some(0);
        }
        if line > self.line_breaks() {
            return None;
        }
        let (prefix, leaf, index) = self.buffer.root.as_ref()?.find(line - 1, |metrics| metrics.breaks);
        let (byte, _) = leaf.bytes().enumerate().filter(|&(_, byte)| byte == b'\n').nth(index)?;
        Some(prefix.bytes + byte + 1)
    }

    /// Zero-based logical line and UTF-8 byte column at a character boundary.
    pub fn byte_to_line_col(&self, byte: usize) -> Option<(usize, usize)> {
        if byte == self.len() {
            let line = self.line_breaks();
            return Some((line, byte - self.line_start(line)?));
        }
        if byte > self.len() {
            return None;
        }
        let (prefix, leaf, index) = self.buffer.root.as_ref()?.find(byte, |metrics| metrics.bytes);
        let line = prefix.breaks + leaf.get(..index)?.bytes().filter(|&byte| byte == b'\n').count();
        Some((line, byte - self.line_start(line)?))
    }

    /// Byte offset of a character boundary within a logical line.
    pub fn line_col_to_byte(&self, line: usize, column: usize) -> Option<usize> {
        let byte = self.line_start(line)?.checked_add(column)?;
        let end = self.line_start(line + 1).map_or(self.len(), |next| next - 1);
        (byte <= end && self.is_char_boundary(byte)).then_some(byte)
    }

    fn chunks(&self) -> Chunks<'_> {
        Chunks(self.buffer.root.as_deref().into_iter().collect())
    }

    /// A contiguous view for bindings and shaping. The first request per snapshot
    /// materializes the document when it spans more than one leaf.
    pub fn as_str(&self) -> &str {
        let Some(root) = &self.buffer.root else { return "" };
        if let Children::Leaf(text) = &root.children {
            return &text.0;
        }
        &self
            .buffer
            .flattened
            .get_or_init(|| {
                let mut text = String::with_capacity(root.metrics.bytes);
                root.visit(0..root.metrics.bytes, &mut |chunk| text.push_str(chunk));
                EditorText(text)
            })
            .0
    }

    /// Copies a UTF-8 range without materializing the whole document.
    pub fn slice(&self, range: Range<usize>) -> Option<String> {
        if range.start > range.end || !self.is_char_boundary(range.start) || !self.is_char_boundary(range.end) {
            return None;
        }
        let mut result = String::with_capacity(range.len());
        if let Some(root) = &self.buffer.root {
            root.visit(range, &mut |chunk| result.push_str(chunk));
        }
        Some(result)
    }

    /// Replaces a UTF-8 byte range, preserving shared chunks outside the edit.
    pub fn replace(&mut self, range: Range<usize>, insert: &str) -> bool {
        let Some(next) = self.replaced(range, insert) else { return false };
        *self = next;
        true
    }

    pub(super) fn range_eq(&self, range: Range<usize>, text: &str) -> bool {
        if range.len() != text.len() {
            return false;
        }
        if let Some(flattened) = self.buffer.flattened.get() {
            return flattened.0.get(range) == Some(text);
        }
        let mut equal = true;
        let mut offset = 0;
        if let Some(root) = &self.buffer.root {
            root.visit(range, &mut |chunk| {
                equal &= text.as_bytes()[offset..offset + chunk.len()] == *chunk.as_bytes();
                offset += chunk.len();
            });
        }
        equal
    }

    pub(super) fn replaced(&self, range: Range<usize>, insert: &str) -> Option<Self> {
        if range.start > range.end || !self.is_char_boundary(range.start) || !self.is_char_boundary(range.end) {
            return None;
        }
        if self.range_eq(range.clone(), insert) {
            return Some(self.clone());
        }
        let (before, rest) = split(self.buffer.root.clone(), range.start);
        let (_, after) = split(rest, range.len());
        let middle = (!insert.is_empty()).then(|| build(insert));
        Some(Self::from_root(concat(concat(before, middle), after)))
    }

    pub(super) fn without_flattened(&self) -> Self {
        if self.buffer.flattened.get().is_some() { Self::from_root(self.buffer.root.clone()) } else { self.clone() }
    }

    pub(super) fn shares_storage(&self, other: &Self) -> bool {
        match (&self.buffer.root, &other.buffer.root) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}

impl fmt::Debug for EditorRope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditorRope").field("bytes", &self.len()).field("utf16", &self.len_utf16()).finish_non_exhaustive()
    }
}

impl PartialEq for EditorRope {
    fn eq(&self, other: &Self) -> bool {
        if self.shares_storage(other) {
            return true;
        }
        if self.len() != other.len() {
            return false;
        }
        if let Some(flattened) = self.buffer.flattened.get() {
            return other.range_eq(0..self.len(), &flattened.0);
        }
        if let Some(flattened) = other.buffer.flattened.get() {
            return self.range_eq(0..self.len(), &flattened.0);
        }
        let (mut ours, mut theirs) = (self.chunks(), other.chunks());
        let (mut left, mut right) = (ours.next().unwrap_or_default(), theirs.next().unwrap_or_default());
        while !left.is_empty() {
            let length = left.len().min(right.len());
            if left[..length] != right[..length] {
                return false;
            }
            left = if length == left.len() { ours.next().unwrap_or_default() } else { &left[length..] };
            right = if length == right.len() { theirs.next().unwrap_or_default() } else { &right[length..] };
        }
        true
    }
}

impl Eq for EditorRope {}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_tree(node: &Node) {
        match &node.children {
            Children::Leaf(text) => {
                assert!(text.0.len() <= CHUNK_BYTES);
                assert_eq!(node.height, 1);
            }
            Children::Branch(left, right) => {
                assert!(left.height.abs_diff(right.height) <= 1);
                assert_eq!(node.height, 1 + left.height.max(right.height));
                assert_eq!(node.metrics.bytes, left.metrics.bytes + right.metrics.bytes);
                check_tree(left);
                check_tree(right);
            }
        }
    }

    #[test]
    fn persistent_edits_match_string_and_keep_balanced_chunks() {
        let original = "0123456789界🎨\r\n".repeat(1000);
        let mut text = original.clone();
        let mut rope = EditorRope::new(text.clone());
        let snapshot = rope.clone();
        assert!(rope.shares_storage(&snapshot));
        assert!(std::ptr::eq(rope.as_str(), snapshot.as_str()));

        let mut state = 21_u32;
        let mut index = |len| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            state as usize % len
        };
        for iteration in 0..750 {
            let start = text.floor_char_boundary(index(text.len() + 1));
            let limit = if iteration % 3 == 0 { 40 } else { 4 };
            let end = text.floor_char_boundary((start + index(limit)).min(text.len()));
            let insert = ["", "\n", "😀", "foo", "文\r\n", "á", "👩🏽‍💻"][index(7)];
            text.replace_range(start..end, insert);
            assert!(rope.replace(start..end, insert));
            assert_eq!(rope.as_str(), text);
            assert_eq!(rope.len_utf16(), text.encode_utf16().count());
            assert_eq!(rope.line_breaks(), text.bytes().filter(|&byte| byte == b'\n').count());
            if let Some(root) = &rope.buffer.root {
                check_tree(root);
            }
        }
        assert_eq!(snapshot.as_str(), original);
    }

    #[test]
    fn compares_independent_roots_without_materializing_text() {
        let text = "a".repeat(CHUNK_BYTES * 4);
        let first = EditorRope::new(&text);
        let mut second = EditorRope::new(text);
        assert_eq!(first, second);
        assert!(second.replace(5..6, "b"));
        assert_ne!(first, second);
        assert!(second.replace(5..6, "a"));
        assert_eq!(first, second);
        assert!(first.buffer.flattened.get().is_none());
        assert!(second.buffer.flattened.get().is_none());
    }

    #[test]
    fn coordinates_and_graphemes_cross_chunk_boundaries() {
        use crate::selection::grapheme_edge;

        let text = format!("{}👩🏽‍💻{}🇦🇺🇫🇷\r\n{}á", "a".repeat(1020), "\n文🎨".repeat(490), "b".repeat(998));
        let rope = EditorRope::new(text.clone());
        assert!(!rope.is_char_boundary(text.find('🎨').unwrap() + 1));
        assert_eq!(rope.slice(1019..1035).as_deref(), text.get(1019..1035));
        let mut utf16 = 0;
        let mut line = 0;
        let mut line_start = 0;
        for (byte, ch) in text.char_indices() {
            assert_eq!(rope.byte_to_utf16(byte), Some(utf16));
            assert_eq!(rope.utf16_to_byte(utf16), Some(byte));
            if ch.len_utf16() == 2 {
                assert_eq!(rope.utf16_to_byte(utf16 + 1), None);
                assert_eq!(rope.utf16_byte_bounds(utf16 + 1), Some((byte, byte + ch.len_utf8())));
            }
            assert_eq!(rope.line_start(line), Some(line_start));
            assert_eq!(rope.byte_to_line_col(byte), Some((line, byte - line_start)));
            assert_eq!(rope.line_col_to_byte(line, byte - line_start), Some(byte));
            assert_eq!(rope.grapheme_edge(byte, true), Some(grapheme_edge(&text, byte, true)));
            assert_eq!(rope.grapheme_edge(byte, false), Some(grapheme_edge(&text, byte, false)));
            utf16 += ch.len_utf16();
            if ch == '\n' {
                line += 1;
                line_start = byte + 1;
            }
        }
        assert_eq!(rope.byte_to_utf16(text.len()), Some(utf16));
        assert_eq!(rope.byte_to_line_col(text.len()), Some((line, text.len() - line_start)));
        assert_eq!(rope.line_col_to_byte(0, usize::MAX), None);
    }
}
