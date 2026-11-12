//! Text editing in text inputs, owned by the runtime: the app keeps writing `value` and
//! handling `on_input(new)`; the selection, the composition and the undo history live here, per
//! field. Carets move by grapheme clusters and Unicode words.
//!
//! Platform text protocols (NSTextInputClient, UITextInput, InputConnection, TSF,
//! text-input-v3) map onto [`App::text_field`], [`App::replace_text`], [`App::set_selection`]
//! and [`App::set_composition`]; ranges are byte offsets, with [`utf16_offset`] and
//! [`byte_offset`] for the protocols that count UTF-16 units.

use super::behavior::Behavior;
use super::pointer::{Modifiers, PointerEvent, PointerKind};
use super::{App, Modality};
use crate::kernel::{HandlerKind, NodeId};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// The most undo steps a field keeps (Rune).
const UNDO_LIMIT: usize = 128;

/// Where the system clipboard is (the shell sets the platform's; tests keep this one).
pub trait Clipboard {
    fn read_text(&mut self) -> Option<String>;
    fn write_text(&mut self, text: &str);
}

/// A clipboard in memory.
#[derive(Default)]
pub struct MemoryClipboard(pub Option<String>);

impl Clipboard for MemoryClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.0.clone()
    }
    fn write_text(&mut self, text: &str) {
        self.0 = Some(text.to_owned());
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Snapshot {
    value: String,
    anchor: usize,
    caret: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Insert,
    Delete,
    Other,
}

/// A field's editing history.
#[derive(Default, Debug)]
pub(crate) struct EditState {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The last edit, for merging a run of typing into one step.
    last: Option<(Kind, Snapshot)>,
}

/// The focused text field, as platform text protocols see it.
#[derive(Clone, PartialEq, Debug)]
pub struct TextField {
    pub text: String,
    /// The selection (bytes, start ≤ end) and whether the caret is at its start.
    pub selection: Range<usize>,
    pub reversed: bool,
    /// What an input method is composing at the caret (not part of `text`).
    pub composition: String,
    /// The caret `(x, y, w, h)` in logical pixels, for the input method's candidate window.
    pub caret: (f32, f32, f32, f32),
}

/// The UTF-16 offset of byte `b` of `s`.
pub fn utf16_offset(s: &str, b: usize) -> usize {
    s[..crate::kernel::floor_boundary(s, b)].encode_utf16().count()
}

/// The byte offset of UTF-16 offset `u` of `s` (clamped; inside a surrogate pair, its start).
pub fn byte_offset(s: &str, u: usize) -> usize {
    let mut units = 0;
    for (b, c) in s.char_indices() {
        if units + c.len_utf16() > u {
            return b;
        }
        units += c.len_utf16();
    }
    s.len()
}

fn prev_grapheme(s: &str, i: usize) -> usize {
    s[..i].grapheme_indices(true).next_back().map_or(0, |(b, _)| b)
}
fn next_grapheme(s: &str, i: usize) -> usize {
    s[i..].graphemes(true).next().map_or(s.len(), |g| i + g.len())
}
fn is_word(w: &str) -> bool {
    w.chars().any(char::is_alphanumeric)
}
/// The start of the word before `i` (skipping spaces and punctuation).
fn prev_word(s: &str, i: usize) -> usize {
    s.split_word_bound_indices().take_while(|(b, _)| *b < i).filter(|(_, w)| is_word(w)).last().map_or(0, |(b, _)| b)
}
/// The end of the word after `i`.
fn next_word(s: &str, i: usize) -> usize {
    s.split_word_bound_indices().map(|(b, w)| (b + w.len(), w)).find(|(e, w)| *e > i && is_word(w)).map_or(s.len(), |(e, _)| e)
}
/// The word (or run of spaces, or punctuation) around `i`.
fn word_at(s: &str, i: usize) -> (usize, usize) {
    let mut last = (s.len(), s.len());
    for (b, w) in s.split_word_bound_indices() {
        let e = b + w.len();
        if i < e {
            return (b, e);
        }
        last = (b, e);
    }
    last
}

/// A text field's pointer behavior: a press places the caret (2 clicks select a word, 3
/// everything, Shift extends); a mouse or pen drag selects (a finger's scrolls).
pub(super) struct Field;
pub(super) static FIELD: Field = Field;

impl Behavior for Field {
    fn down(&self, app: &App, n: NodeId, ev: &PointerEvent, clicks: u32) {
        app.press_in_field(n, (ev.x, ev.y), clicks, ev.modifiers.contains(Modifiers::SHIFT));
    }
    fn drags(&self, kind: PointerKind) -> bool {
        kind != PointerKind::Touch
    }
    fn drag(&self, app: &App, n: NodeId, at: (f32, f32)) {
        app.drag_in_field(n, at);
    }
}

impl App {
    /// The focused text field's value and selection `(anchor, caret)`.
    fn field(&self) -> Option<(NodeId, String, usize, usize)> {
        let (n, value) = self.focused_input()?;
        let (a, c) = self.ui.kernel.borrow().selection(n);
        Some((n, value, a, c))
    }

    /// Writes an edit: the selection now, the new value through the field's `on_input`, and an
    /// undo step (a run of typing or deleting is one step).
    fn apply(&self, n: NodeId, value: String, anchor: usize, caret: usize, kind: Kind) {
        let (old, a0, c0) = {
            let k = self.ui.kernel.borrow();
            let (a, c) = k.selection(n);
            (k.value(n).to_owned(), a, c)
        };
        if value != old {
            let mut i = self.interaction.borrow_mut();
            let st = i.edits.entry(n).or_default();
            let before = Snapshot { value: old.clone(), anchor: a0, caret: c0 };
            // Typing on from where the last edit of the same kind left off continues its step.
            let continues = st.last.as_ref().is_some_and(|(k, s)| *k == kind && kind != Kind::Other && *s == before);
            if !continues {
                st.undo.push(before);
                if st.undo.len() > UNDO_LIMIT {
                    st.undo.remove(0);
                }
            }
            st.redo.clear();
            st.last = Some((kind, Snapshot { value: value.clone(), anchor, caret }));
        }
        // Typing is direct manipulation: what it changes lands at once.
        self.ui.kernel.borrow_mut().set_instant(true);
        self.ui.kernel.borrow_mut().set_selection(n, anchor, caret);
        if value != old {
            self.run(n, &[HandlerKind::Input], Some(value));
        }
    }

    /// Moves the caret (extending the selection with `extend`).
    fn select(&self, n: NodeId, anchor: usize, caret: usize) {
        self.ui.kernel.borrow_mut().set_selection(n, anchor, caret);
        if let Some(st) = self.interaction.borrow_mut().edits.get_mut(&n) {
            st.last = None;
        }
    }

    /// Text typed or committed into the focused field: replaces the selection.
    pub(super) fn insert_text(&self, text: &str) {
        let Some((n, value, a, c)) = self.field() else { return };
        self.ui.kernel.borrow_mut().set_preedit(n, "");
        let (s, e) = (a.min(c), a.max(c));
        let new = format!("{}{text}{}", &value[..s], &value[e..]);
        let at = s + text.len();
        self.apply(n, new, at, at, Kind::Insert);
    }

    /// Moves the caret `lines` visual lines (extending the selection with `extend`), keeping
    /// its x from where the moves by lines began.
    fn by_lines(&self, n: NodeId, anchor: usize, lines: i32, extend: bool) {
        let Some((to, goal)) = self.ui.kernel.borrow().input_offset_by_lines(n, lines) else { return };
        self.select(n, if extend { anchor } else { to }, to);
        self.ui.kernel.borrow_mut().keep_goal(n, goal);
    }

    /// Runs a text-editing command on the focused field; `false` if there is none.
    pub(super) fn edit_command(&self, command: &str) -> bool {
        let Some((n, v, a, c)) = self.field() else { return false };
        let (s, e) = (a.min(c), a.max(c));
        let collapsed = s == e;
        let multiline = self.ui.kernel.borrow().is_multiline(n);
        let page = self.ui.kernel.borrow().input_page_lines(n) as i32;
        let edges = || self.ui.kernel.borrow().input_line_edges(n).unwrap_or((0, v.len()));
        match command {
            "edit.move_up" => self.by_lines(n, a, -1, false),
            "edit.move_down" => self.by_lines(n, a, 1, false),
            "edit.select_up" => self.by_lines(n, a, -1, true),
            "edit.select_down" => self.by_lines(n, a, 1, true),
            "edit.move_page_up" => self.by_lines(n, a, -page, false),
            "edit.move_page_down" => self.by_lines(n, a, page, false),
            "edit.select_page_up" => self.by_lines(n, a, -page, true),
            "edit.select_page_down" => self.by_lines(n, a, page, true),
            "edit.move_to_line_start" => {
                let to = edges().0;
                self.select(n, to, to)
            }
            "edit.move_to_line_end" => {
                let to = edges().1;
                self.select(n, to, to)
            }
            "edit.select_to_line_start" => self.select(n, a, edges().0),
            "edit.select_to_line_end" => self.select(n, a, edges().1),
            "edit.newline" => {
                if !multiline {
                    return false;
                }
                let new = format!("{}\n{}", &v[..s], &v[e..]);
                self.apply(n, new, s + 1, s + 1, Kind::Insert);
            }
            "edit.move_left" => {
                self.select(n, if collapsed { prev_grapheme(&v, c) } else { s }, if collapsed { prev_grapheme(&v, c) } else { s })
            }
            "edit.move_right" => {
                self.select(n, if collapsed { next_grapheme(&v, c) } else { e }, if collapsed { next_grapheme(&v, c) } else { e })
            }
            "edit.move_word_left" => {
                let to = prev_word(&v, c);
                self.select(n, to, to)
            }
            "edit.move_word_right" => {
                let to = next_word(&v, c);
                self.select(n, to, to)
            }
            "edit.move_to_start" => self.select(n, 0, 0),
            "edit.move_to_end" => self.select(n, v.len(), v.len()),
            "edit.select_left" => self.select(n, a, prev_grapheme(&v, c)),
            "edit.select_right" => self.select(n, a, next_grapheme(&v, c)),
            "edit.select_word_left" => self.select(n, a, prev_word(&v, c)),
            "edit.select_word_right" => self.select(n, a, next_word(&v, c)),
            "edit.select_to_start" => self.select(n, a, 0),
            "edit.select_to_end" => self.select(n, a, v.len()),
            "edit.select_all" => self.select(n, 0, v.len()),
            "edit.delete_backward"
            | "edit.delete_forward"
            | "edit.delete_word_backward"
            | "edit.delete_word_forward"
            | "edit.delete_to_start"
            | "edit.delete_to_line_start" => {
                let (from, to) = if !collapsed {
                    (s, e)
                } else {
                    match command {
                        "edit.delete_backward" => (prev_grapheme(&v, c), c),
                        "edit.delete_forward" => (c, next_grapheme(&v, c)),
                        "edit.delete_word_backward" => (prev_word(&v, c), c),
                        "edit.delete_word_forward" => (c, next_word(&v, c)),
                        "edit.delete_to_line_start" => (edges().0.min(c), c),
                        _ => (0, c),
                    }
                };
                if from != to {
                    self.apply(n, format!("{}{}", &v[..from], &v[to..]), from, from, Kind::Delete);
                }
            }
            "edit.copy" | "edit.cut" => {
                if collapsed {
                    return true;
                }
                self.clipboard.borrow_mut().write_text(&v[s..e]);
                if command == "edit.cut" {
                    self.apply(n, format!("{}{}", &v[..s], &v[e..]), s, s, Kind::Other);
                }
            }
            "edit.paste" => {
                let text = self.clipboard.borrow_mut().read_text();
                if let Some(t) = text {
                    // A multi-line field keeps line breaks; one line turns them into spaces.
                    let t = if multiline { t.replace("\r\n", "\n").replace('\r', "\n") } else { t.replace("\r\n", " ").replace(['\n', '\r'], " ") };
                    let new = format!("{}{t}{}", &v[..s], &v[e..]);
                    self.apply(n, new, s + t.len(), s + t.len(), Kind::Other);
                }
            }
            "edit.undo" | "edit.redo" => {
                let undo = command == "edit.undo";
                let step = {
                    let mut i = self.interaction.borrow_mut();
                    let st = i.edits.entry(n).or_default();
                    // The app changed the value itself since: the history no longer applies.
                    if st.last.as_ref().is_some_and(|(_, s)| s.value != v) {
                        *st = EditState::default();
                    }
                    let now = Snapshot { value: v.clone(), anchor: a, caret: c };
                    let step = if undo { st.undo.pop() } else { st.redo.pop() };
                    if step.is_some() {
                        if undo { st.redo.push(now) } else { st.undo.push(now) }
                    }
                    st.last = step.clone().map(|s| (Kind::Other, s));
                    step
                };
                if let Some(s) = step {
                    self.ui.kernel.borrow_mut().set_selection(n, s.anchor, s.caret);
                    self.run(n, &[HandlerKind::Input], Some(s.value));
                }
            }
            _ => return false,
        }
        true
    }

    /// A primary press at window point `at` in text field `n`: `clicks` 1 places the caret (or
    /// extends the selection with Shift), 2 selects a word, 3 selects everything.
    fn press_in_field(&self, n: NodeId, (x, y): (f32, f32), clicks: u32, extend: bool) {
        let Some(at) = self.ui.kernel.borrow().input_offset_at(n, x, y, clicks == 2) else { return };
        let v = self.ui.kernel.borrow().value(n).to_owned();
        let (a, _) = self.ui.kernel.borrow().selection(n);
        match clicks {
            1 if extend => self.select(n, a, at),
            1 => self.select(n, at, at),
            2 => {
                let (s, e) = word_at(&v, at);
                self.select(n, s, e)
            }
            _ => self.select(n, 0, v.len()),
        }
    }

    /// A drag in text field `n` to window point `(x, y)`: the caret follows, the anchor stays.
    fn drag_in_field(&self, n: NodeId, (x, y): (f32, f32)) {
        let Some(at) = self.ui.kernel.borrow().input_offset_at(n, x, y, false) else { return };
        let (a, _) = self.ui.kernel.borrow().selection(n);
        self.select(n, a, at);
    }

    /// Replaces the clipboard (shells install the system's).
    pub fn set_clipboard(&mut self, clipboard: Box<dyn Clipboard>) {
        *self.clipboard.borrow_mut() = clipboard;
    }
    /// What the clipboard holds as text.
    pub fn clipboard_text(&self) -> Option<String> {
        self.clipboard.borrow_mut().read_text()
    }

    /// The focused text field, for platform text protocols.
    pub fn text_field(&self) -> Option<TextField> {
        let (n, text, a, c) = self.field()?;
        let k = self.ui.kernel.borrow();
        let r = k.caret(n)?;
        Some(TextField {
            selection: a.min(c)..a.max(c),
            reversed: c < a,
            composition: k.preedit(n).to_owned(),
            caret: (r.x, r.y, r.w, r.h),
            text,
        })
    }

    /// Replaces `range` (bytes) of the focused field's text with `text`, ending any
    /// composition; the caret goes after it. (NSTextInputClient `insertText:replacementRange:`.)
    pub fn replace_text(&mut self, range: Range<usize>, text: &str) {
        self.input_frame(|| {
            let Some((n, v, _, _)) = self.field() else { return };
            let s = crate::kernel::floor_boundary(&v, range.start);
            let e = crate::kernel::floor_boundary(&v, range.end.max(s));
            self.ui.kernel.borrow_mut().set_preedit(n, "");
            let at = s + text.len();
            self.apply(n, format!("{}{text}{}", &v[..s], &v[e..]), at, at, Kind::Insert);
        });
    }

    /// Selects `anchor..caret` (bytes) in the focused field (the platform moved the selection).
    pub fn set_selection(&mut self, anchor: usize, caret: usize) {
        self.input_frame(|| {
            if let Some((n, ..)) = self.field() {
                self.select(n, anchor, caret);
            }
        });
    }

    /// What an input method is composing in the focused field (pinyin before it becomes 汉字):
    /// shown underlined at the caret, not yet part of the value; it replaces the selection.
    /// Empty ends it. [`App::type_text`] commits.
    pub fn set_composition(&mut self, text: &str) {
        self.input_frame(|| {
            let Some((n, v, a, c)) = self.field() else { return };
            if !text.is_empty() && a != c {
                // Composing over a selection replaces it.
                let (s, e) = (a.min(c), a.max(c));
                self.apply(n, format!("{}{}", &v[..s], &v[e..]), s, s, Kind::Other);
            }
            self.ui.kernel.borrow_mut().set_preedit(n, text);
        });
    }

    /// A value set by assistive technology (or an agent) for `n`: through its `on_input`, with
    /// the caret at the end.
    pub(super) fn set_value_of(&self, n: NodeId, value: String) -> bool {
        self.set_modality(Modality::Keyboard);
        let len = value.len();
        self.ui.kernel.borrow_mut().set_selection(n, len, len);
        self.run(n, &[HandlerKind::Input], Some(value))
    }
}
