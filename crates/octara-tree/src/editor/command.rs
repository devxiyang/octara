//! Semantic editing commands over logical text and optional layout geometry.

use super::{EditorAction, EditorRejection, EditorSession, ordered};
use crate::selection::{paragraph_at, word_edge};
use octara_model::{
    input::{EditorCommand, Key, KeyEvent, Modifiers},
    text::{CommitKind, TextGeometry, TextLine},
};
use std::{cmp::Ordering, ops::Range};
use unicode_segmentation::UnicodeSegmentation;

/// Translate a normalized key when the platform supplies no editing selector.
pub fn command_for_key(key: KeyEvent) -> Option<EditorCommand> {
    use EditorCommand::*;
    let extend = key.modifiers.contains(Modifiers::SHIFT);
    let modifiers = key.modifiers - Modifiers::SHIFT;
    let primary = matches!(modifiers, Modifiers::CTRL | Modifiers::META);
    let word = matches!(modifiers, Modifiers::CTRL | Modifiers::ALT);
    let select = |move_to, select_to| if extend { select_to } else { move_to };
    Some(match key.key {
        Key::Char(c) if primary => match (c.to_ascii_lowercase(), extend) {
            ('a', false) => SelectAll,
            ('c', false) => Copy,
            ('x', false) => Cut,
            ('v', false) => Paste,
            ('z', false) => Undo,
            ('z', true) | ('y', false) => Redo,
            _ => return None,
        },
        Key::Left if modifiers.is_empty() => select(MoveLeft, SelectLeft),
        Key::Right if modifiers.is_empty() => select(MoveRight, SelectRight),
        Key::Up if modifiers.is_empty() => select(MoveUp, SelectUp),
        Key::Down if modifiers.is_empty() => select(MoveDown, SelectDown),
        Key::Left if word => select(MoveWordLeft, SelectWordLeft),
        Key::Right if word => select(MoveWordRight, SelectWordRight),
        Key::Home if primary => select(MoveDocumentStart, SelectDocumentStart),
        Key::End if primary => select(MoveDocumentEnd, SelectDocumentEnd),
        Key::Home if modifiers.is_empty() => select(MoveLineStart, SelectLineStart),
        Key::End if modifiers.is_empty() => select(MoveLineEnd, SelectLineEnd),
        Key::Left if modifiers == Modifiers::META => select(MoveLineStart, SelectLineStart),
        Key::Right if modifiers == Modifiers::META => select(MoveLineEnd, SelectLineEnd),
        Key::Up if modifiers == Modifiers::META => select(MoveDocumentStart, SelectDocumentStart),
        Key::Down if modifiers == Modifiers::META => select(MoveDocumentEnd, SelectDocumentEnd),
        Key::Backspace if modifiers.is_empty() => DeleteBackward,
        Key::Delete if modifiers.is_empty() => DeleteForward,
        Key::Backspace if word => DeleteWordBackward,
        Key::Delete if word => DeleteWordForward,
        Key::Backspace if modifiers == Modifiers::META => DeleteToLineStart,
        Key::Delete if modifiers == Modifiers::META => DeleteToLineEnd,
        Key::Enter if modifiers.is_empty() => InsertNewline,
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum Destination {
    Grapheme(bool),
    Word(bool),
    Vertical(bool),
    Line(bool),
    Document(bool),
}

#[derive(Clone, Copy)]
pub(super) enum Column {
    Visual(f32),
    Logical(usize),
}

impl EditorSession<'_> {
    pub(super) fn route_command(&self, command: EditorCommand, geometry: Option<&TextGeometry>) -> Result<EditorAction, EditorRejection> {
        use Destination::*;
        use EditorCommand::*;
        if self.slot.composition.is_some() {
            return Err(EditorRejection::Composing);
        }
        let (destination, extend) = match command {
            MoveLeft | SelectLeft => (Grapheme(true), command == SelectLeft),
            MoveRight | SelectRight => (Grapheme(false), command == SelectRight),
            MoveWordLeft | SelectWordLeft => (Word(true), command == SelectWordLeft),
            MoveWordRight | SelectWordRight => (Word(false), command == SelectWordRight),
            MoveUp | SelectUp => (Vertical(true), command == SelectUp),
            MoveDown | SelectDown => (Vertical(false), command == SelectDown),
            MoveLineStart | SelectLineStart => (Line(true), command == SelectLineStart),
            MoveLineEnd | SelectLineEnd => (Line(false), command == SelectLineEnd),
            MoveDocumentStart | SelectDocumentStart => (Document(true), command == SelectDocumentStart),
            MoveDocumentEnd | SelectDocumentEnd => (Document(false), command == SelectDocumentEnd),
            SelectAll => return self.select((0, self.slot.rope().len()), None),
            DeleteBackward => return self.delete(Grapheme(true), geometry),
            DeleteForward => return self.delete(Grapheme(false), geometry),
            DeleteWordBackward => return self.delete(Word(true), geometry),
            DeleteWordForward => return self.delete(Word(false), geometry),
            DeleteToLineStart => return self.delete(Line(true), geometry),
            DeleteToLineEnd => return self.delete(Line(false), geometry),
            InsertNewline if self.slot.multiline => return self.insert("\n", None, CommitKind::Typing),
            InsertNewline => return Ok(EditorAction::Unhandled),
            Undo | Redo => {
                return Ok(self.history_proposal(command == Undo).map_or(EditorAction::Handled, EditorAction::Propose));
            }
            Copy | Cut if self.slot.clipboard_text().is_none() => return Ok(EditorAction::Handled),
            Copy | Cut | Paste => return Ok(EditorAction::Clipboard(command)),
        };
        self.navigate(destination, extend, geometry)
    }

    fn navigate(&self, destination: Destination, extend: bool, geometry: Option<&TextGeometry>) -> Result<EditorAction, EditorRejection> {
        let (anchor, caret) = self.slot.selection;
        let rope = self.slot.rope();
        let lines = geometry.map_or(&[][..], |geometry| &*geometry.lines);
        let located = line_at(lines, caret);
        let mut column = None;
        let next = match destination {
            Destination::Grapheme(left) if !extend && anchor != caret => {
                let order = line_at(lines, anchor).zip(located).map_or_else(
                    || anchor.cmp(&caret),
                    |((a, a_line), (b, b_line))| {
                        a.cmp(&b).then_with(|| visual_order(caret_x(a_line, anchor), anchor, caret_x(b_line, caret), caret))
                    },
                );
                if order.is_lt() == left { anchor } else { caret }
            }
            Destination::Grapheme(left) => {
                if located.is_some() {
                    visual_edge(lines, located, caret, left, |_| true).unwrap_or(caret)
                } else {
                    rope.grapheme_edge(caret, left).unwrap_or(caret)
                }
            }
            Destination::Word(left) => {
                if located.is_none() {
                    word_edge(rope.as_str(), caret, left)
                } else {
                    let words: Vec<_> = rope.as_str().unicode_word_indices().map(|(start, word)| start..start + word.len()).collect();
                    visual_edge(lines, located, caret, left, |(byte, rtl)| {
                        let start = left != rtl;
                        if start {
                            words.binary_search_by_key(&byte, |word| word.start).is_ok()
                        } else {
                            words.binary_search_by_key(&byte, |word| word.end).is_ok()
                        }
                    })
                    .or_else(|| visual_limit(lines, left))
                    .unwrap_or(caret)
                }
            }
            Destination::Vertical(up) => {
                let (next, goal) = self.vertical(caret, lines, located, up);
                column = Some(goal);
                next
            }
            Destination::Line(start) => {
                let line = line_range(rope.as_str(), located, caret);
                if start { line.start } else { line.end }
            }
            Destination::Document(start) => {
                if start {
                    0
                } else {
                    rope.len()
                }
            }
        };
        // Layout may be older than a speculative edit. Keep all resulting offsets valid.
        let next = if rope.is_char_boundary(next) { next } else { caret };
        let next = self.slot.rich_text().map_or(next, |rich| match destination {
            Destination::Vertical(_) => next,
            _ => rich.snap_movement(next, next < caret),
        });
        self.select((if extend { anchor } else { next }, next), column)
    }

    fn vertical(&self, caret: usize, lines: &[TextLine], located: Option<(usize, &TextLine)>, up: bool) -> (usize, Column) {
        let text = self.slot.value();
        let document_edge = || if up { 0 } else { text.len() };
        if let Some((index, line)) = located {
            let x = match self.slot.column {
                Some(Column::Visual(x)) => x,
                _ => caret_x(line, caret),
            };
            let next = index
                .checked_add_signed(if up { -1 } else { 1 })
                .and_then(|index| lines.get(index))
                .map_or_else(document_edge, |line| line_at_x(line, x));
            (next, Column::Visual(x))
        } else {
            let line = paragraph_at(text, caret);
            let column = match self.slot.column {
                Some(Column::Logical(column)) => column,
                _ => text[line.start..caret].graphemes(true).count(),
            };
            let target = if up {
                line.start.checked_sub(1).map(|byte| paragraph_at(text, byte))
            } else {
                text[line.end..].find('\n').map(|index| paragraph_at(text, line.end + index + 1))
            };
            let next = target.map_or_else(document_edge, |line| {
                text[line.clone()].grapheme_indices(true).nth(column).map_or(line.end, |(index, _)| line.start + index)
            });
            (next, Column::Logical(column))
        }
    }

    fn delete(&self, destination: Destination, geometry: Option<&TextGeometry>) -> Result<EditorAction, EditorRejection> {
        let selection = ordered(self.slot.selection);
        let caret = selection.start;
        let rope = self.slot.rope();
        let range = if selection.is_empty() {
            let byte = match destination {
                Destination::Grapheme(backward) => {
                    if let Some(atom) = self.slot.rich_text().and_then(|rich| rich.adjacent_atom(caret, backward)) {
                        return self.insert("", Some(atom.range.clone()), CommitKind::Deleting);
                    }
                    rope.grapheme_edge(caret, backward).unwrap_or(caret)
                }
                Destination::Word(backward) => word_edge(rope.as_str(), caret, backward),
                Destination::Line(start) => {
                    let line = line_range(rope.as_str(), geometry.and_then(|geometry| line_at(&geometry.lines, caret)), caret);
                    if start { line.start } else { line.end }
                }
                _ => unreachable!(),
            };
            ordered((caret, byte))
        } else {
            selection
        };
        if range.is_empty() {
            return self.select(self.slot.selection, None);
        }
        self.insert("", Some(range), CommitKind::Deleting)
    }
}

fn line_at(lines: &[TextLine], caret: usize) -> Option<(usize, &TextLine)> {
    lines.iter().enumerate().rfind(|(_, line)| line.range.start <= caret && caret <= line.range.end)
}

fn line_range(text: &str, located: Option<(usize, &TextLine)>, caret: usize) -> Range<usize> {
    if let Some((_, line)) = located
        && let Some(segment) = text.get(line.range.clone())
    {
        return line.range.start..line.range.start + segment.trim_end_matches(['\r', '\n']).len();
    }
    paragraph_at(text, caret)
}

fn caret_x(line: &TextLine, byte: usize) -> f32 {
    let fragment = line
        .fragments
        .iter()
        .find(|fragment| fragment.range.contains(&byte))
        .or_else(|| line.fragments.iter().filter(|fragment| fragment.range.end <= byte).max_by_key(|fragment| fragment.range.end))
        .or_else(|| line.fragments.iter().min_by_key(|fragment| fragment.range.start));
    fragment.map_or(
        line.rect.x,
        |fragment| {
            if (byte >= fragment.range.end) != fragment.rtl { fragment.rect.right() } else { fragment.rect.x }
        },
    )
}

fn caret_stops(line: &TextLine) -> impl Iterator<Item = (f32, usize, bool)> {
    let starts = line.fragments.iter().map(|fragment| {
        let x = if fragment.rtl { fragment.rect.right() } else { fragment.rect.x };
        (x, fragment.range.start, fragment.rtl)
    });
    let end = line.fragments.iter().max_by_key(|fragment| fragment.range.end).map(|fragment| {
        let x = if fragment.rtl { fragment.rect.x } else { fragment.rect.right() };
        (x, fragment.range.end, fragment.rtl)
    });
    starts.chain(end).chain(line.fragments.is_empty().then_some((line.rect.x, line.range.start, false)))
}

fn visual_order(a: f32, a_byte: usize, b: f32, b_byte: usize) -> Ordering {
    a.total_cmp(&b).then_with(|| a_byte.cmp(&b_byte))
}

fn visual_edge(
    lines: &[TextLine],
    located: Option<(usize, &TextLine)>,
    caret: usize,
    left: bool,
    eligible: impl Fn((usize, bool)) -> bool,
) -> Option<usize> {
    let (index, line) = located?;
    // Use the primary caret position at each logical offset. A stable order at
    // shared positions makes movement reversible across directional boundaries.
    let best = |line: &TextLine, current: Option<f32>| {
        caret_stops(line)
            .filter(|&(x, byte, rtl)| {
                byte != caret
                    && current.is_none_or(|current_x| {
                        let order = visual_order(x, byte, current_x, caret);
                        if left { order.is_lt() } else { order.is_gt() }
                    })
                    && eligible((byte, rtl))
            })
            .min_by(|&(ax, a, _), &(bx, b, _)| {
                let order = visual_order(ax, a, bx, b);
                if left { order.reverse() } else { order }
            })
            .map(|(_, byte, _)| byte)
    };
    best(line, Some(caret_x(line, caret))).or_else(|| {
        if left {
            lines[..index].iter().rev().find_map(|line| best(line, None))
        } else {
            lines[index + 1..].iter().find_map(|line| best(line, None))
        }
    })
}

fn visual_limit(lines: &[TextLine], left: bool) -> Option<usize> {
    let line = if left { lines.first()? } else { lines.last()? };
    caret_stops(line)
        .min_by(|&(ax, a, _), &(bx, b, _)| {
            let order = visual_order(ax, a, bx, b);
            if left { order } else { order.reverse() }
        })
        .map(|(_, byte, _)| byte)
}

fn line_at_x(line: &TextLine, x: f32) -> usize {
    caret_stops(line).min_by(|(a, ..), (b, ..)| (x - a).abs().total_cmp(&(x - b).abs())).map_or(line.range.start, |(_, byte, _)| byte)
}
