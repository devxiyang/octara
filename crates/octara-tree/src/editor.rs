//! Logical editing and controlled value settlement for the retained tree.

mod rich;
mod rope;

pub use rich::EditorRichText;
pub use rope::EditorRope;

use crate::{NodeId, Tree};
use octara_model::identity::{EditorSessionId, TreeId};
use octara_model::input::{EditorEvent, KeyEvent};
use octara_model::text::{AtomicRange, CommitKind, InlineAttachment, RichSpan, TextEdit};
use std::{fmt, ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

const UNDO_LIMIT: usize = 128;

/// Supplies rich editor presentation and atomic ranges in logical UTF-8 coordinates.
///
/// Return `None` to use the input's fallback backend or authored rich text. The
/// editor retains ownership of its value, selection, composition and history.
pub trait EditorBackend: Send + Sync {
    fn rich_text(&self, value: &str) -> Option<EditorRichText>;
}

/// Revision and selection of a focused, nonsecure editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorState {
    pub session: EditorSessionId,
    pub revision: u64,
    /// Committed UTF-8 byte offsets with the anchor first.
    pub selection: (usize, usize),
    pub composing: bool,
}

// Changed values, drafts and proposals all release their buffers through this type.
#[derive(Default, PartialEq, Eq)]
struct EditorText(String);

impl Drop for EditorText {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn wipe(text: &mut String) {
    // SAFETY: The bytes are not read while being set to zero, which is valid UTF-8.
    for byte in unsafe { text.as_bytes_mut() } {
        // SAFETY: This byte belongs to the exclusively borrowed string buffer.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    text.clear();
}

/// The displayed document and its marked range during an input-method composition.
#[derive(Clone, PartialEq)]
pub struct Composition {
    text: EditorRope,
    rich: EditorRichText,
    marked: Range<usize>,
    selection: Option<(usize, usize)>,
}

impl Composition {
    pub fn text(&self) -> &str {
        self.text.as_str()
    }

    pub fn marked(&self) -> Range<usize> {
        self.marked.clone()
    }

    /// Byte offsets in the displayed text, with the anchor first. `None` hides the cursor.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selection
    }
}

/// Retained editable state. Text and history are kept out of debug output.
pub struct EditorSlot {
    value: EditorRope,
    rich: EditorRichText,
    value_rev: u64,
    document_rev: u64,
    session_gen: u64,
    composition: Option<Composition>,
    selection: (usize, usize),
    undo: UndoHistory,
    secure: bool,
    pub placeholder: Arc<str>,
    pub multiline: bool,
    pub rows: (u16, u16),
    pub submit_on_enter: bool,
    pub revealed: bool,
}

impl fmt::Debug for EditorSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditorSlot")
            .field("document_rev", &self.document_rev)
            .field("session_gen", &self.session_gen)
            .field("secure", &self.secure)
            .finish_non_exhaustive()
    }
}

