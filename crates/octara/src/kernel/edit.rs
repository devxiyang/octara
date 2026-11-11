//! A text input's line on screen: its selection and composition, where each caret stop is, the
//! horizontal scroll that keeps the caret in view, and how it is painted.

use super::text::{self, Wrap};
use super::{Color, Dirty, Kernel, Kind, NodeId, State, Token};
use crate::paint::{GlyphRun, Item, Quad, Rect};

/// Floors `i` to a char boundary of `s` (clamped to its length).
pub fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A text input's line, laid out.
pub(crate) struct Line {
    /// The text shown: the value with the composition at the caret.
    pub shown: String,
    /// Caret stops of `shown`, `(byte, x)` from the line's start.
    pub stops: Vec<(usize, f32)>,
    /// Where the composition is in `shown`.
    pub preedit: std::ops::Range<usize>,
    /// The selection in `shown` (start ≤ end) and the caret.
    pub sel: (usize, usize),
    pub caret: usize,
    /// The content box's left and top, its width, and the line scroll.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub scroll: f32,
}

impl Line {
    /// The x (from the line's start) of byte `b` of `shown`.
    pub fn x_of(&self, b: usize) -> f32 {
        match self.stops.binary_search_by_key(&b, |s| s.0) {
            Ok(i) => self.stops[i].1,
            Err(i) => {
                // Inside a ligature: interpolate between the stops around it.
                let (a, z) = (self.stops[i.saturating_sub(1)], self.stops[i.min(self.stops.len() - 1)]);
                if z.0 == a.0 { a.1 } else { a.1 + (z.1 - a.1) * (b - a.0) as f32 / (z.0 - a.0) as f32 }
            }
        }
    }
    /// The last stop of `shown` at or before `x`: where the character under `x` starts.
    pub fn stop_before(&self, x: f32) -> usize {
        self.stops.iter().rev().find(|s| s.1 <= x).map_or(0, |s| s.0)
    }
    /// The stop of `shown` nearest to `x` (from the line's start).
    pub fn stop_at(&self, x: f32) -> usize {
        self.stops.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(0, |s| s.0)
    }
}

impl Kernel {
    /// Sets a text input's selection (`anchor`, `caret`: byte offsets into its value).
    /// Kept as given: the value may be about to change (an edit sets the selection, then its
    /// handler sets the value); reads clamp to the value.
    pub fn set_selection(&mut self, id: NodeId, anchor: usize, caret: usize) {
        let sel = (anchor, caret);
        if self.node(id).selection != sel {
            // A composition moves with the caret: the shown text changes from the earlier caret.
            let n = self.node(id);
            if !n.preedit.is_empty() {
                n.text_clean.set(n.text_clean.get().min(n.selection.1.min(caret)));
            }
            self.node_mut(id).selection = sel;
            self.mark(Dirty::PAINT);
        }
    }
    /// A text input's selection `(anchor, caret)`, within its current value.
    pub fn selection(&self, id: NodeId) -> (usize, usize) {
        let n = self.node(id);
        (floor_boundary(&n.value, n.selection.0), floor_boundary(&n.value, n.selection.1))
    }

