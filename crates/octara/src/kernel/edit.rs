//! A text input on screen: its text laid out in visual lines (one for a single-line field,
//! wrapped at the field's width for a multi-line one), its selection and composition, where each
//! caret position is, the scroll that keeps the caret in view, and how it is painted.

use super::text::{self, VisualLine, Wrap};
use super::{Color, Dirty, Kernel, NodeId, State, TextDocument, TextGeometry, TextInputLayout, Token};
use crate::paint::{GlyphRun, Item, Quad, Rect};
use unicode_segmentation::UnicodeSegmentation;

/// Half a caret blink, in seconds (shown this long, then hidden this long; AppKit's and GPUI's).
pub const CARET_BLINK: f64 = 0.5;

/// What a secure field shows for each grapheme of its value.
const MASK: char = '•';

/// How long a touch platform shows the last character typed into a secure field (seconds).
pub const REVEAL_LAST: f64 = 1.5;

/// Floors `i` to a char boundary of `s` (clamped to its length).
pub fn floor_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A text input, laid out (after the last layout).
pub(crate) struct Field {
    /// The text shown: the value with the composition at the caret.
    pub shown: String,
    /// Where the composition is in `shown`.
    pub preedit: std::ops::Range<usize>,
    /// The changed ranges in the displayed draft and committed value. These can extend
    /// beyond the current marked range after a native partial replacement.
    pub overlay: std::ops::Range<usize>,
    pub replaced: std::ops::Range<usize>,
    /// The selection in `shown` (start ≤ end) and the caret.
    pub sel: (usize, usize),
    pub caret: usize,
    pub show_caret: bool,
    /// The content box: left, top, width, height.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub line_height: f32,
    pub lines: Vec<VisualLine>,
    /// How far the text is scrolled (x for one line, y for several).
    pub scroll: (f32, f32),
    /// A masked field: `(value offset, shown offset)` at each grapheme boundary.
    pub masked: Option<Vec<(usize, usize)>>,
}

impl Field {
    /// Read geometry at the already-settled offset. Reveal is an explicit operation on this
    /// owned geometry, used by settlement and native document prediction, never by readers.
    fn new(shown: String, caret: usize, layout: &TextInputLayout, mut lines: Vec<VisualLine>) -> Self {
        let content = layout.content;
        let line_height = layout.style.line_height;
        if lines.is_empty() {
            lines.push(VisualLine { start: 0, end: 0, top: 0.0, height: line_height, stops: vec![(0, 0.0)] });
        }
        Self {
            shown,
            caret,
            preedit: caret..caret,
            overlay: caret..caret,
            replaced: caret..caret,
            sel: (caret, caret),
            show_caret: true,
            x: content.x,
            y: content.y,
            width: content.w,
            height: content.h,
            line_height,
            lines,
            scroll: if layout.multiline { (0.0, layout.scroll) } else { (layout.scroll, 0.0) },
            masked: None,
        }
    }

    pub(crate) fn into_geometry(self) -> TextGeometry {
        TextGeometry { origin: (self.x - self.scroll.0, self.y - self.scroll.1), lines: self.lines }
    }

    /// The same caret reveal for a painted field and a native document not yet committed to
    /// it. This changes only the owned layout, returning the multiline scroll's keep key.
    fn reveal(&mut self, multiline: bool, scroll: f32, kept: Option<(usize, usize)>) -> Option<(usize, usize)> {
        let (caret_x, caret_y) = self.pos(self.caret);
        let keep = |at: f32, size: f32, room: f32, extent: f32| {
            let mut s = scroll;
            if at + size - s > room {
                s = at + size - room;
            }
            if at < s {
                s = at;
            }
            s.min((extent - room).max(0.0)).max(0.0)
        };
        if multiline {
            let now = (self.caret, self.shown.len());
            let sy = if kept == Some(now) {
                scroll.min((self.text_height() - self.height).max(0.0)).max(0.0)
            } else {
                keep(caret_y, self.line_height, self.height, self.text_height())
            };
            self.scroll = (0.0, sy);
            Some(now)
        } else {
            let end_x = self.lines[0].stops.last().map_or(0.0, |s| s.1);
            self.scroll = (keep(caret_x, 0.0, (self.width - 1.0).max(0.0), end_x), 0.0);
            kept
        }
    }

