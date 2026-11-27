//! Incremental Markdown boundaries and the streaming preview pipeline.

use crate::ast::{Align, Block, Inline, Kind, ParseOptions, Parsed, Scope, line_start, parse, parse_preview};
use memchr::{memmem, memrchr2};
use std::ops::Range;
use std::sync::Arc;

mod inline;
mod pacer;

pub use inline::InlineRepair;
#[cfg(test)]
pub(crate) use inline::close_tail;
pub use pacer::{StreamingEffects, StreamingPacer};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamingPolicy {
    /// Wait for a complete construct.
    Hidden,
    /// Show the portion that can already be rendered.
    Progressive,
}

/// Controls how unfinished block constructs appear in a live document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamingFilter {
    pub tables: StreamingPolicy,
    pub code_blocks: StreamingPolicy,
    pub media: StreamingPolicy,
    pub math: StreamingPolicy,
    pub directives: StreamingPolicy,
}

impl Default for StreamingFilter {
    fn default() -> Self {
        Self {
            tables: StreamingPolicy::Progressive,
            code_blocks: StreamingPolicy::Progressive,
            media: StreamingPolicy::Hidden,
            math: StreamingPolicy::Progressive,
            directives: StreamingPolicy::Progressive,
        }
    }
}

/// Byte offset where reparsing can resume from the last known top-level block.
///
/// Reference definitions are checked separately with [`may_define_link_reference`].
pub fn finished_blocks_end(src: &str, blocks: &[Arc<Block>], starts: &[usize]) -> usize {
    let Some(newline) = memrchr2(b'\n', b'\r', src.as_bytes()) else { return 0 };
    if starts.len() != blocks.len() {
        return 0;
    }
    let mut end = starts.partition_point(|&start| start <= newline);
    while end > 1 {
        let i = end - 1;
        let start = starts[i];
        // Hidden or synthetic blocks can overlap a candidate seam. Keep the seam before both.
        if blocks[i - 1].source.end <= start {
            return start;
        }
        end -= 1;
    }
    0
}

/// Detects the required `]:` bytes conservatively, even when their context is unfinished.
/// Pass a slice beginning at least one byte before an append when that byte is `]`.
pub fn may_define_link_reference(src: &str) -> bool {
    memmem::find(src.as_bytes(), b"]:").is_some()
}

/// Parses only the source suffix supplied by the store. All reported offsets are absolute.
pub(crate) fn parse_stream(src: &str, base: usize, options: ParseOptions, filter: StreamingFilter) -> Parsed {
    let mut out = parse(src, 0, options);
    if !out.opaque_tail {
        let table = options.tables.then(|| table_tail(src, &mut out)).flatten();
        let end = table.as_ref().map_or(src.len(), |range| range.start);
        let header = table
            .filter(|range| filter.tables == StreamingPolicy::Progressive && starts_block(src, range.start))
            .and_then(|range| pending_header(&src[range.clone()], options).map(|head| (range, head)));
        out = repair(src, out, end, options, filter.media);
        if let Some((range, head)) = header {
            out.starts.push(range.start);
            out.blocks.push(Block { kind: Kind::Table { align: vec![Align::None; head.len()], head, rows: Vec::new() }, source: range });
        }
    }
    filter_blocks(&mut out.blocks, Some(&mut out.starts), src, filter);
    if base > 0 {
        out.shift(base);
    }
    out
}

fn repair(src: &str, raw: Parsed, end: usize, options: ParseOptions, media: StreamingPolicy) -> Parsed {
    let fallback = |raw| {
        if end == src.len() { raw } else { parse(&src[..end], 0, options) }
    };
    let Some((start, scope)) = raw.tail.filter(|(start, _)| *start < end) else {
        return fallback(raw);
    };
    let mut atomic: Vec<_> = raw
        .math
        .iter()
        .chain(&raw.links)
        .filter(|range| range.start >= start && range.end <= end)
        .map(|range| range.start - start..range.end - start)
        .collect();
    atomic.sort_unstable_by_key(|range| (range.start, std::cmp::Reverse(range.end)));
    let Some(patch) = inline::plan(&src[start..end], scope, options, media, atomic.into_iter()) else {
        return fallback(raw);
    };
    if patch.cut == end - start && patch.closers.is_empty() {
        return fallback(raw);
    }
    let kept = src[..start + patch.cut].trim_end();
    let preview_image = patch.preview_image.map(|at| start + at);
    let plain = || parse(preview_image.map_or(kept, |at| &kept[..at]).trim_end(), 0, options);
    if patch.closers.is_empty() {
        return plain();
    }
    let limit = kept.len();
    let preview = format!("{kept}{}", patch.closers);
    let mut fixed = parse_preview(&preview, options, preview_image, limit);
    let mut markers: Vec<Range<usize>> = patch.markers.iter().map(|m| start + m.start..start + m.end).collect();
    markers.push(limit..preview.len());
    let shows_marker = fixed.shown.iter().any(|shown| {
        markers.iter().any(|marker| {
            let start = shown.start.max(marker.start);
            let end = shown.end.min(marker.end);
            start < end && !fixed.spoiler_markers.iter().any(|hidden| hidden.start <= start && end <= hidden.end)
        })
    });
    if shows_marker {
        return plain();
    }
    fixed.clamp_to_source(limit);
    fixed
}