impl Default for EditorSlot {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl EditorSlot {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: EditorRope::new(value),
            rich: EditorRichText::default(),
            value_rev: 0,
            document_rev: 0,
            session_gen: 0,
            composition: None,
            selection: (0, 0),
            undo: UndoHistory::default(),
            secure: false,
            placeholder: Arc::from(""),
            multiline: false,
            rows: (1, 1),
            submit_on_enter: false,
            revealed: false,
        }
    }

    /// Internal editing value, including when secure. Use `reportable_value` for observations.
    pub fn value(&self) -> &str {
        self.value.as_str()
    }

    /// The committed editor buffer.
    pub fn rope(&self) -> &EditorRope {
        &self.value
    }

    /// The displayed buffer including any marked input-method text.
    pub fn displayed_rope(&self) -> &EditorRope {
        self.composition.as_ref().map_or(&self.value, |c| &c.text)
    }

    /// Shares the committed text for a background reader.
    pub fn snapshot(&self) -> Option<EditorRope> {
        (!self.secure).then(|| self.value.clone())
    }

    fn displayed_rich(&self) -> &EditorRichText {
        self.composition.as_ref().map_or(&self.rich, |c| &c.rich)
    }

    /// Rich styling and atoms in displayed UTF-8 coordinates.
    pub fn rich_text(&self) -> Option<&EditorRichText> {
        (!self.secure).then(|| self.displayed_rich())
    }

    pub fn is_rich(&self) -> bool {
        !self.secure && (!self.rich.is_empty() || !self.displayed_rich().is_empty())
    }

    pub fn decorations(&self) -> &[(Range<usize>, RichSpan)] {
        self.rich_text().map_or(&[], |rich| &rich.decorations)
    }

    pub fn attachments(&self) -> &[InlineAttachment] {
        self.rich_text().map_or(&[], |rich| &rich.attachments)
    }

    pub fn atoms(&self) -> &[AtomicRange] {
        self.rich_text().map_or(&[], |rich| &rich.atoms)
    }

    pub fn committed_decorations(&self) -> &[(Range<usize>, RichSpan)] {
        if self.secure { &[] } else { &self.rich.decorations }
    }

    pub fn committed_attachments(&self) -> &[InlineAttachment] {
        if self.secure { &[] } else { &self.rich.attachments }
    }

    /// Updates rich styling and token ranges for the current committed value.
    pub fn set_rich_text(&mut self, rich: EditorRichText) -> bool {
        let rich = rich.validated(&self.value);
        if self.rich == rich {
            return false;
        }
        self.rich = rich;
        self.advance_value();
        if !self.secure {
            self.selection = self.rich.snap_selection(self.selection);
        }
        if let Some(composition) = &mut self.composition {
            let (start, old_end, new_end) = change_bounds(self.value.as_str(), composition.text());
            composition.rich = self.rich.after_replace(start..old_end, new_end - start);
            if !self.secure {
                composition.selection = composition.selection.map(|selection| composition.rich.snap_selection(selection));
            }
        }
        self.undo.merge = false;
        self.advance_document();
        true
    }

    pub fn reportable_value(&self) -> Option<&str> {
        (!self.secure).then(|| self.value())
    }

    pub fn clipboard_text(&self) -> Option<&str> {
        let value = self.reportable_value()?;
        let range = ordered(self.selection);
        (!range.is_empty()).then(|| &value[range])
    }

    pub fn document_rev(&self) -> u64 {
        self.document_rev
    }

    /// The committed text and styling revision. Selection and composition updates leave it unchanged.
    pub fn value_rev(&self) -> u64 {
        self.value_rev
    }

    pub fn session_gen(&self) -> u64 {
        self.session_gen
    }

    /// Directional, committed byte offsets `(anchor, caret)`.
    pub fn selection(&self) -> (usize, usize) {
        self.selection
    }

    /// Directional selection in the displayed buffer, if the cursor is visible.
    pub fn displayed_selection(&self) -> Option<(usize, usize)> {
        match &self.composition {
            Some(composition) => composition.selection,
            None => Some(self.selection),
        }
    }

    pub fn composition(&self) -> Option<&Composition> {
        self.composition.as_ref()
    }

    pub fn is_secure(&self) -> bool {
        self.secure
    }

    pub fn set_secure(&mut self, secure: bool) -> bool {
        if self.secure == secure {
            return false;
        }
        self.secure = secure;
        if !secure {
            self.selection = self.rich.snap_selection(self.selection);
        }
        self.undo = UndoHistory::default();
        self.composition = None;
        self.invalidate_session();
        self.advance_document();
        true
    }

    pub(crate) fn reset_session(&mut self) {
        self.invalidate_session();
        if self.composition.take().is_some() {
            self.advance_document();
        }
    }

    /// Apply an authored value change outside the controlled edit path.
    pub(crate) fn replace_value(&mut self, value: impl Into<String>) -> bool {
        let value = EditorText(value.into());
        if self.value() == value.0 {
            return false;
        }
        let (start, old_end, new_end) = change_bounds(self.value(), &value.0);
        let selection = map_selection(&value.0, self.selection, start, old_end, new_end);
        self.rich = self.rich.after_replace(start..old_end, new_end - start);
        self.selection = if self.secure { selection } else { self.rich.snap_selection(selection) };
        let rope = self.value.replaced(start..old_end, &value.0[start..new_end]).expect("diff uses valid UTF-8 ranges");
        self.set_value(rope);
        self.composition = None;
        self.undo = UndoHistory::default();
        self.invalidate_session();
        self.advance_document();
        true
    }

    fn invalidate_session(&mut self) {
        self.session_gen = self.session_gen.checked_add(1).expect("editor session generations exhausted");
        self.undo.merge = false;
    }

    fn advance_document(&mut self) {
        self.document_rev = self.document_rev.checked_add(1).expect("editor document revisions exhausted");
    }

    fn set_value(&mut self, value: EditorRope) {
        self.value = value;
        self.advance_value();
    }

    fn advance_value(&mut self) {
        self.value_rev = self.value_rev.checked_add(1).expect("editor value revisions exhausted");
    }

    /// Returns whether the host needs an editor projection, including rejected edits.
    pub(crate) fn apply_update(&mut self, tree: TreeId, node: NodeId, update: EditorRuntimeUpdate) -> bool {
        self.apply_update_with_history(tree, node, update, true)
    }

    fn apply_update_with_history(&mut self, tree: TreeId, node: NodeId, update: EditorRuntimeUpdate, record_history: bool) -> bool {
        if update.stamp.session != (EditorSessionId { tree, node, session_gen: self.session_gen }) || update.stamp.rev != self.document_rev
        {
            return false;
        }
        let changed = match update.kind {
            Update::Select { selection, snapped } => {
                self.undo.merge = false;
                let changed = if let Some(composition) = &mut self.composition {
                    composition.selection.replace(selection) != Some(selection)
                } else {
                    std::mem::replace(&mut self.selection, selection) != selection
                };
                if snapped && !changed {
                    return true;
                }
                changed
            }
            Update::Compose(composition) => {
                self.undo.merge = false;
                let changed = match (&self.composition, &composition) {
                    (Some(old), Some(new)) => {
                        !old.text.shares_storage(&new.text)
                            || old.marked != new.marked
                            || old.selection != new.selection
                            || old.rich != new.rich
                    }
                    _ => self.composition.is_some() != composition.is_some(),
                };
                self.composition = composition;
                changed
            }
            Update::Accept { mut next, history } => {
                if !self.secure {
                    next.selection = next.rich.snap_selection(next.selection);
                }
                let changed_value = !self.value.shares_storage(&next.value);
                let changed_rich = self.rich != next.rich;
                if changed_value {
                    if record_history && !self.secure {
                        let before =
                            EditorVersion { value: self.value.without_flattened(), rich: self.rich.clone(), selection: self.selection };
                        self.undo.accept(history, before, &next);
                    }
                    self.set_value(next.value);
                } else if next.selection != self.selection || changed_rich {
                    self.undo.merge = false;
                }
                if changed_rich && !changed_value {
                    self.advance_value();
                }
                self.rich = next.rich;
                let changed_selection = std::mem::replace(&mut self.selection, next.selection) != next.selection;
                let composing = self.composition.take().is_some();
                changed_value || changed_rich || changed_selection || composing
            }
            Update::Reconcile => {
                self.undo.merge = false;
                if self.composition.take().is_some() {
                    self.advance_document();
                }
                return true;
            }
        };
        if changed {
            self.advance_document();
        }
        changed
    }
}

