//! Opaque block extensions occupy their original source ranges in the CommonMark parser.

use super::{Block, Kind, ParseOptions, line_start};
use memchr::{memchr2, memmem};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use std::borrow::Cow;
use std::ops::Range;

pub(super) fn line_end(src: &str, at: usize) -> usize {
    let Some(offset) = memchr2(b'\r', b'\n', &src.as_bytes()[at..]) else { return src.len() };
    let end = at + offset + 1;
    end + usize::from(src.as_bytes().get(end - 1..end + 1) == Some(b"\r\n"))
}

enum Opening {
    Directive { fence_len: usize, name: String, attrs: Vec<(String, String)> },
    Math,
}

impl Opening {
    fn parse(line: &str, math: bool) -> Option<Self> {
        let line = line.trim_end();
        if math && line.starts_with("$$") && !line.starts_with("$$$") {
            return Some(Self::Math);
        }
        let fence_len = line.bytes().take_while(|&b| b == b':').count();
        let rest = line.get(fence_len..)?;
        if fence_len < 3 || !rest.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
            return None;
        }
        let attribute_start = rest.find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_').unwrap_or(rest.len());
        let name = rest[..attribute_start].to_owned();
        let mut attrs = rest[attribute_start..].trim();
        if let Some(rest) = attrs.strip_prefix('{') {
            attrs = rest.strip_suffix('}')?.trim();
        }
        Some(Self::Directive { fence_len, name, attrs: attributes(attrs)? })
    }

    fn closes(&self, line: &str) -> bool {
        let line = line.trim_matches([' ', '\t', '\r', '\n']);
        match self {
            Self::Directive { fence_len, .. } => line.len() >= *fence_len && line.bytes().all(|b| b == b':'),
            Self::Math => line == "$$",
        }
    }

    fn finish(self, body: String, is_complete: bool) -> Kind {
        match self {
            Self::Directive { name, attrs, .. } => Kind::Directive { name, attrs, body, is_complete },
            Self::Math => Kind::Math { tex: body, is_complete },
        }
    }
}

fn attributes(mut input: &str) -> Option<Vec<(String, String)>> {
    let mut attrs = Vec::new();
    while !input.is_empty() {
        let end = input.find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-').unwrap_or(input.len());
        if end == 0 {
            return None;
        }
        let key = &input[..end];
        input = input[end..].trim_start();
        let Some(value) = input.strip_prefix('=') else {
            attrs.push((key.to_owned(), String::new()));
            continue;
        };
        let value = value.trim_start();
        let next = match value.as_bytes().first() {
            Some(b'\'' | b'"') => {
                let quote = value.as_bytes()[0];
                let end = value.as_bytes()[1..].iter().position(|&b| b == quote)? + 1;
                input = &value[end + 1..];
                &value[1..end]
            }
            Some(_) => {
                let end = value.find(char::is_whitespace).unwrap_or(value.len());
                input = &value[end..];
                &value[..end]
            }
            None => return None,
        };
        if input.starts_with(|ch: char| !ch.is_whitespace()) {
            return None;
        }
        attrs.push((key.to_owned(), next.to_owned()));
        input = input.trim_start();
    }
    Some(attrs)
}

/// Removes a known number of explicitly quoted prefixes, preserving any nested content.
fn quoted_content(line: &str, depth: usize) -> Option<&str> {
    let mut content = line;
    for _ in 0..depth {
        content = content.trim_start_matches([' ', '\t']).strip_prefix('>')?;
        content = content.strip_prefix([' ', '\t']).unwrap_or(content);
    }
    Some(content)
}

fn quote_depth(prefix: &str) -> Option<usize> {
    prefix.bytes().all(|b| matches!(b, b' ' | b'\t' | b'>')).then(|| prefix.bytes().filter(|&b| b == b'>').count())
}

