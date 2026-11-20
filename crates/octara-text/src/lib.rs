//! Window-independent text layout ([design](../../../docs/design/text.md#the-engine)): a
//! replaceable [`TextEngine`] makes [`TextLayout`]s, which text nodes own.
//!
//! [`ParleyEngine`] (the default) shapes with Parley. Headless it has the bundled Inter and IBM
//! Plex Mono only (`system-ui` and `ui-monospace` resolve to them), so a run measures the same
//! on every machine; `ParleyEngine::system` (with `system-fonts`) resolves them to the platform's
//! fonts. It keeps
//! shaped paragraphs by content (text, style, runs), shared by every text showing them, within
//! a byte budget: appending to a stream reshapes its last paragraph, a row scrolling back or a
//! repeated label reshapes nothing, and a new width only breaks lines again. [`MonoEngine`] is
//! a deterministic monospace model for isolating text cost in benchmarks.
use octara_primitives::paint::Rect;
use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, Script, SourceCache};
use parley::{FontContext, FontWeight, Layout, LayoutContext, StyleProperty};
use rustc_hash::{FxHashMap, FxHasher};
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

/// How a text uses the width it is given.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Wrap {
    /// Breaks between words to fit (CSS default).
    #[default]
    Words,
    /// One line per paragraph, as wide as it needs.
    None,
    /// One line, cut to the width it gets (shown with an ellipsis); can shrink to nothing.
    Truncate,
}

/// What a text engine needs to know about a text's style.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextStyle {
    pub font_size: f32,
    /// A family: a registered name, or `system-ui` / `ui-monospace` (resolved per engine).
    pub family: &'static str,
    /// Line height in pixels.
    pub line_height: f32,
    pub weight: u16,
    pub wrap: Wrap,
    pub max_lines: Option<u16>,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle { font_size: 16.0, family: "system-ui", line_height: 24.0, weight: 400, wrap: Wrap::Words, max_lines: None }
    }
}

/// A text laid out at some width.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct TextMetrics {
    pub width: f32,
    pub height: f32,
    /// Lines shown (after `max_lines`).
    pub lines: u32,
    /// Some content is not shown: lines past `max_lines`, or a truncated line.
    pub truncated: bool,
}

/// A styled range of one paragraph as the engine shapes it: the properties that change shapes,
/// and the brush its glyphs carry (the text's run number from 1; 0 is the text's own style).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StyleRun {
    pub start: usize,
    pub end: usize,
    pub brush: u16,
    pub family: Option<&'static str>,
    pub size: Option<f32>,
    pub weight: Option<u16>,
    pub italic: bool,
}

/// A text laid out by an engine: what a text node owns (glyphs, lines, links, selections and
/// accessibility runs derive from it). Breaking at a width is kept inside it.
pub trait LaidText {
    /// Width, line count and height of every paragraph broken at `max_width` (`None`:
    /// unbounded), before any line limit.
    fn measure(&self, max_width: Option<f32>) -> (f32, u32, f32);
    /// Drawing and visible source geometry, relative to the text's top-left. Line limits and
    /// fitting are applied together, at whole shaping-cluster boundaries. An ellipsis is
    /// synthetic: it has geometry and a visible boundary, but owns no source bytes or brush.
    /// Even engines without glyphs must return the visible source geometry.
    fn present(
        &self,
        max_width: Option<f32>,
        max_lines: Option<u16>,
        fit: Option<f32>,
        ellipsis: Option<&(Vec<ShapedRun>, f32)>,
    ) -> TextPresentation;
    fn as_any(&self) -> &dyn std::any::Any;
}

/// A text layout as nodes hold it.
pub type TextLayout = Rc<dyn LaidText>;

/// Lays texts out.
pub trait TextEngine {
    /// Lays `text` out in `style`, `runs` styling ranges of it (byte offsets in the text,
    /// sorted). The paragraphs of `previous` (this text's last layout) that end within its first
    /// `clean` bytes are reused as they are; the others are found by content, or shaped.
    fn layout(&mut self, text: &str, style: &TextStyle, runs: &[StyleRun], previous: Option<&TextLayout>, clean: usize) -> TextLayout;
    /// "…" in `style`: its runs and width (truncated text ends in it).
    fn ellipsis(&mut self, _style: &TextStyle) -> Option<(Vec<ShapedRun>, f32)> {
        None
    }
    /// Adds font data (TTF/OTF, possibly a collection); its families become available by name.
    fn register_font(&mut self, _data: &'static [u8]) {}
    /// Adds font data whose families text falls back to for `scripts` (ISO 15924 tags such as
    /// `*b"Hani"`) when the styled family lacks a character.
    fn register_fallback_font(&mut self, _data: &'static [u8], _scripts: &[[u8; 4]]) {}
    /// Paragraphs shaped so far (for tests and benchmarks; 0 where not counted).
    fn shaped(&self) -> u64 {
        0
    }
    /// What the engine keeps of shaped paragraphs, in (estimated) bytes.
    fn kept_bytes(&self) -> usize {
        0
    }
}

/// A layout's metrics under a style's wrapping and line limit, given the width it gets.
pub fn metrics(layout: &TextLayout, style: &TextStyle, limit: Option<f32>) -> TextMetrics {
    let capped = |(width, lines, height): (f32, u32, f32)| {
        let shown = style.max_lines.map_or(lines, |m| lines.min(m as u32));
        let height = if shown < lines { shown as f32 * style.line_height } else { height };
        TextMetrics { width, height, lines: shown, truncated: shown < lines }
    };
    match style.wrap {
        Wrap::Words => capped(layout.measure(limit)),
        Wrap::None => capped(layout.measure(None)),
        Wrap::Truncate => {
            let natural = capped(layout.measure(None));
            let width = limit.map_or(natural.width, |l| natural.width.min(l));
            let cut = natural.lines > 1 || width < natural.width;
            TextMetrics { width, height: style.line_height, lines: 1, truncated: cut }
        }
    }
}

/// One visible grapheme's source bytes and advance. Logical edges run right-to-left when
/// `rtl` is true. Several fragments may share a shaping ligature; fitting keeps it whole.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFragment {
    pub source: Range<usize>,
    pub x: f32,
    pub width: f32,
    pub rtl: bool,
}

impl TextFragment {
    /// The logical start and end caret coordinates, preserving bidi boundary affinity.
    pub fn edges(&self) -> (f32, f32) {
        if self.rtl { (self.x + self.width, self.x) } else { (self.x, self.x + self.width) }
    }
}

/// A synthetic ellipsis. Pressing or selecting it maps to `boundary`, the source boundary at
/// the physical right edge of the retained fragments (or the empty line's start).
#[derive(Clone, Debug, PartialEq)]
pub struct TextEllipsis {
    pub rect: Rect,
    pub boundary: usize,
}

/// A presented line. Fragments are in visual order and never include hidden source bytes.
/// A hard break owns the LF or CRLF bytes when the following line is presented. `break_at`
/// also marks a visible soft wrap for document selection painting.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentedLine {
    pub start: usize,
    pub top: f32,
    pub height: f32,
    pub fragments: Vec<TextFragment>,
    pub hard_break: Option<Range<usize>>,
    pub break_at: Option<usize>,
    pub ellipsis: Option<TextEllipsis>,
}

