//! Native font selection, paragraph shaping, and text projection.

#[cfg(any(unix, windows))]
mod font;
pub(crate) mod geometry;
mod inline;
mod presentation;

use self::presentation::MeasureCache;
use crate::layout::LayoutKernel;
use crate::paint::paint_icon;
use crate::projected::DenseProjected;
use octara_model::CacheLine;
use octara_model::geometry::{Align, Point, Rect, Size};
use octara_model::paint::{CanvasText, FontData, FontFace, FontId, Glyph, GlyphRun, Item, Quad};
use octara_model::semantics::TextClip;
use octara_model::style::Direction;
use octara_model::text::{ClauseStyle, InlineAttachment, InlineContent, RichSpan, TextAlign, TextGeometry, TextLine, TextStyle, Wrap};
use octara_model::theme::{Color, ColorToken};
use octara_tree::{DirtyBitmap, NodeId, RetiredNode, Tree, TreeDelta};
use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, GenericFamily, Script, SourceCache};
use parley::{Alignment, AlignmentOptions, BaseDirection, FontContext, FontWeight, Layout, LayoutContext, StyleProperty};
use rustc_hash::{FxHashMap, FxHasher};
use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use unicode_segmentation::UnicodeSegmentation;

#[cfg(any(unix, windows))]
pub(crate) use font::map_font;

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
    measures: MeasureCache,
}

type SharedShape = Rc<RefCell<Shape>>;

