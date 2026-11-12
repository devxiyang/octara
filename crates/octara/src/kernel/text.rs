//! Text measurement behind layout: a replaceable [`TextEngine`].
//!
//! [`ParleyEngine`] (the default) shapes with Parley over the bundled Inter faces only, so a
//! headless run measures the same on every machine. [`MonoEngine`] is a deterministic monospace
//! model for isolating text cost in benchmarks.
//!
//! Engines cache per node and per paragraph (`\n`-separated): appending to a streamed text
//! reshapes only its last paragraph, and a new width only re-breaks lines.

use super::NodeId;
use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, Script, SourceCache};
use parley::{FontContext, FontWeight, Layout, LayoutContext, StyleProperty};
use rustc_hash::{FxHashMap, FxHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

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
    /// A registered family name.
    pub family: &'static str,
    /// Line height in pixels.
    pub line_height: f32,
    pub weight: u16,
    pub wrap: Wrap,
    pub max_lines: Option<u16>,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle { font_size: 16.0, family: "Inter", line_height: 24.0, weight: 400, wrap: Wrap::Words, max_lines: None }
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

/// What a text engine caches a text by: the node it belongs to and which of its texts (slot 0
/// is the node's own text; a canvas numbers its texts from 1). Forgetting a node forgets all
/// of them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TextKey {
    /// The kernel the node is in: one text system serves every window.
    pub kernel: u32,
    pub node: NodeId,
    pub slot: u32,
}