    /// Lays out a text input's line (after the last layout).
    pub(crate) fn line(&self, id: NodeId) -> Option<Line> {
        let n = self.node(id);
        if n.kind != Kind::TextInput {
            return None;
        }
        let (anchor, caret) = self.selection(id);
        let shown = format!("{}{}{}", &n.value[..caret], n.preedit, &n.value[caret..]);
        let preedit = caret..caret + n.preedit.len();
        let to_shown = |b: usize| if b <= caret { b } else { b + n.preedit.len() };
        let sel = (to_shown(anchor.min(caret)), to_shown(anchor.max(caret)));
        let (x, y, _, _) = self.shown_rect(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let (cx, cy) = (x + l.border.left + l.padding.left, y + l.border.top + l.padding.top);
        let width = (l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0);
        let style = super::TextStyle { wrap: Wrap::None, ..n.visual.text_style() };
        let stops = if shown.is_empty() {
            vec![(0, 0.0)]
        } else {
            let mut engine = self.text.borrow_mut();
            text::measure(&mut **engine, id, &shown, n.text_clean.replace(shown.len()), &style, None);
            engine.caret_stops(id, &shown, &style)
        };
        let mut line = Line { shown, stops, preedit, sel, caret: caret + n.preedit.len(), x: cx, y: cy, width, scroll: 0.0 };
        // Keep the caret in view, moving the line as little as possible.
        let (caret_x, end_x) = (line.x_of(line.caret), line.stops.last().map_or(0.0, |s| s.1));
        let room = (width - 1.0).max(0.0);
        let mut scroll = n.text_scroll.get();
        if caret_x - scroll > room {
            scroll = caret_x - room;
        }
        if caret_x < scroll {
            scroll = caret_x;
        }
        scroll = scroll.min((end_x - room).max(0.0)).max(0.0);
        n.text_scroll.set(scroll);
        line.scroll = scroll;
        Some(line)
    }

    /// The value offset nearest window x `x` in a text input (where a click puts the caret);
    /// with `under`, where the character under `x` starts (what a double click selects around).
    pub fn input_offset_at(&self, id: NodeId, x: f32, under: bool) -> Option<usize> {
        let line = self.line(id)?;
        let lx = x - line.x + line.scroll;
        let b = if under { line.stop_before(lx) } else { line.stop_at(lx) };
        // Back to value coordinates (the composition is not part of the value).
        Some(if b <= line.preedit.start { b } else { b.saturating_sub(line.preedit.len()).max(line.preedit.start) })
    }

    /// Where the caret of a text input is (logical pixels). Input methods place their
    /// candidate window next to it.
    pub fn caret(&self, id: NodeId) -> Option<Rect> {
        let line = self.line(id)?;
        let x = line.x + line.x_of(line.caret) - line.scroll;
        Some(Rect { x, y: line.y, w: 1.0, h: self.node(id).visual.line_height })
    }

    /// Paints a text input: selection, text, composition underline and caret, clipped to its
    /// content box and scrolled to keep the caret in view.
    pub(crate) fn paint_input(&self, id: NodeId, opacity: f32, out: &mut Vec<Item>) {
        let Some(line) = self.line(id) else { return };
        let n = self.node(id);
        let shown = self.presented(id);
        let v = &shown;
        let fade = |c: Color| if opacity >= 1.0 { c } else { c.multiply_alpha(opacity) };
        let lh = v.line_height;
        let clip = Rect { x: line.x, y: line.y, w: line.width, h: lh };
        out.push(Item::PushClip { source: id, rect: clip, radius: 0.0 });
        let at = |b: usize| line.x + line.x_of(b) - line.scroll;
        let quad = |rect: Rect, fill: Color| {
            Item::Quad(Quad { source: id, rect, radius: 0.0, fill: Some(fill), gradient: None, border: [0.0; 4], border_color: fill })
        };
        let focused = n.state.contains(State::FOCUSED);
        if line.sel.0 != line.sel.1 {
            let accent = self.theme.get(Token::Accent);
            let fill = if focused { accent.multiply_alpha(0.3) } else { accent.multiply_alpha(0.15) };
            let (x0, x1) = (at(line.sel.0), at(line.sel.1));
            out.push(quad(Rect { x: x0, y: line.y, w: x1 - x0, h: lh }, fade(fill)));
        }
        if !line.shown.is_empty() {
            let style = super::TextStyle { wrap: Wrap::None, ..v.text_style() };
            let runs = self.text.borrow_mut().glyph_runs(id, &style, None, None);
            for r in runs {
                let glyphs = r
                    .glyphs
                    .into_iter()
                    .map(|g| crate::paint::Glyph { id: g.id, x: g.x + line.x - line.scroll, y: g.y + line.y })
                    .collect();
                out.push(Item::Glyphs(GlyphRun {
                    source: id,
                    font: r.font,
                    size: r.size,
                    coords: r.coords,
                    embolden: r.embolden,
                    color: fade(v.text),
                    glyphs,
                }));
            }
        }
        if !line.preedit.is_empty() {
            let (x0, x1) = (at(line.preedit.start), at(line.preedit.end));
            out.push(quad(Rect { x: x0, y: line.y + lh - 2.0, w: x1 - x0, h: 1.0 }, fade(v.text)));
        }
        if focused && line.sel.0 == line.sel.1 {
            let x = at(line.caret).min(line.x + line.width - 1.0).max(line.x);
            out.push(quad(Rect { x, y: line.y, w: 1.0, h: lh }, fade(v.text)));
        }
        out.push(Item::PopClip);
    }
}