fn filter_blocks(blocks: &mut Vec<Block>, mut starts: Option<&mut Vec<usize>>, src: &str, filter: StreamingFilter) -> bool {
    let mut index = 0;
    let mut original_index = 0;
    let original_count = blocks.len();
    blocks.retain_mut(|block| {
        let keep = match &mut block.kind {
            Kind::Code { is_complete, .. } => *is_complete || filter.code_blocks == StreamingPolicy::Progressive,
            Kind::Math { is_complete, .. } => *is_complete || filter.math == StreamingPolicy::Progressive,
            Kind::Directive { is_complete, .. } => *is_complete || filter.directives == StreamingPolicy::Progressive,
            Kind::Quote { blocks, .. } => filter_blocks(blocks, None, src, filter) || !blocks.is_empty(),
            Kind::List { items, .. } => {
                let original_count = items.len();
                items.retain_mut(|item| {
                    filter_blocks(&mut item.blocks, None, src, filter) || !item.blocks.is_empty() || item.task.is_some()
                });
                original_count == 0 || !items.is_empty()
            }
            Kind::Table { rows, .. } if block.source.end >= src.len() && !src.ends_with(['\r', '\n']) => {
                if rows.pop().is_some() {
                    block.source.end = line_start(src, src.len());
                    true
                } else {
                    filter.tables == StreamingPolicy::Progressive
                }
            }
            _ => true,
        };
        if keep {
            if let Some(starts) = starts.as_mut() {
                starts[index] = starts[original_index];
            }
            index += 1;
        }
        original_index += 1;
        keep
    });
    if let Some(starts) = starts {
        starts.truncate(index);
    }
    blocks.len() == original_count
}

fn starts_block(src: &str, start: usize) -> bool {
    let before = src[..start].strip_suffix("\r\n").or_else(|| src[..start].strip_suffix(['\r', '\n'])).unwrap_or("");
    before.rsplit(['\r', '\n']).next().is_none_or(|line| line.trim().is_empty())
}

fn table_tail(src: &str, out: &mut Parsed) -> Option<Range<usize>> {
    let end = src.len();
    let last = src.strip_suffix("\r\n").or_else(|| src.strip_suffix(['\r', '\n']));
    let line_end = last.map_or(end, str::len);
    let start = line_start(src, line_end);
    if out.tail.is_some_and(|(_, scope)| scope == Scope::Cell)
        || out.blocks.last().is_some_and(|block| matches!(block.kind, Kind::Table { .. }))
    {
        // Table cells and rows already have a CommonMark interpretation.
        out.tail = None;
        return None;
    }
    let line = &src[start..line_end];
    let piped = |line: &str| line.trim_start().starts_with('|') && !line.trim_start().starts_with("||");
    if !piped(line) || out.math.iter().any(|range| range.contains(&start)) {
        return None;
    }
    let before = src[..start].strip_suffix("\r\n").or_else(|| src[..start].strip_suffix(['\r', '\n']));
    let preceding = before.map(|before| (line_start(before, before.len()), before)).filter(|&(start, before)| piped(&before[start..]));
    if last.is_some() && preceding.is_some() {
        return None;
    }
    if delimiter(line)
        && let Some((preceding, _)) = preceding
    {
        return Some(preceding..end);
    }
    Some(start..end)
}

fn delimiter(line: &str) -> bool {
    line.bytes().all(|b| matches!(b, b'|' | b':' | b'-' | b' ' | b'\t'))
}

fn pending_header(source: &str, options: ParseOptions) -> Option<Vec<Inline>> {
    let mut lines = source.lines();
    let line = lines.next()?;
    if lines.next().is_some_and(|line| !delimiter(line)) || lines.next().is_some() {
        return None;
    }
    let line = line.trim().strip_prefix('|')?;
    let line = line.strip_suffix('|').unwrap_or(line);
    let mut columns = 1;
    let mut escaped = false;
    for b in line.bytes() {
        match b {
            b'\\' => escaped = !escaped,
            b'|' if !escaped => columns += 1,
            _ => escaped = false,
        }
    }
    if columns < 2 {
        return None;
    }
    let mut display = String::with_capacity(source.len() + columns * 4);
    display.push_str(source.lines().next()?);
    display.push_str("\n|");
    for _ in 0..columns {
        display.push_str("---|");
    }
    let Kind::Table { head, .. } = parse(&display, 0, options).blocks.pop()?.kind else { return None };
    Some(head)
}
