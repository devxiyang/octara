//! Translate UIKit's displayed document and UTF-16 selection into guarded editor proposals.

use crate::os::ActiveEditorView;
use octara::EditorRope;
use octara_model::identity::EditorSessionId;
use octara_model::input::{EditorEvent, Key, KeyEvent, Modifiers};
use octara_model::text::{CommitKind, TextEdit};
use std::collections::VecDeque;
use std::ops::Range;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Draft {
    pub(super) text: EditorRope,
    pub(super) marked: Option<Range<usize>>,
    pub(super) selection: Option<(usize, usize)>,
}

impl Draft {
    pub(super) fn from_editor(editor: &ActiveEditorView<'_>) -> Self {
        match editor.editor.composition() {
            Some(composition) => Self {
                text: editor.editor.displayed_rope().clone(),
                marked: Some(composition.marked()),
                selection: composition.selection().map(ordered),
            },
            None => Self::committed(editor),
        }
    }

    pub(super) fn committed(editor: &ActiveEditorView<'_>) -> Self {
        Self { text: editor.editor.rope().clone(), marked: None, selection: Some(ordered(editor.editor.selection())) }
    }

    pub(super) fn same_composition(&self, next: &Self) -> bool {
        self.marked.as_ref().zip(next.marked.as_ref()).is_some_and(|(a, b)| {
            a.start == b.start
                && self.text.as_str()[..a.start] == next.text.as_str()[..b.start]
                && self.text.as_str()[a.end..] == next.text.as_str()[b.end..]
        })
    }

    fn replacement_for(&self, next: &Self, marked: &Range<usize>) -> Option<Range<usize>> {
        let before = next.text.as_str().get(..marked.start)?;
        let after = next.text.as_str().get(marked.end..)?;
        let replaced = self.text.as_str().strip_prefix(before)?.strip_suffix(after)?;
        Some(marked.start..marked.start + replaced.len())
    }

    fn resolve(&self, next: &Self, committed: &Self, session: EditorSessionId, expected_rev: u64) -> Result<Option<EditorEvent>, ()> {
        if self == next {
            return Ok(None);
        }
        if self.text == next.text && self.marked == next.marked {
            return Ok(next.selection.map(|selection| EditorEvent::ProposeSelection { session, expected_rev, selection }));
        }
        if let Some(marked) = &next.marked {
            let text = next.text.slice(marked.clone()).ok_or(())?;
            let selection = next
                .selection
                .filter(|&(start, end)| start >= marked.start && end <= marked.end)
                .map(|(start, end)| (start - marked.start, end - marked.start));
            let replacement = Some(self.replacement_for(next, marked).ok_or(())?);
            return Ok(Some(EditorEvent::Composition { session, expected_rev, text, selection, replacement }));
        }
        let cleared_mark = self.marked.as_ref().is_some_and(|marked| {
            next.text.len() + marked.len() == self.text.len()
                && next.text.as_str().starts_with(&self.text.as_str()[..marked.start])
                && next.text.as_str().ends_with(&self.text.as_str()[marked.end..])
        });
        if self.marked.is_some() && (next == committed || cleared_mark) {
            return Ok(Some(EditorEvent::CancelComposition { session, expected_rev }));
        }
        let selection = next.selection.unwrap_or((next.text.len(), next.text.len()));
        let kind = if self.marked.is_some() || self.text.len() == next.text.len() {
            CommitKind::Other
        } else if self.text.len() > next.text.len() {
            CommitKind::Deleting
        } else {
            CommitKind::Typing
        };
        Ok(Some(EditorEvent::ProposeEdit {
            session,
            expected_rev,
            edit: TextEdit::Commit { text: next.text.as_str().to_owned(), selection, kind },
        }))
    }
}

fn ordered((a, b): (usize, usize)) -> (usize, usize) {
    (a.min(b), a.max(b))
}

pub(super) enum Change {
    Document(Draft),
    Return,
}