    /// The line byte `b` is shown on: where a wrapped line ends, the next line (which starts
    /// there).
    pub fn line_of(&self, b: usize) -> usize {
        self.lines.iter().rposition(|l| l.start <= b).unwrap_or(0)
    }

    /// Where a caret can stand on line `li`: its stops, without its end when the next line
    /// starts there (that position shows on the next line).
    fn stops(&self, li: usize) -> &[(usize, f32)] {
        let l = &self.lines[li];
        let wraps = self.lines.get(li + 1).is_some_and(|n| n.start == l.end);
        if wraps && l.stops.len() > 1 { &l.stops[..l.stops.len() - 1] } else { &l.stops }
    }

    /// The x of byte `b` on line `li` (from the line's start).
    fn x_on(&self, li: usize, b: usize) -> f32 {
        let stops = &self.lines[li].stops;
        match stops.binary_search_by_key(&b, |s| s.0) {
            Ok(i) => stops[i].1,
            Err(i) => {
                // Inside a ligature: interpolate between the stops around it.
                let (a, z) = (stops[i.saturating_sub(1)], stops[i.min(stops.len() - 1)]);
                if z.0 == a.0 { a.1 } else { a.1 + (z.1 - a.1) * (b - a.0) as f32 / (z.0 - a.0) as f32 }
            }
        }
    }

    /// Where byte `b` is, from the content's top-left, unscrolled.
    pub fn pos(&self, b: usize) -> (f32, f32) {
        let li = self.line_of(b);
        (self.x_on(li, b), self.lines[li].top)
    }

    /// The line at `y` (from the content's top, unscrolled), clamped to the text.
    fn line_at(&self, y: f32) -> usize {
        self.lines.iter().rposition(|l| l.top <= y).unwrap_or(0)
    }

    /// The caret position nearest `x` on line `li`; with `under`, the one where the character
    /// under `x` starts.
    fn stop_on(&self, li: usize, x: f32, under: bool) -> usize {
        let stops = self.stops(li);
        if under {
            stops.iter().rev().find(|s| s.1 <= x).map_or(stops[0].0, |s| s.0)
        } else {
            stops.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(0, |s| s.0)
        }
    }

    /// The caret position at `(x, y)` (from the content's top-left, unscrolled).
    pub fn byte_at(&self, x: f32, y: f32, under: bool) -> usize {
        self.stop_on(self.line_at(y), x, under)
    }

    /// The position `delta` lines from `b`, as near as possible to `goal_x`; past the first or
    /// last line, the start or end of the text.
    pub fn by_lines(&self, b: usize, goal_x: f32, delta: i32) -> usize {
        let to = self.line_of(b) as i64 + delta as i64;
        if to < 0 {
            return 0;
        }
        if to >= self.lines.len() as i64 {
            return self.shown.len();
        }
        self.stop_on(to as usize, goal_x, false)
    }

    /// The first and last caret positions of the line `b` is on.
    pub fn line_edges(&self, b: usize) -> (usize, usize) {
        let stops = self.stops(self.line_of(b));
        (stops[0].0, stops[stops.len() - 1].0)
    }

    /// Rectangles covering `s..e` (from the content's top-left, unscrolled), one per line it
    /// touches; a line the selection continues past shows its line break.
    pub fn rects(&self, s: usize, e: usize) -> Vec<Rect> {
        let mut out = Vec::new();
        for (li, l) in self.lines.iter().enumerate() {
            if l.end < s || l.start > e || (l.start == e && s != e) {
                continue;
            }
            let (a, z) = (s.max(l.start), e.min(l.end));
            let x0 = self.x_on(li, a);
            let mut x1 = self.x_on(li, z);
            if e > l.end && self.lines.get(li + 1).is_none_or(|n| n.start > l.end) {
                x1 += self.line_height * 0.25;
            }
            if x1 > x0 {
                out.push(Rect { x: x0, y: l.top, w: x1 - x0, h: self.line_height });
            }
        }
        out
    }

    /// The text's height (its lines).
    pub fn text_height(&self) -> f32 {
        self.lines.len().max(1) as f32 * self.line_height
    }
}

impl Kernel {
    /// A text input's display-only hint, omitted when empty. It is never its name or value.
    pub fn placeholder(&self, id: NodeId) -> Option<&str> {
        self.node(id).input().map(|i| i.placeholder.as_str()).filter(|s| !s.is_empty())
    }

