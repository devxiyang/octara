//! The document a platform text input sees. Native callbacks edit this owned value, then
//! deliver the edit to the window at a safe event-loop boundary. No app handlers run here.

use octara_primitives::paint::Rect;
use octara_primitives::text::TextEdit;
use serde::{Deserialize, Deserializer, Serialize};
use std::ops::Range;

/// An owned platform editing document, suitable for synchronous native queries. The
/// committed value stays unchanged while a composition edits its displayed draft.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct TextDocument {
    pub(crate) value: String,
    pub(crate) anchor: usize,
    pub(crate) caret: usize,
    pub(crate) composition: Option<Composition>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) struct Composition {
    pub text: String,
    pub marked: Range<usize>,
    pub selection: Option<Range<usize>>,
}

impl<'de> Deserialize<'de> for TextDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Document {
            value: String,
            anchor: usize,
            caret: usize,
            composition: Option<Composition>,
        }

        let Document { value, anchor, caret, composition } = Document::deserialize(deserializer)?;
        let valid_range = |text: &str, range: &Range<usize>| {
            range.start <= range.end && text.is_char_boundary(range.start) && text.is_char_boundary(range.end)
        };
        let valid_composition = composition.as_ref().is_none_or(|c| {
            valid_range(&c.text, &c.marked) && c.selection.as_ref().is_none_or(|selection| valid_range(&c.text, selection))
        });
        if !value.is_char_boundary(anchor) || !value.is_char_boundary(caret) || !valid_composition {
            return Err(serde::de::Error::custom("invalid UTF-8 ranges in a text document"));
        }
        Ok(Self { value, anchor, caret, composition })
    }
}

fn range(text: &str, range: Range<usize>) -> Range<usize> {
    let a = super::floor_boundary(text, range.start);
    let b = super::floor_boundary(text, range.end);
    a.min(b)..a.max(b)
}

impl TextDocument {
    /// A committed document and its UTF-8 byte selection. The caret may precede the anchor.
    pub fn new(text: String, anchor: usize, caret: usize) -> Self {
        let anchor = super::floor_boundary(&text, anchor);
        let caret = super::floor_boundary(&text, caret);
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

/// A focused field's published layout configuration. Native text queries can shape their
/// current document with the same fonts, wrapping and caret scrolling as the field, without
/// reentering a window. Only the shared text engine's bounded caches may change.
#[derive(Clone)]
pub struct TextInputLayout {
    pub(crate) resources: std::rc::Rc<crate::kernel::Resources>,
    pub(crate) style: octara_text::TextStyle,
    pub(crate) content: Rect,
    pub(crate) multiline: bool,
    pub(crate) scroll: f32,
    pub(crate) kept: Option<(usize, usize)>,
    pub(crate) fonts: u64,
}

impl PartialEq for TextInputLayout {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.resources, &other.resources)
            && self.style == other.style
            && self.content == other.content
            && self.multiline == other.multiline
            && self.scroll == other.scroll
            && self.kept == other.kept
            && self.fonts == other.fonts
    }
}

impl std::fmt::Debug for TextInputLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextInputLayout")
            .field("style", &self.style)
            .field("content", &self.content)
            .field("multiline", &self.multiline)
            .field("scroll", &self.scroll)
            .field("kept", &self.kept)
            .field("fonts", &self.fonts)
            .finish_non_exhaustive()
    }
}

/// An owned layout for platform text queries, with displayed UTF-8 byte offsets (including
/// marked text). A host may use it only while its document text still matches this frame.
#[derive(Clone, PartialEq, Debug)]
pub struct TextGeometry {
    /// The text's origin in logical window coordinates, including its current scroll.
    pub origin: (f32, f32),
    pub lines: Vec<octara_text::VisualLine>,
}

impl TextGeometry {
    /// The first line fragment of a displayed range: its actual byte range and logical
    /// window rect. The actual range expands to available caret stops. An insertion point
    /// has zero width; the host converts the rect to screen coordinates.
    pub fn first_rect(&self, range: std::ops::Range<usize>) -> Option<(std::ops::Range<usize>, Rect)> {
        let last = self.lines.last()?.end;
        let start = range.start.min(range.end).min(last);
        let end = range.start.max(range.end).min(last);
        let index = self.lines.iter().rposition(|l| l.start <= start).unwrap_or(0);
        let line = &self.lines[index];
        let (s, x) = line.stops.iter().rev().find(|(b, _)| *b <= start).or(line.stops.first()).copied()?;
        let (e, right) =
            if start == end { (s, x) } else { line.stops.iter().find(|(b, _)| *b >= end.min(line.end)).or(line.stops.last()).copied()? };
        // Line ranges omit their newline. Include that gap in a nonempty actual range so
        // a caller iterating line fragments advances even across an otherwise empty line.
        let e = if start != end && end > line.end {
            self.lines.get(index + 1).filter(|next| next.start > line.end).map_or(e, |next| e.max(end.min(next.start)))
        } else {
            e
        };
        Some((s..e, Rect { x: self.origin.0 + x.min(right), y: self.origin.1 + line.top, w: (right - x).abs(), h: line.height }))
    }

    /// The displayed byte index nearest a point in logical window coordinates. Unlike an
    /// editing caret's value offset, indices inside marked text are kept distinct.
    pub fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        let line = self.lines.iter().rev().find(|l| l.top <= y - self.origin.1).or(self.lines.first())?;
        let x = x - self.origin.0;
        line.stops.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map(|s| s.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_documents_validate_unicode_boundaries() {
        let mut document = TextDocument::new("😀Z".into(), 4, 0);
        document.apply(&TextEdit::Mark { text: "你".into(), selection: Some(3..3), replacement: None });
        let serialized = serde_json::to_value(&document).unwrap();
        assert_eq!(serde_json::from_value::<TextDocument>(serialized.clone()).unwrap(), document);

        let mut invalid_base = serialized.clone();
        invalid_base["anchor"] = 1.into();
        assert!(serde_json::from_value::<TextDocument>(invalid_base).is_err());

        let mut invalid_composition = serialized;
        invalid_composition["composition"]["marked"]["end"] = 1.into();
        assert!(serde_json::from_value::<TextDocument>(invalid_composition).is_err());
    }

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
}
