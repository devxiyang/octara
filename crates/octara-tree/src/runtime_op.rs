//! Writes to transient state. Hosts and the interaction arbiter produce these operations.

use crate::editor::EditorRuntimeUpdate;
use crate::selection::DocSelection;
use crate::tables::ScrollSlot;
use crate::{ExitTicket, GeometryDemand, NodeFlags, NodeId, Tree, TreeDelta};
use octara_model::geometry::{Placed, Vec2};
use octara_model::image::{ImageState, MediaAsset};
use octara_model::style::{State, When};
use octara_model::theme::{Breakpoint, ResponsiveBreakpoint, Theme};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

#[derive(Debug)]
pub enum RuntimeOp {
    SetState { node: NodeId, state: State, on: bool },
    SetFocus { node: Option<NodeId>, visible: bool },
    SetScroll { node: NodeId, offset: Vec2, clamped: bool },
    RequestScroll { node: NodeId, target: Vec2 },
    ReleaseFollowTail { node: NodeId },
    ResumeFollowTail { node: NodeId },
    SetPlacement { node: NodeId, placed: Option<Placed> },
    EditorApply { node: NodeId, update: EditorRuntimeUpdate },
    SetDocSelection { region: NodeId, selection: Option<DocSelection> },
    SetMedia { node: NodeId, state: ImageState, asset: Option<MediaAsset> },
    SetInlineMedia { node: NodeId, index: usize, state: ImageState, asset: Option<MediaAsset> },
    SetNativeLive { node: NodeId, live: bool },
    SetViewportContext { breakpoint: Breakpoint, width: f32, touch: bool },
    ThemeChanged(Box<Theme>),
    FinishExit(ExitTicket),
}

