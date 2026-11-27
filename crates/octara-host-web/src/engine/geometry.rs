//! Select dirty geometry and feed one browser measurement batch back to the tree.

use super::DomEngine;
use crate::measure::Geometry;
use crate::undo;
use js_sys::Array;
use octara::runtime::engine::{ProjectFeedback, ScrollRange};
use octara::tree::{GeometryDemand, NodeFlags, NodeId, RuntimeOp, Tree, TreeDelta};
use octara_model::geometry::{Rect, Side, Size, Vec2, WindowViewport};
use octara_model::overlay::AnchorTarget;
use octara_model::semantics::TextClip;
use octara_model::text::Wrap;
use rustc_hash::FxHashSet;
use wasm_bindgen::JsValue;

pub(super) struct GeometryWork {
    candidates: FxHashSet<NodeId>,
    scrollports: FxHashSet<NodeId>,
}

impl GeometryWork {
    pub(super) fn include_moved(&mut self, moved: &FxHashSet<NodeId>) {
        self.candidates.extend(moved);
    }

    pub(super) fn include_images(&mut self, engine: &DomEngine, tree: &Tree, images: FxHashSet<NodeId>) {
        self.scrollports.extend(engine.scroll_ancestors(tree, images.iter().copied()));
        self.candidates.extend(images);
    }
}

impl DomEngine {
    pub(super) fn prepare_geometry(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        resized: &FxHashSet<NodeId>,
        measure_all: bool,
    ) -> GeometryWork {
        let dirty = delta.layout_dirty.iter().chain(&delta.topology_dirty).chain(&delta.overlay_dirty).copied();
        let mut candidates: FxHashSet<_> = dirty.clone().chain(delta.scroll_dirty.iter().chain(resized).copied()).collect();
        let removed_parents = delta.retired.iter().filter_map(|retired| self.entries.get(&retired.node())?.parent);
        let mut scrollports = self.scroll_ancestors(tree, dirty.chain(removed_parents).chain(resized.iter().copied()));
        if measure_all {
            candidates.extend(tree.subtree(tree.root()));
        }
        candidates.extend(&delta.created);
        scrollports.extend(candidates.iter().copied().filter(|&node| self.scrollports.contains(&node) || tree.scroll(node).is_some()));
        scrollports.extend(&delta.scroll_dirty);
        scrollports.extend(delta.geometry_demands.iter().filter_map(|demand| match *demand {
            GeometryDemand::Scrollport(node) => Some(node),
            _ => None,
        }));
        self.save_scrollports(&scrollports);
        GeometryWork { candidates, scrollports }
    }

    pub(super) fn scroll_ancestors(&self, tree: &Tree, nodes: impl IntoIterator<Item = NodeId>) -> FxHashSet<NodeId> {
        let mut visited = FxHashSet::default();
        for node in nodes {
            let mut parent = Some(node);
            while let Some(node) = parent.filter(|&id| visited.insert(id)) {
                parent = tree.parent(node);
            }
        }
        visited.retain(|&node| self.scrollports.contains(&node) || tree.scroll(node).is_some());
        visited
    }

    pub(super) fn save_scrollports(&mut self, scrollports: &FxHashSet<NodeId>) {
        // Layout can clamp offsets implicitly, before any explicit scroll writes.
        for node in scrollports {
            if self.scrollports.contains(node)
                && let Some(element) = self.projected_element(*node)
            {
                self.pending().undo.save_scroll(&element);
            }
        }
    }

    pub(super) fn read_geometry(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        created: &FxHashSet<NodeId>,
        anchors: &[NodeId],
        focus: Option<NodeId>,
        GeometryWork { mut candidates, scrollports }: GeometryWork,
    ) -> Result<Vec<NodeId>, JsValue> {
        candidates.extend(&self.demands);
        candidates.extend(tree.focus().filter(|&node| tree.editor(node).is_some()));
        for demand in &delta.geometry_demands {
            let node = match *demand {
                GeometryDemand::Bounds(node) => {
                    if self.entries.get(&node).is_some_and(|entry| !entry.bounds_demand) {
                        self.remember(node);
                        self.entries.get_mut(&node).expect("retained geometry demand").bounds_demand = true;
                        self.register_indexes(node);
                    }
                    node
                }
                GeometryDemand::Scrollport(node) | GeometryDemand::Editor(node) | GeometryDemand::Anchor(node) => node,
            };
            candidates.insert(node);
        }
        for &node in anchors {
            candidates.insert(node);
            if let Some(target) = tree.overlay(node).and_then(|slot| slot.target).and_then(|(target, _)| target.node()) {
                candidates.insert(target);
            }
        }

        for node in scrollports {
            self.apply_scroll(tree, node, &delta.scroll_dirty, created);
            candidates.insert(node);
        }
        if let Some(focus) = focus {
            self.reveal_focus(tree, focus, &mut candidates);
        }
        candidates.extend(candidates.iter().filter_map(|&node| tree.parent(node)).collect::<Vec<_>>());
        self.measure_nodes(tree, candidates)
    }