    /// Changes the display-only hint without changing selection, composition or sizing.
    pub fn set_placeholder(&mut self, id: NodeId, placeholder: &str) -> bool {
        let Some(input) = self.node_mut(id).input_mut() else { return false };
        if input.placeholder == placeholder {
            return false;
        }
        input.placeholder = placeholder.to_owned();
        self.changed(id, Dirty::PAINT | Dirty::SEMANTICS, "placeholder", || placeholder.to_owned());
        true
    }

    /// Sets a text input's selection (`anchor`, `caret`: byte offsets into its value); the
    /// caret's goal x for moves by lines is forgotten.
    /// Explicit selection changes take precedence over a pending edit proposal. Stored as
    /// given so callers can update a value afterwards; reads clamp to the current value.
    pub fn set_selection(&mut self, id: NodeId, anchor: usize, caret: usize) {
        let sel = (anchor, caret);
        let Some(i) = self.node_mut(id).input_mut() else { return };
        i.goal.set(None);
        i.editing.selection_changed();
        if i.selection != sel {
            // A direct selection change ends the input method's draft without committing it.
            let retired = i.composition.take().is_some();
            if retired {
                i.clean.set(0);
            }
            i.selection = sel;
            if retired {
                self.renew_editor_a11y_runs(id);
            } else if !i.secure {
                self.a11y_touch(id);
            }
            self.mark(Dirty::PAINT);
        }
    }
    /// A text input's selection `(anchor, caret)`, within its current value.
    pub fn selection(&self, id: NodeId) -> (usize, usize) {
        let Some(i) = self.node(id).input() else { return (0, 0) };
        (floor_boundary(&i.value, i.selection.0), floor_boundary(&i.value, i.selection.1))
    }

    /// Makes a text input a secure field (a password): shown as bullets; no copy, cut, undo
    /// history, word motion or input method; bullets to assistive technology and every report.
    pub fn set_secure(&mut self, id: NodeId, on: bool) {
        let Some(i) = self.node_mut(id).input_mut() else { return };
        if i.secure != on {
            i.editing = super::editor::Editing::default();
            i.secure = on;
            i.clean.set(0);
            i.composition = None;
            self.renew_editor_a11y_runs(id);
            self.changed(id, Dirty::PAINT | Dirty::TEXT | Dirty::SEMANTICS, "secure", || on.to_string());
        }
    }
    /// Shows a secure field's text (a "show password" toggle); its other protections stay.
    pub fn set_revealed(&mut self, id: NodeId, on: bool) {
        let Some(i) = self.node_mut(id).input_mut() else { return };
        if i.revealed != on {
            i.revealed = on;
            i.clean.set(0);
            self.changed(id, Dirty::PAINT | Dirty::TEXT | Dirty::SEMANTICS, "revealed", || on.to_string());
        }
    }
    pub fn is_secure(&self, id: NodeId) -> bool {
        self.node(id).input().is_some_and(|i| i.secure)
    }
    /// The value as reports show it (inspect, checks, dumps): a secure field's as bullets.
    pub fn reported_value(&self, id: NodeId) -> String {
        match self.node(id).input() {
            Some(i) if i.secure => i.value.graphemes(true).map(|_| MASK).collect(),
            _ => self.value(id).to_owned(),
        }
    }
    /// Shows the grapheme typed at value offset `at` of a secure field for `REVEAL_LAST`
    /// (1.5) seconds (touch platforms); `None` hides it now.
    pub fn reveal_typed(&mut self, id: NodeId, at: Option<usize>) {
        let until = self.now + REVEAL_LAST;
        let Some(i) = self.node_mut(id).input_mut() else { return };
        let reveal = at.map(|b| (b, until));
        if i.reveal.get() != reveal {
            i.reveal.set(reveal);
            i.clean.set(0);
            self.revealing = reveal.map(|r| (id, r.1));
            self.mark(Dirty::PAINT);
        }
    }
    /// A secure field's shown character hid (its time came): it shows a bullet again.
    pub(crate) fn hide_typed(&mut self, id: NodeId) {
        if let Some(i) = self.node(id).input() {
            i.reveal.set(None);
            i.clean.set(0);
        }
        self.mark(Dirty::PAINT);
    }
    /// When a secure field's shown character hides (kernel time), if one shows.
    /// Whether the caret of field `id` shows now: not in a window that is not active (as the
    /// platform's fields); else always, but while it blinks, the first half of each second since
    /// it last moved.
    pub fn caret_shown(&self, id: NodeId) -> bool {
        if !self.window_active {
            return false;
        }
        match self.caret {
            Some((f, since)) if f == id => ((self.now - since) / CARET_BLINK).floor() as i64 % 2 == 0,
            _ => true,
        }
    }