impl Tree {
    pub fn apply_runtime(&mut self, op: RuntimeOp) -> TreeDelta {
        let mut delta = TreeDelta::default();
        let selections = matches!(
            op,
            RuntimeOp::SetState { .. } | RuntimeOp::SetFocus { .. } | RuntimeOp::SetViewportContext { .. } | RuntimeOp::ThemeChanged(_)
        )
        .then(|| self.selection_projections().collect());
        match op {
            RuntimeOp::SetState { node, state, on } => {
                if self.is_live(node) {
                    let flags = &mut self.nodes[node].state;
                    let previous = *flags;
                    flags.set(state - State::TREE_DERIVED, on);
                    if previous != *flags {
                        self.semantics_changed(node, &mut delta);
                        self.resolve_subtree(node, &mut delta);
                        self.reconcile_focus(&mut delta);
                    }
                }
            }
            RuntimeOp::SetFocus { node, visible } => {
                if self.can_set_focus(node) {
                    self.change_focus(node, visible, &mut delta);
                    self.reconcile_group_stops(&mut delta);
                    let mut restore = std::mem::take(&mut self.focus_restore);
                    restore.remember(self, self.focus);
                    self.focus_restore = restore;
                }
            }
            RuntimeOp::SetScroll { node, offset, clamped } => {
                if self.is_live(node)
                    && finite(offset)
                    && let Some(slot) = self.tables.scrolls.get_mut(node)
                {
                    let next = ScrollSlot { offset, clamped, requested: None };
                    if *slot != next {
                        *slot = next;
                        delta.scroll_dirty.insert(node);
                        self.paint_changed(node, &mut delta);
                    }
                }
            }
            RuntimeOp::RequestScroll { node, target } => {
                if self.is_live(node)
                    && finite(target)
                    && let Some(slot) = self.tables.scrolls.get_mut(node)
                {
                    self.scroll_seq = self.scroll_seq.checked_add(1).expect("scroll request sequence exhausted");
                    slot.requested = Some((self.scroll_seq, target));
                    delta.scroll_dirty.insert(node);
                    delta.geometry_demands.insert(GeometryDemand::Scrollport(node));
                }
            }
            RuntimeOp::ReleaseFollowTail { node } => {
                if let Some(core) = self.nodes.get_mut(node)
                    && core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE)
                {
                    core.flags.remove(NodeFlags::FOLLOW_TAIL_ACTIVE);
                    delta.scroll_dirty.insert(node);
                }
            }
            RuntimeOp::ResumeFollowTail { node } => {
                if let Some(core) = self.nodes.get_mut(node)
                    && core.flags.contains(NodeFlags::FOLLOW_TAIL)
                    && !core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE)
                {
                    core.flags.insert(NodeFlags::FOLLOW_TAIL_ACTIVE);
                    delta.scroll_dirty.insert(node);
                }
            }
            RuntimeOp::SetPlacement { node, placed } => {
                if self.is_live(node)
                    && let Some(overlay) = self.tables.overlays.get_mut(node)
                    && overlay.placed != placed
                {
                    overlay.placed = placed;
                    delta.overlay_dirty.insert(node);
                    self.layout_changed(node, &mut delta);
                }
            }
            RuntimeOp::EditorApply { node, update } => {
                if self.focus == Some(node)
                    && self.is_focusable(node)
                    && let Some(editor) = self.tables.editors.get_mut(node)
                    && editor.apply_update(self.id, node, update)
                {
                    self.editor_changed(node, &mut delta);
                }
            }
            RuntimeOp::SetDocSelection { region, selection } => {
                if let Some(selection) = selection
                    && !self.valid_doc_selection(region, selection)
                {
                    return delta;
                }
                if self.doc_selection(region) != selection {
                    self.semantics_changed(region, &mut delta);
                    let previous = self.doc_selections.remove(&region);
                    if let Some(selection) = selection {
                        self.doc_selections.insert(region, selection);
                    }
                    let dirty: FxHashSet<_> = previous
                        .into_iter()
                        .chain(selection)
                        .flat_map(|range| self.selected_text_ranges(region, range).into_iter().flatten())
                        .map(|(node, _)| node)
                        .collect();
                    for node in dirty {
                        self.paint_changed(node, &mut delta);
                    }
                }
            }
            RuntimeOp::SetMedia { node, state, asset } => {
                if self.is_live(node)
                    && let Some(slot) = self.tables.media_sources.get_mut(node)
                    && slot.set(state, asset)
                {
                    self.content_changed(node, &mut delta);
                }
            }
            RuntimeOp::SetInlineMedia { node, index, state, asset } => {
                if self.is_live(node)
                    && let Some(slot) = self.tables.inline_media.get_mut(node).and_then(|slots| slots.get_mut(&index))
                {
                    let paint_changed = slot.asset != asset;
                    slot.set(state, asset);
                    if paint_changed {
                        self.paint_changed(node, &mut delta);
                    }
                }
            }
            RuntimeOp::SetNativeLive { node, live } => {
                if self.is_live(node)
                    && let Some(native) = self.tables.natives.get_mut(node)
                    && native.live != live
                {
                    native.live = live;
                    self.paint_changed(node, &mut delta);
                    self.semantics_changed(node, &mut delta);
                }
            }
            RuntimeOp::SetViewportContext { breakpoint, width, touch } => {
                let responsive = ResponsiveBreakpoint::at_width(width);
                if self.breakpoint != breakpoint || self.responsive_breakpoint != responsive || self.touch != touch {
                    self.breakpoint = breakpoint;
                    self.responsive_breakpoint = responsive;
                    self.touch = touch;
                    self.resolve_subtree(self.root, &mut delta);
                    self.reconcile_focus(&mut delta);
                }
            }
            RuntimeOp::ThemeChanged(theme) => {
                if self.theme != *theme {
                    self.theme = *theme;
                    self.root_theme_revision = self.next_theme_revision();
                    (self.initial_inherited, self.initial_visual) = crate::style::StyleResolver::initial(&self.theme);
                    self.resolve_subtree(self.root, &mut delta);
                    self.paint_changed(self.root, &mut delta);
                    self.reconcile_focus(&mut delta);
                }
            }
            RuntimeOp::FinishExit(ticket) => self.finish_exit(ticket, &mut delta),
        }
        if let Some(selections) = selections {
            self.revalidate_selection(selections, &mut delta);
        }
        delta
    }

    pub(crate) fn change_focus(&mut self, next: Option<NodeId>, visible: bool, delta: &mut TreeDelta) {
        let previous = self.focus;
        if previous == next && (next.is_none() || self.focus_visible() == visible) {
            return;
        }
        let ancestors = self.focus_ancestors();
        self.focus = next;
        if previous != next {
            for node in previous.into_iter().chain(next) {
                if let Some(editor) = self.tables.editors.get_mut(node) {
                    editor.reset_session();
                    self.editor_changed(node, delta);
                }
            }
        }
        let mut styles = FxHashSet::default();
        self.reconcile_focus_states(ancestors, visible, &mut styles, delta);
        self.resolve_dirty_styles(styles, delta);
    }

    pub(crate) fn focus_visible(&self) -> bool {
        self.focus.and_then(|node| self.node(node)).is_some_and(|core| core.state.contains(State::FOCUS_VISIBLE))
    }

    pub(crate) fn focus_ancestors(&self) -> SmallVec<[NodeId; 8]> {
        std::iter::successors(self.focus, |&node| self.parent(node)).collect()
    }

    pub(crate) fn reconcile_focus_states(
        &mut self,
        previous: SmallVec<[NodeId; 8]>,
        visible: bool,
        style_roots: &mut FxHashSet<NodeId>,
        delta: &mut TreeDelta,
    ) {
        let mut previous: FxHashSet<_> = previous.into_iter().collect();
        let mut current = self.focus.filter(|&node| self.nodes.contains_key(node));
        while let Some(node) = current {
            previous.remove(&node);
            let focused = Some(node) == self.focus;
            let mut state = State::FOCUS_WITHIN;
            state.set(State::FOCUSED, focused);
            state.set(State::FOCUS_VISIBLE, focused && visible);
            self.set_focus_state(node, state, style_roots, delta);
            current = self.parent(node);
        }
        for node in previous {
            self.set_focus_state(node, State::empty(), style_roots, delta);
        }
    }

    fn set_focus_state(&mut self, node: NodeId, state: State, style_roots: &mut FxHashSet<NodeId>, delta: &mut TreeDelta) {
        let Some(core) = self.nodes.get_mut(node) else { return };
        let changed = (core.state & State::TREE_DERIVED) ^ state;
        if changed.is_empty() {
            return;
        }
        core.state = (core.state - State::TREE_DERIVED) | state;
        self.semantics_changed(node, delta);
        if self.style(node).is_some_and(|style| {
            style.authored.overrides.iter().any(|(when, _)| matches!(when, When::State(state) if changed.intersects(*state)))
        }) {
            style_roots.insert(node);
        }
    }
}

fn finite(vector: Vec2) -> bool {
    vector.x.is_finite() && vector.y.is_finite()
}
