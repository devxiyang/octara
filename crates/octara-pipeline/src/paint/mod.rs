//! Compile incremental local drawings into immutable presentation scenes.

mod scene;
mod scrollbars;
pub use scene::PaintScene;

use crate::frame_arena::FrameArena;
use crate::layout::LayoutEngine;
use crate::motion::{MotionOptions, NodeMotion, TargetMotion, next_frame};
use crate::text::{PaintKey, TextEngine};
use octara_model::geometry::{LayoutBox, Point, Rect, Size, Vec2, WindowViewport};
use octara_model::image::MediaAsset;
use octara_model::input::HostReply;
use octara_model::motion::{PropClass, TextRevealTrack, Track};
use octara_model::native::NativeSlot;
use octara_model::paint::{
    CanvasOp, CustomPaint, DisplayList, Image, ImagePaint, Item, NodeIcon, PaintInput, PaintMount, PaintProgram, PathPaint, Quad, Sample,
    ScrollPlacement, ScrollPlacements, Shadow, Transform, TransitionPaint,
};
use octara_model::style::{Display, Ring, Visual};
use octara_model::text::{InlineAttachment, InlineContent};
use octara_model::theme::{BoxShadow, Color, ColorToken, Fill, Gradient};
use octara_tree::{DirtyBitmap, ExitTicket, NodeFlags, NodeId, RetiredNode, Tree, TreeDelta};
use rustc_hash::{FxHashMap, FxHashSet};
use scene::{ChildList, Element, ExitGhost, PaintNode, Translation};
use scrollbars::Scrollbars;
use smallvec::SmallVec;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
struct Decoration {
    source: NodeId,
    borders: [f32; 4],
    shadows: Vec<BoxShadow>,
    custom: Option<(PaintProgram, PaintInput, PaintMount)>,
}

impl Decoration {
    fn items(&self, rect: Rect, visual: Visual) -> Vec<Item> {
        let mut items = Vec::new();
        let radius = radius(visual, rect);
        for shadow in self.shadows.iter().rev() {
            let spread = shadow.spread_radius;
            let rect = Rect::new(
                rect.x + shadow.offset.0 - spread,
                rect.y + shadow.offset.1 - spread,
                rect.w + 2.0 * spread,
                rect.h + 2.0 * spread,
            );
            if !rect.is_empty() {
                items.push(Item::Shadow(Shadow {
                    source: self.source,
                    rect,
                    radius: rect.corner_radii(radius.map(|corner| (corner + spread).max(0.0))),
                    color: shadow.color,
                    blur: shadow.blur_radius,
                }));
            }
        }
        if let Some(ring) = visual.ring
            && let Some(outer) = ring_rect(rect, ring)
        {
            items.push(Item::Quad(Quad {
                source: self.source,
                rect: outer,
                radius: radius.map(|corner| if corner > 0.0 { corner + ring.offset + ring.width } else { 0.0 }),
                fill: None,
                gradient: None,
                border: [ring.width; 4],
                border_color: ring.color,
            }));
        }
        if self.custom.is_some()
            || visual.background.is_some()
            || visual.gradient.is_some()
            || self.borders.iter().any(|width| *width > 0.0)
        {
            let quad = Quad {
                source: self.source,
                rect,
                radius,
                fill: visual.background,
                gradient: visual.gradient,
                border: self.borders,
                border_color: visual.border,
            };
            items.push(self.custom.as_ref().map_or(Item::Quad(quad), |(program, input, mount)| {
                Item::Custom(CustomPaint {
                    fallback: quad,
                    program: program.clone(),
                    mount: mount.clone(),
                    exiting: false,
                    input: input.clone(),
                    opacity: 1.0,
                })
            }));
        }
        items
    }

    fn shadow_bounds(&self, rect: Rect) -> impl Iterator<Item = Rect> + '_ {
        self.shadows.iter().map(move |shadow| {
            let extent = shadow.spread_radius + 1.5 * shadow.blur_radius;
            Rect::new(rect.x + shadow.offset.0 - extent, rect.y + shadow.offset.1 - extent, rect.w + 2.0 * extent, rect.h + 2.0 * extent)
        })
    }
}

fn ring_rect(bounds: Rect, ring: Ring) -> Option<Rect> {
    if bounds.is_empty() || ring.color.components[3] <= 0.0 {
        return None;
    }
    let extent = ring.width + ring.offset;
    let rect = Rect::new(bounds.x - extent, bounds.y - extent, bounds.w + 2.0 * extent, bounds.h + 2.0 * extent);
    [rect.x, rect.y, rect.w, rect.h].into_iter().all(f32::is_finite).then_some(rect)
}

#[derive(PartialEq)]
struct DrawKey {
    rev: u32,
    theme: u64,
    visual: Visual,
    layout: LayoutBox,
    text: Option<PaintKey>,
    window_active: bool,
    images: ImageVersions,
    native_snapshot: Option<u64>,
}

type ImageVersions = SmallVec<[(u64, u64); 2]>;
type ImageSamples = SmallVec<[Sample; 2]>;
type Blink = (NodeId, u64, f64);

struct Drawing {
    key: DrawKey,
    decoration: Arc<Decoration>,
    items: Arc<[Item]>,
    content_start: usize,
}

#[derive(Default)]
struct DrawingCache {
    settled: Option<Rc<Drawing>>,
    completed: Option<Rc<Drawing>>,
}

impl DrawingCache {
    fn get(&self, key: &DrawKey) -> Option<Rc<Drawing>> {
        self.settled.iter().chain(self.completed.iter()).find(|drawing| drawing.key == *key).cloned()
    }
}

