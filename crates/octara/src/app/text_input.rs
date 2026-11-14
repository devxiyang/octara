//! The document a platform text input sees. Native callbacks edit this owned value, then
//! deliver the edit to the window at a safe event-loop boundary. No app handlers run here.

use std::ops::Range;

/// An edit from a platform text input. Ranges count UTF-8 bytes in the displayed document,
/// including marked text. A missing replacement uses the marked range, then the selection.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TextEdit {
    /// Replace a displayed range with uncommitted text. `selection` is relative to `text`;
    /// none hides the input method's cursor. Empty text cancels the composition.
    Mark { text: String, selection: Option<Range<usize>>, replacement: Option<Range<usize>> },
    /// Replace a displayed range and accept the resulting document as one committed edit.
    Insert { text: String, replacement: Option<Range<usize>> },
    /// Accept the displayed document, removing its input-method marking.
    Unmark,
    /// Restore the committed document and the selection from before composition began.
    Cancel,
}

/// An owned platform editing document, suitable for synchronous native queries. The
/// committed value stays unchanged while a composition edits its displayed draft.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TextDocument {
    pub(crate) value: String,
    pub(crate) anchor: usize,
    pub(crate) caret: usize,
    pub(crate) composition: Option<Composition>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Composition {
    pub text: String,
    pub marked: Range<usize>,
    pub selection: Option<Range<usize>>,
}

fn range(text: &str, range: Range<usize>) -> Range<usize> {
    let a = crate::kernel::floor_boundary(text, range.start);
    let b = crate::kernel::floor_boundary(text, range.end);
    a.min(b)..a.max(b)
}

impl TextDocument {
    /// A committed document and its UTF-8 byte selection. The caret may precede the anchor.
    pub fn new(text: String, anchor: usize, caret: usize) -> Self {
        let anchor = crate::kernel::floor_boundary(&text, anchor);
        let caret = crate::kernel::floor_boundary(&text, caret);
        Self { value: text, anchor, caret, composition: None }
    }

    /// The complete displayed text, including uncommitted marked text.
    pub fn text(&self) -> &str {
        self.composition.as_ref().map_or(&self.value, |c| &c.text)
    }

    /// The displayed selection in UTF-8 bytes; none means the input method hides its cursor.
    pub fn selection(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map_or_else(|| Some(self.anchor.min(self.caret)..self.anchor.max(self.caret)), |c| c.selection.clone())
    }

