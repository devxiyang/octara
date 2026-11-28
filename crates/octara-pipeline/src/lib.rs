//! Native projection of the same retained tree used by other Octara hosts.

pub mod a11y;
pub mod paint;
mod projected;

mod airspace;
mod flat_tree;
mod frame_arena;
mod layout;
mod motion;
mod text;

use a11y::{AccessKitProjector, AccessPublication};
use airspace::{Airspace, AirspaceFrame};
use flat_tree::FlatTree;
use layout::LayoutKernel;
use octara_model::geometry::{LayoutBox, Point, Rect, WindowViewport};
use octara_model::identity::{MeasureTicket, NativeId, ViewportRev, WindowRev};
use octara_model::input::{ContactTarget, HostReply};
use octara_model::native::{NativeChange, NativeMeasureRequest, NativeMeasureTicket, NativePlacement, NativeSlot, TypedNativeProps};
use octara_model::paint::{DisplayList, FontData, Item};
use octara_model::semantics::TextClip;
use octara_model::text::TextGeometry;
use octara_model::window::WindowMaterial;
use octara_runtime::engine::{AccessibilityConsumer, ScrollRange};
use octara_runtime::module::{ModuleDispatcher, NativeViewMounts};
use octara_runtime::{CommitOutput, HostEngine, ProjectError, ProjectFeedback};
use octara_tree::arbiter::{ScrollbarHit, ScrollbarKey};
use octara_tree::{NodeId, NodeKind, Tree, TreeDelta};
use paint::{CompileOptions, PaintCompiler, PaintScene};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use text::TextEngine;

pub use a11y::{AccessibilityCommit, AccessibilityContext, AccessibilityWork};
pub use flat_tree::{FlatNode, FlatTreeMetrics};
pub use layout::LayoutWork;
pub use text::TextWork;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeEngineConfig {
    /// Verify incremental geometry and sampled hits against an independent cold layout.
    pub check_layout_cache: bool,
    pub check_paint_cache: bool,
    pub check_a11y_cache: bool,
    pub system_fonts: bool,
    pub reduced_motion: bool,
    pub transparent_window: bool,
    pub supports_window_vibrancy: bool,
}

/// A published native frame. Accessibility, native placements and paint use the same geometry.
pub struct NativeFrame {
    pub number: u64,
    pub sampled_at: std::time::Duration,
    pub draw: Arc<DisplayList>,
    /// Cumulative text work through this publication, including its projection rounds.
    pub text_work: TextWork,
    pub scene: Option<Arc<PaintScene>>,
    /// The incremental accessibility update, if this frame changed the active tree.
    pub accessibility: Option<Arc<accesskit::TreeUpdate>>,
    pub native_changes: Vec<NativeChange>,
    /// The session and revision that produced the caret and text geometry.
    pub editor_stamp: Option<(octara_model::identity::EditorSessionId, u64)>,
    pub editor_caret: Option<Rect>,
    pub editor_geometry: Option<TextGeometry>,
    pub measurements: Vec<NativeMeasureRequest>,
    airspace: AirspaceFrame,
}

impl NativeFrame {
    pub fn contains_native_snapshot(&self, id: NativeId) -> bool {
        self.airspace.images.contains_key(&id.node())
    }
}

/// Allows a platform event loop to consume native-only frame information after Publish.
#[derive(Clone, Default)]
pub struct NativePublication {
    frame: Rc<RefCell<Option<Arc<NativeFrame>>>>,
    accessibility: AccessPublication,
    pending_native_changes: Option<Rc<RefCell<Vec<NativeChange>>>>,
    airspace: Rc<RefCell<Airspace>>,
}

impl NativePublication {
    /// Retain native lifecycle and active accessibility deltas for window delivery.
    pub fn for_window_host() -> Self {
        Self { accessibility: AccessPublication::for_window_host(), pending_native_changes: Some(Rc::default()), ..Self::default() }
    }

    pub fn latest(&self) -> Option<Arc<NativeFrame>> {
        self.frame.borrow().clone()
    }

    pub fn take_native_changes(&self) -> Vec<NativeChange> {
        self.pending_native_changes.as_ref().map_or_else(Vec::new, |changes| std::mem::take(&mut *changes.borrow_mut()))
    }