/// Cumulative work performed by the native paint compiler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaintMetrics {
    pub drawings_compiled: u64,
    pub draw_keys_evaluated: u64,
    pub subtrees_rebuilt: u64,
    /// Persistent child-sequence leaves and branches created by topology or subtree changes.
    pub child_spans_rebuilt: u64,
    /// Scroll placements replaced without recompiling their content.
    pub scroll_wrappers_updated: u64,
    /// Nodes sampled for motion or a caret. Resting spans are shared without sampling their nodes.
    pub timed_nodes_sampled: u64,
    /// Child-sequence leaves and branches examined to assemble timed samples.
    pub timed_child_spans_sampled: u64,
}

#[derive(Clone, Default)]
struct RetainedNode {
    scene: Option<Arc<PaintNode>>,
    motion: Option<Arc<NodeMotion>>,
    images: ImageVersions,
}

#[derive(Default)]
struct Stage {
    nodes: FxHashMap<NodeId, Option<RetainedNode>>,
    scrolls: ScrollPlacements,
    ghosts: FxHashMap<NodeId, Option<Arc<ExitGhost>>>,
    drawings: FxHashMap<NodeId, Rc<Drawing>>,
    destroyed: FxHashSet<NodeId>,
    blink: Option<Blink>,
    window_active: bool,
    image_generation: u64,
    reduced_motion: bool,
    native_snapshots: FxHashMap<NodeId, Sample>,
}

#[derive(Default)]
pub(crate) struct PaintCompiler {
    scrollbars: Scrollbars,
    scrolls: ScrollPlacements,
    cache: FxHashMap<NodeId, DrawingCache>,
    nodes: FxHashMap<NodeId, RetainedNode>,
    ghosts: FxHashMap<NodeId, Arc<ExitGhost>>,
    image_nodes: FxHashSet<NodeId>,
    translation_nodes: FxHashSet<NodeId>,
    stage: Option<Stage>,
    blink: Option<Blink>,
    window_active: bool,
    image_generation: u64,
    reduced_motion: bool,
    metrics: PaintMetrics,
    local_dirty: DirtyBitmap,
    branch_dirty: DirtyBitmap,
    scroll_dirty: DirtyBitmap,
    finished_exits: FrameArena<ExitTicket>,
    last_scene: Option<Arc<PaintScene>>,
    resting_draw: Option<Arc<DisplayList>>,
    native_snapshots: FxHashMap<NodeId, Sample>,
}

pub(crate) struct PaintFrame {
    pub draw: Arc<DisplayList>,
    pub scene: Option<Arc<PaintScene>>,
    pub replies: Vec<HostReply>,
    pub next_deadline: Option<Duration>,
}

pub(crate) struct CompileOptions<'a> {
    pub viewport: WindowViewport,
    pub now: f64,
    pub reduced_motion: bool,
    pub window_active: bool,
    pub verify: bool,
    pub transparent_window: bool,
    pub native_snapshots: &'a FxHashMap<NodeId, Sample>,
}