impl PresentedLine {
    /// Geometry of a visible source range, coalesced only across touching visual fragments.
    pub fn range_rects(&self, range: Range<usize>) -> Vec<Rect> {
        let mut rects: Vec<Rect> = Vec::new();
        for f in &self.fragments {
            if f.source.start >= range.end || f.source.end <= range.start || f.width <= 0.0 {
                continue;
            }
            if let Some(last) = rects.last_mut()
                && (last.right() - f.x).abs() <= 0.01
            {
                last.w = f.x + f.width - last.x;
            } else {
                rects.push(Rect { x: f.x, y: self.top, w: f.width, h: self.height });
            }
        }
        rects
    }

    /// Both edges of a visible grapheme (a hard newline has zero advance).
    pub fn edges(&self, range: &Range<usize>) -> Option<(f32, f32)> {
        if self.hard_break.as_ref().is_some_and(|b| b.start <= range.start && b.end >= range.end) {
            let x = self.break_x();
            return Some((x, x));
        }
        self.fragments.iter().find(|f| f.source.start <= range.start && f.source.end >= range.end).map(TextFragment::edges)
    }

    /// The logical line-end edge used by a visible hard break or soft wrap.
    pub fn break_x(&self) -> f32 {
        let Some(end) = self.break_at.or_else(|| self.hard_break.as_ref().map(|b| b.start)) else { return self.right_boundary().1 };
        self.fragments.iter().find(|f| f.source.end == end).map_or(0.0, |f| f.edges().1)
    }

    fn right_boundary(&self) -> (usize, f32) {
        self.fragments.last().map_or((self.start, 0.0), |f| (if f.rtl { f.source.start } else { f.source.end }, f.x + f.width))
    }
}

/// A single document presentation: glyphs, visible source fragments and synthetic ellipses.
/// Editable navigation can derive its geometry from the full, unfitted presentation.
#[derive(Clone, Debug, Default)]
pub struct TextPresentation {
    pub glyphs: Vec<ShapedRun>,
    pub lines: Vec<PresentedLine>,
}

impl TextPresentation {
    /// Only the visible fragments of `range`, excluding synthetic ellipses.
    pub fn range_rects(&self, range: Range<usize>) -> Vec<Rect> {
        self.lines.iter().flat_map(|line| line.range_rects(range.clone())).collect()
    }

    /// The nearest visible caret boundary. An ellipsis maps to its retained boundary; hidden
    /// bytes are never manufactured as pointer destinations.
    pub fn offset_at(&self, x: f32, y: f32) -> usize {
        let Some(line) = self.lines.iter().find(|l| y < l.top + l.height).or(self.lines.last()) else { return 0 };
        if let Some(e) = &line.ellipsis
            && x >= e.rect.x
        {
            return e.boundary;
        }
        line.fragments
            .iter()
            .flat_map(|f| {
                let (start, end) = f.edges();
                [(f.source.start, start), (f.source.end, end)]
            })
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map_or(line.start, |s| s.0)
    }
}

/// Cluster/source and glyph intervals are recorded together while shaping data is available.
/// Atoms need no per-grapheme allocation or duplicated font/run metadata.
struct PresentationAtom {
    fragments: Range<usize>,
    glyph_end: Option<(usize, usize)>,
    space: bool,
}

struct PresentationLine {
    line: PresentedLine,
    atoms: Vec<PresentationAtom>,
    glyphs: Vec<ShapedRun>,
    baseline: f32,
}

fn present_lines(
    mut lines: Vec<PresentationLine>,
    max_lines: Option<u16>,
    fit: Option<f32>,
    ellipsis: Option<&(Vec<ShapedRun>, f32)>,
) -> TextPresentation {
    let count = max_lines.map_or(lines.len(), usize::from);
    let hidden = lines.len() > count;
    lines.truncate(count);
    let mut dots = Vec::new();
    if let Some(last) = lines.last_mut() {
        let end = last.line.fragments.last().map_or(0.0, |f| f.x + f.width);
        if hidden {
            last.line.hard_break = None;
            last.line.break_at = None;
        }
        if let Some(fit) = fit
            && (hidden || end > fit + 0.01)
        {
            let ellipsis = ellipsis.filter(|(_, w)| *w <= fit && *w > 0.0);
            let limit = (fit - ellipsis.map_or(0.0, |(_, w)| *w)).max(0.0);
            let keep = last
                .atoms
                .iter()
                .take_while(|a| last.line.fragments.get(a.fragments.end - 1).is_none_or(|f| f.x + f.width <= limit + 0.01))
                .count();
            last.atoms.truncate(keep);
            while last.atoms.last().is_some_and(|a| a.space) {
                last.atoms.pop();
            }
            last.line.hard_break = None;
            last.line.break_at = None;
            last.line.fragments.truncate(last.atoms.last().map_or(0, |a| a.fragments.end));
            if let Some((atom, (run, end))) = last.atoms.iter().rev().find_map(|a| a.glyph_end.map(|g| (a, g))) {
                last.glyphs.truncate(run + 1);
                let run = &mut last.glyphs[run];
                run.glyphs.truncate(end);
                let f = &last.line.fragments[atom.fragments.end - 1];
                run.width = f.x + f.width - run.x;
            } else {
                last.glyphs.clear();
            }
            let (boundary, at) = last.line.right_boundary();
            if let Some((runs, width)) = ellipsis {
                last.line.ellipsis =
                    Some(TextEllipsis { rect: Rect { x: at, y: last.line.top, w: *width, h: last.line.height }, boundary });
                for run in runs {
                    let mut run = run.clone();
                    for glyph in &mut run.glyphs {
                        glyph.x += at;
                        glyph.y += last.baseline - run.baseline;
                    }
                    run.x += at;
                    run.brush = 0;
                    run.baseline = last.baseline;
                    run.line_top = last.line.top;
                    run.line_height = last.line.height;
                    dots.push(run);
                }
            }
        }
    }
    let mut out = TextPresentation::default();
    for l in lines {
        out.glyphs.extend(l.glyphs);
        out.lines.push(l.line);
    }
    out.glyphs.extend(dots);
    out
}

