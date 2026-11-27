//! Native font selection, paragraph shaping, and text projection.

mod cache;
pub(crate) mod geometry;
mod inline;
mod presentation;

use self::cache::Cache;
use crate::layout::LayoutEngine;
use octara_model::geometry::{Align, Point, Rect, Size};
use octara_model::paint::{CanvasText, FontFace, FontId, Glyph, GlyphRun, Item, Quad};
use octara_model::semantics::TextClip;
use octara_model::text::{InlineAttachment, RichSpan, TextAlign, TextGeometry, TextLine, TextStyle, Wrap};
use octara_model::theme::{Color, ColorToken};
use octara_tree::{NodeId, RetiredNode, Tree, TreeDelta};
use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, GenericFamily, Script, SourceCache};
use parley::{Alignment, AlignmentOptions, FontContext, FontWeight, Layout, LayoutContext, StyleProperty};
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use unicode_segmentation::UnicodeSegmentation;

const BUNDLED: [&[u8]; 4] = [
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
];
const CACHE_BUDGET: usize = 32 << 20;
const INPUT_WIDTH: f32 = 160.0;
const TABULAR_NUMS: [parley::FontFeature; 1] = [parley::FontFeature::new(parley::setting::Tag::from_bytes(*b"tnum"), 1)];

struct Shape {
    layout: Layout<u16>,
    inline: Vec<inline::Box>,
    broken: Option<(Option<f32>, bool)>,
}

type SharedShape = Rc<RefCell<Shape>>;

impl Shape {
    fn at(&mut self, width: Option<f32>, justify: bool) -> &Self {
        if self.broken != Some((width, justify)) {
            let alignment = if justify { Alignment::Justify } else { Alignment::Left };
            self.layout.set_alignment(alignment, AlignmentOptions::default());
            self.layout.break_all_lines(width);
            self.broken = Some((width, justify));
        }
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ShapeStyle {
    family: &'static str,
    font_size: f32,
    line_height: f32,
    weight: u16,
    italic: bool,
    underline: bool,
    strike: bool,
    letter_spacing: f32,
    tabular_nums: bool,
}

impl From<TextStyle> for ShapeStyle {
    fn from(style: TextStyle) -> Self {
        Self {
            family: style.family,
            font_size: style.font_size,
            line_height: style.line_height,
            weight: style.weight,
            italic: style.italic,
            underline: style.underline,
            strike: style.strike,
            letter_spacing: style.letter_spacing,
            tabular_nums: style.tabular_nums,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Span {
    range: Range<usize>,
    brush: u16,
    family: Option<&'static str>,
    size: Option<f32>,
    weight: Option<u16>,
    italic: bool,
    underline: bool,
    strike: bool,
    inset: f32,
}

struct CachedShape {
    text: Arc<str>,
    style: ShapeStyle,
    spans: Vec<Span>,
    shape: SharedShape,
    used: u64,
}

struct Paragraph {
    start: usize,
    end: usize,
    next: usize,
    shape: SharedShape,
}

struct Document {
    paragraphs: Vec<Paragraph>,
    style: TextStyle,
    offsets: Vec<(usize, usize)>,
    placeholder: bool,
}

impl Document {
    fn byte(&self, display: usize) -> usize {
        if self.placeholder {
            return 0;
        }
        if self.offsets.is_empty() {
            return display;
        }
        let i = self.offsets.partition_point(|&(at, _)| at <= display).saturating_sub(1);
        self.offsets[i].1
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Source {
    Text(*const str, u64),
    Editor { revision: u64, composing: bool, secure: bool, revealed: bool, multiline: bool, placeholder: Arc<str> },
}

#[derive(Clone, PartialEq)]
struct Key {
    source: Source,
    style: TextStyle,
    mono: &'static str,
    font_generation: u64,
}

#[derive(Clone)]
struct TextCache {
    key: Key,
    document: Rc<Document>,
    presentation: Option<(f32, Rc<Presentation>)>,
}

#[derive(Default)]
struct Presentation {
    lines: Arc<[TextLine]>,
    glyphs: Vec<Run>,
    decorations: Vec<Decoration>,
    padding: Vec<SpanEdge>,
    attachments: Vec<(usize, Rect)>,
    size: Size,
    clip: Option<TextClip>,
}

struct SpanEdge {
    brush: u16,
    anchor: usize,
    rect: Rect,
}

struct Decoration {
    brush: u16,
    rect: Rect,
}

impl Presentation {
    fn range_rects(&self, range: &Range<usize>, brush: Option<u16>) -> Vec<Rect> {
        let mut rects = geometry::range_rects(&self.lines, range);
        let edges = self.padding.iter().filter(|edge| range.contains(&edge.anchor) && brush.is_none_or(|brush| edge.brush == brush));
        let count = rects.len();
        rects.extend(edges.map(|edge| edge.rect));
        if count != rects.len() {
            geometry::merge_inline_rects(&mut rects);
        }
        rects
    }

    fn span_rects(&self, range: &Range<usize>, brush: u16, padding_y: f32) -> impl Iterator<Item = Rect> + '_ {
        self.range_rects(range, Some(brush)).into_iter().filter_map(move |rect| {
            let index = self.lines.binary_search_by(|line| line.rect.y.total_cmp(&rect.y)).ok()?;
            let line = &self.lines[index];
            let top = (line.glyph_top - padding_y).max(rect.y);
            let bottom = (line.glyph_top + line.glyph_height + padding_y).min(rect.bottom());
            (bottom > top).then(|| Rect::new(rect.x, top, rect.w, bottom - top))
        })
    }
}

#[derive(Clone)]
struct Run {
    font: FontFace,
    size: f32,
    coords: Vec<i16>,
    embolden: bool,
    skew: f32,
    brush: u16,
    glyphs: Arc<[Glyph]>,
    offset: Point,
}

impl Run {
    fn item(&self, source: NodeId, color: Color, origin: Point) -> Item {
        Item::Glyphs(GlyphRun {
            source,
            font: self.font.clone(),
            size: self.size,
            coords: self.coords.clone(),
            embolden: self.embolden,
            skew: self.skew,
            color,
            colors: None,
            origin: Point::new(origin.x + self.offset.x, origin.y + self.offset.y),
            glyphs: self.glyphs.clone(),
        })
    }
}

struct Projected {
    paint: PaintKey,
    origin: Point,
}

#[derive(Clone)]
pub(crate) struct PaintKey {
    presentation: Rc<Presentation>,
    local_origin: Point,
    clip: Option<Rect>,
    caret: Option<Rect>,
}

impl PartialEq for PaintKey {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.presentation, &other.presentation)
            && self.local_origin == other.local_origin
            && self.clip == other.clip
            && self.caret == other.caret
    }
}

/// Cumulative native text work, including work done in candidate frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextWork {
    pub font_generation: u64,
    pub shapes_built: u64,
    pub shape_cache_hits: u64,
    pub documents_built: u64,
    pub presentations_built: u64,
}

pub(crate) struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<u16>,
    work: TextWork,
    faces: FxHashMap<(u64, u32), FontFace>,
    shapes: FxHashMap<u64, Vec<CachedShape>>,
    shape_bytes: usize,
    tick: u64,
    measures: Cache<TextCache>,
    documents: Cache<TextCache>,
    projected: Cache<Projected>,
    links: Cache<Vec<Rect>>,
    changed: FxHashSet<NodeId>,
    changed_links: FxHashSet<NodeId>,
    needs_projection: bool,
    before_stage: Option<(bool, u64)>,
}

impl TextEngine {
    pub(crate) fn new(system_fonts: bool) -> Self {
        let system_fonts = system_fonts && cfg!(feature = "system-fonts");
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts });
        for data in BUNDLED {
            collection.register_fonts(Blob::new(Arc::new(data)), None);
        }
        let family = |collection: &mut Collection, names: &[&str]| {
            names.iter().find_map(|name| collection.family_by_name(name).map(|family| family.id()))
        };
        let inter = family(&mut collection, &["Inter"]);
        let plex = family(&mut collection, &["IBM Plex Mono"]);
        let sans = if system_fonts { collection.generic_families(GenericFamily::SystemUi).next().or(inter) } else { inter };
        let mono = if system_fonts {
            family(&mut collection, &["SF Mono", "Menlo", "Cascadia Mono", "Consolas", "DejaVu Sans Mono", "Liberation Mono"]).or(plex)
        } else {
            plex
        };
        collection.set_generic_families(GenericFamily::SystemUi, sans.into_iter());
        collection.set_generic_families(GenericFamily::UiMonospace, mono.into_iter());
        if !system_fonts {
            collection.set_generic_families(GenericFamily::SansSerif, sans.into_iter());
            collection.set_generic_families(GenericFamily::Monospace, mono.into_iter());
        }
        Self {
            fonts: FontContext { collection, source_cache: SourceCache::default() },
            layouts: LayoutContext::new(),
            work: TextWork::default(),
            faces: FxHashMap::default(),
            shapes: FxHashMap::default(),
            shape_bytes: 0,
            tick: 0,
            measures: Cache::default(),
            documents: Cache::default(),
            projected: Cache::default(),
            links: Cache::default(),
            changed: FxHashSet::default(),
            changed_links: FxHashSet::default(),
            needs_projection: false,
            before_stage: None,
        }
    }