impl PaintCompiler {
    pub(crate) fn compile(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        layout: &LayoutEngine,
        text: &mut TextEngine,
        options: CompileOptions<'_>,
    ) -> PaintFrame {
        if self.stage.is_none() {
            self.stage = Some(Stage {
                blink: self.blink,
                scrolls: self.scrolls.clone(),
                window_active: self.window_active,
                image_generation: self.image_generation,
                reduced_motion: self.reduced_motion,
                native_snapshots: self.native_snapshots.clone(),
                ..Stage::default()
            });
        }

        let mut local = std::mem::take(&mut self.local_dirty);
        local.union_with(&delta.paint_dirty);
        local.union_with(layout.changed());
        local.union_with(text.changed());
        local.extend(delta.created.iter().copied());
        if self.native_snapshots != *options.native_snapshots {
            local.extend(
                self.native_snapshots
                    .keys()
                    .chain(options.native_snapshots.keys())
                    .filter_map(|&node| (self.native_snapshots.get(&node) != options.native_snapshots.get(&node)).then_some(node)),
            );
            self.native_snapshots.clone_from(options.native_snapshots);
        }
        let mut scroll_updates = std::mem::take(&mut self.scroll_dirty);
        scroll_updates.union_with(layout.scroll_changed());
        let scrollbar_deadline = self.scrollbars.step(tree, layout, &options, &mut scroll_updates);
        if self.window_active != options.window_active {
            self.window_active = options.window_active;
            let regions: FxHashSet<_> = tree.selection_endpoints().filter_map(|node| tree.region_of(node)).collect();
            for region in regions {
                if let Some(ranges) = tree.doc_selection(region).and_then(|selection| tree.selected_text_ranges(region, selection)) {
                    local.extend(ranges.into_iter().filter_map(|(node, range)| (!range.is_empty()).then_some(node)));
                }
            }
        }
        let was_reduced = std::mem::replace(&mut self.reduced_motion, options.reduced_motion);
        let cancel_active_motion = options.reduced_motion && !was_reduced;
        if was_reduced != options.reduced_motion {
            local.extend(tree.text_reveals().map(|(node, _)| node));
        }
        if cancel_active_motion {
            local.extend(self.nodes.iter().filter_map(|(&node, entry)| {
                entry.motion.as_ref().and_then(|motion| motion.rest_at()).is_some_and(|rest| rest > options.now).then_some(node)
            }));
        }
        let mut replies = Vec::new();
        let mut independent: FxHashSet<_> = self.ghosts.keys().copied().collect();
        independent.extend(delta.retired.iter().filter_map(|retired| match *retired {
            RetiredNode::Exiting { ticket, .. } if tree.accepts_exit(ticket) => Some(ticket.node),
            _ => None,
        }));
        for &retired in &delta.retired {
            match retired {
                RetiredNode::Destroyed(node) => {
                    self.set_node(node, None);
                    self.set_ghost(node, None);
                    scroll_updates.insert(node);
                    self.stage.as_mut().expect("open stage").destroyed.insert(node);
                }
                RetiredNode::Exiting { ticket, visible } if tree.accepts_exit(ticket) => {
                    let node = ticket.node;
                    local.insert(node);
                    if !options.reduced_motion
                        && let Some(ghost) = self.capture_exit(tree, ticket, visible, options.now, &independent)
                    {
                        self.set_ghost(node, Some(Arc::new(ghost)));
                    } else {
                        replies.push(HostReply::ExitFinished(ticket));
                    }
                }
                RetiredNode::Exiting { .. } => {}
            }
        }
        let mut finished = std::mem::take(&mut self.finished_exits);
        for ghost in self.ghosts.values() {
            if options.reduced_motion || !ghost.opacity.moving(options.now) || !tree.accepts_exit(ghost.ticket) {
                finished.push(ghost.ticket);
            }
        }
        finished.used().sort_unstable_by_key(|ticket| ticket.node);
        for &ticket in finished.used().iter() {
            self.set_ghost(ticket.node, None);
            local.insert(ticket.node);
            if tree.accepts_exit(ticket) {
                replies.push(HostReply::ExitFinished(ticket));
            }
        }

        finished.finish();
        self.finished_exits = finished;

        let editor =
            tree.focus().filter(|_| options.window_active).and_then(|id| tree.editor(id).map(|editor| (id, editor.document_rev())));
        let previous_blink = self.blink.map(|(node, rev, _)| (node, rev));
        if previous_blink != editor || editor.is_some_and(|(node, _)| delta.editor_dirty.contains(&node)) {
            local.extend(previous_blink.map(|(node, _)| node));
            local.extend(editor.map(|(node, _)| node));
            self.blink = editor.map(|(id, revision)| (id, revision, options.now));
        }

        let image_generation = Image::pixel_generation();
        if self.image_generation != image_generation {
            self.image_generation = image_generation;
            for &node in &self.image_nodes {
                if !local.contains(&node) && self.nodes[&node].images != image_versions(tree, node) {
                    local.insert(node);
                }
            }
        }
        scroll_updates.extend(local.iter().filter(|&node| layout.scroll_range(node).is_some() || self.scrolls.get(node).is_some()));
        for node in scroll_updates.iter() {
            let placement = if layout.scroll_range(node).is_some() {
                let scroll = layout.scroll_offset(node);
                let scrollbars = self.scrollbars.placements(tree, layout, node, self.scrolls.get(node));
                Some(ScrollPlacement { offset: Vec2::new(-scroll.x, -scroll.y), scrollbars })
            } else {
                None
            };
            if self.scrolls.get(node) != placement.as_ref() {
                self.scrolls.set(node, placement);
                self.metrics.scroll_wrappers_updated = self.metrics.scroll_wrappers_updated.saturating_add(1);
            }
        }
        scroll_updates.clear();
        self.scroll_dirty = scroll_updates;
        let mut branches = std::mem::take(&mut self.branch_dirty);
        let mut changed_children = FxHashMap::<NodeId, SmallVec<[NodeId; 4]>>::default();
        for node in local.iter().chain(delta.topology_dirty.iter()) {
            let mut ancestor = Some(node);
            while let Some(id) = ancestor.filter(|&id| branches.insert(id)) {
                ancestor = tree.physical_parent(id);
                if let Some(parent) = ancestor {
                    changed_children.entry(parent).or_default().push(id);
                }
            }
        }
        // A hidden parent can stop descent before the compiler reaches its previous children.
        // Clear their scene slots so a subsequent retirement cannot capture a stale drawing.
        for node in local.iter() {
            if !self.ghosts.contains_key(&node) && (!tree.is_live(node) || layout.bounds(node).is_none()) {
                self.hide_node(node);
            }
        }

        let mut compile = Compile {
            owner: self,
            tree,
            layout,
            text,
            options: &options,
            local,
            branches,
            changed_children,
            topology_dirty: &delta.topology_dirty,
            immediate_layout: !delta.scroll_dirty.is_empty() || !layout.scroll_changed().is_empty(),
            cancel_active_motion,
        };
        let root = compile.node(tree.root());
        let size = options.viewport.size;
        let background = if options.transparent_window { Color::TRANSPARENT } else { tree.theme().get(ColorToken::Background) };
        let scene = if let Some(scene) =
            compile.owner.last_scene.as_ref().filter(|scene| scene.matches(size, background, root.as_ref(), &compile.owner.scrolls))
        {
            scene.clone()
        } else {
            let translations = compile
                .owner
                .last_scene
                .as_ref()
                .filter(|previous| root.as_ref().zip(previous.root.as_ref()).is_some_and(|(a, b)| Arc::ptr_eq(a, b)))
                .map_or_else(|| compile.owner.translations(options.now), |previous| previous.translations.clone());
            let scene = Arc::new(PaintScene { size, background, root, scrolls: compile.owner.scrolls.clone(), translations });
            compile.owner.last_scene = Some(scene.clone());
            compile.owner.resting_draw = None;
            scene
        };
        let (draw, sample) = if scene.timed_at(options.now) {
            scene.sample_counted(options.now)
        } else {
            (compile.owner.resting_draw.get_or_insert_with(|| scene.sample(options.now)).clone(), Default::default())
        };
        compile.owner.metrics.timed_nodes_sampled = compile.owner.metrics.timed_nodes_sampled.saturating_add(sample.nodes);
        compile.owner.metrics.timed_child_spans_sampled = compile.owner.metrics.timed_child_spans_sampled.saturating_add(sample.spans);
        if options.verify {
            compile.verify_draw(&draw, size, background, image_generation);
        }
        let moves = scene.continuously_animated(options.now);
        let blink = compile.owner.blink.map(|(_, _, start)| {
            let next = ((options.now - start).max(0.0) * 2.0).floor() * 0.5 + 0.5 + start;
            Duration::from_secs_f64(next)
        });
        let next_deadline = moves.then(|| next_frame(options.now)).into_iter().chain(blink).chain(scrollbar_deadline).min();
        compile.local.clear();
        compile.branches.clear();
        compile.owner.local_dirty = compile.local;
        compile.owner.branch_dirty = compile.branches;
        PaintFrame { draw, scene: (moves || blink.is_some()).then_some(scene), replies, next_deadline }
    }

