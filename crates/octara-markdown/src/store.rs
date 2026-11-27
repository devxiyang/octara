//! Incremental parsing and document-order metadata kept outside the parsed AST.

use crate::ast::{Block, Inline, Kind, ParseOptions, Style, parse};
use crate::pills::{RecognizedLink, TextRecognizer};
use crate::slots::{BlockPlacement, CodeInfo, SlotContent, SlotId, SlotRecord};
use crate::stream::{StreamingFilter, finished_blocks_end, may_define_link_reference, parse_stream};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

/// An immutable, independently owned document snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedDocument {
    pub blocks: Arc<[Arc<Block>]>,
    pub starts: Arc<[usize]>,
    pub collected: CollectedBlocks,
}

/// Borrowed view of an unchanged prefix. Constructing it never walks the blocks.
#[derive(Clone, Copy, Debug)]
pub struct FinishedBlocks<'a> {
    pub blocks: &'a [Arc<Block>],
    pub starts: &'a [usize],
}

impl FinishedBlocks<'_> {
    pub fn len(self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(self) -> bool {
        self.blocks.is_empty()
    }
}

pub struct DocumentStore {
    source: String,
    blocks: Vec<Arc<Block>>,
    starts: Vec<usize>,
    options: ParseOptions,
    filter: StreamingFilter,
    streaming: bool,
    finished: bool,
    has_reference: bool,
    boundary: usize,
    shared: usize,
    full_parses: u64,
    parsed_bytes: u64,
    overlay: OverlayBuilder,
    snapshot: OnceLock<ParsedDocument>,
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new(false, ParseOptions::default())
    }
}

impl DocumentStore {
    pub fn new(streaming: bool, options: ParseOptions) -> Self {
        Self {
            source: String::new(),
            blocks: Vec::new(),
            starts: Vec::new(),
            options,
            filter: StreamingFilter::default(),
            streaming,
            finished: false,
            has_reference: false,
            boundary: 0,
            shared: 0,
            full_parses: 0,
            parsed_bytes: 0,
            overlay: OverlayBuilder::default(),
            snapshot: OnceLock::new(),
        }
    }

    pub fn with_streaming_filter(mut self, filter: StreamingFilter) -> Self {
        if self.filter != filter {
            self.filter = filter;
            self.refresh();
        }
        self
    }

    pub fn with_text_recognizer(mut self, recognizer: TextRecognizer) -> Self {
        self.overlay = OverlayBuilder { recognizer, ..OverlayBuilder::default() };
        self.overlay.extend(
            &self.blocks,
            0,
            #[cfg(feature = "math")]
            &self.source,
        );
        self.snapshot.take();
        self
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn blocks(&self) -> &[Arc<Block>] {
        &self.blocks
    }

    pub fn starts(&self) -> &[usize] {
        &self.starts
    }

    /// End of the finished prefix that can survive an append.
    pub fn finished_end(&self) -> usize {
        self.boundary
    }

    pub fn shared_leading_blocks(&self) -> FinishedBlocks<'_> {
        FinishedBlocks { blocks: &self.blocks[..self.shared], starts: &self.starts[..self.shared] }
    }

    pub fn full_parses(&self) -> u64 {
        self.full_parses
    }

    pub fn parsed_bytes(&self) -> u64 {
        self.parsed_bytes
    }

    /// Materializes a flat snapshot on demand. Clones of that snapshot share every allocation.
    pub fn snapshot(&self) -> &ParsedDocument {
        self.snapshot.get_or_init(|| ParsedDocument {
            blocks: Arc::from(self.blocks.as_slice()),
            starts: Arc::from(self.starts.as_slice()),
            collected: self.overlay.collected(),
        })
    }

    pub fn overlay(&self) -> AstOverlay {
        AstOverlay(Arc::new(OverlayData {
            blocks: Arc::from(self.overlay.blocks.as_slice()),
            recognizer: self.overlay.recognizer.clone(),
            finished_end: if self.finished || !self.streaming {
                self.source.len()
            } else {
                self.blocks.last().map_or(self.boundary, |block| self.boundary.max(block.source.start))
            },
        }))
    }

    pub fn update(&mut self, source: &str) {
        self.update_with_completion(source, false, self.finished);
    }