    /// When the focused field's caret next shows or hides (none in a window that is not active).
    pub fn caret_deadline(&self) -> Option<f64> {
        let (_, since) = self.caret.filter(|(f, _)| self.window_active && self.is_alive(*f))?;
        Some(since + (((self.now - since) / CARET_BLINK).floor() + 1.0) * CARET_BLINK)
    }

    /// The window became active or stopped being: its focused field's caret shows (blinking
    /// from now) or hides.
    pub fn set_window_active(&mut self, on: bool) {
        if self.window_active == on {
            return;
        }
        self.window_active = on;
        if let Some((f, _)) = self.caret.filter(|(f, _)| self.is_alive(*f)) {
            // Through `changed`: the caret blinks from now.
            self.changed(f, Dirty::PAINT, "window active", || on.to_string());
        }
    }
    pub fn window_active(&self) -> bool {
        self.window_active
    }

    pub fn reveal_deadline(&self) -> Option<f64> {
        self.revealing.filter(|(id, _)| self.is_alive(*id)).map(|(_, t)| t).filter(|t| *t > self.now)
    }

    /// A multi-line text input's height in lines: it follows the text from `min` to `max`
    /// lines, and scrolls beyond.
    pub fn set_rows(&mut self, id: NodeId, min: u16, max: u16) {
        let Some(i) = self.node_mut(id).input_mut() else { return };
        let rows = (min.max(1), max.max(min.max(1)));
        if i.rows != rows {
            i.rows = rows;
            self.update_measure(id);
            self.changed(id, Dirty::LAYOUT, "rows", || format!("{rows:?}"));
        }
    }
    /// Enter submits and Shift+Enter breaks the line (a chat composer).
    pub fn set_submit_on_enter(&mut self, id: NodeId, on: bool) {
        let Some(i) = self.node_mut(id).input_mut() else { return };
        i.submit_on_enter = on;
    }

    /// Makes a text input multi-line (wrapping at its width, Enter breaking lines) or single-line.
    pub fn set_multiline(&mut self, id: NodeId, on: bool) {
        let Some(i) = self.node_mut(id).input_mut() else { return };
        if i.multiline != on {
            i.multiline = on;
            i.clean.set(0);
            self.renew_editor_a11y_runs(id);
            self.update_measure(id);
            self.changed(id, Dirty::LAYOUT | Dirty::SEMANTICS, "multiline", || on.to_string());
        }
    }
    pub fn is_multiline(&self, id: NodeId) -> bool {
        self.node(id).input().is_some_and(|i| i.multiline)
    }