    /// Request captures for the latest committed geometry after its views have mounted.
    pub fn capture_native_airspace(&self, mounts: &mut NativeViewMounts, dispatcher: &ModuleDispatcher) -> bool {
        self.latest().is_some_and(|frame| self.airspace.borrow_mut().start(&frame.airspace, mounts, dispatcher))
    }

    /// Retain a snapshot once the host can include it in a submitted frame.
    pub fn retain_native_snapshot(&self, id: NativeId) {
        self.airspace.borrow_mut().retain_for_presentation(id);
    }

    /// Release the image after a frame physically restores the native view.
    pub fn did_restore_native_snapshot(&self, id: NativeId) {
        if self.latest().is_none_or(|frame| !frame.contains_native_snapshot(id)) {
            self.forget_native_snapshot(id);
        }
    }

    pub fn forget_native_snapshot(&self, id: NativeId) {
        self.airspace.borrow_mut().remove(id);
    }

    /// Build a complete snapshot of the active AccessKit tree.
    pub fn snapshot_full(&self) -> Option<accesskit::TreeUpdate> {
        self.accessibility.snapshot_full()
    }

    pub fn accessibility_work(&self) -> AccessibilityWork {
        self.accessibility.work()
    }

    /// Drain active platform updates in committed frame order.
    pub fn take_accessibility_updates(&self) -> Vec<(u64, Arc<accesskit::TreeUpdate>)> {
        self.accessibility.take_platform_updates()
    }

    /// Drain the same platform queue with each update's committed scope and scale.
    pub fn take_accessibility_commits(&self) -> Vec<AccessibilityCommit> {
        self.accessibility.take_platform_commits()
    }

    /// Scope and scale of the tree returned by `snapshot_full`.
    pub fn accessibility_context(&self) -> Option<AccessibilityContext> {
        self.accessibility.context()
    }

    /// Resolve an exposed, committed platform node to a guarded runtime target.
    pub fn accessibility_target(&self, id: accesskit::NodeId) -> Option<octara_model::identity::TargetGuard> {
        self.accessibility.target(id)
    }

    /// Check whether the current committed target still exposes a platform action.
    pub fn accessibility_supports_action(&self, target: octara_model::identity::TargetGuard, action: accesskit::Action) -> bool {
        self.accessibility.supports_action(target, action)
    }

    pub fn accessibility_node(&self, id: accesskit::NodeId) -> Option<NodeId> {
        self.accessibility.node(id)
    }

    pub fn accessibility_host_action(&self, tree: &Tree, request: accesskit::ActionRequest) -> Option<HostReply> {
        self.accessibility.host_action(tree, request)
    }
}

struct StagedFrame {
    draw: Arc<DisplayList>,
    scene: Option<Arc<PaintScene>>,
    sampled_at: std::time::Duration,
    scale: f32,
    editor_stamp: Option<(octara_model::identity::EditorSessionId, u64)>,
    editor_caret: Option<Rect>,
    editor_geometry: Option<TextGeometry>,
    airspace: AirspaceFrame,
}

pub struct NativeEngine {
    config: NativeEngineConfig,
    window_active: bool,
    vibrant_window: bool,
    layout: LayoutKernel,
    flat: FlatTree,
    text: TextEngine,
    paint: PaintCompiler,
    scrolling: motion::ScrollMotion,
    accessibility: AccessKitProjector,
    native: NativeViews,
    frame: u64,
    committed_scale: f32,
    publication: NativePublication,
    staged: Option<StagedFrame>,
}

impl NativeEngine {
    pub fn new(config: NativeEngineConfig) -> Self {
        Self::with_publication(config, NativePublication::default())
    }

    pub fn with_publication(config: NativeEngineConfig, publication: NativePublication) -> Self {
        Self {
            config,
            window_active: true,
            vibrant_window: false,
            layout: LayoutKernel::default(),
            flat: FlatTree::default(),
            text: TextEngine::new(config.system_fonts),
            paint: PaintCompiler::default(),
            scrolling: motion::ScrollMotion::default(),
            accessibility: AccessKitProjector::new(publication.accessibility.clone()),
            native: NativeViews::default(),
            frame: 0,
            committed_scale: 1.0,
            publication,
            staged: None,
        }
    }

