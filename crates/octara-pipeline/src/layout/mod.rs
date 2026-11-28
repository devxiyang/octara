//! Incremental native layout and projected window geometry.

use octara_model::overlay::AnchorTarget;
mod compute;
mod style;

#[cfg(test)]
mod tests;

use crate::projected::DenseProjected;
use crate::text::TextEngine;
use compute::{BoundaryProbes, BoundaryState, Compute, effective_style, excluded_children, is_measure_leaf, isolated_anchor, layout_style};
use octara_model::geometry::{Edges, LayoutBox, Placed, Point, Rect, Side, Size, Vec2, WindowViewport};
use octara_model::style::{Contain, Display, FlexDirection, LayoutLength, LayoutStyle, Overflow, Position, State};
use octara_model::text::TextStyle;
use octara_runtime::engine::ScrollRange;
use octara_tree::{DirtyBitmap, NodeFlags, NodeId, NodeKind, RetiredNode, RuntimeOp, Tree, TreeDelta};
use slotmap::{Key, KeyData, SecondaryMap};
use smallvec::SmallVec;
use std::{cell::RefCell, collections::BTreeSet};
use taffy::{AvailableSpace, CoreStyle};

#[derive(Clone, Copy, Default, PartialEq)]
struct Fit {
    width: Option<f32>,
    height: Option<f32>,
}

#[derive(Clone, Copy)]
struct NaturalSize {
    authored: Size,
    cross: Option<(Fit, Size)>,
}

