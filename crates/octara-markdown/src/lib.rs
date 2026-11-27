//! Selectable CommonMark/GFM documents with incremental streaming and scoped renderers.

mod ast;
#[cfg(feature = "composer")]
mod composer;
#[cfg(feature = "highlight")]
mod highlight;
#[cfg(feature = "math")]
mod math;
#[cfg(feature = "media")]
mod media;
mod pills;
mod render;
mod renderer;
mod slots;
mod store;
mod stream;
mod theme;

pub use ast::{Align, Block, CalloutKind, Inline, Item, Kind, ParseOptions, Style};
#[cfg(feature = "composer")]
pub use composer::{Completion, MarkdownComposer, TriggerConfig, TriggerQuery};
#[cfg(feature = "highlight")]
pub use highlight::*;
#[cfg(feature = "math")]
pub use math::{BuiltinMathRenderer, MathError, MathErrorHandler, MathLayout, MathRenderStyle, MathRenderer};
pub use pills::{LinkPillContent, LinkVariantRule, LinkVariantRules, RecognizedLink, TextRecognizer};
pub use render::{
    RenderBlock, default_block, default_code_block, default_directive, default_heading, default_image, default_list, default_math,
    default_paragraph, default_quote, default_rule, default_spoiler, default_style_inline, default_table, default_video,
};
pub use renderer::{CodeHandler, ContextMenuHandler, ImageHandler, LinkHandler, MarkdownConfig, MarkdownRenderer, TaskToggle};
pub use slots::{BlockPlacement, BlockSlot, BlockSlotRenderer, CodeInfo, DirectiveAttrs, SlotContent, SlotId, SlotRecord};
pub use store::{AstOverlay, CollectedBlocks, DocumentStore, FinishedBlocks, ImageOccurrence, LinkOccurrence, ParsedDocument};
pub use stream::{
    InlineRepair, StreamingEffects, StreamingFilter, StreamingPacer, StreamingPolicy, finished_blocks_end, may_define_link_reference,
};
pub use theme::{
    CalloutStyle, CalloutStyles, CodeBlockStyle, HeadingStyle, InlineCodeStyle, LinkStyle, ListStyle, MarkdownLocale, MarkdownTheme,
    MathStyle, QuoteStyle, RuleStyle, TableStyle,
};

use octara::prelude::*;
use render::{BlockCache, BlockKey, BlockRef, RenderContext};
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::{Arc, OnceLock};
use std::task::Poll;
use std::time::Duration;

