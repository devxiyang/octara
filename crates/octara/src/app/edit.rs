//! Text input intents and application callbacks. The app writes `value` and handles
//! `on_input(new)`; the Kernel's input node owns selection, composition and accepted history.
//! Carets move by grapheme clusters and Unicode words.
//!
//! TargetPlatform text protocols (NSTextInputClient, UITextInput, InputConnection, TSF,
//! text-input-v3) map onto [`App::text_field`], [`App::replace_text`], [`App::set_selection`]
//! and `InputEvent::Composition`; ranges are byte offsets, with [`utf16_offset`] and
//! [`byte_offset`] for the protocols that count UTF-16 units.

use super::{Modality, TextDocument, TextEdit, Window};
use crate::diagnostics as codes;
use crate::kernel::{EditKind as Kind, HandlerKind, NodeId};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// The focused text field, as platform text protocols see it.
#[derive(Clone, PartialEq, Debug)]
pub struct TextField {
    pub text: String,
    /// The selection (bytes, start ≤ end) and whether the caret is at its start.
    pub selection: Range<usize>,
    pub reversed: bool,
    /// The input method's marked text (not part of the committed `text`).
    pub composition: String,
    /// UTF-8 byte range within `composition`; none means the input method hides its cursor.
    /// The range is clamped to character boundaries and ordered from start to end.
    pub composition_selection: Option<Range<usize>>,
    /// The caret `(x, y, w, h)` in logical pixels, for the input method's candidate window.
    pub caret: (f32, f32, f32, f32),
    /// Whether the framework currently paints the caret (including blink, selection and IME).
    pub caret_visible: bool,
    /// A secure field (a password): shells turn the input method off and the platform's secure
    /// input on while it is focused.
    pub secure: bool,
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

impl Window {
    /// The focused text field's value and selection `(anchor, caret)`.
    fn field(&self) -> Option<(NodeId, String, usize, usize)> {
        let (n, value) = self.focused_input()?;
        let (a, c) = self.w.kernel.borrow().selection(n);
        Some((n, value, a, c))
    }

    /// Propose an edit; selection and history are finalized only after bindings settle.
    fn apply(&self, n: NodeId, value: String, anchor: usize, caret: usize, kind: Kind) -> bool {
        if self.w.kernel.borrow().edit_pending(n) {
            self.app.rt.diag(
                &codes::EDIT_PENDING,
                "the field already has an unresolved edit proposal".into(),
                std::panic::Location::caller(),
            );
            return false;
        }
        let notify = self.w.kernel.borrow_mut().begin_edit(n, &value, (anchor, caret), kind);
        let Some(notify) = notify else { return false };
        let _abandon = std::mem::DropGuard::new(self, |window| {
            if std::thread::panicking() {
                window.w.kernel.borrow_mut().cancel_edit(n);
            }
        });
        self.w.kernel.borrow_mut().set_instant(true);
        self.w.interaction.borrow_mut().reveal = Some(n);
        if notify {
            self.run(n, &[HandlerKind::Input], Some(value));
        }
        self.w.kernel.borrow_mut().finish_edit_callback(n);
        true
    }

    /// Moves the caret (extending the selection with `extend`).
    fn select_field(&self, n: NodeId, anchor: usize, caret: usize) {
        self.w.kernel.borrow_mut().set_selection(n, anchor, caret);
        self.w.interaction.borrow_mut().reveal = Some(n);
    }

