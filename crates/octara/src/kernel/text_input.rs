//! The document a platform text input sees. Native callbacks edit this owned value, then
//! deliver the edit to the window at a safe event-loop boundary. No app handlers run here.

use octara_primitives::paint::Rect;
use octara_primitives::text::TextEdit;
use octara_text::{PresentedLine, ShapedRun, TextFragment, TextPresentation};
use serde::{Deserialize, Deserializer, Serialize};
use std::ops::Range;
use std::sync::Arc;

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
#[derive(Clone, Default, PartialEq, Debug)]
pub struct TextGeometry {
    /// The text's origin in logical window coordinates, including its current scroll.
    pub origin: (f32, f32),
    pub(crate) lines: Arc<[EditorLine]>,
}

/// An editor's unfitted presentation. Paint and platform queries share these visual lines.
pub(crate) struct EditorLayout {
    pub lines: Arc<[EditorLine]>,
    pub glyphs: Vec<ShapedRun>,
}

impl EditorLayout {
    pub fn new(mut presentation: TextPresentation, line_height: f32, width: Option<f32>) -> Self {
        // Even an empty editor has a caret. Providers need not shape an empty document.
        if presentation.lines.is_empty() {
            presentation.lines.push(PresentedLine {
                start: 0,
                top: 0.0,
                height: line_height,
                fragments: Vec::new(),
                hard_break: None,
                break_at: None,
                ellipsis: None,
            });
        }
        Self { lines: presentation.lines.into_iter().map(|line| EditorLine::new(line, width)).collect(), glyphs: presentation.glyphs }
    }
}

#[derive(PartialEq, Debug)]
pub(crate) struct EditorLine {
    pub text: PresentedLine,
    pub newline: Option<TextFragment>,
    stops: Vec<(usize, f32)>,
}

impl EditorLine {
    fn new(text: PresentedLine, width: Option<f32>) -> Self {
        let mut stops: Vec<_> = text
            .fragments
            .iter()
            .flat_map(|f| {
                let (leading, trailing) = f.edges();
                [(f.source.start, false, leading), (f.source.end, true, trailing)]
            })
            .collect();
        // At a shared bidi boundary, prefer the following logical fragment's leading edge.
        stops.sort_unstable_by_key(|&(byte, trailing, _)| (byte, trailing));
        stops.dedup_by_key(|s| s.0);
        let mut stops: Vec<_> = stops.into_iter().map(|(byte, _, x)| (byte, x)).collect();
        if stops.is_empty() {
            stops.push((text.start, 0.0));
        }
        let newline = text.hard_break.clone().map(|source| {
            let edge = text.break_x();
            let marker_width = text.height * 0.25;
            let rtl = text.fragments.iter().any(|f| f.source.end == source.start && f.rtl);
            let left = edge.max(0.0);
            let right = width.map_or(f32::INFINITY, |w| (w - edge).max(0.0));
            let (outward, inward) = if rtl { (left, right) } else { (right, left) };
            // At the content edge move the marker inward, keeping its leading edge
            // on the insertion point before the break.
            let rtl = rtl ^ (outward < marker_width && inward > outward);
            let width = marker_width.min(if rtl { left } else { right });
            let x = if rtl { edge - width } else { edge };
            TextFragment { source, x, width, rtl }
        });
        Self { text, newline, stops }
    }

    pub fn end(&self) -> usize {
        self.stops.last().unwrap().0
    }

    pub fn floor(&self, byte: usize) -> (usize, f32) {
        self.stops[self.stops.partition_point(|s| s.0 <= byte).saturating_sub(1)]
    }

    fn ceil(&self, byte: usize) -> usize {
        self.stops.get(self.stops.partition_point(|s| s.0 < byte)).map_or_else(|| self.end(), |s| s.0)
    }

    pub fn caret_stops(&self) -> &[(usize, f32)] {
        // A shared wrap offset stands on the next line. Hit testing and Home/End use only
        // positions that can subsequently draw a caret on this line.
        if self.text.break_at == Some(self.end()) && self.newline.is_none() && self.stops.len() > 1 {
            &self.stops[..self.stops.len() - 1]
        } else {
            &self.stops
        }
    }

    pub fn index_at(&self, x: f32, under: bool) -> usize {
        let fragments = &self.text.fragments;
        let fragment = fragments.partition_point(|f| f.x <= x).checked_sub(1).and_then(|i| fragments.get(i)).filter(|f| x < f.x + f.width);
        if under && let Some(fragment) = fragment {
            return fragment.source.start;
        }
        // Two logical stops can share an x at a bidi boundary. Prefer the one touching
        // the visual fragment at the pointer.
        let adjacent = |byte| fragment.is_some_and(|f| byte == f.source.start || byte == f.source.end);
        self.caret_stops()
            .iter()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()).then_with(|| adjacent(b.0).cmp(&adjacent(a.0))))
            .unwrap()
            .0
    }

    pub fn range_rects(&self, range: Range<usize>) -> Vec<Rect> {
        if range.is_empty() {
            return Vec::new();
        }
        let mut rects = self.text.range_rects(range.clone());
        if let Some(f) = &self.newline
            && range.start < f.source.end
            && range.end > f.source.start
        {
            let mut marker = Rect { x: f.x, y: self.text.top, w: f.width, h: self.text.height };
            // A directional boundary can put the marker beside or within a visual interval.
            let start = rects.partition_point(|r| r.right() < marker.x - 0.01);
            let end = rects.partition_point(|r| r.x <= marker.right() + 0.01);
            if start < end {
                let right = marker.right().max(rects[end - 1].right());
                marker.x = marker.x.min(rects[start].x);
                marker.w = right - marker.x;
            }
            rects.splice(start..end, [marker]);
        }
        rects
    }

    pub fn line_at(lines: &[Self], y: f32) -> usize {
        lines.partition_point(|l| l.text.top <= y).saturating_sub(1)
    }

    pub fn line_of(lines: &[Self], byte: usize) -> usize {
        lines.partition_point(|l| l.text.start <= byte).saturating_sub(1)
    }
}

impl TextGeometry {
    /// The first line fragment of a displayed range: its actual byte range and logical
    /// window rect. The actual range expands to available caret stops. An insertion point
    /// has zero width; the host converts the rect to screen coordinates.
    pub fn first_rect(&self, range: Range<usize>) -> Option<(Range<usize>, Rect)> {
        let last = self.lines.last()?.end();
        let start = range.start.min(range.end).min(last);
        let end = range.start.max(range.end).min(last);
        let line = &self.lines[EditorLine::line_of(&self.lines, start)];
        let (s, x) = line.floor(start);
        let e = if start == end {
            s
        } else if let Some(newline) = &line.newline
            && end > newline.source.start
        {
            newline.source.end
        } else {
            line.ceil(end)
        };
        let rects = line.range_rects(s..e);
        let (left, right) = rects.first().zip(rects.last()).map_or((x, x), |(first, last)| (first.x, last.right()));
        let rect = Rect { x: self.origin.0 + left, y: self.origin.1 + line.text.top, w: right - left, h: line.text.height };
        Some((s..e, rect))
    }

    /// The displayed byte index nearest a point in logical window coordinates. Unlike an
    /// editing caret's value offset, indices inside marked text are kept distinct.
    pub fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        let line = self.lines.get(EditorLine::line_at(&self.lines, y - self.origin.1))?;
        Some(line.index_at(x - self.origin.0, false))
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