    /// Appends a fragment without comparing the already stored source prefix.
    pub fn append(&mut self, suffix: &str) {
        if suffix.is_empty() {
            self.shared = self.blocks.len();
            return;
        }
        self.has_reference |= (self.source.ends_with(']') && suffix.starts_with(':')) || may_define_link_reference(suffix);
        self.source.push_str(suffix);
        self.reparse(if self.has_reference { 0 } else { self.boundary });
    }

    /// Switches between progressive filtering and the final source parse.
    pub fn set_finished(&mut self, finished: bool) {
        if self.finished != finished {
            self.finished = finished;
            self.reparse(if self.has_reference { 0 } else { self.boundary });
        }
    }

    pub(crate) fn update_with_completion(&mut self, source: &str, appended: bool, finished: bool) {
        let old_len = self.source.len();
        if !appended && source == self.source {
            self.shared = self.blocks.len();
            self.set_finished(finished);
            return;
        }
        let appended = appended || (source.len() > old_len && source.starts_with(&self.source));
        self.finished = finished;
        if appended {
            self.append(&source[old_len..]);
            return;
        }
        self.has_reference = may_define_link_reference(source);
        self.source.clear();
        self.source.push_str(source);
        self.reparse(0);
    }

    fn refresh(&mut self) {
        if !self.source.is_empty() {
            self.reparse(0);
        }
    }

