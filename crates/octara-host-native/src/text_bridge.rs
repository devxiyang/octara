//! Shared prediction, document queries, and reconciliation for native text protocols.

use crate::os::ActiveEditorView;
use octara::EditorRope;
use octara::runtime::Input;
use octara::tree::editor::{EditorMirror, EditorSlot};
#[cfg(any(target_os = "macos", target_os = "ios", test))]
use octara_model::geometry::Point;
use octara_model::geometry::Rect;
use octara_model::identity::EditorSessionId;
#[cfg(any(target_os = "macos", target_os = "ios"))]
use octara_model::input::EditorCommand;
use octara_model::input::EditorEvent;
#[cfg(any(target_os = "macos", target_os = "ios", test))]
use octara_model::text::TextLine;
use octara_model::text::{InputTraits, TextEdit, TextGeometry};
use std::collections::VecDeque;
use std::ops::Range;

#[derive(Default)]
pub(crate) struct HostTextBridge {
    session: Option<BridgeSession>,
    committed: Option<EditorMirror>,
    pending: VecDeque<Proposal>,
    predicted: VecDeque<(u64, EditorMirror)>,
    submitted: Option<Submitted>,
}

struct Proposal {
    event: EditorEvent,
    prediction: Option<u64>,
}

struct Submitted {
    prediction: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SyncOutcome {
    pub session_changed: bool,
    pub rejected: bool,
    pub text_changed: bool,
    pub selection_changed: bool,
    pub composition_changed: bool,
}

#[derive(Clone, Copy)]
enum Reconcile {
    Keep,
    Accept,
    Restyled,
    Replace,
    Reject,
}

pub(crate) struct BridgeSession {
    pub id: EditorSessionId,
    pub caret: Option<Rect>,
    pub geometry: Option<TextGeometry>,
    pub multiline: bool,
    pub secure: bool,
    pub traits: InputTraits,
    document: EditorMirror,
}

#[cfg(any(target_os = "ios", test))]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SelectionBox {
    pub range: Range<usize>,
    pub rect: Rect,
    pub rtl: bool,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SurroundingText<'a> {
    pub text: &'a str,
    pub cursor: usize,
    pub anchor: usize,
    pub start: usize,
}

impl HostTextBridge {
    pub(crate) fn session(&self) -> Option<&BridgeSession> {
        self.session.as_ref()
    }

    pub(crate) fn stamp(&self) -> Option<(EditorSessionId, u64)> {
        let session = self.session()?;
        Some((session.id, session.revision()))
    }

    pub(crate) fn is_composing(&self) -> bool {
        self.session().is_some_and(|session| session.editor().composition().is_some())
    }

    pub(crate) fn edit(&mut self, edit: TextEdit) -> bool {
        self.stamp().is_some_and(|(session, expected_rev)| self.propose(EditorEvent::Edit { session, expected_rev, edit }, false))
    }

