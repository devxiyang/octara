//! Rich text ([design](../../../../docs/design/text.md)): one string and styled runs over it.
//! A run patches the text's own style for its range: weight, slant, monospace, size, colors,
//! decorations, and a link. Plain text is rich text without runs.

use super::theme::{Paint, Token};
use std::ops::Range;
use std::rc::Rc;

/// How a range of a text differs from the text's style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub weight: Option<u16>,
    pub italic: bool,
    /// The theme's monospace family.
    pub mono: bool,
    /// Font size in logical pixels.
    pub size: Option<f32>,
    pub color: Option<Paint>,
    /// Drawn behind the range's glyphs, line by line (inline code).
    pub background: Option<Paint>,
    pub underline: bool,
    pub strike: bool,
    /// A link: what `on_link` gets when it is pressed. The range becomes a `link` element of
    /// its own (focusable, pressable, named by its text).
    pub link: Option<Rc<str>>,
    /// A link's test id.
    pub id: Option<Rc<str>>,
}

impl Span {
    pub fn weight(mut self, w: u16) -> Span {
        self.weight = Some(w);
        self
    }
    pub fn bold(self) -> Span {
        self.weight(700)
    }
    pub fn italic(mut self) -> Span {
        self.italic = true;
        self
    }
    pub fn mono(mut self) -> Span {
        self.mono = true;
        self
    }
    pub fn size(mut self, px: f32) -> Span {
        self.size = Some(px);
        self
    }
    pub fn color(mut self, c: impl Into<Paint>) -> Span {
        self.color = Some(c.into());
        self
    }
    pub fn background(mut self, c: impl Into<Paint>) -> Span {
        self.background = Some(c.into());
        self
    }
    pub fn underline(mut self) -> Span {
        self.underline = true;
        self
    }
    pub fn strike(mut self) -> Span {
        self.strike = true;
        self
    }
    pub fn link(mut self, target: impl AsRef<str>) -> Span {
        self.link = Some(target.as_ref().into());
        self
    }
    /// The link's test id.
    pub fn id(mut self, id: impl AsRef<str>) -> Span {
        self.id = Some(id.as_ref().into());
        self
    }
}

/// Text with styled ranges: `rich().text("See ").link("the docs", url).text(".")`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rich {
    text: String,
    /// Sorted, not overlapping; plain ranges have none.
    runs: Vec<(Range<usize>, Span)>,
}

/// An empty rich text, to add to.
pub fn rich() -> Rich {
    Rich::default()
}

impl Rich {
    /// Plain text in the text's own style.
    pub fn text(mut self, s: impl AsRef<str>) -> Rich {
        self.text.push_str(s.as_ref());
        self
    }
    /// `s` styled by `span`.
    pub fn span(mut self, s: impl AsRef<str>, span: Span) -> Rich {
        let s = s.as_ref();
        if !s.is_empty() {
            let start = self.text.len();
            self.text.push_str(s);
            self.runs.push((start..self.text.len(), span));
        }
        self
    }
    /// `s` styled by `f` applied to a plain span.
    pub fn styled(self, s: impl AsRef<str>, f: impl FnOnce(Span) -> Span) -> Rich {
        self.span(s, f(Span::default()))
    }
    pub fn bold(self, s: impl AsRef<str>) -> Rich {
        self.styled(s, Span::bold)
    }
    pub fn italic(self, s: impl AsRef<str>) -> Rich {
        self.styled(s, Span::italic)
    }
    /// Inline code: monospace on a tinted background.
    pub fn code(self, s: impl AsRef<str>) -> Rich {
        self.styled(s, |p| p.mono().background(Token::SurfaceHover))
    }
    /// A link to `target` showing `s`: the accent color, underlined.
    pub fn link(self, s: impl AsRef<str>, target: impl AsRef<str>) -> Rich {
        self.styled(s, |p| p.link(target))
    }
    pub fn as_str(&self) -> &str {
        &self.text
    }
    pub fn runs(&self) -> &[(Range<usize>, Span)] {
        &self.runs
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    /// The parts, to hand over.
    pub fn into_parts(self) -> (String, Vec<(Range<usize>, Span)>) {
        (self.text, self.runs)
    }
}

impl From<&str> for Rich {
    fn from(s: &str) -> Rich {
        Rich { text: s.to_owned(), runs: Vec::new() }
    }
}

impl From<String> for Rich {
    fn from(text: String) -> Rich {
        Rich { text, runs: Vec::new() }
    }
}

impl From<&String> for Rich {
    fn from(s: &String) -> Rich {
        Rich { text: s.clone(), runs: Vec::new() }
    }
}

// ---- in the kernel ----

use super::text::{self, ShapedRun, StyleRun, TextStyle, VisualLine, Wrap};
use super::{Content, Kernel, Kind, NodeId, Role};
use crate::paint::Rect;

/// A link run: its run number, range, target, test id and words.
type LinkSpec = (usize, Range<usize>, Rc<str>, Option<Rc<str>>, String);

/// A link node's range of its parent text and its target.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LinkContent {
    pub range: Range<usize>,
    pub target: Rc<str>,
    /// Which of the text's runs it is.
    pub run: usize,
}