    fn reparse(&mut self, cut: usize) {
        let kept = self.starts.partition_point(|&at| at < cut);
        self.shared = kept;
        self.full_parses += u64::from(cut == 0);
        self.parsed_bytes += (self.source.len() - cut) as u64;
        let tail = &self.source[cut..];
        let parsed = if self.streaming && !self.finished {
            parse_stream(tail, cut, self.options, self.filter)
        } else {
            parse(tail, cut, self.options)
        };
        // Reuse an equal block even when a reference definition forces a full parse.
        let mut appended = Vec::with_capacity(parsed.blocks.len());
        for (offset, block) in parsed.blocks.into_iter().enumerate() {
            let block = match self.blocks.get(kept + offset) {
                Some(old) if **old == block => old.clone(),
                _ => Arc::new(block),
            };
            appended.push(block);
        }
        self.blocks.truncate(kept);
        self.blocks.extend(appended);
        self.starts.truncate(kept);
        self.starts.extend(parsed.starts);
        self.overlay.extend(
            &self.blocks,
            kept,
            #[cfg(feature = "math")]
            &self.source,
        );
        self.boundary = finished_blocks_end(&self.source, &self.blocks, &self.starts);
        self.snapshot.take();
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CollectedBlocks {
    pub images: Arc<[ImageOccurrence]>,
    pub links: Arc<[LinkOccurrence]>,
    pub slots: Arc<[SlotRecord]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageOccurrence {
    pub id: SlotId,
    pub url: Arc<str>,
    pub alt: Arc<str>,
    pub title: Option<Arc<str>>,
    pub occurrence_index: usize,
    pub placement: BlockPlacement,
    pub block: Range<usize>,
    /// Position of an inline occurrence in its block, in cell order for tables.
    pub inline: Option<(usize, Range<usize>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkOccurrence {
    pub url: Arc<str>,
    pub label: Arc<str>,
    pub block: Range<usize>,
    pub recognized: bool,
}

struct OverlayData {
    blocks: Arc<[Arc<BlockEntries>]>,
    recognizer: TextRecognizer,
    finished_end: usize,
}

/// Side data for immutable Markdown blocks. Keeping an overlay also keeps the
/// matching image/slot occurrence numbering for its particular document version.
#[derive(Clone)]
pub struct AstOverlay(Arc<OverlayData>);

impl PartialEq for AstOverlay {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl AstOverlay {
    fn entries(&self, block: &Block) -> Option<&BlockEntries> {
        let i = self.0.blocks.partition_point(|entry| entry.source.start <= block.source.start);
        self.0.blocks.get(i.checked_sub(1)?).map(Arc::as_ref)
    }

    pub fn slot(&self, block: &Block) -> Option<&SlotRecord> {
        let entry = self.entries(block)?;
        entry.slot_index.get(&block.source).map(|&index| &entry.slots[index])
    }

    #[cfg(feature = "math")]
    pub(crate) fn math_source(&self, block: &Block) -> Option<&Arc<str>> {
        let sources = &self.entries(block)?.math_sources;
        let index = sources.binary_search_by_key(&block.source.start, |(start, _)| *start).ok()?;
        Some(&sources[index].1)
    }

    pub fn images_in<'a>(&'a self, block: &'a Block) -> impl Iterator<Item = &'a ImageOccurrence> {
        self.entries(block).into_iter().flat_map(|entry| entry.images.iter()).filter(move |image| image.block == block.source)
    }

    pub fn links(&self, inline: &Inline) -> Vec<RecognizedLink> {
        auto_links(&self.0.recognizer, inline)
    }

    /// Recognized links stored for an authored inline run in this document version.
    pub fn recognized_for(&self, block: &Block, run: usize) -> Arc<[RecognizedLink]> {
        self.entries(block).and_then(|entry| entry.recognized.get(&(block.source.clone(), run))).cloned().unwrap_or_default()
    }

    pub fn is_finished(&self, block: &Block) -> bool {
        block.source.end <= self.0.finished_end
    }
}

fn auto_links(recognizer: &TextRecognizer, inline: &Inline) -> Vec<RecognizedLink> {
    let mut links = recognizer.recognize(&inline.text);
    if links.is_empty() {
        return links;
    }
    let mut excluded = inline
        .styles
        .iter()
        .filter_map(|(range, style)| {
            (style.is_math() || matches!(style, Style::Link(_) | Style::Image { .. } | Style::Code | Style::Spoiler)).then_some(range)
        })
        .collect::<Vec<_>>();
    excluded.sort_unstable_by_key(|range| range.start);
    let mut ranges = excluded.into_iter().peekable();
    links.retain(|link| {
        while ranges.peek().is_some_and(|range| range.end <= link.range.start) {
            ranges.next();
        }
        ranges.peek().is_none_or(|range| range.start >= link.range.end)
    });
    links
}

#[derive(Default)]
struct OverlayBuilder {
    recognizer: TextRecognizer,
    blocks: Vec<Arc<BlockEntries>>,
    counts: HashMap<Counter, usize>,
    images: usize,
}

#[derive(Default)]
struct BlockEntries {
    source: Range<usize>,
    slots: Vec<SlotRecord>,
    slot_index: HashMap<Range<usize>, usize>,
    #[cfg(feature = "math")]
    math_sources: Vec<(usize, Arc<str>)>,
    recognized: HashMap<(Range<usize>, usize), Arc<[RecognizedLink]>>,
    links: Vec<LinkOccurrence>,
    images: Vec<ImageOccurrence>,
    counters: Vec<Counter>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Counter {
    kind: &'static str,
    hash: u64,
}

impl OverlayBuilder {
    fn extend(&mut self, blocks: &[Arc<Block>], kept: usize, #[cfg(feature = "math")] source: &str) {
        for block in self.blocks.drain(kept..) {
            self.images -= block.images.len();
            for counter in &block.counters {
                let count = self.counts.get_mut(counter).expect("collected occurrence");
                *count -= 1;
                if *count == 0 {
                    self.counts.remove(counter);
                }
            }
        }
        for block in &blocks[kept..] {
            let mut out = BlockEntries { source: block.source.clone(), ..BlockEntries::default() };
            self.visit(
                &mut out,
                block,
                BlockPlacement::Root,
                #[cfg(feature = "math")]
                source,
            );
            self.blocks.push(Arc::new(out));
        }
    }

    fn collected(&self) -> CollectedBlocks {
        CollectedBlocks {
            images: self.blocks.iter().flat_map(|block| block.images.iter().cloned()).collect(),
            links: self.blocks.iter().flat_map(|block| block.links.iter().cloned()).collect(),
            slots: self.blocks.iter().flat_map(|block| block.slots.iter().cloned()).collect(),
        }
    }

    fn visit(&mut self, out: &mut BlockEntries, block: &Block, placement: BlockPlacement, #[cfg(feature = "math")] source: &str) {
        let content = match &block.kind {
            Kind::Code { lang, info, text, is_complete } => SlotContent::Code {
                info: CodeInfo { lang: lang.as_str().into(), info: info.as_str().into() },
                code: text.as_str().into(),
                is_complete: *is_complete,
            },
            Kind::Math { tex, is_complete } => {
                #[cfg(feature = "math")]
                if let Some(raw) = source.get(block.source.clone()).map(str::trim_start)
                    && raw.starts_with("$$")
                {
                    let raw = if *is_complete { raw.trim_end() } else { raw };
                    out.math_sources.push((block.source.start, raw.into()));
                }
                SlotContent::Math { tex: tex.as_str().into(), is_complete: *is_complete }
            }
            Kind::Directive { name, attrs, body, is_complete } => SlotContent::Directive {
                name: name.as_str().into(),
                attrs: attrs.iter().map(|(key, value)| (key.as_str().into(), value.as_str().into())).collect(),
                body: body.as_str().into(),
                is_complete: *is_complete,
            },
            Kind::Video { src, poster } => SlotContent::Video { src: src.as_str().into(), poster: poster.as_deref().map(Arc::from) },
            Kind::Image { url, alt, title } => SlotContent::Image {
                url: url.as_str().into(),
                alt: alt.as_str().into(),
                title: title_value(title),
                occurrence_index: self.images,
            },
            Kind::Paragraph(inline) => {
                self.inline(out, block, placement, 0, inline);
                let Some(url) = standalone_link(inline) else { return };
                SlotContent::StandaloneLink { url: url.into(), label: inline.text.as_str().into() }
            }
            Kind::Heading(_, inline) | Kind::Spoiler(inline) => {
                self.inline(out, block, placement, 0, inline);
                return;
            }
            Kind::Table { head, rows, .. } => {
                let width = std::iter::once(head).chain(rows).map(Vec::len).max().unwrap_or_default();
                for (row, cells) in std::iter::once(head).chain(rows).enumerate() {
                    for (column, cell) in cells.iter().enumerate() {
                        self.inline(out, block, placement, row * width + column, cell);
                    }
                }
                return;
            }
            Kind::Quote { callout, blocks } => {
                let placement = callout.map_or(BlockPlacement::Blockquote, BlockPlacement::Callout);
                for child in blocks {
                    self.visit(
                        out,
                        child,
                        placement,
                        #[cfg(feature = "math")]
                        source,
                    );
                }
                return;
            }
            Kind::List { items, .. } => {
                for child in items.iter().flat_map(|item| &item.blocks) {
                    self.visit(
                        out,
                        child,
                        placement,
                        #[cfg(feature = "math")]
                        source,
                    );
                }
                return;
            }
            Kind::Rule => return,
        };
        let id = self.id(out, &content);
        if let SlotContent::Image { url, alt, title, occurrence_index } = &content {
            out.images.push(ImageOccurrence {
                id: id.clone(),
                url: url.clone(),
                alt: alt.clone(),
                title: title.clone(),
                occurrence_index: *occurrence_index,
                placement,
                block: block.source.clone(),
                inline: None,
            });
            self.images += 1;
        }
        out.slot_index.insert(block.source.clone(), out.slots.len());
        out.slots.push(SlotRecord { id, placement, content });
    }

    fn inline(&mut self, out: &mut BlockEntries, block: &Block, placement: BlockPlacement, run: usize, inline: &Inline) {
        let recognized = auto_links(&self.recognizer, inline);
        let mut links = recognized.iter().map(|link| (link.range.clone(), link.url.clone(), true)).collect::<Vec<_>>();
        if !recognized.is_empty() {
            out.recognized.insert((block.source.clone(), run), recognized.into());
        }
        for (range, style) in &inline.styles {
            if let Style::Link(url) = style {
                links.push((range.clone(), url.as_str().into(), false));
            }
        }
        links.sort_by_key(|(range, _, _)| range.start);
        for (range, url, recognized) in links {
            if let Some(label) = inline.text.get(range) {
                out.links.push(LinkOccurrence { url, label: label.into(), block: block.source.clone(), recognized });
            }
        }
        for (range, style) in inline_images(inline) {
            let Style::Image { url, title } = style else { continue };
            let url: Arc<str> = url.as_str().into();
            let alt: Arc<str> = inline.text.get(range.clone()).unwrap_or_default().into();
            let title = title_value(title);
            let occurrence_index = self.images;
            let id = self.id(out, &SlotContent::Image { url: url.clone(), alt: alt.clone(), title: title.clone(), occurrence_index });
            out.images.push(ImageOccurrence {
                id,
                url,
                alt,
                title,
                occurrence_index,
                placement,
                block: block.source.clone(),
                inline: Some((run, range.clone())),
            });
            self.images += 1;
        }
    }

    fn id(&mut self, out: &mut BlockEntries, content: &SlotContent) -> SlotId {
        let counter = counter(content);
        let occurrence = self.counts.entry(counter).or_default();
        let id = SlotId(format!("{}:{:016x}:{occurrence}", counter.kind, counter.hash).into());
        *occurrence += 1;
        out.counters.push(counter);
        id
    }
}

fn title_value(title: &str) -> Option<Arc<str>> {
    (!title.is_empty()).then(|| title.into())
}

/// The outer image owns its entire alt string, including any nested image alt text.
pub(crate) fn inline_images(inline: &Inline) -> Vec<(&Range<usize>, &Style)> {
    let mut images = inline
        .styles
        .iter()
        .enumerate()
        .filter(|(_, (range, style))| matches!(style, Style::Image { .. }) && inline.text.get(range.clone()).is_some())
        .collect::<Vec<_>>();
    // Styles close inside-out. Order equal ranges with the outer closing style first.
    images.sort_unstable_by_key(|(index, (range, _))| {
        let close_order = if range.is_empty() { *index } else { usize::MAX - *index };
        (range.start, Reverse(range.end), close_order)
    });
    let mut outer: Option<&Range<usize>> = None;
    images
        .into_iter()
        .filter_map(|(_, (range, style))| {
            if outer.is_some_and(|outer| range.start < outer.end) {
                return None;
            }
            outer = Some(range);
            Some((range, style))
        })
        .collect()
}

pub(super) fn standalone_link(inline: &Inline) -> Option<&str> {
    if inline.styles.iter().any(|(_, style)| matches!(style, Style::Image { .. })) {
        return None;
    }
    inline.styles.iter().find_map(|(range, style)| match style {
        Style::Link(url) if *range == (0..inline.text.len()) => Some(url.as_str()),
        _ => None,
    })
}

fn counter(content: &SlotContent) -> Counter {
    let mut hash = 0xcbf29ce484222325u64;
    let mut field = |text: &str| {
        // Length delimiters distinguish adjacent fields and embedded delimiters.
        for byte in (text.len() as u64).to_le_bytes().into_iter().chain(text.bytes()) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    };
    let kind = match content {
        SlotContent::Code { info, .. } => {
            field(&info.info);
            "code"
        }
        SlotContent::Math { .. } => "math",
        SlotContent::Directive { name, attrs, .. } => {
            field(name);
            for (key, value) in attrs.iter() {
                field(key);
                field(value);
            }
            "directive"
        }
        SlotContent::Image { url, alt, title, .. } => {
            field(url);
            field(alt);
            field(title.as_deref().unwrap_or_default());
            "image"
        }
        SlotContent::StandaloneLink { url, label } => {
            field(url);
            field(label);
            "link"
        }
        SlotContent::Video { src, poster } => {
            field(src);
            field(poster.as_deref().unwrap_or_default());
            "video"
        }
    };
    Counter { kind, hash }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_shares_the_ast_and_overlays_of_the_finished_prefix() {
        let mut store = DocumentStore::new(true, ParseOptions::default());
        let mut source = "[link](/old)\n\n".repeat(2048);
        source.push_str("last paragraph\n");
        store.update(&source);
        let before = store.snapshot().clone();
        let old_overlay = store.overlay();
        let bytes = store.parsed_bytes();
        store.append("extended text");

        let shared = store.shared_leading_blocks();
        assert_eq!(shared.len(), 2048);
        assert!(store.parsed_bytes() - bytes < 64);
        assert!(std::ptr::eq(shared.blocks.as_ptr(), store.blocks().as_ptr()));
        for (old, current) in before.blocks.iter().zip(shared.blocks) {
            assert!(Arc::ptr_eq(old, current));
        }
        let new_overlay = store.overlay();
        assert!(std::ptr::eq(old_overlay.slot(&before.blocks[1]).unwrap(), new_overlay.slot(&before.blocks[1]).unwrap()));
    }

    #[test]
    fn reference_definition_split_at_the_colon_revisits_the_prefix() {
        let mut store = DocumentStore::default();
        store.update("[hello][label]\n\nfoo\n\n[label]");
        let before = store.snapshot().clone();
        store.update("[hello][label]\n\nfoo\n\n[label]: /resolved\n");
        let after = store.snapshot();
        assert!(before.collected.links.is_empty());
        assert_eq!(after.collected.links[0].url.as_ref(), "/resolved");
        assert!(!Arc::ptr_eq(&before.blocks[0], &after.blocks[0]));
    }

    #[test]
    fn slots_and_media_indexes_follow_nested_document_order_without_modifying_ast() {
        let mut store = DocumentStore::new(false, ParseOptions::default())
            .with_text_recognizer(TextRecognizer::new().issues(|label| Some(format!("issue:{}", &label[1..]))));
        store
            .update("[linked](/first)\n\n> [!NOTE]\n> ![alt](/a)\n>\n> ```rust\n> one\n> ```\n\ninline ![tiny](/a) and #128\n\n![alt](/a)");
        let snapshot = store.snapshot();
        let overlay = store.overlay();
        assert_eq!(snapshot.collected.links.iter().map(|link| &*link.url).collect::<Vec<_>>(), ["/first", "issue:128"]);
        assert_eq!(snapshot.collected.images.len(), 3);
        assert_eq!(snapshot.collected.images[0].placement, BlockPlacement::Callout(crate::CalloutKind::Note));
        assert!(snapshot.collected.images[1].inline.is_some());
        assert_eq!(snapshot.collected.images[2].occurrence_index, 2);
        assert_ne!(snapshot.collected.images[0].id, snapshot.collected.images[2].id);
        assert_eq!(
            snapshot.collected.slots.iter().map(|slot| slot.id.0.split(':').next().unwrap()).collect::<Vec<_>>(),
            ["link", "image", "code", "image",]
        );
        let block = &snapshot.blocks[2];
        let Kind::Paragraph(inline) = &block.kind else { panic!("expected inline image paragraph") };
        assert!(!inline.styles.iter().any(|(_, s)| matches!(s, Style::Link(_))));
        assert_eq!(overlay.links(inline)[0].url.as_ref(), "issue:128");
    }

    #[test]
    fn overlapping_alt_spans_collect_only_the_rendered_outer_image() {
        let mut store = DocumentStore::default();
        store.update("before ![foo ![bar](/inner)](/outer) after ![](/empty) ![](/last)");
        let images = &store.snapshot().collected.images;
        assert_eq!(images.iter().map(|image| image.url.as_ref()).collect::<Vec<_>>(), ["/outer", "/empty", "/last"]);
        assert_eq!(images[0].alt.as_ref(), "foo bar");
    }

    #[test]
    fn slot_ids_ignore_growing_bodies_and_fence_closures() {
        for (opening, content, close) in [("```rust\n", "fn x() {}\n", "```"), ("$$\n", "x^2\n", "$$"), (":::think\n", "step one\n", ":::")]
        {
            let mut store = DocumentStore::new(true, ParseOptions::default());
            let mut text = opening.to_owned();
            text.push_str(content);
            store.update(&text);
            let before = store.snapshot().clone();
            text.push_str("more\n");
            store.update(&text);
            assert_eq!(before.collected.slots[0].id, store.snapshot().collected.slots[0].id);
            text.push_str(close);
            store.update(&text);
            assert_eq!(before.collected.slots[0].id, store.snapshot().collected.slots[0].id);
            match &store.snapshot().collected.slots[0].content {
                SlotContent::Code { is_complete, .. }
                | SlotContent::Math { is_complete, .. }
                | SlotContent::Directive { is_complete, .. } => {
                    assert!(*is_complete);
                }
                slot => panic!("unexpected slot {slot:?}"),
            }
        }
    }
}