    fn hide_node(&mut self, node: NodeId) {
        if let Some(current) = self.nodes.get(&node).filter(|current| current.scene.is_some()) {
            let retained = current.motion.as_ref().map(|track| RetainedNode { motion: Some(track.clone()), ..RetainedNode::default() });
            self.set_node(node, retained);
        }
    }

    fn capture_exit(
        &self,
        tree: &Tree,
        ticket: ExitTicket,
        visible_only: bool,
        now: f64,
        independent: &FxHashSet<NodeId>,
    ) -> Option<ExitGhost> {
        let node = ticket.node;
        let snapshot = self.nodes.get(&node)?.scene.as_ref()?;
        let element = snapshot.element.as_ref()?;
        let bounds = element.motion.as_ref().map_or(element.bounds, |track| track.bounds(now));
        let mut retiring = Vec::new();
        let mut ancestors = SmallVec::<[_; 8]>::new();
        let mut inherited_opacity = 1.0_f64;
        for parent in std::iter::successors(tree.physical_parent(node), |&parent| tree.physical_parent(parent)) {
            let ancestor = self.nodes.get(&parent)?.scene.as_ref()?.element.as_ref()?;
            let opacity = ancestor.motion.as_ref().map_or(ancestor.visual, |track| track.visual(now)).opacity;
            if opacity <= 0.0 {
                return None;
            }
            let offset = self.scrolls.get(parent).map_or(Vec2::default(), |placement| placement.offset);
            let clip = ancestor.clip_at(now);
            if !tree.is_live(parent) {
                inherited_opacity *= f64::from(opacity.min(1.0));
                retiring.push((parent, clip.clone(), offset));
            }
            ancestors.push((clip, offset));
        }
        if visible_only {
            let size = self.last_scene.as_ref()?.size;
            let mut clip = Rect::new(0.0, 0.0, size.width, size.height);
            let mut offset = Vec2::default();
            for (parent_clip, scroll) in ancestors.into_iter().rev() {
                if let Some(parent_clip) = parent_clip {
                    let rect = match parent_clip {
                        Item::PushClip { rect, .. } => Rect::new(rect.x + offset.x, rect.y + offset.y, rect.w, rect.h),
                        Item::PushAxisClip { rect, horizontal: true, .. } => Rect::new(rect.x + offset.x, clip.y, rect.w, clip.h),
                        Item::PushAxisClip { rect, horizontal: false, .. } => Rect::new(clip.x, rect.y + offset.y, clip.w, rect.h),
                        _ => unreachable!("paint ancestor clip"),
                    };
                    clip = clip.intersect(&rect);
                }
                offset.x += scroll.x;
                offset.y += scroll.y;
            }
            let bounds = Rect::new(bounds.x + offset.x, bounds.y + offset.y, bounds.w, bounds.h);
            if bounds.intersect(&clip).is_empty() {
                return None;
            }
        }
        let mut opacity = Track::new(inherited_opacity);
        opacity.retarget(0.0, now, element.visual.motion.opacity, PropClass::Opacity);
        if !opacity.moving(now) {
            return None;
        }
        ExitGhost::capture(ticket, snapshot, now, opacity, independent, &retiring, &self.scrolls)
    }

    fn set_node(&mut self, node: NodeId, next: Option<RetainedNode>) {
        let previous = self.replace_node(node, next);
        self.stage.as_mut().expect("open stage").nodes.entry(node).or_insert(previous);
    }

    fn translations(&self, now: f64) -> Arc<[Translation]> {
        let mut translations: Vec<_> = self
            .translation_nodes
            .iter()
            .filter_map(|&node| {
                let motion = self.nodes.get(&node)?.motion.as_ref()?;
                let until = motion.rest_at().filter(|&rest| rest > now)?;
                Some(Translation { node, target: motion.bounds(f64::INFINITY), motion: motion.clone(), until })
            })
            .collect();
        translations.sort_unstable_by(|a, b| a.until.total_cmp(&b.until));
        translations.into()
    }

    fn replace_node(&mut self, node: NodeId, next: Option<RetainedNode>) -> Option<RetainedNode> {
        if next.as_ref().and_then(|node| node.scene.as_ref()).is_some_and(|scene| scene.translates) {
            self.translation_nodes.insert(node);
        } else {
            self.translation_nodes.remove(&node);
        }
        if let Some(next) = next {
            if next.images.is_empty() {
                self.image_nodes.remove(&node);
            } else {
                self.image_nodes.insert(node);
            }
            self.nodes.insert(node, next)
        } else {
            self.image_nodes.remove(&node);
            self.nodes.remove(&node)
        }
    }

    fn set_ghost(&mut self, node: NodeId, ghost: Option<Arc<ExitGhost>>) {
        let previous = if let Some(ghost) = ghost { self.ghosts.insert(node, ghost) } else { self.ghosts.remove(&node) };
        self.stage.as_mut().expect("open stage").ghosts.entry(node).or_insert(previous);
    }

    pub(crate) fn commit(&mut self) {
        self.scrollbars.commit();
        if let Some(stage) = self.stage.take() {
            for (node, drawing) in stage.drawings {
                self.cache.insert(node, DrawingCache { settled: Some(drawing), completed: None });
            }
            for node in stage.destroyed {
                self.cache.remove(&node);
            }
        }
    }