#[derive(Clone, Copy, PartialEq)]
struct Frame {
    layout: LayoutBox,
    bounds: Rect,
    scroll_parent: Option<NodeId>,
    clip_parent: Option<NodeId>,
    nearest_rounded_clip: Option<NodeId>,
    rounded_clip: bool,
    local_client: Rect,
    local_content: Rect,
    clip_axes: [bool; 2],
    scroll: Option<ScrollRange>,
    offset: Vec2,
    scroll_clamped: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct ContentAnchor {
    child: NodeId,
    y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutWork {
    /// Taffy computations, including overlay fit passes.
    pub computes: u64,
    /// Window geometry visits across all projected subtrees.
    pub frames: u64,
    /// Anchored overlays considered for placement.
    pub anchors: u64,
    /// Inline links considered for projection.
    pub links: u64,
    /// Relayouts performed using saved boundary constraints.
    pub boundaries_hit: u64,
    /// Local outputs that required recomputing an enclosing container.
    pub escalations: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum Measurement {
    SingleLine(f32),
    FixedRows { rows: u16, line_height: f32 },
    Multiline { value_rev: u64, secure: bool, rows: (u16, u16), style: TextStyle },
    Text { revision: u64, style: TextStyle, mono: &'static str },
    Leaf(u32),
}

#[derive(Clone, Copy, PartialEq)]
enum AnchorPlacement {
    Suppressed,
    Placed(Placed),
}

impl Frame {
    fn anchor_bounds(self) -> (Rect, Rect, Option<NodeId>) {
        (self.bounds, self.local_content, self.scroll_parent)
    }

    fn child_context(self, node: NodeId, overlay_root: bool) -> FrameContext {
        FrameContext {
            origin: Point::new(self.bounds.x, self.bounds.y),
            scroll_parent: if self.scroll.is_some() { Some(node) } else { self.scroll_parent },
            clip_parent: if overlay_root {
                None
            } else if self.clip_axes != [false; 2] {
                Some(node)
            } else {
                self.clip_parent
            },
            nearest_rounded_clip: if overlay_root {
                None
            } else if self.rounded_clip {
                Some(node)
            } else {
                self.nearest_rounded_clip
            },
        }
    }
}

type AnchorTargets = SmallVec<[NodeId; 2]>;

#[derive(Default)]
struct AnchorIndex {
    targets: SecondaryMap<NodeId, AnchorTargets>,
    watchers: SecondaryMap<NodeId, BTreeSet<NodeId>>,
}

impl AnchorIndex {
    fn sync(&mut self, tree: &Tree, node: NodeId) {
        let mut targets = AnchorTargets::new();
        if let Some(target) = tree
            .overlay(node)
            .and_then(|slot| slot.target)
            .and_then(|(target, _)| target.node())
            .filter(|&target| tree.node(target).is_some())
        {
            targets.push(target);
            if tree.link(target).is_some()
                && let Some(parent) = tree.physical_parent(target)
            {
                targets.push(parent);
            }
        }
        if self.targets.get(node) == Some(&targets) {
            return;
        }
        for target in self.targets.remove(node).into_iter().flatten() {
            let Some(watchers) = self.watchers.get_mut(target) else { continue };
            watchers.remove(&node);
            if watchers.is_empty() {
                self.watchers.remove(target);
            }
        }
        if tree.overlay(node).is_some_and(|slot| slot.target.is_some()) {
            for &target in &targets {
                self.watchers.entry(target).expect("anchor target key").or_default().insert(node);
            }
            self.targets.insert(node, targets);
        }
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct LayoutFlags: u16 {
        const DIRTY = 1 << 0;
        const SUBTREE_DIRTY = 1 << 1;
        const BOUNDARY_IMPLICIT = 1 << 2;
        const BOUNDARY_EXPLICIT = 1 << 3;
        const HAS_ESCAPING_OOF = 1 << 4;
        const LIVE = 1 << 5;
        const OFFSCREEN_SKIPPED = 1 << 6;
    }
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct LayoutMeta {
    layout_epoch: u32,
    flags: LayoutFlags,
    depth: u16,
}

const _: () = assert!(std::mem::size_of::<LayoutMeta>() == 8);

#[derive(Clone, Copy, PartialEq, Eq)]
struct InputRevision {
    layout: u32,
    style: u32,
    parent: Option<NodeId>,
}

#[derive(Default)]
pub(crate) struct LayoutKernel {
    meta: SecondaryMap<NodeId, LayoutMeta>,
    layouts: SecondaryMap<NodeId, taffy::Layout>,
    // Indirection keeps empty dense slots at pointer width. Only algorithm-backed nodes
    // allocate a cache, which also owns the optional out-of-flow candidates of its final pass.
    caches: SecondaryMap<NodeId, Box<taffy::Cache>>,
    boundary_states: SecondaryMap<NodeId, Box<BoundaryState>>,
    boundary_probes: SecondaryMap<NodeId, Box<BoundaryProbes>>,
    hoisted_oof: SecondaryMap<NodeId, SmallVec<[NodeId; 2]>>,
    hoisted_by: SecondaryMap<NodeId, NodeId>,
    grid_info: SecondaryMap<NodeId, taffy::DetailedLayoutInfo<String>>,
    excluded_children: SecondaryMap<NodeId, SmallVec<[usize; 2]>>,
    revisions: SecondaryMap<NodeId, InputRevision>,
    epoch: u32,
    dirty_roots: BTreeSet<(u16, NodeId)>,
    dirty_caches: DirtyBitmap,
    layout_touched: DirtyBitmap,
    stage_layout_touched: DirtyBitmap,
    rollback_dirty: DirtyBitmap,
    frames: DenseProjected<Frame>,
    clip_cache: RefCell<SecondaryMap<NodeId, (u64, Rect)>>,
    clip_revision: u64,
    scroll_anchors: DenseProjected<ContentAnchor>,
    virtual_anchor_markers: BTreeSet<NodeId>,
    virtual_anchor_ports: BTreeSet<NodeId>,
    keyboard_focus: Option<NodeId>,
    cold_editor_size: Option<(NodeId, Size)>,
    committed_keyboard_focus: Option<NodeId>,
    links: DenseProjected<Frame>,
    changed: DirtyBitmap,
    local_changed: DirtyBitmap,
    scroll_changed: DirtyBitmap,
    animated_scrolls: DirtyBitmap,
    anchors: AnchorIndex,
    placement: DenseProjected<AnchorPlacement>,
    fits: SecondaryMap<NodeId, Fit>,
    natural_sizes: SecondaryMap<NodeId, NaturalSize>,
    measurements: SecondaryMap<NodeId, Measurement>,
    native_sizes: SecondaryMap<NodeId, Size>,
    native_dirty: BTreeSet<NodeId>,
    viewport: Option<WindowViewport>,
    window: Rect,
    committed_window: Rect,
    font_generation: u64,
    work: LayoutWork,
}

impl LayoutKernel {
    pub(crate) fn project(&mut self, tree: &Tree, delta: &TreeDelta, viewport: WindowViewport, text: &mut TextEngine) -> Vec<RuntimeOp> {
        self.frames.begin_stage();
        self.links.begin_stage();
        self.scroll_anchors.begin();
        self.placement.begin();
        self.changed.clear();
        self.local_changed.clear();
        self.scroll_changed.clear();

        let previous_viewport = self.viewport.replace(viewport);
        let window = Rect::new(0.0, 0.0, viewport.size.width, viewport.size.height);
        if self.window != window {
            self.window = window;
            self.invalidate_clip_cache();
        }

        let cold_editor_size = self.cold_editor_size.take();
        let mut projection = Projection::new(
            viewport,
            EditorProjection {
                previous_size: tree.focus().and_then(|node| {
                    cold_editor_size
                        .filter(|&(focus, _)| focus == node)
                        .map(|(_, size)| size)
                        .or_else(|| self.local_content(node).map(rect_size))
                }),
                reveal: tree.focus().is_some_and(|node| delta.editor_dirty.contains(&node)),
            },
        );
        self.sync(tree, delta, previous_viewport.is_none(), &mut projection);
        if previous_viewport.is_none_or(|previous| previous.size != viewport.size) {
            self.invalidate(tree, tree.root(), false);
            for node in self.anchors.targets.keys().filter(|&node| isolated_anchor(tree, node)) {
                self.natural_sizes.remove(node);
                projection.queue_anchor(tree, node);
            }
        }
        if self.font_generation != text.font_generation() {
            self.font_generation = text.font_generation();
            for (node, core) in tree.nodes() {
                if matches!(core.kind, NodeKind::Text | NodeKind::TextInput) {
                    self.invalidate(tree, node, false);
                }
            }
        }
        for &node in &std::mem::take(&mut self.native_dirty) {
            self.invalidate(tree, node, false);
        }

        let mut updates = Vec::new();
        let computes = self.work.computes;
        self.next_epoch();
        self.relayout(tree, text, &mut projection);
        let recomputed = self.work.computes != computes;
        projection.layout_dirty.union_with(&self.layout_touched);
        projection.layout_dirty.union_with(&self.rollback_dirty);
        self.layout_touched.clear();
        self.rollback_dirty.clear();

        let mut roots = projection.layout_dirty.clone();
        roots.union_with(&delta.scroll_dirty);
        roots.union_with(&delta.editor_dirty);
        roots.union_with(&delta.overlay_dirty);
        roots.extend(
            delta
                .paint_dirty
                .iter()
                .filter(|&node| self.frames.get(node).is_some_and(|frame| frame.rounded_clip != rounded_clip(tree, node, frame.clip_axes))),
        );
        roots.extend(delta.semantics_dirty.iter().filter(|&node| self.frames.contains_key(node) && !tree.is_live(node)));
        let viewport_changed = previous_viewport != Some(viewport);
        for node in tree.overlay_nodes() {
            if tree.overlay(node).is_some_and(|slot| slot.mount.is_some()) && self.frames.contains_key(node) != tree.is_shown(node) {
                roots.insert(node);
            }
        }
        let mut roots: Vec<_> = roots.iter().collect();
        roots.sort_unstable_by_key(|&node| (self.meta.get(node).map_or(u16::MAX, |meta| meta.depth), node));
        for node in roots {
            if isolated_anchor(tree, node) {
                projection.refresh_anchors.insert(node);
                projection.queue_anchor(tree, node);
            } else if !projection.visited.contains(&node) {
                self.walk_frames(tree, text, node, &mut projection);
            }
        }
        for node in delta.overlay_dirty.iter() {
            if tree.overlay(node).is_some_and(|slot| slot.target.is_some()) {
                projection.queue_anchor(tree, node);
            }
        }
        for node in delta.layout_dirty.iter().filter(|&node| tree.text(node).is_some()) {
            self.queue_anchor_watchers(tree, node, &mut projection);
        }
        if viewport_changed
            || recomputed
            || !delta.editor_dirty.is_empty()
            || !delta.semantics_dirty.is_empty()
            || !delta.scroll_dirty.is_empty()
        {
            for node in self.anchors.targets.keys() {
                if viewport_changed
                    || tree
                        .overlay(node)
                        .and_then(|slot| slot.target)
                        .is_some_and(|(target, _)| matches!(target, AnchorTarget::EditorCaret(_) | AnchorTarget::TextSelection(_)))
                {
                    projection.queue_anchor(tree, node);
                }
            }
        }
        let keyboard_focus = tree.focus().filter(|&node| tree.node(node).is_some_and(|core| core.state.contains(State::FOCUS_VISIBLE)));
        if keyboard_focus != self.keyboard_focus {
            self.keyboard_focus = keyboard_focus;
            if let Some(focus) = keyboard_focus {
                self.reveal_focus(tree, text, focus, &mut projection);
            }
        }
        self.place_anchors(tree, text, &mut projection, &mut updates);
        self.layout_touched.clear();
        self.report_scroll(tree, &projection.scroll, &mut updates);
        self.record_scroll_anchors(tree, &projection.scroll);

        Self::frame_changes(&self.frames, &mut self.changed, &mut self.local_changed, &mut self.scroll_changed);
        text.project(tree, delta, self);
        self.project_links(tree, text);
        Self::frame_changes(&self.links, &mut self.changed, &mut self.local_changed, &mut self.scroll_changed);
        updates
    }

    fn frame_changes(frames: &DenseProjected<Frame>, changed: &mut DirtyBitmap, local: &mut DirtyBitmap, scroll: &mut DirtyBitmap) {
        for (node, previous) in frames.stage_changes() {
            let current = frames.get(node);
            let local_change = match (previous, current) {
                (Some(previous), Some(current)) => {
                    if previous.offset != current.offset {
                        scroll.insert(node);
                    }
                    if (Frame { offset: current.offset, scroll_clamped: current.scroll_clamped, ..*previous }) != *current {
                        changed.insert(node);
                    }
                    previous.layout != current.layout
                        || previous.local_client != current.local_client
                        || previous.local_content != current.local_content
                        || previous.clip_axes != current.clip_axes
                        || previous.scroll != current.scroll
                }
                _ => previous != current,
            };
            if local_change {
                local.insert(node);
                changed.insert(node);
            }
        }
    }

    fn sync(&mut self, tree: &Tree, delta: &TreeDelta, initial: bool, projection: &mut Projection) {
        self.sync_virtual_anchors(tree, delta);
        // Remove retired generations before adding a node that reuses the same slot.
        for retired in &delta.retired {
            let node = retired.node();
            self.queue_anchor_watchers(tree, node, projection);
            self.set_frame(tree, node, None, projection);
            self.links.remove(node);
            if let RetiredNode::Destroyed(_) = retired {
                self.meta.remove(node);
                self.revisions.remove(node);
                self.layouts.remove(node);
                self.excluded_children.remove(node);
                self.caches.remove(node);
                self.boundary_states.remove(node);
                self.boundary_probes.remove(node);
                self.grid_info.remove(node);
                self.hoisted_by.remove(node);
                for child in self.hoisted_oof.remove(node).into_iter().flatten() {
                    if self.hoisted_by.get(child) == Some(&node) {
                        self.hoisted_by.remove(child);
                    }
                }
                self.clip_cache.get_mut().remove(node);
                self.anchors.watchers.remove(node);
                self.fits.remove(node);
                self.placement.remove(node);
                self.natural_sizes.remove(node);
                self.measurements.remove(node);
                self.native_sizes.remove(node);
                self.anchors.sync(tree, node);
            }
        }
        let mut topology = delta.topology_dirty.clone();
        if initial {
            for (node, _) in tree.nodes() {
                self.sync_meta(tree, node);
                if !tree.children(node).is_empty() {
                    topology.insert(node);
                }
            }
            for node in tree.overlay_nodes() {
                self.anchors.sync(tree, node);
                if tree.overlay(node).is_some_and(|slot| slot.target.is_some()) {
                    projection.queue_anchor(tree, node);
                }
            }
        } else {
            for node in delta.created.iter().copied().chain(delta.semantics_dirty.iter()) {
                if self.sync_meta(tree, node) {
                    projection.layout_dirty.insert(node);
                }
            }
            for node in delta.overlay_dirty.iter() {
                let previous = self.anchors.targets.contains_key(node);
                self.anchors.sync(tree, node);
                if self.anchors.targets.contains_key(node) != previous {
                    if let Some(parent) = tree.physical_parent(node) {
                        topology.insert(parent);
                        self.invalidate(tree, parent, false);
                    }
                    if !self.anchors.targets.contains_key(node) {
                        self.placement.remove(node);
                        self.fits.remove(node);
                        self.natural_sizes.remove(node);
                    }
                    self.invalidate(tree, node, false);
                }
            }
        }

        for node in topology.iter() {
            if let Some(excluded) = excluded_children(tree, node) {
                self.excluded_children.insert(node, excluded);
            } else {
                self.excluded_children.remove(node);
            }
            for &child in tree.children(node) {
                if self.revisions.get(child).is_none_or(|previous| previous.parent != Some(node)) {
                    for moved in tree.subtree(child) {
                        if self.sync_meta(tree, moved) {
                            projection.layout_dirty.insert(moved);
                        }
                    }
                }
            }
        }

        for node in delta.layout_dirty.iter().chain(topology.iter().filter(|node| !delta.layout_dirty.contains(node))) {
            let Some(core) = tree.node(node).filter(|core| core.kind != NodeKind::Link) else { continue };
            let previous = self.revisions.get(node).copied();
            let revision = InputRevision { layout: core.layout_rev, style: core.style_rev, parent: tree.physical_parent(node) };
            if previous == Some(revision) {
                continue;
            }
            self.revisions.insert(node, revision);
            self.sync_meta(tree, node);
            if !tree.is_live(node) {
                continue;
            }

            let style_changed = previous.is_none_or(|old| old.style != revision.style);
            if style_changed {
                self.boundary_probes.remove(node);
            }
            let children_changed = topology.contains(&node);
            let measured = is_measure_leaf(core.kind);
            let measurement_changed = measured && self.sync_measurement(tree, node);
            let placement_echo = delta.overlay_dirty.contains(&node) && previous.is_some() && !style_changed && !children_changed;
            if style_changed || children_changed || measurement_changed || (!measured && !placement_echo) {
                let allow_self = !style_changed && children_changed && layout_style(tree, node).contain.contains(Contain::SIZE);
                projection.layout_dirty.insert(node);
                self.invalidate(tree, node, allow_self);
            }
        }
        if initial {
            for (node, core) in tree.nodes().filter(|(_, core)| core.kind != NodeKind::Link) {
                self.revisions.entry(node).expect("live tree key").or_insert(InputRevision {
                    layout: core.layout_rev,
                    style: core.style_rev,
                    parent: core.parent,
                });
            }
        }
    }

    fn sync_meta(&mut self, tree: &Tree, node: NodeId) -> bool {
        let Some(core) = tree.node(node).filter(|core| core.kind != NodeKind::Link) else { return false };
        let meta = self.meta.entry(node).expect("live tree key").or_default();
        meta.depth = physical_depth(tree, node).try_into().unwrap_or(u16::MAX);
        let live = tree.is_live(node);
        let attachment_changed = meta.flags.contains(LayoutFlags::LIVE) != live;
        meta.flags.set(LayoutFlags::LIVE, live);
        let parent_changed = self.revisions.get(node).is_some_and(|revision| revision.parent != core.parent);
        if parent_changed {
            self.revisions[node].parent = core.parent;
        }
        attachment_changed || parent_changed
    }

    fn invalidate(&mut self, tree: &Tree, node: NodeId, allow_self: bool) {
        if !tree.is_live(node) || tree.link(node).is_some() {
            return;
        }
        let mut current = Some(node);
        while let Some(at) = current {
            let meta = &mut self.meta[at];
            meta.flags.insert(if at == node { LayoutFlags::DIRTY } else { LayoutFlags::SUBTREE_DIRTY });
            self.dirty_caches.insert(at);
            let is_root = at == tree.root() || isolated_anchor(tree, at);
            let is_boundary = (at != node || allow_self)
                && self.frames.contains_key(at)
                && meta.flags.intersects(LayoutFlags::BOUNDARY_IMPLICIT | LayoutFlags::BOUNDARY_EXPLICIT)
                && self.boundary_states.contains_key(at);
            if is_root || is_boundary {
                self.dirty_roots.insert((meta.depth, at));
                return;
            }
            current = tree.physical_parent(at);
        }
    }

    fn next_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epoch = 1;
            for (_, meta) in self.meta.iter_mut() {
                meta.layout_epoch = 0;
            }
        }
    }

    fn stamp(&mut self, node: NodeId) {
        if let Some(meta) = self.meta.get_mut(node) {
            meta.layout_epoch = self.epoch;
            self.layout_touched.insert(node);
            self.stage_layout_touched.insert(node);
        }
    }

    fn relayout(&mut self, tree: &Tree, text: &mut TextEngine, projection: &mut Projection) {
        while let Some((_, node)) = self.dirty_roots.pop_first() {
            for dirty in self.dirty_caches.iter() {
                if let Some(cache) = self.caches.get_mut(dirty) {
                    cache.clear();
                }
            }
            self.dirty_caches.clear();
            let Some(meta) = self.meta.get(node) else { continue };
            if !tree.is_live(node) || !meta.flags.intersects(LayoutFlags::DIRTY | LayoutFlags::SUBTREE_DIRTY) {
                continue;
            }
            if isolated_anchor(tree, node) {
                self.natural_sizes.remove(node);
                projection.queue_anchor(tree, node);
                continue;
            }
            let mut compute = Compute::new(self, tree, text);
            if node == tree.root() {
                compute.root(node, projection.viewport.size);
            } else if let Some(previous) = compute.kernel.boundary_states.get(node).map(|state| (**state).clone()) {
                let (output, probes_changed) = compute.boundary(node, previous.input);
                self.work.boundaries_hit += 1;
                let contained = layout_style(tree, node).contain.contains(Contain::LAYOUT | Contain::SIZE);
                if probes_changed
                    || output.size != previous.output.size
                    || (!contained
                        && (output.scrollable_overflow_rect != previous.output.scrollable_overflow_rect
                            || output.baselines != previous.output.baselines))
                    || !output.oof_candidates.is_empty()
                {
                    self.work.escalations += 1;
                    self.meta[node].flags.remove(LayoutFlags::BOUNDARY_IMPLICIT | LayoutFlags::BOUNDARY_EXPLICIT);
                    self.boundary_states.remove(node);
                    if let Some(parent) = tree.physical_parent(node) {
                        self.invalidate(tree, parent, true);
                    }
                }
            }
        }
    }

    pub(crate) fn copy_measure_inputs_to(&self, tree: &Tree, cold: &mut Self) {
        cold.native_sizes = self.native_sizes.clone();
        cold.animated_scrolls = self.animated_scrolls.clone();
        cold.keyboard_focus = self.keyboard_focus;
        cold.cold_editor_size = tree.focus().and_then(|node| Some((node, rect_size(self.local_content(node)?))));
        cold.virtual_anchor_markers = self.virtual_anchor_markers.clone();
        cold.virtual_anchor_ports = self.virtual_anchor_ports.clone();
        for (node, &anchor) in self.scroll_anchors.iter() {
            cold.scroll_anchors.insert(node, anchor);
        }
    }

    fn sync_measurement(&mut self, tree: &Tree, node: NodeId) -> bool {
        let measurement = if let Some(editor) = tree.editor(node) {
            let style = tree.visual(node).text_style();
            let min_rows = editor.rows.0.max(1);
            if !editor.committed_attachments().is_empty() || !editor.committed_decorations().is_empty() {
                Measurement::Multiline { value_rev: editor.value_rev(), secure: editor.is_secure(), rows: editor.rows, style }
            } else if !editor.multiline {
                Measurement::SingleLine(style.line_height)
            } else if min_rows >= editor.rows.1 {
                Measurement::FixedRows { rows: min_rows, line_height: style.line_height }
            } else {
                Measurement::Multiline { value_rev: editor.value_rev(), secure: editor.is_secure(), rows: editor.rows, style }
            }
        } else if let Some(text) = tree.text(node) {
            Measurement::Text { revision: text.text_rev, style: tree.visual(node).text_style(), mono: tree.theme_at(node).font_mono }
        } else {
            Measurement::Leaf(tree.node(node).expect("retained measure node").layout_rev)
        };
        self.measurements.insert(node, measurement) != Some(measurement)
    }

    fn walk_frames(&mut self, tree: &Tree, text: &mut TextEngine, root: NodeId, projection: &mut Projection) {
        let parent = if root == tree.root() {
            Some(FrameContext::default())
        } else {
            tree.physical_parent(root).and_then(|node| {
                let frame = self.frames.get(node)?;
                Some(frame.child_context(node, node == tree.overlay_root()))
            })
        };
        let mut remaining: SmallVec<[_; 16]> = [(root, parent)].into_iter().collect();
        while let Some((node, parent)) = remaining.pop() {
            self.work.frames += 1;
            projection.visited.insert(node);
            let overlay = tree.overlay(node);
            let anchor = overlay.and_then(|overlay| overlay.target);
            let visible = tree.is_live(node)
                && layout_style(tree, node).display != Display::None
                && overlay.is_none_or(|overlay| overlay.mount.is_none() || tree.is_shown(node))
                && anchor.is_none_or(|(target, _)| {
                    target.node().is_none_or(|target| tree.is_shown(target))
                        && self.placement.get(node) != Some(&AnchorPlacement::Suppressed)
                });
            let previous = self.frames.get(node).copied();
            let next = parent
                .zip(self.layouts.get(node))
                .filter(|_| visible)
                .map(|(parent, layout)| self.frame(tree, text, node, layout, parent, projection));
            self.set_frame(tree, node, next, projection);
            if next.is_some_and(|frame| frame.scroll.is_some()) {
                projection.scroll.insert(node);
            }
            if tree.text(node).is_some() || (next.is_none() && previous.is_none()) {
                continue;
            }
            let child_context = |frame: Frame| frame.child_context(node, node == tree.overlay_root());
            let child = next.map(child_context);
            let context_changed = previous.map(child_context) != child;
            for &child_node in tree.children(node).iter().rev() {
                if isolated_anchor(tree, child_node) {
                    if context_changed {
                        projection.queue_anchor(tree, child_node);
                    }
                    continue;
                }
                if context_changed
                    || child.is_some()
                        && (self.meta.get(child_node).is_some_and(|meta| meta.layout_epoch == self.epoch)
                            || projection.layout_dirty.contains(&child_node))
                {
                    remaining.push((child_node, child));
                }
            }
        }
    }

    fn set_frame(&mut self, tree: &Tree, node: NodeId, next: Option<Frame>, projection: &mut Projection) {
        let previous = self.frames.get(node).copied();
        if self.frames.set(node, next) {
            self.invalidate_clip_cache();
            if previous.map(Frame::anchor_bounds) != next.map(Frame::anchor_bounds) {
                self.queue_anchor_watchers(tree, node, projection);
            }
            if previous.map(|frame| frame.offset) != next.map(|frame| frame.offset)
                && (previous.is_some_and(|frame| frame.offset != Vec2::default())
                    || next.is_some_and(|frame| frame.offset != Vec2::default()))
            {
                for target in self.anchors.watchers.keys() {
                    if self.scroll_parents(target).any(|parent| parent == node) {
                        self.queue_anchor_watchers(tree, target, projection);
                    }
                }
            }
        }
        if next.is_none_or(|frame| frame.scroll.is_none()) {
            self.scroll_anchors.set(node, None);
        }
    }

    fn queue_anchor_watchers(&self, tree: &Tree, target: NodeId, projection: &mut Projection) {
        if let Some(watchers) = self.anchors.watchers.get(target) {
            for &node in watchers {
                projection.queue_anchor(tree, node);
            }
        }
    }

    fn frame(
        &self,
        tree: &Tree,
        text: &mut TextEngine,
        node: NodeId,
        result: &taffy::Layout,
        parent: FrameContext,
        projection: &Projection,
    ) -> Frame {
        let FrameContext { origin, .. } = parent;
        let containing_block = self.hoisted_by.get(node).copied();
        let FrameContext { origin: position_origin, scroll_parent, clip_parent, nearest_rounded_clip } = containing_block
            .and_then(|owner| self.frames.get(owner).map(|frame| frame.child_context(owner, owner == tree.overlay_root())))
            .unwrap_or(parent);
        let position = match self.placement.get(node) {
            Some(AnchorPlacement::Placed(placed)) if tree.overlay(node).is_some_and(|slot| slot.target.is_some()) => Some(*placed),
            _ => None,
        };
        let (x, y) = position.map_or_else(
            || (position_origin.x + result.location.x, position_origin.y + result.location.y),
            |placed| {
                let scroll = self.translation_from(scroll_parent);
                (placed.x - scroll.x, placed.y - scroll.y)
            },
        );
        let bounds = Rect::new(x, y, result.size.width, result.size.height);
        let border = edges(result.border);
        let padding = edges(result.padding);
        let client_edges =
            Edges { right: border.right + result.scrollbar_size.width, bottom: border.bottom + result.scrollbar_size.height, ..border };
        let local_client = inner(Rect::new(0.0, 0.0, bounds.w, bounds.h), client_edges);
        let local_content = inner(local_client, padding);
        let overflow = effective_style(tree, node, &self.fits, self.window).overflow();
        let paint_contained = layout_style(tree, node).contain.contains(Contain::PAINT);
        let clip_axes =
            [paint_contained || overflow.x != taffy::Overflow::Visible, paint_contained || overflow.y != taffy::Overflow::Visible];
        let mut max = Vec2::new(
            if overflow.x == taffy::Overflow::Scroll { (result.scrollable_overflow_rect.right - local_client.w).max(0.0) } else { 0.0 },
            if overflow.y == taffy::Overflow::Scroll { (result.scrollable_overflow_rect.bottom - local_client.h).max(0.0) } else { 0.0 },
        );
        let slot = tree.scroll(node);
        if tree.editor(node).is_some_and(|editor| editor.multiline) {
            max.y = max.y.max(text.editor_scroll_range(tree, node, local_content.w, local_content.h));
        }
        let scroll = slot.map(|_| ScrollRange { max });
        let (offset, scroll_clamped) = slot.zip(scroll).map_or_else(Default::default, |(slot, range)| {
            let target = if let Some((_, requested)) = slot.requested {
                requested
            } else if self.animated_scrolls.contains(&node) {
                slot.offset
            } else if follows_tail(tree, node) {
                Vec2::new(slot.offset.x, range.max.y)
            } else if tree.focus() == Some(node)
                && (projection.editor.reveal || projection.editor.previous_size != Some(rect_size(local_content)))
                && let Some(caret) = text.editor_caret_in_content(tree, node, local_content.w)
            {
                Vec2::new(slot.offset.x, ScrollRange::reveal_axis(slot.offset.y, local_content.h, caret.y, caret.bottom()))
            } else if let Some(focus) = projection.focus_reveals.iter().find_map(|&(port, offset)| (port == node).then_some(offset)) {
                focus
            } else if let Some(delta) = self.scroll_anchor_delta(tree, node) {
                Vec2::new(slot.offset.x, slot.offset.y + delta)
            } else {
                slot.offset
            };
            let offset = range.clamp(target);
            (offset, offset != target)
        });
        let rect = if position.is_some() || position_origin != origin {
            Rect::new(x - origin.x, y - origin.y, bounds.w, bounds.h)
        } else {
            Rect::new(result.location.x, result.location.y, bounds.w, bounds.h)
        };
        Frame {
            layout: LayoutBox { rect, padding, border },
            bounds,
            scroll_parent,
            clip_parent,
            nearest_rounded_clip,
            rounded_clip: rounded_clip(tree, node, clip_axes),
            local_client,
            local_content,
            clip_axes,
            scroll,
            offset,
            scroll_clamped,
        }
    }

    fn sync_virtual_anchors(&mut self, tree: &Tree, delta: &TreeDelta) {
        let mut ports = DirtyBitmap::default();
        ports.extend(delta.topology_dirty.iter().filter(|&node| tree.scroll(node).is_some() || self.virtual_anchor_ports.contains(&node)));
        ports.extend(
            delta
                .layout_dirty
                .iter()
                .filter(|&node| tree.scroll(node).is_some() != self.frames.get(node).is_some_and(|frame| frame.scroll.is_some())),
        );
        for node in delta.created.iter().copied().chain(delta.semantics_dirty.iter()) {
            let marked = tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::VIRTUAL_SCROLL_ANCHOR));
            let changed = if marked { self.virtual_anchor_markers.insert(node) } else { self.virtual_anchor_markers.remove(&node) };
            if changed && let Some(parent) = tree.physical_parent(node) {
                ports.insert(parent);
            }
        }
        for retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = *retired {
                self.virtual_anchor_markers.remove(&node);
                self.virtual_anchor_ports.remove(&node);
            }
        }
        for port in ports.iter() {
            let marked = tree.scroll(port).is_some() && tree.children(port).iter().any(|child| self.virtual_anchor_markers.contains(child));
            if marked {
                self.virtual_anchor_ports.insert(port);
            } else {
                self.virtual_anchor_ports.remove(&port);
            }
        }
    }

    fn maintains_scroll_anchor(&self, tree: &Tree, node: NodeId) -> bool {
        tree.node(node).is_some_and(|core| {
            !core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE)
                && (core.flags.intersects(NodeFlags::FOLLOW_TAIL | NodeFlags::MAINTAIN_SCROLL_ANCHOR)
                    || self.virtual_anchor_ports.contains(&node))
        })
    }

