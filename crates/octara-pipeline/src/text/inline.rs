//! Inline boxes reserve space for attachments and span edges while retaining source offsets.

use super::{ShapeStyle, SharedShape, Span, TextEngine, TextWork};
use octara_model::geometry::{Point, Size};
use octara_model::text::{InlineAlign, InlineAttachment, InlineContent, InlinePill};
use parley::{InlineBox, InlineBoxKind, Layout, RangedBuilder, StyleProperty, TextWrapMode, VerticalAlign};
use std::borrow::Cow;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

const OBJECT: char = '\u{fffc}';
const OBJECT_BYTES: usize = OBJECT.len_utf8();

pub(super) enum Content {
    Edge { brush: u16, span: Range<usize>, opening: bool },
    Attachment(usize),
    Pill { index: usize, label: Option<PillLabel> },
}

pub(super) struct PillLabel {
    pub(super) shape: SharedShape,
    pub(super) origin: Point,
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
        (offset, first): (usize, usize),
        engine: &mut TextEngine,
        style: ShapeStyle,
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
        boxes.extend(attachments.iter().enumerate().map(|(index, attachment)| {
            let index = first + index;
            let range = attachment.range.start - offset..attachment.range.end - offset;
            let (content, size, align, baseline) = if let InlineContent::Pill(pill) = &attachment.content {
                let mut inherited = style;
                for span in spans.iter().filter(|span| span.range.contains(&range.start)) {
                    inherited.family = span.family.unwrap_or(inherited.family);
                    inherited.italic |= span.italic;
                }
                let (size, baseline, label) = engine.measure_pill(pill, inherited);
                (Content::Pill { index, label }, size, InlineAlign::Baseline, Some(baseline))
            } else {
                (Content::Attachment(index), attachment.layout_size(), attachment.align, attachment.layout_baseline())
            };
            Self { content, range, index: 0, size, align, baseline }
        }));
        if boxes.is_empty() {
            return (Cow::Borrowed(text), boxes);
        }
        boxes.sort_by_key(|item| {
            let order = match item.content {
                Content::Edge { brush, opening, .. } => (u8::from(opening), brush),
                Content::Attachment(_) | Content::Pill { .. } => (2, 0),
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

    pub(super) fn align(items: &[Self], layout: &mut Layout<u16>, font_size: f32, work: &mut TextWork) {
        if !items.iter().any(|item| item.align == InlineAlign::CapHeightCenter) {
            return;
        }
        layout.break_all_lines(None);
        work.line_breaks += 1;
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

impl TextEngine {
    fn measure_pill(&mut self, pill: &InlinePill, inherited: ShapeStyle) -> (Size, f32, Option<PillLabel>) {
        let style = pill.style.normalized();
        let icon = if pill.icon.is_some() { style.size } else { 0.0 };
        let label = pill.layout_label();
        if label.is_empty() {
            let size = Size::new(icon + 2.0 * style.padding_x, icon + 2.0 * style.padding_y);
            let baseline = if pill.icon.is_some() { style.padding_y + icon } else { size.height };
            return (size, baseline, None);
        }

        let gap = if pill.icon.is_some() { style.gap } else { 0.0 };
        let shape = self.shape(
            &label,
            ShapeStyle {
                font_size: style.size * inherited.text_scale,
                weight: style.weight,
                line_height: None,
                underline: false,
                strike: false,
                ..inherited
            },
            &[],
            true,
        );
        let (width, height, x, y, baseline) = {
            let mut shaped = shape.borrow_mut();
            let layout = &shaped.at(None, false, &mut self.work).layout;
            let line = layout.get(0).expect("nonempty pill label");
            let metrics = line.metrics();
            (metrics.advance, metrics.line_height, metrics.offset + metrics.inline_min_coord, metrics.block_min_coord, metrics.baseline)
        };
        let body_height = icon.max(height);
        let top = style.padding_y + (body_height - height) * 0.5;
        let size = Size::new(2.0 * style.padding_x + icon + gap + width, 2.0 * style.padding_y + body_height);
        let origin = Point::new(style.padding_x + icon + gap - x, top - y);
        (size, origin.y + baseline, Some(PillLabel { shape, origin }))
    }
}
