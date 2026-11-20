use octara::app::{InputEvent, Key, KeyEvent, Modifiers, Window};
use octara::kernel::TextDocument;
use octara_primitives::identity::NodeId;
use octara_primitives::text::TextEdit;
use std::collections::VecDeque;
use std::ops::Range;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Draft {
    pub(crate) text: String,
    pub(crate) marked: Option<Range<usize>>,
    pub(crate) selection: Option<Range<usize>>,
}

pub(crate) enum PendingInput {
    Draft(Draft),
    Return,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Accepted,
    Discard,
    Return,
}

#[derive(Default)]
pub(crate) struct PendingInputs(VecDeque<PendingInput>);

impl PendingInputs {
    pub(crate) fn record(&mut self, draft: Draft) {
        if let Some(PendingInput::Draft(previous)) = self.0.back_mut() {
            if *previous == draft {
                return;
            }
            if previous.same_composition(&draft) {
                *previous = draft;
                return;
            }
        }
        self.0.push_back(PendingInput::Draft(draft));
    }

    pub(crate) fn record_return(&mut self) {
        self.0.push_back(PendingInput::Return);
    }

    pub(crate) fn take(&mut self) -> Option<PendingInput> {
        self.0.pop_front()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// Ranged drafts after a mutating Return describe UIKit's document before that key.
    /// Consecutive keys remain valid because they carry no offsets or text.
    pub(crate) fn reconcile(&mut self, delivery: Delivery, document_changed: bool) {
        match delivery {
            Delivery::Discard => self.clear(),
            Delivery::Return if document_changed => {
                if let Some(stale) = self.0.iter().position(|input| matches!(input, PendingInput::Draft(_))) {
                    self.0.truncate(stale);
                }
            }
            _ => {}
        }
    }
}

impl PendingInput {
    /// Deliver one observation against the document on which the native session started
    /// this step. Published state decides whether later snapshots still have that base.
    pub(crate) fn deliver(
        &self,
        window: &mut Window,
        owner: NodeId,
        before: &TextDocument,
        current: impl Fn(&Window) -> Option<TextDocument>,
    ) -> Delivery {
        let current = |window: &Window| window.frame_is_settled().then(|| current(window)).flatten();
        if current(window).as_ref() != Some(before) {
            return Delivery::Discard;
        }
        match self {
            Self::Return if before.marked().is_none() => {
                window.event(InputEvent::Key(KeyEvent::new(Key::Enter, Modifiers::empty())));
                if current(window).is_some() { Delivery::Return } else { Delivery::Discard }
            }
            Self::Draft(draft) => {
                let Some(resolved) = draft.resolve(before) else { return Delivery::Discard };
                if let Some(edit) = &resolved.edit
                    && !window.apply_text_edit(owner, before, edit)
                {
                    return Delivery::Discard;
                }
                if current(window).as_ref() != Some(&resolved.document) {
                    return Delivery::Discard;
                }
                if let Some(range) = resolved.selection
                    && (!window.apply_text_selection(owner, &resolved.document, range.clone())
                        || current(window).is_none_or(|actual| {
                            actual.text() != resolved.document.text()
                                || actual.marked() != resolved.document.marked()
                                || actual.selection() != Some(range)
                        }))
                {
                    return Delivery::Discard;
                }
                if resolved.native_matches { Delivery::Accepted } else { Delivery::Discard }
            }
            Self::Return => Delivery::Discard,
        }
    }
}

struct DraftChange {
    edit: Option<TextEdit>,
    document: TextDocument,
    selection: Option<Range<usize>>,
    native_matches: bool,
}

impl Draft {
    fn same_composition(&self, next: &Self) -> bool {
        self.marked.as_ref().zip(next.marked.as_ref()).is_some_and(|(a, b)| {
            a.start == b.start && self.text[..a.start] == next.text[..b.start] && self.text[a.end..] == next.text[b.end..]
        })
    }