    pub(crate) fn rollback(&mut self) {
        self.scrollbars.rollback();
        let Some(stage) = self.stage.take() else { return };
        for (node, previous) in stage.nodes {
            self.replace_node(node, previous);
        }
        for (node, previous) in stage.ghosts {
            if let Some(previous) = previous {
                self.ghosts.insert(node, previous);
            } else {
                self.ghosts.remove(&node);
            }
        }
        self.scrolls = stage.scrolls;
        self.blink = stage.blink;
        self.window_active = stage.window_active;
        self.image_generation = stage.image_generation;
        self.reduced_motion = stage.reduced_motion;
        self.native_snapshots = stage.native_snapshots;
        // The tree retains speculative writes. Completed local drawings remain usable by key on retry.
    }

    pub(crate) fn built(&self) -> u64 {
        self.metrics.drawings_compiled
    }

    pub(crate) fn scrollbar_hit(
        &self,
        tree: &Tree,
        layout: &LayoutEngine,
        node: NodeId,
        position: Point,
    ) -> Option<octara_tree::arbiter::ScrollbarHit> {
        scrollbars::hit(tree, layout, node, position)
    }

    pub(crate) fn update_scrollbars(
        &mut self,
        hovered: Option<octara_tree::arbiter::ScrollbarKey>,
        dragged: Option<octara_tree::arbiter::ScrollbarKey>,
    ) -> bool {
        self.scrollbars.interact(hovered, dragged)
    }

    pub(crate) fn metrics(&self) -> PaintMetrics {
        self.metrics
    }
}

struct Compile<'a> {
    owner: &'a mut PaintCompiler,
    tree: &'a Tree,
    layout: &'a LayoutEngine,
    text: &'a mut TextEngine,
    options: &'a CompileOptions<'a>,
    local: DirtyBitmap,
    branches: DirtyBitmap,
    changed_children: FxHashMap<NodeId, SmallVec<[NodeId; 4]>>,
    topology_dirty: &'a DirtyBitmap,
    immediate_layout: bool,
    cancel_active_motion: bool,
}

