use super::{Inline, Style};
use memchr::memchr_iter;
use std::ops::Range;

/// Use raw text events to distinguish delimiters from code spans, HTML and entity escapes.
pub(super) fn record_pipes(src: &str, text: &str, source: &Range<usize>, at: usize, pipes: &mut Vec<(usize, usize)>) {
    if src.get(source.clone()) != Some(text) {
        return;
    }
    for offset in memchr_iter(b'|', text.as_bytes()) {
        let raw = source.start + offset;
        let escapes = src.as_bytes()[..raw].iter().rev().take_while(|&&byte| byte == b'\\').count();
        if escapes % 2 == 0 {
            pipes.push((raw, at + offset));
        }
    }
}

pub(super) fn apply(inline: &mut Inline, pipes: &[(usize, usize)], raw_markers: &mut Vec<Range<usize>>) -> usize {
    let mut opener = None;
    let mut markers = Vec::new();
    let mut content = Vec::new();
    let mut remaining = pipes;
    while let Some(&(source, local)) = remaining.first() {
        let run = remaining.iter().enumerate().take_while(|&(i, &(raw, text))| raw == source + i && text == local + i).count();
        remaining = &remaining[run..];
        if run != 2 {
            continue;
        }
        match opener {
            Some((raw_open, text_open)) if !inline.text[text_open + 2..local].trim().is_empty() => {
                raw_markers.extend([raw_open..raw_open + 2, source..source + 2]);
                markers.extend([text_open, local]);
                content.push(text_open + 2..local);
                opener = None;
            }
            _ => opener = Some((source, local)),
        }
    }
    if markers.is_empty() {
        return 0;
    }
    let adjusted = |offset: usize| {
        let before = markers.partition_point(|&marker| marker + 2 <= offset);
        let inside = markers.get(before).map_or(0, |&marker| offset.saturating_sub(marker));
        offset - before * 2 - inside
    };
    let mut text = String::with_capacity(inline.text.len() - markers.len() * 2);
    let mut from = 0;
    for &marker in &markers {
        text.push_str(&inline.text[from..marker]);
        from = marker + 2;
    }
    text.push_str(&inline.text[from..]);
    inline.text = text;
    for (range, _) in &mut inline.styles {
        *range = adjusted(range.start)..adjusted(range.end);
    }
    inline.styles.retain(|(range, style)| !range.is_empty() || matches!(style, Style::Image { .. }));
    inline.styles.extend(content.into_iter().map(|range| (adjusted(range.start)..adjusted(range.end), Style::Spoiler)));
    markers.len() / 2
}
