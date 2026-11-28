//! Line breaking and positioned glyph extraction from shaped paragraphs.

use super::{Decoration, Document, PillPaint, Presentation, Run, Shape, SpanEdge, TextEngine, TextWork, inline};
use octara_model::geometry::{Point, Rect, Size};
use octara_model::paint::Glyph;
use octara_model::semantics::TextClip;
use octara_model::text::{TextAlign, TextFragment, TextLine, TextStyle, Wrap};
use parley::PositionedLayoutItem;
use smallvec::SmallVec;
use std::ops::Range;

fn line_width(metrics: &parley::LineMetrics) -> f32 {
    (metrics.advance - metrics.hanging_advance).max(0.0)
}

fn line_limit(style: TextStyle) -> usize {
    let limit = style.max_lines.map_or(usize::MAX, usize::from);
    if style.wrap == Wrap::Truncate { limit.min(1) } else { limit }
}

fn wrap_width(style: TextStyle, width: Option<f32>) -> Option<f32> {
    if style.wrap == Wrap::Words { width } else { None }
}

#[derive(Clone, Copy)]
struct MeasureProbe {
    width_bits: Option<u32>,
    max_lines: usize,
    size: Size,
    lines: usize,
}

#[derive(Default)]
pub(super) struct MeasureCache {
    probes: SmallVec<[MeasureProbe; 4]>,
    unwrapped: Option<(Size, usize)>,
}

impl MeasureCache {
    fn get(&mut self, width_bits: Option<u32>, max_lines: usize, work: &mut TextWork) -> Option<(Size, usize)> {
        let measured = if width_bits.is_none()
            && let Some((size, lines)) = self.unwrapped
            && lines <= max_lines
        {
            (size, lines)
        } else {
            let index = self.probes.iter().position(|probe| probe.width_bits == width_bits && probe.max_lines == max_lines)?;
            let probe = self.probes[index];
            self.probes[index..].rotate_left(1);
            (probe.size, probe.lines)
        };
        work.measure_probe_hits += 1;
        Some(measured)
    }

    fn store(&mut self, width_bits: Option<u32>, max_lines: usize, (size, lines): (Size, usize), complete: bool) {
        if width_bits.is_none() && complete {
            self.unwrapped = Some((size, lines));
        } else {
            if self.probes.len() == self.probes.inline_size() {
                self.probes.remove(0);
            }
            self.probes.push(MeasureProbe { width_bits, max_lines, size, lines });
        }
    }
}

impl Shape {
    fn measure(&mut self, width: Option<f32>, max_lines: usize, justify: bool, work: &mut TextWork) -> (Size, usize) {
        let width_bits = width.map(f32::to_bits);
        if let Some(measured) = self.measures.get(width_bits, max_lines, work) {
            return measured;
        }
        // Alignment changes positioned glyphs but leaves the measured line metrics intact.
        if self.broken.is_none_or(|(current, _)| current != width) {
            self.at(width, justify, work);
        }
        let mut size = Size::<f32>::default();
        for line in self.layout.lines().take(max_lines) {
            let metrics = line.metrics();
            size.width = size.width.max(line_width(metrics));
            size.height += metrics.line_height;
        }
        let lines = self.layout.len().min(max_lines);
        self.measures.store(width_bits, max_lines, (size, lines), lines == self.layout.len());
        (size, lines)
    }
}

impl Document {
    pub(super) fn measure(&self, width: Option<f32>, max_lines: usize, work: &mut TextWork) -> (Size, usize) {
        let breaks = wrap_width(self.style, width);
        let justify = self.style.align == TextAlign::Justify && breaks.is_some();
        let limit = line_limit(self.style).min(max_lines);
        if limit == 0 {
            return (Size::default(), 0);
        }
        let (mut size, lines) = if let [paragraph] = self.paragraphs.as_slice() {
            paragraph.shape.borrow_mut().measure(breaks, limit, justify, work)
        } else {
            self.measure_paragraphs(breaks, limit, justify, work)
        };
        if self.style.wrap == Wrap::Truncate
            && let Some(width) = width
        {
            size.width = size.width.min(width);
        }
        (size, lines)
    }