    fn scroll_anchor_delta(&self, tree: &Tree, node: NodeId) -> Option<f32> {
        let anchor = self.scroll_anchors.get(node)?;
        if !self.maintains_scroll_anchor(tree, node)
            || tree.physical_parent(anchor.child) != Some(node)
            || !anchor_child(tree, anchor.child)
        {
            return None;
        }
        let layout = self.layouts.get(anchor.child)?;
        Some(layout.location.y - anchor.y)
    }

    fn record_scroll_anchors(&mut self, tree: &Tree, scrollports: &DirtyBitmap) {
        for node in scrollports.iter() {
            let anchor = self
                .maintains_scroll_anchor(tree, node)
                .then(|| {
                    let clip = self.child_clip(node).or_else(|| self.clip(node))?;
                    tree.children(node).iter().copied().filter(|&child| anchor_child(tree, child)).find_map(|child| {
                        let frame = self.frames.get(child)?;
                        (!self.bounds(child)?.intersect(&clip).is_empty()).then_some(ContentAnchor { child, y: frame.layout.rect.y })
                    })
                })
                .flatten();
            self.scroll_anchors.set(node, anchor);
        }
    }

    fn reveal_focus(&mut self, tree: &Tree, text: &mut TextEngine, focus: NodeId, projection: &mut Projection) {
        let bounds = if let Some(link) = tree.link(focus) {
            tree.physical_parent(focus).and_then(|parent| text.link_bounds_in(tree, parent, &link.range, self.content_bounds(parent)?))
        } else {
            self.bounds(focus)
        };
        let Some(mut target) = bounds else { return };
        let mut scrollports = SmallVec::<[NodeId; 4]>::new();
        for node in std::iter::successors(tree.physical_parent(focus), |&node| tree.physical_parent(node)) {
            let Some(frame) = self.frames.get(node) else { break };
            let Some(range) = frame.scroll else { continue };
            let Some(slot) = tree.scroll(node) else { continue };
            if slot.requested.is_some() || self.animated_scrolls.contains(&node) || follows_tail(tree, node) {
                continue;
            }
            let Some(client) = self.scroll_client(node) else { continue };
            let offset = frame.offset;
            let desired = range.clamp(Vec2::new(
                ScrollRange::reveal_axis(slot.offset.x, client.w, target.x - client.x + offset.x, target.right() - client.x + offset.x),
                ScrollRange::reveal_axis(slot.offset.y, client.h, target.y - client.y + offset.y, target.bottom() - client.y + offset.y),
            ));
            projection.focus_reveals.push((node, desired));
            if desired != offset {
                target.x += offset.x - desired.x;
                target.y += offset.y - desired.y;
                scrollports.push(node);
            }
        }
        for node in scrollports.into_iter().rev() {
            self.walk_frames(tree, text, node, projection);
        }
    }