    #[cfg(any(target_os = "macos", target_os = "ios", test))]
    pub(crate) fn select(&mut self, selection: (usize, usize)) -> bool {
        self.stamp().is_some_and(|(session, expected_rev)| self.propose(EditorEvent::Select { session, expected_rev, selection }, false))
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub(crate) fn command(&mut self, command: EditorCommand) -> bool {
        self.stamp().is_some_and(|(session, expected_rev)| self.propose(EditorEvent::Command { session, expected_rev, command }, true))
    }

    fn propose(&mut self, event: EditorEvent, keep_without_prediction: bool) -> bool {
        let Some(session) = &mut self.session else { return false };
        let displayed = session.rope().clone();
        let advanced = session.document.apply_with_geometry(event.clone(), session.geometry.as_ref());
        if !advanced && !keep_without_prediction {
            return false;
        }
        let prediction = advanced.then(|| {
            if session.rope() != &displayed {
                session.geometry = None;
            }
            let revision = session.revision();
            self.predicted.push_back((revision, session.document.clone()));
            revision
        });
        self.pending.push_back(Proposal { event, prediction });
        true
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Submit one event and wait for the retained revision before offering the next.
    pub(crate) fn take(&mut self) -> Option<Input> {
        if self.submitted.is_some() {
            return None;
        }
        let proposal = self.pending.pop_front()?;
        self.submitted = Some(Submitted { prediction: proposal.prediction });
        Some(Input::Editor(proposal.event))
    }

    pub(crate) fn drain(&mut self, out: &mut Vec<Input>) {
        out.extend(self.take());
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Preview the notifications before a platform delegate can query the old display.
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn sync_changes(&self, active: Option<&ActiveEditorView<'_>>) -> SyncOutcome {
        let next = active.map(BridgeSession::new);
        let reconcile = self.reconcile(next.as_ref());
        if matches!(reconcile, Reconcile::Restyled) {
            let rebased = self.rebase(next.expect("restyled active editor"));
            return self.sync_outcome(rebased.session(), false);
        }
        self.sync_outcome(self.visible_after_sync(next.as_ref(), reconcile), matches!(reconcile, Reconcile::Reject))
    }

    pub(crate) fn sync(&mut self, active: Option<&ActiveEditorView<'_>>) -> SyncOutcome {
        let next = active.map(BridgeSession::new);
        let reconcile = self.reconcile(next.as_ref());
        if matches!(reconcile, Reconcile::Restyled) {
            let rebased = self.rebase(next.expect("restyled active editor"));
            let outcome = self.sync_outcome(rebased.session(), false);
            *self = rebased;
            return outcome;
        }
        let outcome = self.sync_outcome(self.visible_after_sync(next.as_ref(), reconcile), matches!(reconcile, Reconcile::Reject));
        self.submitted = None;
        match reconcile {
            Reconcile::Keep | Reconcile::Accept => {
                let next = next.expect("reconciliation keeps an active editor");
                let session = self.session.as_mut().expect("active editor");
                if matches!(reconcile, Reconcile::Accept) {
                    self.predicted.pop_front();
                    self.committed = Some(next.document.clone());
                }
                session.caret = next.caret;
                session.multiline = next.multiline;
                session.secure = next.secure;
                session.traits = next.traits;
                if self.predicted.is_empty() {
                    session.document = next.document;
                    session.geometry = next.geometry;
                }
            }
            Reconcile::Replace | Reconcile::Reject => {
                self.pending.clear();
                self.predicted.clear();
                self.committed = next.as_ref().map(|next| next.document.clone());
                self.session = next;
            }
            Reconcile::Restyled => unreachable!(),
        }
        outcome
    }

    fn rebase(&self, next: BridgeSession) -> Self {
        let committed = Some(next.document.clone());
        let mut rebased = Self { session: Some(next), committed, ..Self::default() };
        let mut predictions = self.predicted.iter().skip(1);
        for proposal in &self.pending {
            let event = &proposal.event;
            if rebased.stamp() != Some((event.session(), event.expected_rev())) {
                break;
            }
            let retained = rebased.propose(event.clone(), matches!(event, EditorEvent::Command { .. }));
            if !retained {
                break;
            }
            if proposal.prediction.is_some() {
                let compatible = predictions
                    .next()
                    .zip(rebased.predicted.back())
                    .is_some_and(|((rev, old), (new_rev, new))| rev == new_rev && old.same_editing_state(new));
                // Later proposals may contain byte offsets into the original speculative document.
                if !compatible {
                    break;
                }
            }
        }
        rebased
    }

    fn reconcile(&self, next: Option<&BridgeSession>) -> Reconcile {
        let (Some(current), Some(next)) = (self.session(), next) else { return Reconcile::Replace };
        if current.id != next.id {
            return Reconcile::Replace;
        }
        if let Some(expected) = self.submitted.as_ref().and_then(|event| event.prediction) {
            let Some((_, predicted)) = self.predicted.front().filter(|(rev, _)| *rev == expected && next.revision() == expected) else {
                return Reconcile::Reject;
            };
            if predicted == &next.document {
                return Reconcile::Accept;
            }
            return if predicted.same_editing_state(&next.document) { Reconcile::Restyled } else { Reconcile::Reject };
        }
        if self.committed.as_ref().is_some_and(|mirror| mirror.editor().document_rev() == next.revision() && mirror == &next.document) {
            Reconcile::Keep
        } else {
            Reconcile::Replace
        }
    }

    fn visible_after_sync<'a>(&'a self, next: Option<&'a BridgeSession>, reconcile: Reconcile) -> Option<&'a BridgeSession> {
        match reconcile {
            Reconcile::Keep | Reconcile::Accept => self.session(),
            Reconcile::Replace | Reconcile::Reject => next,
            Reconcile::Restyled => unreachable!(),
        }
    }

    fn sync_outcome(&self, visible: Option<&BridgeSession>, rejected: bool) -> SyncOutcome {
        let current = self.session();
        SyncOutcome {
            session_changed: current.map(|session| session.id) != visible.map(|session| session.id),
            rejected,
            text_changed: current.map(BridgeSession::rope) != visible.map(BridgeSession::rope),
            selection_changed: current.and_then(BridgeSession::selection) != visible.and_then(BridgeSession::selection),
            composition_changed: current.and_then(BridgeSession::marked_range) != visible.and_then(BridgeSession::marked_range),
        }
    }
}

impl BridgeSession {
    fn new(active: &ActiveEditorView<'_>) -> Self {
        Self {
            id: active.session,
            caret: active.caret,
            geometry: active.geometry.cloned(),
            multiline: active.multiline,
            secure: active.secure,
            traits: active.editor.input_traits,
            document: EditorMirror::new(active.editor),
        }
    }

    pub(crate) fn revision(&self) -> u64 {
        self.editor().document_rev()
    }

    pub(crate) fn editor(&self) -> &EditorSlot {
        self.document.editor()
    }

    pub(crate) fn rope(&self) -> &EditorRope {
        self.editor().displayed_rope()
    }

    pub(crate) fn selection(&self) -> Option<(usize, usize)> {
        self.editor().displayed_selection()
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub(crate) fn selected_range(&self) -> Option<Range<usize>> {
        self.selection().map(|(anchor, caret)| anchor.min(caret)..anchor.max(caret))
    }

    pub(crate) fn marked_range(&self) -> Option<Range<usize>> {
        self.editor().composition().map(|composition| composition.marked())
    }

    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn surrounding_text(&self, max_bytes: usize) -> Option<SurroundingText<'_>> {
        if self.secure {
            return None;
        }
        Some(surrounding(self.editor().rope().as_str(), self.editor().selection(), max_bytes))
    }

    #[cfg(any(target_os = "macos", target_os = "ios", test))]
    pub(crate) fn first_rect(&self, range: Range<usize>) -> Option<(Range<usize>, Rect)> {
        let caret = self.selection().map(|(_, caret)| caret);
        let fallback = || self.caret.map(|rect| (caret.unwrap_or(range.start)..caret.unwrap_or(range.start), rect));
        let measured = || self.geometry.as_ref().and_then(|geometry| first_rect(geometry, range.clone()));
        if range.is_empty() && caret == Some(range.start) { fallback().or_else(measured) } else { measured().or_else(fallback) }
    }

    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn selection_rects(&self, range: Range<usize>) -> Vec<SelectionBox> {
        let Some(geometry) = &self.geometry else { return Vec::new() };
        geometry
            .lines
            .iter()
            .flat_map(|line| line.fragments.iter())
            .filter(|fragment| range.start < fragment.range.end && fragment.range.start < range.end)
            .map(|fragment| SelectionBox {
                range: fragment.range.clone(),
                rect: offset_rect(fragment.rect, geometry.origin),
                rtl: fragment.rtl,
            })
            .collect()
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn character_index_at(&self, point: Point) -> Option<usize> {
        self.hit_index(point, false)
    }

    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn closest_character_index_at(&self, point: Point) -> Option<usize> {
        self.hit_index(point, true)
    }

    #[cfg(any(target_os = "macos", target_os = "ios", test))]
    fn hit_index(&self, point: Point, closest: bool) -> Option<usize> {
        let geometry = self.geometry.as_ref()?;
        let y = point.y - geometry.origin.y;
        let x = point.x - geometry.origin.x;
        let line = if closest {
            let distance = |line: &TextLine| (line.rect.y - y).max(y - line.rect.bottom()).max(0.0);
            geometry.lines.iter().min_by(|left, right| distance(left).total_cmp(&distance(right)))?
        } else {
            geometry.lines.iter().find(|line| line.rect.y <= y && y < line.rect.bottom())?
        };
        Some(
            line.fragments
                .iter()
                .flat_map(|fragment| {
                    let left = (fragment.rect.x, if fragment.rtl { fragment.range.end } else { fragment.range.start });
                    let right = (fragment.rect.right(), if fragment.rtl { fragment.range.start } else { fragment.range.end });
                    [left, right]
                })
                .min_by(|(left, _), (right, _)| (x - left).abs().total_cmp(&(x - right).abs()))
                .map_or(line.range.start, |(_, index)| index),
        )
    }
}

#[cfg(any(target_os = "ios", test))]
pub(crate) fn utf16_offset(rope: &EditorRope, offset: usize) -> usize {
    rope.utf16_byte_bounds(offset).map_or_else(|| rope.len(), |(floor, _)| floor)
}

#[cfg(any(target_os = "macos", target_os = "ios", test))]
pub(crate) fn utf16_range(rope: &EditorRope, range: Range<usize>) -> Option<Range<usize>> {
    if range.end < range.start {
        return None;
    }
    let (start, _) = rope.utf16_byte_bounds(range.start)?;
    let end = if range.is_empty() { start } else { rope.utf16_byte_bounds(range.end).map_or_else(|| rope.len(), |(_, ceil)| ceil) };
    Some(start..end)
}

#[cfg(any(target_os = "macos", target_os = "ios", test))]
pub(crate) fn native_range(rope: &EditorRope, range: Range<usize>) -> Option<Range<usize>> {
    Some(rope.byte_to_utf16(range.start)?..rope.byte_to_utf16(range.end)?)
}

#[cfg(any(target_os = "linux", test))]
fn surrounding(text: &str, (mut anchor, cursor): (usize, usize), max_bytes: usize) -> SurroundingText<'_> {
    let limit = max_bytes.min(4000);
    if anchor.abs_diff(cursor) > limit {
        // Retain trustworthy cursor context when the selection is wider than one protocol message.
        anchor = cursor;
    }
    let low = anchor.min(cursor);
    let high = anchor.max(cursor);
    let center = low + (high - low) / 2;
    let ideal = center.saturating_sub(limit / 2).min(text.len().saturating_sub(limit));
    let start = text.ceil_char_boundary(ideal.clamp(high.saturating_sub(limit), low));
    let end = text.floor_char_boundary(start.saturating_add(limit));
    SurroundingText { text: &text[start..end], cursor: cursor - start, anchor: anchor - start, start }
}

#[cfg(any(target_os = "macos", target_os = "ios", test))]
fn first_rect(geometry: &TextGeometry, range: Range<usize>) -> Option<(Range<usize>, Rect)> {
    let line = geometry.lines.iter().rfind(|line| line.range.start <= range.start)?;
    let mut covered = range.start..range.start;
    let mut rect = Rect::new(caret_x(line, range.start), line.rect.y, 0.0, line.rect.h);
    if !range.is_empty() {
        let mut fragments = line.fragments.iter().filter(|fragment| range.start < fragment.range.end && fragment.range.start < range.end);
        if let Some(first) = fragments.next() {
            covered = first.range.clone();
            rect = first.rect;
            for fragment in fragments {
                covered = covered.start.min(fragment.range.start)..covered.end.max(fragment.range.end);
                let left = rect.x.min(fragment.rect.x);
                rect = Rect::new(left, line.rect.y, rect.right().max(fragment.rect.right()) - left, line.rect.h);
            }
        }
    }
    Some((covered, offset_rect(rect, geometry.origin)))
}

#[cfg(any(target_os = "macos", target_os = "ios", test))]
fn caret_x(line: &TextLine, byte: usize) -> f32 {
    line.fragments
        .iter()
        .find(|fragment| fragment.range.contains(&byte))
        .or_else(|| line.fragments.iter().filter(|fragment| fragment.range.end <= byte).max_by_key(|fragment| fragment.range.end))
        .map_or(line.rect.x, |fragment| if (byte >= fragment.range.end) != fragment.rtl { fragment.rect.right() } else { fragment.rect.x })
}

#[cfg(any(target_os = "macos", target_os = "ios", test))]
fn offset_rect(rect: Rect, offset: Point) -> Rect {
    Rect { x: rect.x + offset.x, y: rect.y + offset.y, ..rect }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::identity::{NodeId, TreeId};

    fn view(editor: &EditorSlot) -> ActiveEditorView<'_> {
        ActiveEditorView {
            session: EditorSessionId { tree: TreeId::new([19, 4], 1), node: NodeId::default(), session_gen: editor.session_gen() },
            document_rev: editor.document_rev(),
            value: editor.value(),
            selection: editor.selection(),
            composition: editor.composition(),
            editor,
            multiline: editor.multiline,
            secure: editor.is_secure(),
            caret: None,
            geometry: None,
        }
    }

    fn insert(text: &str) -> TextEdit {
        TextEdit::Insert { text: text.into(), replacement: None }
    }

    #[test]
    fn predictions_keep_the_unsent_suffix_after_an_accepted_prefix() {
        let initial = EditorSlot::new("");
        let mut retained = EditorMirror::new(&initial);
        let mut bridge = HostTextBridge::default();
        bridge.sync(Some(&view(retained.editor())));
        assert!(bridge.edit(insert("A")));
        assert!(bridge.edit(insert("😀")));
        let displayed = |bridge: &HostTextBridge| bridge.session().unwrap().rope().as_str().to_owned();
        assert_eq!(displayed(&bridge), "A😀");
        assert_eq!(bridge.sync(Some(&self::view(retained.editor()))), SyncOutcome::default());
        let Some(Input::Editor(first)) = bridge.take() else { panic!("missing prefix") };
        assert!(bridge.take().is_none());
        assert!(retained.apply_with_geometry(first, None));
        assert_eq!(bridge.sync(Some(&self::view(retained.editor()))), SyncOutcome::default());
        assert_eq!(displayed(&bridge), "A😀");
        let Some(Input::Editor(second)) = bridge.take() else { panic!("missing suffix") };
        assert!(retained.apply_with_geometry(second, None));
        assert_eq!(bridge.sync(Some(&self::view(retained.editor()))), SyncOutcome::default());
        assert_eq!(retained.editor().value(), "A😀");
    }

    #[test]
    fn restyling_an_accepted_edit_preserves_the_queued_native_suffix() {
        use octara::tree::editor::{EditorAction, EditorRichText, EditorSession};
        use octara::tree::{NodeContent, NodeSpec, RuntimeOp, Tree};
        use octara_model::text::RichSpan;

        let mut tree = Tree::new(TreeId::new([19, 4], 1), Default::default());
        let root = tree.node_ref(tree.root());
        let mut tx = tree.transaction();
        let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::default())));
        tx.place(root, 0, editor);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let node = receipt.node(editor).unwrap();
        tree.apply_runtime(RuntimeOp::SetFocus { node: Some(node), visible: false });
        let editor = EditorSession::active(&tree).unwrap();
        let mut active = view(editor.editor());
        active.session = editor.id();