#[derive(Default)]
pub(super) struct PendingInputs(VecDeque<Change>);

impl PendingInputs {
    pub(super) fn record(&mut self, draft: Draft) {
        if let Some(Change::Document(previous)) = self.0.back_mut() {
            if *previous == draft {
                return;
            }
            if previous.same_composition(&draft) {
                *previous = draft;
                return;
            }
        }
        self.0.push_back(Change::Document(draft));
    }

    pub(super) fn record_return(&mut self) {
        self.0.push_back(Change::Return);
    }

    pub(super) fn take(&mut self) -> Option<Change> {
        self.0.pop_front()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

pub(super) fn proposal(
    change: &Change,
    previous: &Draft,
    committed: &Draft,
    session: EditorSessionId,
    expected_rev: u64,
) -> Result<Option<EditorEvent>, ()> {
    match change {
        Change::Document(draft) => previous.resolve(draft, committed, session, expected_rev),
        Change::Return if previous.marked.is_none() => {
            Ok(Some(EditorEvent::KeyIntercept { session, expected_rev, key: KeyEvent::new(Key::Enter, Modifiers::empty()) }))
        }
        Change::Return => Ok(None),
    }
}

pub(super) fn byte_offset(text: &EditorRope, position: usize) -> usize {
    text.utf16_byte_bounds(position).map_or(text.len(), |(floor, _)| floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::identity::{NodeId, TreeId};

    fn draft(text: &str, marked: Option<Range<usize>>, selection: (usize, usize)) -> Draft {
        Draft { text: EditorRope::new(text), marked, selection: Some(selection) }
    }

    fn session() -> EditorSessionId {
        EditorSessionId { tree: TreeId::new([0, 0], 42), node: NodeId::default(), session_gen: 2 }
    }

    #[test]
    fn native_mark_over_utf16_selection_commits_before_return() {
        let before = draft("A😀Z", None, (1, 5));
        assert_eq!(byte_offset(&before.text, 3), 5);
        assert_eq!(before.text.byte_to_utf16(5), Some(3));

        let mut inputs = PendingInputs::default();
        inputs.record(draft("AnZ", Some(1..2), (2, 2)));
        let composed = draft("AnihaoZ", Some(1..6), (6, 6));
        inputs.record(composed.clone());
        inputs.record(draft("A你好Z", None, (7, 7)));
        inputs.record_return();

        let observed = proposal(&inputs.take().unwrap(), &before, &before, session(), 7).unwrap();
        assert!(matches!(observed, Some(EditorEvent::Composition {
            expected_rev: 7, text, selection: Some((5, 5)), replacement: Some(range), ..
        }) if text == "nihao" && range == (1..5)));

        let observed = proposal(&inputs.take().unwrap(), &composed, &before, session(), 8).unwrap();
        assert!(matches!(observed, Some(EditorEvent::ProposeEdit {
            expected_rev: 8, edit: TextEdit::Commit { text, selection: (7, 7), .. }, ..
        }) if text == "A你好Z"));

        let after = draft("A你好Z", None, (7, 7));
        let observed = proposal(&inputs.take().unwrap(), &after, &after, session(), 9).unwrap();
        assert!(matches!(observed, Some(EditorEvent::KeyIntercept { expected_rev: 9, key: KeyEvent { key: Key::Enter, .. }, .. })));
        assert!(inputs.is_empty());
    }

    #[test]
    fn cancelling_a_native_mark_restores_the_replaced_selection() {
        let before = draft("A😀Z", None, (1, 5));
        let composed = draft("AnihaoZ", Some(1..6), (6, 6));
        let cancelled = Change::Document(draft("AZ", None, (1, 1)));
        let event = proposal(&cancelled, &composed, &before, session(), 8).unwrap();
        assert_eq!(event, Some(EditorEvent::CancelComposition { session: session(), expected_rev: 8 }));
    }
}