/// Paragraph source offsets exclude their LF or CRLF while preserving the original bytes.
fn paragraphs(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split('\n').scan(0, move |base, raw| {
        let start = *base;
        let end = start + raw.len();
        *base = end + 1;
        let paragraph = if end < text.len() { raw.strip_suffix('\r').unwrap_or(raw) } else { raw };
        Some((start, paragraph))
    })
}

/// Monospace lines: a space that breaks a line stays at its end.
fn even_lines(text: &str, em: f32, max_width: Option<f32>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for (base, para) in paragraphs(text) {
        // Line starts: a word that does not fit after the previous one begins a line.
        let mut starts = vec![0usize];
        if let Some(max) = max_width {
            let (mut line, mut at) = (0.0f32, 0usize);
            for word in para.split(' ') {
                let w = word.chars().count() as f32 * em;
                let with_space = if at > *starts.last().unwrap() { line + em + w } else { w };
                if at > *starts.last().unwrap() && with_space > max {
                    starts.push(at);
                    line = w;
                } else {
                    line = with_space;
                }
                at += word.len() + 1;
            }
        }
        for (i, &s) in starts.iter().enumerate() {
            let e = starts.get(i + 1).copied().unwrap_or(para.len());
            out.push(base + s..base + e);
        }
    }
    out
}

/// Glyphs of one font, positioned relative to their text's top-left corner.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub font: linebender_resource_handle::FontData,
    pub size: f32,
    pub coords: Vec<i16>,
    pub embolden: bool,
    pub glyphs: Vec<octara_primitives::paint::Glyph>,
    /// The text's run this is from (0: the text's own style).
    pub brush: u16,
    /// Where the run starts on its line and how far it advances.
    pub x: f32,
    pub width: f32,
    /// Its baseline, and its line's top and height, from the text's top.
    pub baseline: f32,
    pub line_top: f32,
    pub line_height: f32,
}

// ---- monospace ----

/// Every character is half an em wide; breaks at spaces. Deterministic and nearly free.
#[derive(Default)]
pub struct MonoEngine;

struct MonoText {
    text: String,
    em: f32,
    line_height: f32,
}

impl MonoText {
    fn paragraph(&self, text: &str, max_width: Option<f32>) -> (f32, u32) {
        let em = self.em;
        let Some(max) = max_width else { return (text.chars().count() as f32 * em, 1) };
        let (mut widest, mut line, mut lines) = (0.0f32, 0.0f32, 1u32);
        for word in text.split(' ') {
            let w = word.chars().count() as f32 * em;
            let with_space = if line > 0.0 { line + em + w } else { w };
            if line > 0.0 && with_space > max {
                widest = widest.max(line);
                lines += 1;
                line = w;
            } else {
                line = with_space;
            }
        }
        (widest.max(line), lines)
    }
}

impl LaidText for MonoText {
    fn measure(&self, max_width: Option<f32>) -> (f32, u32, f32) {
        let (mut width, mut lines) = (0.0f32, 0u32);
        for (_, p) in paragraphs(&self.text) {
            let (w, l) = self.paragraph(p, max_width);
            width = width.max(w);
            lines += l;
        }
        (width, lines, lines as f32 * self.line_height)
    }
    fn present(
        &self,
        max_width: Option<f32>,
        max_lines: Option<u16>,
        fit: Option<f32>,
        ellipsis: Option<&(Vec<ShapedRun>, f32)>,
    ) -> TextPresentation {
        let lines = even_lines(&self.text, self.em, max_width);
        let mut presented = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            let mut x = 0.0;
            let mut fragments = Vec::new();
            let atoms = self.text[l.start..l.end]
                .grapheme_indices(true)
                .map(|(b, g)| {
                    let width = g.chars().count() as f32 * self.em;
                    fragments.push(TextFragment { source: l.start + b..l.start + b + g.len(), x, width, rtl: false });
                    x += width;
                    PresentationAtom {
                        fragments: fragments.len() - 1..fragments.len(),
                        glyph_end: None,
                        space: g.chars().all(|c| c == ' ' || c == '\u{a0}'),
                    }
                })
                .collect();
            let next_start = lines.get(i + 1).map(|next| next.start);
            let hard_break = next_start.filter(|&start| start > l.end).map(|start| l.end..start);
            let top = i as f32 * self.line_height;
            presented.push(PresentationLine {
                line: PresentedLine {
                    start: l.start,
                    top,
                    height: self.line_height,
                    fragments,
                    hard_break,
                    break_at: next_start.map(|_| l.end),
                    ellipsis: None,
                },
                atoms,
                glyphs: Vec::new(),
                baseline: top + self.line_height,
            });
        }
        present_lines(presented, max_lines, fit, ellipsis)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl TextEngine for MonoEngine {
    fn layout(&mut self, text: &str, style: &TextStyle, _: &[StyleRun], _: Option<&TextLayout>, _: usize) -> TextLayout {
        Rc::new(MonoText { text: text.to_owned(), em: style.font_size * 0.5, line_height: style.line_height })
    }
    fn ellipsis(&mut self, style: &TextStyle) -> Option<(Vec<ShapedRun>, f32)> {
        Some((Vec::new(), style.font_size * 0.5))
    }
}

// ---- Parley ----

/// Bundled faces (SIL OFL): Inter for sans, IBM Plex Mono for mono.
static BUNDLED: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
];
/// A width as a hashable key (`None` is unbounded).
type WidthKey = Option<u32>;

fn width_key(w: Option<f32>) -> WidthKey {
    w.map(f32::to_bits)
}

/// What a paragraph is shaped in.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ShapeStyle {
    family: &'static str,
    size: f32,
    weight: u16,
    line_height: f32,
}

impl ShapeStyle {
    fn of(s: &TextStyle) -> ShapeStyle {
        ShapeStyle { family: s.family, size: s.font_size, weight: s.weight, line_height: s.line_height }
    }
}

/// A shaped paragraph, shared by every text showing it in its style.
struct Shaped {
    text: String,
    style: ShapeStyle,
    runs: Vec<StyleRun>,
    layout: RefCell<Layout<u16>>,
    /// The width the layout is broken at now.
    broken: Cell<Option<WidthKey>>,
    /// Recent results by width: Taffy asks for min-content, max-content and the final width.
    results: RefCell<Vec<(WidthKey, f32, u32, f32)>>,
}

impl Shaped {
    fn break_at(&self, max_width: Option<f32>) {
        let key = width_key(max_width);
        if self.broken.get() != Some(key) {
            self.layout.borrow_mut().break_all_lines(max_width);
            self.broken.set(Some(key));
        }
    }
    fn measure(&self, max_width: Option<f32>) -> (f32, u32, f32) {
        let key = width_key(max_width);
        if let Some(&(_, w, l, h)) = self.results.borrow().iter().find(|r| r.0 == key) {
            return (w, l, h);
        }
        self.break_at(max_width);
        let layout = self.layout.borrow();
        let r = (layout.width(), layout.len() as u32, layout.height());
        let mut results = self.results.borrow_mut();
        if results.len() == 4 {
            results.remove(0);
        }
        results.push((key, r.0, r.1, r.2));
        r
    }
    /// What keeping it costs, estimated (Rune's: 64 bytes per byte of text and 2 KB).
    fn bytes(&self) -> usize {
        self.text.len() * 64 + 2048
    }
}