impl Compile<'_> {
    fn node(&mut self, node: NodeId) -> Option<Arc<PaintNode>> {
        let previous = self.owner.nodes.get(&node);
        if !self.branches.contains(&node) {
            return previous.and_then(|entry| entry.scene.clone());
        }
        let result = if self.tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::EXITING)) {
            let ghost = self.owner.ghosts.get(&node).cloned();
            let previous_children = previous.and_then(|entry| entry.scene.as_ref()).map(|scene| scene.children.clone());
            let children = self.children(node, previous_children);
            RetainedNode { scene: Some(Arc::new(PaintNode::ghost(ghost, children))), ..RetainedNode::default() }
        } else {
            let previous_scene = previous.and_then(|entry| entry.scene.as_ref());
            let previous_children = previous_scene.map(|scene| scene.children.clone());
            let existing = previous_scene.and_then(|scene| scene.element.as_ref());
            let (element, images) = if let Some(element) = existing.filter(|_| !self.local.contains(&node)) {
                (element.clone(), previous.expect("retained element").images.clone())
            } else {
                let (bounds, local) = self.geometry(node)?;
                let visual = self.tree.visual(node);
                let binding = self.tree.paint(node).and_then(|slot| slot.transition.as_ref());
                let motion = (!visual.motion.is_none() || binding.is_some()).then(|| {
                    let previous_motion = previous
                        .and_then(|entry| entry.motion.as_deref())
                        .filter(|motion| !self.cancel_active_motion || motion.rest_at().is_none_or(|rest| rest <= self.options.now));
                    Arc::new(NodeMotion::updated(
                        previous_motion,
                        TargetMotion { bounds, visual, transition: binding.map(|binding| (binding.key, binding.curve)) },
                        MotionOptions {
                            now: self.options.now,
                            reduced: self.options.reduced_motion,
                            immediate_layout: self.immediate_layout,
                        },
                    ))
                });
                let previous_reveal = existing.and_then(|element| element.reveal);
                let samples = image_samples(self.tree, node);
                let key = self.draw_key(node, visual, local, &samples);
                self.owner.metrics.draw_keys_evaluated = self.owner.metrics.draw_keys_evaluated.saturating_add(1);
                let images = key.images.clone();
                let drawing = self.owner.cache.get(&node).and_then(|entry| entry.get(&key)).unwrap_or_else(|| {
                    let drawing = Rc::new(self.drawing(node, key, samples));
                    self.owner.metrics.drawings_compiled = self.owner.metrics.drawings_compiled.saturating_add(1);
                    self.owner.cache.entry(node).or_default().completed = Some(drawing.clone());
                    drawing
                });
                let reveal = self
                    .tree
                    .text_reveal(node)
                    .copied()
                    .filter(|reveal| reveal.enabled(self.options.reduced_motion))
                    .map(|reveal| TextRevealTrack::update(previous_reveal, reveal, self.options.now));
                let mut element = self.element(node, bounds, visual, &drawing, motion);
                element.reveal = reveal;
                element.reveal_tint = reveal.and_then(TextRevealTrack::tint).map(|color| color.resolve(self.tree.theme_at(node)).0);
                self.owner.stage.as_mut().expect("open stage").drawings.insert(node, drawing);
                (Arc::new(element), images)
            };
            let motion = element.motion.clone();
            let children = self.children(node, previous_children);
            let translates = self.tree.children(node).is_empty()
                && !element.scroll
                && element.reveal.is_none()
                && element.caret.is_none()
                && element.transition.is_none()
                && !element.own.iter().any(|item| matches!(item, Item::Native { .. } | Item::Custom(_)))
                && motion.as_ref().is_some_and(|motion| motion.translation_only(self.options.now));
            let scene = Some(Arc::new(PaintNode::element(element, children, translates)));
            RetainedNode { scene, motion, images }
        };
        self.owner.metrics.subtrees_rebuilt = self.owner.metrics.subtrees_rebuilt.saturating_add(1);
        let scene = result.scene.clone();
        self.owner.set_node(node, Some(result));
        scene
    }

    fn children(&mut self, node: NodeId, previous: Option<ChildList>) -> ChildList {
        let children = self.tree.children(node);
        let dirty = self.changed_children.remove(&node).unwrap_or_default();
        let height = (usize::BITS - children.len().leading_zeros()) as usize;
        let sparse = dirty.is_empty() || dirty.len().saturating_mul(height) < children.len();
        let mut rebuilt = 0;
        let list = if let Some(mut previous) = previous.filter(|previous| {
            sparse
                && !self.topology_dirty.contains(&node)
                && dirty.iter().all(|&child| previous.has_z_index(child, self.tree.stacking_key(child).1))
        }) {
            for child in dirty {
                let paint = self.node(child);
                previous.replace(child, paint, &mut rebuilt);
            }
            previous
        } else {
            let order = crate::paint_order(self.tree, node);
            ChildList::new(order.iter().map(|&child| (child, self.tree.stacking_key(child).1, self.node(child))), &mut rebuilt)
        };
        self.owner.metrics.child_spans_rebuilt = self.owner.metrics.child_spans_rebuilt.saturating_add(rebuilt);
        list
    }

    #[cold]
    #[inline(never)]
    fn verify_draw(&mut self, draw: &DisplayList, size: Size, background: Color, image_generation: u64) {
        let root = self.uncached_node(self.tree.root());
        if image_generation == Image::pixel_generation() {
            let scene = PaintScene {
                root,
                size,
                background,
                scrolls: self.owner.scrolls.clone(),
                translations: self.owner.translations(self.options.now),
            };
            assert_eq!(draw.items(), scene.sample(self.options.now).items(), "incremental paint projection diverged");
        }
    }

    fn uncached_node(&mut self, node: NodeId) -> Option<Arc<PaintNode>> {
        if self.tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::EXITING)) {
            let ghost = self.owner.ghosts.get(&node).cloned();
            let children = self.uncached_children(node);
            return Some(Arc::new(PaintNode::ghost(ghost, children)));
        }
        let (bounds, local) = self.geometry(node)?;
        let visual = self.tree.visual(node);
        let samples = image_samples(self.tree, node);
        let drawing = self.drawing(node, self.draw_key(node, visual, local, &samples), samples);
        let motion = self.owner.nodes.get(&node).and_then(|entry| entry.motion.clone());
        let mut element = self.element(node, bounds, visual, &drawing, motion);
        if let Some(sample) = self.owner.nodes.get(&node).and_then(|entry| entry.scene.as_ref()?.element.as_ref()) {
            element.reveal = sample.reveal;
            element.reveal_tint = sample.reveal_tint;
        }
        let children = self.uncached_children(node);
        let translates = self.owner.nodes.get(&node).and_then(|node| node.scene.as_ref()).is_some_and(|scene| scene.translates);
        Some(Arc::new(PaintNode::element(Arc::new(element), children, translates)))
    }

    fn uncached_children(&mut self, node: NodeId) -> ChildList {
        let order = crate::paint_order(self.tree, node);
        ChildList::new(order.iter().map(|&child| (child, self.tree.stacking_key(child).1, self.uncached_node(child))), &mut 0)
    }

    fn geometry(&self, node: NodeId) -> Option<(Rect, LayoutBox)> {
        if !self.tree.is_live(node) || self.tree.layout_style(node).display == Display::None {
            return None;
        }
        let bounds = self.layout.paint_bounds(node)?;
        let layout = self.layout.geometry(node)?;
        let local = LayoutBox { rect: Rect::new(0.0, 0.0, bounds.w, bounds.h), ..layout };
        Some((bounds, local))
    }

    fn draw_key(&self, node: NodeId, visual: Visual, local: LayoutBox, images: &[Sample]) -> DrawKey {
        DrawKey {
            rev: self.tree.node(node).expect("shown paint node").paint_rev,
            theme: self.tree.theme_revision_at(node),
            visual,
            layout: local,
            text: self.text.paint_key(node),
            window_active: self.options.window_active,
            images: images.iter().map(|sample| (sample.source().id(), sample.frame().version)).collect(),
            native_snapshot: self.options.native_snapshots.get(&node).map(|image| image.source().id()),
        }
    }

    fn drawing(&mut self, node: NodeId, key: DrawKey, samples: ImageSamples) -> Drawing {
        let decoration = Arc::new(Decoration {
            source: node,
            shadows: key.visual.shadow.resolve(self.tree.theme_at(node)),
            borders: [key.layout.border.top, key.layout.border.right, key.layout.border.bottom, key.layout.border.left],
            custom: self
                .tree
                .paint(node)
                .and_then(|slot| slot.custom.as_ref())
                .map(|custom| (custom.program.clone(), custom.input.clone(), custom.mount.clone())),
        });
        let mut items = decoration.items(key.layout.rect, key.visual);
        let content_start = items.len();
        self.content(node, key.layout, key.visual, samples.into_iter(), &mut items);
        Drawing { key, decoration, items: items.into(), content_start }
    }

    fn element(&self, node: NodeId, bounds: Rect, visual: Visual, drawing: &Drawing, motion: Option<Arc<NodeMotion>>) -> Element {
        let transition = self.tree.paint(node).and_then(|slot| slot.transition.as_ref()).map(|binding| TransitionPaint {
            source: node,
            mount: binding.mount.clone(),
            run: binding.mount.clone(),
            revision: binding.key,
            program: binding.program.clone(),
            progress: 1.0,
            opacity: visual.opacity,
        });
        let caret = self.owner.blink.filter(|(focus, _, _)| *focus == node).and_then(|(_, _, start)| {
            let local = self.text.local_caret(node)?;
            let rect = Rect::new(bounds.x + local.x, bounds.y + local.y, local.w, local.h);
            Some((Item::Quad(solid(node, rect, visual.text)), start))
        });
        Element {
            source: node,
            bounds,
            child_clip: self.layout.local_client(node).and_then(|client| {
                let axes = self.layout.clip_axes(node);
                (axes != [false; 2]).then(|| (Rect::new(bounds.x + client.x, bounds.y + client.y, client.w, client.h), axes))
            }),
            scroll: self.layout.scroll_range(node).is_some(),
            visual,
            decoration: drawing.decoration.clone(),
            own: drawing.items.clone(),
            content_start: drawing.content_start,
            motion,
            reveal: None,
            reveal_tint: None,
            transition,
            caret,
        }
    }

    fn content(&mut self, node: NodeId, layout: LayoutBox, visual: Visual, mut images: impl Iterator<Item = Sample>, out: &mut Vec<Item>) {
        let rect = layout.rect;
        let content = inset(
            rect,
            [
                layout.border.top + layout.padding.top,
                layout.border.right + layout.padding.right,
                layout.border.bottom + layout.padding.bottom,
                layout.border.left + layout.padding.left,
            ],
        );

        let image_fit = self.tree.image(node).map(|slot| (slot.fit, None)).or_else(|| {
            let slot = self.tree.media_source(node)?;
            matches!(slot.asset, Some(MediaAsset::Pixels(_))).then(|| (slot.fit, slot.natural_size()))
        });
        if let Some((fit, natural_size)) = image_fit {
            let sample = images.next().expect("mounted image sample");
            let (width, height) = natural_size.unwrap_or((sample.frame().width, sample.frame().height));
            let inner = inset(rect, [layout.border.top, layout.border.right, layout.border.bottom, layout.border.left]);
            let fit = fit.place(Size::new(width as f32, height as f32), inner);
            if !fit.is_empty() {
                out.push(Item::Image(ImagePaint {
                    source: node,
                    image: sample,
                    rect: fit,
                    bounds: inner,
                    radius: radius(visual, inner),
                    opacity: 1.0,
                }));
            }
        }

        if let Some(icon) = self.tree.icon(node) {
            paint_icon(node, icon, content, visual.text, out);
        }

        if let Some(native) = self.tree.native(node) {
            if !native.live {
                out.push(Item::Quad(solid(node, rect, self.tree.theme_at(node).get(ColorToken::Surface))));
            }
            if let Some(image) = self.options.native_snapshots.get(&node) {
                out.push(Item::Image(ImagePaint {
                    source: node,
                    image: image.clone(),
                    rect,
                    bounds: rect,
                    radius: radius(visual, rect),
                    opacity: 1.0,
                }));
            } else {
                out.push(Item::Native {
                    source: node,
                    rect,
                    radius: radius(visual, rect),
                    slot: native.slot,
                    cut: native.live && native.slot == NativeSlot::Underlay,
                });
            }
        }
        if let Some(ops) = self.tree.canvas(node) {
            self.canvas(node, ops, content, visual, &mut images, out);
        }
        out.extend(self.text.items(self.tree, node, self.options.window_active));
        if !node_attachments(self.tree, node).is_empty() {
            self.inline_attachments(node, visual, images, out);
        }
    }

    fn inline_attachments(&mut self, node: NodeId, visual: Visual, mut images: impl Iterator<Item = Sample>, out: &mut Vec<Item>) {
        let (clip, mut positions) = self.text.attachments(node);
        if positions.is_empty() {
            return;
        }
        positions.sort_unstable_by_key(|&(index, _)| index);
        let mut positions = positions.into_iter().peekable();
        let clipped = clip.is_some();
        out.extend(clip);
        for (index, attachment) in node_attachments(self.tree, node).iter().enumerate() {
            let mut samples = images.by_ref().take(attachment_images(self.tree, node, index, attachment).count());
            if positions.peek().is_some_and(|position| position.0 == index) {
                let (_, rect) = positions.next().expect("positioned inline object");
                match &attachment.content {
                    InlineContent::Spacer { .. } | InlineContent::Pill(_) => {}
                    InlineContent::Image { fit, radius, .. } | InlineContent::ImageSource { fit, radius, .. } => {
                        let Some(image) = samples.next() else { continue };
                        let size = Size::new(image.frame().width as f32, image.frame().height as f32);
                        let placed = fit.place(size, rect);
                        if !placed.is_empty() {
                            out.push(Item::Image(ImagePaint {
                                source: node,
                                image,
                                rect: placed,
                                bounds: rect,
                                radius: [radius.max(0.0); 4],
                                opacity: 1.0,
                            }));
                        }
                    }
                    InlineContent::Icon { icon, color } => {
                        let color = color.map_or(visual.text, |color| color.resolve(self.tree.theme_at(node)).0);
                        paint_icon(node, icon, rect, color, out);
                    }
                    InlineContent::Vector { ops } => self.canvas(node, ops, rect, visual, &mut samples, out),
                }
            }
            samples.for_each(drop);
        }
        if clipped {
            out.push(Item::PopClip);
        }
    }

    fn canvas(
        &mut self,
        node: NodeId,
        ops: &[CanvasOp],
        content: Rect,
        visual: Visual,
        mut images: impl Iterator<Item = Sample>,
        out: &mut Vec<Item>,
    ) {
        let origin = Transform::translate(content.x, content.y);
        let place = |rect: Rect| Rect::new(rect.x + content.x, rect.y + content.y, rect.w, rect.h);
        let color = |value: octara_model::theme::ColorValue| value.resolve(self.tree.theme_at(node)).0;
        out.push(Item::PushClip { source: node, rect: content, radius: [0.0; 4] });
        for op in ops {
            match op {
                CanvasOp::Path { path, style, fill, transform } => {
                    let (color, gradient) = match *fill {
                        Fill::Solid(value) => (color(value), None),
                        Fill::Linear(value) => (
                            Color::TRANSPARENT,
                            Some(Gradient {
                                angle: value.angle,
                                from: (color(value.from.color), value.from.percentage),
                                to: (color(value.to.color), value.to.percentage),
                            }),
                        ),
                    };
                    let transform = transform.then(&origin);
                    let bounds = path.bounds().map_or(content, |bounds| transform_bounds(transform, bounds));
                    out.push(Item::Path(PathPaint {
                        source: node,
                        path: path.clone(),
                        style: style.clone(),
                        color,
                        gradient,
                        transform,
                        bounds,
                    }));
                }
                CanvasOp::Image { rect, .. } => {
                    let rect = place(*rect);
                    out.push(Item::Image(ImagePaint {
                        source: node,
                        image: images.next().expect("canvas image sample"),
                        rect,
                        bounds: rect,
                        radius: [0.0; 4],
                        opacity: 1.0,
                    }));
                }
                CanvasOp::Text(text) => {
                    let text_color = text.color.map_or(visual.text, color);
                    for mut item in self.text.canvas_items(node, text, visual.text_style(), text_color) {
                        item.translate(content.x, content.y);
                        out.push(item);
                    }
                }
                CanvasOp::PushClip { rect, radius } => out.push(Item::PushClip { source: node, rect: place(*rect), radius: [*radius; 4] }),
                CanvasOp::PopClip => out.push(Item::PopClip),
            }
        }
        out.push(Item::PopClip);
    }
}