    fn anchor_bounds(&self, tree: &Tree, text: &mut TextEngine, target: AnchorTarget) -> Option<Rect> {
        if target.node().is_some_and(|node| !tree.is_shown(node)) {
            return None;
        }
        match target {
            AnchorTarget::Node(node) => {
                if let Some(link) = tree.link(node) {
                    let parent = tree.physical_parent(node)?;
                    return text.link_bounds_in(tree, parent, &link.range, self.content_bounds(parent)?);
                }
                self.bounds(node)
            }
            AnchorTarget::EditorCaret(node) => text.caret_in(tree, self, node),
            AnchorTarget::TextSelection(node) => text.selection_rect_in(tree, self, node),
            AnchorTarget::Point(_) | AnchorTarget::Rect(_) => target.fixed_rect(),
        }
    }

    fn place_anchors(&mut self, tree: &Tree, text: &mut TextEngine, projection: &mut Projection, updates: &mut Vec<RuntimeOp>) {
        let viewport = projection.viewport;
        let safe = viewport.safe_bounds();
        while let Some((_, node)) = projection.anchors.pop_first() {
            projection.placed.insert(node);
            let slot = tree.overlay(node);
            let Some((target, placement)) = slot.and_then(|slot| slot.target) else {
                self.placement.set(node, None);
                continue;
            };
            self.work.anchors += 1;
            let target_bounds = tree.is_shown(node).then(|| self.anchor_bounds(tree, text, target)).flatten();
            let Some(target) = target_bounds else {
                self.placement.set(node, Some(AnchorPlacement::Suppressed));
                if self.frames.contains_key(node) {
                    self.walk_frames(tree, text, node, projection);
                }
                if slot.and_then(|slot| slot.placed).is_some() {
                    updates.push(RuntimeOp::SetPlacement { node, placed: None });
                }
                continue;
            };
            if !tree.is_live(node) || !tree.physical_parent(node).is_some_and(|parent| self.frames.contains_key(parent)) {
                continue;
            }
            let computes = self.work.computes;
            let mut natural = if let Some(&natural) = self.natural_sizes.get(node) {
                natural
            } else {
                if isolated_anchor(tree, node) {
                    self.fits.remove(node);
                    self.compute_overlay(tree, node, viewport, text);
                }
                NaturalSize { authored: self.layout_size(node), cross: None }
            };
            let mut fit = Fit::default();
            let mut size = natural.authored;
            let usable = Size::new((safe.w - 2.0 * placement.margin).max(0.0), (safe.h - 2.0 * placement.margin).max(0.0));
            if placement.fit {
                // A cross-axis limit can wrap text and change the size we need on the main axis.
                fit = match placement.side {
                    Side::Top | Side::Bottom => Fit { width: (size.width > usable.width).then_some(usable.width), height: None },
                    Side::Left | Side::Right => Fit { width: None, height: (size.height > usable.height).then_some(usable.height) },
                };
                if fit != Fit::default() {
                    size = if let Some((_, constrained)) = natural.cross.filter(|(cross, _)| *cross == fit) {
                        constrained
                    } else {
                        self.apply_fit(tree, node, fit, viewport, text);
                        let constrained = self.layout_size(node);
                        natural.cross = Some((fit, constrained));
                        constrained
                    };
                }
            }
            self.natural_sizes.insert(node, natural);
            let mut placed = placement.place(target, size, safe);
            if placement.fit {
                let (extent, available, axis) = match placed.side {
                    Side::Top | Side::Bottom => (size.height, usable.height, &mut fit.height),
                    Side::Left | Side::Right => (size.width, usable.width, &mut fit.width),
                };
                let limit = placed.limit.map_or(available, |limit| limit.min(available));
                placed.limit = (extent > limit).then_some(limit);
                *axis = placed.limit;
            }
            self.apply_fit(tree, node, fit, viewport, text);
            let actual = self.layout_size(node);
            let position = placement.position(placed.side, target, actual, safe);
            let placed = Placed { x: position.x, y: position.y, ..placed };
            let placement_changed = self.placement.set(node, Some(AnchorPlacement::Placed(placed)));
            if self.work.computes != computes
                || placement_changed
                || projection.refresh_anchors.contains(&node)
                || !self.frames.contains_key(node)
            {
                self.walk_frames(tree, text, node, projection);
            }
            if slot.and_then(|slot| slot.placed) != Some(placed) {
                updates.push(RuntimeOp::SetPlacement { node, placed: Some(placed) });
            }
        }
    }