        let mut bridge = HostTextBridge::default();
        bridge.sync(Some(&active));
        assert!(bridge.edit(insert("A")));
        assert!(bridge.edit(insert("😀")));
        let Some(Input::Editor(first)) = bridge.take() else { panic!("missing first proposal") };
        let EditorAction::Propose(mut first) = editor.route(first).unwrap() else { panic!("expected an edit proposal") };
        first.set_rich_text(EditorRichText {
            decorations: [(0..1, RichSpan { underline: true, ..Default::default() })].into(),
            ..Default::default()
        });
        tree.apply_runtime(RuntimeOp::EditorApply { node, update: first.into_update(true) });

        let editor = EditorSession::active(&tree).unwrap();
        let mut active = view(editor.editor());
        active.session = editor.id();
        assert_eq!(bridge.sync_changes(Some(&active)), SyncOutcome::default());
        assert_eq!(bridge.sync(Some(&active)), SyncOutcome::default());
        let predicted = bridge.session().unwrap().editor();
        assert_eq!(predicted.value(), "A😀");
        assert_eq!(predicted.decorations().len(), 1);
        let Some(Input::Editor(second)) = bridge.take() else { panic!("missing rebased proposal") };
        let EditorAction::Propose(second) = editor.route(second).unwrap() else { panic!("expected an edit proposal") };
        tree.apply_runtime(RuntimeOp::EditorApply { node, update: second.into_update(true) });