/// A text laid out by Parley: its paragraphs.
struct ParleyText {
    paras: Vec<(usize, Rc<Shaped>)>,
    line_height: f32,
}

impl LaidText for ParleyText {
    fn measure(&self, max_width: Option<f32>) -> (f32, u32, f32) {
        let (mut width, mut lines, mut height) = (0.0f32, 0u32, 0.0f32);
        for (_, p) in &self.paras {
            let (w, l, h) = p.measure(max_width);
            width = width.max(w);
            lines += l.max(1);
            height += if l == 0 { self.line_height } else { h };
        }
        (width, lines, height)
    }
    fn present(
        &self,
        max_width: Option<f32>,
        max_lines: Option<u16>,
        fit: Option<f32>,
        ellipsis: Option<&(Vec<ShapedRun>, f32)>,
    ) -> TextPresentation {
        let mut lines = Vec::new();
        let mut top = 0.0;
        // One extra line establishes hidden content without extracting the entire hidden tail.
        let needed = max_lines.map_or(usize::MAX, |n| usize::from(n) + 1);
        'paras: for (pi, &(base, ref p)) in self.paras.iter().enumerate() {
            let next = self.paras.get(pi + 1).map(|&(start, _)| start);
            let break_range = || next.map(|start| base + p.text.len()..start);
            p.break_at(max_width);
            let layout = p.layout.borrow();
            let count = lines.len();
            for (li, line) in layout.lines().enumerate() {
                if lines.len() == needed {
                    break 'paras;
                }
                let m = line.metrics();
                let range = line.text_range();
                let end = base + range.end.min(p.text.len());
                let hard_break = if li + 1 == layout.len() { break_range() } else { None };
                let baselines: Vec<_> = line
                    .items()
                    .filter_map(|item| match item {
                        parley::PositionedLayoutItem::GlyphRun(run) => Some((run.style_index(), run.baseline())),
                        _ => None,
                    })
                    .collect();
                let mut presented = PresentationLine {
                    line: PresentedLine {
                        start: base + range.start.min(p.text.len()),
                        top: top + m.block_min_coord,
                        height: m.block_max_coord - m.block_min_coord,
                        fragments: Vec::new(),
                        hard_break,
                        break_at: (li + 1 < layout.len() || next.is_some()).then_some(end),
                        ellipsis: None,
                    },
                    atoms: Vec::new(),
                    glyphs: Vec::new(),
                    baseline: top + m.baseline,
                };
                let mut x = m.offset + m.inline_min_coord;
                for run in line.runs() {
                    let clusters: Vec<_> = run.visual_clusters().collect();
                    let mut current_run = None;
                    let mut start = 0;
                    for end in 1..=clusters.len() {
                        // RTL ligatures are visited continuation-first; LTR ones start-first.
                        let joins = clusters.get(end).is_some_and(|c| {
                            if c.is_rtl() { clusters[end - 1].is_ligature_continuation() } else { c.is_ligature_continuation() }
                        });
                        if !joins {
                            x = append_parley_atom(&mut presented, &clusters[start..end], base, x, top, &baselines, &mut current_run);
                            start = end;
                        }
                    }
                }
                lines.push(presented);
            }
            if lines.len() == count {
                if lines.len() == needed {
                    break;
                }
                lines.push(PresentationLine {
                    line: PresentedLine {
                        start: base,
                        top,
                        height: self.line_height,
                        fragments: Vec::new(),
                        hard_break: break_range(),
                        break_at: next.map(|_| base),
                        ellipsis: None,
                    },
                    atoms: Vec::new(),
                    glyphs: Vec::new(),
                    baseline: top + self.line_height,
                });
            }
            top += if layout.is_empty() { self.line_height } else { layout.height() };
        }
        present_lines(lines, max_lines, fit, ellipsis)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Extract glyphs and source intervals together, without inferring bytes from glyph x later.
/// Font metadata and glyph storage are allocated once per actual styled font run.
fn append_parley_atom(
    line: &mut PresentationLine,
    clusters: &[parley::Cluster<'_, u16>],
    base: usize,
    x: f32,
    top: f32,
    baselines: &[(u16, f32)],
    current_run: &mut Option<(u16, usize)>,
) -> f32 {
    let start = line.line.fragments.len();
    let mut edge = x;
    for c in clusters {
        let range = c.text_range();
        let width = c.advance();
        line.line.fragments.push(TextFragment { source: base + range.start..base + range.end, x: edge, width, rtl: c.is_rtl() });
        edge += width;
    }
    let glyph_end = clusters.iter().find(|c| c.glyphs().next().is_some()).map(|c| {
        let index = match *current_run {
            Some((style, index)) if style == c.style_index() => index,
            _ => {
                let r = c.run();
                let baseline =
                    top + baselines.iter().find(|(style, _)| *style == c.style_index()).expect("a glyph cluster has a positioned run").1;
                let index = line.glyphs.len();
                line.glyphs.push(ShapedRun {
                    font: r.font().clone(),
                    size: r.font_size(),
                    coords: r.normalized_coords().iter().map(|c| c.to_bits()).collect(),
                    embolden: r.synthesis().embolden(),
                    glyphs: Vec::new(),
                    brush: c.style().brush,
                    x,
                    width: 0.0,
                    baseline,
                    line_top: line.line.top,
                    line_height: line.line.height,
                });
                *current_run = Some((c.style_index(), index));
                index
            }
        };
        let run = &mut line.glyphs[index];
        let mut at = x;
        run.glyphs.extend(c.glyphs().map(|g| {
            let glyph = octara_primitives::paint::Glyph { id: g.id, x: at + g.x, y: run.baseline + g.y };
            at += g.advance;
            glyph
        }));
        run.width = edge - run.x;
        (index, run.glyphs.len())
    });
    line.atoms.push(PresentationAtom {
        fragments: start..line.line.fragments.len(),
        glyph_end,
        space: clusters.iter().all(parley::Cluster::is_space_or_nbsp),
    });
    edge
}

/// How much shaped text an engine keeps by default.
const KEEP_BYTES: usize = 32 << 20;

/// Parley over bundled and registered fonts. With `system-fonts`, `ParleyEngine::system` also
/// includes the platform's fonts.
pub struct ParleyEngine {
    fonts: FontContext,
    layouts: LayoutContext<u16>,
    /// Shaped paragraphs by content hash (a chain for the rare collision), with when they were
    /// last used.
    cache: FxHashMap<u64, Vec<(Rc<Shaped>, u64)>>,
    /// What the cache keeps, estimated.
    bytes: usize,
    budget: usize,
    tick: u64,
    /// "…" shaped per style, for truncated text.
    ellipses: Vec<(TextStyle, Vec<ShapedRun>, f32)>,
    /// Paragraphs shaped, for tests and benchmarks.
    pub shaped: u64,
}

impl Default for ParleyEngine {
    fn default() -> Self {
        Self::with_options(false)
    }
}

impl ParleyEngine {
    /// The bundled faces plus the platform's installed fonts, with its fallback for scripts the
    /// bundled faces lack (CJK, emoji, …); `system-ui` and `ui-monospace` are the platform's.
    /// For real windows; tests keep the default.
    #[cfg(feature = "system-fonts")]
    pub fn system() -> Self {
        Self::with_options(true)
    }

    fn with_options(system_fonts: bool) -> Self {
        use parley::fontique::GenericFamily;
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts });
        for data in BUNDLED {
            collection.register_fonts(Blob::new(Arc::new(data)), None);
        }
        let id = |c: &mut Collection, names: &[&str]| names.iter().find_map(|n| c.family_by_name(n).map(|f| f.id()));
        let inter = id(&mut collection, &["Inter"]);
        let plex = id(&mut collection, &["IBM Plex Mono"]);
        // `system-ui` and `ui-monospace`: the platform's where there is one, else the bundled.
        let ui = if system_fonts { collection.generic_families(GenericFamily::SystemUi).next().or(inter) } else { inter };
        let mono = if system_fonts {
            id(&mut collection, &["SF Mono", "Menlo", "Cascadia Mono", "Consolas", "DejaVu Sans Mono", "Liberation Mono"]).or(plex)
        } else {
            plex
        };
        if let Some(ui) = ui {
            collection.set_generic_families(GenericFamily::SystemUi, std::iter::once(ui));
            if !system_fonts {
                collection.set_generic_families(GenericFamily::SansSerif, std::iter::once(ui));
            }
        }
        if let Some(mono) = mono {
            collection.set_generic_families(GenericFamily::UiMonospace, std::iter::once(mono));
            if !system_fonts {
                collection.set_generic_families(GenericFamily::Monospace, std::iter::once(mono));
            }
        }
        ParleyEngine {
            fonts: FontContext { collection, source_cache: SourceCache::default() },
            layouts: LayoutContext::new(),
            cache: FxHashMap::default(),
            bytes: 0,
            budget: KEEP_BYTES,
            tick: 0,
            ellipses: Vec::new(),
            shaped: 0,
        }
    }