    pub(crate) fn begin_stage(&mut self) {
        self.before_stage.get_or_insert((self.needs_projection, self.work.font_generation));
        self.measures.begin();
        self.documents.begin();
        self.projected.begin();
        self.links.begin();
    }

    pub(crate) fn commit_stage(&mut self) {
        self.before_stage = None;
        self.measures.commit();
        self.documents.commit();
        self.projected.commit();
        self.links.commit();
    }

    pub(crate) fn rollback_stage(&mut self) {
        if let Some((dirty, generation)) = self.before_stage.take() {
            self.needs_projection |= dirty || generation != self.work.font_generation;
        }
        self.measures.rollback();
        self.documents.rollback();
        self.projected.rollback();
        self.links.rollback();
        self.changed = FxHashSet::default();
        self.changed_links = FxHashSet::default();
    }

    pub(crate) fn work(&self) -> TextWork {
        self.work
    }

    pub(crate) fn register_font(&mut self, data: Arc<[u8]>, scripts: &[[u8; 4]]) {
        let families =
            self.fonts.collection.register_fonts(Blob::new(Arc::new(data)), None).into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        if families.is_empty() {
            return;
        }
        for script in scripts {
            self.fonts.collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*script), None), families.iter().copied());
        }
        self.work.font_generation = self.work.font_generation.checked_add(1).expect("font generations exhausted");
        self.shapes.clear();
        self.shape_bytes = 0;
        self.needs_projection = true;
    }

    pub(crate) fn font_generation(&self) -> u64 {
        self.work.font_generation
    }

    fn shape(&mut self, text: &str, style: ShapeStyle, spans: &[Span], cache: bool) -> SharedShape {
        self.shape_paragraph(text, style, spans, &[], (0, 0), cache)
    }

    fn shape_paragraph(
        &mut self,
        text: &str,
        style: ShapeStyle,
        spans: &[Span],
        attachments: &[InlineAttachment],
        (offset, first): (usize, usize),
        cache: bool,
    ) -> SharedShape {
        let cache = cache && attachments.is_empty();
        let mut hasher = FxHasher::default();
        if cache {
            text.hash(&mut hasher);
            (
                style.family,
                style.font_size.to_bits(),
                style.line_height.to_bits(),
                style.weight,
                style.italic,
                style.underline,
                style.strike,
                style.letter_spacing.to_bits(),
                style.tabular_nums,
            )
                .hash(&mut hasher);
            for span in spans {
                (
                    &span.range,
                    span.brush,
                    span.family,
                    span.size.map(f32::to_bits),
                    span.weight,
                    span.italic,
                    span.underline,
                    span.strike,
                    span.inset.to_bits(),
                )
                    .hash(&mut hasher);
            }
        }
        let key = hasher.finish();
        self.tick = self.tick.wrapping_add(1);
        if cache
            && let Some(entries) = self.shapes.get_mut(&key)
            && let Some(entry) = entries.iter_mut().find(|entry| &*entry.text == text && entry.style == style && entry.spans == spans)
        {
            entry.used = self.tick;
            self.work.shape_cache_hits += 1;
            return entry.shape.clone();
        }
        self.work.shapes_built += 1;
        let (shaped_text, inline) = inline::Box::project(text, spans, attachments, offset, first);
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, &shaped_text, 1.0, true);
        builder.push_default(StyleProperty::Brush(0));
        builder.push_default(StyleProperty::FontFamily(style.family.into()));
        builder.push_default(StyleProperty::FontSize(style.font_size));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(f32::from(style.weight))));
        builder.push_default(StyleProperty::LineHeight(parley::LineHeight::Absolute(style.line_height)));
        if style.italic {
            builder.push_default(StyleProperty::FontStyle(parley::FontStyle::Italic));
        }
        if style.underline {
            builder.push_default(StyleProperty::Underline(true));
        }
        if style.strike {
            builder.push_default(StyleProperty::Strikethrough(true));
        }
        if style.letter_spacing != 0.0 {
            builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
        }
        if style.tabular_nums {
            builder.push_default(StyleProperty::FontFeatures((&TABULAR_NUMS).into()));
        }
        for span in spans {
            if span.range.is_empty() {
                continue;
            }
            let range = inline::Box::encoded(&inline, span.range.start, true)..inline::Box::encoded(&inline, span.range.end, false);
            builder.push(StyleProperty::Brush(span.brush), range.clone());
            if let Some(family) = span.family {
                builder.push(StyleProperty::FontFamily(family.into()), range.clone());
            }
            if let Some(size) = span.size {
                builder.push(StyleProperty::FontSize(size), range.clone());
                let ratio = style.line_height / style.font_size.max(1.0);
                builder.push(StyleProperty::LineHeight(parley::LineHeight::FontSizeRelative(ratio)), range.clone());
            }
            if let Some(weight) = span.weight {
                builder.push(StyleProperty::FontWeight(FontWeight::new(f32::from(weight))), range.clone());
            }
            if span.italic {
                builder.push(StyleProperty::FontStyle(parley::FontStyle::Italic), range.clone());
            }
            if span.underline {
                builder.push(StyleProperty::Underline(true), range.clone());
            }
            if span.strike {
                builder.push(StyleProperty::Strikethrough(true), range);
            }
        }
        inline::Box::push(&inline, text, &mut builder);
        let mut layout = builder.build(&shaped_text);
        inline::Box::align(&inline, &mut layout, style.font_size);
        let shape = Rc::new(RefCell::new(Shape { layout, inline, broken: None }));
        if cache {
            self.shape_bytes += shape_bytes(text);
            self.shapes.entry(key).or_default().push(CachedShape {
                text: text.into(),
                style,
                spans: spans.to_vec(),
                shape: shape.clone(),
                used: self.tick,
            });
        }
        shape
    }

    fn trim(&mut self) {
        if self.shape_bytes <= CACHE_BUDGET {
            return;
        }
        let mut eligible: Vec<_> = self
            .shapes
            .values()
            .flatten()
            .filter(|shape| Rc::strong_count(&shape.shape) == 1)
            .map(|shape| (shape.used, shape_bytes(&shape.text)))
            .collect();
        eligible.sort_unstable();
        let mut keep = self.shape_bytes;
        let used = eligible
            .into_iter()
            .take_while(|&(_, bytes)| {
                let trim = keep > 3 * CACHE_BUDGET / 4;
                if trim {
                    keep -= bytes;
                }
                trim
            })
            .last();
        if let Some((cutoff, _)) = used {
            self.shapes.retain(|_, entries| {
                entries.retain(|entry| entry.used > cutoff || Rc::strong_count(&entry.shape) > 1);
                !entries.is_empty()
            });
            self.shape_bytes = keep;
        }
    }

    fn document(&mut self, tree: &Tree, node: NodeId, committed: bool) -> Option<Rc<Document>> {
        let mut style = tree.visual(node).text_style();
        let mono = tree.theme_at(node).font_mono;
        let text = tree.text(node);
        let editor = tree.editor(node);
        let source = if let Some(text) = text {
            Source::Text(Arc::as_ptr(&text.text), text.text_rev)
        } else {
            let editor = editor?;
            style.wrap = if editor.multiline { Wrap::Words } else { Wrap::None };
            style.max_lines = None;
            let composing = !committed && editor.composition().is_some();
            Source::Editor {
                revision: if composing { editor.document_rev() } else { editor.value_rev() },
                composing,
                secure: editor.is_secure(),
                revealed: editor.revealed,
                multiline: editor.multiline,
                placeholder: editor.placeholder.clone(),
            }
        };
        let key = Key { source, style, mono, font_generation: self.work.font_generation };
        let cache = if committed { &self.measures } else { &self.documents };
        if let Some(entry) = cache.get(&node).filter(|entry| entry.key == key) {
            return Some(entry.document.clone());
        }
        let mut safe_value = String::new();
        let mut offsets = Vec::new();
        let mut placeholder = false;
        let mut shared = true;
        let (value, rich, attachments) = if let Some(text) = text {
            (&*text.text, text.runs.as_slice(), text.attachments.as_slice())
        } else {
            let editor = editor?;
            let value = if committed { editor.value() } else { editor.composition().map_or(editor.value(), |draft| draft.text()) };
            let value = if !committed && value.is_empty() && editor.attachments().is_empty() && !editor.placeholder.is_empty() {
                placeholder = true;
                &*editor.placeholder
            } else if editor.is_secure() && (committed || !editor.revealed) {
                for (byte, grapheme) in value.grapheme_indices(true) {
                    offsets.push((safe_value.len(), byte));
                    if editor.multiline && matches!(grapheme, "\n" | "\r\n") {
                        safe_value.push_str(grapheme);
                    } else {
                        safe_value.push('•');
                    }
                }
                offsets.push((safe_value.len(), value.len()));
                &safe_value
            } else {
                shared = !editor.is_secure();
                value
            };
            let single_line = (!editor.multiline && value.contains(['\r', '\n'])).then(|| value.replace(['\r', '\n'], " "));
            let value = if let Some(single_line) = single_line {
                safe_value = single_line;
                &*safe_value
            } else {
                value
            };
            if editor.is_secure() || placeholder {
                (value, &[][..], &[][..])
            } else if committed {
                (value, editor.committed_decorations(), editor.committed_attachments())
            } else {
                (value, editor.decorations(), editor.attachments())
            }
        };
        let spans: Vec<_> = rich
            .iter()
            .take(usize::from(u16::MAX))
            .enumerate()
            .map(|(i, (range, span))| Span {
                range: range.clone(),
                brush: (i + 1) as u16,
                family: span.mono.then_some(mono),
                size: span.size,
                weight: span.weight,
                italic: span.italic,
                underline: span.underline,
                strike: span.strike,
                inset: nonnegative(span.padding_x) + f32::from(span.border_color.is_some()),
            })
            .collect();
        let paragraphs = self.paragraphs(value, style, &spans, attachments, shared);
        self.work.documents_built += 1;
        let document = Rc::new(Document { paragraphs, style, offsets, placeholder });
        let cache = if committed { &mut self.measures } else { &mut self.documents };
        cache.insert(node, TextCache { key, document: document.clone(), presentation: None });
        self.trim();
        Some(document)
    }

    fn paragraphs(
        &mut self,
        value: &str,
        style: TextStyle,
        spans: &[Span],
        attachments: &[InlineAttachment],
        share: bool,
    ) -> Vec<Paragraph> {
        let mut start = 0;
        let mut first = 0;
        let mut current = 0;
        let mut local = Vec::new();
        value
            .match_indices('\n')
            .map(|(byte, _)| byte)
            .chain(std::iter::once(value.len()))
            .filter_map(|delimiter| {
                let hard_break = delimiter < value.len();
                while attachments.get(current).is_some_and(|attachment| attachment.range.end <= delimiter) {
                    current += 1;
                }
                if hard_break && attachments.get(current).is_some_and(|attachment| attachment.range.contains(&delimiter)) {
                    return None;
                }
                let mut end = delimiter;
                if hard_break
                    && value[start..delimiter].ends_with('\r')
                    && attachments.get(first..current).is_none_or(|items| items.last().is_none_or(|item| item.range.end < delimiter))
                {
                    end -= 1;
                }
                let next = delimiter + usize::from(hard_break);
                local.clear();
                local.extend(
                    spans
                        .iter()
                        .filter(|span| span.range.start < end && span.range.end > start)
                        .map(|span| Span { range: span.range.start.max(start) - start..span.range.end.min(end) - start, ..span.clone() }),
                );
                let shape =
                    self.shape_paragraph(&value[start..end], style.into(), &local, &attachments[first..current], (start, first), share);
                let paragraph = Paragraph { start, end, next, shape };
                start = next;
                first = current;
                Some(paragraph)
            })
            .collect()
    }

    pub(crate) fn measure(&mut self, tree: &Tree, node: NodeId, known: Option<f32>, width: Option<f32>) -> Option<Size> {
        if let Some(editor) = tree.editor(node) {
            let line_height = tree.visual(node).line_height;
            if !editor.committed_attachments().is_empty() || !editor.committed_decorations().is_empty() {
                let (width, min, max) = if editor.multiline {
                    (known.unwrap_or(INPUT_WIDTH), usize::from(editor.rows.0.max(1)), usize::from(editor.rows.1.max(editor.rows.0).max(1)))
                } else {
                    (INPUT_WIDTH, 1, 1)
                };
                let document = self.document(tree, node, true)?;
                let (measured, lines) = presentation::measure_up_to(&document, Some(width), max);
                let height = (measured.height + min.saturating_sub(lines) as f32 * line_height).max(min as f32 * line_height);
                return Some(Size::new(width, height.ceil()));
            }
            if !editor.multiline {
                return Some(Size::new(INPUT_WIDTH, line_height));
            }
            let width = known.unwrap_or(INPUT_WIDTH);
            let min = usize::from(editor.rows.0.max(1));
            let max = usize::from(editor.rows.1.max(editor.rows.0).max(1));
            if min == max {
                return Some(Size::new(width, (min as f32 * line_height).ceil()));
            }
            let document = self.document(tree, node, true)?;
            let (_, lines) = presentation::measure(&document, Some(width));
            return Some(Size::new(width, (lines.clamp(min, max) as f32 * line_height).ceil()));
        }
        let static_text = tree.text(node)?;
        if static_text.text.is_empty() && static_text.attachments.is_empty() {
            return Some(Size::default());
        }
        let document = self.document(tree, node, false)?;
        let (size, _) = presentation::measure(&document, width);
        Some(Size::new(size.width.ceil(), size.height.ceil()))
    }

    fn node_presentation(&mut self, tree: &Tree, node: NodeId, width: f32) -> Option<Rc<Presentation>> {
        let document = self.document(tree, node, false)?;
        let cache = self.documents.get(&node).expect("document cache");
        if let Some((_, result)) = cache.presentation.as_ref().filter(|(at, _)| *at == width) {
            return Some(result.clone());
        }
        let result = Rc::new(self.present(&document, Some(width)));
        self.work.presentations_built += 1;
        self.documents.get_mut(&node).expect("document cache").presentation = Some((width, result.clone()));
        Some(result)
    }

    pub(crate) fn editor_scroll_range(&mut self, tree: &Tree, node: NodeId, width: f32, height: f32) -> f32 {
        self.node_presentation(tree, node, width).map_or(0.0, |text| (text.size.height - height).max(0.0))
    }

    pub(crate) fn editor_caret_in_content(&mut self, tree: &Tree, node: NodeId, width: f32) -> Option<Rect> {
        let editor = tree.editor(node)?;
        let (_, at) = editor.composition().map_or_else(|| Some(editor.selection()), |composition| composition.selection())?;
        let text = self.node_presentation(tree, node, width)?;
        geometry::caret(&text.lines, at)
    }

    pub(crate) fn link_bounds_in(&mut self, tree: &Tree, node: NodeId, range: &Range<usize>, content: Rect) -> Option<Rect> {
        let text = self.node_presentation(tree, node, content.w)?;
        let bounds = geometry::union(text.range_rects(range, None))?;
        Some(geometry::translate(bounds, leaf_origin(tree, node, content, text.size.height)))
    }

    fn editor_origin(
        &self,
        tree: &Tree,
        layout: &LayoutEngine,
        node: NodeId,
        content: Rect,
        presentation: &Presentation,
        caret: Option<Rect>,
    ) -> Point {
        let mut origin = Point::new(content.x, content.y - layout.scroll_offset(node).y);
        if tree.editor(node).is_some_and(|editor| !editor.multiline) {
            origin.y += ((content.h - presentation.size.height) * 0.5).max(0.0);
            let current =
                self.projected.get(&node).and_then(|previous| Some(previous.paint.clip?.x - previous.paint.local_origin.x)).unwrap_or(0.0);
            let max = (presentation.size.width + 1.0 - content.w).max(0.0);
            let (low, high) = caret.map_or((0.0, max), |caret| {
                let low = (caret.right() - content.w).clamp(0.0, max);
                (low, caret.x.clamp(low, max))
            });
            origin.x -= current.clamp(low, high);
        }
        origin
    }

    pub(crate) fn caret_in(&mut self, tree: &Tree, layout: &LayoutEngine, node: NodeId) -> Option<Rect> {
        if tree.focus() != Some(node) {
            return None;
        }
        let content = layout.content_bounds(node)?;
        let presentation = self.node_presentation(tree, node, content.w)?;
        let caret = self.editor_caret_in_content(tree, node, content.w)?;
        let origin = self.editor_origin(tree, layout, node, content, &presentation, Some(caret));
        let rect = geometry::translate(caret, origin).intersect(&content);
        let rect = layout.child_clip(node).map_or(rect, |clip| rect.intersect(&clip));
        (!rect.is_empty()).then_some(rect)
    }

    pub(crate) fn selection_rect_in(&mut self, tree: &Tree, layout: &LayoutEngine, node: NodeId) -> Option<Rect> {
        if let Some(editor) = tree.editor(node) {
            if tree.focus() != Some(node) {
                return None;
            }
            let (anchor, caret) = editor.composition().map_or_else(|| Some(editor.selection()), |composition| composition.selection())?;
            if anchor == caret {
                return None;
            }
            let content = layout.content_bounds(node)?;
            let presentation = self.node_presentation(tree, node, content.w)?;
            let caret_rect = geometry::caret(&presentation.lines, caret);
            let origin = self.editor_origin(tree, layout, node, content, &presentation, caret_rect);
            let clip = layout.child_clip(node).map_or(content, |clip| content.intersect(&clip));
            return geometry::union(
                presentation
                    .range_rects(&(anchor.min(caret)..anchor.max(caret)), None)
                    .into_iter()
                    .map(|rect| geometry::translate(rect, origin).intersect(&clip))
                    .filter(|rect| !rect.is_empty()),
            );
        }
        let ranges = if let Some(selection) = tree.doc_selection(node) {
            tree.selected_text_ranges(node, selection)?
        } else {
            vec![(node, doc_selection(tree, node)?)]
        };
        geometry::union(ranges.into_iter().filter_map(|(node, range)| {
            let bounds = self.link_bounds_in(tree, node, &range, layout.content_bounds(node)?)?;
            let bounds = layout.child_clip(node).map_or(bounds, |clip| bounds.intersect(&clip));
            (!bounds.is_empty()).then_some(bounds)
        }))
    }

    pub(crate) fn selection_rect(&self, tree: &Tree, node: NodeId) -> Option<Rect> {
        let selection = |node, range: Range<usize>| {
            let projected = self.projected.get(&node)?;
            let paint = &projected.paint;
            let clip = paint.clip.map(|clip| {
                geometry::translate(clip, Point::new(projected.origin.x - paint.local_origin.x, projected.origin.y - paint.local_origin.y))
            });
            geometry::union(
                paint
                    .presentation
                    .range_rects(&range, None)
                    .into_iter()
                    .map(|rect| {
                        let rect = geometry::translate(rect, projected.origin);
                        clip.map_or(rect, |clip| rect.intersect(&clip))
                    })
                    .filter(|rect| !rect.is_empty()),
            )
        };
        if let Some(editor) = tree.editor(node) {
            if tree.focus() != Some(node) {
                return None;
            }
            let (a, b) = editor.composition().map_or_else(|| Some(editor.selection()), |composition| composition.selection())?;
            return (a != b).then(|| selection(node, a.min(b)..a.max(b))).flatten();
        }
        if let Some(document) = tree.doc_selection(node) {
            return geometry::union(
                tree.selected_text_ranges(node, document)?.into_iter().filter_map(|(node, range)| selection(node, range)),
            );
        }
        selection(node, doc_selection(tree, node)?)
    }

    pub(crate) fn project(&mut self, tree: &Tree, delta: &TreeDelta, layout: &LayoutEngine) {
        self.changed = FxHashSet::default();
        self.changed_links = FxHashSet::default();
        for &retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = retired {
                self.documents.remove(&node);
                self.measures.remove(&node);
                self.projected.remove(&node);
                self.remove_link(node);
            }
        }
        let mut changed = layout.changed().union(&delta.layout_dirty).copied().collect::<FxHashSet<_>>();
        changed.extend(delta.editor_dirty.iter().copied());
        if std::mem::take(&mut self.needs_projection) {
            changed.extend(self.projected.keys().copied());
        }
        for node in changed {
            if tree.text(node).is_none() && tree.editor(node).is_none() {
                continue;
            }
            let content = layout.content_bounds(node);
            let Some((content, bounds)) = content.zip(layout.bounds(node)) else {
                if self.projected.remove(&node) {
                    self.changed.insert(node);
                }
                if let Some(text) = tree.text(node) {
                    for link in &text.links {
                        self.remove_link(*link);
                    }
                }
                if tree.editor(node).is_some_and(|editor| editor.is_secure()) {
                    self.documents.remove(&node);
                }
                continue;
            };
            let presentation = self.node_presentation(tree, node, content.w).expect("shown text has a document");
            let caret = if tree.focus() == Some(node) { self.editor_caret_in_content(tree, node, content.w) } else { None };
            let previous = self.projected.get(&node);
            let (origin, clip) = if tree.editor(node).is_some() {
                let origin = self.editor_origin(tree, layout, node, content, &presentation, caret);
                let local = Rect::new(content.x - bounds.x, content.y - bounds.y, content.w, content.h);
                (origin, Some(local))
            } else {
                let origin = leaf_origin(tree, node, content, presentation.size.height);
                let clip = layout.child_clip(node).map(|clip| geometry::translate(clip, Point::new(-bounds.x, -bounds.y)));
                (origin, clip)
            };
            let local_origin = Point::new(origin.x - bounds.x, origin.y - bounds.y);
            let paint = PaintKey { presentation, local_origin, clip, caret };
            let projected_changed = previous.is_none_or(|previous| previous.paint != paint || previous.origin != origin);
            if let Some(text) = tree.text(node) {
                for &link in &text.links {
                    let Some(link_slot) = tree.link(link) else { continue };
                    let rects: Vec<_> = paint
                        .presentation
                        .range_rects(&link_slot.range, None)
                        .into_iter()
                        .map(|rect| geometry::translate(rect, origin))
                        .collect();
                    if rects.is_empty() {
                        self.remove_link(link);
                    } else if self.links.get(&link) != Some(&rects) {
                        self.links.insert(link, rects);
                        self.changed_links.insert(link);
                    }
                }
            }
            if projected_changed {
                self.changed.insert(node);
                self.projected.insert(node, Projected { paint, origin });
            }
        }
    }

    pub(crate) fn geometry(&self, node: NodeId) -> Option<TextGeometry> {
        let projected = self.projected.get(&node)?;
        Some(TextGeometry { origin: projected.origin, lines: projected.paint.presentation.lines.clone() })
    }

    pub(crate) fn clipped(&self) -> Vec<(NodeId, TextClip)> {
        let mut clipped: Vec<_> =
            self.projected.iter().filter_map(|(&node, projected)| Some((node, projected.paint.presentation.clip?))).collect();
        clipped.sort_unstable_by_key(|&(node, _)| node);
        clipped
    }

    pub(crate) fn changed(&self) -> &FxHashSet<NodeId> {
        &self.changed
    }

    pub(crate) fn paint_key(&self, node: NodeId) -> Option<PaintKey> {
        Some(self.projected.get(&node)?.paint.clone())
    }

    pub(crate) fn link_rects(&self, node: NodeId) -> &[Rect] {
        self.links.get(&node).map_or(&[], Vec::as_slice)
    }

    fn remove_link(&mut self, node: NodeId) {
        if self.links.remove(&node) {
            self.changed_links.insert(node);
        }
    }

    pub(crate) fn changed_links(&self) -> &FxHashSet<NodeId> {
        &self.changed_links
    }

    pub(crate) fn link_bounds(&self, node: NodeId) -> Option<Rect> {
        geometry::union(self.link_rects(node).iter().copied())
    }

    pub(crate) fn caret(&self, node: NodeId) -> Option<Rect> {
        let text = self.projected.get(&node)?;
        let local = &text.paint;
        let caret = geometry::translate(local.caret?, local.local_origin);
        let caret = local.clip.map_or(caret, |clip| caret.intersect(&clip));
        (!caret.is_empty())
            .then(|| geometry::translate(caret, Point::new(text.origin.x - local.local_origin.x, text.origin.y - local.local_origin.y)))
    }

    pub(crate) fn items(&self, tree: &Tree, node: NodeId, window_active: bool) -> Vec<Item> {
        let Some(text) = self.projected.get(&node).map(|projected| &projected.paint) else { return Vec::new() };
        let mut items = Vec::new();
        let theme = tree.theme_at(node);
        let mut foreground = tree.visual(node).text;
        let editor = tree.editor(node);
        let rich = tree.text(node).map_or_else(|| editor.map_or(&[][..], |editor| editor.decorations()), |text| text.runs.as_slice());
        let origin = text.local_origin;
        let theme_color = |token| theme.get(token);
        let selection_color = || {
            if window_active { theme_color(ColorToken::Selection) } else { ColorToken::TextSubtle.alpha(0.28).resolve(theme).0 }
        };
        if let Some(rect) = text.clip {
            items.push(Item::PushClip { source: node, rect, radius: [0.0; 4] });
        }
        let range_rects =
            |range| geometry::range_rects(&text.presentation.lines, &range).into_iter().map(|rect| geometry::translate(rect, origin));
        for (index, (range, span)) in rich.iter().enumerate() {
            let fill = span.background.map(|color| color.resolve(theme).0);
            let border = span.border_color.map(|color| color.resolve(theme).0);
            if fill.is_none() && border.is_none() {
                continue;
            }
            let brush = u16::try_from(index + 1).unwrap_or(0);
            let rects = text.presentation.span_rects(range, brush, nonnegative(span.padding_y));
            items.extend(rects.map(|rect| {
                Item::Quad(Quad {
                    source: node,
                    rect: geometry::translate(rect, origin),
                    radius: [nonnegative(span.radius); 4],
                    fill,
                    gradient: None,
                    border: [f32::from(border.is_some()); 4],
                    border_color: border.unwrap_or(Color::TRANSPARENT),
                })
            }));
        }
        if let Some(editor) = editor {
            let displayed = editor.composition().map_or(editor.value(), |composition| composition.text());
            if displayed.is_empty() {
                foreground = theme_color(ColorToken::TextMuted);
            }
            if tree.focus() == Some(node) {
                let selection = editor.composition().map_or_else(|| Some(editor.selection()), |composition| composition.selection());
                if let Some((a, b)) = selection
                    && a != b
                {
                    let color = selection_color();
                    items.extend(range_rects(a.min(b)..a.max(b)).map(|rect| quad(node, rect, color)));
                }
                if let Some(composition) = editor.composition() {
                    items.extend(
                        range_rects(composition.marked())
                            .map(|rect| quad(node, Rect::new(rect.x, rect.bottom() - 1.0, rect.w, 1.0), foreground)),
                    );
                }
            }
        } else if let Some(range) = doc_selection(tree, node) {
            let color = selection_color();
            items.extend(range_rects(range).map(|rect| quad(node, rect, color)));
        }
        let color = |index| brush(rich, index).and_then(|span| span.color).map_or(foreground, |color| color.resolve(theme).0);
        items.extend(text.presentation.glyphs.iter().map(|run| run.item(node, color(run.brush), origin)));
        items
            .extend(text.presentation.decorations.iter().map(|line| quad(node, geometry::translate(line.rect, origin), color(line.brush))));
        if text.clip.is_some() {
            items.push(Item::PopClip);
        }
        items
    }

    pub(crate) fn attachments(&self, node: NodeId) -> (Option<Rect>, Vec<(usize, Rect)>) {
        let Some(text) = self.projected.get(&node).map(|projected| &projected.paint) else { return (None, Vec::new()) };
        let positions =
            text.presentation.attachments.iter().map(|&(index, rect)| (index, geometry::translate(rect, text.local_origin))).collect();
        (text.clip, positions)
    }

    pub(crate) fn attachment_bounds(&self, node: NodeId, index: usize) -> Option<Rect> {
        let text = self.projected.get(&node)?;
        let &(_, rect) = text.paint.presentation.attachments.iter().find(|&&(candidate, _)| candidate == index)?;
        Some(geometry::translate(rect, text.origin))
    }

    pub(crate) fn canvas_items(&mut self, source: NodeId, text: &CanvasText, mut style: TextStyle, color: Color) -> Vec<Item> {
        let ratio = style.line_height / style.font_size.max(1.0);
        style.font_size = text.size.unwrap_or(style.font_size) * text.scale;
        style.line_height = style.font_size * ratio;
        style.weight = text.weight.unwrap_or(style.weight);
        style.wrap = if text.max_width.is_some() { Wrap::Words } else { Wrap::None };
        style.align = TextAlign::Start;
        style.max_lines = None;
        let document =
            Document { paragraphs: self.paragraphs(&text.text, style, &[], &[], true), style, offsets: Vec::new(), placeholder: false };
        let presentation = self.present(&document, text.max_width);
        let shift = |align, extent| match align {
            Align::Start => 0.0,
            Align::Center => extent * 0.5,
            Align::End => extent,
        };
        let origin =
            Point::new(text.at.x - shift(text.align, presentation.size.width), text.at.y - shift(text.valign, presentation.size.height));
        let items = presentation
            .glyphs
            .iter()
            .map(|run| run.item(source, color, origin))
            .chain(presentation.decorations.iter().map(|line| quad(source, geometry::translate(line.rect, origin), color)))
            .collect();
        self.trim();
        items
    }

    fn face(&mut self, font: &linebender_resource_handle::FontData) -> FontFace {
        static NEXT_FACE: AtomicU64 = AtomicU64::new(1);
        self.faces
            .entry((font.data.id(), font.index))
            .or_insert_with(|| FontFace {
                id: FontId(NEXT_FACE.fetch_add(1, Ordering::Relaxed)),
                data: Arc::from(font.data.data()),
                index: font.index,
            })
            .clone()
    }
}