    /// Text typed or committed into the focused field: replaces the selection.
    pub(super) fn insert_text(&self, text: &str) {
        let Some((n, value, a, c)) = self.field() else { return };
        let document = self.w.kernel.borrow().text_document(n);
        if let Some(mut document) = document.filter(|d| d.composition.is_some()) {
            document.apply(&TextEdit::Insert { text: text.to_owned(), replacement: None });
            self.apply(n, document.value, document.anchor, document.caret, Kind::Other);
            return;
        }
        let (s, e) = (a.min(c), a.max(c));
        let new = format!("{}{text}{}", &value[..s], &value[e..]);
        let at = s + text.len();
        // On touch platforms a secure field shows what was just typed, for a moment.
        let reveal = {
            let k = self.w.kernel.borrow();
            (k.is_secure(n) && k.touch()).then(|| new[..at].grapheme_indices(true).next_back().map(|(b, _)| b)).flatten()
        };
        self.apply(n, new, at, at, Kind::Insert);
        let secure = {
            let k = self.w.kernel.borrow();
            k.is_alive(n) && k.is_secure(n)
        };
        if secure {
            self.w.kernel.borrow_mut().reveal_typed(n, reveal);
        }
    }

    /// Moves the caret `lines` visual lines (extending the selection with `extend`), keeping
    /// its x from where the moves by lines began.
    fn by_lines(&self, n: NodeId, anchor: usize, lines: i32, extend: bool) {
        let Some((to, goal)) = self.w.kernel.borrow().input_offset_by_lines(n, lines) else { return };
        self.select_field(n, if extend { anchor } else { to }, to);
        self.w.kernel.borrow_mut().keep_goal(n, goal);
    }

    /// Whether text-editing `command` does something in the focused field now (`None`: no field
    /// has focus), as a menu item for it shows: copy and cut a selection (not of a secure
    /// field), undo and redo a history, select all some text.
    pub(super) fn edit_enabled(&self, command: &str) -> Option<bool> {
        let (n, v, a, c) = self.field()?;
        let k = self.w.kernel.borrow();
        let secure = k.is_secure(n);
        Some(match command {
            "edit.copy" | "edit.cut" => a != c && !secure,
            "edit.undo" | "edit.redo" => k.history_available(n, command == "edit.undo"),
            "edit.select_all" => !v.is_empty(),
            "edit.newline" => k.is_multiline(n),
            _ => true,
        })
    }