    fn check_layout(&self, tree: &Tree, cold: &LayoutKernel, cold_text: &TextEngine, projected: Option<(&Arc<DisplayList>, f32)>) {
        let mut cold_flat = FlatTree::default();
        cold_flat.project(tree, &TreeDelta::default(), cold);
        cold_flat.project_draw(projected);
        let incremental_flat = self.flat.fork_for_layout_check();
        for (node, _) in tree.nodes().filter(|(node, _)| tree.is_live(*node)) {
            assert_eq!(self.layout.geometry(node), cold.geometry(node), "layout box for {node:?}");
            assert_eq!(self.layout.bounds(node), cold.bounds(node), "world bounds for {node:?}");
            assert_eq!(self.layout.content_bounds(node), cold.content_bounds(node), "content bounds for {node:?}");
            assert_eq!(self.layout.clip(node), cold.clip(node), "clip for {node:?}");
            assert_eq!(self.layout.scroll_range(node), cold.scroll_range(node), "scroll range for {node:?}");
            assert_eq!(self.layout.scroll_offset(node), cold.scroll_offset(node), "scroll offset for {node:?}");
            if let (Some(bounds), Some(clip)) = (cold.bounds(node), cold.clip(node)) {
                let visible = bounds.intersect(&clip);
                if !visible.is_empty() {
                    let point = Point::new(visible.x + visible.w * 0.5, visible.y + visible.h * 0.5);
                    assert_eq!(
                        incremental_flat.hit(tree, &self.layout, &self.text, point),
                        cold_flat.hit(tree, cold, cold_text, point),
                        "flat hit for {node:?} at {point:?}"
                    );
                }
            }
        }
    }

    pub fn publication(&self) -> NativePublication {
        self.publication.clone()
    }

    /// Change the motion policy used by the next projection.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.config.reduced_motion = reduced_motion;
    }

    pub fn drawings_compiled(&self) -> u64 {
        self.paint.built()
    }

    pub fn layout_work(&self) -> LayoutWork {
        self.layout.work()
    }

    pub fn flat_tree_metrics(&self) -> FlatTreeMetrics {
        self.flat.metrics()
    }

    pub fn text_work(&self) -> TextWork {
        self.text.work()
    }

    pub fn paint_metrics(&self) -> paint::PaintMetrics {
        self.paint.metrics()
    }

    /// Register a font and its supported script tags before mounting application windows.
    pub fn register_font(&mut self, data: Arc<[u8]>, scripts: &[[u8; 4]]) {
        self.register_font_data(data.into(), scripts);
    }

    /// Register embedded or externally owned bytes without copying the font into the heap.
    pub fn register_font_data(&mut self, data: FontData, scripts: &[[u8; 4]]) {
        self.text.register_font(data, scripts);
    }

    /// Map an immutable font asset from disk and register its script fallbacks.
    ///
    /// # Safety
    ///
    /// The file must stay unchanged and untruncated while the engine or any of
    /// its published frames can still reference it. Replacing the path
    /// atomically leaves the original file backing the mapping intact.
    #[cfg(any(unix, windows))]
    pub unsafe fn register_font_file(&mut self, path: &std::path::Path, scripts: &[[u8; 4]]) -> std::io::Result<()> {
        // SAFETY: The caller guarantees the lifetime and immutability of the file.
        let data = unsafe { text::map_font(path)? };
        if !self.text.register_font(data, scripts) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "No usable font faces in asset"));
        }
        Ok(())
    }

    pub fn text_hit(&self, node: NodeId, pos: Point) -> Option<usize> {
        text::geometry::hit_byte(&self.text.geometry(node, &self.layout)?, self.flat.source_point(&self.layout, node, pos)?)
    }

    pub fn text_vertical(&self, node: NodeId, byte: usize, direction: i32, column: Option<f32>) -> Option<(usize, f32)> {
        text::geometry::vertical_byte(&self.text.geometry(node, &self.layout)?, byte, direction, column)
    }
}