    /// Lays out a text input (after the last layout).
    pub(crate) fn field(&self, id: NodeId) -> Option<Field> {
        let n = self.node(id);
        let input = n.input()?;
        let (anchor, caret) = self.selection(id);
        // A masked field shows a bullet per grapheme (the last typed one for a moment on touch
        // platforms) and no composition.
        let masked = input.masked().then(|| {
            let reveal = input.reveal.get().filter(|r| self.now < r.1).map(|r| r.0);
            let mut shown = String::new();
            let mut pairs = Vec::new();
            for (b, g) in input.value.grapheme_indices(true) {
                pairs.push((b, shown.len()));
                if reveal == Some(b) { shown.push_str(g) } else { shown.push(MASK) }
            }
            pairs.push((input.value.len(), shown.len()));
            (shown, pairs)
        });
        let composing = input.composition.is_some();
        let (shown, preedit, sel, shown_caret, show_caret, pairs) = match masked {
            Some((shown, pairs)) => {
                let to_shown = |b| pairs[pairs.partition_point(|x| x.0 <= b).saturating_sub(1)].1;
                let sel = (to_shown(anchor.min(caret)), to_shown(anchor.max(caret)));
                let caret = to_shown(caret);
                (shown, caret..caret, sel, caret, true, Some(pairs))
            }
            None if composing => {
                let c = input.composition.as_ref().expect("a composition");
                let selection = c.selection.clone().unwrap_or(c.marked.end..c.marked.end);
                (c.text.clone(), c.marked.clone(), (selection.start, selection.end), selection.end, c.selection.is_some(), None)
            }
            None => (input.value.clone(), caret..caret, (anchor.min(caret), anchor.max(caret)), caret, true, None),
        };
        let (overlay, replaced) = if composing {
            let mut prefix = input.value.bytes().zip(shown.bytes()).take_while(|(a, b)| a == b).count();
            while !input.value.is_char_boundary(prefix) || !shown.is_char_boundary(prefix) {
                prefix -= 1;
            }
            let limit = input.value.len().min(shown.len()) - prefix;
            let mut suffix = input.value.bytes().rev().zip(shown.bytes().rev()).take(limit).take_while(|(a, b)| a == b).count();
            while !input.value.is_char_boundary(input.value.len() - suffix) || !shown.is_char_boundary(shown.len() - suffix) {
                suffix -= 1;
            }
            (prefix..shown.len() - suffix, prefix..input.value.len() - suffix)
        } else {
            (caret..caret, caret..caret)
        };
        let layout = self.input_layout(id);
        let lines = if shown.is_empty() {
            Vec::new()
        } else {
            self.shown_layout(id, &shown, &layout.style).visual_lines(layout.multiline.then_some(layout.content.w))
        };
        let mut f = Field::new(shown, shown_caret, &layout, lines);
        f.preedit = preedit;
        f.overlay = overlay;
        f.replaced = replaced;
        f.sel = sel;
        f.show_caret = show_caret;
        f.masked = pairs;
        Some(f)
    }

    /// Settle internal editor scrolling after layout, including caret-only updates that did
    /// not need Taffy. Geometry reads never write these node-owned facts.
    pub(super) fn settle_fields(&mut self, relaid: bool) {
        let ids: Vec<_> = self.nodes.iter().filter(|(_, n)| n.input().is_some()).map(|(id, _)| id).collect();
        for id in ids {
            let Some(mut field) = self.field(id) else { continue };
            let a11y_layout = (relaid && !self.is_secure(id)).then(|| {
                let style = input_style(self.node(id).visual.text_style(), self.is_multiline(id));
                (self.input_content_box(id), style, self.fonts_generation)
            });
            let input = self.node_mut(id).input_mut().expect("an input");
            let (layout_changed, shape_changed) = a11y_layout.map_or((false, false), |(content, style, fonts)| {
                let previous = input.a11y_layout.replace((content, style, fonts));
                let shape_changed = previous.is_none_or(|(before, old_style, old_fonts)| {
                    style != old_style || fonts != old_fonts || (input.multiline && content.w != before.w)
                });
                (previous != a11y_layout, shape_changed)
            });
            let kept = field.reveal(input.multiline, input.scroll, input.kept);
            let scroll = if input.multiline { field.scroll.1 } else { field.scroll.0 };
            let scrolled = input.scroll != scroll;
            if scrolled || input.kept != kept {
                input.scroll = scroll;
                input.kept = kept;
                self.mark(Dirty::PAINT);
            }
            if shape_changed {
                self.renew_editor_a11y_runs(id);
            } else if layout_changed || scrolled {
                self.a11y_touch(id);
            }
        }
    }

    /// The text input's content box, in its own coordinates.
    pub(super) fn input_content_box(&self, id: NodeId) -> Rect {
        let l = self.taffy.layout(self.node(id).taffy).expect("taffy");
        Rect {
            x: l.border.left + l.padding.left,
            y: l.border.top + l.padding.top,
            w: (l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right).max(0.0),
            h: (l.size.height - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom).max(0.0),
        }
    }

    pub(crate) fn input_layout(&self, id: NodeId) -> TextInputLayout {
        let n = self.node(id);
        let input = n.input().expect("a text input");
        let (x, y, _, _) = self.shown_rect(id);
        let mut content = self.input_content_box(id);
        content.x += x;
        content.y += y;
        TextInputLayout {
            resources: self.res.clone(),
            style: input_style(n.visual.text_style(), input.multiline),
            content,
            multiline: input.multiline,
            scroll: input.scroll,
            kept: input.kept,
            fonts: self.fonts_generation,
        }
    }