fn node_images(tree: &Tree, node: NodeId) -> impl Iterator<Item = &Image> {
    tree.image(node)
        .map(|slot| &slot.image)
        .into_iter()
        .chain(tree.media_source(node).and_then(|slot| match slot.asset.as_ref()? {
            MediaAsset::Pixels(image) => Some(image),
            MediaAsset::BrowserUri(_) => None,
        }))
        .chain(canvas_images(tree.canvas(node).unwrap_or_default()))
        .chain(
            node_attachments(tree, node)
                .iter()
                .enumerate()
                .flat_map(move |(index, attachment)| attachment_images(tree, node, index, attachment)),
        )
}

fn node_attachments(tree: &Tree, node: NodeId) -> &[InlineAttachment] {
    tree.text(node).map_or_else(|| tree.editor(node).map_or(&[][..], |editor| editor.attachments()), |text| &text.attachments)
}

fn canvas_images(ops: &[CanvasOp]) -> impl Iterator<Item = &Image> {
    ops.iter().filter_map(|op| match op {
        CanvasOp::Image { image, .. } => Some(image),
        _ => None,
    })
}

fn attachment_images<'a>(tree: &'a Tree, node: NodeId, index: usize, attachment: &'a InlineAttachment) -> impl Iterator<Item = &'a Image> {
    let (image, ops) = match &attachment.content {
        InlineContent::Image { image, .. } => (Some(image), &[][..]),
        InlineContent::ImageSource { .. } => {
            let image = tree.inline_media_source(node, index).and_then(|slot| match slot.asset.as_ref()? {
                MediaAsset::Pixels(image) => Some(image),
                MediaAsset::BrowserUri(_) => None,
            });
            (image, &[][..])
        }
        InlineContent::Vector { ops } => (None, ops.as_ref()),
        _ => (None, &[][..]),
    };
    image.into_iter().chain(canvas_images(ops))
}