impl HostEngine for NativeEngine {
    fn set_accessibility_active(&self, tree: &Tree, consumer: AccessibilityConsumer, active: bool) {
        let publication = &self.publication.accessibility;
        publication.set_active(consumer, active);
        if active && self.frame != 0 && self.staged.is_none() && !publication.has_tree() {
            self.accessibility.activate(tree, &self.layout, &self.text, self.committed_scale, self.config.check_a11y_cache);
        }
    }

    fn set_window_active(&mut self, active: bool) {
        self.window_active = active;
    }

    fn set_reduced_motion(&mut self, reduced_motion: bool) {
        NativeEngine::set_reduced_motion(self, reduced_motion);
    }

    fn set_window_material(&mut self, material: WindowMaterial) -> bool {
        let vibrant = self.config.supports_window_vibrancy && matches!(material, WindowMaterial::Vibrancy(_));
        std::mem::replace(&mut self.vibrant_window, vibrant) != vibrant && !self.config.transparent_window
    }

    fn project(&mut self, tree: &Tree, delta: &TreeDelta, viewport: WindowViewport, now: f64) -> Result<ProjectFeedback, ProjectError> {
        if !now.is_finite() || now < 0.0 {
            return Err(timestamp_error());
        }
        if !viewport.scale.is_finite()
            || viewport.scale <= 0.0
            || !viewport.size.width.is_finite()
            || !viewport.size.height.is_finite()
            || viewport.size.width < 0.0
            || viewport.size.height < 0.0
        {
            return Err(viewport_error());
        }
        self.text.begin_stage();
        self.text.retire(delta);
        self.layout.set_animated_scrolls(self.scrolling.active_nodes(tree));
        let layout_viewport = WindowViewport { size: viewport.unobscured_size(), keyboard_inset: 0.0, ..viewport };
        let oracle = self.config.check_layout_cache.then(|| {
            let mut cold = LayoutKernel::default();
            self.layout.copy_measure_inputs_to(tree, &mut cold);
            (cold, self.text.fork_for_layout_check())
        });
        let mut updates = self.layout.project(tree, delta, layout_viewport, &mut self.text);
        self.flat.project(tree, delta, &self.layout);
        let scroll_deadline = self.scrolling.step(tree, &self.layout, &mut updates, now, self.config.reduced_motion);
        self.accessibility.project(tree, delta, &self.layout, &self.text, viewport.scale, self.config.check_a11y_cache);
        let airspace = self.publication.airspace.borrow().project(tree, &self.layout);
        let frame = self.paint.compile(
            tree,
            delta,
            &self.layout,
            &mut self.text,
            CompileOptions {
                viewport,
                now,
                reduced_motion: self.config.reduced_motion,
                verify: self.config.check_paint_cache,
                transparent_window: self.config.transparent_window || self.vibrant_window,
                window_active: self.window_active,
                native_snapshots: &airspace.images,
            },
        );
        let projected = frame.has_projected_layers.then_some((&frame.draw, viewport.scale));
        self.flat.project_draw(projected);
        if let Some((mut cold, mut text)) = oracle {
            cold.project(tree, delta, layout_viewport, &mut text);
            self.check_layout(tree, &cold, &text, projected);
        }
        let editor = tree.focus().filter(|&node| tree.editor(node).is_some());
        self.native.project(tree, &self.layout, &frame.draw, viewport);
        self.staged = Some(StagedFrame {
            draw: frame.draw,
            scene: frame.scene,
            sampled_at: std::time::Duration::from_secs_f64(now),
            scale: viewport.scale,
            editor_stamp: octara_tree::editor::EditorSession::active(tree).map(|session| (session.id(), session.editor().document_rev())),
            editor_caret: editor.filter(|_| self.window_active).and_then(|node| self.text.caret(node, &self.layout)),
            editor_geometry: editor.and_then(|node| self.text.geometry(node, &self.layout)),
            airspace,
        });
        Ok(ProjectFeedback { updates, replies: frame.replies, next_deadline: frame.next_deadline.into_iter().chain(scroll_deadline).min() })
    }