impl From<NodeId> for TextKey {
    fn from(node: NodeId) -> TextKey {
        TextKey { kernel: 0, node, slot: 0 }
    }
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

/// Measures paragraphs. Implementations cache per `(text, paragraph)`.
pub trait TextEngine {
    /// Width, line count and height of one paragraph broken at `max_width` (`None`:
    /// unbounded), its `runs` styling ranges of it (byte offsets in the paragraph).
    /// `unchanged`: the kernel knows this paragraph's text is the same as when the engine last
    /// saw paragraph `index` of `key` (so a cache may skip comparing it).
    #[allow(clippy::too_many_arguments)]
    fn paragraph(
        &mut self,
        key: TextKey,
        index: usize,
        text: &str,
        unchanged: bool,
        style: &TextStyle,
        runs: &[StyleRun],
        max_width: Option<f32>,
    ) -> (f32, u32, f32);
    /// Drops cached paragraphs from `count` on (the text now has fewer).
    fn truncate(&mut self, key: TextKey, count: usize);
    /// Forgets a destroyed node's texts.
    fn forget(&mut self, kernel: u32, node: NodeId);
    /// Adds font data (TTF/OTF, possibly a collection); its families become available by name.
    fn register_font(&mut self, _data: &'static [u8]) {}
    /// Adds font data whose families text falls back to for `scripts` (ISO 15924 tags such as
    /// `*b"Hani"`) when the styled family lacks a character.
    fn register_fallback_font(&mut self, _data: &'static [u8], _scripts: &[[u8; 4]]) {}
    /// The positioned glyphs of `key`'s text (measured just before) broken at `max_width`,
    /// relative to the text's top-left corner, at most `style.max_lines` lines. With `fit`, a
    /// last line that is wider than `fit`, or followed by lines `max_lines` hides, ends in an
    /// ellipsis within `fit`. Engines that only measure return nothing.
    fn glyph_runs(&mut self, _key: TextKey, _style: &TextStyle, _max_width: Option<f32>, _fit: Option<f32>) -> Vec<ShapedRun> {
        Vec::new()
    }
    /// The visual lines of `text` (measured just before for `key`) broken at `max_width`
    /// (`None`: one line per paragraph): what editing places the caret, moves it by lines and
    /// draws selections with.
    fn visual_lines(&mut self, key: TextKey, text: &str, style: &TextStyle, max_width: Option<f32>) -> Vec<VisualLine>;
    /// Paragraphs shaped so far (for tests and benchmarks; 0 where not counted).
    fn shaped(&self) -> u64 {
        0
    }
}

/// One line of a laid-out text as shown: its byte range (a line break's `\n` is in neither
/// line), its top (from the text's top) and height, and where a caret can stand on it, `(byte,
/// x)` in byte order from `(start, ..)` to `(end, ..)`. A line that wraps ends where the next one
/// starts.
#[derive(Clone, PartialEq, Debug)]
pub struct VisualLine {
    pub start: usize,
    pub end: usize,
    pub top: f32,
    pub height: f32,
    pub stops: Vec<(usize, f32)>,
}

impl VisualLine {
    /// Where on the line a caret at byte `at` stands (the nearest stop at or before it).
    pub fn x_of(&self, at: usize) -> f32 {
        self.stops.iter().rev().find(|(b, _)| *b <= at).or(self.stops.first()).map_or(0.0, |s| s.1)
    }
}

/// The rects `range` covers in `lines`, line by line, relative to the text's top-left.
pub fn range_rects(lines: &[VisualLine], range: std::ops::Range<usize>) -> Vec<crate::paint::Rect> {
    let mut out = Vec::new();
    for l in lines {
        let (s, e) = (range.start.max(l.start), range.end.min(l.end));
        if s >= e && !(range.is_empty() && s == e) {
            continue;
        }
        let (x0, x1) = (l.x_of(s), l.x_of(e));
        if x1 > x0 {
            out.push(crate::paint::Rect { x: x0, y: l.top, w: x1 - x0, h: l.height });
        }
    }
    out
}

/// The byte nearest `(x, y)` (relative to the text's top-left) in `lines`.
pub fn offset_at(lines: &[VisualLine], x: f32, y: f32) -> usize {
    let Some(line) = lines.iter().find(|l| y < l.top + l.height).or(lines.last()) else { return 0 };
    line.stops.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(line.start, |s| s.0)
}

/// Monospace lines: `em` per character, broken at spaces to fit `max_width` (as
/// [`MonoEngine::paragraph`] measures), a space that breaks a line staying at its end.
fn even_lines(text: &str, em: f32, line_height: f32, max_width: Option<f32>) -> Vec<VisualLine> {
    let mut out = Vec::new();
    let mut base = 0;
    for para in text.split('\n') {
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
            let mut stops: Vec<(usize, f32)> =
                para[s..e].char_indices().enumerate().map(|(n, (b, _))| (base + s + b, n as f32 * em)).collect();
            stops.push((base + e, para[s..e].chars().count() as f32 * em));
            out.push(VisualLine { start: base + s, end: base + e, top: out.len() as f32 * line_height, height: line_height, stops });
        }
        base += para.len() + 1;
    }
    out
}

/// Glyphs of one font, positioned relative to their text's top-left corner.
#[derive(Clone, Debug)]
pub struct ShapedRun {
    pub font: parley::FontData,
    pub size: f32,
    pub coords: Vec<i16>,
    pub embolden: bool,
    pub glyphs: Vec<crate::paint::Glyph>,
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

/// Lays out `text` (all paragraphs) at `max_width` with `style.max_lines` applied, `runs`
/// styling ranges of it (byte offsets in the text, sorted). The first `clean` bytes are known
/// unchanged since the engine last measured this node: paragraphs that end within them are not
/// compared again (appending to a long text touches only its tail).
#[allow(clippy::too_many_arguments)]
pub fn measure(
    engine: &mut dyn TextEngine,
    key: TextKey,
    text: &str,
    clean: usize,
    style: &TextStyle,
    runs: &[StyleRun],
    max_width: Option<f32>,
) -> TextMetrics {
    let (mut width, mut lines, mut height, mut count, mut start) = (0.0f32, 0u32, 0.0f32, 0, 0usize);
    let mut local = Vec::new();
    for (i, p) in text.split('\n').enumerate() {
        let end = start + p.len();
        local.clear();
        for r in runs.iter().filter(|r| r.end > start && r.start < end) {
            local.push(StyleRun { start: r.start.max(start) - start, end: r.end.min(end) - start, ..*r });
        }
        let (w, l, h) = engine.paragraph(key, i, p, end < clean, style, &local, max_width);
        start = end + 1;
        width = width.max(w);
        lines += l.max(1);
        height += if l == 0 { style.line_height } else { h };
        count = i + 1;
    }
    engine.truncate(key, count);
    let shown = style.max_lines.map_or(lines, |m| lines.min(m as u32));
    let height = if shown < lines { shown as f32 * style.line_height } else { height };
    TextMetrics { width, height, lines: shown, truncated: shown < lines }
}

// ---- monospace ----

/// Every character is half an em wide; breaks at spaces. Deterministic and nearly free.
#[derive(Default)]
pub struct MonoEngine;

impl TextEngine for MonoEngine {
    fn paragraph(
        &mut self,
        _: TextKey,
        _: usize,
        text: &str,
        _: bool,
        style: &TextStyle,
        _: &[StyleRun],
        max_width: Option<f32>,
    ) -> (f32, u32, f32) {
        let em = style.font_size * 0.5;
        let Some(max) = max_width else { return (text.chars().count() as f32 * em, 1, style.line_height) };
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
        (widest.max(line), lines, lines as f32 * style.line_height)
    }
    fn truncate(&mut self, _: TextKey, _: usize) {}
    fn forget(&mut self, _: u32, _: NodeId) {}
    fn visual_lines(&mut self, _: TextKey, text: &str, style: &TextStyle, max_width: Option<f32>) -> Vec<VisualLine> {
        even_lines(text, style.font_size * 0.5, style.line_height, max_width)
    }
}

// ---- Parley ----

/// Bundled faces (SIL OFL): Inter for sans, IBM Plex Mono for mono.
static BUNDLED: [&[u8]; 4] = [
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
];

/// A width as a hashable key (`None` is unbounded).
type WidthKey = Option<u32>;

fn width_key(w: Option<f32>) -> WidthKey {
    w.map(f32::to_bits)
}

struct Paragraph {
    hash: u64,
    style: TextStyle,
    runs: Vec<StyleRun>,
    layout: Layout<u16>,
    /// Recent results by width: Taffy asks for min-content, max-content and the final width.
    results: Vec<(WidthKey, f32, u32, f32)>,
}

/// A text node's paragraphs, by slot (a field's value and its placeholder are two).
type Slots = Vec<(u32, Vec<Paragraph>)>;

/// Parley over bundled and registered fonts only (no system fonts, so results do not depend on
/// the machine).
pub struct ParleyEngine {
    fonts: FontContext,
    layouts: LayoutContext<u16>,
    /// Paragraphs by node, then by slot.
    cache: FxHashMap<(u32, NodeId), Slots>,
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
    /// bundled faces lack (CJK, emoji, …). For real windows; tests keep the default.
    #[cfg(feature = "system-fonts")]
    pub fn system() -> Self {
        Self::with_options(true)
    }

