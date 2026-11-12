//! Text layout behind the kernel ([design](../../../../docs/design/text.md#the-engine)): a
//! replaceable [`TextEngine`] makes [`TextLayout`]s, which text nodes own.
//!
//! [`ParleyEngine`] (the default) shapes with Parley. Headless it has the bundled Inter and IBM
//! Plex Mono only (`system-ui` and `ui-monospace` resolve to them), so a run measures the same
//! on every machine; [`ParleyEngine::system`] resolves them to the platform's fonts. It keeps
//! shaped paragraphs by content (text, style, runs), shared by every text showing them, within
//! a byte budget: appending to a stream reshapes its last paragraph, a row scrolling back or a
//! repeated label reshapes nothing, and a new width only breaks lines again. [`MonoEngine`] is
//! a deterministic monospace model for isolating text cost in benchmarks.
use parley::fontique::{Blob, Collection, CollectionOptions, FallbackKey, Script, SourceCache};
use parley::{FontContext, FontWeight, Layout, LayoutContext, StyleProperty};
use rustc_hash::{FxHashMap, FxHasher};
use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
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
    /// The positioned glyphs broken at `max_width`, at most `max_lines` lines, relative to the
    /// text's top-left corner. With `ellipsis` (its runs and width) and `fit`, a last line wider
    /// than `fit`, or followed by lines the limit hides, ends in it within `fit`. Engines that only
    /// measure return nothing.
    fn glyph_runs(
        &self,
        max_width: Option<f32>,
        max_lines: Option<u16>,
        fit: Option<f32>,
        ellipsis: Option<&(Vec<ShapedRun>, f32)>,
    ) -> Vec<ShapedRun>;
    /// The visual lines broken at `max_width` (`None`: one per paragraph): what editing places
    /// the caret with, and links and selections are drawn by.
    fn visual_lines(&self, max_width: Option<f32>) -> Vec<VisualLine>;
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
        for p in self.text.split('\n') {
            let (w, l) = self.paragraph(p, max_width);
            width = width.max(w);
            lines += l;
        }
        (width, lines, lines as f32 * self.line_height)
    }
    fn glyph_runs(&self, _: Option<f32>, _: Option<u16>, _: Option<f32>, _: Option<&(Vec<ShapedRun>, f32)>) -> Vec<ShapedRun> {
        Vec::new()
    }
    fn visual_lines(&self, max_width: Option<f32>) -> Vec<VisualLine> {
        even_lines(&self.text, self.em, self.line_height, max_width)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl TextEngine for MonoEngine {
    fn layout(&mut self, text: &str, style: &TextStyle, _: &[StyleRun], _: Option<&TextLayout>, _: usize) -> TextLayout {
        Rc::new(MonoText { text: text.to_owned(), em: style.font_size * 0.5, line_height: style.line_height })
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
    paras: Vec<Rc<Shaped>>,
    line_height: f32,
}

impl LaidText for ParleyText {
    fn measure(&self, max_width: Option<f32>) -> (f32, u32, f32) {
        let (mut width, mut lines, mut height) = (0.0f32, 0u32, 0.0f32);
        for p in &self.paras {
            let (w, l, h) = p.measure(max_width);
            width = width.max(w);
            lines += l.max(1);
            height += if l == 0 { self.line_height } else { h };
        }
        (width, lines, height)
    }
    fn glyph_runs(
        &self,
        max_width: Option<f32>,
        max_lines: Option<u16>,
        fit: Option<f32>,
        ellipsis: Option<&(Vec<ShapedRun>, f32)>,
    ) -> Vec<ShapedRun> {
        let max_lines = max_lines.map_or(u32::MAX, u32::from);
        // Every line shown, as runs with their glyphs' advances.
        let mut lines: Vec<Vec<(ShapedRun, Vec<f32>)>> = Vec::new();
        let (mut top, mut hidden) = (0.0f32, false);
        'paras: for p in &self.paras {
            p.break_at(max_width);
            let layout = p.layout.borrow();
            for line in layout.lines() {
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
            if layout.is_empty() {
                if lines.len() as u32 >= max_lines {
                    hidden = true;
                    break;
                }
                lines.push(Vec::new());
            }
            top += if layout.is_empty() { self.line_height } else { layout.height() };
        }
        // The last line ends in an ellipsis if it does not fit or lines are hidden after it.
        if let (Some(fit), Some((dots, dots_w))) = (fit, ellipsis)
            && let Some(last) = lines.last_mut()
        {
            let end = last.iter().flat_map(|(r, a)| r.glyphs.iter().zip(a).map(|(g, a)| g.x + a.abs())).fold(0.0f32, f32::max);
            if hidden || end > fit + 0.01 {
                let limit = (fit - dots_w).max(0.0);
                let mut at = 0.0f32;
                let line_top = last.first().map_or(0.0, |(r, _)| r.line_top);
                let baseline_shift = last.first().map_or(0.0, |(r, _)| r.baseline - r.line_top);
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
                for run in dots.iter() {
                    let mut run = run.clone();
                    for g in &mut run.glyphs {
                        g.x += at;
                        g.y += line_top + baseline_shift - run.baseline;
                    }
                    run.x += at;
                    run.line_top = line_top;
                    run.baseline = line_top + baseline_shift;
                    last.push((run, Vec::new()));
                }
            }
        }
        lines.into_iter().flatten().map(|(r, _)| r).collect()
    }
    fn visual_lines(&self, max_width: Option<f32>) -> Vec<VisualLine> {
        let mut out = Vec::new();
        let mut base = 0;
        let mut top = 0.0f32;
        for p in &self.paras {
            p.break_at(max_width);
            let layout = p.layout.borrow();
            let para = p.text.as_str();
            let count = out.len();
            for line in layout.lines() {
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
                out.push(VisualLine { start: base, end: base, top, height: self.line_height, stops: vec![(base, 0.0)] });
            }
            top += if layout.is_empty() { self.line_height } else { layout.height() };
            base += para.len() + 1;
        }
        out
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// How much shaped text an engine keeps by default.
const KEEP_BYTES: usize = 32 << 20;

/// Parley over bundled and registered fonts (and, from [`ParleyEngine::system`], the
/// platform's).
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
        let mut start = 0usize;
        for (i, p) in text.split('\n').enumerate() {
            let end = start + p.len();
            local.clear();
            for r in runs.iter().filter(|r| r.end > start && r.start < end) {
                local.push(StyleRun { start: r.start.max(start) - start, end: r.end.min(end) - start, ..*r });
            }
            // Unchanged since the last layout: the same paragraph, without comparing it.
            let kept = prev.and_then(|pt| pt.paras.get(i)).filter(|s| end < clean && s.style == shape && s.runs == local).cloned();
            paras.push(match kept {
                Some(s) => s,
                None => self.find_or_shape(p, &shape, &local),
            });
            start = end + 1;
        }
        // Paragraphs this text replaced that nothing else shows leave the cache now (a stream's
        // tail, typing): they would only wait for the budget.
        if let Some(pt) = prev {
            let replaced: Vec<Rc<Shaped>> =
                pt.paras.iter().filter(|o| Rc::strong_count(o) == 2 && !paras.iter().any(|n| Rc::ptr_eq(n, o))).cloned().collect();
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
                runs.push(shaped_run(&run, 0.0, line.metrics()).0);
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
    fn parley_visual_lines_wrap_with_caret_stops() {
        let mut e = ParleyEngine::default();
        let s = TextStyle::default();
        let one = metrics(&lay(&mut e, "hello world", &s), &s, None);
        let narrow = Some(one.width * 0.7);
        let lines = lay(&mut e, "hello world\nsecond", &s).visual_lines(narrow);
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
        assert_eq!(metrics(&lay(&mut e, "abcd", &s), &s, None).width, 32.0);
        let m = metrics(&lay(&mut e, "abcd efgh", &s), &s, Some(40.0));
        assert_eq!((m.width, m.lines), (32.0, 2));
    }
}