    fn resolve(&self, expected: &TextDocument) -> Option<DraftChange> {
        let removed_mark = expected.marked().is_some_and(|mark| {
            self.marked.is_none()
                && self.text.len() + mark.len() == expected.text().len()
                && self.text.starts_with(&expected.text()[..mark.start])
                && self.text.ends_with(&expected.text()[mark.end..])
        });
        let edit = self.edit(expected, removed_mark);
        let mut document = expected.clone();
        if let Some(edit) = &edit {
            document.apply(edit);
        }
        let native_matches = self.text == document.text() && self.marked == document.marked();
        let cancelled = matches!(edit, Some(TextEdit::Cancel));
        if !native_matches && !cancelled {
            return None;
        }
        let selection = self.selection.clone().filter(|s| !cancelled && document.selection().as_ref() != Some(s));
        Some(DraftChange { edit, document, selection, native_matches })
    }

    pub(crate) fn edit(&self, document: &TextDocument, cancelled: bool) -> Option<TextEdit> {
        let previous = document.text();
        if let Some(marked) = &self.marked {
            let prefix = self.text.get(..marked.start)?;
            let suffix = self.text.get(marked.end..)?;
            let replaced = previous.strip_prefix(prefix)?.strip_suffix(suffix)?;
            let selection = self
                .selection
                .as_ref()
                .filter(|r| r.start >= marked.start && r.end <= marked.end)
                .map(|r| r.start - marked.start..r.end - marked.start);
            return Some(TextEdit::Mark {
                text: self.text.get(marked.clone())?.to_owned(),
                selection,
                replacement: Some(marked.start..marked.start + replaced.len()),
            });
        }
        if document.marked().is_some() {
            if cancelled {
                return Some(TextEdit::Cancel);
            }
            if previous == self.text {
                return Some(TextEdit::Unmark);
            }
            let mut committed = document.clone();
            committed.apply(&TextEdit::Cancel);
            // A candidate can reproduce the original text while advancing the caret.
            if committed.text() == self.text && (self.selection.is_none() || self.selection == committed.selection()) {
                return Some(TextEdit::Cancel);
            }
        }
        if previous == self.text {
            return None;
        }
        // The active selection/mark disambiguates equal text on either side of the edit.
        let preferred = document.marked().or_else(|| document.selection());
        if let Some(range) = preferred
            && let Some(text) = self.text.strip_prefix(&previous[..range.start]).and_then(|s| s.strip_suffix(&previous[range.end..]))
            && self.selection.as_ref().is_some_and(|s| s.start == range.start + text.len())
        {
            return Some(TextEdit::Insert { text: text.to_owned(), replacement: Some(range) });
        }
        let start = previous.chars().zip(self.text.chars()).take_while(|(a, b)| a == b).map(|(c, _)| c.len_utf8()).sum();
        let suffix: usize = previous[start..]
            .chars()
            .rev()
            .zip(self.text[start..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum();
        Some(TextEdit::Insert {
            text: self.text[start..self.text.len() - suffix].to_owned(),
            replacement: Some(start..previous.len() - suffix),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::platform::HeadlessWindow;
    use octara::prelude::{Signal, col, focus, input, state, textarea};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn draft(text: &str, marked: Option<Range<usize>>, caret: usize) -> Draft {
        Draft { text: text.into(), marked, selection: Some(caret..caret) }
    }

    fn editor(receive: impl Fn(Signal<String>, String) + 'static) -> (Window, NodeId) {
        let receive = Rc::new(receive);
        let mut window = Window::headless(move || {
            let value = state("note", || "A😀\nZ".to_owned());
            let receive = receive.clone();
            col()
                .child(textarea().id("note").label("Note").value(value).on_input(move |text| receive(value, text)))
                .child(input().id("other").label("Other"))
        });
        window.tap("note").unwrap();
        window.set_selection(5, 1);
        let owner = window.kernel().find("note").unwrap();
        (window, owner)
    }

    fn document(window: &Window, owner: NodeId) -> Option<TextDocument> {
        let platform = window.platform_window();
        let native = platform.as_any().downcast_ref::<HeadlessWindow>().unwrap().text_input.as_ref()?;
        native.document.clone().filter(|_| native.id == owner)
    }

    fn drain(pending: &mut PendingInputs, window: &mut Window, owner: NodeId) -> Vec<Delivery> {
        let mut outcomes = Vec::new();
        while let (Some(before), Some(next)) = (document(window, owner), pending.take()) {
            let outcome = next.deliver(window, owner, &before, |window| document(window, owner));
            pending.reconcile(outcome, document(window, owner).as_ref() != Some(&before));
            outcomes.push(outcome);
        }
        outcomes
    }

    fn consecutive_compositions() -> PendingInputs {
        let mut pending = PendingInputs::default();
        pending.record(draft("Ani\nZ", Some(1..3), 3));
        pending.record(draft("Ani\nZ", Some(1..3), 3));
        pending.record(draft("Ani hao\nZ", Some(1..7), 7));
        // UIKit may replace a candidate while it is still marked, then unmark it.
        pending.record(draft("A你\nZ", Some(1..4), 4));
        pending.record(draft("A你\nZ", None, 4));
        pending.record(draft("A你hao\nZ", Some(4..7), 7));
        pending
    }

    #[test]
    fn one_drain_keeps_a_candidate_commit_before_the_next_composition() {
        let edits = Rc::new(RefCell::new(Vec::new()));
        let observed = edits.clone();
        let (mut window, owner) = editor(move |value, text| {
            observed.borrow_mut().push(text.clone());
            value.set(text);
        });
        let mut pending = consecutive_compositions();
        assert_eq!(drain(&mut pending, &mut window, owner), [Delivery::Accepted, Delivery::Accepted, Delivery::Accepted]);
        assert_eq!(&*edits.borrow(), &["A你\nZ"]);
        let current = document(&window, owner).unwrap();
        assert_eq!(current.text(), "A你hao\nZ");
        assert_eq!(current.marked(), Some(4..7));
        assert_eq!(window.text_field().unwrap().text, "A你\nZ");

        pending.record(draft("A你\nZ", None, 4));
        assert_eq!(drain(&mut pending, &mut window, owner), [Delivery::Accepted]);
        assert_eq!(document(&window, owner).unwrap(), TextDocument::new("A你\nZ".into(), 4, 4));
        assert_eq!(&*edits.borrow(), &["A你\nZ"], "cancelling the second mark does not edit the accepted candidate");
    }

    #[test]
    fn a_rejected_or_rewritten_commit_discards_dependent_native_input() {
        for replacement in [None, Some("Server value")] {
            let edits = Rc::new(RefCell::new(Vec::new()));
            let observed = edits.clone();
            let (mut window, owner) = editor(move |value, text| {
                observed.borrow_mut().push(text);
                if let Some(replacement) = replacement {
                    value.set(replacement.to_owned());
                }
            });
            let mut pending = consecutive_compositions();
            pending.record_return();
            assert_eq!(drain(&mut pending, &mut window, owner), [Delivery::Accepted, Delivery::Discard]);
            assert!(pending.is_empty());
            let current = document(&window, owner).unwrap();
            assert_eq!(current.text(), replacement.unwrap_or("A😀\nZ"));
            assert_eq!(current.marked(), None);
            assert_eq!(&*edits.borrow(), &["A你\nZ"]);
        }
    }

    #[test]
    fn focus_transfer_retires_the_remaining_drafts() {
        let (mut window, owner) = editor(|value, text| {
            value.set(text);
            focus("other");
        });
        let mut pending = consecutive_compositions();
        assert_eq!(drain(&mut pending, &mut window, owner), [Delivery::Accepted, Delivery::Discard]);
        assert!(pending.is_empty());
        assert!(document(&window, owner).is_none());
        assert_eq!(window.kernel().value_of("note").as_deref(), Some("A你\nZ"));
        assert_eq!(window.text_field().unwrap().text, "");
    }

    #[test]
    fn returns_stay_ordered_and_drop_snapshots_taken_before_the_keys() {
        let (mut window, owner) = editor(|value, text| value.set(text));
        let mut pending = PendingInputs::default();
        pending.record_return();
        pending.record_return();
        pending.record(draft("Aother\nZ", None, 6));
        assert_eq!(drain(&mut pending, &mut window, owner), [Delivery::Return, Delivery::Return]);
        assert_eq!(document(&window, owner).unwrap().text(), "A\n\n\nZ");
    }

    #[test]
    fn pinyin_replaces_the_selected_emoji_and_commits_once() {
        let mut document = TextDocument::new("A😀Z".into(), 5, 1);
        let draft = Draft { text: "Ani haoZ".into(), marked: Some(1..7), selection: Some(7..7) };
        assert!(document.apply(&draft.edit(&document, false).unwrap()));
        assert_eq!(document.marked(), Some(1..7));
        let commit = Draft { text: "A你好Z".into(), marked: None, selection: Some(7..7) };
        assert!(document.apply(&commit.edit(&document, false).unwrap()));
        assert_eq!(document.text(), "A你好Z");
        assert_eq!(document.marked(), None);
        assert_eq!(commit.edit(&document, false), None);
    }

    #[test]
    fn empty_mark_cancellation_restores_the_replaced_selection() {
        let original = TextDocument::new("A😀Z".into(), 5, 1);
        let mut document = original.clone();
        document.apply(&TextEdit::Mark { text: "ni".into(), selection: None, replacement: None });
        let cleared = Draft { text: "AZ".into(), marked: None, selection: Some(1..1) };
        document.apply(&cleared.edit(&document, true).unwrap());
        assert_eq!(document, original);
    }

    #[test]
    fn multiline_candidate_preserves_the_surrounding_breaks() {
        let original = TextDocument::new("A\n😀\r\nZ".into(), 6, 2);
        let mut document = original.clone();
        let draft = Draft { text: "A\nni hao\r\nZ".into(), marked: Some(2..8), selection: Some(5..8) };
        let edit = draft.edit(&document, false).unwrap();
        assert_eq!(edit, TextEdit::Mark { text: "ni hao".into(), selection: Some(3..6), replacement: Some(2..6) });
        assert!(document.apply(&edit));
        assert_eq!(document.text(), draft.text);
        assert_eq!(document.marked(), draft.marked);
        assert_eq!(document.selection(), draft.selection);

        let cleared = Draft { text: "A\n\r\nZ".into(), marked: None, selection: Some(2..2) };
        let mut cancelled = document.clone();
        assert!(cancelled.apply(&cleared.edit(&cancelled, true).unwrap()));
        assert_eq!(cancelled, original);

        // The snapshot can include a later edit made after confirming the candidate.
        let commit = Draft { text: "A\n你好\r\nZ!".into(), marked: None, selection: Some(12..12) };
        assert!(document.apply(&commit.edit(&document, false).unwrap()));
        assert_eq!(document, TextDocument::new(commit.text.clone(), 12, 12));
        assert_eq!(commit.edit(&document, false), None);
    }

    #[test]
    fn inserting_a_repeated_break_keeps_the_native_caret_for_the_next_edit() {
        let mut document = TextDocument::new("😀\r\nline\nend".into(), 10, 10);
        let split = Draft { text: "😀\r\nline\n\nend".into(), marked: None, selection: Some(11..11) };
        let edit = split.edit(&document, false).unwrap();
        assert_eq!(edit, TextEdit::Insert { text: "\n".into(), replacement: Some(10..10) });
        assert!(document.apply(&edit));
        assert_eq!(document.selection(), split.selection);

        let pasted = Draft { text: "😀\r\nline\n語\r\n🙂\n\nend".into(), marked: None, selection: Some(21..21) };
        assert!(document.apply(&pasted.edit(&document, false).unwrap()));
        assert_eq!(document, TextDocument::new(pasted.text, 21, 21));
    }

    #[test]
    fn committing_the_original_text_can_collapse_the_selection() {
        let mut document = TextDocument::new("A\n你\r\nZ".into(), 5, 2);
        let draft = Draft { text: "A\nni\r\nZ".into(), marked: Some(2..4), selection: Some(4..4) };
        assert!(document.apply(&draft.edit(&document, false).unwrap()));

        let restored = Draft { text: "A\n你\r\nZ".into(), marked: None, selection: Some(2..5) };
        assert_eq!(restored.edit(&document, false), Some(TextEdit::Cancel));

        let commit = Draft { selection: Some(5..5), ..restored };
        assert!(document.apply(&commit.edit(&document, false).unwrap()));
        assert_eq!(document, TextDocument::new(commit.text, 5, 5));
    }
}