    fn apply_fit(&mut self, tree: &Tree, node: NodeId, fit: Fit, viewport: WindowViewport, text: &mut TextEngine) {
        if self.fits.get(node).copied().unwrap_or_default() != fit {
            if fit == Fit::default() {
                self.fits.remove(node);
            } else {
                self.fits.insert(node, fit);
            }
            self.compute_overlay(tree, node, viewport, text);
        }
    }

    fn compute_overlay(&mut self, tree: &Tree, node: NodeId, viewport: WindowViewport, text: &mut TextEngine) {
        if let Some(cache) = self.caches.get_mut(node) {
            cache.clear();
        }
        let size = tree
            .physical_parent(node)
            .and_then(|parent| self.frames.get(parent))
            .map_or(viewport.size, |frame| rect_size(frame.local_client));
        Compute::new(self, tree, text).root(node, size);
    }

    fn layout_size(&self, node: NodeId) -> Size {
        self.layouts.get(node).map_or_else(Size::default, |result| Size::new(result.size.width, result.size.height))
    }

    fn report_scroll(&self, tree: &Tree, candidates: &DirtyBitmap, updates: &mut Vec<RuntimeOp>) {
        for node in candidates.iter() {
            if let Some(frame) = self.frames.get(node)
                && let Some(slot) = tree.scroll(node)
                && (slot.requested.is_some() || slot.offset != frame.offset)
            {
                if slot.requested.is_some() && follows_tail(tree, node) && frame.scroll.is_some_and(|range| frame.offset.y < range.max.y) {
                    updates.push(RuntimeOp::ReleaseFollowTail { node });
                }
                updates.push(RuntimeOp::SetScroll { node, offset: frame.offset, clamped: frame.scroll_clamped });
            }
        }
    }