    /// Keeps at most `bytes` of shaped text (estimated) beyond what texts are showing.
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
        self.trim();
    }
}

/// A positioned glyph run used to shape a synthetic ellipsis.
fn shaped_run(run: &parley::GlyphRun<'_, u16>, top: f32, line: &parley::LineMetrics) -> ShapedRun {
    let r = run.run();
    let glyphs = run.positioned_glyphs().map(|g| octara_primitives::paint::Glyph { id: g.id, x: g.x, y: g.y + top }).collect();
    ShapedRun {
        font: r.font().clone(),
        size: r.font_size(),
        coords: r.normalized_coords().iter().map(|c| c.to_bits()).collect(),
        embolden: r.synthesis().embolden(),
        glyphs,
        brush: run.style().brush,
        x: run.offset(),
        width: run.advance(),
        baseline: top + run.baseline(),
        line_top: top + line.block_min_coord,
        line_height: line.block_max_coord - line.block_min_coord,
    }
}

impl ParleyEngine {
    fn shape(&mut self, text: &str, style: &ShapeStyle, runs: &[StyleRun]) -> Layout<u16> {
        self.shaped += 1;
        let mut b = self.layouts.ranged_builder(&mut self.fonts, text, 1.0, true);
        b.push_default(StyleProperty::Brush(0));
        b.push_default(StyleProperty::FontFamily(style.family.into()));
        b.push_default(StyleProperty::FontSize(style.size));
        b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight as f32)));
        b.push_default(StyleProperty::LineHeight(parley::LineHeight::Absolute(style.line_height)));
        let ratio = style.line_height / style.size.max(1.0);
        for r in runs {
            let range = r.start..r.end;
            b.push(StyleProperty::Brush(r.brush), range.clone());
            if let Some(f) = r.family {
                b.push(StyleProperty::FontFamily(f.into()), range.clone());
            }
            if let Some(size) = r.size {
                b.push(StyleProperty::FontSize(size), range.clone());
                // A bigger run gets lines as tall for it as the text's are for the text.
                b.push(StyleProperty::LineHeight(parley::LineHeight::FontSizeRelative(ratio)), range.clone());
            }
            if let Some(w) = r.weight {
                b.push(StyleProperty::FontWeight(FontWeight::new(w as f32)), range.clone());
            }
            if r.italic {
                b.push(StyleProperty::FontStyle(parley::FontStyle::Italic), range.clone());
            }
        }
        b.build(text)
    }

    fn key(text: &str, style: &ShapeStyle, runs: &[StyleRun]) -> u64 {
        let mut h = FxHasher::default();
        text.hash(&mut h);
        style.family.hash(&mut h);
        style.size.to_bits().hash(&mut h);
        style.weight.hash(&mut h);
        style.line_height.to_bits().hash(&mut h);
        for r in runs {
            (r.start, r.end, r.brush, r.family, r.size.map(f32::to_bits), r.weight, r.italic).hash(&mut h);
        }
        h.finish()
    }

    /// The shaped paragraph for this content: kept, or shaped now and kept.
    fn find_or_shape(&mut self, text: &str, style: &ShapeStyle, runs: &[StyleRun]) -> Rc<Shaped> {
        self.tick += 1;
        let key = Self::key(text, style, runs);
        if let Some(chain) = self.cache.get_mut(&key)
            && let Some(entry) = chain.iter_mut().find(|(s, _)| s.text == text && s.style == *style && s.runs == runs)
        {
            entry.1 = self.tick;
            return entry.0.clone();
        }
        let layout = self.shape(text, style, runs);
        let shaped = Rc::new(Shaped {
            text: text.to_owned(),
            style: *style,
            runs: runs.to_vec(),
            layout: RefCell::new(layout),
            broken: Cell::new(None),
            results: RefCell::new(Vec::new()),
        });
        self.bytes += shaped.bytes();
        self.cache.entry(key).or_default().push((shaped.clone(), self.tick));
        shaped
    }

    /// Drops `shaped` from the cache (a paragraph its text replaced).
    fn evict(&mut self, shaped: &Rc<Shaped>) {
        let key = Self::key(&shaped.text, &shaped.style, &shaped.runs);
        if let Some(chain) = self.cache.get_mut(&key) {
            let before = chain.len();
            chain.retain(|(s, _)| !Rc::ptr_eq(s, shaped));
            if chain.len() < before {
                self.bytes -= shaped.bytes();
            }
            if chain.is_empty() {
                self.cache.remove(&key);
            }
        }
    }

    /// Over the budget: the least recently used paragraphs no text shows go first.
    fn trim(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let mut idle: Vec<(u64, u64, usize)> = self
            .cache
            .iter()
            .flat_map(|(k, chain)| {
                chain.iter().enumerate().filter(|(_, (s, _))| Rc::strong_count(s) == 1).map(move |(i, (_, t))| (*t, *k, i))
            })
            .collect();
        idle.sort_unstable();
        let mut gone: Vec<Rc<Shaped>> = Vec::new();
        let mut freed = 0usize;
        for (_, k, i) in idle {
            // Down to three quarters, so trimming is not paid on every new paragraph.
            if self.bytes - freed <= self.budget / 4 * 3 {
                break;
            }
            if let Some((s, _)) = self.cache.get(&k).and_then(|c| c.get(i)) {
                freed += s.bytes();
                gone.push(s.clone());
            }
        }
        for s in gone {
            self.evict(&s);
        }
    }
}