/// What a text's glyphs were painted for, and the glyphs.
#[derive(Debug)]
pub(crate) struct Painted {
    key: (u64, Option<u32>, Option<u32>, TextStyle, u64),
    pub runs: Rc<Vec<ShapedRun>>,
}

/// What a text's lines were laid out for, and the lines.
#[derive(Debug)]
pub(crate) struct Lines {
    key: (u64, Option<u32>, TextStyle, u64),
    pub lines: Rc<Vec<VisualLine>>,
}

impl Kernel {
    /// Sets a text node's rich text: its string and runs. A run with a link gets a `link` child
    /// (pressable and focusable, named by its text). Returns whether anything changed.
    pub fn set_rich(&mut self, id: NodeId, text: &str, runs: &[(Range<usize>, Span)]) -> bool {
        let (same_text, same_runs) = match &self.node(id).content {
            Content::Text(t) => (t.text == text, t.runs == runs),
            _ => return false,
        };
        if same_text && same_runs {
            self.counters.prop_noops += 1;
            return false;
        }
        if !same_text {
            self.set_text(id, text);
        }
        if !same_runs {
            let shape = self.style_runs(runs);
            if let Content::Text(t) = &mut self.node_mut(id).content {
                t.runs = runs.to_vec();
                t.shape = shape;
            }
            self.text_changed(id, "runs", "");
            self.sync_links(id);
        }
        true
    }

    /// The text's runs as the engine shapes them: every run carries its brush (its number from
    /// 1), and its shape-changing properties.
    pub(super) fn style_runs(&self, runs: &[(Range<usize>, Span)]) -> Vec<StyleRun> {
        let mono = self.res.theme.borrow().font_mono;
        runs.iter()
            .enumerate()
            .map(|(i, (r, s))| StyleRun {
                start: r.start,
                end: r.end,
                brush: i as u16 + 1,
                family: s.mono.then_some(mono),
                size: s.size,
                weight: s.weight,
                italic: s.italic,
            })
            .collect()
    }

    /// The theme changed: runs resolve their monospace family again.
    pub(super) fn restyle_runs(&mut self) {
        let ids: Vec<NodeId> =
            self.nodes.iter().filter(|(_, n)| matches!(&n.content, Content::Text(t) if !t.runs.is_empty())).map(|(id, _)| id).collect();
        for id in ids {
            let runs = match &self.node(id).content {
                Content::Text(t) => t.runs.clone(),
                _ => continue,
            };
            let shape = self.style_runs(&runs);
            let changed = matches!(&self.node(id).content, Content::Text(t) if t.shape != shape);
            if changed {
                if let Content::Text(t) = &mut self.node_mut(id).content {
                    t.shape = shape;
                }
                self.text_changed(id, "runs", "");
            }
        }
    }

    /// Keeps a text's link children one per link run, in order.
    fn sync_links(&mut self, id: NodeId) {
        let wanted: Vec<LinkSpec> = match &self.node(id).content {
            Content::Text(t) => t
                .runs
                .iter()
                .enumerate()
                .filter_map(|(i, (r, s))| s.link.clone().map(|l| (i, r.clone(), l, s.id.clone(), t.text[r.clone()].to_owned())))
                .collect(),
            _ => return,
        };
        let existing: Vec<NodeId> = self.node(id).children.clone();
        for (k, (run, range, target, test_id, label)) in wanted.iter().enumerate() {
            let link = match existing.get(k) {
                Some(l) => *l,
                None => {
                    let l = self.create(Kind::Link);
                    self.node_mut(l).parent = Some(id);
                    self.node_mut(l).focusable = true;
                    self.node_mut(id).children.push(l);
                    if self.node(id).attached {
                        self.set_attached(l, true);
                    }
                    self.a11y_children_changed(id);
                    l
                }
            };
            if let Content::Link(c) = &mut self.node_mut(link).content {
                **c = LinkContent { range: range.clone(), target: target.clone(), run: *run };
            }
            self.set_role(link, Role::Link);
            self.set_label(link, Some(label));
            self.set_value(link, target);
            self.set_test_id(link, test_id.as_deref());
        }
        for extra in existing.into_iter().skip(wanted.len()) {
            let pos = self.node(id).children.iter().position(|c| *c == extra).expect("a link child");
            self.node_mut(id).children.remove(pos);
            self.node_mut(extra).parent = None;
            self.a11y_children_changed(id);
            self.a11y_forget(extra);
            self.destroy(extra);
        }
    }

