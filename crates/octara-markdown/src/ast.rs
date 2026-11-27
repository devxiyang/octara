//! Markdown into blocks: pulldown-cmark's events built into a tree of blocks with inline styles
//! as ranges, and where each top-level block starts (where a growing source parses again).

use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::ops::Range;
#[cfg(feature = "math")]
use std::sync::Arc;

mod extension;
mod spoiler;
#[cfg(test)]
mod tests;

/// Parsing extensions and how soft line breaks appear in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseOptions {
    pub tables: bool,
    pub strikethrough: bool,
    pub tasklists: bool,
    pub gfm_alerts: bool,
    pub math: bool,
    pub hard_breaks: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self { tables: true, strikethrough: true, tasklists: true, gfm_alerts: true, math: true, hard_breaks: false }
    }
}

impl ParseOptions {
    fn parser_options(self) -> Options {
        let mut options = Options::ENABLE_FOOTNOTES;
        options.set(Options::ENABLE_TABLES, self.tables);
        options.set(Options::ENABLE_STRIKETHROUGH, self.strikethrough);
        options.set(Options::ENABLE_TASKLISTS, self.tasklists);
        options.set(Options::ENABLE_GFM, self.gfm_alerts);
        options.set(Options::ENABLE_MATH, self.math);
        options
    }
}

/// A block with its byte range in the original Markdown source.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: Kind,
    pub source: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Paragraph(Inline),
    Heading(u8, Inline),
    /// A code block with the language, complete info string, and text without the final line break.
    Code {
        lang: String,
        info: String,
        text: String,
        is_complete: bool,
    },
    List {
        start: Option<u64>,
        tight: bool,
        items: Vec<Item>,
    },
    Quote {
        callout: Option<CalloutKind>,
        blocks: Vec<Block>,
    },
    Rule,
    Table {
        align: Vec<Align>,
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
    Image {
        url: String,
        alt: String,
        title: String,
    },
    Math {
        tex: String,
        is_complete: bool,
    },
    Directive {
        name: String,
        attrs: Vec<(String, String)>,
        body: String,
        is_complete: bool,
    },
    Video {
        src: String,
        poster: Option<String>,
    },
    Spoiler(Inline),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalloutKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// A task item's box: checked or not.
    pub task: Option<bool>,
    /// The three bytes of the task marker in the original source.
    pub task_source: Option<Range<usize>>,
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
    Image {
        url: String,
        title: String,
    },
    Math,
    /// Math with a source spelling that cannot be recovered from the normalized text.
    #[cfg(feature = "math")]
    MathSource {
        source: Arc<str>,
        display: bool,
        is_complete: bool,
    },
    FootnoteRef,
    Spoiler,
}

impl Style {
    pub(crate) fn is_math(&self) -> bool {
        match self {
            Self::Math => true,
            #[cfg(feature = "math")]
            Self::MathSource { .. } => true,
            _ => false,
        }
    }
}

/// Where the last inline run of a parse is: how far a repair of the source's end may reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
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
pub(crate) struct Parsed {
    pub blocks: Vec<Block>,
    /// The start of each top-level block's first line.
    pub starts: Vec<usize>,
    /// The last block's inline run, if the source ends in one: its start and scope.
    pub tail: Option<(usize, Scope)>,
    /// The source ends in an opaque block where inline repairs do not apply.
    pub opaque_tail: bool,
    /// Source ranges drawn as text (to check a repair shows no marker).
    pub shown: Vec<Range<usize>>,
    /// Recognized math spans that the repair must scan as a single unit.
    pub math: Vec<Range<usize>>,
    /// Complete links and images that need no inline repair.
    pub links: Vec<Range<usize>>,
    /// Raw delimiter ranges omitted when rendering spoilers.
    pub spoiler_markers: Vec<Range<usize>>,
}

enum Frame {
    Root,
    Quote { callout: Option<CalloutKind>, blocks: Vec<Block> },
    List { start: Option<u64>, tight: bool, items: Vec<Item> },
    Item { task: Option<bool>, task_source: Option<Range<usize>>, blocks: Vec<Block> },
    Table { align: Vec<Align>, head: Vec<Inline>, rows: Vec<Vec<Inline>>, row: Vec<Inline> },
}

struct Run {
    scope: Scope,
    level: u8,
    inline: Inline,
    open: Vec<(usize, Style)>,
    at: usize,
    media: Option<(Range<usize>, usize)>,
    pipes: Vec<(usize, usize)>,
}

