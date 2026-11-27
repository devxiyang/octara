//! Rich text and incremental text-buffer bindings.

use super::prop::{IntoProp, Prop, static_prop};
use octara_model::{
    image::ImageSource,
    text::{InlineAttachment, InlineContent, RichSpan},
};
use octara_runtime::{
    MountCx, TextBuffer,
    graph::{delta::Changes, text_buf::TextEdit},
    media::MediaResolver,
};
use octara_tree::{NodeChange, NodeRef, tables::LinkSlot, transaction::RichText};
use std::sync::Arc;

pub use octara_model::text::RichSpan as Span;

/// A string with styled UTF-8 ranges and links.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rich {
    text: String,
    spans: Vec<(std::ops::Range<usize>, RichSpan)>,
    links: Vec<LinkSlot>,
    attachments: Vec<InlineAttachment>,
}

pub fn rich() -> Rich {
    Rich::default()
}

impl Rich {
    pub fn text(mut self, text: impl AsRef<str>) -> Self {
        self.text.push_str(text.as_ref());
        self
    }

    pub fn span(mut self, text: impl AsRef<str>, style: Span) -> Self {
        let start = self.text.len();
        self.text.push_str(text.as_ref());
        if self.text.len() > start {
            self.spans.push((start..self.text.len(), style));
        }
        self
    }

    pub fn link(mut self, label: impl AsRef<str>, href: impl Into<Arc<str>>) -> Self {
        let start = self.text.len();
        self.text.push_str(label.as_ref());
        let end = self.text.len();
        self.link_range(start..end, href)
    }

    /// Adds a link with the specified range style.
    pub fn link_span(self, label: impl AsRef<str>, href: impl Into<Arc<str>>, style: Span) -> Self {
        let start = self.text.len();
        let rich = self.span(label, style);
        let end = rich.text.len();
        rich.link_range(start..end, href)
    }

    /// Adds one link over previously appended text, including any existing style runs.
    ///
    /// Panics if the range is out of bounds or splits a UTF-8 character.
    pub fn link_range(mut self, range: std::ops::Range<usize>, href: impl Into<Arc<str>>) -> Self {
        assert!(self.text.get(range.clone()).is_some(), "link range must be valid UTF-8");
        if !range.is_empty() {
            self.links.push(LinkSlot { range, href: href.into() });
        }
        self
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Adds an inline object over a logical UTF-8 range in the complete text.
    pub fn attachment(mut self, attachment: InlineAttachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    pub(crate) fn change(mut self, resolver: Option<&dyn MediaResolver>) -> NodeChange {
        for attachment in &mut self.attachments {
            if let InlineContent::ImageSource { source, .. } = &mut attachment.content {
                *source = resolve_image_source(source.clone(), resolver);
            }
        }
        if self.spans.is_empty() && self.links.is_empty() && self.attachments.is_empty() {
            NodeChange::Text(self.text.into())
        } else {
            NodeChange::RichText(Box::new(RichText {
                text: self.text.into(),
                runs: self.spans,
                links: self.links,
                attachments: self.attachments,
            }))
        }
    }
}

pub(super) fn resolve_image_source(source: ImageSource, resolver: Option<&dyn MediaResolver>) -> ImageSource {
    let source = match (&source, resolver) {
        (ImageSource::Uri(uri), Some(resolver)) => resolver.resolve(uri).unwrap_or(source),
        _ => source,
    };
    source.normalize_cache_policy()
}

impl From<&str> for Rich {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

impl From<String> for Rich {
    fn from(text: String) -> Self {
        Self { text, ..Self::default() }
    }
}

impl From<&String> for Rich {
    fn from(text: &String) -> Self {
        Self::from(text.as_str())
    }
}

impl From<Arc<str>> for Rich {
    fn from(text: Arc<str>) -> Self {
        Self::from(text.as_ref())
    }
}

static_prop! { Rich => [Rich, &str, &String, String, Arc<str>] }

#[doc(hidden)]
pub enum TextContent {
    Rich(Prop<Rich>),
    Buffer(TextBuffer),
}

/// Text accepts a fixed or tracked string, rich text, or a delta-backed buffer.
pub trait IntoText {
    fn into_text(self) -> TextContent;
}

impl<T: IntoProp<Rich>> IntoText for T {
    fn into_text(self) -> TextContent {
        TextContent::Rich(self.into_prop())
    }
}

impl IntoText for TextBuffer {
    fn into_text(self) -> TextContent {
        TextContent::Buffer(self)
    }
}

pub(crate) fn mount_buffer(cx: &mut MountCx<'_>, node: NodeRef, buffer: TextBuffer) {
    let mut cursor = None;
    let mut len = 0;
    cx.bind_changes(node, move |r| {
        if let Some(cursor) = cursor.as_mut()
            && let Changes::Delta(edits) = r.text_changes(buffer, cursor)
        {
            return edits
                .into_iter()
                .map(|edit| {
                    let (range, insert) = match edit {
                        TextEdit::Append(insert) => (len..len, insert),
                        TextEdit::Splice { range, insert } => (range, insert),
                    };
                    len = len - range.len() + insert.len();
                    NodeChange::SpliceText { range, insert: insert.into() }
                })
                .collect();
        }
        let value = r.text(buffer);
        cursor = Some(r.text_cursor(buffer));
        len = value.len();
        vec![NodeChange::Text(value.into())]
    });
}
