use super::{StreamingFilter, StreamingPolicy, parse_stream};
use crate::ast::{Block, ParseOptions, Scope};
use memchr::memchr2;
use std::ops::Range;

/// Tail-only repairs for incomplete inline Markdown during streaming.
#[derive(Clone, Copy, Debug, Default)]
pub struct InlineRepair;

impl InlineRepair {
    /// Parses an unfinished tail with streaming previews enabled. Ranges refer to this tail.
    pub fn parse_tail(self, tail: &str, options: ParseOptions) -> Vec<Block> {
        parse_stream(tail, 0, options, StreamingFilter::default()).blocks
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct Patch {
    pub cut: usize,
    pub closers: String,
    pub markers: Vec<Range<usize>>,
    pub preview_image: Option<usize>,
}

struct Open {
    at: usize,
    ch: u8,
    n: usize,
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace())
}

/// Closes what the end of run `s` leaves open: emphasis, strong and strike opened at a word
/// start, code spans, links; cuts a trailing run of markers and openers with nothing after them.
/// `None`: the run is finished (a blank line after it, a heading's or a row's line ended).
#[cfg(test)]
pub(crate) fn close_tail(s: &str, scope: Scope, options: ParseOptions, atomic: impl Iterator<Item = Range<usize>>) -> Option<Patch> {
    plan(s, scope, options, StreamingPolicy::Hidden, atomic)
}

pub(super) fn plan(
    s: &str,
    scope: Scope,
    options: ParseOptions,
    media: StreamingPolicy,
    atomic: impl Iterator<Item = Range<usize>>,
) -> Option<Patch> {
    match scope {
        Scope::Para if paragraph_finished(s) => return None,
        Scope::Line | Scope::Cell if s.contains(['\r', '\n']) => return None,
        _ => {}
    }
    let b = s.as_bytes();
    let ch_before = |i: usize| s[..i].chars().next_back();
    let ch_at = |i: usize| s[i..].chars().next();
    let word_start = |i: usize| ch_before(i).is_none_or(|c| c.is_whitespace() || is_punct(c));
    let mut stack: Vec<Open> = Vec::new();
    let mut code: Option<(usize, usize)> = None;
    let mut atomic = atomic.peekable();
    let mut cut = s.len();
    let mut preview_image = None;
    let mut i = 0;
    'scan: while i < b.len() {
        while atomic.peek().is_some_and(|r| r.end <= i) {
            atomic.next();
        }
        if let Some(range) = atomic.peek()
            && range.start <= i
        {
            i = range.end;
            atomic.next();
            continue;
        }
        let c = b[i];
        match c {
            b'\\' => i += 1 + ch_at(i + 1).map_or(0, char::len_utf8),
            b'\r' | b'\n' => {
                if scope != Scope::Para {
                    stack.clear();
                }
                i += 1;
            }
            b'|' if scope == Scope::Cell => {
                stack.clear();
                i += 1;
            }
            b'|' | b'$' if c == b'|' || options.math => {
                let n = b[i..].iter().take_while(|&&byte| byte == c).count();
                let width = if c == b'|' { 2 } else { 1 };
                if i + n == b.len() && n < width && (word_start(i) || stack.iter().any(|open| open.ch == c)) {
                    cut = i;
                    break 'scan;
                }
                if n == width {
                    let after = ch_at(i + n);
                    let opens = if c == b'|' {
                        word_start(i) || after.is_some_and(|ch| !is_punct(ch) && !ch.is_whitespace())
                    } else {
                        after.is_some_and(|ch| !ch.is_whitespace())
                    };
                    if let Some(pos) = stack.iter().rposition(|open| open.ch == c)
                        && (c == b'|' || ch_before(i).is_some_and(|ch| !ch.is_whitespace()))
                    {
                        stack.truncate(pos);
                    } else if opens {
                        stack.push(Open { at: i, ch: c, n });
                    } else if i + n == b.len() && word_start(i) {
                        cut = i;
                        break 'scan;
                    }
                }
                i += n;
            }
            b'`' => {
                let n = b[i..].iter().take_while(|x| **x == b'`').count();
                match closing_ticks(b, i + n, n) {
                    Some(j) => i = j + n,
                    None => {
                        code = Some((i, n));
                        break 'scan;
                    }
                }
            }
            b'h' | b'w'
                if word_start(i) && (s[i..].starts_with("http://") || s[i..].starts_with("https://") || s[i..].starts_with("www.")) =>
            {
                i += s[i..].find(char::is_whitespace).unwrap_or(s.len() - i);
            }
            b'!' if i + 1 == b.len() && word_start(i) => {
                cut = i;
                break 'scan;
            }
            b'!' if b.get(i + 1) == Some(&b'[') => {
                if let Some(label) = pending_image_label(s, i) {
                    if media == StreamingPolicy::Progressive && !s[label.clone()].trim().is_empty() {
                        cut = label.end;
                        preview_image = Some(i);
                    } else {
                        cut = i;
                    }
                    break 'scan;
                }
                i += 2;
            }
            b'[' if word_start(i) && !matches!(b.get(i + 1), Some(b'!' | b'^')) => {
                stack.push(Open { at: i, ch: b'[', n: 1 });
                i += 1;
            }
            b']' => {
                let Some(top) = stack.iter().rposition(|o| o.ch == b'[') else {
                    i += 1;
                    continue;
                };
                if i + 1 == b.len() {
                    let label = &s[stack[top].at + 1..i];
                    if !options.tasklists && (label == " " || label.eq_ignore_ascii_case("x")) {
                        stack.remove(top);
                    } else {
                        cut = i;
                    }
                    break 'scan;
                }
                if b[i + 1] == b'(' {
                    match closing_destination(&s[i + 2..]) {
                        Some(j) => {
                            stack.truncate(top);
                            i = i + 2 + j + 1;
                        }
                        None => {
                            // The destination is arriving; a title may have opened.
                            let rest = &s[i + 2..];
                            let quote =
                                rest.find(' ').and_then(|sp| rest[sp..].trim_start().bytes().next()).filter(|q| *q == b'"' || *q == b'\'');
                            let closed = quote.is_some_and(|q| rest.bytes().filter(|x| *x == q).count() % 2 == 0);
                            stack[top].ch = match quote {
                                Some(q) if !closed => q,
                                _ => b'(',
                            };
                            break 'scan;
                        }
                    }
                } else {
                    stack.remove(top);
                    i += 1;
                }
            }
            b'*' | b'_' | b'~' if c != b'~' || options.strikethrough => {
                let n = b[i..].iter().take_while(|x| **x == c).count();
                // `~` strikes in pairs; one at the very end may become two (opening or closing).
                if c == b'~' && n != 2 {
                    let may_close = ch_before(i).is_some_and(|x| !x.is_whitespace()) && stack.iter().any(|o| o.ch == c);
                    if n == 1 && i + 1 == b.len() && (word_start(i) || may_close) {
                        cut = i;
                        break 'scan;
                    }
                    i += n;
                    continue;
                }
                let before = ch_before(i);
                let after = ch_at(i + n);
                let at_end = i + n == b.len();
                let starts_word = before.is_none_or(|x| x.is_whitespace() || is_punct(x));
                if at_end {
                    let may_close = before.is_some_and(|x| !x.is_whitespace()) && stack.iter().any(|o| o.ch == c);
                    if starts_word || may_close {
                        cut = i;
                        break 'scan;
                    }
                    i += n;
                    continue;
                }
                let left = after.is_some_and(|a| !a.is_whitespace()) && (after.is_some_and(|a| !is_punct(a)) || starts_word);
                let right = before.is_some_and(|x| !x.is_whitespace())
                    && (before.is_some_and(|x| !is_punct(x)) || after.is_none_or(|a| a.is_whitespace() || is_punct(a)));
                if right && stack.iter().any(|o| o.ch == c) {
                    let mut left_n = n;
                    while left_n > 0 {
                        let Some(pos) = stack.iter().rposition(|o| o.ch == c) else { break };
                        let take = left_n.min(stack[pos].n);
                        stack[pos].n -= take;
                        left_n -= take;
                        if stack[pos].n == 0 {
                            stack.truncate(pos);
                        }
                    }
                } else if left && starts_word {
                    stack.push(Open { at: i, ch: c, n });
                }
                i += n;
            }
            _ => i += ch_at(i).map_or(1, char::len_utf8),
        }
    }
    // A code span with nothing in it yet is cut.
    if let Some((at, n)) = code
        && s[at + n..cut].trim().is_empty()
    {
        cut = at;
        code = None;
    }
    // Openers with nothing after them (or a `[` with only `x`: maybe a task box) are cut.
    while let Some(o) = stack.last() {
        let rest = s[(o.at + o.n).min(cut)..cut].trim();
        if o.at >= cut || rest.is_empty() || (options.tasklists && o.ch == b'[' && rest.eq_ignore_ascii_case("x")) {
            cut = cut.min(o.at);
            stack.pop();
        } else {
            break;
        }
    }
    let mut closers = String::new();
    let mut markers: Vec<Range<usize>> = stack.iter().map(|o| o.at..o.at + o.n).collect();
    let preview_image = preview_image.filter(|&at| at + 2 < cut);
    if let Some(at) = preview_image {
        closers.push_str("]()");
        markers.push(at..at + 2);
    }
    if let Some((at, n)) = code {
        if s[at + n..cut].starts_with(' ') {
            closers.push(' ');
        }
        closers.push_str(&"`".repeat(n));
        markers.push(at..at + n);
    }
    for o in stack.iter().rev() {
        match o.ch {
            b'[' => closers.push_str("]()"),
            b'(' => closers.push(')'),
            q @ (b'"' | b'\'') => {
                closers.push(q as char);
                closers.push(')');
            }
            ch => closers.push_str(&(ch as char).to_string().repeat(o.n)),
        }
    }
    Some(Patch { cut, closers, markers, preview_image })
}