    /// The value offset of a text field `field` shows at byte `b` (the composition is not part
    /// of the value).
    fn value_offset(field: &Field, b: usize) -> usize {
        if let Some(p) = &field.masked {
            return p[p.partition_point(|x| x.1 <= b).saturating_sub(1)].0;
        }
        if (field.overlay.is_empty() && field.replaced.is_empty()) || b <= field.overlay.start {
            b
        } else if b < field.overlay.end {
            field.replaced.start
        } else {
            b.saturating_sub(field.overlay.len()) + field.replaced.len()
        }
    }

    /// The value offset nearest window point `(x, y)` in a text input (where a click puts the
    /// caret); with `under`, where the character under it starts (what a double click selects
    /// around).
    pub fn input_offset_at(&self, id: NodeId, x: f32, y: f32, under: bool) -> Option<usize> {
        let f = self.field(id)?;
        let b = f.byte_at(x - f.x + f.scroll.0, y - f.y + f.scroll.1, under);
        Some(Self::value_offset(&f, b))
    }

    /// The value offset `lines` visual lines from the caret of a text input and the goal x to
    /// keep: the caret's x when the moves by lines began.
    pub fn input_offset_by_lines(&self, id: NodeId, lines: i32) -> Option<(usize, f32)> {
        let f = self.field(id)?;
        let goal = self.goal(id).unwrap_or_else(|| f.pos(f.caret).0);
        Some((Self::value_offset(&f, f.by_lines(f.caret, goal, lines)), goal))
    }

    /// Keeps `goal` as the caret's x for the next move by lines (any other selection change
    /// forgets it).
    pub fn keep_goal(&mut self, id: NodeId, goal: f32) {
        if let Some(i) = self.node(id).input() {
            i.goal.set(Some(goal));
        }
    }
    pub fn goal(&self, id: NodeId) -> Option<f32> {
        self.node(id).input().and_then(|i| i.goal.get())
    }

    /// The first and last value offsets of the visual line the caret of a text input is on.
    pub fn input_line_edges(&self, id: NodeId) -> Option<(usize, usize)> {
        let f = self.field(id)?;
        let (s, e) = f.line_edges(f.caret);
        Some((Self::value_offset(&f, s), Self::value_offset(&f, e)))
    }

    /// How many lines of a text input show at once (at least 1).
    pub fn input_page_lines(&self, id: NodeId) -> usize {
        self.field(id).map_or(1, |f| ((f.height / f.line_height.max(1.0)).floor() as usize).max(1))
    }

    /// Where the caret of a text input is (logical pixels). Input methods place their
    /// candidate window next to it.
    pub fn caret(&self, id: NodeId) -> Option<Rect> {
        let f = self.field(id)?;
        let (cx, cy) = f.pos(f.caret);
        Some(Rect { x: f.x + cx - f.scroll.0, y: f.y + cy - f.scroll.1, w: 1.0, h: f.line_height })
    }