    /// The input method's marked range in displayed UTF-8 bytes, or none when not composing.
    pub fn marked(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|c| c.marked.clone())
    }

    /// Apply an edit without calling application code. Ranges are ordered and clamped to
    /// Unicode scalar boundaries. Returns whether the document changed.
    pub fn apply(&mut self, edit: &TextEdit) -> bool {
        if matches!(edit, TextEdit::Cancel) || matches!(edit, TextEdit::Mark { text, .. } if text.is_empty()) {
            return self.composition.take().is_some();
        }
        if matches!(edit, TextEdit::Unmark) {
            let Some(c) = self.composition.take() else { return false };
            let selection = c.selection.unwrap_or(c.marked.end..c.marked.end);
            self.value = c.text;
            self.anchor = selection.start;
            self.caret = selection.end;
            return true;
        }
        let (text, replacement) = match edit {
            TextEdit::Mark { text, replacement, .. } | TextEdit::Insert { text, replacement } => (text, replacement),
            TextEdit::Unmark | TextEdit::Cancel => unreachable!(),
        };
        let replacement =
            replacement.clone().or_else(|| self.marked()).or_else(|| self.selection()).unwrap_or(self.text().len()..self.text().len());
        let replacement = range(self.text(), replacement);
        let end = replacement.start + text.len();
        let shown = format!("{}{text}{}", &self.text()[..replacement.start], &self.text()[replacement.end..]);
        match edit {
            TextEdit::Mark { selection, .. } => {
                let selection = selection.clone().map(|s| {
                    let s = range(text, s);
                    replacement.start + s.start..replacement.start + s.end
                });
                let composition = Composition { text: shown, marked: replacement.start..end, selection };
                if self.composition.as_ref() == Some(&composition) {
                    return false;
                }
                self.composition = Some(composition);
            }
            TextEdit::Insert { .. } => {
                let changed = self.value != shown || self.anchor != end || self.caret != end || self.composition.is_some();
                self.value = shown;
                self.anchor = end;
                self.caret = end;
                self.composition = None;
                return changed;
            }
            TextEdit::Unmark | TextEdit::Cancel => unreachable!(),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{byte_offset, utf16_offset};

    #[test]
    fn reconversion_uses_displayed_ranges_and_preserves_the_original_until_commit() {
        let original = TextDocument::new("A😀LatinZ".into(), 10, 5);
        let mut document = original.clone();
        document.apply(&TextEdit::Mark { text: "nihao".into(), selection: Some(2..5), replacement: None });
        assert_eq!(document.text(), "A😀nihaoZ");
        assert_eq!(document.marked(), Some(5..10));
        assert_eq!(document.selection(), Some(7..10));
        assert_eq!(document.value, "A😀LatinZ");
        document.apply(&TextEdit::Cancel);
        assert_eq!(document, original, "cancelling restores the reversed original selection");

        document.apply(&TextEdit::Mark { text: "nihao".into(), selection: Some(5..5), replacement: Some(5..10) });
        document.apply(&TextEdit::Insert { text: "你好".into(), replacement: None });
        assert_eq!(document.text(), "A😀你好Z");
        assert_eq!(document.selection(), Some(11..11));
        assert_eq!(document.marked(), None);
    }

    #[test]
    fn partial_marked_and_arbitrary_replacement_ranges_do_not_commit_early() {
        let mut document = TextDocument::new("<Latin>".into(), 1, 6);
        document.apply(&TextEdit::Mark { text: "abcde".into(), selection: None, replacement: None });
        assert_eq!(document.selection(), None);
        document.apply(&TextEdit::Mark { text: "你".into(), selection: Some(3..3), replacement: Some(2..4) });
        assert_eq!(document.text(), "<a你de>");
        assert_eq!(document.marked(), Some(2..5));
        assert_eq!(document.value, "<Latin>");
        document.apply(&TextEdit::Insert { text: "!".into(), replacement: Some(0..1) });
        assert_eq!(document.text(), "!a你de>", "explicit ranges replace the displayed document, retaining the draft outside that range");

        document.apply(&TextEdit::Mark { text: "拼".into(), selection: Some(0..3), replacement: Some(0..1) });
        document.apply(&TextEdit::Unmark);
        assert_eq!(document.text(), "拼a你de>");
        assert_eq!(document.selection(), Some(0..3));
        assert_eq!(document.marked(), None, "unmark accepts the draft instead of cancelling it");
    }

    #[test]
    fn utf16_surrogates_and_invalid_byte_ranges_are_clamped_without_splitting_text() {
        let mut document = TextDocument::new("A😀Z".into(), 1, 5);
        assert_eq!(byte_offset(document.text(), 2), 1, "the inside of a surrogate pair floors to its start");
        document.apply(&TextEdit::Mark {
            text: "中😀文".into(),
            selection: Some(Range { start: usize::MAX, end: 4 }),
            replacement: Some(Range { start: 4, end: 2 }),
        });
        assert_eq!(document.text(), "A中😀文😀Z");
        assert_eq!(document.marked(), Some(1..11));
        let selection = document.selection().unwrap();
        assert_eq!(selection, 4..11);
        assert_eq!(utf16_offset(document.text(), selection.start)..utf16_offset(document.text(), selection.end), 2..5);
        document.apply(&TextEdit::Insert { text: "好".into(), replacement: Some(Range { start: usize::MAX, end: 4 }) });
        assert_eq!(document.text(), "A中好");
    }
}