pub(crate) fn paint_icon(node: NodeId, icon: &NodeIcon, rect: Rect, color: Color, out: &mut Vec<Item>) {
    let view = icon.view_box;
    let size = (rect.w / view.w).min(rect.h / view.h);
    if !size.is_finite() || size <= 0.0 {
        return;
    }
    let x = rect.x + (rect.w - view.w * size) * 0.5;
    let y = rect.y + (rect.h - view.h * size) * 0.5;
    let transform = Transform::translate(-view.x, -view.y).then(&Transform::scale(size, size)).then(&Transform::translate(x, y));
    out.extend(icon.shapes.iter().map(|shape| {
        Item::Path(PathPaint {
            source: node,
            path: shape.path.clone(),
            transform,
            style: shape.style.clone(),
            color,
            gradient: None,
            bounds: rect,
        })
    }));
}

fn image_samples(tree: &Tree, node: NodeId) -> ImageSamples {
    node_images(tree, node).map(Image::sample).collect()
}

fn image_versions(tree: &Tree, node: NodeId) -> ImageVersions {
    node_images(tree, node).map(|image| (image.id(), image.frame().version)).collect()
}

fn solid(source: NodeId, rect: Rect, color: Color) -> Quad {
    Quad { source, rect, radius: [0.0; 4], fill: Some(color), gradient: None, border: [0.0; 4], border_color: Color::TRANSPARENT }
}

fn inset(rect: Rect, [top, right, bottom, left]: [f32; 4]) -> Rect {
    Rect::new(rect.x + left, rect.y + top, (rect.w - left - right).max(0.0), (rect.h - top - bottom).max(0.0))
}

pub(crate) fn radius(visual: Visual, rect: Rect) -> [f32; 4] {
    rect.corner_radii(visual.radius)
}

fn transform_bounds(transform: Transform, rect: Rect) -> Rect {
    let points = [
        Point::new(rect.x, rect.y),
        Point::new(rect.right(), rect.y),
        Point::new(rect.x, rect.bottom()),
        Point::new(rect.right(), rect.bottom()),
    ]
    .map(|point| transform.apply(point));
    let x = points.iter().map(|point| point.x).fold(f32::INFINITY, f32::min);
    let y = points.iter().map(|point| point.y).fold(f32::INFINITY, f32::min);
    let right = points.iter().map(|point| point.x).fold(f32::NEG_INFINITY, f32::max);
    let bottom = points.iter().map(|point| point.y).fold(f32::NEG_INFINITY, f32::max);
    Rect::new(x, y, right - x, bottom - y)
}