    fn commit(&mut self) -> CommitOutput {
        let staged = self.staged.take().expect("native publication requires a projected frame");
        self.frame = self.frame.checked_add(1).expect("native frame identity exhausted");
        self.committed_scale = staged.scale;
        self.layout.commit_stage();
        self.flat.commit();
        self.text.commit_stage();
        self.paint.commit();
        self.scrolling.commit_stage();
        let accessibility = self.accessibility.commit(self.frame);
        let (native_changes, measurements) = self.native.commit();
        self.publication.airspace.borrow_mut().commit(&staged.airspace);
        if let Some(pending) = &self.publication.pending_native_changes {
            pending.borrow_mut().extend(native_changes.iter().cloned());
        }
        let frame = Arc::new(NativeFrame {
            number: self.frame,
            sampled_at: staged.sampled_at,
            draw: staged.draw.clone(),
            text_work: self.text_work(),
            scene: staged.scene,
            accessibility,
            native_changes,
            measurements,
            editor_stamp: staged.editor_stamp,
            editor_caret: staged.editor_caret,
            editor_geometry: staged.editor_geometry,
            airspace: staged.airspace,
        });
        *self.publication.frame.borrow_mut() = Some(frame);
        CommitOutput { draw: Some(staged.draw), frame: Some(self.frame) }
    }

    fn rollback(&mut self) {
        self.layout.rollback_stage();
        self.flat.rollback();
        self.text.rollback_stage();
        self.paint.rollback();
        self.scrolling.rollback_stage();
        self.accessibility.reset_staging();
        self.native.rollback();
        self.staged = None;
    }

    fn hit_test(&self, tree: &Tree, pos: Point) -> Option<ContactTarget> {
        let hit = self.flat.hit(tree, &self.layout, &self.text, pos)?;
        let action = std::iter::successors(Some(hit), |&node| tree.logical_parent(node)).find(|&node| {
            let kind = tree.node(node).map(|core| core.kind);
            matches!(kind, Some(NodeKind::TextInput | NodeKind::Link)) || tree.has_activation_handler(node) || tree.is_focusable(node)
        });
        Some(ContactTarget { hit: tree.guard(hit), action: action.map(|node| tree.guard(node)) })
    }

    fn geometry(&self, node: NodeId) -> Option<LayoutBox> {
        self.layout.geometry(node)
    }

    fn interaction_bounds(&self, node: NodeId) -> Option<Rect> {
        self.layout.bounds(node)
    }

    fn interaction_local_point(&self, node: NodeId, pos: Point) -> Option<Point> {
        let bounds = self.layout.bounds(node)?;
        let pos = self.flat.source_point(&self.layout, node, pos)?;
        Some(Point::new(pos.x - bounds.x, pos.y - bounds.y))
    }

    fn inline_attachment_bounds(&self, node: NodeId, index: usize) -> Option<Rect> {
        self.text.attachment_bounds(node, index, &self.layout)
    }

    fn caret_rect(&self, tree: &Tree, node: NodeId) -> Option<Rect> {
        (tree.focus() == Some(node)).then(|| self.text.caret(node, &self.layout)).flatten()
    }

    fn selection_rect(&self, tree: &Tree, node: NodeId) -> Option<Rect> {
        self.text.selection_rect(tree, &self.layout, node)
    }

    fn text_geometry(&self, _: &Tree, node: NodeId) -> Option<TextGeometry> {
        self.text.geometry(node, &self.layout)
    }

    fn text_pointer_hit(&self, node: NodeId, pos: Point) -> Option<(usize, usize)> {
        text::geometry::hit_text(&self.text.geometry(node, &self.layout)?, self.flat.source_point(&self.layout, node, pos)?)
    }

    fn text_vertical(&self, node: NodeId, byte: usize, direction: i32, column: Option<f32>) -> Option<(usize, f32)> {
        NativeEngine::text_vertical(self, node, byte, direction, column)
    }

    fn clipped_text(&self, _: &Tree) -> Vec<(NodeId, TextClip)> {
        self.text.clipped()
    }

    fn scroll_range(&self, node: NodeId) -> Option<ScrollRange> {
        self.layout.scroll_range(node)
    }

    fn scroll_anchor(&self, _: &Tree, node: NodeId) -> Option<NodeId> {
        self.layout.scroll_anchor(node)
    }