/// Read-only view of the currently focused editor. Proposals never mutate the tree.
pub struct EditorSession<'a> {
    id: EditorSessionId,
    slot: &'a EditorSlot,
}

impl<'a> EditorSession<'a> {
    pub fn active(tree: &'a Tree) -> Option<Self> {
        let node = tree.focus()?;
        if !tree.is_focusable(node) {
            return None;
        }
        let slot = tree.editor(node)?;
        Some(Self { id: EditorSessionId { tree: tree.id(), node, session_gen: slot.session_gen }, slot })
    }

    pub fn id(&self) -> EditorSessionId {
        self.id
    }

    pub fn editor(&self) -> &'a EditorSlot {
        self.slot
    }

    pub fn state(&self) -> Option<EditorState> {
        (!self.slot.is_secure()).then(|| EditorState {
            session: self.id,
            revision: self.slot.document_rev(),
            selection: self.slot.selection(),
            composing: self.slot.composition().is_some(),
        })
    }

    pub fn route(&self, event: EditorEvent) -> Result<EditorAction, EditorRejection> {
        let event = EventText(event);
        let event = &event.0;
        if event.session() != self.id {
            return Err(EditorRejection::WrongSession);
        }
        if event.expected_rev() != self.slot.document_rev {
            return Err(EditorRejection::StaleRevision);
        }
        match event {
            EditorEvent::ProposeEdit { edit, .. } => self.route_edit(edit),
            EditorEvent::ProposeSelection { selection, .. } => {
                validate_selection(self.slot.displayed_rope(), *selection)?;
                let proposed = *selection;
                let selection = self.slot.rich_text().map_or(proposed, |rich| rich.snap_selection(proposed));
                Ok(self.update(Update::Select { selection, snapped: proposed != selection }))
            }
            EditorEvent::Composition { text, selection, replacement, .. } => self.compose(text, *selection, replacement.clone()),
            EditorEvent::Commit { text, .. } => self.insert(text, None),
            EditorEvent::CancelComposition { .. } => Ok(self.update(Update::Compose(None))),
            EditorEvent::KeyIntercept { key, .. } => Ok(EditorAction::KeyIntercept(*key)),
            EditorEvent::KeyFallback { key, .. } => Ok(EditorAction::KeyFallback(*key)),
        }
    }

    pub fn undo(&self) -> Option<EditorProposal> {
        self.history_proposal(true)
    }

    pub fn redo(&self) -> Option<EditorProposal> {
        self.history_proposal(false)
    }

    fn history_proposal(&self, undo: bool) -> Option<EditorProposal> {
        if self.slot.secure {
            return None;
        }
        let step = if undo { self.slot.undo.undo.last()? } else { self.slot.undo.redo.last()? };
        let (next, history) = if undo { (step.before.clone(), HistoryEdit::Undo) } else { (step.after.clone(), HistoryEdit::Redo) };
        Some(EditorProposal { stamp: self.stamp(), next, history })
    }

    fn route_edit(&self, edit: &TextEdit) -> Result<EditorAction, EditorRejection> {
        match edit {
            TextEdit::Mark { text, selection, replacement } => {
                self.compose(text, selection.as_ref().map(|range| (range.start, range.end)), replacement.clone())
            }
            TextEdit::Insert { text, replacement } => self.insert(text, replacement.clone()),
            TextEdit::Commit { text, selection, kind } => self.commit(text, *selection, *kind),
            TextEdit::Unmark => {
                let composition = self.slot.composition.as_ref().ok_or(EditorRejection::NoComposition)?;
                let end = composition.marked.end;
                let selection = composition.selection.unwrap_or((end, end));
                Ok(self.propose(composition.text.clone(), composition.rich.clone(), selection, None))
            }
            TextEdit::Cancel => Ok(self.update(Update::Compose(None))),
        }
    }

    fn compose(
        &self,
        text: &str,
        selection: Option<(usize, usize)>,
        replacement: Option<Range<usize>>,
    ) -> Result<EditorAction, EditorRejection> {
        if text.is_empty() {
            return Ok(self.update(Update::Compose(None)));
        }
        if selection.is_some_and(|(a, b)| !text.is_char_boundary(a) || !text.is_char_boundary(b)) {
            return Err(EditorRejection::InvalidRange);
        }
        let range = self.replacement(replacement)?;
        let (value, rich) = self.replace(range.clone(), text)?;
        let marked = range.start..range.start + text.len();
        let selection = selection.map(|(anchor, caret)| (marked.start + anchor, marked.start + caret));
        Ok(self.update(Update::Compose(Some(Composition { text: value, rich, marked, selection }))))
    }

    fn insert(&self, text: &str, replacement: Option<Range<usize>>) -> Result<EditorAction, EditorRejection> {
        let range = self.replacement(replacement)?;
        let (value, rich) = self.replace(range.clone(), text)?;
        let end = range.start + text.len();
        let kind = if text.is_empty() { CommitKind::Deleting } else { CommitKind::Typing };
        Ok(self.propose(value, rich, (end, end), self.change(range, text.len(), kind)))
    }

    fn commit(&self, text: &str, selection: (usize, usize), kind: CommitKind) -> Result<EditorAction, EditorRejection> {
        if !text.is_char_boundary(selection.0) || !text.is_char_boundary(selection.1) {
            return Err(EditorRejection::InvalidRange);
        }
        let old = self.slot.displayed_rope().as_str();
        let old_selection = self.slot.composition.as_ref().and_then(Composition::selection).unwrap_or(self.slot.selection);
        let (start, old_end, new_end) = committed_change(old, text, old_selection, selection, kind);
        if kind == CommitKind::Deleting && start == new_end && start < old_end && old_selection.0 == old_selection.1 {
            let caret = old_selection.1;
            let backward = old_end == caret;
            if (backward || start == caret)
                && let Some(atom) = self.slot.rich_text().and_then(|rich| rich.adjacent_atom(caret, backward))
            {
                return self.insert("", Some(atom.range.clone()));
            }
        }
        let range = self.replacement(Some(start..old_end))?;
        let insert = &text[start..new_end];
        let selection = map_splice_selection(selection, start..old_end, new_end - start, &range);
        let (value, rich) = self.replace(range.clone(), insert)?;
        Ok(self.propose(value, rich, selection, self.change(range, insert.len(), kind)))
    }

    fn replacement(&self, explicit: Option<Range<usize>>) -> Result<Range<usize>, EditorRejection> {
        let range =
            explicit.unwrap_or_else(|| self.slot.composition.as_ref().map_or_else(|| ordered(self.slot.selection), Composition::marked));
        validate_selection(self.slot.displayed_rope(), (range.start, range.end))?;
        if range.start > range.end {
            return Err(EditorRejection::InvalidRange);
        }
        Ok(self.slot.rich_text().map_or_else(|| range.clone(), |rich| rich.expand_replacement(range.clone())))
    }

    fn replace(&self, range: Range<usize>, text: &str) -> Result<(EditorRope, EditorRichText), EditorRejection> {
        let previous = self.slot.displayed_rope();
        let value = previous.replaced(range.clone(), text).ok_or(EditorRejection::InvalidRange)?;
        let rich = self.slot.displayed_rich();
        let rich = if previous.shares_storage(&value) { rich.clone() } else { rich.after_replace(range, text.len()) };
        Ok((value, rich))
    }

    fn change(&self, range: Range<usize>, inserted: usize, kind: CommitKind) -> Option<Change> {
        (self.slot.composition.is_none()
            && kind != CommitKind::Other
            && self.slot.rich_text().is_none_or(|rich| !rich.intersects_atom(&range)))
        .then_some(Change { range, inserted, kind })
    }

    fn propose(&self, value: EditorRope, rich: EditorRichText, selection: (usize, usize), change: Option<Change>) -> EditorAction {
        let (value, rich) = if self.slot.composition.is_some() && value == self.slot.value {
            (self.slot.value.clone(), self.slot.rich.clone())
        } else {
            (value, rich)
        };
        let selection = if self.slot.secure { selection } else { rich.snap_selection(selection) };
        let next = EditorVersion { value, rich, selection };
        EditorAction::Propose(EditorProposal { stamp: self.stamp(), next, history: HistoryEdit::Record(change) })
    }

    fn stamp(&self) -> Stamp {
        Stamp { session: self.id, rev: self.slot.document_rev }
    }

    fn update(&self, kind: Update) -> EditorAction {
        EditorAction::Apply(EditorRuntimeUpdate { stamp: self.stamp(), kind })
    }
}

