//! Node-owned edit acceptance and history. No application callbacks run here.
use super::{Dirty, Kernel, NodeId};
use unicode_segmentation::UnicodeSegmentation;

const UNDO_LIMIT: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EditKind {
    Insert,
    Delete,
    Other,
    Undo,
    Redo,
}

/// One undo step: at byte `at`, `removed` was replaced by `inserted`; the selection
/// `(anchor, caret)` before and after. Only the change is kept, so long texts cost their edits,
/// not a copy per step.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Step {
    at: usize,
    removed: String,
    inserted: String,
    before: (usize, usize),
    after: (usize, usize),
}

impl Step {
    /// The step from `old` to `new` (their common start and end left out).
    fn between(old: &str, new: &str, before: (usize, usize), after: (usize, usize)) -> Step {
        let mut p = old.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(p) || !new.is_char_boundary(p) {
            p -= 1;
        }
        let max = old.len().min(new.len()) - p;
        let mut q = old.bytes().rev().zip(new.bytes().rev()).take(max).take_while(|(a, b)| a == b).count();
        while !old.is_char_boundary(old.len() - q) || !new.is_char_boundary(new.len() - q) {
            q -= 1;
        }
        Step { at: p, removed: old[p..old.len() - q].to_owned(), inserted: new[p..new.len() - q].to_owned(), before, after }
    }
    /// `v` with this step undone (`back`) or done again.
    fn apply(&self, v: &str, back: bool) -> Option<String> {
        let (out, ins) = if back { (&self.inserted, &self.removed) } else { (&self.removed, &self.inserted) };
        let end = self.at.checked_add(out.len())?;
        if v.get(self.at..end)? != out {
            return None;
        }
        Some(format!("{}{ins}{}", v.get(..self.at)?, v.get(end..)?))
    }
    /// This step followed by `next` as one step, if `next` continues it (typing on, deleting
    /// back or forward from where it stopped).
    fn merge(&mut self, next: &Step) -> bool {
        if next.removed.is_empty() && self.removed.is_empty() && next.at == self.at + self.inserted.len() {
            self.inserted.push_str(&next.inserted);
        } else if next.inserted.is_empty() && self.inserted.is_empty() && next.at + next.removed.len() == self.at {
            self.removed.insert_str(0, &next.removed);
            self.at = next.at;
        } else if next.inserted.is_empty() && self.inserted.is_empty() && next.at == self.at {
            self.removed.push_str(&next.removed);
        } else {
            return false;
        }
        self.after = next.after;
        true
    }
}

impl Drop for Step {
    fn drop(&mut self) {
        super::wipe(&mut self.removed);
        super::wipe(&mut self.inserted);
    }
}

/// Short-lived evidence across callback and binding settlement. Never printed, and wiped
/// on every exit path, including secure inputs and interrupted callbacks.
struct Proposal {
    before: String,
    value: String,
    before_selection: (usize, usize),
    selection: (usize, usize),
    kind: EditKind,
    active: bool,
}
impl Drop for Proposal {
    fn drop(&mut self) {
        super::wipe(&mut self.before);
        super::wipe(&mut self.value);
    }
}

#[derive(Default)]
pub(super) struct Editing {
    undo: Vec<Step>,
    redo: Vec<Step>,
    merge: Option<(EditKind, (usize, usize))>,
    pending: Option<Proposal>,
}

