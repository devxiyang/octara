//! A growing source: a parse kept so a source that grows parses again only from its last
//! finished block, and the end of a streaming source repaired for the parse (never in the
//! source) so an open span shows styled and no marker flashes.

use super::parse::{Block, Inline, Kind, Parsed, Scope, parse};
use std::ops::Range;

/// A Markdown source and its parse.
#[derive(Default)]
pub struct Doc {
    src: String,
    parsed: Parsed,
    /// The end is repaired (a stream).
    streaming: bool,
    /// How many leading blocks the last update kept as they were.
    kept: usize,
    /// Whole parses and bytes parsed so far (to check a stream parses only its end).
    pub full_parses: u64,
    pub parsed_bytes: u64,
}

impl Doc {
    pub fn new(streaming: bool) -> Doc {
        Doc { streaming, ..Doc::default() }
    }

    pub fn blocks(&self) -> &[Block] {
        &self.parsed.blocks
    }

    /// Where each top-level block's first line starts.
    pub fn starts(&self) -> &[usize] {
        &self.parsed.starts
    }

    pub fn source(&self) -> &str {
        &self.src
    }

    /// How many leading blocks the last update kept (the others may have changed).
    pub fn kept(&self) -> usize {
        self.kept
    }

    /// Parses `src`: from the last top-level block whose first line is finished when it grew from
    /// the last source, whole otherwise. A reference definition (`]:`) may change links anywhere:
    /// a whole parse.
    pub fn update(&mut self, src: &str) {
        if src == self.src && (!self.src.is_empty() || !self.parsed.blocks.is_empty()) {
            self.kept = self.parsed.blocks.len();
            return;
        }
        let old = self.src.len();
        let grown = src.len() > old && src.starts_with(&self.src) && !src.contains("]:");
        let cut = grown
            .then(|| self.parsed.starts.iter().rposition(|&at| at > 0 && at <= old && self.src[at..].contains('\n')))
            .flatten()
            .filter(|i| *i < self.parsed.blocks.len());
        match cut {
            Some(i) => {
                self.kept = i;
                let at = self.parsed.starts[i];
                let tail = self.parse_from(src, at);
                self.parsed.blocks.truncate(i);
                self.parsed.blocks.extend(tail.blocks);
                self.parsed.starts.truncate(i);
                self.parsed.starts.extend(tail.starts);
                self.parsed.shown.retain(|r| r.end <= at);
                self.parsed.shown.extend(tail.shown);
                self.parsed.tail = tail.tail;
                self.parsed.in_code = tail.in_code;
            }
            None => {
                self.kept = 0;
                self.full_parses += 1;
                self.parsed = self.parse_from(src, 0);
            }
        }
        if grown {
            self.src.push_str(&src[old..]);
        } else {
            self.src = src.to_owned();
        }
    }

    /// Parses `src[at..]` (offsets `src`'s), its end repaired when streaming.
    fn parse_from(&mut self, src: &str, at: usize) -> Parsed {
        self.parsed_bytes += (src.len() - at) as u64;
        let raw = parse(&src[at..], at);
        if !self.streaming { raw } else { repair(src, at, raw) }
    }
}

/// The parse of `src[at..]` with its end repaired: trailing table lines held back (a header
/// without its delimiter row yet shows as a header alone), open spans closed, a trailing run of
/// markers left out. A repair that would show a marker falls back to the parse without it.
fn repair(src: &str, at: usize, raw: Parsed) -> Parsed {
    if raw.in_code {
        return raw;
    }
    let hold = held_table(src, at, &raw);
    let end = hold.unwrap_or(src.len());
    let mut out = match raw.tail.filter(|(start, _)| *start >= at && *start < end) {
        Some((start, scope)) => match close_tail(&src[start..end], scope) {
            Some(patch) => {
                let kept = src[at..start + patch.cut].trim_end();
                let plain = || parse(kept, at);
                if patch.closers.is_empty() {
                    plain()
                } else {
                    let view = format!("{kept}{}", patch.closers);
                    let fixed = parse(&view, at);
                    let limit = at + kept.len();
                    let markers: Vec<Range<usize>> = patch.markers.iter().map(|m| start + m.start..start + m.end).collect();
                    let shows_marker =
                        fixed.shown.iter().any(|s| s.end > limit || markers.iter().any(|m| s.start < m.end && m.start < s.end));
                    if shows_marker { plain() } else { fixed }
                }
            }
            None if hold.is_some() => parse(&src[at..end], at),
            None => raw,
        },
        None if hold.is_some() => parse(&src[at..end], at),
        None => raw,
    };
    // A held header shows as a table's head when it starts a block (not under a paragraph's line).
    let starts_block = |h: usize| h == 0 || src[..h - 1].rsplit('\n').next().is_none_or(|l| l.trim().is_empty());
    if let Some(h) = hold
        && starts_block(h)
        && let Some(head) = pending_header(&src[h..])
    {
        out.starts.push(h);
        out.blocks.push(Block {
            kind: Kind::Table { align: vec![super::parse::Align::None; head.len()], head, rows: Vec::new() },
            source: h..src.len(),
        });
    }
    out
}