/// A host's speculative view of an editor while synchronous platform queries
/// run ahead of the retained tree. Cloning shares text and rich metadata.
#[doc(hidden)]
pub struct EditorMirror(EditorSlot);

impl EditorMirror {
    pub fn new(slot: &EditorSlot) -> Self {
        Self(EditorSlot {
            value: slot.value.clone(),
            rich: slot.rich.clone(),
            value_rev: slot.value_rev,
            document_rev: slot.document_rev,
            session_gen: slot.session_gen,
            composition: slot.composition.clone(),
            selection: slot.selection,
            undo: UndoHistory::default(),
            secure: slot.secure,
            placeholder: slot.placeholder.clone(),
            multiline: slot.multiline,
            rows: slot.rows,
            submit_on_enter: slot.submit_on_enter,
            revealed: slot.revealed,
        })
    }

    pub fn editor(&self) -> &EditorSlot {
        &self.0
    }

    pub fn apply(&mut self, event: EditorEvent) -> bool {
        let id = event.session();
        let rev = self.0.document_rev;
        let session = EditorSession { id, slot: &self.0 };
        let update = match session.route(event) {
            Ok(EditorAction::Apply(update)) => update,
            Ok(EditorAction::Propose(proposal)) => proposal.into_update(true),
            _ => return false,
        };
        self.0.apply_update_with_history(id.tree, id.node, update, false);
        self.0.document_rev != rev
    }
}

impl Clone for EditorMirror {
    fn clone(&self) -> Self {
        Self::new(&self.0)
    }
}

impl PartialEq for EditorMirror {
    fn eq(&self, other: &Self) -> bool {
        self.0.selection == other.0.selection
            && self.0.rich == other.0.rich
            && self.0.value == other.0.value
            && self.0.composition == other.0.composition
    }
}

struct EventText(EditorEvent);