impl Run {
    fn new(scope: Scope, level: u8, at: usize) -> Self {
        Self { scope, level, inline: Inline::default(), open: Vec::new(), at, media: None, pipes: Vec::new() }
    }
}

struct Code {
    info: String,
    text: String,
    fence: Option<(u8, usize)>,
    body_start: usize,
    text_end: usize,
}

struct Builder<'s> {
    src: &'s str,
    root: Vec<Block>,
    stack: Vec<Frame>,
    run: Option<Run>,
    code: Option<Code>,
    html: Option<String>,
    hard_breaks: bool,
    out: Parsed,
    extensions: std::iter::Peekable<std::vec::IntoIter<Block>>,
    preview_image: Option<usize>,
    in_preview_image: bool,
    #[cfg(feature = "math")]
    authored_len: usize,
}

/// Parses `src` whole; offsets are `src`'s plus `base`.
pub(crate) fn parse(src: &str, base: usize, options: ParseOptions) -> Parsed {
    let mut out = parse_preview(src, options, None, src.len());
    if base > 0 {
        out.shift(base);
    }
    out
}

pub(super) fn parse_preview(src: &str, options: ParseOptions, preview_image: Option<usize>, _authored_len: usize) -> Parsed {
    let (masked, extensions) = extension::prepare(src, options);
    let mut b = Builder {
        src,
        root: Vec::new(),
        stack: vec![Frame::Root],
        run: None,
        code: None,
        html: None,
        hard_breaks: options.hard_breaks,
        out: Parsed::default(),
        extensions: extensions.into_iter().peekable(),
        preview_image,
        in_preview_image: false,
        #[cfg(feature = "math")]
        authored_len: _authored_len,
    };
    for (event, range) in Parser::new_ext(&masked, options.parser_options()).into_offset_iter() {
        b.event(event, range);
    }
    b.flush(src.len());
    let mut out = b.out;
    out.blocks = b.root;
    out
}

impl Parsed {
    /// Moves every offset by `by` (a parse of a source's tail).
    pub(crate) fn shift(&mut self, by: usize) {
        adjust_ranges(&mut self.blocks, |r| r.start + by..r.end + by);
        self.starts.iter_mut().for_each(|s| *s += by);
        if let Some((at, _)) = &mut self.tail {
            *at += by;
        }
        self.shown
            .iter_mut()
            .chain(&mut self.math)
            .chain(&mut self.links)
            .chain(&mut self.spoiler_markers)
            .for_each(|r| *r = r.start + by..r.end + by);
    }

    pub(super) fn clamp_to_source(&mut self, end: usize) {
        adjust_ranges(&mut self.blocks, |r| r.start.min(end)..r.end.min(end));
    }
}

fn adjust_ranges(blocks: &mut [Block], adjust: impl Fn(Range<usize>) -> Range<usize> + Copy) {
    for block in blocks {
        block.source = adjust(block.source.clone());
        match &mut block.kind {
            Kind::List { items, .. } => {
                for item in items {
                    if let Some(range) = &mut item.task_source {
                        *range = adjust(range.clone());
                    }
                    adjust_ranges(&mut item.blocks, adjust);
                }
            }
            Kind::Quote { blocks, .. } => adjust_ranges(blocks, adjust),
            _ => {}
        }
    }
}

pub(crate) fn line_start(src: &str, at: usize) -> usize {
    memchr::memrchr2(b'\r', b'\n', &src.as_bytes()[..at]).map_or(0, |i| i + 1)
}

/// A blank line immediately before a block or item at the same quote depth.
fn blank_before(src: &str, at: usize) -> bool {
    let start = line_start(src, at);
    let before = &src[..start];
    let Some(before) = before.strip_suffix("\r\n").or_else(|| before.strip_suffix(['\r', '\n'])) else { return false };
    let mut previous = before.rsplit(['\r', '\n']).next().unwrap_or("").trim_start();
    let mut prefix = src[start..at].trim_start();
    while let Some(rest) = prefix.strip_prefix('>') {
        let Some(line) = previous.strip_prefix('>') else { return false };
        prefix = rest.trim_start();
        previous = line.trim_start();
    }
    previous.is_empty()
}

