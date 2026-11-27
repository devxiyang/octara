//! Incremental Taffy layout and projected window geometry.

use octara_model::overlay::AnchorTarget;
mod style;

#[cfg(test)]
mod tests;

use crate::text::TextEngine;
use octara_model::geometry::{Edges, LayoutBox, Placed, Point, Rect, Side, Size, Vec2, WindowViewport};
use octara_model::style::{Display, FlexDirection, LayoutLength, Overflow, Position, State};
use octara_model::text::TextStyle;
use octara_runtime::engine::ScrollRange;
use octara_tree::{NodeFlags, NodeId, NodeKind, RetiredNode, RuntimeOp, Tree, TreeDelta};
use rustc_hash::{FxHashMap, FxHashSet};
use slotmap::SecondaryMap;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use taffy::{AvailableSpace, TaffyTree};

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
    client: Rect,
    content: Rect,
    clip: Rect,
    child_clip: Rect,
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

/// Taffy may remain ahead of the published frame after an aborted turn. Record only
/// the first value replaced for each projected entry until that turn is committed.
struct Projected<T> {
    entries: FxHashMap<NodeId, T>,
    undo: FxHashMap<NodeId, Option<T>>,
    before_project: FxHashMap<NodeId, Option<T>>,
}

impl<T> Default for Projected<T> {
    fn default() -> Self {
        Self { entries: FxHashMap::default(), undo: FxHashMap::default(), before_project: FxHashMap::default() }
    }
}

impl<T: Copy + PartialEq> Projected<T> {
    fn set(&mut self, node: NodeId, next: Option<T>) -> bool {
        let current = self.entries.get(&node).copied();
        if current == next {
            return false;
        }
        self.undo.entry(node).or_insert(current);
        self.before_project.entry(node).or_insert(current);
        replace_entry(&mut self.entries, node, next);
        true
    }

    fn changes(&mut self) -> impl Iterator<Item = NodeId> + '_ {
        std::mem::take(&mut self.before_project)
            .into_iter()
            .filter_map(|(node, before)| (self.entries.get(&node).copied() != before).then_some(node))
    }

    fn commit(&mut self) {
        self.undo = FxHashMap::default();
    }

    fn rollback(&mut self) {
        for (node, frame) in std::mem::take(&mut self.undo) {
            replace_entry(&mut self.entries, node, frame);
        }
        self.before_project = FxHashMap::default();
    }
}

impl Frame {
    fn anchor_bounds(self) -> (Rect, Rect) {
        (self.bounds, self.content)
    }
}

fn replace_entry<T>(entries: &mut FxHashMap<NodeId, T>, node: NodeId, value: Option<T>) {
    if let Some(value) = value {
        entries.insert(node, value);
    } else {
        entries.remove(&node);
    }
}

type AnchorTargets = SmallVec<[NodeId; 2]>;

#[derive(Default)]
struct AnchorIndex {
    targets: FxHashMap<NodeId, AnchorTargets>,
    watchers: FxHashMap<NodeId, FxHashSet<NodeId>>,
}

impl AnchorIndex {
    fn sync(&mut self, tree: &Tree, node: NodeId) {
        let mut targets = AnchorTargets::new();
        if let Some(target) = tree.overlay(node).and_then(|slot| slot.target).and_then(|(target, _)| target.node()) {
            targets.push(target);
            if tree.link(target).is_some()
                && let Some(parent) = tree.parent(target)
            {
                targets.push(parent);
            }
        }
        if self.targets.get(&node) == Some(&targets) {
            return;
        }
        for target in self.targets.remove(&node).into_iter().flatten() {
            let watchers = self.watchers.get_mut(&target).expect("indexed anchor dependency");
            watchers.remove(&node);
            if watchers.is_empty() {
                self.watchers.remove(&target);
            }
        }
        if tree.overlay(node).is_some_and(|slot| slot.target.is_some()) {
            for &target in &targets {
                self.watchers.entry(target).or_default().insert(node);
            }
            self.targets.insert(node, targets);
        }
    }
}