impl Drop for EventText {
    fn drop(&mut self) {
        match &mut self.0 {
            EditorEvent::Composition { text, .. }
            | EditorEvent::Commit { text, .. }
            | EditorEvent::ProposeEdit {
                edit: TextEdit::Mark { text, .. } | TextEdit::Insert { text, .. } | TextEdit::Commit { text, .. },
                ..
            } => {
                wipe(text);
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorRejection {
    WrongSession,
    StaleRevision,
    InvalidRange,
    NoComposition,
}

pub enum EditorAction {
    Propose(EditorProposal),
    Apply(EditorRuntimeUpdate),
    KeyIntercept(KeyEvent),
    KeyFallback(KeyEvent),
}

impl fmt::Debug for EditorAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Propose(proposal) => f.debug_tuple("Propose").field(proposal).finish(),
            Self::Apply(update) => f.debug_tuple("Apply").field(update).finish(),
            Self::KeyIntercept(_) => f.debug_struct("KeyIntercept").finish_non_exhaustive(),
            Self::KeyFallback(_) => f.debug_struct("KeyFallback").finish_non_exhaustive(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Stamp {
    session: EditorSessionId,
    rev: u64,
}

/// A tentative edit to deliver to the application's input handler.
pub struct EditorProposal {
    stamp: Stamp,
    next: EditorVersion,
    history: HistoryEdit,
}

impl fmt::Debug for EditorProposal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditorProposal").field("stamp", &self.stamp).finish_non_exhaustive()
    }
}

impl EditorProposal {
    pub fn value(&self) -> &str {
        self.next.value.as_str()
    }

    pub fn selection(&self) -> (usize, usize) {
        self.next.selection
    }

    /// Attach styling for this proposal's logical value before settling the edit.
    pub fn set_rich_text(&mut self, rich: EditorRichText) {
        let rich = rich.validated(&self.next.value);
        if self.next.rich.atoms != rich.atoms
            && let HistoryEdit::Record(change) = &mut self.history
        {
            *change = None;
        }
        self.next.rich = rich;
    }

    /// Compares the proposal to a complete value without flattening its rope.
    pub fn matches_value(&self, value: &str) -> bool {
        self.next.value.range_eq(0..self.next.value.len(), value)
    }

    /// Compares to the retained buffer when the controlled value did not publish.
    #[doc(hidden)]
    pub fn matches_retained(&self, slot: &EditorSlot) -> bool {
        self.next.value.shares_storage(&slot.value)
    }

    /// Compare the proposal with the value provided by the application's binding after handling input.
    pub fn settle(self, bound_value: Option<&str>) -> EditorRuntimeUpdate {
        let accepted = bound_value.is_some_and(|value| self.matches_value(value));
        self.into_update(accepted)
    }

    /// Consumes a proposal after comparing its value to the active binding.
    #[doc(hidden)]
    pub fn into_update(self, accepted: bool) -> EditorRuntimeUpdate {
        let kind = if accepted { Update::Accept { next: self.next, history: self.history } } else { Update::Reconcile };
        EditorRuntimeUpdate { stamp: self.stamp, kind }
    }
}

/// A session- and revision-guarded update for `RuntimeOp::EditorApply`.
pub struct EditorRuntimeUpdate {
    stamp: Stamp,
    kind: Update,
}

impl fmt::Debug for EditorRuntimeUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EditorRuntimeUpdate").field("stamp", &self.stamp).finish_non_exhaustive()
    }
}

impl EditorRuntimeUpdate {
    pub fn session(&self) -> EditorSessionId {
        self.stamp.session
    }

    pub fn expected_rev(&self) -> u64 {
        self.stamp.rev
    }

    pub fn is_reconciliation(&self) -> bool {
        matches!(self.kind, Update::Reconcile)
    }
}

enum Update {
    Select { selection: (usize, usize), snapped: bool },
    Compose(Option<Composition>),
    Accept { next: EditorVersion, history: HistoryEdit },
    Reconcile,
}

enum HistoryEdit {
    Record(Option<Change>),
    Undo,
    Redo,
}

#[derive(Clone)]
struct EditorVersion {
    value: EditorRope,
    rich: EditorRichText,
    selection: (usize, usize),
}

impl EditorVersion {
    fn for_history(&self) -> Self {
        Self { value: self.value.without_flattened(), rich: self.rich.clone(), selection: self.selection }
    }
}

struct Change {
    range: Range<usize>,
    inserted: usize,
    kind: CommitKind,
}

impl Change {
    fn merge(&mut self, next: &Self) -> bool {
        if self.kind != next.kind {
            return false;
        }
        if self.range.is_empty() && next.range.is_empty() && self.range.start + self.inserted == next.range.start {
            self.inserted += next.inserted;
        } else if self.inserted == 0 && next.inserted == 0 && next.range.end == self.range.start {
            self.range.start = next.range.start;
        } else if self.inserted == 0 && next.inserted == 0 && self.range.start == next.range.start {
            self.range.end += next.range.len();
        } else {
            return false;
        }
        true
    }
}

#[derive(Default)]
struct UndoHistory {
    undo: Vec<Step>,
    redo: Vec<Step>,
    merge: bool,
}

impl UndoHistory {
    fn accept(&mut self, history: HistoryEdit, before: EditorVersion, after: &EditorVersion) {
        match history {
            HistoryEdit::Undo => {
                self.redo.extend(self.undo.pop());
                self.merge = false;
            }
            HistoryEdit::Redo => {
                self.undo.extend(self.redo.pop());
                self.merge = false;
            }
            HistoryEdit::Record(change) => {
                let merged = self.merge
                    && self.undo.last_mut().is_some_and(|last| {
                        if last.after.selection != before.selection
                            || !last.change.as_mut().zip(change.as_ref()).is_some_and(|(previous, next)| previous.merge(next))
                        {
                            return false;
                        }
                        last.after = after.for_history();
                        true
                    });
                self.merge = change.is_some();
                if !merged {
                    if self.undo.len() == UNDO_LIMIT {
                        self.undo.remove(0);
                    }
                    self.undo.push(Step { before, after: after.for_history(), change });
                }
                self.redo.clear();
            }
        }
    }
}

struct Step {
    before: EditorVersion,
    after: EditorVersion,
    change: Option<Change>,
}

fn ordered((anchor, caret): (usize, usize)) -> Range<usize> {
    anchor.min(caret)..anchor.max(caret)
}

fn validate_selection(text: &EditorRope, (anchor, caret): (usize, usize)) -> Result<(), EditorRejection> {
    if text.is_char_boundary(anchor) && text.is_char_boundary(caret) { Ok(()) } else { Err(EditorRejection::InvalidRange) }
}

// Native controls can send a whole document. Use the previous and new selections
// to disambiguate edits in repeated text before falling back to a common prefix/suffix.
fn committed_change(old: &str, new: &str, before: (usize, usize), after: (usize, usize), kind: CommitKind) -> (usize, usize, usize) {
    if old == new {
        return (old.len(), old.len(), old.len());
    }
    let selection = ordered(before);
    if after.0 == after.1 {
        let caret = after.1;
        let start = if selection.is_empty() && kind == CommitKind::Deleting { caret.min(selection.start) } else { selection.start };
        if caret >= start
            && let Some(old_end) = old.len().checked_sub(new.len() - caret)
            && old_end >= selection.end
            && old.get(..start) == new.get(..start)
            && old.get(old_end..) == new.get(caret..)
        {
            return (start, old_end, caret);
        }
    }
    change_bounds(old, new)
}

// Maps a native selection when its proposed splice is expanded to atom edges.
fn map_splice_selection(selection: (usize, usize), original: Range<usize>, inserted: usize, replaced: &Range<usize>) -> (usize, usize) {
    if original == *replaced {
        return selection;
    }
    let original_end = original.start + inserted;
    let map = |byte| {
        if (original.start..=original_end).contains(&byte) {
            return replaced.start + (byte - original.start);
        }
        let old = if byte < original.start { byte } else { original.end + (byte - original_end) };
        if old <= replaced.start {
            old
        } else if old >= replaced.end {
            replaced.start + inserted + (old - replaced.end)
        } else {
            replaced.start
        }
    };
    (map(selection.0), map(selection.1))
}

fn change_bounds(old: &str, new: &str) -> (usize, usize, usize) {
    let start = old.bytes().zip(new.bytes()).take_while(|(old, new)| old == new).count();
    let start = old.floor_char_boundary(start).min(new.floor_char_boundary(start));
    let mut suffix = old[start..].bytes().rev().zip(new[start..].bytes().rev()).take_while(|(old, new)| old == new).count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    (start, old.len() - suffix, new.len() - suffix)
}

fn map_selection(new: &str, (anchor, caret): (usize, usize), start: usize, old_end: usize, new_end: usize) -> (usize, usize) {
    let map = |offset| {
        let mapped = if offset <= start {
            offset
        } else if offset >= old_end {
            new_end + (offset - old_end)
        } else {
            new_end
        };
        if mapped == new.len() {
            return mapped;
        }
        new.grapheme_indices(true).take_while(|(offset, _)| *offset <= mapped).last().map_or(0, |(offset, _)| offset)
    };
    (map(anchor), map(caret))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NodeContent, NodeSpec, Retirement, RuntimeOp};

