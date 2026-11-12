//! Markdown into blocks: pulldown-cmark's events built into a tree of blocks with inline styles
//! as ranges, and where each top-level block starts (where a growing source parses again).

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// One block: a paragraph, a heading, a code block, a list, a quote, a rule or a table, with the
/// source it was parsed from.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: Kind,
    pub source: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Paragraph(Inline),
    Heading(u8, Inline),
    /// A code block: its language (the info string's first word) and its text, without the
    /// final line break.
    Code {
        lang: String,
        text: String,
    },
    /// A list: the first number of an ordered one.
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Quote(Vec<Block>),
    Rule,
    Table {
        align: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// A task item's box: checked or not.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// Text with styled ranges (byte offsets into `text`; they may nest and overlap).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inline {
    pub text: String,
    pub styles: Vec<(Range<usize>, Style)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Style {
    Strong,
    Emphasis,
    Strike,
    Code,
    Link(String),
}

/// Where the last inline run of a parse is: how far a repair of the source's end may reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// A paragraph or a list item's text: until a blank line.
    Para,
    /// A heading: its line.
    Line,
    /// A table cell: to the next `|` or the line's end.
    Cell,
}

/// A parse: the top-level blocks, the line each starts on, and what a repair of the source's end
/// needs (the last inline run, the source ranges shown as text).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    pub blocks: Vec<Block>,
    /// The start of each top-level block's first line.
    pub starts: Vec<usize>,
    /// The last block's inline run, if the source ends in one: its start and scope.
    pub tail: Option<(usize, Scope)>,
    /// Whether the source ends inside a code block.
    pub in_code: bool,
    /// Source ranges drawn as text (to check a repair shows no marker).
    pub shown: Vec<Range<usize>>,
}

enum Frame {
    Root,
    Quote(Vec<Block>),
    List { start: Option<u64>, items: Vec<Item> },
    Item { task: Option<bool>, blocks: Vec<Block> },
    Table { align: Vec<Align>, head: Vec<Inline>, rows: Vec<Vec<Inline>>, row: Vec<Inline> },
}

struct Run {
    scope: Scope,
    level: u8,
    inline: Inline,
    open: Vec<(usize, Style)>,
    at: usize,
}

struct Builder<'s> {
    src: &'s str,
    root: Vec<Block>,
    stack: Vec<Frame>,
    run: Option<Run>,
    code: Option<(String, String, usize)>,
    html: Option<(String, usize)>,
    out: Parsed,
}

pub fn options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS
}

/// Parses `src` whole; offsets are `src`'s plus `base`.
pub fn parse(src: &str, base: usize) -> Parsed {
    let mut b = Builder { src, root: Vec::new(), stack: vec![Frame::Root], run: None, code: None, html: None, out: Parsed::default() };
    for (event, range) in Parser::new_ext(src, options()).into_offset_iter() {
        b.event(event, range);
    }
    b.flush(src.len());
    let mut out = b.out;
    out.blocks = b.root;
    if base > 0 {
        out.shift(base);
    }
    out
}

impl Parsed {
    /// Moves every offset by `by` (a parse of a source's tail).
    fn shift(&mut self, by: usize) {
        fn blocks(bs: &mut [Block], by: usize) {
            for b in bs {
                b.source = b.source.start + by..b.source.end + by;
                match &mut b.kind {
                    Kind::List { items, .. } => items.iter_mut().for_each(|i| blocks(&mut i.blocks, by)),
                    Kind::Quote(q) => blocks(q, by),
                    _ => {}
                }
            }
        }
        blocks(&mut self.blocks, by);
        self.starts.iter_mut().for_each(|s| *s += by);
        if let Some((at, _)) = &mut self.tail {
            *at += by;
        }
        self.shown.iter_mut().for_each(|r| *r = r.start + by..r.end + by);
    }
}

fn line_start(src: &str, at: usize) -> usize {
    src[..at].rfind('\n').map_or(0, |i| i + 1)
}