fn paragraph_finished(source: &str) -> bool {
    let bytes = source.as_bytes();
    let mut start = 0;
    while let Some(offset) = memchr2(b'\r', b'\n', &bytes[start..]) {
        let end = start + offset;
        if start > 0 && source[start..end].trim().is_empty() {
            return true;
        }
        start = end + 1 + usize::from(bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n'));
    }
    false
}

/// Returns the end of a link destination while respecting nested parentheses and escapes.
fn closing_destination(source: &str) -> Option<usize> {
    let mut depth = 0;
    let mut bytes = source.bytes().enumerate();
    while let Some((at, byte)) = bytes.next() {
        match byte {
            b'\\' => {
                bytes.next();
            }
            b'(' => depth += 1,
            b')' if depth == 0 => return Some(at),
            b')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn pending_image_label(source: &str, start: usize) -> Option<Range<usize>> {
    let from = start + 2;
    let mut depth = 0;
    let mut bytes = source[from..].bytes().enumerate();
    while let Some((at, byte)) = bytes.next() {
        match byte {
            b'\\' => {
                bytes.next();
            }
            b'[' => depth += 1,
            b']' if depth > 0 => depth -= 1,
            b']' => {
                let end = from + at;
                let suffix = &source[end + 1..];
                let pending = suffix.is_empty()
                    || suffix.strip_prefix('(').is_some_and(|link| closing_destination(link).is_none())
                    || suffix.strip_prefix('[').is_some_and(|reference| !reference.contains(']'));
                return pending.then_some(from..end);
            }
            _ => {}
        }
    }
    Some(from..source.len())
}

/// Where a run of exactly `n` backticks starts at or after `from`.
fn closing_ticks(b: &[u8], from: usize, n: usize) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        if b[i] == b'`' {
            let m = b[i..].iter().take_while(|x| **x == b'`').count();
            if m == n {
                return Some(i);
            }
            i += m;
        } else {
            i += 1;
        }
    }
    None
}