impl Editing {
    pub(super) fn selection_changed(&mut self) {
        if self.pending.is_some() {
            *self = Self::default();
        }
        self.merge = None;
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// Map corresponding prefix/suffix positions, moving replaced interiors to the new end.
/// Floor to grapheme boundaries, preserving anchor/caret direction.
pub(super) fn map_selection(old: &str, new: &str, selection: (usize, usize)) -> (usize, usize) {
    let change = Step::between(old, new, (0, 0), (0, 0));
    let old_end = change.at + change.removed.len();
    let new_end = change.at + change.inserted.len();
    let map = |at: usize| {
        let at = at.min(old.len());
        let mapped = if at <= change.at {
            at
        } else if at >= old_end {
            new_end + at - old_end
        } else {
            new_end
        };
        if mapped >= new.len() {
            return new.len();
        }
        new.grapheme_indices(true).take_while(|(b, _)| *b <= mapped).last().map_or(0, |(b, _)| b)
    };
    (map(selection.0), map(selection.1))
}

impl Kernel {
    pub(crate) fn edit_pending(&self, id: NodeId) -> bool {
        self.nodes.get(id).and_then(|n| n.input()).is_some_and(|i| i.editing.pending())
    }

    pub(crate) fn edit_in_progress(&self, id: NodeId) -> bool {
        self.nodes.get(id).and_then(|n| n.input()).is_some_and(|i| i.composition.is_some() || i.editing.pending())
    }

    /// Returns whether a changed value needs to be proposed to the application's handler.
    pub(crate) fn begin_edit(&mut self, id: NodeId, value: &str, selection: (usize, usize), kind: EditKind) -> Option<bool> {
        let i = self.nodes.get_mut(id)?.input_mut()?;
        if i.editing.pending() {
            return None;
        }
        let before_selection = (super::floor_boundary(&i.value, i.selection.0), super::floor_boundary(&i.value, i.selection.1));
        i.editing.pending =
            Some(Proposal { before: i.value.clone(), value: value.to_owned(), before_selection, selection, kind, active: true });
        let notify = i.value != value;
        if i.composition.take().is_some() {
            i.clean.set(0);
            self.renew_editor_a11y_runs(id);
        }
        self.mark(Dirty::PAINT);
        Some(notify)
    }

    pub(crate) fn finish_edit_callback(&mut self, id: NodeId) {
        if let Some(p) = self.nodes.get_mut(id).and_then(|n| n.input_mut()).and_then(|i| i.editing.pending.as_mut()) {
            p.active = false;
        }
    }

    pub(crate) fn cancel_edit(&mut self, id: NodeId) {
        if let Some(i) = self.nodes.get_mut(id).and_then(|n| n.input_mut())
            && i.editing.pending.take().is_some()
        {
            i.editing = Editing::default();
        }
    }

    pub(crate) fn cancel_pending_edits(&mut self) {
        for (_, n) in &mut self.nodes {
            if let Some(i) = n.input_mut()
                && i.editing.pending.take().is_some()
            {
                i.editing = Editing::default();
            }
        }
    }

    pub(crate) fn retire_editors(&mut self, root: NodeId) {
        let ids: Vec<_> = self.nodes.keys().filter(|&id| self.is_in_interaction_scope(id, root)).collect();
        for id in ids {
            if let Some(i) = self.node_mut(id).input_mut() {
                i.editing = Editing::default();
            }
        }
    }

    /// A nested frame cannot publish while an input callback is still accepting its proposal.
    pub(crate) fn has_active_edit(&self) -> bool {
        self.nodes.values().any(|n| n.input().is_some_and(|i| i.editing.pending.as_ref().is_some_and(|p| p.active)))
    }

    pub(crate) fn reconcile_edits(&mut self) {
        let mut changed = false;
        let mut selection_changes = Vec::new();
        for (id, node) in &mut self.nodes {
            let Some(i) = node.input_mut() else { continue };
            if i.editing.pending.as_ref().is_none_or(|p| p.active) {
                continue;
            }
            let p = i.editing.pending.take().unwrap();
            let selection = if i.value == p.value {
                if !i.secure && p.before != p.value {
                    let history = &mut i.editing;
                    match p.kind {
                        EditKind::Undo => {
                            if let Some(step) = history.undo.pop() {
                                history.redo.push(step);
                            }
                            history.merge = None;
                        }
                        EditKind::Redo => {
                            if let Some(step) = history.redo.pop() {
                                history.undo.push(step);
                            }
                            history.merge = None;
                        }
                        kind => {
                            let step = Step::between(&p.before, &p.value, p.before_selection, p.selection);
                            let continues = kind != EditKind::Other && history.merge == Some((kind, p.before_selection));
                            if !(continues && history.undo.last_mut().is_some_and(|last| last.merge(&step))) {
                                history.undo.push(step);
                                if history.undo.len() > UNDO_LIMIT {
                                    history.undo.remove(0);
                                }
                            }
                            history.redo.clear();
                            history.merge = Some((kind, p.selection));
                        }
                    }
                }
                p.selection
            } else if i.value == p.before {
                i.editing.merge = None;
                p.before_selection
            } else {
                i.editing = Editing::default();
                map_selection(&p.value, &i.value, p.selection)
            };
            let selection = (super::floor_boundary(&i.value, selection.0), super::floor_boundary(&i.value, selection.1));
            if i.selection != selection && !i.secure && self.a11y_live.get() {
                selection_changes.push(id);
            }
            i.selection = selection;
            i.goal.set(None);
            changed = true;
        }
        if changed {
            self.mark(Dirty::PAINT);
        }
        for id in selection_changes {
            self.a11y_touch(id);
        }
    }

    pub(crate) fn history_available(&self, id: NodeId, undo: bool) -> bool {
        self.nodes.get(id).and_then(|n| n.input()).is_some_and(|i| {
            !i.secure && !i.editing.pending() && if undo { !i.editing.undo.is_empty() } else { !i.editing.redo.is_empty() }
        })
    }

    pub(crate) fn history_proposal(&mut self, id: NodeId, undo: bool) -> Option<(String, (usize, usize))> {
        if !self.history_available(id, undo) {
            return None;
        }
        let i = self.node_mut(id).input_mut()?;
        let step = if undo { i.editing.undo.last() } else { i.editing.redo.last() }?;
        let Some(value) = step.apply(&i.value, undo) else {
            i.editing = Editing::default();
            return None;
        };
        Some((value, if undo { step.before } else { step.after }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_mapping_preserves_correspondence_and_graphemes() {
        assert_eq!(map_selection("a!😀b", "a😀b", (6, 1)), (5, 1));
        assert_eq!(map_selection("abc", "aXYZc", (2, 1)), (4, 1));
        assert_eq!(map_selection("ab", "a\u{301}b", (1, 2)), (0, 4));
        assert_eq!(map_selection("", "😀", (0, 0)), (0, 0));
        assert_eq!(map_selection("abc", "", (3, 1)), (0, 0));
    }

    #[test]
    fn history_rejects_changed_source_and_invalid_utf8_ranges() {
        let step = Step::between("a", "ab", (1, 1), (2, 2));
        assert!(step.apply("ac", true).is_none());
        assert!(step.apply("😀", true).is_none());
        assert!(step.apply("", true).is_none());
    }

    #[test]
    fn steps_keep_only_the_change_and_round_trip() {
        let old = "héllo wörld";
        let new = "héllo, wörld";
        let s = Step::between(old, new, (6, 6), (7, 7));
        assert_eq!((s.at, s.removed.as_str(), s.inserted.as_str()), (6, "", ","), "the first difference");
        let both = Step::between("aé", "aè", (0, 0), (0, 0));
        assert_eq!((both.at, both.removed.as_str(), both.inserted.as_str()), (1, "é", "è"), "whole characters");
        for (a, b) in [(old, new), (old, "héllo, wörld!"), ("", "abc"), ("abc", ""), ("aaa", "aaaa"), ("日本語", "日本")] {
            let s = Step::between(a, b, (0, 0), (0, 0));
            assert_eq!(s.apply(a, false).as_deref(), Some(b));
            assert_eq!(s.apply(b, true).as_deref(), Some(a));
        }
    }

    #[test]
    fn runs_of_typing_and_deleting_merge() {
        let mut typing = Step::between("ab", "abc", (2, 2), (3, 3));
        assert!(typing.merge(&Step::between("abc", "abcd", (3, 3), (4, 4))));
        assert_eq!((typing.at, typing.inserted.as_str(), typing.after), (2, "cd", (4, 4)));
        let mut back = Step::between("abcd", "abc", (4, 4), (3, 3));
        assert!(back.merge(&Step::between("abc", "ab", (3, 3), (2, 2))));
        assert_eq!((back.at, back.removed.as_str()), (2, "cd"));
        let mut forward = Step::between("abcd", "acd", (1, 1), (1, 1));
        assert!(forward.merge(&Step::between("acd", "ad", (1, 1), (1, 1))));
        assert_eq!((forward.at, forward.removed.as_str()), (1, "bc"));
        assert!(!typing.merge(&Step::between("abcd", "xabcd", (0, 0), (1, 1))), "elsewhere: a new step");
    }
}