    /// Runs a text-editing command on the focused field; `false` if there is none.
    pub(super) fn edit_command(&self, command: &str) -> bool {
        let Some((n, v, a, c)) = self.field() else { return false };
        let (s, e) = (a.min(c), a.max(c));
        let collapsed = s == e;
        let multiline = self.w.kernel.borrow().is_multiline(n);
        let secure = self.w.kernel.borrow().is_secure(n);
        let page = self.w.kernel.borrow().input_page_lines(n) as i32;
        let edges = || self.w.kernel.borrow().input_line_edges(n).unwrap_or((0, v.len()));
        // A secure field: words are not revealed (word moves go to the ends), and nothing is
        // copied out or kept in a history.
        type Word = fn(&str, usize) -> usize;
        let (prev_word, next_word): (Word, Word) = if secure { (|_, _| 0, |s, _| s.len()) } else { (prev_word, next_word) };
        if secure && matches!(command, "edit.copy" | "edit.cut" | "edit.undo" | "edit.redo") {
            return true;
        }
        if secure && command.starts_with("edit.") {
            self.w.kernel.borrow_mut().reveal_typed(n, None);
        }
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
                self.select_field(n, to, to)
            }
            "edit.move_to_line_end" => {
                let to = edges().1;
                self.select_field(n, to, to)
            }
            "edit.select_to_line_start" => self.select_field(n, a, edges().0),
            "edit.select_to_line_end" => self.select_field(n, a, edges().1),
            "edit.newline" => {
                if !multiline {
                    return false;
                }
                let new = format!("{}\n{}", &v[..s], &v[e..]);
                self.apply(n, new, s + 1, s + 1, Kind::Insert);
            }
            "edit.move_left" => {
                self.select_field(n, if collapsed { prev_grapheme(&v, c) } else { s }, if collapsed { prev_grapheme(&v, c) } else { s })
            }
            "edit.move_right" => {
                self.select_field(n, if collapsed { next_grapheme(&v, c) } else { e }, if collapsed { next_grapheme(&v, c) } else { e })
            }
            "edit.move_word_left" => {
                let to = prev_word(&v, c);
                self.select_field(n, to, to)
            }
            "edit.move_word_right" => {
                let to = next_word(&v, c);
                self.select_field(n, to, to)
            }
            "edit.move_to_start" => self.select_field(n, 0, 0),
            "edit.move_to_end" => self.select_field(n, v.len(), v.len()),
            "edit.select_left" => self.select_field(n, a, prev_grapheme(&v, c)),
            "edit.select_right" => self.select_field(n, a, next_grapheme(&v, c)),
            "edit.select_word_left" => self.select_field(n, a, prev_word(&v, c)),
            "edit.select_word_right" => self.select_field(n, a, next_word(&v, c)),
            "edit.select_to_start" => self.select_field(n, a, 0),
            "edit.select_to_end" => self.select_field(n, a, v.len()),
            "edit.select_all" => self.select_field(n, 0, v.len()),
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
                self.app.system.borrow_mut().write_text(&v[s..e]);
                if command == "edit.cut" {
                    self.apply(n, format!("{}{}", &v[..s], &v[e..]), s, s, Kind::Other);
                }
            }
            "edit.paste" => {
                let text = self.app.system.borrow_mut().read_text();
                if let Some(t) = text {
                    // A multi-line field keeps line breaks; one line turns them into spaces.
                    let t = if multiline {
                        t.replace("\r\n", "\n").replace('\r', "\n")
                    } else {
                        t.replace("\r\n", " ").replace(['\n', '\r'], " ")
                    };
                    let new = format!("{}{t}{}", &v[..s], &v[e..]);
                    self.apply(n, new, s + t.len(), s + t.len(), Kind::Other);
                }
            }
            "edit.undo" | "edit.redo" => {
                let undo = command == "edit.undo";
                let proposal = self.w.kernel.borrow_mut().history_proposal(n, undo);
                if let Some((value, (anchor, caret))) = proposal {
                    self.apply(n, value, anchor, caret, if undo { Kind::Undo } else { Kind::Redo });
                }
            }
            _ => return false,
        }
        true
    }

    /// A primary press at window point `at` in text field `n`: `clicks` 1 places the caret (or
    /// extends the selection with Shift), 2 selects a word, 3 selects everything.
    pub(super) fn press_in_field(&self, n: NodeId, (x, y): (f32, f32), clicks: u32, extend: bool) {
        let Some(at) = self.w.kernel.borrow().input_offset_at(n, x, y, clicks == 2) else { return };
        let v = self.w.kernel.borrow().value(n).to_owned();
        let (a, _) = self.w.kernel.borrow().selection(n);
        match clicks {
            1 if extend => self.select_field(n, a, at),
            1 => self.select_field(n, at, at),
            2 => {
                let (s, e) = word_at(&v, at);
                self.select_field(n, s, e)
            }
            _ => self.select_field(n, 0, v.len()),
        }
    }

    /// A drag in text field `n` to window point `(x, y)`: the caret follows, the anchor stays.
    pub(super) fn drag_in_field(&self, n: NodeId, (x, y): (f32, f32)) {
        let Some(at) = self.w.kernel.borrow().input_offset_at(n, x, y, false) else { return };
        let (a, _) = self.w.kernel.borrow().selection(n);
        self.select_field(n, a, at);
    }

    /// What the clipboard holds as text.
    pub fn clipboard_text(&self) -> Option<String> {
        self.app.system.borrow_mut().read_text()
    }

    /// The focused text field, for platform text protocols.
    pub fn text_field(&self) -> Option<TextField> {
        let (n, text, a, c) = self.field()?;
        let k = self.w.kernel.borrow();
        let r = k.caret(n)?;
        let composition = k.preedit(n).to_owned();
        let composition_selection = k.preedit_selection(n);
        let collapsed = if composition.is_empty() { a == c } else { composition_selection.as_ref().is_some_and(Range::is_empty) };
        Some(TextField {
            secure: k.is_secure(n),
            selection: a.min(c)..a.max(c),
            reversed: c < a,
            composition,
            composition_selection,
            caret: (r.x, r.y, r.w, r.h),
            caret_visible: collapsed && k.caret_shown(n),
            text,
        })
    }

    /// Replaces `range` (bytes) of the focused field's committed text with `text`, ending any
    /// composition; the caret goes after it. Platform edits in displayed-document coordinates
    /// use [`apply_text_edit`](Self::apply_text_edit) instead.
    pub fn replace_text(&mut self, range: Range<usize>, text: &str) {
        self.input_frame(|| {
            let Some((n, v, _, _)) = self.field() else { return };
            let s = crate::kernel::floor_boundary(&v, range.start);
            let e = crate::kernel::floor_boundary(&v, range.end.max(s));
            let at = s + text.len();
            self.apply(n, format!("{}{text}{}", &v[..s], &v[e..]), at, at, Kind::Insert);
        });
    }

    /// Selects `anchor..caret` (bytes) in the focused field (the platform moved the selection).
    pub fn set_selection(&mut self, anchor: usize, caret: usize) {
        self.input_frame(|| {
            if let Some((n, ..)) = self.field() {
                self.select_field(n, anchor, caret);
            }
        });
    }

    /// Applies a queued platform edit at a normal input boundary. The field must still own
    /// input and its complete editing document must equal `expected`; otherwise nothing is
    /// changed. Native callbacks edit their owned [`TextDocument`] first and enqueue this
    /// operation rather than calling application handlers synchronously.
    pub fn apply_text_edit(&mut self, owner: NodeId, expected: &TextDocument, edit: &TextEdit) -> bool {
        let mut accepted = false;
        self.input_frame(|| {
            let Some((id, ..)) = self.field() else { return };
            let mut document = {
                let k = self.w.kernel.borrow();
                if id != owner || k.is_secure(id) || k.edit_pending(id) {
                    return;
                }
                let Some(document) = k.text_document(id) else { return };
                if &document != expected {
                    return;
                }
                document
            };
            let composing = document.composition.is_some();
            document.apply(edit);
            accepted = true;
            match edit {
                TextEdit::Insert { .. } | TextEdit::Unmark if composing || matches!(edit, TextEdit::Insert { .. }) => {
                    accepted =
                        self.apply(id, document.value, document.anchor, document.caret, if composing { Kind::Other } else { Kind::Insert });
                }
                _ => {
                    self.w.kernel.borrow_mut().set_composition(id, document.composition);
                }
            }
        });
        accepted
    }

    /// What an input method is composing in the focused field (pinyin before it becomes 汉字):
    /// shown underlined in place of the selection, without changing the committed value.
    /// Empty ends it. `Text` commits.
    pub(super) fn on_composition(&mut self, text: &str) {
        self.on_preedit(text, Some((text.len(), text.len())));
    }

    pub(super) fn on_preedit(&mut self, text: &str, selection: Option<(usize, usize)>) {
        self.input_frame(|| {
            let Some((n, ..)) = self.field() else { return };
            // No input method in a secure field or over an unresolved edit.
            let unavailable = {
                let k = self.w.kernel.borrow();
                k.is_secure(n) || k.edit_pending(n)
            };
            if unavailable {
                return;
            }
            self.w.kernel.borrow_mut().set_preedit_selection(n, text, selection);
        });
    }

    /// A value set by assistive technology (or an agent) for `n`: through its `on_input`, with
    /// the caret at the end.
    pub(super) fn set_value_of(&self, n: NodeId, value: String) -> bool {
        {
            let k = self.w.kernel.borrow();
            if k.check_activation(n, self.top_modal()).is_err() || k.handler(n, HandlerKind::Input).is_none() {
                return false;
            }
        }
        self.set_modality(Modality::Keyboard);
        if self.w.kernel.borrow().kind(n) != crate::kernel::Kind::TextInput {
            self.run(n, &[HandlerKind::Input], Some(value));
            return true;
        }
        let len = value.len();
        self.apply(n, value, len, len, Kind::Other)
    }
}