    fn project_links(&mut self, tree: &Tree, text: &TextEngine) {
        let mut dirty = DirtyBitmap::default();
        dirty.union_with(text.changed_links());
        for node in self.changed.iter() {
            if let Some(text) = tree.text(node) {
                dirty.extend(text.links.iter().copied());
            }
        }
        for node in dirty.iter() {
            self.work.links += 1;
            let frame = (|| {
                let parent_id = tree.physical_parent(node)?;
                let parent = self.frames.get(parent_id)?;
                let rect = text.link_local_bounds(node)?;
                let bounds = translated(rect, Vec2::new(parent.bounds.x, parent.bounds.y));
                Some(Frame {
                    layout: LayoutBox { rect, ..Default::default() },
                    bounds,
                    scroll_parent: parent.scroll_parent,
                    clip_parent: parent.child_context(parent_id, false).clip_parent,
                    nearest_rounded_clip: parent.child_context(parent_id, false).nearest_rounded_clip,
                    rounded_clip: false,
                    local_client: Rect::new(0.0, 0.0, bounds.w, bounds.h),
                    local_content: Rect::new(0.0, 0.0, bounds.w, bounds.h),
                    clip_axes: [false; 2],
                    scroll: None,
                    offset: Vec2::default(),
                    scroll_clamped: false,
                })
            })();
            self.links.set(node, frame);
        }
    }