    const TREE: TreeId = TreeId::new([1, 2], 0);

    fn session(slot: &EditorSlot) -> EditorSession<'_> {
        EditorSession { id: EditorSessionId { tree: TREE, node: NodeId::default(), session_gen: slot.session_gen() }, slot }
    }

    fn update(slot: &mut EditorSlot, update: EditorRuntimeUpdate) -> bool {
        slot.apply_update(TREE, NodeId::default(), update)
    }

    fn route_edit(slot: &EditorSlot, edit: TextEdit) -> Result<EditorAction, EditorRejection> {
        let session = session(slot);
        session.route(EditorEvent::ProposeEdit { session: session.id(), expected_rev: slot.document_rev(), edit })
    }

    fn proposal(slot: &EditorSlot, edit: TextEdit) -> EditorProposal {
        let EditorAction::Propose(proposal) = route_edit(slot, edit).unwrap() else { panic!("expected a proposal") };
        proposal
    }

    fn select(slot: &mut EditorSlot, selection: (usize, usize)) {
        let session = session(slot);
        let EditorAction::Apply(change) =
            session.route(EditorEvent::ProposeSelection { session: session.id(), expected_rev: slot.document_rev(), selection }).unwrap()
        else {
            panic!("expected a selection update");
        };
        assert!(update(slot, change));
    }

    #[test]
    fn edit_acceptance_preserves_direction_and_rejects_other_bound_values() {
        let mut slot = EditorSlot::new("éx");
        select(&mut slot, (3, 0));
        let revision = slot.document_rev();
        assert_eq!(slot.value_rev(), 0);
        let edit = TextEdit::Commit { text: "日本".into(), selection: (6, 3), kind: CommitKind::Other };
        let rejected = proposal(&slot, edit.clone()).settle(Some("別"));
        assert!(rejected.is_reconciliation());
        assert!(update(&mut slot, rejected));
        assert_eq!(slot.document_rev(), revision);
        assert_eq!(slot.value_rev(), 0);
        assert_eq!(slot.value(), "éx");
        assert!(session(&slot).undo().is_none());

        let accepted = proposal(&slot, edit).settle(Some("日本"));
        assert!(update(&mut slot, accepted));
        assert_eq!(slot.document_rev(), revision + 1);
        assert_eq!(slot.value_rev(), 1);
        assert_eq!(slot.selection(), (6, 3));
        assert_eq!(slot.clipboard_text(), Some("本"));

        let undo = session(&slot).undo().unwrap().settle(Some("éx"));
        assert!(update(&mut slot, undo));
        assert_eq!(slot.value(), "éx");
        assert_eq!(slot.selection(), (3, 0));
        let redo = session(&slot).redo().unwrap().settle(Some("日本"));
        assert!(update(&mut slot, redo));
        assert_eq!(slot.selection(), (6, 3));
    }

    #[test]
    fn composition_remains_tentative_until_the_application_accepts_commit() {
        let mut slot = EditorSlot::new("abc");
        select(&mut slot, (2, 1));
        let old = session(&slot).id();
        let old_rev = slot.document_rev();
        let draft = route_edit(&slot, TextEdit::Mark { text: "中".into(), selection: Some(Range { start: 3, end: 0 }), replacement: None })
            .unwrap();
        let EditorAction::Apply(draft) = draft else { panic!("expected a composition update") };
        assert!(update(&mut slot, draft));
        assert_eq!(slot.value(), "abc");
        let composition = slot.composition().unwrap();
        assert_eq!(composition.text(), "a中c");
        assert_eq!(composition.marked(), 1..4);
        assert_eq!(composition.selection(), Some((4, 1)));
        let stale = session(&slot).route(EditorEvent::CancelComposition { session: old, expected_rev: old_rev });
        assert_eq!(stale.unwrap_err(), EditorRejection::StaleRevision);

        let commit = proposal(&slot, TextEdit::Unmark);
        assert_eq!(commit.value(), "a中c");
        assert_eq!(commit.selection(), (4, 1));
        assert!(update(&mut slot, commit.settle(None)));
        assert!(slot.composition().is_none());
        assert_eq!(slot.selection(), (2, 1));

        let accepted = proposal(&slot, TextEdit::Insert { text: "中".into(), replacement: None }).settle(Some("a中c"));
        assert!(update(&mut slot, accepted));
        assert_eq!(slot.value(), "a中c");
    }

    #[test]
    fn reset_rejects_delayed_editor_updates_even_with_a_matching_revision() {
        let mut slot = EditorSlot::new("hello");
        let id = session(&slot).id();
        let rev = slot.document_rev();
        let old = proposal(&slot, TextEdit::Insert { text: "!".into(), replacement: Some(5..5) }).settle(Some("hello!"));
        slot.reset_session();
        assert_eq!(slot.document_rev(), rev);
        assert!(!update(&mut slot, old));
        assert_eq!(slot.value(), "hello");
        assert_eq!(
            session(&slot).route(EditorEvent::CancelComposition { session: id, expected_rev: rev }).unwrap_err(),
            EditorRejection::WrongSession,
        );
        let id = session(&slot).id();
        assert!(slot.replace_value("new"));
        assert_ne!(session(&slot).id(), id);
    }

    #[test]
    fn secure_values_are_excluded_from_copy_history_and_debug() {
        let mut slot = EditorSlot::new("mypassword");
        assert!(slot.set_secure(true));
        slot.revealed = true;
        select(&mut slot, (10, 0));
        let edit = proposal(&slot, TextEdit::Insert { text: "newpassword".into(), replacement: None });
        assert!(!format!("{slot:?}{edit:?}").contains("password"));
        assert!(update(&mut slot, edit.settle(Some("newpassword"))));
        assert_eq!(slot.reportable_value(), None);
        assert_eq!(slot.clipboard_text(), None);
        assert!(session(&slot).undo().is_none());
    }

    fn accept(slot: &mut EditorSlot, proposal: EditorProposal) {
        let text = proposal.value().to_owned();
        assert!(update(slot, proposal.settle(Some(&text))));
    }

    #[test]
    fn atomic_delete_is_a_separate_undo_step_and_restores_metadata() {
        let mut slot = EditorSlot::new("A @Émile! ✅");
        let atom = AtomicRange { range: 2..9, kind: Arc::from("mention") };
        let decoration = (2..9, RichSpan { underline: true, ..Default::default() });
        slot.set_rich_text(EditorRichText {
            atoms: Arc::from([atom.clone()]),
            decorations: Arc::from([decoration.clone()]),
            ..Default::default()
        });
        select(&mut slot, (9, 9));
        let snapshot = slot.snapshot().unwrap();

        let deletion = proposal(&slot, TextEdit::Insert { text: String::new(), replacement: Some(8..9) });
        assert_eq!(deletion.selection(), (2, 2));
        accept(&mut slot, deletion);
        assert_eq!(slot.value(), "A ! ✅");
        assert!(slot.atoms().is_empty());
        let adjacent = proposal(&slot, TextEdit::Insert { text: String::new(), replacement: Some(1..2) });
        accept(&mut slot, adjacent);
        assert_eq!(slot.value(), "A! ✅");

        let undo_adjacent = session(&slot).undo().unwrap();
        accept(&mut slot, undo_adjacent);
        assert_eq!(slot.value(), "A ! ✅");
        let undo_atom = session(&slot).undo().unwrap();
        accept(&mut slot, undo_atom);
        assert_eq!(slot.value(), snapshot.as_str());
        assert_eq!(slot.atoms(), &[atom]);
        assert_eq!(slot.decorations(), &[decoration]);
        assert_eq!(slot.selection(), (9, 9));

        let redo = session(&slot).redo().unwrap();
        accept(&mut slot, redo);
        assert_eq!(slot.value(), "A ! ✅");
        assert!(slot.atoms().is_empty());
    }

    #[test]
    fn selection_and_native_commits_respect_atoms_in_repeated_text() {
        let mut slot = EditorSlot::new("aaaaa");
        slot.set_rich_text(EditorRichText {
            atoms: Arc::from([AtomicRange { range: 1..4, kind: Arc::from("token") }]),
            ..Default::default()
        });
        select(&mut slot, (3, 3));
        assert_eq!(slot.selection(), (4, 4));
        select(&mut slot, (5, 2));
        assert_eq!(slot.selection(), (5, 1));
        select(&mut slot, (4, 4));

        let deleted = proposal(&slot, TextEdit::Commit { text: "aaaa".into(), selection: (3, 3), kind: CommitKind::Deleting });
        assert_eq!(deleted.value(), "aa");
        assert_eq!(deleted.selection(), (1, 1));
        accept(&mut slot, deleted);

        let undo = session(&slot).undo().unwrap();
        accept(&mut slot, undo);
        select(&mut slot, (1, 1));
        let forward = proposal(&slot, TextEdit::Commit { text: "aaaa".into(), selection: (1, 1), kind: CommitKind::Deleting });
        assert_eq!(forward.value(), "aa");
        assert_eq!(forward.selection(), (1, 1));
    }

    #[test]
    fn native_deletion_does_not_remove_text_outside_an_adjacent_atom() {
        let mut slot = EditorSlot::new("áZ");
        slot.set_rich_text(EditorRichText {
            atoms: Arc::from([AtomicRange { range: 1..3, kind: Arc::from("mark") }]),
            ..Default::default()
        });
        select(&mut slot, (3, 3));
        let deletion = proposal(&slot, TextEdit::Commit { text: "Z".into(), selection: (0, 0), kind: CommitKind::Deleting });
        assert_eq!(deletion.value(), "aZ");
        assert_eq!(deletion.selection(), (1, 1));
    }

    #[test]
    fn composition_remaps_atoms_and_secure_mode_hides_rich_metadata() {
        let mut slot = EditorSlot::new("abc @xyz");
        slot.set_rich_text(EditorRichText {
            atoms: Arc::from([AtomicRange { range: 4..8, kind: Arc::from("token") }]),
            ..Default::default()
        });
        let EditorAction::Apply(draft) =
            route_edit(&slot, TextEdit::Mark { text: "😀".into(), selection: Some(4..4), replacement: Some(1..2) }).unwrap()
        else {
            panic!("expected a composition update")
        };
        assert!(update(&mut slot, draft));
        assert_eq!(slot.displayed_rope().as_str(), "a😀c @xyz");
        assert_eq!(slot.atoms()[0].range, 7..11);
        assert_eq!(slot.displayed_rope().byte_to_utf16(7), Some(5));

        assert!(slot.set_rich_text(EditorRichText {
            atoms: Arc::from([AtomicRange { range: 4..8, kind: Arc::from("updated") }]),
            ..Default::default()
        }));
        assert_eq!(slot.composition().unwrap().text(), "a😀c @xyz");
        assert_eq!(slot.atoms()[0].kind.as_ref(), "updated");
        assert_eq!(slot.atoms()[0].range, 7..11);

        assert!(slot.set_secure(true));
        assert!(slot.snapshot().is_none());
        assert!(!slot.is_rich());
        assert!(slot.atoms().is_empty());
        assert!(slot.composition().is_none());
    }

    #[test]
    fn malformed_ranges_cannot_panic_or_split_utf8() {
        let slot = EditorSlot::new("日本");
        for replacement in [Some(1..3), Some(usize::MAX..usize::MAX)] {
            let bad = route_edit(&slot, TextEdit::Insert { text: "き".into(), replacement });
            assert_eq!(bad.unwrap_err(), EditorRejection::InvalidRange);
        }
    }

    #[test]
    fn reorder_keeps_composition_and_focus_reentry_or_retirement_invalidates_it() {
        let mut tree = Tree::new(TREE, Default::default());
        let root = tree.node_ref(tree.root());
        let mut tx = tree.transaction();
        let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("note")))));
        let sibling = tx.create(NodeSpec::new(NodeContent::Box));
        tx.place(root, 0, editor);
        tx.place(root, 1, sibling);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let node = receipt.node(editor).unwrap();
        let sibling = receipt.node(sibling).unwrap();
        let node_ref = tree.node_ref(node);
        tree.apply_runtime(RuntimeOp::SetFocus { node: Some(node), visible: false });

        let session = EditorSession::active(&tree).unwrap();
        let id = session.id();
        let EditorAction::Apply(draft) = session
            .route(EditorEvent::Composition {
                session: id,
                expected_rev: session.editor().document_rev(),
                text: "文".into(),
                selection: Some((3, 0)),
                replacement: Some(0..0),
            })
            .unwrap()
        else {
            panic!("expected a composition update");
        };
        assert!(tree.apply_runtime(RuntimeOp::EditorApply { node, update: draft }).editor_dirty.contains(&node));
        let draft_rev = tree.editor(node).unwrap().document_rev();
        let mut tx = tree.transaction();
        tx.place(root, 1, node_ref);
        tree.apply(tx.finish()).unwrap();
        assert_eq!(tree.children(tree.root()), [sibling, node, tree.overlay_root()]);

        let session = EditorSession::active(&tree).unwrap();
        assert_eq!(session.id(), id);
        assert_eq!(session.editor().document_rev(), draft_rev);
        assert_eq!(session.editor().composition().unwrap().text(), "文note");
        let EditorAction::Propose(commit) =
            session.route(EditorEvent::Commit { session: id, expected_rev: draft_rev, text: "文".into() }).unwrap()
        else {
            panic!("expected a commit proposal");
        };
        let late = commit.settle(Some("文note"));

        tree.apply_runtime(RuntimeOp::SetFocus { node: None, visible: false });
        tree.apply_runtime(RuntimeOp::SetFocus { node: Some(node), visible: false });
        let session = EditorSession::active(&tree).unwrap();
        assert_ne!(session.id(), id);
        assert!(session.editor().composition().is_none());
        assert!(tree.apply_runtime(RuntimeOp::EditorApply { node, update: late }).editor_dirty.is_empty());
        assert_eq!(tree.editor(node).unwrap().value(), "note");

        let session = EditorSession::active(&tree).unwrap();
        let EditorAction::Apply(late) = session
            .route(EditorEvent::Composition {
                session: session.id(),
                expected_rev: session.editor().document_rev(),
                text: "文".into(),
                selection: None,
                replacement: None,
            })
            .unwrap()
        else {
            panic!("expected a composition update");
        };
        let mut tx = tree.transaction();
        tx.retire(node_ref, Retirement::Exit);
        tree.apply(tx.finish()).unwrap();
        assert!(EditorSession::active(&tree).is_none());
        assert!(tree.apply_runtime(RuntimeOp::EditorApply { node, update: late }).editor_dirty.is_empty());
    }
}