    fn scrollbar_hit(&self, tree: &Tree, position: Point) -> Option<ScrollbarHit> {
        let hit = self.flat.hit(tree, &self.layout, &self.text, position)?;
        // A parent's scrollbar paints over its descendants.
        std::iter::successors(Some(hit), |&node| tree.physical_parent(node))
            .filter_map(|node| {
                self.layout.scroll_range(node)?;
                let position = self.flat.scrollbar_point(tree, &self.layout, node, position)?;
                self.paint.scrollbar_hit(tree, &self.layout, node, position)
            })
            .last()
    }

    fn update_scrollbars(&mut self, hovered: Option<ScrollbarKey>, dragged: Option<ScrollbarKey>) -> bool {
        self.paint.update_scrollbars(hovered, dragged)
    }

    fn native_reply(&mut self, reply: &HostReply) {
        if let HostReply::NativeSnapshot { request, snapshot } = reply {
            self.publication.airspace.borrow_mut().resolve(*request, snapshot);
        }
        if let HostReply::NativeMeasurement { ticket, size } = reply
            && self.native.accept_measurement(*ticket)
        {
            self.layout.set_native_measure(ticket.node, *size);
        }
    }
}

#[cold]
#[inline(never)]
fn timestamp_error() -> ProjectError {
    ProjectError("native projection requires a nonnegative monotonic timestamp".into())
}

#[cold]
#[inline(never)]
fn viewport_error() -> ProjectError {
    ProjectError("native viewport requires nonnegative finite size and positive finite scale".into())
}

fn paint_order(tree: &Tree, node: NodeId) -> std::borrow::Cow<'_, [NodeId]> {
    let children = tree.children(node);
    let order = |&child: &NodeId| tree.stacking_key(child);
    if children.is_sorted_by_key(order) {
        std::borrow::Cow::Borrowed(children)
    } else {
        let mut children = children.to_vec();
        children.sort_by_key(order);
        std::borrow::Cow::Owned(children)
    }
}

#[derive(Clone)]
struct NativeView {
    revision: u64,
    kind: &'static str,
    slot: NativeSlot,
    props: serde_json::Value,
    typed_props: Option<TypedNativeProps>,
    placement: NativePlacement,
    viewport: WindowViewport,
}

#[derive(Default)]
struct NativeViews {
    committed: FxHashMap<NodeId, NativeView>,
    staged: FxHashMap<NodeId, NativeView>,
    pending_measurements: FxHashMap<NodeId, NativeMeasureTicket>,
    staged_measurements: Vec<NativeMeasureRequest>,
    viewport: Option<WindowViewport>,
    viewport_rev: ViewportRev,
    request: u64,
}

impl NativeViews {
    fn project(&mut self, tree: &Tree, layout: &LayoutKernel, draw: &DisplayList, viewport: WindowViewport) {
        if self.viewport != Some(viewport) {
            if self.viewport.is_some() {
                self.viewport_rev.0 = self.viewport_rev.0.checked_add(1).expect("native viewport identity exhausted");
            }
            self.viewport = Some(viewport);
        }
        self.staged.clear();
        self.staged_measurements.clear();
        // Only underlays need paint coverage. Overlay-only windows keep the
        // display list's shared spans through native placement.
        let underlays = tree.native_nodes().any(|node| tree.native(node).is_some_and(|native| native.slot == NativeSlot::Underlay));
        let items = if underlays { draw.items() } else { Vec::new() };
        for node in tree.native_nodes() {
            let native = tree.native(node).expect("indexed native content");
            let rect = layout.bounds(node).unwrap_or_default();
            let clip = layout.clip(node).unwrap_or_else(|| viewport.visible_bounds());
            let visual = tree.visual(node);
            self.staged.insert(
                node,
                NativeView {
                    revision: native.props_rev,
                    kind: native.kind,
                    slot: native.slot,
                    props: native.props.clone(),
                    typed_props: native.typed_props.clone(),
                    viewport,
                    placement: NativePlacement {
                        id: NativeId::from(node),
                        slot: native.slot,
                        rect,
                        radius: paint::radius(visual, rect),
                        clip,
                        clip_radius: 0.0,
                        opacity: visual.opacity,
                        visible: tree.is_shown(node) && !rect.intersect(&clip).is_empty() && visual.opacity > 0.0,
                        covered: if native.slot == NativeSlot::Underlay {
                            covered(node, rect.intersect(&clip), &items)
                        } else {
                            Vec::new()
                        },
                    },
                },
            );

            if self.committed.get(&node).is_none_or(|previous| previous.revision != native.props_rev || previous.viewport != viewport) {
                self.request = self.request.checked_add(1).expect("native measure request identity exhausted");
                self.staged_measurements.push(NativeMeasureRequest {
                    ticket: NativeMeasureTicket {
                        measure: MeasureTicket {
                            tree: tree.id(),
                            window_rev: WindowRev { tree: tree.revision(), viewport: self.viewport_rev },
                            request: self.request,
                        },
                        node,
                        props_rev: native.props_rev,
                    },
                    kind: native.kind.to_owned(),
                    slot: native.slot,
                    props: native.props.clone(),
                    typed_props: native.typed_props.clone(),
                    viewport,
                });
            }
        }
        self.staged_measurements.sort_by_key(|request| request.ticket.node);
    }