type TextReader = dyn FnMut(&mut ReadCx<'_>) -> String;

/// Static text, a tracked text computation, or a streaming buffer.
pub enum Source {
    Text(String),
    Dynamic(Box<TextReader>),
    Buffer(TextBuffer),
}

#[derive(Clone, Copy)]
enum ReadSource {
    Text(ReadSignal<String>),
    Buffer(TextBuffer),
}

impl ReadSource {
    fn with<T>(self, read: &mut ReadCx<'_>, project: impl FnOnce(&str) -> T) -> T {
        match self {
            Self::Text(text) => read.with(text, |text| project(text)),
            Self::Buffer(buffer) => read.with_text(buffer, project),
        }
    }
}

pub trait IntoMarkdown {
    fn into_markdown(self) -> Source;
}

impl IntoMarkdown for &str {
    fn into_markdown(self) -> Source {
        Source::Text(self.to_owned())
    }
}

impl IntoMarkdown for String {
    fn into_markdown(self) -> Source {
        Source::Text(self)
    }
}

impl IntoMarkdown for &String {
    fn into_markdown(self) -> Source {
        self.as_str().into_markdown()
    }
}

impl IntoMarkdown for TextBuffer {
    fn into_markdown(self) -> Source {
        Source::Buffer(self)
    }
}

impl IntoMarkdown for &TextBuffer {
    fn into_markdown(self) -> Source {
        (*self).into_markdown()
    }
}

impl<F: FnMut(&mut ReadCx<'_>) -> String + 'static> IntoMarkdown for F {
    fn into_markdown(self) -> Source {
        Source::Dynamic(Box::new(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamingOptions {
    pub filter: StreamingFilter,
    pub effects: StreamingEffects,
    /// Batches recent input into words and table rows. Set to false to render
    /// source edits in the same turn while keeping the streaming filter and reveal.
    pub pace: bool,
}

impl Default for StreamingOptions {
    fn default() -> Self {
        Self { filter: StreamingFilter::default(), effects: StreamingEffects::None, pace: true }
    }
}

/// Formats offered to the document's selection context menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmartCopy {
    Plain,
    Markdown,
    TableMarkdown,
    ImageUrl,
}

impl From<SmartCopy> for CopyFormat {
    fn from(format: SmartCopy) -> Self {
        match format {
            SmartCopy::Plain => Self::Plain,
            SmartCopy::Markdown => Self::Markdown,
            SmartCopy::TableMarkdown => Self::Custom("TableMarkdown".into()),
            SmartCopy::ImageUrl => Self::Custom("ImageUrl".into()),
        }
    }
}

fn scoped_renderer(build: &BuildCx<'_>) -> Arc<dyn MarkdownRenderer> {
    static DEFAULT: OnceLock<Arc<renderer::DocumentRenderer>> = OnceLock::new();
    build.context::<Arc<dyn MarkdownRenderer>>().unwrap_or_else(|| DEFAULT.get_or_init(|| Arc::new(renderer::DocumentRenderer)).clone())
}

/// Renders a document with its scoped renderer. Buffers render immediately and
/// repair unfinished spans while they grow.
#[ui]
#[track_caller]
pub fn markdown(build: &mut BuildCx<'_>, source: impl IntoMarkdown) -> El {
    let renderer = scoped_renderer(build);
    markdown_document(build, source.into_markdown(), renderer, None, StreamingFilter::default(), None)
}

/// Renders a document with an explicit renderer.
#[ui]
#[track_caller]
pub fn markdown_with(build: &mut BuildCx<'_>, source: impl IntoMarkdown, config: impl MarkdownRenderer) -> El {
    // Renderer callbacks belong to the UI scope.
    #[allow(clippy::arc_with_non_send_sync)]
    let renderer = Arc::new(config);
    markdown_document(build, source.into_markdown(), renderer, None, StreamingFilter::default(), None)
}

/// Renders a streaming buffer. Setting `finished` drains any paced text and switches
/// the document to its final parse. Splicing a paced source starts a new pacing session.
#[ui]
#[track_caller]
pub fn markdown_stream(build: &mut BuildCx<'_>, source: TextBuffer, finished: ReadSignal<bool>, options: StreamingOptions) -> El {
    let renderer = scoped_renderer(build);
    let (buffer, finished) = if options.pace { pace(build, source, finished) } else { (source, finished) };
    markdown_document(build, Source::Buffer(buffer), renderer, Some(finished), options.filter, Some(options.effects))
}

enum StreamEdit {
    Append(String),
    Replace(String),
}

fn pace(build: &mut BuildCx<'_>, source: TextBuffer, finished: ReadSignal<bool>) -> (TextBuffer, ReadSignal<bool>) {
    let (initial, mut cursor, initial_finished) = build.read(|r| (r.text(source), r.text_cursor(source), r.get(finished)));
    let initial_pacer = StreamingPacer::from_visible(&initial);
    let output = build.text_buffer(initial);
    let drained = build.signal(initial_finished);
    let (send, mut receive) = channel::<(StreamEdit, bool, Duration)>();
    build.watch(
        move |read| (read.text_cursor(source), read.get(finished)),
        move |write, &(_, finished), _| {
            let changes = write.read(|r| r.text_changes(source, &mut cursor));
            let edit = match changes {
                Changes::Delta(changes) if changes.iter().all(|edit| matches!(edit, TextBufferEdit::Append(_))) => {
                    let mut text = String::new();
                    for change in changes {
                        if let TextBufferEdit::Append(part) = change {
                            text.push_str(&part);
                        }
                    }
                    StreamEdit::Append(text)
                }
                _ => StreamEdit::Replace(write.text(source)),
            };
            let _ = send.send((edit, finished, write.now()));
        },
    );
    build.spawn("pace Markdown", move |task| async move {
        let mut pacer = initial_pacer;
        let mut done = initial_finished;
        loop {
            let next = if pacer.has_pending() {
                let mut next = pin!(receive.recv());
                let mut deadline = pin!(task.sleep(Duration::from_millis(8)));
                poll_fn(|cx| {
                    if let Poll::Ready(input) = next.as_mut().poll(cx) {
                        return Poll::Ready(Some(input));
                    }
                    deadline.as_mut().poll(cx).map(|()| None)
                })
                .await
            } else {
                Some(receive.recv().await)
            };
            match next {
                Some(Some((edit, finished, at))) => {
                    match edit {
                        StreamEdit::Append(text) => pacer.push(&text, at),
                        StreamEdit::Replace(text) => {
                            pacer = StreamingPacer::from_visible(&text);
                            task.write(|w| {
                                let len = w.with_text(output, str::len);
                                w.splice_text(output, TextBufferEdit::Splice { range: 0..len, insert: text });
                            });
                        }
                    }
                    done = finished;
                    if done {
                        pacer.finish(at);
                    } else {
                        pacer.cancel_finish();
                    }
                }
                Some(None) => break,
                None => {}
            }
            let chunk = pacer.tick(task.now());
            if task
                .write(|w| {
                    if let Some(chunk) = chunk {
                        w.append_text(output, &chunk);
                    }
                    w.set_if_changed(drained, done && !pacer.has_pending());
                })
                .is_none()
            {
                break;
            }
        }
    });
    (output, drained.read_only())
}

fn markdown_document(
    build: &mut BuildCx<'_>,
    source: Source,
    renderer: Arc<dyn MarkdownRenderer>,
    completion: Option<ReadSignal<bool>>,
    filter: StreamingFilter,
    effects: Option<StreamingEffects>,
) -> El {
    let theme_renderer = renderer.clone();
    let theme = build.memo(move |r| theme_renderer.theme(r));
    let source = match source {
        Source::Text(text) => ReadSource::Text(build.signal(text).read_only()),
        Source::Dynamic(read) => ReadSource::Text(build.memo(read)),
        Source::Buffer(buffer) => ReadSource::Buffer(buffer),
    };
    let streaming = matches!(source, ReadSource::Buffer(_));
    let finished = completion.unwrap_or_else(|| build.signal(!streaming).read_only());
    let mut doc = DocumentStore::new(streaming, renderer.parse_options()).with_streaming_filter(filter);
    if let Some(recognizer) = renderer.text_recognizer() {
        doc = doc.with_text_recognizer(recognizer.clone());
    }
    let mut cache = Cache { doc, blocks: BlockCache::default(), announced: 0, heard: String::new(), overlay: None };
    let mut cursor = Cursor::default();
    let rendered = build.memo(move |r| {
        let appended = if let ReadSource::Buffer(buffer) = source
            && let Changes::Delta(edits) = r.text_changes(buffer, &mut cursor)
        {
            !edits.is_empty() && edits.iter().all(|edit| matches!(edit, TextBufferEdit::Append(_)))
        } else {
            false
        };
        let finished = r.get(finished);
        source.with(r, |text| cache.update(text, finished, appended))
    });
    let overlay = build.memo(move |r| r.with(rendered, |rendered| rendered.overlay.clone()));
    let effects = if streaming { effects.unwrap_or_else(|| renderer.streaming_effects()) } else { StreamingEffects::None };
    let context = RenderContext::new(
        renderer.clone(),
        theme,
        overlay,
        effects,
        finished,
        #[cfg(feature = "math")]
        math::MathDocument::new(build),
    );
    let blocks = build.derive_list(move |r| r.with(rendered, |rendered| rendered.blocks.clone()));
    let announcement = streaming
        .then(|| el().size(0.0).live(Live::Polite).label(move |r: &mut ReadCx<'_>| r.with(rendered, |rendered| rendered.heard.clone())));
    let copy_markdown = move |read: &mut ReadCx<'_>, copied: &[CopiedText]| {
        let selection = read.with(rendered, |rendered| source_selection(rendered, copied))?;
        source.with(read, |source| match selection {
            SourceSelection::Whole => Some(source.to_owned()),
            SourceSelection::Blocks { start, end } => source.get(start..end.unwrap_or(source.len())).map(|text| text.trim_end().to_owned()),
        })
    };
    let document = col()
        .role(Role::Document)
        .selectable(true)
        .copy_selection_with(copy_markdown)
        .copy_format(SmartCopy::Markdown.into(), copy_markdown)
        .copy_format(SmartCopy::TableMarkdown.into(), move |read, copied| {
            let range = read.with(rendered, |rendered| {
                let block = single_selected_block(rendered, copied)?;
                matches!(block.kind, Kind::Table { .. }).then(|| block.source.clone())
            })?;
            source.with(read, |text| text.get(range).map(|table| table.trim_end().to_owned()))
        })
        .copy_format(SmartCopy::ImageUrl.into(), move |read, copied| {
            if !copied.first()?.whole_row {
                return None;
            }
            read.with(rendered, |rendered| {
                let block = single_selected_block(rendered, copied)?;
                let mut images = rendered.overlay.images_in(block);
                let image = images.next()?;
                images.next().is_none().then(|| image.url.to_string())
            })
        })
        .min_w(0)
        .style_with(move |r, p| {
            r.with(theme, |theme| {
                let p = p.gap(theme.block_gap).text_size(theme.text_size).text_color(theme.text_color);
                match theme.line_height {
                    Some(height) => p.line_height(height),
                    None => p,
                }
            })
        })
        .child(each(blocks, move |b, _, block| render::block_view(b, block, context.clone())))
        .children(announcement);
    let document = if renderer.on_link().is_some() {
        let renderer = renderer.clone();
        document.on_link(move |w, link| renderer.on_link().is_some_and(|handle| handle(w, link)))
    } else {
        document
    };
    if renderer.on_context_menu().is_some() {
        document.on_context_menu(move |w, event| renderer.on_context_menu().is_some_and(|handle| handle(w, event)))
    } else {
        document
    }
}

/// Plain text and source Markdown for an unmounted document.
pub struct RowText {
    pub text: String,
    pub markdown: String,
}

impl DocumentStore {
    pub fn row_text(&self) -> RowText {
        RowText { text: block_text(self.blocks().iter().map(Arc::as_ref)), markdown: self.source().to_owned() }
    }
}

fn block_text<'a>(blocks: impl Iterator<Item = &'a Block>) -> String {
    blocks.map(plain).filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n\n")
}

fn plain(block: &Block) -> String {
    match &block.kind {
        Kind::Paragraph(i) | Kind::Heading(_, i) => visible_inline(i),
        Kind::Spoiler(_) => "Spoiler".to_owned(),
        Kind::Code { text, .. } => text.clone(),
        Kind::Directive { body, .. } => body.clone(),
        Kind::List { items, .. } => items.iter().map(|i| block_text(i.blocks.iter())).collect::<Vec<_>>().join("\n"),
        Kind::Quote { blocks, .. } => block_text(blocks.iter()),
        Kind::Image { alt, .. } => alt.clone(),
        Kind::Math { tex, .. } => tex.clone(),
        Kind::Video { src, .. } => src.clone(),
        Kind::Rule => String::new(),
        Kind::Table { head, rows, .. } => std::iter::once(head)
            .chain(rows)
            .map(|row| row.iter().map(visible_inline).collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn visible_inline(inline: &Inline) -> String {
    let mut ranges = inline.styles.iter().filter_map(|(range, style)| matches!(style, Style::Spoiler).then_some(range)).collect::<Vec<_>>();
    ranges.sort_unstable_by_key(|range| range.start);
    let mut text = String::new();
    let mut at = 0;
    for range in ranges {
        if range.end > at {
            if range.start >= at {
                text.push_str(&inline.text[at..range.start]);
                text.push_str("Spoiler");
            }
            at = range.end;
        }
    }
    text.push_str(&inline.text[at..]);
    text
}

#[derive(PartialEq)]
struct Rendered {
    blocks: Vec<(BlockKey, BlockRef)>,
    starts: Vec<usize>,
    overlay: Arc<AstOverlay>,
    heard: String,
}

struct Cache {
    doc: DocumentStore,
    blocks: BlockCache,
    announced: usize,
    heard: String,
    overlay: Option<Arc<AstOverlay>>,
}

impl Cache {
    fn update(&mut self, source: &str, finished: bool, appended: bool) -> Rendered {
        let old_bytes = self.doc.parsed_bytes();
        let old_parses = self.doc.full_parses();
        self.doc.update_with_completion(source, appended, finished);
        if self.overlay.is_none() || old_bytes != self.doc.parsed_bytes() || old_parses != self.doc.full_parses() {
            self.overlay = Some(Arc::new(self.doc.overlay()));
        }
        let overlay = self.overlay.as_ref().expect("initialized document overlay");
        let done = if finished { self.doc.blocks().len() } else { self.doc.blocks().len().saturating_sub(1) };
        if (appended || finished) && done > self.announced {
            self.heard = self.doc.blocks()[self.announced..done].iter().map(|block| plain(block)).collect::<Vec<_>>().join("\n");
        } else if !appended {
            self.heard.clear();
        }
        self.announced = done;
        Rendered {
            blocks: self.blocks.update(self.doc.blocks(), self.doc.shared_leading_blocks().len()),
            starts: self.doc.starts().to_vec(),
            overlay: overlay.clone(),
            heard: self.heard.clone(),
        }
    }
}

enum SourceSelection {
    Whole,
    Blocks { start: usize, end: Option<usize> },
}

fn block_index(rendered: &Rendered, copied: &CopiedText) -> Option<usize> {
    copied.rows.first()?.strip_prefix('b')?.parse().ok().filter(|&i| i < rendered.blocks.len())
}

fn single_selected_block<'a>(rendered: &'a Rendered, copied: &[CopiedText]) -> Option<&'a Block> {
    let first = block_index(rendered, copied.first()?)?;
    let last = block_index(rendered, copied.last()?)?;
    (first == last).then(|| &*rendered.blocks[first].1)
}

/// Default clipboard behavior uses source for whole blocks and the host's
/// plain selection for a partial selection within one block.
fn source_selection(rendered: &Rendered, copied: &[CopiedText]) -> Option<SourceSelection> {
    let (start, end) = (copied.first()?, copied.last()?);
    let (first, last) = (block_index(rendered, start)?, block_index(rendered, end)?);
    if first == last && !start.whole_row {
        return None;
    }
    if first == 0 && last == rendered.blocks.len() - 1 && start.whole_row && end.whole_row {
        return Some(SourceSelection::Whole);
    }
    let starts = &rendered.starts;
    Some(SourceSelection::Blocks { start: starts[first], end: starts.get(last + 1).copied() })
}
