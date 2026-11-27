//! Visual text geometry with UTF-8 source offsets.

use octara_model::geometry::{Point, Rect};
use octara_model::text::{TextFragment, TextGeometry, TextLine};
use std::ops::Range;

pub(crate) fn hit_byte(geometry: &TextGeometry, at: Point) -> Option<usize> {
    hit_text(geometry, at).map(|(byte, _)| byte)
}

pub(crate) fn hit_text(geometry: &TextGeometry, at: Point) -> Option<(usize, usize)> {
    let x = at.x - geometry.origin.x;
    let y = at.y - geometry.origin.y;
    let line = geometry.lines.iter().find(|line| y < line.rect.bottom()).or_else(|| geometry.lines.last())?;
    let cluster = line.fragments.iter().find(|fragment| x <= fragment.rect.right()).or_else(|| line.fragments.last());
    Some((line_byte(line, x), cluster.map_or(line.range.start, |fragment| fragment.range.start)))
}

fn line_byte(line: &TextLine, x: f32) -> usize {
    let Some(first) = line.fragments.first() else { return line.range.start };
    if x <= first.rect.x {
        return edges(first).0;
    }
    for fragment in &*line.fragments {
        let (left, right) = edges(fragment);
        if x < fragment.rect.x + fragment.rect.w * 0.5 {
            return left;
        }
        if x <= fragment.rect.right() {
            return right;
        }
    }
    edges(line.fragments.last().expect("visual fragments")).1
}

fn edges(fragment: &TextFragment) -> (usize, usize) {
    if fragment.rtl { (fragment.range.end, fragment.range.start) } else { (fragment.range.start, fragment.range.end) }
}

pub(crate) fn caret(lines: &[TextLine], byte: usize) -> Option<Rect> {
    let line = lines.iter().rfind(|line| line.range.start <= byte && byte <= line.range.end).or_else(|| lines.last())?;
    let fragment = line
        .fragments
        .iter()
        .find(|fragment| fragment.range.start <= byte && byte < fragment.range.end)
        .or_else(|| line.fragments.iter().filter(|fragment| fragment.range.end <= byte).max_by_key(|fragment| fragment.range.end))
        .or_else(|| line.fragments.iter().min_by_key(|fragment| fragment.range.start));
    let x = fragment
        .map_or(line.rect.x, |fragment| if (byte >= fragment.range.end) != fragment.rtl { fragment.rect.right() } else { fragment.rect.x });
    Some(Rect::new(x, line.rect.y, 1.0, line.rect.h))
}

pub(crate) fn vertical_byte(geometry: &TextGeometry, byte: usize, direction: i32, x: Option<f32>) -> Option<(usize, f32)> {
    let current = caret(&geometry.lines, byte)?;
    let line = geometry.lines.iter().position(|line| line.rect.y == current.y)?;
    let next = line.checked_add_signed(direction as isize)?;
    let x = x.unwrap_or(current.x);
    Some((line_byte(geometry.lines.get(next)?, x), x))
}

pub(super) fn range_rects(lines: &[TextLine], range: &Range<usize>) -> Vec<Rect> {
    let mut rects = Vec::new();
    if range.is_empty() {
        return rects;
    }
    let mut active: Option<Rect> = None;
    for line in lines.iter().filter(|line| line.range.start < range.end && line.range.end > range.start) {
        for fragment in &*line.fragments {
            if !fragment.range.is_empty() && range.start < fragment.range.end && range.end > fragment.range.start {
                let rect = fragment.rect;
                if let Some(last) = &mut active
                    && (last.right() - rect.x).abs() < 0.01
                {
                    last.w += rect.w;
                } else {
                    rects.extend(active.replace(rect));
                }
            } else {
                rects.extend(active.take());
            }
        }
        let fragment_end = line.fragments.iter().map(|f| f.range.end).max().unwrap_or(line.range.start);
        if fragment_end < line.range.end && range.end > fragment_end {
            let x = caret(std::slice::from_ref(line), fragment_end).map_or(line.rect.right(), |rect| rect.x);
            rects.push(Rect::new(x, line.rect.y, 1.0, line.rect.h));
        }
        rects.extend(active.take());
    }
    rects
}

pub(super) fn merge_inline_rects(rects: &mut Vec<Rect>) {
    rects.sort_unstable_by(|a, b| a.y.total_cmp(&b.y).then_with(|| a.x.total_cmp(&b.x)));
    let mut count = 0;
    for i in 0..rects.len() {
        let rect = rects[i];
        if let Some(previous) = rects[..count].last_mut()
            && previous.y == rect.y
            && rect.x <= previous.right() + 0.01
        {
            previous.w = previous.right().max(rect.right()) - previous.x;
        } else {
            rects[count] = rect;
            count += 1;
        }
    }
    rects.truncate(count);
}

pub(super) fn translate(rect: Rect, offset: Point) -> Rect {
    Rect::new(rect.x + offset.x, rect.y + offset.y, rect.w, rect.h)
}

pub(super) fn union(rects: impl IntoIterator<Item = Rect>) -> Option<Rect> {
    rects.into_iter().reduce(|a, b| {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Rect::new(x, y, a.right().max(b.right()) - x, a.bottom().max(b.bottom()) - y)
    })
}