    fn commit(&mut self) -> (Vec<NativeChange>, Vec<NativeMeasureRequest>) {
        let mut changes = Vec::new();
        let mut retired: Vec<_> = self.committed.keys().filter(|&node| !self.staged.contains_key(node)).copied().collect();
        retired.sort_unstable();
        for node in retired {
            self.pending_measurements.remove(&node);
            changes.push(NativeChange::Destroy { id: NativeId::from(node) });
        }
        let mut nodes: Vec<_> = self.staged.keys().copied().collect();
        nodes.sort_unstable();
        let mut placements = Vec::new();
        for node in nodes {
            let current = &self.staged[&node];
            let id = current.placement.id;
            match self.committed.get(&node) {
                None => changes.push(current.create()),
                Some(previous) if previous.typed_props.is_none() && current.typed_props.is_some() => {
                    changes.push(NativeChange::Destroy { id });
                    changes.push(current.create());
                }
                Some(previous) if previous.revision != current.revision => {
                    changes.push(match &current.typed_props {
                        Some(props) => NativeChange::UpdateTyped { id, props: props.clone() },
                        None => NativeChange::Update { id, props: current.props.clone() },
                    });
                }
                _ => {}
            }
            if self.committed.get(&node).is_none_or(|previous| {
                previous.placement != current.placement || previous.typed_props.is_none() && current.typed_props.is_some()
            }) {
                placements.push(current.placement.clone());
            }
        }
        if !placements.is_empty() {
            changes.push(NativeChange::Place { placements });
        }
        let measurements = std::mem::take(&mut self.staged_measurements);
        self.pending_measurements.extend(measurements.iter().map(|request| (request.ticket.node, request.ticket)));
        self.committed = std::mem::take(&mut self.staged);
        (changes, measurements)
    }

    fn accept_measurement(&mut self, ticket: NativeMeasureTicket) -> bool {
        if self.pending_measurements.get(&ticket.node) != Some(&ticket) {
            return false;
        }
        self.pending_measurements.remove(&ticket.node);
        true
    }

    fn rollback(&mut self) {
        self.staged.clear();
        self.staged_measurements.clear();
    }
}

impl NativeView {
    fn create(&self) -> NativeChange {
        let id = self.placement.id;
        let kind = self.kind.into();
        let slot = self.slot;
        match &self.typed_props {
            Some(props) => NativeChange::CreateTyped { id, kind, slot, props: props.clone() },
            None => NativeChange::Create { id, kind, slot, props: self.props.clone() },
        }
    }
}

fn covered(node: NodeId, rect: Rect, items: &[Item]) -> Vec<Rect> {
    items
        .iter()
        .skip_while(|item| !matches!(item, Item::Native { source, .. } if *source == node))
        .skip(1)
        .filter_map(|item| {
            let own = match item {
                Item::Quad(quad) => quad.rect,
                Item::Custom(paint) => paint.fallback.rect,
                Item::Image(image) => image.rect.intersect(&image.bounds),
                Item::Path(path) => path.bounds,
                _ => return None,
            };
            let overlap = own.intersect(&rect);
            (!overlap.is_empty()).then_some(overlap)
        })
        .collect()
}