impl Builder<'_> {
    fn push_block(&mut self, block: Block) {
        if let [.., Frame::List { tight, .. }, Frame::Item { blocks, .. }] = self.stack.as_mut_slice()
            && !blocks.is_empty()
            && blank_before(self.src, block.source.start)
        {
            *tight = false;
        }
        match self.stack.last_mut() {
            Some(Frame::Quote { blocks, .. } | Frame::Item { blocks, .. }) => blocks.push(block),
            _ => {
                self.out.starts.push(line_start(self.src, block.source.start));
                self.root.push(block);
            }
        }
    }

    /// The inline run text goes to (a tight list item's text has no paragraph: one is made).
    fn run(&mut self, at: usize) -> &mut Run {
        self.run.get_or_insert_with(|| Run::new(Scope::Para, 0, at))
    }

    fn text(&mut self, s: &str, range: Range<usize>) {
        if let Some(code) = &mut self.code {
            code.text.push_str(s);
            code.text_end = code.text_end.max(range.end);
            return;
        }
        if let Some(html) = &mut self.html {
            html.push_str(s);
            return;
        }
        self.out.shown.push(range.clone());
        let src = self.src;
        let run = self.run(range.start);
        spoiler::record_pipes(src, s, &range, run.inline.text.len(), &mut run.pipes);
        run.inline.text.push_str(s);
    }

    fn styled(&mut self, text: &str, style: Style, range: Range<usize>, delimiters: usize, standalone: bool) {
        let start = range.start + delimiters;
        self.out.shown.push(start..range.end.saturating_sub(delimiters).max(start));
        let run = self.run(range.start);
        let start = run.inline.text.len();
        run.inline.text.push_str(text);
        if standalone {
            run.media = Some((range, run.inline.styles.len()));
        }
        run.inline.styles.push((start..run.inline.text.len(), style));
    }

    /// Ends the inline run at source offset `end`: a paragraph or heading block, or a table cell.
    fn flush(&mut self, end: usize) {
        let Some(mut run) = self.run.take() else { return };
        // The source's end is in the last run flushed: a repair of it may reach back to its start.
        self.out.tail = Some((run.at, run.scope));
        self.out.opaque_tail = false;
        if run.scope == Scope::Para
            && let Some((range, index)) = run.media
            && self.src[run.at..range.start].trim().is_empty()
            && self.src[range.end..end].trim().is_empty()
        {
            let (_, style) = run.inline.styles.swap_remove(index);
            let kind = match style {
                Style::Image { url, title } => Kind::Image { url, title, alt: run.inline.text },
                style if style.is_math() => Kind::Math { tex: run.inline.text, is_complete: true },
                _ => unreachable!(),
            };
            self.push_block(Block { kind, source: run.at..end });
            return;
        }
        let len = run.inline.text.trim_end().len();
        run.inline.text.truncate(len);
        for (start, style) in run.open.drain(..).rev() {
            run.inline.styles.push((start.min(len)..len, style));
        }
        run.inline.styles.retain(|(r, style)| matches!(style, Style::Image { .. }) || r.start < r.end.min(len));
        for (r, _) in &mut run.inline.styles {
            r.start = r.start.min(len);
            r.end = r.end.min(len);
        }
        let spoilers = spoiler::apply(&mut run.inline, &run.pipes, &mut self.out.spoiler_markers);
        if let Some(Frame::Table { row, .. }) = self.stack.last_mut()
            && run.scope == Scope::Cell
        {
            row.push(run.inline);
            return;
        }
        if run.inline.text.is_empty() && run.inline.styles.is_empty() {
            return;
        }
        let source = run.at..end;
        #[cfg(feature = "media")]
        if run.scope == Scope::Para
            && let Some((src, poster)) = crate::media::parse_video(&self.src[source.clone()])
        {
            self.out.tail = None;
            self.out.opaque_tail = true;
            self.push_block(Block { kind: Kind::Video { src, poster }, source });
            return;
        }
        let kind = match run.scope {
            Scope::Line => Kind::Heading(run.level, run.inline),
            Scope::Para
                if spoilers == 1
                    && run
                        .inline
                        .styles
                        .iter()
                        .any(|(r, style)| *style == Style::Spoiler && r.start == 0 && r.end == run.inline.text.len()) =>
            {
                Kind::Spoiler(run.inline)
            }
            _ => Kind::Paragraph(run.inline),
        };
        self.push_block(Block { kind, source });
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(tag, range),
            Event::Text(t) => self.text(&t, range),
            Event::Code(t) => {
                let ticks = self.src[range.clone()].bytes().take_while(|b| *b == b'`').count();
                self.styled(&t, Style::Code, range, ticks, false);
            }
            Event::Html(t) | Event::InlineHtml(t) => self.text(&t, range),
            Event::InlineMath(t) => {
                self.out.math.push(range.clone());
                self.math(&t, range, 1);
            }
            Event::DisplayMath(t) => {
                self.out.math.push(range.clone());
                self.math(&t, range, 2);
            }
            Event::FootnoteReference(t) => self.styled(&format!("[^{t}]"), Style::FootnoteRef, range, 0, false),
            Event::SoftBreak => self.text(if self.hard_breaks { "\n" } else { " " }, range),
            Event::HardBreak => self.text("\n", range),
            Event::Rule => {
                self.flush(range.start);
                self.out.tail = None;
                if !self.emit_extension(range.start) {
                    self.out.opaque_tail = false;
                    self.push_block(Block { kind: Kind::Rule, source: range });
                }
            }
            Event::TaskListMarker(checked) => {
                if let Some(Frame::Item { task, task_source, .. }) = self.stack.last_mut() {
                    *task = Some(checked);
                    *task_source = Some(range.end - 3..range.end);
                }
            }
        }
    }

    fn math(&mut self, tex: &str, range: Range<usize>, delimiters: usize) {
        let style = Style::Math;
        #[cfg(feature = "math")]
        let style = {
            let is_complete = range.end <= self.authored_len;
            let raw = &self.src[range.start.min(self.authored_len)..range.end.min(self.authored_len)];
            if delimiters == 2 || raw.strip_prefix('$').and_then(|raw| raw.strip_suffix('$')) != Some(tex) {
                Style::MathSource { source: raw.into(), display: delimiters == 2, is_complete }
            } else {
                style
            }
        };
        self.styled(tex, style, range, delimiters, delimiters == 2);
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
                // Pulldown emits explicit paragraphs only for loose list items.
                if let [.., Frame::List { tight, .. }, Frame::Item { .. }] = self.stack.as_mut_slice() {
                    *tight = false;
                }
                self.run = Some(Run::new(Scope::Para, 0, range.start));
            }
            Tag::Heading { level, .. } => {
                if self.emit_extension(range.start) {
                    return;
                }
                let level = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.run = Some(Run::new(Scope::Line, level, range.start));
            }
            Tag::CodeBlock(kind) => {
                let (info, fence) = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let fence = self.src.as_bytes()[range.start];
                        let len = self.src.as_bytes()[range.start..].iter().take_while(|&&ch| ch == fence).count();
                        (info.into_string(), Some((fence, len)))
                    }
                    CodeBlockKind::Indented => (String::new(), None),
                };
                let body_start = extension::line_end(self.src, range.start);
                self.code = Some(Code { info, text: String::new(), fence, body_start, text_end: 0 });
            }
            Tag::HtmlBlock => self.html = Some(String::new()),
            Tag::BlockQuote(kind) => {
                let callout = kind.map(|kind| match kind {
                    BlockQuoteKind::Note => CalloutKind::Note,
                    BlockQuoteKind::Tip => CalloutKind::Tip,
                    BlockQuoteKind::Important => CalloutKind::Important,
                    BlockQuoteKind::Warning => CalloutKind::Warning,
                    BlockQuoteKind::Caution => CalloutKind::Caution,
                });
                self.stack.push(Frame::Quote { callout, blocks: Vec::new() });
            }
            Tag::List(start) => self.stack.push(Frame::List { start, tight: true, items: Vec::new() }),
            Tag::Item => {
                self.flush(range.start);
                // A list with only non-paragraph blocks can still be loose.
                if let Some(Frame::List { tight, items, .. }) = self.stack.last_mut()
                    && !items.is_empty()
                    && blank_before(self.src, range.start)
                {
                    *tight = false;
                }
                self.stack.push(Frame::Item { task: None, task_source: None, blocks: Vec::new() });
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
                self.run = Some(Run::new(Scope::Cell, 0, range.start));
            }
            Tag::Emphasis => self.open(Style::Emphasis, range.start),
            Tag::Strong => self.open(Style::Strong, range.start),
            Tag::Strikethrough => self.open(Style::Strike, range.start),
            Tag::Link { dest_url, .. } => self.open(Style::Link(dest_url.to_string()), range.start),
            Tag::Image { dest_url, title, .. } => {
                self.in_preview_image |= self.preview_image == Some(range.start);
                self.open(Style::Image { url: dest_url.into_string(), title: title.into_string() }, range.start);
            }
            _ => {}
        }
    }

    fn open(&mut self, style: Style, at: usize) {
        let run = self.run(at);
        let start = run.inline.text.len();
        run.open.push((start, style));
    }

    fn close(&mut self, range: Range<usize>) {
        if let Some(run) = &mut self.run
            && let Some((start, style)) = run.open.pop()
        {
            let image = matches!(style, Style::Image { .. });
            if matches!(style, Style::Link(_) | Style::Image { .. }) {
                if self.in_preview_image {
                    // The temporary destination lets CommonMark parse the alt text, which
                    // remains visible without mounting media or links inside the image.
                    self.in_preview_image = self.preview_image != Some(range.start);
                    return;
                }
                // CommonMark flattens links and images within an image's alt text.
                if run.open.iter().any(|(_, opening)| matches!(opening, Style::Image { .. })) {
                    return;
                }
                self.out.links.push(range.clone());
            }
            if image && run.open.is_empty() {
                run.media = Some((range, run.inline.styles.len()));
            }
            if let Style::Image { url, .. } = &style
                && run.inline.text.len() == start
                && run.open.iter().any(|(_, opening)| matches!(opening, Style::Link(_)))
            {
                run.inline.text.push_str(url);
            }
            let end = run.inline.text.len();
            if end > start || image {
                run.inline.styles.push((start..end, style));
            }
        }
    }

    fn emit_extension(&mut self, at: usize) -> bool {
        if self.extensions.peek().is_none_or(|block| block.source.start != at) {
            return false;
        }
        self.out.tail = None;
        self.out.opaque_tail = true;
        let block = self.extensions.next().unwrap();
        self.push_block(block);
        true
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::TableCell => self.flush(range.end),
            TagEnd::CodeBlock => {
                if let Some(Code { info, mut text, fence, body_start, text_end }) = self.code.take() {
                    let lang = info.split(|c: char| c.is_whitespace() || c == ',').next().unwrap_or("").to_owned();
                    if text.contains('\r') {
                        text = text.replace("\r\n", "\n").replace('\r', "\n");
                    }
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    let is_complete =
                        fence.is_none_or(|(fence, len)| fence_closed(self.src, body_start.max(text_end), range.end, fence, len));
                    self.push_block(Block { kind: Kind::Code { lang, info, text, is_complete }, source: range });
                }
                self.out.tail = None;
                self.out.opaque_tail = true;
            }
            TagEnd::HtmlBlock => {
                if let Some(html) = self.html.take() {
                    let text = html.trim_end().to_owned();
                    if !text.is_empty() {
                        #[cfg(feature = "media")]
                        let video = crate::media::parse_video(&text);
                        #[cfg(not(feature = "media"))]
                        let video: Option<(String, Option<String>)> = None;
                        let kind = match video {
                            Some((src, poster)) => Kind::Video { src, poster },
                            None => Kind::Paragraph(Inline { text, styles: Vec::new() }),
                        };
                        self.push_block(Block { kind, source: range });
                    }
                }
                self.out.tail = None;
                self.out.opaque_tail = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush(range.end);
                if let Some(Frame::Quote { callout, blocks }) = self.stack.pop() {
                    self.push_block(Block { kind: Kind::Quote { callout, blocks }, source: range.clone() });
                }
            }
            TagEnd::Item => {
                self.flush(range.end);
                if let Some(Frame::Item { task, task_source, blocks }) = self.stack.pop()
                    && let Some(Frame::List { items, .. }) = self.stack.last_mut()
                {
                    items.push(Item { task, task_source, blocks });
                }
            }
            TagEnd::List(_) => {
                self.flush(range.end);
                if let Some(Frame::List { start, tight, items }) = self.stack.pop() {
                    self.push_block(Block { kind: Kind::List { start, tight, items }, source: range.clone() });
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
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link | TagEnd::Image => self.close(range),
            _ => {}
        }
    }
}

/// The parser emits the fence separately from its text. Use only the bytes after the body.
fn fence_closed(src: &str, after_text: usize, end: usize, fence: u8, len: usize) -> bool {
    let Some(tail) = src.get(after_text..end) else { return false };
    let prefix = tail.trim_start_matches([' ', '\t', '\r', '\n', '>']);
    let markers = prefix.bytes().take_while(|&ch| ch == fence).count();
    markers >= len && prefix[markers..].trim().is_empty()
}