    fn projected_frame(&self, node: NodeId) -> Option<&Frame> {
        self.frames.get(node).or_else(|| self.links.get(node))
    }

    pub(crate) fn commit_stage(&mut self) {
        self.frames.commit();
        self.scroll_anchors.commit();
        self.committed_keyboard_focus = self.keyboard_focus;
        self.committed_window = self.window;
        self.links.commit();
        self.placement.commit();
        self.stage_layout_touched.clear();
    }

    pub(crate) fn rollback_stage(&mut self) {
        self.rollback_dirty.extend(self.frames.undo_keys());
        self.rollback_dirty.extend(self.placement.undo_keys());
        self.frames.rollback();
        self.scroll_anchors.rollback();
        self.keyboard_focus = self.committed_keyboard_focus;
        self.window = self.committed_window;
        self.links.rollback();
        self.placement.rollback();
        self.changed.clear();
        self.local_changed.clear();
        self.scroll_changed.clear();
        self.invalidate_clip_cache();
        // The authored tree is already current. Reproject the warm geometry that
        // was calculated against it, including changes beyond the authored delta.
        self.rollback_dirty.union_with(&self.stage_layout_touched);
        self.stage_layout_touched.clear();
    }

    pub(crate) fn work(&self) -> LayoutWork {
        self.work
    }

    pub(crate) fn set_native_measure(&mut self, node: NodeId, size: Size) {
        if size.width.is_finite()
            && size.height.is_finite()
            && size.width >= 0.0
            && size.height >= 0.0
            && self.meta.contains_key(node)
            && self.native_sizes.insert(node, size) != Some(size)
        {
            self.native_dirty.insert(node);
        }
    }

    pub(crate) fn geometry(&self, node: NodeId) -> Option<LayoutBox> {
        self.projected_frame(node).map(|frame| frame.layout)
    }

    pub(crate) fn bounds(&self, node: NodeId) -> Option<Rect> {
        Some(translated(self.projected_frame(node)?.bounds, self.scroll_translation(node)))
    }

    pub(crate) fn content_bounds(&self, node: NodeId) -> Option<Rect> {
        let frame = self.projected_frame(node)?;
        let bounds = self.bounds(node)?;
        Some(translated(frame.local_content, Vec2::new(bounds.x, bounds.y)))
    }

    pub(crate) fn scroll_client(&self, node: NodeId) -> Option<Rect> {
        self.scroll_range(node)?;
        self.world_client(node)
    }

    fn world_client(&self, node: NodeId) -> Option<Rect> {
        let local = self.local_client(node)?;
        let bounds = self.bounds(node)?;
        Some(translated(local, Vec2::new(bounds.x, bounds.y)))
    }

    /// Preserve the full rounded padding box even when an outer scissor intersects it.
    pub(crate) fn rounded_child_clip(&self, node: NodeId) -> Option<Rect> {
        (self.clip_axes(node) == [true; 2]).then(|| self.world_client(node)).flatten()
    }

    pub(crate) fn scroll_anchor(&self, node: NodeId) -> Option<NodeId> {
        self.scroll_anchors.get(node).map(|anchor| anchor.child)
    }

    pub(crate) fn nearest_rounded_clip(&self, node: NodeId) -> Option<NodeId> {
        self.projected_frame(node)?.nearest_rounded_clip
    }

    pub(crate) fn clip_ancestor(&self, node: NodeId) -> Option<NodeId> {
        self.projected_frame(node)?.clip_parent
    }

    pub(crate) fn clip(&self, node: NodeId) -> Option<Rect> {
        self.projected_frame(node)?;
        self.clip_through(self.clip_ancestor(node))
    }