/// Where trailing table lines start that are not a table yet: the line arriving, when it starts
/// with `|` (with the header above it when it is a partial delimiter row); a finished `|` line
/// whose line above is not one (a header waiting for its delimiter row).
fn held_table(src: &str, at: usize, raw: &Parsed) -> Option<usize> {
    if raw.blocks.last().is_some_and(|b| matches!(b.kind, Kind::Table { .. })) {
        return None;
    }
    let body = &src[at..];
    let piped = |l: &str| l.trim_start().starts_with('|');
    let line_at = |end: usize| body[..end].rfind('\n').map_or(0, |i| i + 1);
    if !body.ends_with('\n') {
        let last = line_at(body.len());
        if !piped(&body[last..]) {
            return None;
        }
        // A partial delimiter row holds its header too.
        let delim = body[last..].chars().all(|c| matches!(c, '|' | ':' | '-' | ' '));
        if delim && last > 0 {
            let prev = line_at(last - 1);
            if piped(&body[prev..last - 1]) {
                return Some(at + prev);
            }
        }
        return Some(at + last);
    }
    let end = body.len() - 1;
    let last = line_at(end);
    if !piped(&body[last..end]) {
        return None;
    }
    let above = (last > 0).then(|| line_at(last - 1)).filter(|p| piped(&body[*p..last - 1]));
    above.is_none().then_some(at + last)
}

/// A held header shown as a table's head: a line of at least two cells (and maybe a partial
/// delimiter row under it).
fn pending_header(held: &str) -> Option<Vec<Inline>> {
    let mut lines = held.lines();
    let head = lines.next()?;
    if lines.next().is_some_and(|l| !l.chars().all(|c| matches!(c, '|' | ':' | '-' | ' '))) || lines.next().is_some() {
        return None;
    }
    let cells = split_row(head);
    (cells.len() >= 2).then(|| {
        cells
            .into_iter()
            .map(|c| match parse(&c, 0).blocks.into_iter().next().map(|b| b.kind) {
                Some(Kind::Paragraph(i)) => i,
                _ => Inline { text: c.trim().to_owned(), styles: Vec::new() },
            })
            .collect()
    })
}

/// A table row's cells (`\|` is a pipe in a cell).
fn split_row(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let mut cells = vec![String::new()];
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cells.last_mut().unwrap().push('|');
                chars.next();
            }
            '|' => cells.push(String::new()),
            c => cells.last_mut().unwrap().push(c),
        }
    }
    if cells.last().is_some_and(|c| c.trim().is_empty()) {
        cells.pop();
    }
    cells.into_iter().map(|c| c.trim().to_owned()).collect()
}

/// How to repair the end of an inline run: where to cut it, what to append, and where the
/// openers are (they must not show).
#[derive(Debug, PartialEq)]
pub struct Patch {
    pub cut: usize,
    pub closers: String,
    pub markers: Vec<Range<usize>>,
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
pub fn close_tail(s: &str, scope: Scope) -> Option<Patch> {
    match scope {
        Scope::Para if s.contains("\n\n") || s.trim_end_matches([' ', '\t']).ends_with("\n\n") => return None,
        Scope::Line | Scope::Cell if s.contains('\n') => return None,
        _ => {}
    }
    let b = s.as_bytes();
    let ch_before = |i: usize| s[..i].chars().next_back();
    let ch_at = |i: usize| s[i..].chars().next();
    let word_start = |i: usize| ch_before(i).is_none_or(|c| c.is_whitespace() || is_punct(c));
    let mut stack: Vec<Open> = Vec::new();
    let mut code: Option<(usize, usize)> = None;
    let mut cut = s.len();
    let mut i = 0;
    'scan: while i < b.len() {
        let c = b[i];
        match c {
            b'\\' => i += 2,
            b'\n' => {
                if scope != Scope::Para {
                    stack.clear();
                }
                i += 1;
            }
            b'|' if scope == Scope::Cell => {
                stack.clear();
                i += 1;
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
            b'[' if word_start(i) || (i > 0 && b[i - 1] == b'!') => {
                let at = if i > 0 && b[i - 1] == b'!' { i - 1 } else { i };
                stack.push(Open { at, ch: b'[', n: 1 });
                i += 1;
            }
            b']' => {
                let Some(top) = stack.iter().rposition(|o| o.ch == b'[') else {
                    i += 1;
                    continue;
                };
                if i + 1 == b.len() {
                    cut = i;
                    break 'scan;
                }
                if b[i + 1] == b'(' {
                    match s[i + 2..].find(')') {
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
            b'*' | b'_' | b'~' => {
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
        if o.at >= cut || rest.is_empty() || (o.ch == b'[' && rest.eq_ignore_ascii_case("x")) {
            cut = cut.min(o.at);
            stack.pop();
        } else {
            break;
        }
    }
    let mut closers = String::new();
    let mut markers: Vec<Range<usize>> = stack.iter().map(|o| o.at..o.at + o.n).collect();
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
    Some(Patch { cut, closers, markers })
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