impl TextEngine for ParleyEngine {
    fn layout(&mut self, text: &str, style: &TextStyle, runs: &[StyleRun], previous: Option<&TextLayout>, clean: usize) -> TextLayout {
        let shape = ShapeStyle::of(style);
        let prev = previous.and_then(|p| p.as_any().downcast_ref::<ParleyText>());
        let mut paras = Vec::new();
        let mut local = Vec::new();
        for (i, (start, p)) in paragraphs(text).enumerate() {
            let end = start + p.len();
            local.clear();
            for r in runs.iter().filter(|r| r.end > start && r.start < end) {
                local.push(StyleRun { start: r.start.max(start) - start, end: r.end.min(end) - start, ..*r });
            }
            // Unchanged since the last layout: the same paragraph, without comparing it.
            let kept = prev.and_then(|pt| pt.paras.get(i)).filter(|(old_start, s)| {
                *old_start == start && end < clean && s.text.len() == p.len() && s.style == shape && s.runs == local
            });
            let shaped = kept.map_or_else(|| self.find_or_shape(p, &shape, &local), |(_, shape)| shape.clone());
            paras.push((start, shaped));
        }
        // Paragraphs this text replaced that nothing else shows leave the cache now (a stream's
        // tail, typing): they would only wait for the budget.
        if let Some(pt) = prev {
            let replaced: Vec<_> = pt
                .paras
                .iter()
                .filter(|(_, o)| Rc::strong_count(o) == 2 && !paras.iter().any(|(_, n)| Rc::ptr_eq(n, o)))
                .map(|(_, old)| old.clone())
                .collect();
            for o in &replaced {
                self.evict(o);
            }
        }
        self.trim();
        Rc::new(ParleyText { paras, line_height: style.line_height })
    }
    fn ellipsis(&mut self, style: &TextStyle) -> Option<(Vec<ShapedRun>, f32)> {
        if let Some((_, runs, w)) = self.ellipses.iter().find(|(s, ..)| s == style) {
            return Some((runs.clone(), *w));
        }
        let mut layout = self.shape("…", &ShapeStyle::of(style), &[]);
        self.shaped -= 1;
        layout.break_all_lines(None);
        let mut runs = Vec::new();
        if let Some(line) = layout.lines().next() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
                runs.push(shaped_run(&run, 0.0, line.metrics()));
            }
        }
        let w = layout.width();
        self.ellipses.push((*style, runs.clone(), w));
        Some((runs, w))
    }
    fn shaped(&self) -> u64 {
        self.shaped
    }
    fn kept_bytes(&self) -> usize {
        self.bytes
    }
    fn register_fallback_font(&mut self, data: &'static [u8], scripts: &[[u8; 4]]) {
        let mut families: Vec<_> =
            self.fonts.collection.register_fonts(Blob::new(Arc::new(data)), None).into_iter().map(|(id, _)| id).collect();
        families.dedup();
        for s in scripts {
            self.fonts.collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*s), None), families.iter().copied());
        }
        self.forget_shapes();
    }
    fn register_font(&mut self, data: &'static [u8]) {
        self.fonts.collection.register_fonts(Blob::new(Arc::new(data)), None);
        self.forget_shapes();
    }
}