pub(crate) struct LayoutEngine {
    taffy: TaffyTree<NodeId>,
    keys: SecondaryMap<NodeId, taffy::NodeId>,
    frames: Projected<Frame>,
    scroll_anchors: Projected<ContentAnchor>,
    keyboard_focus: Option<NodeId>,
    committed_keyboard_focus: Option<NodeId>,
    links: Projected<Frame>,
    changed: FxHashSet<NodeId>,
    scrolled: FxHashSet<NodeId>,
    animated_scrolls: FxHashSet<NodeId>,
    anchors: AnchorIndex,
    placement: Projected<AnchorPlacement>,
    fits: FxHashMap<NodeId, Fit>,
    natural_sizes: FxHashMap<NodeId, NaturalSize>,
    measurements: FxHashMap<NodeId, Measurement>,
    native_sizes: FxHashMap<NodeId, Size>,
    viewport: Option<WindowViewport>,
    font_generation: u64,
    stage_collected: bool,
    needs_collection: bool,
    work: LayoutWork,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        let mut taffy = TaffyTree::new();
        // Taffy's rounding pass can follow a stale hoisted child after its parent becomes a leaf.
        // Keep layout coordinates at logical precision, including at fractional window scales.
        taffy.disable_rounding();
        Self {
            taffy,
            keys: SecondaryMap::new(),
            frames: Projected::default(),
            scroll_anchors: Projected::default(),
            keyboard_focus: None,
            committed_keyboard_focus: None,
            links: Projected::default(),
            changed: FxHashSet::default(),
            scrolled: FxHashSet::default(),
            animated_scrolls: FxHashSet::default(),
            anchors: AnchorIndex::default(),
            placement: Projected::default(),
            fits: FxHashMap::default(),
            natural_sizes: FxHashMap::default(),
            measurements: FxHashMap::default(),
            native_sizes: FxHashMap::default(),
            viewport: None,
            font_generation: 0,
            stage_collected: false,
            needs_collection: false,
            work: LayoutWork::default(),
        }
    }
}