    /// Paints a text input: selection, text, composition underline and caret, clipped to its
    /// content box and scrolled to keep the caret in view.
    pub(crate) fn paint_input(&self, id: NodeId, opacity: f32, out: &mut Vec<Item>) {
        let Some(f) = self.field(id) else { return };
        let n = self.node(id);
        let shown = self.presented(id);
        let v = &shown;
        let fade = |c: Color| if opacity >= 1.0 { c } else { c.multiply_alpha(opacity) };
        let lh = f.line_height;
        let (ox, oy) = (f.x - f.scroll.0, f.y - f.scroll.1);
        let clip = Rect { x: f.x, y: f.y, w: f.width, h: f.height.max(lh) };
        out.push(Item::PushClip { source: id, rect: clip, radius: 0.0 });
        let quad = |r: Rect, fill: Color| {
            Item::Quad(Quad {
                source: id,
                rect: Rect { x: r.x + ox, y: r.y + oy, ..r },
                radius: 0.0,
                fill: Some(fill),
                gradient: None,
                border: [0.0; 4],
                border_color: fill,
            })
        };
        let focused = n.state.contains(State::FOCUSED);
        if f.sel.0 != f.sel.1 {
            let accent = self.res.theme.borrow().get(Token::Accent);
            let fill = if focused { accent.multiply_alpha(0.3) } else { accent.multiply_alpha(0.15) };
            for r in f.rects(f.sel.0, f.sel.1) {
                out.push(quad(r, fade(fill)));
            }
        }
        let placeholder = n.input().filter(|i| i.value.is_empty() && i.composition.is_none()).and_then(|_| self.placeholder(id));
        if !f.shown.is_empty() || placeholder.is_some() {
            let multiline = self.is_multiline(id);
            let style = input_style(v.text_style(), multiline);
            let (runs, color) = if let Some(placeholder) = placeholder {
                // Shape hints separately: they must not displace the editable paragraph cache
                // or change native query, hit-test, selection and caret geometry.
                let mut engine = self.res.text.borrow_mut();
                let layout = engine.layout(placeholder, &style, &[], None, 0);
                let ellipsis = (!multiline).then(|| engine.ellipsis(&style)).flatten();
                let runs = layout
                    .present(multiline.then_some(f.width), (!multiline).then_some(1), (!multiline).then_some(f.width), ellipsis.as_ref())
                    .glyphs;
                (runs, self.res.theme.borrow().get(Token::TextMuted))
            } else {
                (self.shown_layout(id, &f.shown, &style).present(multiline.then_some(f.width), None, None, None).glyphs, v.text)
            };
            for r in runs {
                let glyphs = r.glyphs.into_iter().map(|g| crate::paint::Glyph { id: g.id, x: g.x, y: g.y }).collect();
                self.glyph_runs_built.set(self.glyph_runs_built.get() + 1);
                out.push(Item::Glyphs(GlyphRun {
                    source: id,
                    font: r.font,
                    size: r.size,
                    coords: r.coords,
                    embolden: r.embolden,
                    color: fade(color),
                    origin: (ox, oy),
                    glyphs,
                }));
            }
        }
        if !f.preedit.is_empty() {
            for r in f.rects(f.preedit.start, f.preedit.end) {
                out.push(quad(Rect { y: r.y + lh - 2.0, h: 1.0, ..r }, fade(v.text)));
            }
        }
        if focused && f.show_caret && f.sel.0 == f.sel.1 && self.caret_shown(id) {
            let (cx, cy) = f.pos(f.caret);
            // A single line's caret stays inside the box (its last position may be past it).
            let x = if self.is_multiline(id) { cx } else { (cx - f.scroll.0).min(f.width - 1.0).max(0.0) + f.scroll.0 };
            out.push(quad(Rect { x, y: cy, w: 1.0, h: lh }, fade(v.text)));
        }
        out.push(Item::PopClip);
    }
}

impl TextInputLayout {
    /// Lays out the current native document using this frame's field configuration. Returns
    /// none if shaping finds the shared text engine borrowed; no window, kernel or
    /// application handlers are entered. An empty document still has an insertion-point line.
    pub fn layout(&self, document: &TextDocument) -> Option<TextGeometry> {
        if !self.resources.text.is_available() {
            return None;
        }
        let shown = document.text();
        let caret = document.composition.as_ref().map_or(document.caret, |c| c.selection.as_ref().map_or(c.marked.end, |s| s.end));
        let lines = if shown.is_empty() {
            Vec::new()
        } else {
            let mut engine = self.resources.text.try_borrow_mut()?;
            engine.layout(shown, &self.style, &[], None, 0).visual_lines(self.multiline.then_some(self.content.w))
        };
        let mut field = Field::new(shown.to_owned(), caret, self, lines);
        field.reveal(self.multiline, self.scroll, self.kept);
        Some(field.into_geometry())
    }
}

/// How a text input's text is laid out: one line, or wrapped at the field's width.
pub(crate) fn input_style(base: super::TextStyle, multiline: bool) -> super::TextStyle {
    super::TextStyle { wrap: if multiline { Wrap::Words } else { Wrap::None }, max_lines: None, ..base }
}

