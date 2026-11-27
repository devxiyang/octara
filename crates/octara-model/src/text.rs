//! Shared text style values and platform editing operations.

use crate::geometry::{Point, Rect, Size};
use crate::image::ImageSource;
use crate::paint::{CanvasOp, Image, NodeIcon, ObjectFit};
use crate::theme::ColorValue;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// How a text uses the width it is given.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Wrap {
    /// Breaks between words to fit.
    #[default]
    Words,
    /// One line per paragraph, as wide as it needs.
    None,
    /// One line with ellipsis truncation. Can shrink to zero width.
    Truncate,
}

/// Alignment along the line's inline axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Left,
    Right,
    Justify,
}

/// Font weight on the numeric scale used by CSS and OpenType.
pub type FontWeight = u16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
}

/// What a text engine needs to know about a text's style.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextStyle {
    pub font_size: f32,
    /// A family: a registered name, or `system-ui` / `ui-monospace` (resolved per engine).
    pub family: &'static str,
    /// Line height in pixels.
    pub line_height: f32,
    pub weight: FontWeight,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Additional space between glyphs in logical pixels.
    pub letter_spacing: f32,
    pub tabular_nums: bool,
    pub wrap: Wrap,
    pub align: TextAlign,
    pub max_lines: Option<u16>,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font_size: 16.0,
            family: "system-ui",
            line_height: 24.0,
            weight: 400,
            italic: false,
            underline: false,
            strike: false,
            letter_spacing: 0.0,
            tabular_nums: false,
            wrap: Wrap::Words,
            align: TextAlign::Start,
            max_lines: None,
        }
    }
}

/// An edit from a platform text input. Ranges count UTF-8 bytes in the displayed document,
/// including marked text. A missing replacement uses the marked range, then the selection.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextEdit {
    /// Replace a displayed range with uncommitted text. The start and end of `selection`
    /// encode the anchor and caret relative to `text`. No selection hides the input method's
    /// cursor. Empty text cancels the composition.
    Mark { text: String, selection: Option<Range<usize>>, replacement: Option<Range<usize>> },
    /// Replace a displayed range and accept the resulting document as one committed edit.
    Insert { text: String, replacement: Option<Range<usize>> },
    /// Commit the document and its directional selection as one native edit.
    Commit { text: String, selection: (usize, usize), kind: CommitKind },
    /// Accept the displayed document, removing its input-method marking.
    Unmark,
    /// Restore the committed document and the selection from before composition began.
    Cancel,
}

/// How a committed edit can continue the editor's current undo step.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitKind {
    Typing,
    Deleting,
    Other,
}

impl fmt::Debug for TextEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mark { selection, replacement, .. } => {
                f.debug_struct("Mark").field("selection", selection).field("replacement", replacement).finish_non_exhaustive()
            }
            Self::Insert { replacement, .. } => f.debug_struct("Insert").field("replacement", replacement).finish_non_exhaustive(),
            Self::Commit { selection, kind, .. } => {
                f.debug_struct("Commit").field("selection", selection).field("kind", kind).finish_non_exhaustive()
            }
            Self::Unmark => f.write_str("Unmark"),
            Self::Cancel => f.write_str("Cancel"),
        }
    }
}

/// An indivisible token in an editor's logical UTF-8 text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtomicRange {
    pub range: Range<usize>,
    /// Semantic tag such as "mention", "channel", or "command".
    pub kind: Arc<str>,
}

/// Overrides to the base text style for one logical UTF-8 range.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RichSpan {
    pub weight: Option<FontWeight>,
    pub italic: bool,
    /// Use the theme's monospace family.
    pub mono: bool,
    pub size: Option<f32>,
    pub color: Option<ColorValue>,
    pub background: Option<ColorValue>,
    /// Draw a one-pixel border around the span background.
    pub border_color: Option<ColorValue>,
    /// Corner radius of the span background in pixels.
    pub radius: Option<f32>,
    /// Additional inline space in pixels at both ends of the span.
    pub padding_x: Option<f32>,
    /// Additional space above and below the span's glyph band, limited by its line box.
    pub padding_y: Option<f32>,
    pub underline: bool,
    pub strike: bool,
}