    /// Where a text's content box starts inside its node, the width it breaks at (`None`:
    /// unbounded) and the content box's width.
    pub(crate) fn text_box(&self, id: NodeId) -> ((f32, f32), Option<f32>, f32) {
        let n = self.node(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let inner = (l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0);
        let limit = match n.visual.text_style().wrap {
            Wrap::Words => Some(inner),
            Wrap::None | Wrap::Truncate => None,
        };
        ((l.border.left + l.padding.left, l.border.top + l.padding.top), limit, inner)
    }

    /// A text's lines as laid out now (kept until the text, its width or its style changes).
    pub(crate) fn text_lines(&self, id: NodeId) -> Rc<Vec<VisualLine>> {
        let n = self.node(id);
        let Content::Text(t) = &n.content else { return Rc::default() };
        let style = n.visual.text_style();
        let (_, limit, _) = self.text_box(id);
        let key = (t.version, limit.map(f32::to_bits), style, self.fonts_generation);
        if let Some(l) = t.lines.borrow().as_ref().filter(|l| l.key == key) {
            return l.lines.clone();
        }
        let Some(layout) = super::node_layout(&mut **self.res.text.borrow_mut(), n, self.fonts_generation) else { return Rc::default() };
        let lines = Rc::new(layout.visual_lines(limit));
        *t.lines.borrow_mut() = Some(Lines { key, lines: lines.clone() });
        lines
    }

    /// A text's glyphs at `limit`, ending in an ellipsis within `fit` (kept until the text, the
    /// width or the style changes).
    pub(crate) fn text_glyphs(&self, id: NodeId, style: &TextStyle, limit: Option<f32>, fit: Option<f32>) -> Rc<Vec<ShapedRun>> {
        let n = self.node(id);
        let Content::Text(t) = &n.content else { return Rc::default() };
        let key = (t.version, limit.map(f32::to_bits), fit.map(f32::to_bits), *style, self.fonts_generation);
        if let Some(p) = t.painted.borrow().as_ref().filter(|p| p.key == key) {
            return p.runs.clone();
        }
        let mut engine = self.res.text.borrow_mut();
        let Some(layout) = super::node_layout(&mut **engine, n, self.fonts_generation) else { return Rc::default() };
        let ellipsis = fit.and_then(|_| engine.ellipsis(style));
        let runs = Rc::new(layout.glyph_runs(limit, style.max_lines, fit, ellipsis.as_ref()));
        self.glyphs_taken.set(self.glyphs_taken.get() + 1);
        *t.painted.borrow_mut() = Some(Painted { key, runs: runs.clone() });
        runs
    }

    /// The rects a link covers, in its parent text's node coordinates.
    pub fn link_rects(&self, link: NodeId) -> Vec<Rect> {
        let Content::Link(c) = &self.node(link).content else { return Vec::new() };
        let Some(text) = self.node(link).parent else { return Vec::new() };
        let ((ox, oy), _, _) = self.text_box(text);
        text::range_rects(&self.text_lines(text), c.range.clone()).into_iter().map(|r| Rect { x: r.x + ox, y: r.y + oy, ..r }).collect()
    }

    /// A link's frame in its text: the box around the rects it covers.
    pub(crate) fn link_frame(&self, link: NodeId) -> (f32, f32, f32, f32) {
        let rects = self.link_rects(link);
        let Some(first) = rects.first() else { return (0.0, 0.0, 0.0, 0.0) };
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.right(), first.bottom());
        for r in &rects[1..] {
            x0 = x0.min(r.x);
            y0 = y0.min(r.y);
            x1 = x1.max(r.right());
            y1 = y1.max(r.bottom());
        }
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// The text's runs (rich text).
    pub fn runs(&self, id: NodeId) -> &[(Range<usize>, Span)] {
        match &self.node(id).content {
            Content::Text(t) => &t.runs,
            _ => &[],
        }
    }

    /// A link node's target.
    pub fn link_target(&self, id: NodeId) -> Option<Rc<str>> {
        match &self.node(id).content {
            Content::Link(c) => Some(c.target.clone()),
            _ => None,
        }
    }

    /// Paragraphs the text engine has shaped (where it counts them).
    pub fn text_engine_shaped(&self) -> u64 {
        self.res.text.borrow().shaped()
    }

    /// The first node with `role`, in tree order.
    pub fn find_by_role(&self, role: Role) -> Option<NodeId> {
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            if id != self.root && self.role(id) == role {
                return Some(id);
            }
            stack.extend(self.node(id).children.iter().rev().copied());
        }
        None
    }