impl Kernel {
    /// The layout of what field `id` shows (`shown`, in `style`): kept while they stay the same,
    /// made from the last one otherwise (its unchanged paragraphs reused).
    pub(crate) fn shown_layout(&self, id: NodeId, shown: &str, style: &text::TextStyle) -> text::TextLayout {
        let input = self.node(id).input().expect("a text input");
        let mut kept = input.layout.borrow_mut();
        if let Some((s, st, fonts, l)) = kept.as_ref()
            && s == shown
            && st == style
            && *fonts == self.fonts_generation
        {
            return l.clone();
        }
        let clean = input.clean.replace(shown.len());
        let layout = self.res.text.borrow_mut().layout(shown, style, &[], kept.as_ref().map(|k| &k.3), clean);
        *kept = Some((shown.to_owned(), *style, self.fonts_generation, layout.clone()));
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{Length, StyleProp};

    fn editor(multiline: bool) -> (Kernel, NodeId) {
        let mut k = Kernel::default();
        *k.res.text.borrow_mut() = Box::new(text::MonoEngine);
        let id = k.create_input();
        k.append_child(k.root(), id);
        k.set_style(id, StyleProp::Width(Length::Px(80.0)));
        k.set_style(id, StyleProp::Height(Length::Px(40.0)));
        k.set_multiline(id, multiline);
        k.set_value(id, "中文 abc def\n中文 ghi jkl\n中文 mno pqr\n中文 stu xyz");
        k.layout(400.0, 300.0);
        (k, id)
    }

    fn observe(k: &Kernel, id: NodeId, reverse: bool) -> TextGeometry {
        if reverse {
            k.accessibility_tree(Some(id), 1.0);
            k.display_list();
        }
        let caret = k.caret(id).unwrap();
        k.input_offset_at(id, caret.x, caret.y, false);
        let geometry = k.field(id).unwrap().into_geometry();
        if !reverse {
            k.display_list();
            k.accessibility_tree(Some(id), 1.0);
        }
        geometry
    }

    #[test]
    fn editor_geometry_reads_do_not_settle_pending_caret_scroll() {
        for multiline in [false, true] {
            let (mut k, id) = editor(multiline);
            let before = k.input_layout(id);
            let end = k.value(id).len();
            k.set_selection(id, end, end);
            let generation = k.mutation_generation();
            assert_eq!(observe(&k, id, false), observe(&k, id, true));
            assert_eq!(k.input_layout(id), before, "even pending reads cannot change scroll or keep state");
            assert_eq!(k.mutation_generation(), generation);
            assert!(!k.layout(400.0, 300.0), "caret-only settlement does not run Taffy");
            assert!(k.input_layout(id).scroll > 0.0, "settlement reveals the caret");
            let settled = k.input_layout(id);
            let geometry = observe(&k, id, true);
            assert_eq!(geometry, observe(&k, id, false));
            assert_eq!(settled.layout(&k.text_document(id).unwrap()), Some(geometry));
            assert_eq!(k.input_layout(id), settled);
        }
    }

    #[test]
    fn manual_editor_scroll_survives_observation_until_editing_moves_the_caret() {
        let (mut k, id) = editor(true);
        let end = k.value(id).len();
        k.set_selection(id, end, end);
        k.layout(400.0, 300.0);
        assert!(k.scroll_offset(id).1 > 0.0);
        k.scroll_to(id, 0.0, 0.0);
        let before = k.input_layout(id);
        observe(&k, id, false);
        k.layout(400.0, 300.0);
        observe(&k, id, true);
        assert_eq!(k.input_layout(id), before);
        k.set_selection(id, end - 1, end - 1);
        k.layout(400.0, 300.0);
        assert!(k.scroll_offset(id).1 > 0.0);
        k.set_style(id, StyleProp::Height(Length::Px(300.0)));
        k.layout(400.0, 400.0);
        assert_eq!(k.scroll_offset(id).1, 0.0, "resize clamps before any geometry query");
        k.set_style(id, StyleProp::Height(Length::Px(40.0)));
        k.set_selection(id, end, end);
        assert!(!k.scroll_to(id, 0.0, 0.0), "a manual no-op still acknowledges the new caret");
        k.layout(400.0, 300.0);
        assert_eq!(k.scroll_offset(id).1, 0.0);
    }

    #[test]
    fn native_prediction_is_owned_and_busy_shaping_refuses_without_changing_scroll() {
        let (k, id) = editor(true);
        let published = k.input_layout(id);
        let original = k.text_document(id).unwrap();
        let mut draft = original.clone();
        let end = original.text().len();
        draft.apply(&octara_primitives::text::TextEdit::Mark {
            text: "候选中文\n更多候选中文".into(),
            selection: Some(25..25),
            replacement: Some(end..end),
        });
        {
            let _busy = k.res.text.borrow_mut();
            assert!(published.layout(&draft).is_none());
        }
        let predicted = published.layout(&draft).unwrap();
        assert!(predicted.origin.1 < published.content.y);
        assert_eq!(k.input_layout(id), published);
        assert_eq!(k.text_document(id), Some(original));
    }
}