/// Vertical alignment of an embedded object in a text line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InlineAlign {
    #[default]
    Baseline,
    CapHeightCenter,
    Middle,
    Bottom,
}

/// Visual content embedded in a text line.
#[derive(Clone, Debug, PartialEq)]
pub enum InlineContent {
    Spacer {
        width: f32,
    },
    Image {
        image: Image,
        fit: ObjectFit,
        radius: f32,
    },
    /// A viewport-managed image in a `text` view. The attachment size reserves its line space.
    ImageSource {
        source: ImageSource,
        fit: ObjectFit,
        radius: f32,
    },
    Icon {
        icon: NodeIcon,
        color: Option<ColorValue>,
    },
    Vector {
        ops: Arc<[CanvasOp]>,
    },
}

/// An object associated with a logical range in the containing text.
#[derive(Clone, Debug, PartialEq)]
pub struct InlineAttachment {
    /// An empty range inserts at the given byte. A nonempty range replaces the glyphs while
    /// retaining the source text for copying and accessibility.
    pub range: Range<usize>,
    pub size: Size,
    pub align: InlineAlign,
    /// The distance in logical pixels from the top of a baseline-aligned object
    /// to its baseline. `None` uses the bottom edge.
    pub baseline: Option<f32>,
    pub content: InlineContent,
}

impl InlineAttachment {
    /// The layout size, with a spacer contributing only horizontal advance.
    pub fn layout_size(&self) -> Size {
        let dimension = |value: f32| if value.is_finite() { value.max(0.0) } else { 0.0 };
        match self.content {
            InlineContent::Spacer { width } => Size::new(dimension(width), 0.0),
            _ => Size::new(dimension(self.size.width), dimension(self.size.height)),
        }
    }

    /// The baseline within the resolved object bounds, if specified.
    pub fn layout_baseline(&self) -> Option<f32> {
        self.baseline.filter(|offset| offset.is_finite()).map(|offset| offset.clamp(0.0, self.layout_size().height))
    }

    /// Checks UTF-8 boundaries and that no two attachments consume the same source bytes.
    pub fn valid_ranges(attachments: &[Self], text: &str) -> bool {
        let mut ranges: Vec<_> = attachments.iter().map(|attachment| &attachment.range).collect();
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        let mut end = 0;
        ranges.into_iter().all(|range| {
            let valid = range.start >= end && text.get(range.clone()).is_some();
            end = range.end;
            valid
        })
    }
}

impl RichSpan {
    /// Sets vertical padding in logical pixels.
    pub fn padding_y(mut self, padding: f32) -> Self {
        self.padding_y = Some(padding);
        self
    }

    /// Set the type style for this span.
    pub fn font_style(mut self, style: FontStyle) -> Self {
        self.italic = style == FontStyle::Italic;
        self
    }
}

/// An owned editing document for synchronous platform queries.
/// The committed value stays intact while composition edits its displayed draft.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
pub struct TextDocument {
    value: String,
    anchor: usize,
    caret: usize,
    composition: Option<Composition>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Composition {
    text: String,
    marked: Range<usize>,
    selection: Option<(usize, usize)>,
}

impl fmt::Debug for TextDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextDocument")
            .field("selection", &self.selection_endpoints())
            .field("marked", &self.marked())
            .finish_non_exhaustive()
    }
}

impl<'de> Deserialize<'de> for TextDocument {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Document {
            value: String,
            anchor: usize,
            caret: usize,
            composition: Option<Composition>,
        }

        let Document { value, anchor, caret, composition } = Document::deserialize(deserializer)?;
        let valid_selection = |text: &str, (a, b)| text.is_char_boundary(a) && text.is_char_boundary(b);
        let valid_composition = composition.as_ref().is_none_or(|c| {
            c.marked.start <= c.marked.end
                && valid_selection(&c.text, (c.marked.start, c.marked.end))
                && c.selection.is_none_or(|selection| valid_selection(&c.text, selection))
        });
        if !valid_selection(&value, (anchor, caret)) || !valid_composition {
            return Err(serde::de::Error::custom("invalid UTF-8 ranges in a text document"));
        }
        Ok(Self { value, anchor, caret, composition })
    }
}

