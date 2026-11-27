use octara::prelude::*;
use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};
use std::{ops::Range, sync::Arc};

pub(super) struct MarkdownBackend;

impl EditorBackend for MarkdownBackend {
    fn rich_text(&self, source: &str) -> Option<EditorRichText> {
        Some(presentation(source))
    }
}

fn presentation(source: &str) -> EditorRichText {
    let mut decorations = Vec::new();
    let mut attachments = Vec::new();
    let mut atoms = Vec::new();
    let mut literal_depth = 0usize;
    for (event, range) in Parser::new(source).into_offset_iter() {
        match event {
            Event::Start(Tag::Strong) => decorations.push((range, RichSpan { weight: Some(700), ..Default::default() })),
            Event::Start(Tag::Emphasis) => decorations.push((range, RichSpan { italic: true, ..Default::default() })),
            Event::Start(Tag::CodeBlock(_)) => {
                literal_depth += 1;
                decorations.push((range, code()));
            }
            Event::Code(_) => decorations.push((range, code())),
            Event::Start(Tag::Link { link_type, dest_url, .. }) => {
                literal_depth += 1;
                if let LinkType::Inline = link_type
                    && let Some(kind) = atom_kind(&dest_url)
                    && let Some(label) = label_range(source, &range)
                {
                    attachments.push(hidden(range.start..label.start));
                    let mut bytes = source[label.clone()].bytes().enumerate().peekable();
                    while let Some((offset, ch)) = bytes.next() {
                        if ch == b'\\' && bytes.peek().is_some_and(|(_, next)| next.is_ascii_punctuation()) {
                            let byte = label.start + offset;
                            attachments.push(hidden(byte..byte + 1));
                            bytes.next();
                        }
                    }
                    attachments.push(hidden(label.end..range.end));
                    decorations.push((label, pill()));
                    atoms.push(AtomicRange { range, kind: Arc::from(kind) });
                } else {
                    decorations.push((range, link()));
                }
            }
            Event::Start(Tag::Image { .. }) => literal_depth += 1,
            Event::End(TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock) => literal_depth -= 1,
            Event::Text(_) if literal_depth == 0 => {
                decorations.extend(bare_links(&source[range.clone()]).map(|url| (range.start + url.start..range.start + url.end, link())));
            }
            _ => {}
        }
    }
    EditorRichText { decorations: decorations.into(), attachments: attachments.into(), atoms: atoms.into() }
}

pub(super) fn literal_at(source: &str, byte: usize) -> bool {
    Parser::new(source).into_offset_iter().any(|(event, range)| {
        if !range.contains(&byte) {
            return false;
        }
        match event {
            Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::Start(Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. }) => true,
            Event::Text(_) => bare_links(&source[range.clone()]).any(|link| link.contains(&(byte - range.start))),
            _ => false,
        }
    })
}

fn bare_links(text: &str) -> impl Iterator<Item = Range<usize>> {
    let mut scanned = 0;
    text.match_indices("http").filter_map(move |(start, _)| {
        if start < scanned || text[..start].ends_with(|ch: char| ch.is_alphanumeric() || ch == '_') {
            return None;
        }
        let tail = &text[start..];
        let url = tail.strip_prefix("https://").or_else(|| tail.strip_prefix("http://"))?;
        let rest = url.split(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`')).next()?;
        let scheme = tail.len() - url.len();
        scanned = start + scheme + rest.len();
        let rest = rest.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let end = rest.trim_end_matches([')', ']', '}']).len();
        (end > 0).then_some(start..start + scheme + end)
    })
}

fn atom_kind(destination: &str) -> Option<&str> {
    let (scheme, path) = destination.split_once("://")?;
    if path.is_empty()
        || !scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
        || !scheme.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
    {
        return None;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "ftp" | "file" => None,
        "user" => Some("mention"),
        "channel" => Some("channel"),
        "command" => Some("command"),
        _ => Some(scheme),
    }
}

fn label_range(source: &str, link: &Range<usize>) -> Option<Range<usize>> {
    let label_start = link.start + 1;
    let mut nested = 0;
    let mut bytes = source.get(label_start..link.end)?.bytes().enumerate();
    while let Some((offset, byte)) = bytes.next() {
        match byte {
            b'\\' => {
                bytes.next();
            }
            b'[' => nested += 1,
            b']' if nested == 0 => {
                let end = label_start + offset;
                return (end > label_start && source.as_bytes().get(end + 1) == Some(&b'(')).then_some(label_start..end);
            }
            b']' => nested -= 1,
            _ => {}
        }
    }
    None
}

fn code() -> RichSpan {
    RichSpan { mono: true, background: Some(ColorToken::SurfacePressed.into()), ..Default::default() }
}

fn link() -> RichSpan {
    RichSpan { color: Some(ColorToken::Accent.into()), underline: true, ..Default::default() }
}

fn pill() -> RichSpan {
    RichSpan {
        weight: Some(600),
        background: Some(ColorToken::SurfacePressed.into()),
        color: Some(ColorToken::Accent.into()),
        padding_x: Some(3.0),
        radius: Some(4.0),
        ..Default::default()
    }
}

fn hidden(range: Range<usize>) -> InlineAttachment {
    InlineAttachment {
        range,
        size: Size::default(),
        align: InlineAlign::Baseline,
        baseline: None,
        content: InlineContent::Spacer { width: 0.0 },
    }
}