    fn measure_paragraphs(&self, width: Option<f32>, limit: usize, justify: bool, work: &mut TextWork) -> (Size, usize) {
        let mut measures = self.measures.borrow_mut();
        let cache = measures.get_or_insert_with(Box::default);
        let width_bits = width.map(f32::to_bits);
        if let Some(measured) = cache.get(width_bits, limit, work) {
            return measured;
        }

        let mut size = Size::<f32>::default();
        let mut lines = 0;
        for paragraph in &self.paragraphs {
            let (paragraph_size, count) = paragraph.shape.borrow_mut().measure(width, limit - lines, justify, work);
            size.width = size.width.max(paragraph_size.width);
            size.height += paragraph_size.height;
            lines += count;
            if lines == limit {
                break;
            }
        }
        cache.store(width_bits, limit, (size, lines), lines < limit);
        (size, lines)
    }
}

struct ClusterBox<'a> {
    fragment: TextFragment,
    inline: Option<&'a inline::Box>,
    object_rect: Option<Rect>,
    glyphs: usize,
    joined_to_next: bool,
    ligature_continuation: bool,
    space: bool,
}

impl TextEngine {
    pub(super) fn present(&mut self, document: &Document, width: Option<f32>) -> Presentation {
        let breaks = wrap_width(document.style, width);
        let justify = document.style.align == TextAlign::Justify && breaks.is_some();
        let limit = line_limit(document.style);
        let mut lines = Vec::new();
        let mut out = Presentation::default();
        for (index, paragraph) in document.paragraphs.iter().enumerate() {
            if lines.len() == limit {
                break;
            }
            let mut shape = paragraph.shape.borrow_mut();
            let shape = shape.at(breaks, justify, &mut self.work);
            let byte = |index| (paragraph.start + inline::Box::decoded(&shape.inline, index)).min(paragraph.end);
            let mut paragraph_height = 0.0;
            for (line_index, line) in shape.layout.lines().take(limit - lines.len()).enumerate() {
                let metrics = line.metrics();
                let remaining = line_index + 1 < shape.layout.len() || index + 1 < document.paragraphs.len();
                let hidden = lines.len() + 1 == limit && remaining;
                let width_overflow = document.style.wrap == Wrap::Truncate && width.is_some_and(|w| line_width(metrics) > w + 0.01);
                if hidden || width_overflow {
                    out.clip = if document.style.wrap == Wrap::Truncate {
                        Some(TextClip::Ellipsis)
                    } else {
                        document.style.max_lines.map(|limit| TextClip::LineClamp(u32::from(limit)))
                    };
                }
                let y = out.size.height + paragraph_height;
                let mut x = metrics.offset + metrics.inline_min_coord;
                let mut clusters = Vec::<ClusterBox>::new();
                let mut positioned = line.items().filter_map(|item| match item {
                    PositionedLayoutItem::InlineBox(item) => Some(item),
                    _ => None,
                });
                for cluster in line.runs().flat_map(|run| run.visual_clusters()) {
                    let advance = cluster.advance();
                    let range = cluster.text_range();
                    let inline = inline::Box::at(&shape.inline, range.start);
                    let object_rect = inline.and_then(|item| {
                        let positioned = positioned.next()?;
                        (!matches!(item.content, inline::Content::Edge { .. })).then(|| {
                            Rect::new(positioned.x, positioned.y + y - metrics.block_min_coord, positioned.width, positioned.height)
                        })
                    });
                    let source_range = byte(range.start)..byte(range.end);
                    if let Some(previous) = clusters.last_mut() {
                        let ligature = previous.fragment.rtl == cluster.is_rtl()
                            && if cluster.is_rtl() { previous.ligature_continuation } else { cluster.is_ligature_continuation() };
                        let same_span = |item: &inline::Box, range: &Range<usize>| {
                            let inline::Content::Edge { span, .. } = &item.content else { return false };
                            range.start < paragraph.start + span.end && paragraph.start + span.start < range.end
                        };
                        previous.joined_to_next = ligature
                            || inline.is_some_and(|item| same_span(item, &previous.fragment.range))
                            || previous.inline.is_some_and(|item| same_span(item, &source_range));
                    }
                    clusters.push(ClusterBox {
                        fragment: TextFragment {
                            range: source_range,
                            rect: Rect::new(x, y, advance, metrics.line_height),
                            rtl: cluster.is_rtl(),
                        },
                        inline,
                        object_rect,
                        glyphs: cluster.glyphs().count(),
                        joined_to_next: false,
                        ligature_continuation: cluster.is_ligature_continuation(),
                        space: cluster.is_space_or_nbsp(),
                    });
                    x += advance;
                }
                let rtl = shape.layout.is_rtl();
                let cut = hidden || width_overflow;
                let ellipsis = cut.then(|| self.shape("…", document.style.into(), &[], true));
                let mut ellipsis_shape = ellipsis.as_ref().map(|shape| shape.borrow_mut());
                let ellipsis_layout = ellipsis_shape.as_mut().map(|shape| &shape.at(None, false, &mut self.work).layout);
                let mut dots_width = ellipsis_layout.map_or(0.0, |layout| layout.width());
                if width.is_some_and(|width| dots_width > width) {
                    dots_width = 0.0;
                }
                let kept = if cut {
                    cut_clusters(&clusters, width.map_or(f32::MAX, |w| (w - dots_width).max(0.0)), rtl)
                } else {
                    0..clusters.len()
                };
                let glyph_start: usize = clusters[..kept.start].iter().map(|cluster| cluster.glyphs).sum();
                let glyph_end = glyph_start + clusters[kept.clone()].iter().map(|cluster| cluster.glyphs).sum::<usize>();
                let shown = &clusters[kept];
                let left = shown.first().map_or(x, |cluster| cluster.fragment.rect.x);
                let right = shown.last().map_or(left, |cluster| cluster.fragment.rect.right());
                let used_width = if cut { right - left + dots_width } else { line_width(metrics) };
                let align_offset = width.map_or(0.0, |width| {
                    let spare = (width - used_width).max(0.0);
                    match (document.style.align, rtl) {
                        (TextAlign::Center, _) => spare * 0.5,
                        (TextAlign::Right, _) | (TextAlign::Start | TextAlign::Justify, true) | (TextAlign::End, false) => spare,
                        _ => 0.0,
                    }
                });
                let shift = if cut {
                    align_offset - left + if rtl { dots_width } else { 0.0 }
                } else if justify {
                    0.0
                } else if rtl {
                    align_offset - metrics.offset
                } else {
                    align_offset
                };
                let mut byte_range = line.text_range();
                byte_range.start = byte(byte_range.start);
                byte_range.end = if line_index + 1 == shape.layout.len() { paragraph.next } else { byte(byte_range.end) };
                if cut {
                    let start = shown.iter().map(|cluster| cluster.fragment.range.start).min().unwrap_or(byte_range.start);
                    let end = shown.iter().map(|cluster| cluster.fragment.range.end).max().unwrap_or(start);
                    byte_range = start..end;
                }
                let fragments = shown
                    .iter()
                    .filter(|cluster| !cluster.fragment.range.is_empty() || cluster.object_rect.is_some())
                    .filter(|_| !document.placeholder)
                    .map(|cluster| {
                        let f = &cluster.fragment;
                        TextFragment {
                            range: document.byte(f.range.start)..document.byte(f.range.end),
                            rect: Rect { x: f.rect.x + shift, ..f.rect },
                            rtl: f.rtl,
                        }
                    })
                    .collect::<Vec<_>>();
                out.padding.extend(shown.iter().filter_map(|cluster| {
                    let item = cluster.inline?;
                    let inline::Content::Edge { brush, opening, .. } = item.content else { return None };
                    let anchor = paragraph.start + item.range.start - usize::from(!opening);
                    let rect = cluster.fragment.rect;
                    Some(SpanEdge { brush, anchor, rect: Rect { x: rect.x + shift, ..rect } })
                }));
                for cluster in shown {
                    let Some((item, rect)) = cluster.inline.zip(cluster.object_rect) else { continue };
                    let rect = Rect { x: rect.x + shift, ..rect };
                    match &item.content {
                        inline::Content::Attachment(index) => out.attachments.push((*index, rect)),
                        inline::Content::Pill { index, label } => {
                            let glyphs = label.as_ref().map_or_else(Vec::new, |label| self.pill_glyphs(label, rect));
                            out.pills.push(PillPaint { index: *index, rect, glyphs });
                        }
                        inline::Content::Edge { .. } => {}
                    }
                }
                let rect = Rect::new(align_offset, y, used_width, metrics.line_height);
                let glyph_top = (metrics.content_block_min_coord - metrics.block_min_coord).min(rect.h).max(0.0);
                let glyph_bottom = (metrics.content_block_max_coord - metrics.block_min_coord).min(rect.h).max(glyph_top);
                lines.push(TextLine {
                    range: document.byte(byte_range.start)..document.byte(byte_range.end),
                    rect,
                    glyph_top: y + glyph_top,
                    glyph_height: glyph_bottom - glyph_top,
                    fragments: fragments.into(),
                });
                let origin = Point::new(shift, y - metrics.block_min_coord);
                let mut visual_clusters = clusters.iter().peekable();
                let mut glyph_ends = std::iter::from_fn(|| {
                    let mut cluster = visual_clusters.next()?;
                    let mut end = cluster.fragment.range.end;
                    let mut count = cluster.glyphs;
                    while let Some(next) = visual_clusters.next_if(|next| {
                        cluster.fragment.rtl == next.fragment.rtl
                            && if cluster.fragment.rtl { cluster.ligature_continuation } else { next.ligature_continuation }
                    }) {
                        cluster = next;
                        count += next.glyphs;
                        end = end.max(next.fragment.range.end);
                    }
                    Some(std::iter::repeat_n(end, count))
                })
                .flatten();
                self.extract_runs(line, if cut { glyph_start..glyph_end } else { 0..usize::MAX }, origin, &mut glyph_ends, &mut out);
                if dots_width > 0.0
                    && let Some(ellipsis) = ellipsis_layout.and_then(|layout| layout.lines().next())
                {
                    let dots_x = if rtl { align_offset } else { align_offset + right - left };
                    let dots_y = y + metrics.baseline - metrics.block_min_coord - ellipsis.metrics().baseline;
                    let mut ends = std::iter::repeat(document.byte(byte_range.end));
                    self.extract_runs(ellipsis, 0..usize::MAX, Point::new(dots_x, dots_y), &mut ends, &mut out);
                }
                out.size.width = out.size.width.max(used_width);
                paragraph_height += metrics.line_height;
            }
            out.size.height += paragraph_height;
        }
        if limit == 0
            && document.paragraphs.iter().any(|paragraph| paragraph.start != paragraph.end || !paragraph.shape.borrow().inline.is_empty())
        {
            out.clip = Some(TextClip::LineClamp(0));
        }
        out.lines = lines.into();
        out
    }