    fn reveal_focus(&mut self, tree: &Tree, focus: NodeId, candidates: &mut FxHashSet<NodeId>) {
        let Some(target) = self.projected_element(focus) else { return };
        for node in std::iter::successors(tree.parent(focus), |&node| tree.parent(node)) {
            let Some(slot) = tree.scroll(node) else { continue };
            if slot.requested.is_some() || tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE)) {
                continue;
            }
            let Some(port) = self.projected_element(node) else { continue };
            let bounds = port.get_bounding_client_rect();
            let target = target.get_bounding_client_rect();
            let offset = undo::scroll_offset(&port);
            let x = target.x() as f32 - bounds.x() as f32 - port.client_left() as f32 + offset.x;
            let y = target.y() as f32 - bounds.y() as f32 - port.client_top() as f32 + offset.y;
            let range = ScrollRange {
                max: Vec2::new((port.scroll_width() - port.client_width()) as f32, (port.scroll_height() - port.client_height()) as f32),
            };
            let next = range.clamp(Vec2::new(
                ScrollRange::reveal_axis(slot.offset.x, port.client_width() as f32, x, x + target.width() as f32),
                ScrollRange::reveal_axis(slot.offset.y, port.client_height() as f32, y, y + target.height() as f32),
            ));
            if next != offset {
                self.pending().undo.snapshot_scroll(&port, offset);
                undo::scroll_to(&port, next);
            }
            candidates.insert(node);
        }
    }

    fn apply_scroll(&mut self, tree: &Tree, node: NodeId, dirty: &FxHashSet<NodeId>, created: &FxHashSet<NodeId>) {
        let Some(element) = self.projected_element(node) else { return };
        let slot = tree.scroll(node).filter(|_| tree.is_live(node));
        let scrollable = slot.is_some();
        if self.entries.get(&node).is_some_and(|entry| entry.scrollport != scrollable) {
            self.remember(node);
            self.entries.get_mut(&node).expect("retained scrollport").scrollport = scrollable;
            self.register_indexes(node);
        }
        let Some(slot) = slot else { return };
        let follows = tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE));
        if slot.requested.is_some() || dirty.contains(&node) || created.contains(&node) || follows {
            let mut target = slot.requested.map_or(slot.offset, |(_, target)| target);
            if follows && slot.requested.is_none() {
                target.y = f32::MAX;
            }
            let offset = undo::scroll_offset(&element);
            if offset != target {
                self.pending().undo.snapshot_scroll(&element, offset);
                undo::scroll_to(&element, target);
            }
        }
    }

    fn measure_nodes(&mut self, tree: &Tree, candidates: FxHashSet<NodeId>) -> Result<Vec<NodeId>, JsValue> {
        let elements = Array::new();
        let mut nodes = Vec::with_capacity(candidates.len());
        for node in candidates {
            if tree.is_live(node)
                && let Some(element) = self.projected_element(node)
            {
                elements.push(&element);
                nodes.push(node);
            }
        }
        let samples: Vec<_> = self.measure.read(&self.surface, &elements)?.collect();
        for (&node, geometry) in nodes.iter().zip(samples) {
            let parent = tree.parent(node);
            let text_clip = tree.text(node).and_then(|_| {
                let visual = tree.visual(node);
                match visual.wrap {
                    Wrap::Truncate if geometry.content.width > geometry.client.width => Some(TextClip::Ellipsis),
                    Wrap::Words if geometry.content.height > geometry.client.height => {
                        visual.max_lines.map(|n| TextClip::LineClamp(u32::from(n)))
                    }
                    _ => None,
                }
            });
            if self
                .entries
                .get(&node)
                .is_some_and(|entry| entry.geometry != Some(geometry) || entry.parent != parent || entry.text_clip != text_clip)
            {
                self.remember(node);
                let entry = self.entries.get_mut(&node).expect("retained geometry");
                entry.geometry = Some(geometry);
                entry.parent = parent;
                entry.text_clip = text_clip;
                self.register_indexes(node);
            }
        }
        Ok(nodes)
    }

    pub(super) fn report_feedback(
        &self,
        tree: &Tree,
        viewport: WindowViewport,
        anchors: &[NodeId],
        measured: &[NodeId],
        feedback: &mut ProjectFeedback,
    ) {
        for &node in measured {
            if let Some(slot) = tree.scroll(node)
                && let Some(geometry) = self.sample(node)
                && (slot.requested.is_some() || slot.offset != geometry.scroll)
            {
                let target = slot.requested.map_or(slot.offset, |(_, target)| target);
                let range = ScrollRange { max: geometry.scroll_max() };
                if slot.requested.is_some()
                    && geometry.scroll.y < range.max.y
                    && tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE))
                {
                    feedback.updates.push(RuntimeOp::ReleaseFollowTail { node });
                }
                feedback.updates.push(RuntimeOp::SetScroll { node, offset: geometry.scroll, clamped: range.clamp(target) != target });
            }
        }
        let safe = viewport.safe_bounds();
        let mut unsettled = FxHashSet::default();
        for &node in anchors {
            let Some(slot) = tree.overlay(node) else { continue };
            let Some((target, placement)) = slot.target else { continue };
            if std::iter::successors(target.node(), |&node| tree.parent(node)).any(|node| unsettled.contains(&node)) {
                unsettled.insert(node);
                continue;
            }
            let anchor = match target {
                AnchorTarget::Node(node) => self.sample(node).map(|geometry| geometry.bounds),
                AnchorTarget::EditorCaret(node) => self.text_anchor_rect(tree, node, true),
                AnchorTarget::TextSelection(node) => self.text_anchor_rect(tree, node, false),
                AnchorTarget::Point(_) | AnchorTarget::Rect(_) => target.fixed_rect(),
            };
            let Some((anchor, overlay)) = anchor.zip(self.sample(node)) else {
                if slot.placed.is_some() {
                    feedback.updates.push(RuntimeOp::SetPlacement { node, placed: None });
                }
                continue;
            };
            let mut natural = Size::new(overlay.bounds.w, overlay.bounds.h);
            if let Some(previous) = slot.placed.filter(|previous| previous.limit.is_some()) {
                match previous.side {
                    Side::Top | Side::Bottom => {
                        natural.height = natural.height.max(overlay.content.height + overlay.border.top + overlay.border.bottom)
                    }
                    Side::Left | Side::Right => {
                        natural.width = natural.width.max(overlay.content.width + overlay.border.left + overlay.border.right)
                    }
                }
            }
            let placed = placement.place(anchor, natural, safe);
            if slot.placed != Some(placed) {
                feedback.updates.push(RuntimeOp::SetPlacement { node, placed: Some(placed) });
                unsettled.insert(node);
            }
        }
        for node in tree.overlay_nodes() {
            if !anchors.contains(&node) && tree.overlay(node).is_some_and(|slot| slot.placed.is_some()) {
                feedback.updates.push(RuntimeOp::SetPlacement { node, placed: None });
            }
        }
    }

    pub(super) fn text_anchor_rect(&self, tree: &Tree, node: NodeId, caret: bool) -> Option<Rect> {
        let element = self.projected_element(node)?;
        let points = Array::new();
        if let Some(editor) = tree.editor(node) {
            if tree.focus() != Some(node) {
                return None;
            }
            let (anchor, focus) = editor.displayed_selection()?;
            let (start, end) = if caret { (focus, focus) } else { (anchor.min(focus), anchor.max(focus)) };
            if !caret && start == end {
                return None;
            }
            if element.is_content_editable() {
                for byte in [start, end] {
                    let (point, offset) = crate::rich::point_at(&element, byte);
                    points.push(&point);
                    points.push(&offset.into());
                }
            } else {
                for byte in [start, end] {
                    let offset = editor.displayed_rope().byte_to_utf16(byte)?;
                    points.push(&(offset as u32).into());
                }
            }
        }
        crate::measure::text_anchor(&self.surface, &element, caret, &points)
    }

    pub(super) fn sample(&self, node: NodeId) -> Option<Geometry> {
        self.entries.get(&node)?.geometry
    }
}