#[cfg(test)]
mod tests {
    use super::{TextDocument, TextEdit, byte_offset, utf16_offset};
    use std::ops::Range;

    #[test]
    fn editor_history_follows_value_and_security_boundaries() {
        use crate::prelude::*;

        let mut window = Window::headless(|| {
            let value = state("value", || "a".to_owned());
            text_input().id("field").value(value).on_input(move |text| value.set(text)).into()
        });
        window.tap("field").unwrap();
        window.set_selection(1, 1);
        window.type_text("b");
        let id = window.kernel().find("field").unwrap();
        {
            let mut kernel = window.w.kernel.borrow_mut();
            assert!(kernel.history_available(id, true));
            kernel.set_preedit(id, "ni");
            kernel.set_value(id, "ab");
            assert!(kernel.history_available(id, true), "same-value writes preserve history");
            assert_eq!(kernel.preedit(id), "ni");
            kernel.set_value(id, "😀");
            assert!(!kernel.history_available(id, true));
            assert!(kernel.preedit(id).is_empty());
            kernel.set_secure(id, true);
        }
        window.type_text("secret");
        assert!(!window.w.kernel.borrow().history_available(id, true));
        assert!(!window.w.kernel.borrow().edit_pending(id));
        window.app().shutdown();
    }

    #[test]
    fn failed_edit_callbacks_and_bindings_retire_proposals_without_publishing() {
        use crate::prelude::*;
        use std::panic::{AssertUnwindSafe, catch_unwind};

        for fail_in_binding in [false, true] {
            let mut window = Window::headless(move || {
                let value = state("value", || "before".to_owned());
                text_input()
                    .id("field")
                    .value(move || {
                        let value = value.get();
                        assert!(!fail_in_binding || value == "before", "binding failed");
                        value
                    })
                    .on_input(move |text| {
                        value.set(text);
                        assert!(fail_in_binding, "callback failed");
                    })
                    .into()
            });
            window.tap("field").unwrap();
            let id = window.kernel().find("field").unwrap();
            let frames = window.frames_presented();
            assert!(catch_unwind(AssertUnwindSafe(|| window.type_text("X"))).is_err());
            assert!(window.app().is_failed());
            assert!(!window.w.kernel.borrow().edit_pending(id));
            assert!(!window.w.kernel.borrow().history_available(id, true));
            assert_eq!(window.frames_presented(), frames);
            window.app().shutdown();
        }
    }

    #[test]
    fn utf16_surrogates_and_invalid_byte_ranges_are_clamped_without_splitting_text() {
        let mut document = TextDocument::new("A😀Z".into(), 1, 5);
        assert_eq!(byte_offset(document.text(), 2), 1, "the inside of a surrogate pair floors to its start");
        document.apply(&TextEdit::Mark {
            text: "中😀文".into(),
            selection: Some(Range { start: usize::MAX, end: 4 }),
            replacement: Some(Range { start: 4, end: 2 }),
        });
        assert_eq!(document.text(), "A中😀文😀Z");
        assert_eq!(document.marked(), Some(1..11));
        let selection = document.selection().unwrap();
        assert_eq!(selection, 4..11);
        assert_eq!(utf16_offset(document.text(), selection.start)..utf16_offset(document.text(), selection.end), 2..5);
        document.apply(&TextEdit::Insert { text: "好".into(), replacement: Some(Range { start: usize::MAX, end: 4 }) });
        assert_eq!(document.text(), "A中好");
    }
}