    /// How many times text glyphs were taken from the engine (not reused): a stream's cost.
    pub fn glyph_extractions(&self) -> u64 {
        self.glyphs_taken.get()
    }
}

impl Kernel {
    /// Paints a text's shaped runs at `origin`: each run in its span's color (a link in the
    /// accent, brighter while hovered or pressed), backgrounds behind, underlines and
    /// strikethroughs over.
    pub(crate) fn paint_runs(
        &self,
        id: NodeId,
        runs: &[ShapedRun],
        origin: (f32, f32),
        color: super::Color,
        opacity: f32,
        out: &mut Vec<crate::paint::Item>,
    ) {
        use crate::paint::{GlyphRun, Item, Quad};
        let fade = |c: super::Color| c.multiply_alpha(opacity);
        let theme = self.res.theme.borrow();
        let spans = self.runs(id);
        // A link's state: its node is the n-th link child.
        let link_state = |run: usize| {
            self.node(id).children.iter().find_map(|c| match &self.node(*c).content {
                Content::Link(l) if l.run == run => Some(self.node(*c).state),
                _ => None,
            })
        };
        let span_of = |brush: u16| (brush as usize).checked_sub(1).and_then(|i| spans.get(i).map(|s| (i, &s.1)));
        let resolve = |p: &super::Paint| match p {
            super::Paint::Token(t) => theme.get(*t),
            super::Paint::Color(c) => *c,
        };
        let (ox, oy) = origin;
        // Backgrounds first, behind every glyph.
        for r in runs {
            if let Some((_, span)) = span_of(r.brush)
                && let Some(bg) = &span.background
            {
                out.push(Item::Quad(Quad {
                    source: id,
                    // Around the glyphs (ascent and descent), not the whole line box.
                    rect: Rect { x: ox + r.x - 2.0, y: oy + r.baseline - r.size * 0.95, w: r.width + 4.0, h: r.size * 1.25 },
                    radius: 3.0,
                    fill: Some(fade(resolve(bg))),
                    gradient: None,
                    border: [0.0; 4],
                    border_color: super::Color::TRANSPARENT,
                }));
            }
        }
        for r in runs {
            let span = span_of(r.brush);
            let link = span.and_then(|(i, s)| s.link.as_ref().map(|_| link_state(i)));
            let c = match (span, link) {
                (Some((_, s)), _) if s.color.is_some() => resolve(s.color.as_ref().expect("checked")),
                (_, Some(state)) => {
                    let lit = state.is_some_and(|st| st.intersects(super::State::HOVER | super::State::PRESSED));
                    theme.get(if lit { super::Token::AccentHover } else { super::Token::Accent })
                }
                _ => color,
            };
            let glyphs: std::sync::Arc<[_]> = r.glyphs.iter().map(|g| crate::paint::Glyph { id: g.id, x: g.x, y: g.y }).collect();
            self.glyph_runs_built.set(self.glyph_runs_built.get() + 1);
            out.push(Item::Glyphs(GlyphRun {
                source: id,
                font: r.font.clone(),
                size: r.size,
                coords: r.coords.clone(),
                embolden: r.embolden,
                color: fade(c),
                origin: (ox, oy),
                glyphs,
            }));
            let underline = span.is_some_and(|(_, s)| s.underline || s.link.is_some());
            let strike = span.is_some_and(|(_, s)| s.strike);
            let thick = (r.size / 14.0).max(1.0);
            for (on, y) in [(underline, r.baseline + r.size * 0.12), (strike, r.baseline - r.size * 0.28)] {
                if on && r.width > 0.0 {
                    out.push(Item::Quad(Quad {
                        source: id,
                        rect: Rect { x: ox + r.x, y: oy + y, w: r.width, h: thick },
                        radius: 0.0,
                        fill: Some(fade(c)),
                        gradient: None,
                        border: [0.0; 4],
                        border_color: super::Color::TRANSPARENT,
                    }));
                }
            }
        }
    }
}