fn shape_bytes(text: &str) -> usize {
    2048 + text.len().saturating_mul(64)
}

fn leaf_origin(tree: &Tree, node: NodeId, content: Rect, text_height: f32) -> Point {
    let spare = (content.h - text_height).max(0.0);
    let offset = match tree.layout_style(node).text_block_align() {
        Align::Start => 0.0,
        Align::Center => spare * 0.5,
        Align::End => spare,
    };
    Point::new(content.x, content.y + offset)
}

fn nonnegative(value: Option<f32>) -> f32 {
    value.filter(|value| value.is_finite() && *value > 0.0).unwrap_or(0.0)
}

fn brush(rich: &[(Range<usize>, RichSpan)], brush: u16) -> Option<RichSpan> {
    brush.checked_sub(1).and_then(|index| rich.get(usize::from(index))).map(|(_, span)| *span)
}

fn quad(source: NodeId, rect: Rect, color: Color) -> Item {
    Item::Quad(Quad { source, rect, radius: [0.0; 4], fill: Some(color), gradient: None, border: [0.0; 4], border_color: color })
}

fn doc_selection(tree: &Tree, node: NodeId) -> Option<Range<usize>> {
    let region = tree.region_of(node)?;
    let selection = tree.doc_selection(region)?;
    tree.selected_text_ranges(region, selection)?.into_iter().find_map(|(selected, range)| (selected == node).then_some(range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::geometry::WindowViewport;
    use octara_tree::{NodeContent, NodeSpec, TreeId, TxBuilder};

    #[test]
    fn retains_shaping_and_projects_utf8_to_visual_lines() {
        let mut tree = Tree::new(TreeId::new([3, 5], 2), Default::default());
        let mut delta = tree.initial_delta();
        let content = "A e\u{301} b\r\nsecond line";
        let mut tx = TxBuilder::new(tree.id(), tree.revision());
        let label = tx.create(NodeSpec::new(NodeContent::Text(content.into())));
        tx.place(tree.node_ref(tree.root()), 0, label);
        let (receipt, created) = tree.apply(tx.finish()).unwrap();
        delta.merge(created);
        let label = receipt.node(label).unwrap();
        let viewport = WindowViewport { size: Size::new(300.0, 160.0), scale: 1.0, safe_area: None };
        let mut layout = LayoutEngine::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        let geometry = text.geometry(label).unwrap();
        assert_eq!(geometry.lines.len(), 2);
        assert_eq!(geometry.lines[0].range.end, content.find('s').unwrap());
        assert!(geometry.lines[0].fragments.iter().any(|fragment| fragment.range == (2..5)));
        assert_eq!(geometry::hit_byte(&geometry, geometry.origin), Some(0));
        assert_eq!(geometry::vertical_byte(&geometry, 0, 1, None), Some((9, 0.0)));
        assert!(text.items(&tree, label, true).iter().any(|item| matches!(item, Item::Glyphs(run) if !run.font.data.is_empty())));
        let shapes = text.tick;
        let thinner = WindowViewport { size: Size::new(240.0, 160.0), ..viewport };
        layout.project(&tree, &TreeDelta::default(), thinner, &mut text);
        assert_eq!(text.tick, shapes);
    }

    #[test]
    fn inline_replacement_keeps_multibyte_source_and_reserves_its_box() {
        use octara_model::paint::{Image, ObjectFit};
        use octara_model::text::{InlineAlign, InlineContent};

        let value = "A界\nβB";
        let range = 1..value.len() - 1;
        let attachments = [
            InlineAttachment {
                range: 0..0,
                size: Size::default(),
                align: InlineAlign::Baseline,
                baseline: None,
                content: InlineContent::Spacer { width: 7.0 },
            },
            InlineAttachment {
                range: range.clone(),
                size: Size::new(24.0, 30.0),
                align: InlineAlign::CapHeightCenter,
                baseline: None,
                content: InlineContent::Image { image: Image::rgba(1, 1, [0; 4]), fit: ObjectFit::Contain, radius: 0.0 },
            },
        ];
        let style = TextStyle::default();
        let mut engine = TextEngine::new(false);
        let document = Document {
            paragraphs: engine.paragraphs(value, style, &[], &attachments, true),
            style,
            offsets: Vec::new(),
            placeholder: false,
        };
        let presentation = engine.present(&document, None);
        assert_eq!(presentation.lines.len(), 1);
        assert_eq!(presentation.lines[0].range, 0..value.len());
        let fragment = presentation.lines[0].fragments.iter().find(|fragment| fragment.range == range).unwrap();
        assert_eq!(&value[fragment.range.clone()], "界\nβ");
        assert_eq!(fragment.rect.w, 24.0);
        let geometry = TextGeometry { origin: Point::default(), lines: presentation.lines.clone() };
        assert_eq!(geometry::hit_byte(&geometry, Point::new(fragment.rect.x + 4.0, 0.0)), Some(range.start));
        assert_eq!(geometry::hit_byte(&geometry, Point::new(fragment.rect.right() - 4.0, 0.0)), Some(range.end));
        assert_eq!(presentation.attachments.len(), 2);
        assert_eq!(presentation.attachments[0].1.w, 7.0);
        let image = presentation.attachments[1].1;
        assert_eq!((image.w, image.h), (24.0, 30.0));
        assert!((image.x - fragment.rect.x).abs() < 0.01);
        assert!(image.y >= 0.0 && image.bottom() <= presentation.size.height);
    }

    #[test]
    fn inline_image_drawings_refresh_the_sample_and_keep_logical_accessibility_text() {
        use crate::{NativeEngine, NativeEngineConfig};
        use octara_model::paint::{Image, ObjectFit};
        use octara_model::text::{InlineAlign, InlineContent};
        use octara_runtime::HostEngine;
        use octara_tree::transaction::RichText;
        use octara_tree::{NodeChange, NodeFlags};

        let image = Image::rgba(1, 1, [128, 0, 0, 255]);
        let source = "a formula b";
        let mut tree = Tree::new(TreeId::new([3, 5], 20), Default::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let label = tx.create(NodeSpec::new(NodeContent::Text("".into())));
        tx.change(label, NodeChange::Flags(NodeFlags::SELECT_REGION));
        tx.change(
            label,
            NodeChange::RichText(Box::new(RichText {
                text: source.into(),
                attachments: vec![InlineAttachment {
                    range: 2..9,
                    size: Size::new(22.0, 26.0),
                    align: InlineAlign::Baseline,
                    baseline: None,
                    content: InlineContent::Image { image: image.clone(), fit: ObjectFit::Fill, radius: 2.0 },
                }],
                ..Default::default()
            })),
        );
        tx.place(tree.node_ref(tree.root()), 0, label);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        delta.merge(mounted);
        let label = receipt.node(label).unwrap();
        let viewport = WindowViewport { size: Size::new(300.0, 160.0), scale: 1.0, safe_area: None };
        let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
        engine.project(&tree, &delta, viewport, 0.0).unwrap();
        engine.commit();
        let first = engine.publication().latest().unwrap();
        let version = |draw: &octara_model::paint::DisplayList| {
            draw.items().into_iter().find_map(|item| match item {
                Item::Image(item) if item.source == label => Some(item.image.frame().version),
                _ => None,
            })
        };
        assert_eq!(version(&first.draw), Some(0));
        assert!(first.accessibility.nodes.iter().any(|(_, node)| node.value() == Some(source)));
        image.set(1, 1, [0, 180, 0, 255]);
        engine.project(&tree, &TreeDelta::default(), viewport, 1.0).unwrap();
        engine.commit();
        assert_eq!(version(&engine.publication().latest().unwrap().draw), Some(1));
        assert_eq!(version(&first.draw), Some(0));
    }

    #[test]
    fn pill_edges_take_part_in_wrapping_and_keep_source_offsets() {
        let style = TextStyle::default();
        let mut engine = TextEngine::new(false);
        let span = |range, inset| Span {
            range,
            brush: 1,
            family: None,
            size: None,
            weight: None,
            italic: false,
            underline: false,
            strike: false,
            inset,
        };
        let document = |engine: &mut TextEngine, text, span, style| Document {
            paragraphs: engine.paragraphs(text, style, &[span], &[], true),
            style,
            offsets: Vec::new(),
            placeholder: false,
        };
        let plain = document(&mut engine, "go café", span(3..8, 0.0), style);
        let padded = document(&mut engine, "go café", span(3..8, 8.0), style);
        let (plain_size, _) = presentation::measure(&plain, None);
        let (padded_size, _) = presentation::measure(&padded, None);
        assert!((padded_size.width - plain_size.width - 16.0).abs() < 0.01);
        assert_eq!(plain_size.height, padded_size.height);
        let width = plain_size.width + 1.0;
        let (_, line_count) = presentation::measure(&padded, Some(width));
        assert_eq!(line_count, 2);
        let shown = engine.present(&padded, Some(width));
        assert_eq!(shown.lines[1].range, 3..8);
        assert_eq!(shown.lines[1].fragments.last().unwrap().range, 6..8);
        let pill = shown.range_rects(&(3..8), Some(1));
        assert_eq!(pill.len(), 1);
        assert_eq!(pill[0].y, shown.lines[1].rect.y);
        assert!((shown.lines[1].fragments[0].rect.x - pill[0].x - 8.0).abs() < 0.01);
        assert!((pill[0].right() - shown.lines[1].fragments.last().unwrap().rect.right() - 8.0).abs() < 0.01);

        let bidi = document(&mut engine, "left אבג right", span(5..11, 8.0), style);
        let shown = engine.present(&bidi, Some(200.0));
        let pill = shown.range_rects(&(5..11), Some(1));
        assert_eq!(pill.len(), 1);
        let ink = shown.lines[0].fragments.iter().filter(|fragment| fragment.rtl).collect::<Vec<_>>();
        assert!((ink[0].rect.x - pill[0].x - 8.0).abs() < 0.01);
        assert!((pill[0].right() - ink.last().unwrap().rect.right() - 8.0).abs() < 0.01);

        let truncate = TextStyle { wrap: Wrap::Truncate, ..style };
        let clipped = document(&mut engine, "go café later", span(3..8, 8.0), truncate);
        let shown = engine.present(&clipped, Some(44.0));
        assert_eq!(shown.clip, Some(TextClip::Ellipsis));
        assert!(shown.range_rects(&(3..8), Some(1)).is_empty(), "padding cannot be truncated away from its text");
    }

    #[test]
    fn centers_leaf_text_and_link_geometry_inside_the_content_box() {
        use octara_model::style::{AlignItems, FlexDirection, JustifyContent, StyleProp};
        use octara_tree::NodeChange;
        use octara_tree::tables::{AuthoredStyle, LinkSlot};
        use octara_tree::transaction::RichText;

        let mut tree = Tree::new(TreeId::new([3, 5], 4), Default::default());
        let mut delta = tree.initial_delta();
        let mut tx = TxBuilder::new(tree.id(), tree.revision());
        let cases = [
            (FlexDirection::Row, AlignItems::Center, JustifyContent::End, 0.5),
            (FlexDirection::RowReverse, AlignItems::FlexEnd, JustifyContent::Center, 1.0),
            (FlexDirection::Column, AlignItems::Start, JustifyContent::Center, 0.5),
            (FlexDirection::ColumnReverse, AlignItems::Center, JustifyContent::FlexStart, 1.0),
            (FlexDirection::ColumnReverse, AlignItems::End, JustifyContent::FlexEnd, 0.0),
        ];
        let labels = cases.map(|(direction, align, justify, fraction)| {
            let label = tx.create(NodeSpec::new(NodeContent::Text("".into())));
            tx.change(
                label,
                NodeChange::Style(Box::new(AuthoredStyle {
                    base: [
                        StyleProp::MinHeight(64.0.into()),
                        StyleProp::PaddingTop(5.0.into()),
                        StyleProp::PaddingBottom(3.0.into()),
                        StyleProp::Direction(direction),
                        StyleProp::AlignItems(align),
                        StyleProp::Justify(justify),
                    ]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                })),
            );
            tx.change(
                label,
                NodeChange::RichText(Box::new(RichText {
                    text: "link".into(),
                    links: vec![LinkSlot { range: 0..4, href: "#link".into() }],
                    ..Default::default()
                })),
            );
            tx.place(tree.node_ref(tree.root()), 0, label);
            (label, fraction)
        });
        let (receipt, created) = tree.apply(tx.finish()).unwrap();
        delta.merge(created);
        let viewport = WindowViewport { size: Size::new(600.0, 160.0), scale: 1.0, safe_area: None };
        let mut layout = LayoutEngine::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        for (label, fraction) in labels {
            let label = receipt.node(label).unwrap();
            let link = tree.children(label)[0];
            let content = layout.content_bounds(label).unwrap();
            let geometry = text.geometry(label).unwrap();
            let line_height = geometry.lines[0].rect.h;
            assert_eq!(geometry.origin.y, content.y + fraction * (content.h - line_height));
            assert_eq!(text.link_rects(link), &[layout.bounds(link).unwrap()]);
            assert_eq!(text.link_rects(link)[0].y, geometry.origin.y);
        }
    }

    #[test]
    fn paints_wrapped_pills_in_the_glyph_band_and_keeps_full_height_selection() {
        use octara_model::style::{LineHeight, StyleProp};
        use octara_tree::NodeChange;
        use octara_tree::tables::AuthoredStyle;
        use octara_tree::transaction::RichText;

        let mut tree = Tree::new(TreeId::new([3, 5], 5), Default::default());
        let mut delta = tree.initial_delta();
        let mut tx = TxBuilder::new(tree.id(), tree.revision());
        let source = "abc def ghi jkl";
        let labels = [0.0, 2.0, 100.0].map(|padding| {
            let label = tx.create(NodeSpec::new(NodeContent::Text("".into())));
            tx.change(
                label,
                NodeChange::Style(Box::new(AuthoredStyle {
                    base: [StyleProp::Width(58.0.into()), StyleProp::LineHeight(LineHeight::Len(48.0.into()))].into_iter().collect(),
                    ..Default::default()
                })),
            );
            tx.change(
                label,
                NodeChange::RichText(Box::new(RichText {
                    text: source.into(),
                    runs: vec![(
                        0..source.len(),
                        RichSpan { background: Some(ColorToken::Accent.into()), ..Default::default() }.padding_y(padding),
                    )],
                    ..Default::default()
                })),
            );
            tx.place(tree.node_ref(tree.root()), 0, label);
            (label, padding)
        });
        let (receipt, created) = tree.apply(tx.finish()).unwrap();
        delta.merge(created);
        let viewport = WindowViewport { size: Size::new(350.0, 400.0), scale: 1.0, safe_area: None };
        let mut layout = LayoutEngine::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        for (label, padding) in labels {
            let label = receipt.node(label).unwrap();
            let geometry = text.geometry(label).unwrap();
            assert!(geometry.lines.len() > 1);
            let local_y = geometry.origin.y - layout.bounds(label).unwrap().y;
            let items = text.items(&tree, label, true);
            let pills = items.iter().filter_map(|item| if let Item::Quad(quad) = item { Some(quad.rect) } else { None });
            assert_eq!(pills.clone().count(), geometry.lines.len());
            for (pill, line) in pills.zip(&*geometry.lines) {
                assert_eq!(line.rect.h, 48.0);
                assert!(line.fragments.iter().all(|fragment| fragment.rect.h == line.rect.h));
                assert!(line.glyph_height < line.rect.h);
                let top = line.rect.y.max(line.glyph_top - padding);
                let bottom = line.rect.bottom().min(line.glyph_top + line.glyph_height + padding);
                assert_eq!((pill.y, pill.bottom()), (top + local_y, bottom + local_y));
            }
            let selected = geometry::range_rects(&geometry.lines, &(0..source.len()));
            assert!(selected.iter().all(|rect| rect.h == 48.0));
        }
    }

    #[test]
    fn shapes_italic_tracking_and_tabular_digits_and_extracts_decorations() {
        let mut engine = TextEngine::new(false);
        let mut present = |value, style| {
            let paragraphs = engine.paragraphs(value, style, &[], &[], true);
            engine.present(&Document { paragraphs, style, offsets: Vec::new(), placeholder: false }, None)
        };
        let base = TextStyle::default();
        let normal = present("WWWW", base);
        let styled = present("WWWW", TextStyle { italic: true, underline: true, strike: true, letter_spacing: 2.0, ..base });
        assert!(styled.size.width > normal.size.width + 4.0);
        assert!(styled.glyphs.iter().any(|run| run.skew != 0.0));
        assert_eq!(styled.decorations.len(), 2);
        assert!(styled.decorations[0].rect.y > styled.decorations[1].rect.y);
        assert!(normal.decorations.is_empty());
        assert!(normal.glyphs.iter().all(|run| run.skew == 0.0));

        let plain = present("1010", base);
        let tabular = present("1010", TextStyle { tabular_nums: true, ..base });
        let widths = |text: &Presentation| text.lines[0].fragments.iter().map(|fragment| fragment.rect.w).collect::<Vec<_>>();
        assert_ne!(widths(&plain)[0], widths(&plain)[1]);
        assert_eq!(widths(&tabular), [widths(&tabular)[0]; 4]);
    }

    #[test]
    fn projects_partially_visible_links_and_masks_editor_graphemes() {
        use octara_model::style::{Length, StyleProp};
        use octara_tree::NodeChange;
        use octara_tree::tables::{AuthoredStyle, EditorSlot, LinkSlot};
        use octara_tree::transaction::RichText;

        let mut tree = Tree::new(TreeId::new([3, 5], 3), Default::default());
        let mut delta = tree.initial_delta();
        let mut tx = TxBuilder::new(tree.id(), tree.revision());
        let label = tx.create(NodeSpec::new(NodeContent::Text(Arc::from(""))));
        let content = "abcdef more text beyond the allocated width";
        tx.change(
            label,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [StyleProp::Width(Length::Px(60.0)), StyleProp::TextWrap(Wrap::Truncate)].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.change(
            label,
            NodeChange::RichText(Box::new(RichText {
                text: content.into(),
                links: vec![LinkSlot { range: 0..6, href: "#label".into() }],
                ..Default::default()
            })),
        );
        let mut editor = EditorSlot::new("é界");
        editor.set_secure(true);
        let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(editor))));
        tx.place(tree.node_ref(tree.root()), 0, label);
        tx.place(tree.node_ref(tree.root()), 1, editor);
        let (receipt, created) = tree.apply(tx.finish()).unwrap();
        delta.merge(created);
        let label = receipt.node(label).unwrap();
        let editor = receipt.node(editor).unwrap();
        let link = tree.children(label)[0];
        let viewport = WindowViewport { size: Size::new(300.0, 160.0), scale: 1.0, safe_area: None };
        let mut layout = LayoutEngine::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        assert_eq!(text.clipped(), vec![(label, TextClip::Ellipsis)]);
        let link_bounds = layout.bounds(link).expect("partially visible inline link geometry");
        assert_eq!(text.link_rects(link), &[link_bounds]);
        assert!(link_bounds.w < 60.0);
        let geometry = text.geometry(editor).unwrap();
        let fragments = &geometry.lines[0].fragments;
        assert_eq!(fragments.iter().map(|fragment| fragment.range.clone()).collect::<Vec<_>>(), [0..2, 2..5]);
        let glyphs: Vec<_> = text
            .items(&tree, editor, true)
            .iter()
            .filter_map(|item| match item {
                Item::Glyphs(run) => Some(run.glyphs.iter().map(|glyph| glyph.id)),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(glyphs.len(), 2);
        assert_eq!(glyphs[0], glyphs[1]);
    }
}