    fn clip_through(&self, parent: Option<NodeId>) -> Option<Rect> {
        let Some(parent) = parent else { return Some(self.window) };
        if let Some(&(_, clip)) = self.clip_cache.borrow().get(parent).filter(|(revision, _)| *revision == self.clip_revision) {
            return Some(clip);
        }
        let frame = self.frames.get(parent)?;
        let clip = self.clip_through(frame.clip_parent)?;
        let [x, y] = frame.clip_axes;
        let clip = clip_axes(clip, self.world_client(parent)?, x, y);
        self.clip_cache.borrow_mut().insert(parent, (self.clip_revision, clip));
        Some(clip)
    }

    fn invalidate_clip_cache(&mut self) {
        self.clip_revision = self.clip_revision.wrapping_add(1);
        if self.clip_revision == 0 {
            self.clip_cache.get_mut().clear();
        }
    }

    pub(crate) fn child_clip(&self, node: NodeId) -> Option<Rect> {
        let [x, y] = self.clip_axes(node);
        (x || y).then(|| self.clip_through(Some(node))).flatten()
    }

    pub(crate) fn paint_bounds(&self, node: NodeId) -> Option<Rect> {
        self.projected_frame(node).map(|frame| frame.bounds)
    }

    pub(crate) fn local_client(&self, node: NodeId) -> Option<Rect> {
        self.frames.get(node).map(|frame| frame.local_client)
    }

    pub(crate) fn local_content(&self, node: NodeId) -> Option<Rect> {
        self.frames.get(node).map(|frame| frame.local_content)
    }

    pub(crate) fn clip_axes(&self, node: NodeId) -> [bool; 2] {
        self.frames.get(node).map_or([false; 2], |frame| frame.clip_axes)
    }

    pub(crate) fn scroll_ancestor(&self, node: NodeId) -> Option<NodeId> {
        self.projected_frame(node)?.scroll_parent
    }

    fn scroll_parents(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        std::iter::successors(self.scroll_ancestor(node), |&parent| self.scroll_ancestor(parent))
    }

    fn translation_from(&self, ancestor: Option<NodeId>) -> Vec2 {
        std::iter::successors(ancestor, |&parent| self.scroll_ancestor(parent)).fold(Vec2::default(), |shift, node| {
            let offset = self.scroll_offset(node);
            Vec2::new(shift.x - offset.x, shift.y - offset.y)
        })
    }

    pub(crate) fn scroll_translation(&self, node: NodeId) -> Vec2 {
        self.translation_from(self.scroll_ancestor(node))
    }

    pub(crate) fn scroll_range(&self, node: NodeId) -> Option<ScrollRange> {
        self.frames.get(node)?.scroll
    }

    pub(crate) fn scroll_offset(&self, node: NodeId) -> Vec2 {
        self.frames.get(node).map_or_else(Vec2::default, |frame| frame.offset)
    }

    pub(crate) fn changed(&self) -> &DirtyBitmap {
        &self.changed
    }

    pub(crate) fn local_changed(&self) -> &DirtyBitmap {
        &self.local_changed
    }

    pub(crate) fn scroll_changed(&self) -> &DirtyBitmap {
        &self.scroll_changed
    }

    pub(crate) fn set_animated_scrolls(&mut self, nodes: impl Iterator<Item = NodeId>) {
        self.animated_scrolls.clear();
        self.animated_scrolls.extend(nodes);
    }
}

#[derive(Clone, Copy)]
struct EditorProjection {
    previous_size: Option<Size>,
    reveal: bool,
}

#[derive(Clone, Copy, Default, PartialEq)]
struct FrameContext {
    origin: Point,
    scroll_parent: Option<NodeId>,
    clip_parent: Option<NodeId>,
    nearest_rounded_clip: Option<NodeId>,
}

struct Projection {
    viewport: WindowViewport,
    editor: EditorProjection,
    focus_reveals: SmallVec<[(NodeId, Vec2); 4]>,
    scroll: DirtyBitmap,
    anchors: BTreeSet<(usize, NodeId)>,
    placed: DirtyBitmap,
    refresh_anchors: DirtyBitmap,
    layout_dirty: DirtyBitmap,
    visited: DirtyBitmap,
}

impl Projection {
    fn new(viewport: WindowViewport, editor: EditorProjection) -> Self {
        Self {
            viewport,
            editor,
            focus_reveals: SmallVec::new(),
            scroll: DirtyBitmap::default(),
            anchors: BTreeSet::new(),
            placed: DirtyBitmap::default(),
            refresh_anchors: DirtyBitmap::default(),
            layout_dirty: DirtyBitmap::default(),
            visited: DirtyBitmap::default(),
        }
    }

    fn queue_anchor(&mut self, tree: &Tree, node: NodeId) {
        if !self.placed.contains(&node) {
            // Resolve targets within an anchored overlay after their enclosing anchor.
            let depth = std::iter::successors(Some(node), |&node| {
                tree.overlay(node).and_then(|slot| slot.target.and_then(|(target, _)| target.node())).or_else(|| tree.physical_parent(node))
            })
            .count();
            self.anchors.insert((depth, node));
        }
    }
}

fn follows_tail(tree: &Tree, node: NodeId) -> bool {
    tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE))
}

fn anchor_child(tree: &Tree, node: NodeId) -> bool {
    tree.is_live(node)
        && tree.node(node).is_some_and(|core| !core.flags.contains(NodeFlags::HIDDEN))
        && tree.layout_style(node).display != Display::None
        && tree.overlay(node).is_none_or(|overlay| overlay.target.is_none())
}

fn physical_depth(tree: &Tree, node: NodeId) -> usize {
    std::iter::successors(tree.physical_parent(node), |&parent| tree.physical_parent(parent)).count()
}

fn rect_size(rect: Rect) -> Size {
    Size::new(rect.w, rect.h)
}

fn edges(rect: taffy::Rect<f32>) -> Edges {
    Edges { top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left }
}

fn inner(rect: Rect, edges: Edges) -> Rect {
    Rect::new(
        rect.x + edges.left,
        rect.y + edges.top,
        (rect.w - edges.left - edges.right).max(0.0),
        (rect.h - edges.top - edges.bottom).max(0.0),
    )
}

fn translated(rect: Rect, offset: Vec2) -> Rect {
    Rect::new(rect.x + offset.x, rect.y + offset.y, rect.w, rect.h)
}

fn rounded_clip(tree: &Tree, node: NodeId, axes: [bool; 2]) -> bool {
    axes == [true; 2] && tree.style(node).is_some_and(|style| style.visual.radius.into_iter().any(|radius| radius > 0.0))
}

fn clip_axes(clip: Rect, client: Rect, x: bool, y: bool) -> Rect {
    clip.intersect(&Rect::new(
        if x { client.x } else { clip.x },
        if y { client.y } else { clip.y },
        if x { client.w } else { clip.w },
        if y { client.h } else { clip.h },
    ))
}