impl Shape {
    fn at(&mut self, width: Option<f32>, justify: bool, work: &mut TextWork) -> &Self {
        if self.broken != Some((width, justify)) {
            let alignment = if justify { Alignment::Justify } else { Alignment::Left };
            self.layout.set_alignment(alignment, AlignmentOptions::default());
            self.layout.break_all_lines(width);
            work.line_breaks += 1;
            self.broken = Some((width, justify));
        }
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ShapeStyle {
    direction: Direction,
    text_scale: f32,
    family: &'static str,
    font_size: f32,
    line_height: Option<f32>,
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
            direction: style.direction,
            text_scale: style.text_scale,
            family: style.family,
            font_size: style.font_size,
            line_height: Some(style.line_height),
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
    measures: RefCell<Option<Box<MeasureCache>>>,
}

impl Document {
    fn new(paragraphs: Vec<Paragraph>, style: TextStyle, offsets: Vec<(usize, usize)>, placeholder: bool) -> Self {
        Self { paragraphs, style, offsets, placeholder, measures: RefCell::default() }
    }

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
    pills: Vec<PillPaint>,
    size: Size,
    clip: Option<TextClip>,
}

struct SpanEdge {
    brush: u16,
    anchor: usize,
    rect: Rect,
}

struct PillPaint {
    index: usize,
    rect: Rect,
    glyphs: Vec<Run>,
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

#[derive(Clone)]
struct Projected {
    paint: PaintKey,
}

#[derive(Clone)]
pub(crate) struct PaintKey {
    presentation: Rc<Presentation>,
    local_origin: Point,
    clip: Option<(Rect, [bool; 2])>,
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
    /// Measurement results reused from a shape or document probe.
    pub measure_probe_hits: u64,
    /// Parley line-breaking passes, including those used for presentation.
    pub line_breaks: u64,
    /// Text and editor nodes considered by local projection.
    pub nodes_projected: u64,
}

pub(crate) struct TextEngine {
    fonts: FontContext,
    layouts: LayoutContext<u16>,
    work: TextWork,
    faces: FxHashMap<(u64, u32), FontFace>,
    shapes: FxHashMap<u64, Vec<CachedShape>>,
    shape_bytes: usize,
    tick: u64,
    measures: DenseProjected<TextCache>,
    documents: DenseProjected<TextCache>,
    projected: DenseProjected<Projected>,
    links: DenseProjected<Vec<Rect>>,
    changed: DirtyBitmap,
    changed_links: DirtyBitmap,
    candidates: DirtyBitmap,
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
        Self::with_collection(collection, 0)
    }

    pub(crate) fn fork_for_layout_check(&self) -> Self {
        Self::with_collection(self.fonts.collection.clone(), self.font_generation())
    }

    fn with_collection(collection: Collection, font_generation: u64) -> Self {
        Self {
            fonts: FontContext { collection, source_cache: SourceCache::default() },
            layouts: LayoutContext::new(),
            work: TextWork { font_generation, ..TextWork::default() },
            faces: FxHashMap::default(),
            shapes: FxHashMap::default(),
            shape_bytes: 0,
            tick: 0,
            measures: DenseProjected::default(),
            documents: DenseProjected::default(),
            projected: DenseProjected::default(),
            links: DenseProjected::default(),
            changed: DirtyBitmap::default(),
            changed_links: DirtyBitmap::default(),
            candidates: DirtyBitmap::default(),
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
        self.changed.clear();
        self.changed_links.clear();
    }

    pub(crate) fn work(&self) -> TextWork {
        self.work
    }

    /// Clear retired measurement and document slots before layout can reuse their indices.
    pub(crate) fn retire(&mut self, delta: &TreeDelta) {
        for &retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = retired {
                self.documents.remove(node);
                self.measures.remove(node);
            }
        }
    }

    pub(crate) fn register_font(&mut self, data: FontData, scripts: &[[u8; 4]]) -> bool {
        let families = self.fonts.collection.register_fonts(Blob::new(data.into_shared()), None);
        if families.is_empty() {
            return false;
        }
        for script in scripts {
            self.fonts.collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*script), None), families.iter().map(|(id, _)| *id));
        }
        self.work.font_generation = self.work.font_generation.checked_add(1).expect("font generations exhausted");
        self.shapes.clear();
        self.shape_bytes = 0;
        self.needs_projection = true;
        true
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
                style.line_height.map(f32::to_bits),
                style.weight,
                style.italic,
                style.underline,
                style.strike,
                style.letter_spacing.to_bits(),
                style.tabular_nums,
                style.direction,
                style.text_scale.to_bits(),
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
        let (shaped_text, inline) = inline::Box::project(text, spans, attachments, (offset, first), self, style);
        // A pill label retains its fractional metrics inside the enclosing paragraph.
        let mut builder = self.layouts.ranged_builder(&mut self.fonts, &shaped_text, 1.0, style.line_height.is_some());
        builder.set_base_direction(match style.direction {
            Direction::Inherit => BaseDirection::Auto,
            Direction::Ltr => BaseDirection::Ltr,
            Direction::Rtl => BaseDirection::Rtl,
        });
        builder.push_default(StyleProperty::Brush(0));
        builder.push_default(StyleProperty::FontFamily(style.family.into()));
        builder.push_default(StyleProperty::FontSize(style.font_size));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(f32::from(style.weight))));
        if let Some(height) = style.line_height {
            builder.push_default(StyleProperty::LineHeight(parley::LineHeight::Absolute(height)));
        }
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
                if let Some(height) = style.line_height {
                    let ratio = height / style.font_size.max(1.0);
                    builder.push(StyleProperty::LineHeight(parley::LineHeight::FontSizeRelative(ratio)), range.clone());
                }
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
        inline::Box::align(&inline, &mut layout, style.font_size, &mut self.work);
        let shape = Rc::new(RefCell::new(Shape { layout, inline, broken: None, measures: MeasureCache::default() }));
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
        if let Some(entry) = cache.get(node).filter(|entry| entry.key == key) {
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
                size: span.size.map(|size| size * style.text_scale),
                weight: span.weight,
                italic: span.italic,
                underline: span.underline,
                strike: span.strike,
                inset: nonnegative(span.padding_x) + f32::from(span.border_color.is_some()),
            })
            .collect();
        let paragraphs = self.paragraphs(value, style, &spans, attachments, shared);
        self.work.documents_built += 1;
        let document = Rc::new(Document::new(paragraphs, style, offsets, placeholder));
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
                let (measured, lines) = document.measure(Some(width), max, &mut self.work);
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
            let (_, lines) = document.measure(Some(width), max, &mut self.work);
            return Some(Size::new(width, (lines.clamp(min, max) as f32 * line_height).ceil()));
        }
        let static_text = tree.text(node)?;
        if static_text.text.is_empty() && static_text.attachments.is_empty() {
            return Some(Size::default());
        }
        let document = self.document(tree, node, false)?;
        let (size, _) = document.measure(width, usize::MAX, &mut self.work);
        Some(Size::new(size.width.ceil(), size.height.ceil()))
    }

    fn node_presentation(&mut self, tree: &Tree, node: NodeId, width: f32) -> Option<Rc<Presentation>> {
        let document = self.document(tree, node, false)?;
        let cache = self.documents.get(node).expect("document cache");
        if let Some((_, result)) = cache.presentation.as_ref().filter(|(at, _)| *at == width) {
            return Some(result.clone());
        }
        let result = Rc::new(self.present(&document, Some(width)));
        self.work.presentations_built += 1;
        self.documents.get_mut(node).expect("document cache").presentation = Some((width, result.clone()));
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
        layout: &LayoutKernel,
        node: NodeId,
        content: Rect,
        presentation: &Presentation,
        caret: Option<Rect>,
    ) -> Point {
        let mut origin = Point::new(content.x, content.y - layout.scroll_offset(node).y);
        if tree.editor(node).is_some_and(|editor| !editor.multiline) {
            origin.y += ((content.h - presentation.size.height) * 0.5).max(0.0);
            let current =
                self.projected.get(node).and_then(|previous| Some(previous.paint.clip?.0.x - previous.paint.local_origin.x)).unwrap_or(0.0);
            let max = (presentation.size.width + 1.0 - content.w).max(0.0);
            let (low, high) = caret.map_or((0.0, max), |caret| {
                let low = (caret.right() - content.w).clamp(0.0, max);
                (low, caret.x.clamp(low, max))
            });
            origin.x -= current.clamp(low, high);
        }
        origin
    }

    pub(crate) fn caret_in(&mut self, tree: &Tree, layout: &LayoutKernel, node: NodeId) -> Option<Rect> {
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

    pub(crate) fn selection_rect_in(&mut self, tree: &Tree, layout: &LayoutKernel, node: NodeId) -> Option<Rect> {
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

    pub(crate) fn selection_rect(&self, tree: &Tree, layout: &LayoutKernel, node: NodeId) -> Option<Rect> {
        let selection = |node, range: Range<usize>| {
            let projected = self.projected.get(node)?;
            let paint = &projected.paint;
            let bounds = layout.bounds(node)?;
            let position = Point::new(bounds.x, bounds.y);
            let origin = Point::new(position.x + paint.local_origin.x, position.y + paint.local_origin.y);
            let clip = if tree.editor(node).is_some() {
                paint.clip.map(|(clip, _)| geometry::translate(clip, position))
            } else {
                layout.child_clip(node)
            };
            geometry::union(
                paint
                    .presentation
                    .range_rects(&range, None)
                    .into_iter()
                    .map(|rect| {
                        let rect = geometry::translate(rect, origin);
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

    pub(crate) fn project(&mut self, tree: &Tree, delta: &TreeDelta, layout: &LayoutKernel) {
        self.changed.clear();
        self.changed_links.clear();
        for &retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = retired {
                self.projected.remove(node);
                self.remove_link(node);
            }
        }
        let mut changed = std::mem::take(&mut self.candidates);
        changed.union_with(layout.local_changed());
        changed.extend(layout.scroll_changed().iter().filter(|&node| tree.editor(node).is_some()));
        changed.union_with(&delta.layout_dirty);
        changed.union_with(&delta.editor_dirty);
        if std::mem::take(&mut self.needs_projection) {
            changed.extend(self.projected.keys());
        }
        for node in changed.iter() {
            if tree.text(node).is_none() && tree.editor(node).is_none() {
                continue;
            }
            self.work.nodes_projected += 1;
            let Some(content) = layout.local_content(node) else {
                if self.projected.remove(node) {
                    self.changed.insert(node);
                }
                if let Some(text) = tree.text(node) {
                    for link in &text.links {
                        self.remove_link(*link);
                    }
                }
                if tree.editor(node).is_some_and(|editor| editor.is_secure()) {
                    self.documents.remove(node);
                }
                continue;
            };
            let presentation = self.node_presentation(tree, node, content.w).expect("shown text has a document");
            let caret = if tree.focus() == Some(node) { self.editor_caret_in_content(tree, node, content.w) } else { None };
            let previous = self.projected.get(node);
            let (local_origin, clip) = if tree.editor(node).is_some() {
                let origin = self.editor_origin(tree, layout, node, content, &presentation, caret);
                (origin, Some((content, [true, true])))
            } else {
                let origin = leaf_origin(tree, node, content, presentation.size.height);
                let axes = layout.clip_axes(node);
                (origin, layout.local_client(node).filter(|_| axes != [false; 2]).map(|clip| (clip, axes)))
            };
            let paint = PaintKey { presentation, local_origin, clip, caret };
            let projected_changed = previous.is_none_or(|previous| previous.paint != paint);
            if let Some(text) = tree.text(node) {
                for &link in &text.links {
                    let Some(link_slot) = tree.link(link) else { continue };
                    let rects: Vec<_> = paint
                        .presentation
                        .range_rects(&link_slot.range, None)
                        .into_iter()
                        .map(|rect| geometry::translate(rect, paint.local_origin))
                        .collect();
                    if rects.is_empty() {
                        self.remove_link(link);
                    } else if self.links.get(link) != Some(&rects) {
                        self.links.insert(link, rects);
                        self.changed_links.insert(link);
                    }
                }
            }
            if projected_changed {
                self.changed.insert(node);
                self.projected.insert(node, Projected { paint });
            }
        }
        changed.clear();
        self.candidates = changed;
    }

    pub(crate) fn geometry(&self, node: NodeId, layout: &LayoutKernel) -> Option<TextGeometry> {
        let mut geometry = self.local_geometry(node)?;
        let bounds = layout.bounds(node)?;
        geometry.origin.x += bounds.x;
        geometry.origin.y += bounds.y;
        Some(geometry)
    }

    pub(crate) fn local_geometry(&self, node: NodeId) -> Option<TextGeometry> {
        let projected = self.projected.get(node)?;
        Some(TextGeometry { origin: projected.paint.local_origin, lines: projected.paint.presentation.lines.clone() })
    }

    pub(crate) fn clipped(&self) -> Vec<(NodeId, TextClip)> {
        let mut clipped: Vec<_> =
            self.projected.iter().filter_map(|(node, projected)| Some((node, projected.paint.presentation.clip?))).collect();
        clipped.sort_unstable_by_key(|&(node, _)| node);
        clipped
    }

    pub(crate) fn changed(&self) -> &DirtyBitmap {
        &self.changed
    }

    pub(crate) fn paint_key(&self, node: NodeId) -> Option<PaintKey> {
        Some(self.projected.get(node)?.paint.clone())
    }

    pub(crate) fn link_contains(&self, node: NodeId, layout: &LayoutKernel, pos: Point) -> bool {
        self.link_window_rects(node, layout).any(|rect| rect.contains(pos))
    }

    fn link_window_rects(&self, node: NodeId, layout: &LayoutKernel) -> impl Iterator<Item = Rect> + '_ {
        let origin = layout
            .bounds(node)
            .zip(layout.geometry(node))
            .map_or(Point::default(), |(bounds, layout)| Point::new(bounds.x - layout.rect.x, bounds.y - layout.rect.y));
        self.links.get(node).into_iter().flatten().map(move |rect| geometry::translate(*rect, origin))
    }

    fn remove_link(&mut self, node: NodeId) {
        if self.links.remove(node) {
            self.changed_links.insert(node);
        }
    }

    pub(crate) fn changed_links(&self) -> &DirtyBitmap {
        &self.changed_links
    }

    pub(crate) fn link_local_bounds(&self, node: NodeId) -> Option<Rect> {
        geometry::union(self.links.get(node)?.iter().copied())
    }

    pub(crate) fn local_caret(&self, node: NodeId) -> Option<Rect> {
        let paint = &self.projected.get(node)?.paint;
        let caret = geometry::translate(paint.caret?, paint.local_origin);
        let caret = paint.clip.map_or(caret, |(clip, _)| caret.intersect(&clip));
        (!caret.is_empty()).then_some(caret)
    }

    pub(crate) fn caret(&self, node: NodeId, layout: &LayoutKernel) -> Option<Rect> {
        let bounds = layout.bounds(node)?;
        Some(geometry::translate(self.local_caret(node)?, Point::new(bounds.x, bounds.y)))
    }

    pub(crate) fn items(&self, tree: &Tree, node: NodeId, window_active: bool) -> Vec<Item> {
        let Some(text) = self.projected.get(node).map(|projected| &projected.paint) else { return Vec::new() };
        let mut items = Vec::new();
        let theme = tree.theme_at(node);
        let text_color = tree.visual(node).text;
        let mut foreground = text_color;
        let editor = tree.editor(node);
        let (rich, attachments) = tree.text(node).map_or_else(
            || editor.map_or((&[][..], &[][..]), |editor| (editor.decorations(), editor.attachments())),
            |text| (text.runs.as_slice(), text.attachments.as_slice()),
        );
        let origin = text.local_origin;
        let theme_color = |token| theme.get(token);
        let selection_color = || {
            if window_active { theme_color(ColorToken::Selection) } else { ColorToken::TextSubtle.alpha(0.28).resolve(theme).0 }
        };
        if let Some((rect, axes)) = text.clip {
            items.extend(Item::clip(node, rect, axes, [0.0; 4]));
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
        let pills = || {
            text.presentation.pills.iter().filter_map(|paint| {
                let InlineContent::Pill(pill) = &attachments.get(paint.index)?.content else { return None };
                let rect = geometry::translate(paint.rect, origin);
                (!rect.is_empty()).then_some((paint, pill, rect, pill.style.normalized()))
            })
        };
        for (_, _, rect, style) in pills() {
            let fill = style.background.map(|color| color.resolve(theme).0);
            let border = style.border_color.map(|color| color.resolve(theme).0);
            if fill.is_some() || border.is_some() {
                items.push(Item::Quad(Quad {
                    source: node,
                    rect,
                    radius: rect.corner_radii([style.radius; 4]),
                    fill,
                    gradient: None,
                    border: [f32::from(border.is_some()); 4],
                    border_color: border.unwrap_or(Color::TRANSPARENT),
                }));
            }
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
                    let marked = composition.marked();
                    let mut underline = |range, thickness: f32| {
                        items.extend(range_rects(range).map(|rect| {
                            let height = thickness.min(rect.h);
                            quad(node, Rect::new(rect.x, rect.bottom() - height, rect.w, height), foreground)
                        }));
                    };
                    let mut start = marked.start;
                    for clause in composition.clauses() {
                        let range = marked.start + clause.range.start..marked.start + clause.range.end;
                        underline(start..range.start, 1.0);
                        start = range.end;
                        let thickness = match clause.style {
                            ClauseStyle::Clause => 1.0,
                            ClauseStyle::ActiveClause => 2.0,
                        };
                        underline(range, thickness);
                    }
                    underline(start..marked.end, 1.0);
                }
            }
        } else if let Some(range) = doc_selection(tree, node) {
            let color = selection_color();
            items.extend(range_rects(range).map(|rect| quad(node, rect, color)));
        }
        let color = |index| brush(rich, index).and_then(|span| span.color).map_or(foreground, |color| color.resolve(theme).0);
        items.extend(text.presentation.glyphs.iter().map(|run| run.item(node, color(run.brush), origin)));
        for (paint, pill, rect, style) in pills() {
            let range_start = attachments[paint.index].range.start;
            let color = style
                .color
                .or_else(|| rich.iter().rev().filter(|(range, _)| range.contains(&range_start)).find_map(|(_, span)| span.color))
                .map_or(text_color, |color| color.resolve(theme).0);
            if let Some(icon) = &pill.icon {
                let y = rect.y + (rect.h - style.size) * 0.5;
                paint_icon(node, icon, Rect::new(rect.x + style.padding_x, y, style.size, style.size), color, &mut items);
            }
            items.extend(paint.glyphs.iter().map(|run| run.item(node, color, origin)));
        }
        items
            .extend(text.presentation.decorations.iter().map(|line| quad(node, geometry::translate(line.rect, origin), color(line.brush))));
        if text.clip.is_some() {
            items.push(Item::PopClip);
        }
        items
    }

    pub(crate) fn attachments(&self, node: NodeId) -> (Option<Item>, Vec<(usize, Rect)>) {
        let Some(text) = self.projected.get(node).map(|projected| &projected.paint) else { return (None, Vec::new()) };
        let positions =
            text.presentation.attachments.iter().map(|&(index, rect)| (index, geometry::translate(rect, text.local_origin))).collect();
        (text.clip.and_then(|(rect, axes)| Item::clip(node, rect, axes, [0.0; 4])), positions)
    }

    pub(crate) fn attachment_bounds(&self, node: NodeId, index: usize, layout: &LayoutKernel) -> Option<Rect> {
        let text = self.projected.get(node)?;
        let presentation = &text.paint.presentation;
        let rect = presentation
            .attachments
            .iter()
            .find_map(|&(candidate, rect)| (candidate == index).then_some(rect))
            .or_else(|| presentation.pills.iter().find_map(|pill| (pill.index == index).then_some(pill.rect)))?;
        let bounds = layout.bounds(node)?;
        let origin = Point::new(bounds.x + text.paint.local_origin.x, bounds.y + text.paint.local_origin.y);
        Some(geometry::translate(rect, origin))
    }

    pub(crate) fn canvas_items(&mut self, source: NodeId, text: &CanvasText, mut style: TextStyle, color: Color) -> Vec<Item> {
        let ratio = style.line_height / style.font_size.max(1.0);
        style.font_size = text.size.map_or(style.font_size, |size| size * style.text_scale) * text.scale;
        style.line_height = style.font_size * ratio;
        style.weight = text.weight.unwrap_or(style.weight);
        style.wrap = if text.max_width.is_some() { Wrap::Words } else { Wrap::None };
        style.align = TextAlign::Start;
        style.max_lines = None;
        let document = Document::new(self.paragraphs(&text.text, style, &[], &[], true), style, Vec::new(), false);
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
        static NEXT_FACE: CacheLine<AtomicU64> = CacheLine(AtomicU64::new(1));
        self.faces
            .entry((font.data.id(), font.index))
            .or_insert_with(|| FontFace {
                id: FontId(NEXT_FACE.fetch_add(1, Ordering::Relaxed)),
                data: FontData::new(font.data.clone().into_raw_parts().0),
                index: font.index,
            })
            .clone()
    }
}

fn shape_bytes(text: &str) -> usize {
    2048 + size_of::<MeasureCache>() + text.len().saturating_mul(64)
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

    #[cfg(any(unix, windows))]
    #[test]
    fn mapped_font_reaches_rasterization_without_copying() {
        use crate::{NativeEngine, NativeEngineConfig};

        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/Inter-Regular.ttf"));
        let mut engine = NativeEngine::new(NativeEngineConfig::default());
        engine.text.fonts.collection.clear();
        // SAFETY: The repository font fixture is immutable during the test.
        unsafe { engine.register_font_file(path, &[*b"Latn"]) }.unwrap();
        let inter = engine.text.fonts.collection.family_by_name("Inter").unwrap();
        let address = inter.fonts()[0].load(None).unwrap().data().as_ptr();
        drop(inter);
        let style = TextStyle { family: "Inter", ..TextStyle::default() };
        let document = Document::new(engine.text.paragraphs("Hi", style, &[], &[], true), style, Vec::new(), false);
        let mut presentation = engine.text.present(&document, None);
        assert_eq!(presentation.glyphs.len(), 1);
        let run = presentation.glyphs.remove(0);
        assert!(run.glyphs.iter().all(|glyph| glyph.id != 0));
        drop(engine);
        drop(document);
        assert_eq!(run.font.data.as_ptr(), address);
        assert_eq!(run.font.data.as_ref(), std::fs::read(path).unwrap().as_slice());
    }

    #[test]
    fn paragraph_direction_changes_alignment_and_invalidates_the_shape_cache() {
        let mut engine = TextEngine::new(false);
        let layout = |engine: &mut TextEngine, direction| {
            let style = TextStyle { direction, ..TextStyle::default() };
            let paragraphs = engine.paragraphs("ABC אבג", style, &[], &[], true);
            assert_eq!(paragraphs[0].shape.borrow().layout.is_rtl(), direction == Direction::Rtl);
            let document = Document::new(paragraphs, style, Vec::new(), false);
            engine.present(&document, Some(240.0))
        };
        let ltr = layout(&mut engine, Direction::Ltr);
        let rtl = layout(&mut engine, Direction::Rtl);
        assert!(ltr.lines[0].fragments.first().unwrap().range.start < 4);
        assert!(rtl.lines[0].fragments.first().unwrap().rtl);
        assert!(rtl.lines[0].rect.x > ltr.lines[0].rect.x);
        assert!((rtl.lines[0].rect.right() - 240.0).abs() < 0.1);
        let repeated = layout(&mut engine, Direction::Rtl);
        assert_eq!(rtl.lines, repeated.lines);
        assert_eq!(engine.work.shapes_built, 2);
    }

    #[test]
    fn measurement_probes_survive_node_recycling_and_keep_final_lines_warm() {
        use octara_model::style::StyleProp;
        use octara_tree::tables::AuthoredStyle;
        use octara_tree::{NodeChange, Retirement};

        let mut tree = Tree::new(TreeId::new([3, 5], 6), Default::default());
        let label = |tx: &mut TxBuilder| {
            let label = tx.create(NodeSpec::new(NodeContent::Text("Words that wrap across several lines".into())));
            tx.change(
                label,
                NodeChange::Style(Box::new(AuthoredStyle {
                    base: [StyleProp::TextAlign(TextAlign::Justify)].into_iter().collect(),
                    ..Default::default()
                })),
            );
            label
        };
        let mut tx = tree.transaction();
        let (first, second) = (label(&mut tx), label(&mut tx));
        tx.place(tree.node_ref(tree.root()), 0, first);
        tx.place(tree.node_ref(tree.root()), 1, second);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let (first, second) = (receipt.node(first).unwrap(), receipt.node(second).unwrap());
        let mut text = TextEngine::new(false);
        let widths = [None, Some(0.0), Some(48.0), Some(92.0), Some(128.0)];
        let measured = widths.map(|width| text.measure(&tree, first, None, width).unwrap());
        let cold = text.work();
        assert_eq!(cold.line_breaks, 5);
        assert!(measured[0].width > measured[4].width);
        assert!(measured[0].height < measured[4].height);

        let warm = widths.map(|width| text.measure(&tree, second, None, width).unwrap());
        assert_eq!(warm, measured);
        assert_eq!(text.work().measure_probe_hits - cold.measure_probe_hits, 5);
        assert_eq!(text.work().line_breaks, cold.line_breaks);
        let final_lines = text.node_presentation(&tree, second, 128.0).unwrap();
        assert_eq!(final_lines.size.height.ceil(), measured[4].height);
        assert_eq!(text.work().line_breaks, cold.line_breaks);

        let mut tx = tree.transaction();
        tx.retire(tree.node_ref(first), Retirement::Destroy);
        tx.retire(tree.node_ref(second), Retirement::Destroy);
        let (_, retired) = tree.apply(tx.finish()).unwrap();
        text.retire(&retired);
        assert!(text.documents.is_empty());
        let mut tx = tree.transaction();
        let replacement = label(&mut tx);
        tx.place(tree.node_ref(tree.root()), 0, replacement);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let replacement = receipt.node(replacement).unwrap();
        assert_ne!(replacement, second);

        let recreated = widths.map(|width| text.measure(&tree, replacement, None, width).unwrap());
        assert_eq!(recreated, measured);
        assert_eq!(text.work().shapes_built, cold.shapes_built);
        assert_eq!(text.work().measure_probe_hits - cold.measure_probe_hits, 10);
        assert_eq!(text.work().line_breaks, cold.line_breaks);

        let warm = text.work();
        let mut oracle = text.fork_for_layout_check();
        assert_eq!(oracle.measure(&tree, replacement, None, widths[4]), Some(measured[4]));
        assert_eq!(oracle.work().shapes_built, 1);
        assert_eq!(oracle.work().measure_probe_hits, 0);
        assert_eq!(oracle.work().line_breaks, 1);
        assert_eq!(text.work(), warm);
    }

    #[test]
    fn measurement_matches_mixed_direction_rich_inline_lines() {
        use octara_model::text::InlineAlign;

        let value = "Latin אבג tail item after\nSecond paragraph with several words";
        let span = Span {
            range: 6..12,
            brush: 1,
            family: None,
            size: Some(22.3),
            weight: Some(600),
            italic: true,
            underline: false,
            strike: false,
            inset: 6.0,
        };
        let item = value.find("item").unwrap();
        let attachments = [InlineAttachment {
            range: item..item + "item".len(),
            size: Size::default(),
            align: InlineAlign::Baseline,
            baseline: None,
            content: InlineContent::Spacer { width: 32.0 },
        }];
        let mut text = TextEngine::new(false);
        let style = TextStyle { direction: Direction::Rtl, line_height: 24.1, ..TextStyle::default() };
        let paragraphs = text.paragraphs(value, style, &[span], &attachments, true);
        let mut document = Document::new(paragraphs, style, Vec::new(), false);
        for width in [None, Some(0.0), Some(80.0), Some(115.0)] {
            let shown = text.present(&document, width);
            assert!(shown.lines.iter().flat_map(|line| line.fragments.iter()).any(|fragment| fragment.rtl));
            assert_eq!(shown.attachments[0].1.w, 32.0);
            assert_eq!(document.measure(width, usize::MAX, &mut text.work), (shown.size, shown.lines.len()));
            let first = shown.lines[0].rect;
            assert_eq!(document.measure(width, 1, &mut text.work), (Size::new(first.w, first.h), 1));
        }

        document.style.wrap = Wrap::None;
        // Width-independent probes leave the shapes at the last definite width.
        let unwrapped = document.measure(None, usize::MAX, &mut text.work);
        assert_eq!(unwrapped.1, 2);
        let breaks = text.work().line_breaks;
        assert_eq!(document.measure(Some(64.0), usize::MAX, &mut text.work), unwrapped);
        document.style.wrap = Wrap::Truncate;
        let (first, count) = document.measure(None, usize::MAX, &mut text.work);
        assert_eq!(count, 1);
        assert_eq!(document.measure(Some(12.0), usize::MAX, &mut text.work), (Size::new(12.0, first.height), 1));
        assert_eq!(text.work().line_breaks, breaks);
    }

    #[test]
    fn ancestor_translation_keeps_local_text_and_links_warm() {
        use octara_model::style::StyleProp;
        use octara_tree::NodeChange;
        use octara_tree::tables::{AuthoredStyle, LinkSlot};
        use octara_tree::transaction::RichText;

        let mut tree = Tree::new(TreeId::new([3, 5], 7), Default::default());
        let mut delta = tree.initial_delta();
        let mut tx = tree.transaction();
        let root = tree.node_ref(tree.root());
        let spacer = tx.create(NodeSpec::new(NodeContent::Box));
        let row = tx.create(NodeSpec::new(NodeContent::Box));
        let label = tx.create(NodeSpec::new(NodeContent::Text("".into())));
        let height = |value: f32| {
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [StyleProp::Height(value.into()), StyleProp::Shrink(0.0)].into_iter().collect(),
                ..Default::default()
            }))
        };
        tx.change(spacer, height(20.0));
        tx.change(row, height(32.0));
        tx.change(
            label,
            NodeChange::RichText(Box::new(RichText {
                text: "link".into(),
                links: vec![LinkSlot { range: 0..4, href: "#link".into() }],
                ..Default::default()
            })),
        );
        tx.place(root, 0, spacer);
        tx.place(root, 1, row);
        tx.place(row, 0, label);
        let (receipt, created) = tree.apply(tx.finish()).unwrap();
        delta.merge(created);
        let (spacer, label) = (receipt.node(spacer).unwrap(), receipt.node(label).unwrap());
        let link = tree.children(label)[0];
        let viewport = WindowViewport { size: Size::new(300.0, 160.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let mut layout = LayoutKernel::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        let before = text.geometry(label, &layout).unwrap();
        let link_before = text.link_window_rects(link, &layout).next().unwrap();
        let work = text.work();

        let mut tx = tree.transaction();
        tx.change(tree.node_ref(spacer), height(36.0));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        layout.project(&tree, &delta, viewport, &mut text);
        let after = text.geometry(label, &layout).unwrap();
        let link_after = text.link_window_rects(link, &layout).next().unwrap();
        assert_eq!(after.origin.y - before.origin.y, 16.0);
        assert!(Arc::ptr_eq(&after.lines, &before.lines));
        assert_eq!(link_after, Rect { y: link_before.y + 16.0, ..link_before });
        assert_eq!(text.work().nodes_projected, work.nodes_projected);
    }

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
        let viewport = WindowViewport { size: Size::new(300.0, 160.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let mut layout = LayoutKernel::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        let geometry = text.geometry(label, &layout).unwrap();
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
        let document = Document::new(engine.paragraphs(value, style, &[], &attachments, true), style, Vec::new(), false);
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
    fn inline_pills_use_natural_font_metrics_and_keep_logical_geometry() {
        use octara_model::paint::{Icon, NodeIcon, Path};
        use octara_model::text::{InlinePill, PillStyle};

        let value = "A aéb B";
        let source = 2..6;
        let style = TextStyle { family: "Inter", font_size: 12.0, line_height: 18.0, ..Default::default() };
        let spans = [Span {
            range: 3..5,
            brush: 1,
            family: None,
            size: None,
            weight: None,
            italic: false,
            underline: false,
            strike: false,
            inset: 4.0,
        }];
        let pill_style = PillStyle { size: 20.0, weight: 600, padding_x: 3.5, padding_y: 4.25, gap: 2.75, ..Default::default() };
        let icon = NodeIcon::new(&Icon::path(Path::rect(Rect::new(4.0, 4.0, 16.0, 16.0))), None);
        let mut engine = TextEngine::new(false);
        let mut widths = Vec::new();
        for label in ["iiii", "WWWW", "WWWW\r\n"] {
            let pill = InlinePill { icon: Some(icon.clone()), label: label.into(), style: pill_style };
            let visual_label = pill.layout_label();
            let mut builder = engine.layouts.ranged_builder(&mut engine.fonts, &visual_label, 1.0, false);
            builder.push_default(StyleProperty::FontFamily(style.family.into()));
            builder.push_default(StyleProperty::FontSize(pill_style.size));
            builder.push_default(StyleProperty::FontWeight(FontWeight::new(f32::from(pill_style.weight))));
            let mut reference = builder.build(&visual_label);
            reference.break_all_lines(None);
            let expected_width = 2.0 * pill_style.padding_x + pill_style.size + pill_style.gap + reference.full_width();
            let expected_height = 2.0 * pill_style.padding_y + pill_style.size.max(reference.height());

            let attachments = [pill.attachment(source.clone())];
            let document = Document::new(engine.paragraphs(value, style, &spans, &attachments, true), style, Vec::new(), false);
            let drawing = engine.present(&document, None);
            assert_eq!(drawing.lines.len(), 1);
            assert_eq!(drawing.pills.len(), 1);
            assert!(drawing.padding.is_empty());
            let paint = &drawing.pills[0];
            assert!(paint.glyphs.iter().flat_map(|run| run.glyphs.iter()).all(|glyph| glyph.source_end == 0));
            assert!((paint.rect.w - expected_width).abs() < 0.01);
            assert!((paint.rect.h - expected_height).abs() < 0.01);
            widths.push(paint.rect.w);
            let glyph_baseline = |run: &Run| run.offset.y + run.glyphs[0].y;
            assert!((glyph_baseline(&drawing.glyphs[0]) - glyph_baseline(&paint.glyphs[0])).abs() < 0.01);

            let selection = drawing.range_rects(&source, None);
            assert_eq!(selection.len(), 1);
            assert!((selection[0].w - paint.rect.w).abs() < 0.01);
            let geometry = TextGeometry { origin: Point::default(), lines: drawing.lines.clone() };
            assert_eq!(geometry::hit_byte(&geometry, Point::new(paint.rect.x + 1.0, paint.rect.y)), Some(source.start));
            assert_eq!(geometry::hit_byte(&geometry, Point::new(paint.rect.right() - 1.0, paint.rect.y)), Some(source.end));

            let wrapped = engine.present(&document, Some(paint.rect.w + 0.5));
            assert!(wrapped.lines.len() > 1);
            assert_eq!(wrapped.pills.len(), 1);
            assert_eq!(wrapped.pills[0].rect.w, paint.rect.w);
        }
        assert!(widths[0] < widths[1]);
        assert!(widths[1] < widths[2]);
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
        let viewport = WindowViewport { size: Size::new(300.0, 160.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
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
        engine.set_accessibility_active(&tree, crate::AccessibilityConsumer::Inspector, true);
        assert!(engine.publication().snapshot_full().unwrap().nodes.iter().any(|(_, node)| node.value() == Some(source)));
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
        let document = |engine: &mut TextEngine, text, span, style| {
            Document::new(engine.paragraphs(text, style, &[span], &[], true), style, Vec::new(), false)
        };
        let plain = document(&mut engine, "go café", span(3..8, 0.0), style);
        let padded = document(&mut engine, "go café", span(3..8, 8.0), style);
        let (plain_size, _) = plain.measure(None, usize::MAX, &mut engine.work);
        let (padded_size, _) = padded.measure(None, usize::MAX, &mut engine.work);
        assert!((padded_size.width - plain_size.width - 16.0).abs() < 0.01);
        assert_eq!(plain_size.height, padded_size.height);
        let width = plain_size.width + 1.0;
        let (_, line_count) = padded.measure(Some(width), usize::MAX, &mut engine.work);
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
        let viewport = WindowViewport { size: Size::new(600.0, 160.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let mut layout = LayoutKernel::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        for (label, fraction) in labels {
            let label = receipt.node(label).unwrap();
            let link = tree.children(label)[0];
            let content = layout.content_bounds(label).unwrap();
            let geometry = text.geometry(label, &layout).unwrap();
            let line_height = geometry.lines[0].rect.h;
            assert_eq!(geometry.origin.y, content.y + fraction * (content.h - line_height));
            assert_eq!(text.link_window_rects(link, &layout).collect::<Vec<_>>(), &[layout.bounds(link).unwrap()]);
            assert_eq!(text.link_window_rects(link, &layout).collect::<Vec<_>>()[0].y, geometry.origin.y);
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
        let viewport = WindowViewport { size: Size::new(350.0, 400.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let mut layout = LayoutKernel::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        for (label, padding) in labels {
            let label = receipt.node(label).unwrap();
            let geometry = text.geometry(label, &layout).unwrap();
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
            engine.present(&Document::new(paragraphs, style, Vec::new(), false), None)
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
        let viewport = WindowViewport { size: Size::new(300.0, 160.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let mut layout = LayoutKernel::default();
        let mut text = TextEngine::new(false);
        layout.project(&tree, &delta, viewport, &mut text);
        assert_eq!(text.clipped(), vec![(label, TextClip::Ellipsis)]);
        let link_bounds = layout.bounds(link).expect("partially visible inline link geometry");
        assert_eq!(text.link_window_rects(link, &layout).collect::<Vec<_>>(), &[link_bounds]);
        assert!(link_bounds.w < 60.0);
        let geometry = text.geometry(editor, &layout).unwrap();
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