impl TextDocument {
    /// A committed value with its UTF-8 byte selection, ordered as anchor then caret.
    pub fn new(value: String, anchor: usize, caret: usize) -> Self {
        let anchor = value.floor_char_boundary(anchor);
        let caret = value.floor_char_boundary(caret);
        Self { value, anchor, caret, composition: None }
    }

    /// The complete displayed text, including any marked draft.
    pub fn text(&self) -> &str {
        self.composition.as_ref().map_or(&self.value, |c| &c.text)
    }

    /// The displayed selection in ascending UTF-8 byte order.
    pub fn selection(&self) -> Option<Range<usize>> {
        self.selection_endpoints().map(|(anchor, caret)| anchor.min(caret)..anchor.max(caret))
    }

    /// The displayed anchor and caret in UTF-8 bytes, if the cursor is visible.
    pub fn selection_endpoints(&self) -> Option<(usize, usize)> {
        match &self.composition {
            Some(composition) => composition.selection,
            None => Some((self.anchor, self.caret)),
        }
    }

    /// Updates the directional selection in the displayed document.
    pub fn set_selection(&mut self, (anchor, caret): (usize, usize)) -> bool {
        let text = self.text();
        let selection = (text.floor_char_boundary(anchor), text.floor_char_boundary(caret));
        if self.selection_endpoints() == Some(selection) {
            return false;
        }
        if let Some(composition) = &mut self.composition {
            composition.selection = Some(selection);
        } else {
            (self.anchor, self.caret) = selection;
        }
        true
    }

    /// The input method's marked range in displayed UTF-8 bytes.
    pub fn marked(&self) -> Option<Range<usize>> {
        self.composition.as_ref().map(|c| c.marked.clone())
    }

    /// Installs an authoritative draft and its UTF-8 ranges for native text queries.
    pub fn sync_composition(&mut self, text: &str, marked: Range<usize>, selection: Option<(usize, usize)>) -> bool {
        if marked.is_empty()
            || !text.is_char_boundary(marked.start)
            || !text.is_char_boundary(marked.end)
            || selection.is_some_and(|(a, b)| !text.is_char_boundary(a) || !text.is_char_boundary(b))
        {
            return false;
        }
        self.composition = Some(Composition { text: text.to_owned(), marked, selection });
        true
    }

    /// The committed value and its anchor and caret in UTF-8 bytes.
    pub fn committed(&self) -> (&str, usize, usize) {
        (&self.value, self.anchor, self.caret)
    }

    /// Takes the committed value and its anchor and caret in UTF-8 bytes.
    pub fn into_committed(self) -> (String, usize, usize) {
        (self.value, self.anchor, self.caret)
    }

    /// Updates the displayed anchor and caret during an active composition.
    pub fn select_marked(&mut self, selection: Range<usize>) -> bool {
        let Some(composition) = &mut self.composition else { return false };
        composition.selection =
            Some((composition.text.floor_char_boundary(selection.start), composition.text.floor_char_boundary(selection.end)));
        true
    }