impl Builder<'_> {
    fn push_block(&mut self, block: Block) {
        match self.stack.last_mut() {
            Some(Frame::Quote(bs)) | Some(Frame::Item { blocks: bs, .. }) => bs.push(block),
            _ => {
                self.out.starts.push(line_start(self.src, block.source.start));
                self.root.push(block);
            }
        }
    }

    /// The inline run text goes to (a tight list item's text has no paragraph: one is made).
    fn run(&mut self, at: usize) -> &mut Run {
        if self.run.is_none() {
            self.run = Some(Run { scope: Scope::Para, level: 0, inline: Inline::default(), open: Vec::new(), at });
        }
        self.run.as_mut().unwrap()
    }

    fn text(&mut self, s: &str, range: Range<usize>) {
        if let Some((_, code, _)) = &mut self.code {
            code.push_str(s);
            return;
        }
        if let Some((html, _)) = &mut self.html {
            html.push_str(s);
            return;
        }
        self.out.shown.push(range.clone());
        self.run(range.start).inline.text.push_str(s);
    }

    /// Ends the inline run at source offset `end`: a paragraph or heading block, or a table cell.
    fn flush(&mut self, end: usize) {
        let Some(mut run) = self.run.take() else { return };
        // The source's end is in the last run flushed: a repair of it may reach back to its start.
        self.out.tail = Some((run.at, run.scope));
        self.out.in_code = false;
        let len = run.inline.text.trim_end().len();
        run.inline.text.truncate(len);
        for (start, style) in run.open.drain(..).rev() {
            run.inline.styles.push((start.min(len)..len, style));
        }
        run.inline.styles.retain(|(r, _)| r.start < r.end.min(len));
        for (r, _) in &mut run.inline.styles {
            r.end = r.end.min(len);
        }
        if let Some(Frame::Table { row, .. }) = self.stack.last_mut()
            && run.scope == Scope::Cell
        {
            row.push(run.inline);
            return;
        }
        if run.inline.text.is_empty() {
            return;
        }
        let source = run.at..end;
        let kind = if run.scope == Scope::Line { Kind::Heading(run.level, run.inline) } else { Kind::Paragraph(run.inline) };
        self.push_block(Block { kind, source });
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(tag, range),
            Event::Text(t) => self.text(&t, range),
            Event::Code(t) => {
                // What shows is the content, not the backticks around it.
                let ticks = self.src[range.clone()].bytes().take_while(|b| *b == b'`').count();
                self.out.shown.push(range.start + ticks..range.end.saturating_sub(ticks).max(range.start + ticks));
                let run = self.run(range.start);
                let start = run.inline.text.len();
                run.inline.text.push_str(&t);
                let end = run.inline.text.len();
                run.inline.styles.push((start..end, Style::Code));
            }
            Event::Html(t) | Event::InlineHtml(t) => self.text(&t, range),
            Event::InlineMath(t) | Event::DisplayMath(t) => self.text(&t, range),
            Event::FootnoteReference(t) => self.text(&format!("[^{t}]"), range),
            Event::SoftBreak => self.text(" ", range),
            Event::HardBreak => self.text("\n", range),
            Event::Rule => {
                self.flush(range.start);
                self.out.tail = None;
                self.push_block(Block { kind: Kind::Rule, source: range.clone() });
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { task, .. }) = self.stack.last_mut() {
                    *task = Some(checked);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::BlockQuote(_)
            | Tag::List(_)
            | Tag::Table(_) => {
                // A tight item's text ends where a nested block starts.
                self.flush(range.start);
            }
            _ => {}
        }
        match tag {
            Tag::Paragraph => {
                self.run = Some(Run { scope: Scope::Para, level: 0, inline: Inline::default(), open: Vec::new(), at: range.start })
            }
            Tag::Heading { level, .. } => {
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.run = Some(Run { scope: Scope::Line, level, inline: Inline::default(), open: Vec::new(), at: range.start });
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split(|c: char| c.is_whitespace() || c == ',').next().unwrap_or("").to_owned(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new(), range.start));
            }
            Tag::HtmlBlock => self.html = Some((String::new(), range.start)),
            Tag::BlockQuote(_) => self.stack.push(Frame::Quote(Vec::new())),
            Tag::List(start) => self.stack.push(Frame::List { start, items: Vec::new() }),
            Tag::Item => {
                self.flush(range.start);
                self.stack.push(Frame::Item { task: None, blocks: Vec::new() });
            }
            Tag::Table(align) => {
                let align = align
                    .into_iter()
                    .map(|a| match a {
                        pulldown_cmark::Alignment::None => Align::None,
                        pulldown_cmark::Alignment::Left => Align::Left,
                        pulldown_cmark::Alignment::Center => Align::Center,
                        pulldown_cmark::Alignment::Right => Align::Right,
                    })
                    .collect();
                self.stack.push(Frame::Table { align, head: Vec::new(), rows: Vec::new(), row: Vec::new() });
            }
            Tag::TableCell => {
                self.run = Some(Run { scope: Scope::Cell, level: 0, inline: Inline::default(), open: Vec::new(), at: range.start });
            }
            Tag::Emphasis => self.open(Style::Emphasis, range.start),
            Tag::Strong => self.open(Style::Strong, range.start),
            Tag::Strikethrough => self.open(Style::Strike, range.start),
            Tag::Link { dest_url, .. } => self.open(Style::Link(dest_url.to_string()), range.start),
            _ => {}
        }
    }

    fn open(&mut self, style: Style, at: usize) {
        let run = self.run(at);
        let start = run.inline.text.len();
        run.open.push((start, style));
    }

    fn close(&mut self) {
        if let Some(run) = &mut self.run
            && let Some((start, style)) = run.open.pop()
        {
            let end = run.inline.text.len();
            if end > start {
                run.inline.styles.push((start..end, style));
            }
        }
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell => self.flush(range.end),
            TagEnd::CodeBlock => {
                if let Some((lang, mut text, _)) = self.code.take() {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.push_block(Block { kind: Kind::Code { lang, text }, source: range.clone() });
                }
            }
            TagEnd::HtmlBlock => {
                if let Some((html, _)) = self.html.take() {
                    let text = html.trim_end().to_owned();
                    if !text.is_empty() {
                        self.push_block(Block { kind: Kind::Paragraph(Inline { text, styles: Vec::new() }), source: range.clone() });
                    }
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush(range.end);
                if let Some(Frame::Quote(blocks)) = self.stack.pop() {
                    self.push_block(Block { kind: Kind::Quote(blocks), source: range.clone() });
                }
            }
            TagEnd::Item => {
                self.flush(range.end);
                if let Some(Frame::Item { task, blocks }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(Item { task, blocks });
                }
            }
            TagEnd::List(_) => {
                self.flush(range.end);
                if let Some(Frame::List { start, items, .. }) = self.stack.pop() {
                    self.push_block(Block { kind: Kind::List { start, items }, source: range.clone() });
                }
            }
            TagEnd::TableHead => {
                if let Some(Frame::Table { head, row, .. }) = self.stack.last_mut() {
                    *head = std::mem::take(row);
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table { rows, row, .. }) = self.stack.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::Table => {
                if let Some(Frame::Table { align, head, rows, .. }) = self.stack.pop() {
                    self.push_block(Block { kind: Kind::Table { align, head, rows }, source: range.clone() });
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => self.close(),
            _ => {}
        }
        match tag {
            TagEnd::CodeBlock => {
                self.out.tail = None;
                self.out.in_code = range.end >= self.src.len() && !fence_closed(&self.src[range]);
            }
            TagEnd::HtmlBlock => self.out.tail = None,
            _ => {}
        }
    }
}

/// Whether a fenced code block's source ends with its closing fence.
fn fence_closed(block: &str) -> bool {
    let block = block.trim_end_matches('\n');
    let first = block.lines().next().unwrap_or("").trim_start();
    let ch = first.chars().next().unwrap_or('`');
    if ch != '`' && ch != '~' {
        // An indented block closes with the source's lines.
        return true;
    }
    let n = first.chars().take_while(|c| *c == ch).count();
    let mut lines = block.lines();
    lines.next();
    lines.next_back().is_some_and(|l| {
        let l = l.trim();
        l.len() >= n && l.chars().all(|c| c == ch)
    })
}