        let editor = EditorSession::active(&tree).unwrap();
        let mut active = view(editor.editor());
        active.session = editor.id();
        assert_eq!(bridge.sync(Some(&active)), SyncOutcome::default());
        assert_eq!(active.value, "A😀");
        assert_eq!(active.editor.decorations().len(), 1);
    }

    #[test]
    fn rejection_restores_authority_and_discards_dependent_edits() {
        let retained = EditorSlot::new("before");
        let mut bridge = HostTextBridge::default();
        let active = view(&retained);
        bridge.sync(Some(&active));
        assert!(bridge.edit(insert("X")));
        assert!(bridge.edit(insert("Y")));
        assert!(bridge.take().is_some());
        let planned = bridge.sync_changes(Some(&active));
        assert!(planned.rejected && planned.text_changed);
        assert_ne!(bridge.session().unwrap().rope().as_str(), "before");
        assert_eq!(bridge.sync(Some(&active)), planned);
        assert_eq!(bridge.session().unwrap().rope().as_str(), "before");
        assert!(bridge.take().is_none());
    }

    #[test]
    fn utf16_ranges_preserve_surrogate_boundaries() {
        let rope = EditorRope::new("A😀界Z");
        assert_eq!(utf16_range(&rope, 1..3), Some(1..5));
        assert_eq!(native_range(&rope, 1..5), Some(1..3));
        assert_eq!(utf16_range(&rope, 2..2), Some(1..1));
        assert_eq!(utf16_range(&rope, 2..3), Some(1..5));
        assert_eq!(utf16_offset(&rope, 3), 5);
        assert_eq!(utf16_offset(&rope, 9), rope.len());
        assert_eq!(utf16_range(&rope, 7..9), None);
    }

    #[test]
    fn shared_geometry_reports_visual_emoji_bounds_and_directional_selection() {
        use octara_model::text::TextFragment;

        let retained = EditorSlot::new("😀A");
        let mut active = view(&retained);
        let geometry = TextGeometry {
            origin: Point::new(5.0, 7.0),
            lines: [TextLine {
                range: 0..5,
                rect: Rect::new(0.0, 0.0, 30.0, 12.0),
                glyph_top: 0.0,
                glyph_height: 12.0,
                fragments: [
                    TextFragment { range: 0..4, rect: Rect::new(0.0, 0.0, 20.0, 12.0), rtl: false },
                    TextFragment { range: 4..5, rect: Rect::new(20.0, 0.0, 10.0, 12.0), rtl: false },
                ]
                .into(),
            }]
            .into(),
        };
        active.geometry = Some(&geometry);
        let mut bridge = HostTextBridge::default();
        bridge.sync(Some(&active));
        let session = bridge.session().unwrap();
        let rect = Rect::new(5.0, 7.0, 20.0, 12.0);
        assert_eq!(session.first_rect(0..4), Some((0..4, rect)));
        assert_eq!(session.selection_rects(0..4), [SelectionBox { range: 0..4, rect, rtl: false }]);
        assert_eq!(session.character_index_at(Point::new(23.0, 12.0)), Some(4));
        assert_eq!(session.closest_character_index_at(Point::new(23.0, -100.0)), Some(4));
        assert_eq!(session.character_index_at(Point::new(23.0, -100.0)), None);
        assert!(bridge.select((4, 0)));
        assert_eq!(bridge.session().and_then(BridgeSession::selection), Some((4, 0)));
    }

    #[test]
    fn surrounding_text_retains_a_valid_bounded_utf8_window() {
        let editor = EditorSlot::new("a😀z");
        let active = view(&editor);
        let session = BridgeSession::new(&active);
        assert!(session.surrounding_text(4000).unwrap().text.len() <= 4000);
        let text = "😀語".repeat(1600);
        for selection in [(0, 0), (4620, 4624), (11_200, 11_200), (11_200, 0), (7700, 3850)] {
            let view = surrounding(&text, selection, usize::MAX);
            assert!(view.text.len() <= 4000);
            assert!(view.text.is_char_boundary(view.cursor) && view.text.is_char_boundary(view.anchor));
            assert_eq!(view.start + view.cursor, selection.1);
            if selection.0.abs_diff(selection.1) <= 4000 {
                assert_eq!(view.start + view.anchor, selection.0);
            } else {
                assert_eq!(view.cursor, view.anchor);
            }
        }
    }
}