    /// Applies a platform edit to this document, clamping offsets to UTF-8 boundaries.
    /// Returns whether the value, selection or composition changed.
    pub fn apply(&mut self, edit: &TextEdit) -> bool {
        match edit {
            TextEdit::Cancel => self.composition.take().is_some(),
            TextEdit::Unmark => {
                let Some(composition) = self.composition.take() else { return false };
                let end = composition.marked.end;
                self.value = composition.text;
                (self.anchor, self.caret) = composition.selection.unwrap_or((end, end));
                true
            }
            TextEdit::Commit { text, selection: (anchor, caret), .. } => {
                let anchor = text.floor_char_boundary(*anchor);
                let caret = text.floor_char_boundary(*caret);
                self.commit(text, anchor, caret)
            }
            TextEdit::Mark { text, .. } if text.is_empty() => self.composition.take().is_some(),
            TextEdit::Mark { text, selection, replacement } => {
                let (display, marked) = self.with_replacement(text, replacement.as_ref());
                let selection = selection.as_ref().map(|range| {
                    (marked.start + text.floor_char_boundary(range.start), marked.start + text.floor_char_boundary(range.end))
                });
                let composition = Composition { text: display, marked, selection };
                if self.composition.as_ref() == Some(&composition) {
                    return false;
                }
                self.composition = Some(composition);
                true
            }
            TextEdit::Insert { text, replacement } => {
                let (display, inserted) = self.with_replacement(text, replacement.as_ref());
                self.commit(&display, inserted.end, inserted.end)
            }
        }
    }

    fn commit(&mut self, text: &str, anchor: usize, caret: usize) -> bool {
        let changed = self.composition.take().is_some() || self.value != text || self.anchor != anchor || self.caret != caret;
        if changed {
            if self.value != text {
                self.value.clear();
                self.value.push_str(text);
            }
            self.anchor = anchor;
            self.caret = caret;
        }
        changed
    }

    fn with_replacement(&self, text: &str, replacement: Option<&Range<usize>>) -> (String, Range<usize>) {
        let displayed = self.text();
        let replacement =
            replacement.cloned().or_else(|| self.marked()).or_else(|| self.selection()).unwrap_or(displayed.len()..displayed.len());
        let a = displayed.floor_char_boundary(replacement.start);
        let b = displayed.floor_char_boundary(replacement.end);
        let start = a.min(b);
        let end = a.max(b);
        (format!("{}{text}{}", &displayed[..start], &displayed[end..]), start..start + text.len())
    }
}

/// A layout engine's visual text geometry. Rectangles are relative to `origin`.
/// All source positions are UTF-8 byte offsets in the layout's displayed document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextGeometry {
    pub origin: Point,
    pub lines: Arc<[TextLine]>,
}

/// A visual line of the displayed document.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub range: Range<usize>,
    pub rect: Rect,
    /// Top of the typographic glyph band, relative to the text origin.
    pub glyph_top: f32,
    /// Height of the glyph band after clamping it to the line box.
    pub glyph_height: f32,
    /// Grapheme boxes in visual order, with their logical ranges.
    pub fragments: Arc<[TextFragment]>,
}

/// A visible grapheme's byte range and the direction of its advance.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFragment {
    pub range: Range<usize>,
    pub rect: Rect,
    pub rtl: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marking_and_unmarking_preserves_reverse_utf8_selection() {
        let mut document = TextDocument::new("a界b".into(), 4, 1);
        let edit = TextEdit::Mark { text: "語".into(), selection: Some(Range { start: 3, end: 1 }), replacement: None };
        assert!(document.apply(&edit));
        assert_eq!(document.committed(), ("a界b", 4, 1));
        assert_eq!(document.text(), "a語b");
        assert_eq!(document.selection_endpoints(), Some((4, 1)));
        assert_eq!(document.marked(), Some(1..4));
        assert!(!format!("{document:?} {edit:?}").contains("語"));
        assert!(document.apply(&TextEdit::Unmark));
        assert_eq!(document.committed(), ("a語b", 4, 1));
        assert_eq!(document.marked(), None);
    }

    #[test]
    fn a_published_draft_preserves_its_exact_text_and_committed_selection() {
        let mut document = TextDocument::new("A界Z".into(), 4, 1);
        assert!(document.sync_composition("你好Z", 3..6, Some((6, 0))));
        assert_eq!(document.text(), "你好Z");
        assert_eq!(document.selection_endpoints(), Some((6, 0)));
        assert!(!document.sync_composition("你好Z", 1..6, None));
        assert_eq!(document.marked(), Some(3..6));
        assert!(document.apply(&TextEdit::Cancel));
        assert_eq!(document.text(), "A界Z");
        assert_eq!(document.selection_endpoints(), Some((4, 1)));
    }
}