    fn pill_glyphs(&mut self, label: &inline::PillLabel, rect: Rect) -> Vec<Run> {
        let shape = label.shape.borrow();
        let offset = Point::new(rect.x + label.origin.x, rect.y + label.origin.y);
        shape
            .layout
            .lines()
            .flat_map(|line| line.items())
            .filter_map(|item| {
                let PositionedLayoutItem::GlyphRun(run) = item else { return None };
                // Attachment labels render with their icon and background while document text reveals.
                let glyphs = run.positioned_glyphs().map(|glyph| Glyph { id: glyph.id, x: glyph.x, y: glyph.y, source_end: 0 }).collect();
                Some(self.glyph_run(&run, glyphs, offset))
            })
            .collect()
    }

    fn glyph_run(&mut self, run: &parley::GlyphRun<'_, u16>, glyphs: Vec<Glyph>, offset: Point) -> Run {
        let shaped = run.run();
        let synthesis = shaped.synthesis();
        Run {
            font: self.face(shaped.font()),
            size: shaped.font_size(),
            coords: shaped.normalized_coords().iter().map(|coord| coord.to_bits()).collect(),
            embolden: synthesis.embolden(),
            skew: synthesis.skew().unwrap_or_default(),
            brush: run.style().brush,
            glyphs: glyphs.into(),
            offset,
        }
    }

