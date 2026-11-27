//! Inline boxes reserve space for attachments and span edges while retaining source offsets.

use super::Span;
use octara_model::geometry::Size;
use octara_model::text::{InlineAlign, InlineAttachment};
use parley::{InlineBox, InlineBoxKind, Layout, RangedBuilder, StyleProperty, TextWrapMode, VerticalAlign};
use std::borrow::Cow;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

const OBJECT: char = '\u{fffc}';
const OBJECT_BYTES: usize = OBJECT.len_utf8();

pub(super) enum Content {
    Edge { brush: u16, span: Range<usize>, opening: bool },
    Attachment(usize),
}

pub(super) struct Box {
    pub(super) content: Content,
    pub(super) range: Range<usize>,
    index: usize,
    size: Size,
    align: InlineAlign,
    baseline: Option<f32>,
}

impl Box {
    pub(super) fn project<'a>(
        text: &'a str,
        spans: &[Span],
        attachments: &[InlineAttachment],
        offset: usize,
        first: usize,
    ) -> (Cow<'a, str>, Vec<Self>) {
        let mut boxes = Vec::new();
        for span in spans.iter().filter(|span| span.inset > 0.0 && !span.range.is_empty()) {
            for (byte, opening) in [(span.range.start, true), (span.range.end, false)] {
                if attachments.iter().any(|item| item.range.start < byte + offset && byte + offset < item.range.end) {
                    continue;
                }
                boxes.push(Self {
                    content: Content::Edge { brush: span.brush, span: span.range.clone(), opening },
                    range: byte..byte,
                    index: 0,
                    size: Size::new(span.inset, 0.0),
                    align: InlineAlign::Baseline,
                    baseline: None,
                });
            }
        }
        boxes.extend(attachments.iter().enumerate().map(|(index, attachment)| Self {
            content: Content::Attachment(first + index),
            range: attachment.range.start - offset..attachment.range.end - offset,
            index: 0,
            size: attachment.layout_size(),
            align: attachment.align,
            baseline: attachment.layout_baseline(),
        }));
        if boxes.is_empty() {
            return (Cow::Borrowed(text), boxes);
        }
        boxes.sort_by_key(|item| {
            let order = match item.content {
                Content::Edge { brush, opening, .. } => (u8::from(opening), brush),
                Content::Attachment(_) => (2, 0),
            };
            (item.range.start, item.range.end, order)
        });
        let replaced = attachments.iter().map(|attachment| attachment.range.len()).sum::<usize>();
        let mut shaped = String::with_capacity(text.len() - replaced + OBJECT_BYTES * boxes.len());
        let mut byte = 0;
        for item in &mut boxes {
            shaped.push_str(&text[byte..item.range.start]);
            item.index = shaped.len();
            shaped.push(OBJECT);
            byte = item.range.end;
        }
        shaped.push_str(&text[byte..]);
        (Cow::Owned(shaped), boxes)
    }

    pub(super) fn push(items: &[Self], text: &str, builder: &mut RangedBuilder<'_, u16>) {
        for (id, item) in items.iter().enumerate() {
            let middle = Some(item.size.height * 0.5);
            let (vertical_align, baseline) = match item.align {
                InlineAlign::Baseline => (VerticalAlign::BASELINE, item.baseline),
                InlineAlign::CapHeightCenter => (VerticalAlign::BASELINE, middle),
                InlineAlign::Middle => (VerticalAlign::MIDDLE, middle),
                InlineAlign::Bottom => (VerticalAlign::BOTTOM, None),
            };
            builder.push_inline_box(InlineBox {
                id: id as u64,
                kind: InlineBoxKind::InFlow,
                index: item.index,
                width: item.size.width,
                height: item.size.height,
                baseline,
                vertical_align,
            });
            if let Content::Edge { opening, .. } = item.content {
                // Parley uses the previous atom's wrap mode at a potential break.
                let attached = if opening {
                    item.index..item.index + OBJECT_BYTES
                } else {
                    let (start, _) = text[..item.range.start].grapheme_indices(true).next_back().expect("nonempty span");
                    Self::encoded(items, start, true)..item.index
                };
                builder.push(StyleProperty::TextWrapMode(TextWrapMode::NoWrap), attached);
            }
        }
    }

    pub(super) fn align(items: &[Self], layout: &mut Layout<u16>, font_size: f32) {
        if !items.iter().any(|item| item.align == InlineAlign::CapHeightCenter) {
            return;
        }
        layout.break_all_lines(None);
        let alignments: Vec<_> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.align == InlineAlign::CapHeightCenter)
            .map(|(index, item)| {
                let cap = layout
                    .lines()
                    .flat_map(|line| line.runs())
                    .find(|run| run.text_range().contains(&item.index))
                    .and_then(|run| run.font_metrics().cap_height)
                    .unwrap_or(font_size * 0.7);
                (index, VerticalAlign::length(cap * 0.5))
            })
            .collect();
        for (index, alignment) in alignments {
            layout.edit_inline_box(index, |item| item.vertical_align = alignment);
        }
    }

    pub(super) fn encoded(items: &[Self], byte: usize, start: bool) -> usize {
        let count = items.partition_point(|item| item.range.start < byte || (start && item.range.is_empty() && item.range.start == byte));
        count.checked_sub(1).map_or(byte, |i| {
            let item = &items[i];
            if byte < item.range.end && start { item.index } else { item.index + OBJECT_BYTES + byte.saturating_sub(item.range.end) }
        })
    }

    pub(super) fn decoded(items: &[Self], index: usize) -> usize {
        let count = items.partition_point(|item| item.index <= index);
        count.checked_sub(1).map_or(index, |i| {
            let item = &items[i];
            let end = item.index + OBJECT_BYTES;
            if index < end { item.range.start } else { item.range.end + index - end }
        })
    }

    pub(super) fn at(items: &[Self], index: usize) -> Option<&Self> {
        items.binary_search_by_key(&index, |item| item.index).ok().map(|i| &items[i])
    }
}