    fn with_options(system_fonts: bool) -> Self {
        let mut collection = Collection::new(CollectionOptions { shared: false, system_fonts });
        for data in BUNDLED {
            collection.register_fonts(Blob::new(Arc::new(data)), None);
        }
        ParleyEngine {
            fonts: FontContext { collection, source_cache: SourceCache::default() },
            layouts: LayoutContext::new(),
            cache: FxHashMap::default(),
            ellipses: Vec::new(),
            shaped: 0,
        }
    }
}

/// A positioned run as the renderers want it, `top` down, with each glyph's advance (negative
/// for a space, so an ellipsis can drop trailing ones).
fn shaped_run(run: &parley::GlyphRun<'_, u16>, top: f32, line: &parley::LineMetrics) -> (ShapedRun, Vec<f32>) {
    let r = run.run();
    let spaces: Vec<bool> = r.visual_clusters().flat_map(|c| std::iter::repeat_n(c.is_space_or_nbsp(), c.glyphs().count())).collect();
    let (glyphs, advances) = run
        .positioned_glyphs()
        .enumerate()
        .map(|(i, g)| {
            let space = spaces.get(i).copied().unwrap_or(false);
            (crate::paint::Glyph { id: g.id, x: g.x, y: g.y + top }, if space { -g.advance.max(f32::MIN_POSITIVE) } else { g.advance })
        })
        .unzip();
    let shaped = ShapedRun {
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
    };
    (shaped, advances)
}

impl ParleyEngine {
    /// "…" in `style`, as runs on the first line, with their advances, and its width.
    fn ellipsis(&mut self, style: &TextStyle) -> (Vec<(ShapedRun, Vec<f32>)>, f32) {
        if let Some((_, runs, w)) = self.ellipses.iter().find(|(s, ..)| s == style) {
            return (runs.iter().map(|r| (r.clone(), Vec::new())).collect(), *w);
        }
        let mut layout = self.shape("…", style, &[]);
        self.shaped -= 1;
        layout.break_all_lines(None);
        let mut runs = Vec::new();
        if let Some(line) = layout.lines().next() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
                runs.push(shaped_run(&run, 0.0, line.metrics()).0);
            }
        }
        let w = layout.width();
        self.ellipses.push((*style, runs.clone(), w));
        (runs.into_iter().map(|r| (r, Vec::new())).collect(), w)
    }