impl ParleyEngine {
    /// Shapes may now resolve to other faces: nothing kept is reused.
    fn forget_shapes(&mut self) {
        self.cache.clear();
        self.bytes = 0;
        self.ellipses.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(e: &mut dyn TextEngine, text: &str, s: &TextStyle) -> TextLayout {
        e.layout(text, s, &[], None, 0)
    }

    #[test]
    fn parley_wraps_and_measures() {
        let mut e = ParleyEngine::default();
        let s = TextStyle::default();
        let l = lay(&mut e, "hello world", &s);
        let one = metrics(&l, &s, None);
        assert!(one.width > 50.0 && one.width < 120.0, "{one:?}");
        assert_eq!((one.lines, one.height), (1, 24.0));
        let narrow = metrics(&l, &s, Some(one.width / 2.0 + 10.0));
        assert_eq!(narrow.lines, 2);
        assert!(narrow.width < one.width);
        assert_eq!(metrics(&l, &s, Some(0.0)).lines, 2, "min-content breaks at every opportunity");
        let mono = metrics(&lay(&mut e, "iiii", &TextStyle { family: "ui-monospace", ..s }), &s, None);
        let sans = metrics(&lay(&mut e, "iiii", &s), &s, None);
        assert!(mono.width > sans.width * 1.5, "ui-monospace is the bundled monospace: {mono:?} vs {sans:?}");
        let capped = TextStyle { max_lines: Some(2), ..s };
        let m = metrics(&lay(&mut e, "a\nb\nc", &capped), &capped, None);
        assert_eq!((m.lines, m.truncated), (2, true));
    }

    #[test]
    fn shapes_are_kept_by_content() {
        let mut e = ParleyEngine::default();
        let s = TextStyle::default();
        let first = lay(&mut e, "first\nsecond", &s);
        let shaped = e.shaped;
        // Appending: only the last paragraph is shaped, the first reused from the last layout.
        let grown = e.layout("first\nsecond more", &s, &[], Some(&first), "first\nsecond".len());
        assert_eq!(e.shaped - shaped, 1);
        // The same content elsewhere (a row scrolling back, a repeated label): nothing shaped.
        let shaped = e.shaped;
        let again = lay(&mut e, "first\nsecond more", &s);
        assert_eq!(e.shaped, shaped, "found by content");
        assert_eq!(metrics(&again, &s, None), metrics(&grown, &s, None));
        // A style that changes shapes is other content; a width is not.
        lay(&mut e, "first", &TextStyle { weight: 600, ..s });
        assert_eq!(e.shaped, shaped + 1);
        let _ = metrics(&again, &s, Some(10.0));
        assert_eq!(e.shaped, shaped + 1);
    }

    #[test]
    fn parley_reuses_shapes_after_line_delimiter_edits() {
        let mut engine = ParleyEngine::default();
        let style = TextStyle::default();
        let original = lay(&mut engine, "first\nsecond\r\nlast", &style);
        let shaped = engine.shaped();
        let edited = engine.layout("first\r\nsecond\nlast", &style, &[], Some(&original), "first".len());
        assert_eq!(engine.shaped(), shaped);
        let original = original.present(None, None, None, None);
        let edited = edited.present(None, None, None, None);
        assert_eq!(original.lines[0].hard_break, Some(5..6));
        assert_eq!(edited.lines[0].hard_break, Some(5..7));
        assert_eq!(original.lines[1].fragments[0].source, 6..7);
        assert_eq!(edited.lines[1].fragments[0].source, 7..8);
        assert_eq!(original.lines[1].hard_break, Some(12..14));
        assert_eq!(edited.lines[1].hard_break, Some(13..14));
        assert_eq!(original.lines[2], edited.lines[2]);
    }

    #[test]
    fn a_stream_keeps_the_kept_bytes_flat() {
        let mut e = ParleyEngine::default();
        let s = TextStyle::default();
        let mut text = String::from("Streaming");
        let mut layout = lay(&mut e, &text, &s);
        let start = e.kept_bytes();
        for i in 0..200 {
            let clean = text.len();
            text.push_str(&format!(" w{i}"));
            layout = e.layout(&text, &s, &[], Some(&layout), clean);
        }
        assert!(e.kept_bytes() < start + 64 * text.len() + 4096, "the superseded tails left the cache: {}", e.kept_bytes());
        let _ = layout;
    }

    #[test]
    fn the_budget_drops_what_no_text_shows() {
        let mut e = ParleyEngine::default();
        let s = TextStyle::default();
        let shown = lay(&mut e, "still shown", &s);
        for i in 0..100 {
            lay(&mut e, &format!("row {i}"), &s);
        }
        e.set_budget(4096 * 4);
        assert!(e.kept_bytes() <= 4096 * 4, "{}", e.kept_bytes());
        let shaped = e.shaped;
        lay(&mut e, "still shown", &s);
        assert_eq!(e.shaped, shaped, "what a text shows stays");
        let _ = shown;
    }

    #[test]
    fn full_presentations_wrap_with_caret_edges() {
        for engine in [&mut MonoEngine as &mut dyn TextEngine, &mut ParleyEngine::default()] {
            let style = TextStyle::default();
            let layout = lay(engine, "hello world\nsecond", &style);
            let narrow = Some(layout.measure(None).0 * 0.7);
            let full = layout.present(narrow, None, None, None);
            assert_eq!(full.lines.len(), 3, "{:?}", full.lines);
            for (row, (line, source)) in full.lines.iter().zip([0..6, 6..11, 12..18]).enumerate() {
                assert_eq!((line.start, line.top, line.height), (source.start, row as f32 * style.line_height, style.line_height));
                let first = line.fragments.first().unwrap();
                let last = line.fragments.last().unwrap();
                assert_eq!(first.source.start..last.source.end, source);
                assert!(line.fragments.array_windows::<2>().all(|[a, b]| a.source.end == b.source.start && a.edges().1 <= b.edges().0));
                let y = line.top + line.height / 2.0;
                assert_eq!(first.edges().0, 0.0);
                assert_eq!(full.offset_at(0.0, y), source.start);
                assert_eq!(full.offset_at(last.edges().1, y), source.end);
            }
            assert_eq!(full.lines[0].break_at, Some(6));
            assert_eq!(full.lines[0].hard_break, None);
            assert_eq!(full.lines[1].break_at, Some(11));
            assert_eq!(full.lines[1].hard_break, Some(11..12));
        }
    }

    #[test]
    fn mono_is_half_an_em_per_char() {
        let mut e = MonoEngine;
        let s = TextStyle::default();
        assert_eq!(metrics(&lay(&mut e, "abcd", &s), &s, None).width, 32.0);
        let m = metrics(&lay(&mut e, "abcd efgh", &s), &s, Some(40.0));
        assert_eq!((m.width, m.lines), (32.0, 2));
    }

    #[test]
    fn presentations_share_line_limits_breaks_and_unicode_boundaries() {
        for mut engine in [Box::new(MonoEngine) as Box<dyn TextEngine>, Box::new(ParleyEngine::default())] {
            let style = TextStyle::default();
            let text = "中文😀\r\n\nhidden\r\n";
            let layout = lay(&mut *engine, text, &style);
            let lf = lay(&mut *engine, "中文😀\n\nhidden\n", &style);
            assert_eq!(layout.measure(None), lf.measure(None));
            let full = layout.present(None, None, None, None);
            assert_eq!(full.lines.len(), 4);
            assert!(full.lines[1].fragments.is_empty());
            assert!(full.lines[3].fragments.is_empty());
            assert!(full.lines[3].hard_break.is_none());
            let first_break = "中文😀".len();
            let hidden_start = first_break + 3;
            let breaks = [first_break..first_break + 2, first_break + 2..hidden_start, text.len() - 2..text.len()];
            for (row, range) in breaks.into_iter().enumerate() {
                let line = &full.lines[row];
                let x = line.fragments.last().map_or(0.0, |f| f.edges().1);
                assert_eq!(line.hard_break.as_ref(), Some(&range));
                assert_eq!(line.edges(&range), Some((x, x)));
                assert_eq!(full.offset_at(x + 1.0, line.top + 1.0), range.start);
                assert_eq!(full.lines[row + 1].start, range.end);
                assert!(full.range_rects(range).is_empty());
            }
            let dots = engine.ellipsis(&style);
            let shown = layout.present(None, Some(1), Some(200.0), dots.as_ref());
            assert_eq!(shown.lines.len(), 1);
            assert_eq!(shown.lines[0].hard_break, None, "a hidden next line has no positional newline");
            assert!(shown.range_rects(hidden_start..text.len()).is_empty());
            assert!(shown.lines[0].ellipsis.is_some());
            for f in &shown.lines[0].fragments {
                assert!(text.get(f.source.clone()).is_some(), "source bytes stay on UTF-8 boundaries");
                let (start, end) = f.edges();
                assert_eq!(shown.offset_at(start, 1.0), f.source.start);
                assert_eq!(shown.offset_at(end, 1.0), f.source.end);
            }
            let empty = layout.present(None, Some(0), Some(200.0), dots.as_ref());
            assert!(empty.lines.is_empty() && empty.glyphs.is_empty());
            let tiny = layout.present(None, Some(1), Some(0.01), dots.as_ref());
            assert!(tiny.glyphs.is_empty());
            assert!(tiny.lines[0].fragments.is_empty() && tiny.lines[0].ellipsis.is_none());
        }
    }

    #[test]
    fn truncation_never_splits_emoji_and_ellipsis_owns_no_source_or_brush() {
        let mut engine = ParleyEngine::default();
        let style = TextStyle::default();
        let text = "中👩‍👩‍👧‍👦文 suffix";
        let layout = engine.layout(
            text,
            &style,
            &[StyleRun { start: 0, end: text.len(), brush: 1, family: None, size: None, weight: None, italic: false }],
            None,
            0,
        );
        let full = layout.present(None, None, None, None);
        let emoji = &full.lines[0].fragments[1];
        assert_eq!(&text[emoji.source.clone()], "👩‍👩‍👧‍👦");
        let dots = engine.ellipsis(&style).unwrap();
        let width = emoji.x + emoji.width * 0.5 + dots.1;
        let shown = layout.present(None, Some(1), Some(width), Some(&dots));
        assert!(shown.range_rects(emoji.source.clone()).is_empty(), "a partial emoji is not visible");
        let ellipsis = shown.lines[0].ellipsis.as_ref().unwrap();
        assert_eq!(ellipsis.boundary, "中".len());
        assert_eq!(shown.offset_at(ellipsis.rect.right(), 1.0), "中".len());
        assert_eq!(shown.glyphs.last().unwrap().brush, 0);
        assert!(shown.glyphs.iter().filter(|r| r.brush != 0).all(|r| r.x + r.width <= ellipsis.rect.x + 0.01));
    }

    #[test]
    fn fitting_keeps_real_multi_grapheme_ligatures_whole() {
        let mut engine = ParleyEngine::default();
        let style = TextStyle::default();
        // Inter's discretionary arrow ligature spans several graphemes. Enable it in the
        // shaping fixture so this regression cannot pass by merely counting ordinary letters.
        let text = "A --> Z";
        let mut builder = engine.layouts.ranged_builder(&mut engine.fonts, text, 1.0, true);
        builder.push_default(StyleProperty::FontFamily("Inter".into()));
        builder.push_default(StyleProperty::FontSize(style.font_size));
        builder.push_default(StyleProperty::LineHeight(parley::LineHeight::Absolute(style.line_height)));
        builder.push_default(StyleProperty::FontFeatures("dlig".into()));
        let mut shaped = builder.build(text);
        shaped.break_all_lines(None);
        assert!(shaped.lines().flat_map(|l| l.runs()).flat_map(|r| r.clusters()).any(|c| c.is_ligature_start()));
        let shaped = Rc::new(Shaped {
            text: text.into(),
            style: ShapeStyle::of(&style),
            runs: Vec::new(),
            layout: RefCell::new(shaped),
            broken: Cell::new(Some(None)),
            results: RefCell::default(),
        });
        let layout = ParleyText { paras: vec![(0, shaped)], line_height: style.line_height };
        let full = layout.present(None, None, None, None);
        let arrow = full.range_rects(2..5)[0];
        let dots = engine.ellipsis(&style).unwrap();
        let cut = layout.present(None, Some(1), Some(arrow.x + arrow.w * 0.5 + dots.1), Some(&dots));
        assert!(cut.range_rects(2..5).is_empty(), "no continuation of an undrawn ligature gets source geometry");
        let boundary = cut.lines[0].ellipsis.as_ref().unwrap().boundary;
        assert!(boundary <= 2);
    }

    #[test]
    fn presented_glyph_positions_match_parley_for_mixed_styles_and_bidi() {
        let mut engine = ParleyEngine::default();
        let text = "Hello 中文 office אבג مرحبا fin\nnext";
        let style = TextStyle::default();
        let runs = [
            StyleRun { start: 6, end: 12, brush: 1, family: None, size: Some(28.0), weight: Some(600), italic: false },
            StyleRun { start: 13, end: 19, brush: 2, family: None, size: Some(12.0), weight: Some(500), italic: true },
        ];
        let layout = engine.layout(text, &style, &runs, None, 0);
        for width in [None, Some(120.0), Some(250.0)] {
            let shown = layout.present(width, None, None, None);
            let mut expected = Vec::new();
            let mut top = 0.0;
            for (_, p) in &layout.as_any().downcast_ref::<ParleyText>().unwrap().paras {
                p.break_at(width);
                let shaped = p.layout.borrow();
                for line in shaped.lines() {
                    for item in line.items() {
                        if let parley::PositionedLayoutItem::GlyphRun(run) = item {
                            expected.extend(run.positioned_glyphs().map(|g| (g.id, g.x, g.y + top)));
                        }
                    }
                }
                top += shaped.height();
            }
            let actual: Vec<_> = shown.glyphs.iter().flat_map(|r| r.glyphs.iter().map(|g| (g.id, g.x, g.y))).collect();
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(expected) {
                assert_eq!(actual.0, expected.0);
                assert!((actual.1 - expected.1).abs() < 0.01 && (actual.2 - expected.2).abs() < 0.01, "{actual:?} != {expected:?}");
            }
        }
    }

    #[test]
    fn rtl_ranges_and_bidi_affinities_remain_visible_after_fitting() {
        let mut engine = ParleyEngine::default();
        let style = TextStyle::default();
        let text = "abc אבג def";
        let layout = lay(&mut engine, text, &style);
        let full = layout.present(None, None, None, None);
        let rtl = "abc ".len().."abc אבג".len();
        let rects = full.range_rects(rtl.clone());
        assert_eq!(rects.len(), 1);
        assert!(rects[0].w > 0.0);
        let fragments = &full.lines[0].fragments;
        let boundary = rtl.start;
        let edges: Vec<_> = fragments
            .iter()
            .flat_map(|f| {
                let (s, e) = f.edges();
                [(f.source.start, s), (f.source.end, e)]
            })
            .filter(|(b, _)| *b == boundary)
            .collect();
        assert!(edges.len() >= 2 && edges[0].1 != edges[1].1, "one source byte can have two bidi affinities: {edges:?}");
        let dots = engine.ellipsis(&style).unwrap();
        let shown = layout.present(None, Some(1), Some(rects[0].x + rects[0].w * 0.7 + dots.1), Some(&dots));
        for f in &shown.lines[0].fragments {
            assert_eq!(shown.range_rects(f.source.clone()).len(), 1);
            assert!(text.get(f.source.clone()).is_some());
        }
        assert!(shown.lines[0].fragments.iter().any(|f| f.rtl));
    }
}