fn opaque_block(src: &str, start: usize, depth: usize, opening: Opening, masked: &mut [u8]) -> Block {
    let header_end = line_end(src, start);
    let is_math = matches!(opening, Opening::Math);
    let mut body = String::new();
    if is_math {
        body.push_str(&src[start + 2..header_end]);
    }
    // A thematic break interrupts preceding text and provides an event at the extension's start.
    // Math has a two-byte opener. Keep its shape by using an empty ATX heading as the sentinel.
    blank_bytes(&mut masked[start..header_end]);
    if is_math {
        masked[start] = b'#';
    } else {
        masked[start..start + 3].copy_from_slice(b"***");
    }
    let mut end = header_end;
    let mut is_complete = false;
    while end < src.len() {
        let next = line_end(src, end);
        let line = &src[end..next];
        let Some(content) = quoted_content(line, depth) else { break };
        let content_start = next - content.len();
        blank_bytes(&mut masked[content_start..next]);
        end = next;
        if opening.closes(content) {
            is_complete = true;
            break;
        }
        body.push_str(content);
    }
    Block { kind: opening.finish(body, is_complete), source: start..end }
}

fn blank_bytes(line: &mut [u8]) {
    for byte in line {
        if !matches!(*byte, b'\r' | b'\n') {
            *byte = b' ';
        }
    }
}

struct Candidates {
    openings: Vec<(usize, usize, Opening)>,
    spans: Vec<Range<usize>>,
}

fn candidates(src: &str, options: ParseOptions, math: bool) -> Candidates {
    let mut candidates = Vec::new();
    let mut complete_math = Vec::new();
    let mut spans = Vec::new();
    let mut opaque = false;
    for (event, range) in Parser::new_ext(src, options.parser_options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                opaque = true;
                spans.push(range);
            }
            Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock) => opaque = false,
            Event::DisplayMath(_) => {
                complete_math.push(range.start);
                spans.push(range);
            }
            Event::Code(_) | Event::InlineMath(_) => spans.push(range),
            Event::Start(Tag::Paragraph) | Event::Text(_) if !opaque => {
                if candidates.last().is_some_and(|(at, _, _)| *at == range.start)
                    || !matches!(src.as_bytes().get(range.start), Some(b':' | b'$'))
                {
                    continue;
                }
                if let Some(depth) = quote_depth(&src[line_start(src, range.start)..range.start])
                    && let Some(opening) = Opening::parse(&src[range.start..line_end(src, range.start)], math)
                {
                    candidates.push((range.start, depth, opening));
                }
            }
            _ => {}
        }
    }
    candidates.retain(|(at, _, opening)| !(matches!(opening, Opening::Math) && complete_math.binary_search(at).is_ok()));
    Candidates { openings: candidates, spans }
}

pub(super) fn prepare(src: &str, options: ParseOptions) -> (Cow<'_, str>, Vec<Block>) {
    let directives = memmem::find(src.as_bytes(), b":::").is_some();
    let math = options.math && memmem::find(src.as_bytes(), b"$$").is_some();
    if !directives && !math {
        return (Cow::Borrowed(src), Vec::new());
    }
    let mut source = Cow::Borrowed(src);
    let mut blocks: Vec<Block> = Vec::new();
    loop {
        let Candidates { openings, spans } = candidates(&source, options, math);
        if openings.is_empty() {
            break;
        }
        let mut masked = source.into_owned().into_bytes();
        let mut spans = spans.into_iter().peekable();
        let mut parsed_to = 0;
        let mut rescan = false;
        for (at, depth, opening) in openings {
            if at < parsed_to {
                continue;
            }
            let block = opaque_block(src, at, depth, opening, &mut masked);
            parsed_to = block.source.end;
            // A code span or fence opened inside an opaque extension may hide later headers.
            // Revisit the masked document if such a range extends past the closing marker.
            while let Some(span) = spans.next_if(|span| span.start < parsed_to) {
                rescan |= span.start >= at && span.end > parsed_to;
            }
            blocks.push(block);
        }
        source = Cow::Owned(String::from_utf8(masked).expect("Markdown sentinels replace entire UTF-8 substrings"));
        if !rescan {
            break;
        }
    }
    blocks.sort_unstable_by_key(|block| block.source.start);
    let mut parsed_to = 0;
    blocks.retain(|block| {
        if block.source.start < parsed_to {
            return false;
        }
        parsed_to = block.source.end;
        true
    });
    (source, blocks)
}