    fn shape(&mut self, text: &str, style: &TextStyle, runs: &[StyleRun]) -> Layout<u16> {
        self.shaped += 1;
        let mut b = self.layouts.ranged_builder(&mut self.fonts, text, 1.0, true);
        b.push_default(StyleProperty::Brush(0));
        b.push_default(StyleProperty::FontFamily(style.family.into()));
        b.push_default(StyleProperty::FontSize(style.font_size));
        b.push_default(StyleProperty::FontWeight(FontWeight::new(style.weight as f32)));
        b.push_default(StyleProperty::LineHeight(parley::LineHeight::Absolute(style.line_height)));
        let ratio = style.line_height / style.font_size.max(1.0);
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
}

impl ParleyEngine {
    /// The cached paragraphs of `key`, if any.
    fn paras(&mut self, key: TextKey) -> Option<&mut Vec<Paragraph>> {
        self.cache.get_mut(&(key.kernel, key.node))?.iter_mut().find(|(s, _)| *s == key.slot).map(|(_, p)| p)
    }
    /// The cached paragraphs of `key`, made on first use.
    fn paras_entry(&mut self, key: TextKey) -> &mut Vec<Paragraph> {
        let slots = self.cache.entry((key.kernel, key.node)).or_default();
        let at = match slots.iter().position(|(s, _)| *s == key.slot) {
            Some(i) => i,
            None => {
                slots.push((key.slot, Vec::new()));
                slots.len() - 1
            }
        };
        &mut slots[at].1
    }
}

impl TextEngine for ParleyEngine {
    fn paragraph(
        &mut self,
        key: TextKey,
        index: usize,
        text: &str,
        unchanged: bool,
        style: &TextStyle,
        runs: &[StyleRun],
        max_width: Option<f32>,
    ) -> (f32, u32, f32) {
        let paras = self.paras_entry(key);
        let known = unchanged && paras.get(index).is_some_and(|p| p.style == *style && p.runs == runs);
        let hash = if known {
            paras[index].hash
        } else {
            let mut h = FxHasher::default();
            text.hash(&mut h);
            h.finish()
        };
        let fresh = !known && paras.get(index).is_none_or(|p| p.hash != hash || p.style != *style || p.runs != runs);
        if fresh {
            let layout = self.shape(text, style, runs);
            let paras = self.paras_entry(key);
            let p = Paragraph { hash, style: *style, runs: runs.to_vec(), layout, results: Vec::new() };
            if index < paras.len() {
                paras[index] = p;
            } else {
                paras.push(p);
            }
        }
        let p = &mut self.paras_entry(key)[index];
        let key = width_key(max_width);
        if let Some(&(_, w, l, h)) = p.results.iter().find(|r| r.0 == key) {
            return (w, l, h);
        }
        p.layout.break_all_lines(max_width);
        let (w, l, h) = (p.layout.width(), p.layout.len() as u32, p.layout.height());
        if p.results.len() == 4 {
            p.results.remove(0);
        }
        p.results.push((key, w, l, h));
        (w, l, h)
    }
    fn truncate(&mut self, key: TextKey, count: usize) {
        if let Some(p) = self.paras(key) {
            p.truncate(count);
        }
    }
    fn forget(&mut self, kernel: u32, node: NodeId) {
        self.cache.remove(&(kernel, node));
    }
    fn glyph_runs(&mut self, key: TextKey, style: &TextStyle, max_width: Option<f32>, fit: Option<f32>) -> Vec<ShapedRun> {
        let ellipsis = fit.map(|_| self.ellipsis(style));
        let Some(paras) = self.paras(key) else { return Vec::new() };
        let max_lines = style.max_lines.map_or(u32::MAX, u32::from);
        // Every line shown, as runs with their glyphs' advances.
        let mut lines: Vec<Vec<(ShapedRun, Vec<f32>)>> = Vec::new();
        let (mut top, mut hidden) = (0.0f32, false);
        'paras: for p in paras.iter_mut() {
            p.layout.break_all_lines(max_width);
            for line in p.layout.lines() {
                if lines.len() as u32 >= max_lines {
                    hidden = true;
                    break 'paras;
                }
                let mut runs = Vec::new();
                for item in line.items() {
                    let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
                    runs.push(shaped_run(&run, top, line.metrics()));
                }
                lines.push(runs);
            }
            if p.layout.is_empty() {
                if lines.len() as u32 >= max_lines {
                    hidden = true;
                    break;
                }
                lines.push(Vec::new());
            }
            top += if p.layout.is_empty() { style.line_height } else { p.layout.height() };
        }
        // The last line ends in an ellipsis if it does not fit or lines are hidden after it.
        if let (Some(fit), Some((dots, dots_w))) = (fit, ellipsis)
            && let Some(last) = lines.last_mut()
        {
            let end = last.iter().flat_map(|(r, a)| r.glyphs.iter().zip(a).map(|(g, a)| g.x + a.abs())).fold(0.0f32, f32::max);
            if hidden || end > fit + 0.01 {
                let limit = (fit - dots_w).max(0.0);
                let mut at = 0.0f32;
                for (run, advances) in last.iter_mut() {
                    let keep: Vec<bool> = run.glyphs.iter().zip(advances.iter()).map(|(g, a)| g.x + a.abs() <= limit + 0.01).collect();
                    let mut k = keep.iter();
                    run.glyphs.retain(|_| *k.next().unwrap());
                    let mut k = keep.iter();
                    advances.retain(|_| *k.next().unwrap());
                }
                // No space before the ellipsis.
                'trim: for (run, advances) in last.iter_mut().rev() {
                    while advances.last().is_some_and(|a| *a < 0.0) {
                        advances.pop();
                        run.glyphs.pop();
                    }
                    if !run.glyphs.is_empty() {
                        break 'trim;
                    }
                }
                for (run, advances) in last.iter() {
                    at = run.glyphs.iter().zip(advances.iter()).map(|(g, a)| g.x + a.abs()).fold(at, f32::max);
                }
                last.retain(|(r, _)| !r.glyphs.is_empty());
                let line_top = (lines.len() - 1) as f32 * style.line_height;
                let last = lines.last_mut().expect("a last line");
                for (mut run, _) in dots {
                    for g in &mut run.glyphs {
                        g.x += at;
                        g.y += line_top;
                    }
                    last.push((run, Vec::new()));
                }
            }
        }
        lines.into_iter().flatten().map(|(r, _)| r).collect()
    }
    fn visual_lines(&mut self, key: TextKey, text: &str, style: &TextStyle, max_width: Option<f32>) -> Vec<VisualLine> {
        let Some(paras) = self.paras(key) else { return even_lines(text, style.font_size * 0.5, style.line_height, max_width) };
        let mut out = Vec::new();
        let mut base = 0;
        let mut top = 0.0f32;
        for (para, p) in text.split('\n').zip(paras.iter_mut()) {
            p.layout.break_all_lines(max_width);
            let count = out.len();
            for line in p.layout.lines() {
                let m = line.metrics();
                let (line_top, line_height) = (top + m.block_min_coord, m.block_max_coord - m.block_min_coord);
                let r = line.text_range();
                let (s, e) = (base + r.start.min(para.len()), base + r.end.min(para.len()));
                let mut stops: Vec<(usize, f32)> = Vec::new();
                for item in line.items() {
                    let parley::PositionedLayoutItem::GlyphRun(run) = item else { continue };
                    let mut x = run.offset();
                    for c in run.run().visual_clusters() {
                        let cr = c.text_range();
                        let (lead, trail) = if c.is_rtl() { (cr.end, cr.start) } else { (cr.start, cr.end) };
                        stops.push((base + lead, x));
                        x += c.advance();
                        stops.push((base + trail, x));
                    }
                }
                stops.retain(|(b, _)| *b >= s && *b <= e);
                if !stops.iter().any(|st| st.0 == s) {
                    stops.push((s, 0.0));
                }
                if !stops.iter().any(|st| st.0 == e) {
                    let right = stops.iter().map(|st| st.1).fold(0.0f32, f32::max);
                    stops.push((e, right));
                }
                stops.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
                stops.dedup_by_key(|st| st.0);
                out.push(VisualLine { start: s, end: e, top: line_top, height: line_height, stops });
            }
            // An empty paragraph is one empty line.
            if out.len() == count {
                out.push(VisualLine { start: base, end: base, top, height: style.line_height, stops: vec![(base, 0.0)] });
            }
            top += if p.layout.is_empty() { style.line_height } else { p.layout.height() };
            base += para.len() + 1;
        }
        out
    }
    fn shaped(&self) -> u64 {
        self.shaped
    }
    fn register_fallback_font(&mut self, data: &'static [u8], scripts: &[[u8; 4]]) {
        let mut families: Vec<_> =
            self.fonts.collection.register_fonts(Blob::new(Arc::new(data)), None).into_iter().map(|(id, _)| id).collect();
        families.dedup();
        for s in scripts {
            self.fonts.collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*s), None), families.iter().copied());
        }
        self.cache.clear();
        self.ellipses.clear();
    }
    fn register_font(&mut self, data: &'static [u8]) {
        self.fonts.collection.register_fonts(Blob::new(Arc::new(data)), None);
        // Shapes may now resolve to the new faces.
        self.cache.clear();
        self.ellipses.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slotmap::KeyData;

    fn node() -> NodeId {
        NodeId::from(KeyData::from_ffi(1))
    }

    #[test]
    fn parley_wraps_and_caches_by_paragraph() {
        let mut e = ParleyEngine::default();
        let s = TextStyle::default();
        let one = measure(&mut e, node().into(), "hello world", 0, &s, &[], None);
        assert!(one.width > 50.0 && one.width < 120.0, "{one:?}");
        assert_eq!((one.lines, one.height), (1, 24.0));
        let narrow = measure(&mut e, node().into(), "hello world", 0, &s, &[], Some(one.width / 2.0 + 10.0));
        assert_eq!(narrow.lines, 2);
        assert!(narrow.width < one.width);
        let min = measure(&mut e, node().into(), "hello world", 0, &s, &[], Some(0.0));
        assert_eq!(min.lines, 2, "min-content breaks at every opportunity");

        let before = e.shaped;
        measure(&mut e, node().into(), "first\nsecond", 0, &s, &[], None);
        measure(&mut e, node().into(), "first\nsecond more", 0, &s, &[], None);
        assert_eq!(e.shaped - before, 3, "appending reshapes only the last paragraph");
        let mono = measure(&mut e, node().into(), "iiii", 0, &TextStyle { family: "IBM Plex Mono", ..s }, &[], None);
        let sans = measure(&mut e, node().into(), "iiii", 0, &s, &[], None);
        assert!(mono.width > sans.width * 1.5, "mono is monospaced: {mono:?} vs {sans:?}");
        let capped = measure(&mut e, node().into(), "a\nb\nc", 0, &TextStyle { max_lines: Some(2), ..s }, &[], None);
        assert_eq!((capped.lines, capped.truncated), (2, true));
    }

    #[test]
    fn parley_visual_lines_wrap_with_caret_stops() {
        let mut e = ParleyEngine::default();
        let s = TextStyle::default();
        let text = "hello world\nsecond";
        let one = measure(&mut e, node().into(), "hello world", 0, &s, &[], None);
        let narrow = Some(one.width * 0.7);
        measure(&mut e, node().into(), text, 0, &s, &[], narrow);
        let lines = e.visual_lines(node().into(), text, &s, narrow);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!((lines[0].start, lines[1].start, lines[1].end, lines[2].start, lines[2].end), (0, 6, 11, 12, 18));
        assert_eq!(lines.iter().map(|l| l.top).collect::<Vec<_>>(), [0.0, 24.0, 48.0]);
        for l in &lines {
            assert_eq!((l.stops[0].0, l.stops.last().unwrap().0), (l.start, l.end));
            assert!(l.stops.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 <= w[1].1), "{:?}", l.stops);
        }
        assert_eq!(lines[1].stops[0].1, 0.0, "a wrapped line starts at the left edge");
    }

    #[test]
    fn mono_is_half_an_em_per_char() {
        let mut e = MonoEngine;
        let s = TextStyle::default();
        assert_eq!(measure(&mut e, node().into(), "abcd", 0, &s, &[], None).width, 32.0);
        let m = measure(&mut e, node().into(), "abcd efgh", 0, &s, &[], Some(40.0));
        assert_eq!((m.width, m.lines), (32.0, 2));
    }
}