impl LayoutEngine {
    pub(crate) fn project(&mut self, tree: &Tree, delta: &TreeDelta, viewport: WindowViewport, text: &mut TextEngine) -> Vec<RuntimeOp> {
        self.sync(tree, delta);
        self.changed = FxHashSet::default();

        if self.font_generation != text.font_generation() {
            self.font_generation = text.font_generation();
            for (node, core) in tree.nodes() {
                if matches!(core.kind, NodeKind::Text | NodeKind::TextInput)
                    && let Some(&key) = self.keys.get(node)
                {
                    self.taffy.mark_dirty(key).expect("invalidate font metrics");
                }
            }
        }

        let viewport_changed = self.viewport != Some(viewport);
        self.viewport = Some(viewport);
        let root = self.keys[tree.root()];
        let dirty = self.taffy.dirty(root).expect("retained layout root");
        let recompute = dirty || viewport_changed;
        let mut updates = Vec::new();

        let mut projection = Projection::new(
            viewport,
            EditorProjection {
                previous_size: tree.focus().and_then(|node| self.content_bounds(node)).map(rect_size),
                reveal: tree.focus().is_some_and(|node| delta.editor_dirty.contains(&node)),
            },
        );
        if recompute {
            for (node, _) in std::mem::take(&mut self.fits) {
                if let Some(&key) = self.keys.get(node) {
                    self.taffy.set_style(key, compile(tree, node, Fit::default(), viewport.size)).expect("reset overlay sizing");
                }
            }
            self.compute(tree, viewport, text);
            self.natural_sizes.clear();
        }

        if std::mem::take(&mut self.needs_collection) || recompute {
            self.collect_frames(tree, text, &mut projection);
            for &node in self.anchors.targets.keys() {
                projection.queue_anchor(tree, node);
            }
        } else {
            let mut roots: Vec<_> = delta.scroll_dirty.union(&delta.editor_dirty).copied().collect();
            roots.extend(delta.overlay_dirty.iter().copied());
            roots.sort_unstable_by_key(|&node| (physical_depth(tree, node), node));
            let mut visited = FxHashSet::default();
            for node in roots {
                if !visited.contains(&node) {
                    self.walk_frames(tree, text, node, &mut projection, &mut visited, false);
                }
            }
        }
        for &node in &delta.overlay_dirty {
            projection.queue_anchor(tree, node);
        }
        for &node in &delta.layout_dirty {
            if tree.text(node).is_some() {
                self.queue_anchor_watchers(tree, node, &mut projection);
            }
        }
        for retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = *retired {
                self.queue_anchor_watchers(tree, node, &mut projection);
            }
        }
        if recompute || !delta.editor_dirty.is_empty() || !delta.semantics_dirty.is_empty() || !delta.scroll_dirty.is_empty() {
            for &node in self.anchors.targets.keys() {
                if tree
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
        self.report_scroll(tree, &projection.scroll, &mut updates);
        self.record_scroll_anchors(tree, &projection.scroll);
        self.scrolled = self
            .frames
            .before_project
            .iter()
            .filter_map(|(&node, previous)| {
                let frame = self.frames.entries.get(&node)?;
                (frame.scroll.is_some() && previous.is_some_and(|previous| previous.offset != frame.offset)).then_some(node)
            })
            .collect();
        self.changed.extend(self.frames.changes());
        self.placement.before_project = FxHashMap::default();
        text.project(tree, delta, self);
        self.project_links(tree, text);
        updates
    }

    fn sync(&mut self, tree: &Tree, delta: &TreeDelta) {
        let extent = self.viewport.map_or_else(Size::default, |viewport| viewport.size);
        for &node in &delta.overlay_dirty {
            self.anchors.sync(tree, node);
        }
        // A merged delta can retire a node and create another in the same slot.
        for &retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = retired {
                if let Some(key) = self.keys.remove(node) {
                    self.taffy.remove(key).expect("remove destroyed layout node");
                }
                self.fits.remove(&node);
                self.placement.set(node, None);
                self.natural_sizes.remove(&node);
                self.measurements.remove(&node);
                self.native_sizes.remove(&node);
                self.anchors.sync(tree, node);
            }
        }
        for &node in &delta.created {
            let Some(core) = tree.node(node) else { continue };
            if core.kind == NodeKind::Link || self.keys.contains_key(node) {
                continue;
            }
            let style = compile(tree, node, Fit::default(), extent);
            let key = if matches!(
                core.kind,
                NodeKind::Text | NodeKind::TextInput | NodeKind::Native | NodeKind::Image | NodeKind::Media | NodeKind::Icon
            ) {
                self.taffy.new_leaf_with_context(style, node)
            } else {
                self.taffy.new_leaf(style)
            }
            .expect("allocate retained layout node");
            self.keys.insert(node, key);
        }

        for &node in &delta.topology_dirty {
            let Some(&key) = self.keys.get(node) else { continue };
            let children: Vec<_> = tree
                .children(node)
                .iter()
                .filter(|&&child| tree.node(child).is_some_and(|core| !core.flags.contains(NodeFlags::EXITING)))
                .filter_map(|&child| self.keys.get(child).copied())
                .collect();
            if self.taffy.children(key).expect("retained layout children") != children {
                self.taffy.set_children(key, &children).expect("synchronize layout children");
            }
        }

        for &node in delta.layout_dirty.union(&delta.overlay_dirty) {
            let Some(&key) = self.keys.get(node) else { continue };
            let anchored = tree.overlay(node).is_some_and(|slot| slot.target.is_some());
            let fit = if anchored { self.fits.get(&node).copied().unwrap_or_default() } else { Fit::default() };
            let style = compile(tree, node, fit, extent);
            let measurement =
                delta.layout_dirty.contains(&node) && self.taffy.get_node_context(key).is_some() && self.sync_measurement(tree, node);
            if *self.taffy.style(key).expect("retained layout style") != style {
                self.taffy.set_style(key, style).expect("synchronize layout style");
            } else if measurement {
                self.taffy.mark_dirty(key).expect("invalidate retained measurement");
            }
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

    fn compute(&mut self, tree: &Tree, viewport: WindowViewport, text: &mut TextEngine) {
        self.work.computes += 1;
        let available_space =
            taffy::Size { width: AvailableSpace::Definite(viewport.size.width), height: AvailableSpace::Definite(viewport.size.height) };
        let sizes = &self.native_sizes;
        self.taffy
            .compute_layout_with_measure(self.keys[tree.root()], available_space, |inputs, _, context, style| {
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, available| {
                        let size = context.map_or_else(Size::default, |&mut node| {
                            let width = known.width.or(match available.width {
                                AvailableSpace::Definite(width) => Some(width),
                                AvailableSpace::MinContent => Some(0.0),
                                AvailableSpace::MaxContent => None,
                            });
                            if let Some(size) = text.measure(tree, node, known.width, width) {
                                size
                            } else if let Some(size) = sizes.get(&node) {
                                *size
                            } else if tree.icon(node).is_some() {
                                Size::new(tree.visual(node).font_size, tree.visual(node).font_size)
                            } else if let Some((width, height)) = tree
                                .image(node)
                                .map(|slot| slot.image.size())
                                .or_else(|| tree.media_source(node).map(|slot| slot.natural_size().unwrap_or((300, 150))))
                            {
                                let ratio = width as f32 / (height as f32).max(1.0);
                                match (known.width, known.height) {
                                    (Some(w), None) => Size::new(w, w / ratio.max(f32::EPSILON)),
                                    (None, Some(h)) => Size::new(h * ratio, h),
                                    _ => Size::new(width as f32, height as f32),
                                }
                            } else {
                                Size::new(300.0, 150.0)
                            }
                        });
                        taffy::Size { width: known.width.unwrap_or(size.width), height: known.height.unwrap_or(size.height) }
                    },
                )
            })
            .expect("compute retained layout");
    }

    fn collect_frames(&mut self, tree: &Tree, text: &mut TextEngine, projection: &mut Projection) {
        self.stage_collected = true;
        let mut visited = FxHashSet::default();
        self.walk_frames(tree, text, tree.root(), projection, &mut visited, true);
        let stale: Vec<_> = self.frames.entries.keys().filter(|node| !visited.contains(node)).copied().collect();
        for node in stale {
            self.set_frame(tree, node, None, projection);
        }
    }

    fn walk_frames(
        &mut self,
        tree: &Tree,
        text: &mut TextEngine,
        root: NodeId,
        projection: &mut Projection,
        visited: &mut FxHashSet<NodeId>,
        full: bool,
    ) {
        let window = projection.window;
        let parent = if root == tree.root() {
            Some(FrameContext { clip: window, ..Default::default() })
        } else {
            tree.parent(root).and_then(|node| {
                let frame = self.frames.entries.get(&node)?;
                Some(FrameContext::children(*frame, node == tree.overlay_root(), window))
            })
        };
        let mut remaining: SmallVec<[_; 16]> = [(root, parent)].into_iter().collect();
        while let Some((node, parent)) = remaining.pop() {
            self.work.frames += 1;
            let overlay = tree.overlay(node);
            let anchor = overlay.and_then(|overlay| overlay.target);
            let visible = tree.is_live(node)
                && tree.layout_style(node).display != Display::None
                && overlay.is_none_or(|overlay| overlay.mount.is_none() || tree.is_shown(node))
                && anchor.is_none_or(|(target, _)| {
                    target.node().is_none_or(|target| tree.is_shown(target))
                        && self.placement.entries.get(&node) != Some(&AnchorPlacement::Suppressed)
                });
            let previous = self.frames.entries.get(&node).copied();
            let next = parent
                .zip(self.keys.get(node).copied())
                .filter(|_| visible)
                .map(|(parent, key)| self.frame(tree, text, node, key, parent, projection));

            if next.is_some() || !full {
                visited.insert(node);
            }
            self.set_frame(tree, node, next, projection);
            if next.is_some_and(|frame| frame.scroll.is_some()) {
                projection.scroll.insert(node);
            }
            let child_context = |frame| FrameContext::children(frame, node == tree.overlay_root(), window);
            let child = next.map(child_context);
            if (full && child.is_some() || previous.map(child_context) != child) && tree.text(node).is_none() {
                remaining.extend(tree.children(node).iter().rev().map(|&node| (node, child)));
            }
        }
    }

    fn set_frame(&mut self, tree: &Tree, node: NodeId, next: Option<Frame>, projection: &mut Projection) {
        let previous = self.frames.entries.get(&node).copied();
        if self.frames.set(node, next) && previous.map(Frame::anchor_bounds) != next.map(Frame::anchor_bounds) {
            self.queue_anchor_watchers(tree, node, projection);
        }
        if next.is_none_or(|frame| frame.scroll.is_none()) {
            self.scroll_anchors.set(node, None);
        }
    }

    fn queue_anchor_watchers(&self, tree: &Tree, target: NodeId, projection: &mut Projection) {
        if let Some(watchers) = self.anchors.watchers.get(&target) {
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
        key: taffy::NodeId,
        parent: FrameContext,
        projection: &Projection,
    ) -> Frame {
        let result = self.taffy.layout(key).expect("computed layout");
        let FrameContext { origin, clip, offset: parent_scroll } = parent;
        let position = match self.placement.entries.get(&node) {
            Some(AnchorPlacement::Placed(placed)) if tree.overlay(node).is_some_and(|slot| slot.target.is_some()) => Some(*placed),
            _ => None,
        };
        let (x, y) = position
            .map_or((origin.x + result.location.x - parent_scroll.x, origin.y + result.location.y - parent_scroll.y), |placement| {
                (placement.x, placement.y)
            });
        let bounds = Rect::new(x, y, result.size.width, result.size.height);
        let border = edges(result.border);
        let padding = edges(result.padding);
        let client = inner(
            bounds,
            Edges { right: border.right + result.scrollbar_size.width, bottom: border.bottom + result.scrollbar_size.height, ..border },
        );
        let content = inner(client, padding);
        let native = self.taffy.style(key).expect("compiled layout");
        let clip_x = native.overflow.x != taffy::Overflow::Visible;
        let clip_y = native.overflow.y != taffy::Overflow::Visible;
        let child_clip = clip_axes(clip, client, clip_x, clip_y);
        let mut max = Vec2::new(
            if native.overflow.x == taffy::Overflow::Scroll { (result.scrollable_overflow_rect.right - client.w).max(0.0) } else { 0.0 },
            if native.overflow.y == taffy::Overflow::Scroll { (result.scrollable_overflow_rect.bottom - client.h).max(0.0) } else { 0.0 },
        );
        let slot = tree.scroll(node);
        if tree.editor(node).is_some_and(|editor| editor.multiline) {
            max.y = max.y.max(text.editor_scroll_range(tree, node, content.w, content.h));
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
                && (projection.editor.reveal || projection.editor.previous_size != Some(rect_size(content)))
                && let Some(caret) = text.editor_caret_in_content(tree, node, content.w)
            {
                Vec2::new(slot.offset.x, ScrollRange::reveal_axis(slot.offset.y, content.h, caret.y, caret.bottom()))
            } else if let Some(&focus) = projection.focus_reveals.get(&node) {
                focus
            } else if let Some(delta) = self.scroll_anchor_delta(tree, node) {
                Vec2::new(slot.offset.x, slot.offset.y + delta)
            } else {
                slot.offset
            };
            let offset = range.clamp(target);
            (offset, offset != target)
        });
        let rect = if position.is_some() {
            Rect::new(x - origin.x + parent_scroll.x, y - origin.y + parent_scroll.y, bounds.w, bounds.h)
        } else {
            Rect::new(result.location.x, result.location.y, bounds.w, bounds.h)
        };
        Frame {
            layout: LayoutBox { rect, padding, border },
            bounds,
            client,
            content,
            clip,
            child_clip,
            clip_axes: [clip_x, clip_y],
            scroll,
            offset,
            scroll_clamped,
        }
    }

    fn scroll_anchor_delta(&self, tree: &Tree, node: NodeId) -> Option<f32> {
        let anchor = self.scroll_anchors.entries.get(&node)?;
        if !tree.maintains_scroll_anchor(node) || tree.parent(anchor.child) != Some(node) || !anchor_child(tree, anchor.child) {
            return None;
        }
        let layout = self.taffy.layout(*self.keys.get(anchor.child)?).ok()?;
        Some(layout.location.y - anchor.y)
    }

    fn record_scroll_anchors(&mut self, tree: &Tree, scrollports: &FxHashSet<NodeId>) {
        for &node in scrollports {
            let anchor = tree
                .maintains_scroll_anchor(node)
                .then(|| {
                    let port = self.frames.entries.get(&node)?;
                    tree.children(node).iter().copied().filter(|&child| anchor_child(tree, child)).find_map(|child| {
                        let frame = self.frames.entries.get(&child)?;
                        (!frame.bounds.intersect(&port.child_clip).is_empty()).then_some(ContentAnchor { child, y: frame.layout.rect.y })
                    })
                })
                .flatten();
            self.scroll_anchors.set(node, anchor);
        }
        self.scroll_anchors.before_project.clear();
    }

    fn reveal_focus(&mut self, tree: &Tree, text: &mut TextEngine, focus: NodeId, projection: &mut Projection) {
        let bounds = if let Some(link) = tree.link(focus) {
            tree.parent(focus).and_then(|parent| text.link_bounds_in(tree, parent, &link.range, self.content_bounds(parent)?))
        } else {
            self.bounds(focus)
        };
        let Some(mut target) = bounds else { return };
        let mut outermost = None;
        for node in std::iter::successors(tree.parent(focus), |&node| tree.parent(node)) {
            let Some(frame) = self.frames.entries.get(&node) else { break };
            let Some(range) = frame.scroll else { continue };
            let Some(slot) = tree.scroll(node) else { continue };
            if slot.requested.is_some() || self.animated_scrolls.contains(&node) || follows_tail(tree, node) {
                continue;
            }
            let client = frame.client;
            let offset = frame.offset;
            let desired = range.clamp(Vec2::new(
                ScrollRange::reveal_axis(slot.offset.x, client.w, target.x - client.x + offset.x, target.right() - client.x + offset.x),
                ScrollRange::reveal_axis(slot.offset.y, client.h, target.y - client.y + offset.y, target.bottom() - client.y + offset.y),
            ));
            projection.focus_reveals.insert(node, desired);
            if desired != offset {
                target.x += offset.x - desired.x;
                target.y += offset.y - desired.y;
                outermost = Some(node);
            }
        }
        if let Some(node) = outermost {
            self.walk_frames(tree, text, node, projection, &mut FxHashSet::default(), false);
        }
    }

    fn anchor_bounds(&self, tree: &Tree, text: &mut TextEngine, target: AnchorTarget) -> Option<Rect> {
        if target.node().is_some_and(|node| !tree.is_shown(node)) {
            return None;
        }
        match target {
            AnchorTarget::Node(node) => {
                if let Some(link) = tree.link(node) {
                    let parent = tree.parent(node)?;
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
                if self.frames.entries.contains_key(&node) {
                    self.walk_frames(tree, text, node, projection, &mut FxHashSet::default(), false);
                }
                if slot.and_then(|slot| slot.placed).is_some() {
                    updates.push(RuntimeOp::SetPlacement { node, placed: None });
                }
                continue;
            };
            if !self.keys.contains_key(node) || !tree.parent(node).is_some_and(|parent| self.frames.entries.contains_key(&parent)) {
                continue;
            }
            let mut natural =
                self.natural_sizes.get(&node).copied().unwrap_or_else(|| NaturalSize { authored: self.layout_size(node), cross: None });
            let computes = self.work.computes;
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
            if self.work.computes != computes {
                self.collect_frames(tree, text, projection);
            } else if placement_changed || !self.frames.entries.contains_key(&node) {
                self.walk_frames(tree, text, node, projection, &mut FxHashSet::default(), false);
            }
            if slot.and_then(|slot| slot.placed) != Some(placed) {
                updates.push(RuntimeOp::SetPlacement { node, placed: Some(placed) });
            }
        }
    }

    fn apply_fit(&mut self, tree: &Tree, node: NodeId, fit: Fit, viewport: WindowViewport, text: &mut TextEngine) {
        if self.fits.get(&node).copied().unwrap_or_default() != fit {
            if fit == Fit::default() {
                self.fits.remove(&node);
            } else {
                self.fits.insert(node, fit);
            }
            self.taffy.set_style(self.keys[node], compile(tree, node, fit, viewport.size)).expect("fit anchored overlay");
            self.compute(tree, viewport, text);
        }
    }

    fn layout_size(&self, node: NodeId) -> Size {
        let result = self.taffy.layout(self.keys[node]).expect("computed overlay size");
        Size::new(result.size.width, result.size.height)
    }

    fn report_scroll(&self, tree: &Tree, candidates: &FxHashSet<NodeId>, updates: &mut Vec<RuntimeOp>) {
        for &node in candidates {
            if let Some(frame) = self.frames.entries.get(&node)
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
        let mut dirty: FxHashSet<_> = text.changed_links().iter().copied().collect();
        for &node in &self.changed {
            if let Some(text) = tree.text(node) {
                dirty.extend(text.links.iter().copied());
            }
        }
        for node in dirty {
            self.work.links += 1;
            let frame = (|| {
                let parent = self.frames.entries.get(&tree.parent(node)?)?;
                let bounds = text.link_bounds(node)?;
                let rect = Rect::new(bounds.x - parent.bounds.x, bounds.y - parent.bounds.y, bounds.w, bounds.h);
                Some(Frame {
                    layout: LayoutBox { rect, ..Default::default() },
                    bounds,
                    client: bounds,
                    content: bounds,
                    clip: parent.child_clip,
                    child_clip: parent.child_clip,
                    clip_axes: [false; 2],
                    scroll: None,
                    offset: Vec2::default(),
                    scroll_clamped: false,
                })
            })();
            self.links.set(node, frame);
        }
        self.changed.extend(self.links.changes());
    }

    fn projected_frame(&self, node: NodeId) -> Option<&Frame> {
        self.frames.entries.get(&node).or_else(|| self.links.entries.get(&node))
    }

    pub(crate) fn commit_stage(&mut self) {
        self.frames.commit();
        self.scroll_anchors.commit();
        self.committed_keyboard_focus = self.keyboard_focus;
        self.links.commit();
        self.placement.commit();
        self.stage_collected = false;
    }

    pub(crate) fn rollback_stage(&mut self) {
        self.frames.rollback();
        self.scroll_anchors.rollback();
        self.keyboard_focus = self.committed_keyboard_focus;
        self.links.rollback();
        self.placement.rollback();
        self.changed = FxHashSet::default();
        // A full pass can change geometry outside the authored delta. Project those warm Taffy results again on retry.
        self.needs_collection |= std::mem::take(&mut self.stage_collected);
    }

    pub(crate) fn work(&self) -> LayoutWork {
        self.work
    }

    pub(crate) fn set_native_measure(&mut self, node: NodeId, size: Size) {
        if size.width.is_finite()
            && size.height.is_finite()
            && size.width >= 0.0
            && size.height >= 0.0
            && let Some(&key) = self.keys.get(node)
            && self.native_sizes.insert(node, size) != Some(size)
        {
            self.taffy.mark_dirty(key).expect("invalidate native view measurement");
        }
    }

    pub(crate) fn geometry(&self, node: NodeId) -> Option<LayoutBox> {
        self.projected_frame(node).map(|frame| frame.layout)
    }

    pub(crate) fn bounds(&self, node: NodeId) -> Option<Rect> {
        self.projected_frame(node).map(|frame| frame.bounds)
    }

    pub(crate) fn content_bounds(&self, node: NodeId) -> Option<Rect> {
        self.projected_frame(node).map(|frame| frame.content)
    }

    pub(crate) fn scroll_client(&self, node: NodeId) -> Option<Rect> {
        let frame = self.frames.entries.get(&node)?;
        frame.scroll.map(|_| frame.client)
    }

    pub(crate) fn scroll_anchor(&self, node: NodeId) -> Option<NodeId> {
        self.scroll_anchors.entries.get(&node).map(|anchor| anchor.child)
    }

    pub(crate) fn clip(&self, node: NodeId) -> Option<Rect> {
        self.projected_frame(node).map(|frame| frame.clip)
    }

    pub(crate) fn child_clip(&self, node: NodeId) -> Option<Rect> {
        self.frames.entries.get(&node).filter(|frame| frame.clip_axes != [false; 2]).map(|frame| frame.child_clip)
    }

    /// An ancestor's scissor can intersect the padding box without changing its corners.
    pub(crate) fn rounded_child_clip(&self, node: NodeId) -> Option<Rect> {
        self.frames.entries.get(&node).filter(|frame| frame.clip_axes == [true; 2]).map(|frame| frame.client)
    }

    pub(crate) fn scroll_range(&self, node: NodeId) -> Option<ScrollRange> {
        self.frames.entries.get(&node)?.scroll
    }

    pub(crate) fn scroll_offset(&self, node: NodeId) -> Vec2 {
        self.frames.entries.get(&node).map_or_else(Vec2::default, |frame| frame.offset)
    }

    pub(crate) fn changed(&self) -> &FxHashSet<NodeId> {
        &self.changed
    }

    pub(crate) fn scrolled(&self) -> &FxHashSet<NodeId> {
        &self.scrolled
    }

    pub(crate) fn set_animated_scrolls(&mut self, nodes: impl Iterator<Item = NodeId>) {
        self.animated_scrolls.clear();
        self.animated_scrolls.extend(nodes);
    }
}

fn compile(tree: &Tree, node: NodeId, fit: Fit, extent: Size) -> taffy::Style {
    let mut layout = tree.layout_style(node);
    let full = Some(LayoutLength::Pct(1.0));
    if node == tree.root() || node == tree.overlay_root() {
        layout.size = Size::new(full, full);
        layout.flex_direction = FlexDirection::Column;
    }
    if node == tree.overlay_root() || tree.overlay(node).is_some_and(|overlay| overlay.target.is_some()) {
        let zero = Some(LayoutLength::Px(0.0));
        layout.position = Position::Absolute;
        layout.inset = Edges { left: zero, top: zero, ..Edges::default() };
    }
    if let Some(width) = fit.width {
        limit_axis(width, extent.width, &mut layout.min_size.width, &mut layout.max_size.width);
        layout.overflow_x = Overflow::Scroll;
    }
    if let Some(height) = fit.height {
        limit_axis(height, extent.height, &mut layout.min_size.height, &mut layout.max_size.height);
        layout.overflow_y = Overflow::Scroll;
    }
    style::compile(&layout)
}

fn limit_axis(limit: f32, available: f32, min: &mut Option<LayoutLength>, max: &mut Option<LayoutLength>) {
    let resolve = |length| match length {
        LayoutLength::Px(px) => px,
        LayoutLength::Pct(fraction) => fraction * available,
    };
    if min.is_some_and(|min| resolve(min) > limit) {
        *min = Some(LayoutLength::Px(limit));
    }
    if max.is_none_or(|max| resolve(max) > limit) {
        *max = Some(LayoutLength::Px(limit));
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
    clip: Rect,
    offset: Vec2,
}

impl FrameContext {
    fn children(frame: Frame, overlay_root: bool, window: Rect) -> Self {
        Self {
            origin: Point::new(frame.bounds.x, frame.bounds.y),
            clip: if overlay_root { window } else { frame.child_clip },
            offset: frame.offset,
        }
    }
}

struct Projection {
    viewport: WindowViewport,
    window: Rect,
    editor: EditorProjection,
    focus_reveals: FxHashMap<NodeId, Vec2>,
    scroll: FxHashSet<NodeId>,
    anchors: BTreeSet<(usize, NodeId)>,
    placed: FxHashSet<NodeId>,
}

impl Projection {
    fn new(viewport: WindowViewport, editor: EditorProjection) -> Self {
        Self {
            viewport,
            window: Rect::new(0.0, 0.0, viewport.size.width, viewport.size.height),
            editor,
            focus_reveals: FxHashMap::default(),
            scroll: FxHashSet::default(),
            anchors: BTreeSet::new(),
            placed: FxHashSet::default(),
        }
    }

    fn queue_anchor(&mut self, tree: &Tree, node: NodeId) {
        if !self.placed.contains(&node) {
            // Resolve targets within an anchored overlay after their enclosing anchor.
            let depth = std::iter::successors(Some(node), |&node| {
                tree.overlay(node).and_then(|slot| slot.target.and_then(|(target, _)| target.node())).or_else(|| tree.parent(node))
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
    std::iter::successors(tree.parent(node), |&parent| tree.parent(parent)).count()
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

fn clip_axes(clip: Rect, client: Rect, x: bool, y: bool) -> Rect {
    clip.intersect(&Rect::new(
        if x { client.x } else { clip.x },
        if y { client.y } else { clip.y },
        if x { client.w } else { clip.w },
        if y { client.h } else { clip.h },
    ))
}