    fn extract_runs(
        &mut self,
        line: parley::Line<'_, u16>,
        keep: Range<usize>,
        offset: Point,
        glyph_ends: &mut impl Iterator<Item = usize>,
        out: &mut Presentation,
    ) {
        let mut start = 0;
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(run) = item else { continue };
            let glyphs = run.positioned_glyphs();
            let count = glyphs.clone().count();
            let end = start + count;
            let ends = glyph_ends.by_ref().take(count).chain(std::iter::repeat(0)).take(count);
            if start < keep.end && end > keep.start {
                let skip = keep.start.saturating_sub(start);
                let take = keep.end.min(end) - keep.start.max(start);
                let glyphs: Vec<_> = glyphs
                    .zip(ends)
                    .skip(skip)
                    .take(take)
                    .map(|(glyph, source_end)| Glyph { id: glyph.id, x: glyph.x, y: glyph.y, source_end })
                    .collect();
                if let Some(remaining) = (count - skip - take).checked_sub(1) {
                    glyph_ends.nth(remaining);
                }
                let style = run.style();
                out.glyphs.push(self.glyph_run(&run, glyphs, offset));
                if style.underline.is_some() || style.strikethrough.is_some() {
                    let mut glyphs = run.glyphs();
                    let leading = glyphs.by_ref().take(skip).map(|glyph| glyph.advance).sum::<f32>();
                    let width = if take == count { run.advance() } else { glyphs.take(take).map(|glyph| glyph.advance).sum() };
                    let x = run.offset() + offset.x + leading;
                    let baseline = run.baseline() + offset.y;
                    let metrics = run.run().font_metrics();
                    for (decoration, position, thickness) in [
                        (&style.underline, metrics.underline_offset, metrics.underline_size),
                        (&style.strikethrough, metrics.strikethrough_offset, metrics.strikethrough_size),
                    ] {
                        if let Some(decoration) = decoration
                            && width > 0.0
                        {
                            let y = baseline - decoration.offset.unwrap_or(position);
                            let height = decoration.size.unwrap_or(thickness).max(1.0);
                            out.decorations.push(Decoration { brush: decoration.brush, rect: Rect::new(x, y, width, height) });
                        }
                    }
                }
            } else if let Some(last) = count.checked_sub(1) {
                glyph_ends.nth(last);
            }
            start = end;
        }
    }
}

fn cut_clusters(clusters: &[ClusterBox<'_>], width: f32, rtl: bool) -> Range<usize> {
    let Some(first) = clusters.first() else { return 0..0 };
    let last = clusters.last().expect("nonempty clusters");
    if rtl {
        let mut start = clusters.len();
        while start > 0 {
            let mut next = start - 1;
            while next > 0 && clusters[next - 1].joined_to_next {
                next -= 1;
            }
            if last.fragment.rect.right() - clusters[next].fragment.rect.x > width + 0.01 {
                break;
            }
            start = next;
        }
        while start < clusters.len() && clusters[start].space {
            start += 1;
        }
        start..clusters.len()
    } else {
        let mut end = 0;
        while end < clusters.len() {
            let mut next = end + 1;
            while next < clusters.len() && clusters[next - 1].joined_to_next {
                next += 1;
            }
            if clusters[next - 1].fragment.rect.right() - first.fragment.rect.x > width + 0.01 {
                break;
            }
            end = next;
        }
        while end > 0 && clusters[end - 1].space {
            end -= 1;
        }
        0..end
    }
}
