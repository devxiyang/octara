//! Incremental, rollback-capable publication of the retained tree into one DOM surface.

mod assets;
mod editor;
mod exit;
mod geometry;
mod projection;
mod reveal;

use crate::dom::{self, DomHandle};
use crate::image::CanvasSize;
use crate::measure::{Geometry, MeasureBuffer, MeasureMode};
use crate::undo::{self, DomUndoLog, InputState};
use octara::runtime::engine::{CommitOutput, HostEngine, ProjectError, ProjectFeedback, ScrollRange};
use octara::tree::{NodeFlags, NodeId, Tree, TreeDelta};
use octara_model::geometry::{LayoutBox, Point, Rect, WindowViewport};
use octara_model::input::ContactTarget;
use octara_model::paint::{NodeIcon, Sample};
use octara_model::semantics::TextClip;
use octara_model::style::State;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, HtmlCanvasElement, HtmlElement};

#[derive(Clone, Default)]
struct EntryState {
    geometry: Option<Geometry>,
    parent: Option<NodeId>,
    text_rev: Option<(u64, u64, u64)>,
    rich_editor_rev: Option<(u64, u64, u64)>,
    rich_selection: Option<(usize, usize)>,
    inline_images: bool,
    image: Option<Sample>,
    media: Option<assets::MediaFrame>,
    inline_media: Option<Box<FxHashMap<usize, assets::MediaFrame>>>,
    icon: Option<(Arc<NodeIcon>, web_sys::Element)>,
    canvas: Option<CanvasFrame>,
    overlay_visible: Option<bool>,
    affordance: Option<projection::Affordance>,
    relates: bool,
    bounds_demand: bool,
    scrollport: bool,
    text_clip: Option<TextClip>,
}

#[derive(Clone)]
struct CanvasFrame {
    canvas: HtmlCanvasElement,
    size: CanvasSize,
    paint_rev: u32,
    images: Vec<(u64, u64)>,
}

#[derive(Default)]
struct Pending {
    undo: DomUndoLog,
    entries: FxHashMap<NodeId, Option<EntryState>>,
    actions: FxHashMap<NodeId, Option<NodeId>>,
    created: FxHashSet<NodeId>,
    retired: FxHashSet<NodeId>,
    exits: FxHashMap<NodeId, exit::ExitPlan>,
    observed: FxHashSet<NodeId>,
    full_measure: bool,
    reveals: Option<(FxHashMap<NodeId, octara_model::motion::TextRevealTrack>, js_sys::Array)>,
    viewport: Option<WindowViewport>,
    modal: Option<NodeId>,
    focus: Option<NodeId>,
    keyboard_focus: Option<NodeId>,
    reduced_motion: Option<bool>,
    inline_pixel_generation: Option<u64>,
    #[cfg(feature = "webgpu")]
    gpu: FxHashMap<NodeId, Option<crate::gpu::Plan>>,
}

struct ParkedEditor {
    node: NodeId,
    element: HtmlElement,
    stand_in: HtmlElement,
    retained_focus: bool,
}

struct FocusSnapshot {
    element: HtmlElement,
    input: Option<InputState>,
    rich_selection: Option<(usize, usize)>,
    composing: Option<NodeId>,
}

pub struct DomEngine {
    document: Document,
    surface: HtmlElement,
    parking: HtmlElement,
    dom: DomHandle,
    root: Option<NodeId>,
    entries: FxHashMap<NodeId, EntryState>,
    demands: FxHashSet<NodeId>,
    scrollports: FxHashSet<NodeId>,
    images: FxHashSet<NodeId>,
    canvas_images: FxHashSet<NodeId>,
    inline_images: FxHashSet<NodeId>,
    inline_pixel_generation: u64,
    relations: FxHashSet<NodeId>,
    clipped: FxHashMap<NodeId, TextClip>,
    measure: MeasureBuffer,
    exits: FxHashMap<NodeId, exit::ExitAnimation>,
    parked: Option<ParkedEditor>,
    fallback_focus: Option<FocusSnapshot>,
    pending: Option<Pending>,
    viewport: Option<WindowViewport>,
    modal: Option<NodeId>,
    keyboard_focus: Option<NodeId>,
    frame: u64,
    reveals: FxHashMap<NodeId, octara_model::motion::TextRevealTrack>,
    reduced_motion: bool,
    published_reduced_motion: bool,
    #[cfg(feature = "webgpu")]
    gpu: std::rc::Rc<crate::gpu::Canvases>,
}

impl DomEngine {
    pub fn new(surface: HtmlElement) -> Result<Self, JsValue> {
        Self::new_for_host(
            surface,
            #[cfg(feature = "webgpu")]
            None,
        )
    }

    #[cfg(feature = "webgpu")]
    pub(crate) fn with_surface_renderer(surface: HtmlElement, factory: octara_render::SurfaceRendererFactory) -> Result<Self, JsValue> {
        Self::new_for_host(surface, Some(factory))
    }

    fn new_for_host(
        surface: HtmlElement,
        #[cfg(feature = "webgpu")] surface_renderer: Option<octara_render::SurfaceRendererFactory>,
    ) -> Result<Self, JsValue> {
        let document = surface.owner_document().ok_or_else(|| JsValue::from_str("Octara's DOM surface requires an owning document"))?;
        let parking = dom::html(&document, "div")?;
        parking.set_attribute("data-octara-editor-parking", "")?;
        parking.set_attribute("style", "position:fixed;top:0;left:-100000px;width:1px;height:1px;overflow:hidden;pointer-events:none;")?;
        surface.append_child(&parking)?;
        Ok(Self {
            document,
            surface,
            parking,
            dom: DomHandle::default(),
            root: None,
            entries: FxHashMap::default(),
            demands: FxHashSet::default(),
            scrollports: FxHashSet::default(),
            images: FxHashSet::default(),
            canvas_images: FxHashSet::default(),
            inline_images: FxHashSet::default(),
            inline_pixel_generation: 0,
            relations: FxHashSet::default(),
            clipped: FxHashMap::default(),
            measure: MeasureBuffer::default(),
            exits: FxHashMap::default(),
            parked: None,
            fallback_focus: None,
            pending: None,
            viewport: None,
            modal: None,
            keyboard_focus: None,
            frame: 0,
            reveals: FxHashMap::default(),
            reduced_motion: false,
            published_reduced_motion: false,
            #[cfg(feature = "webgpu")]
            gpu: crate::gpu::Canvases::new(surface_renderer),
        })
    }

    pub(crate) fn dom(&self) -> DomHandle {
        self.dom.clone()
    }

    pub fn set_measure_mode(&self, mode: MeasureMode) {
        self.dom.set_measure_mode(mode);
    }

    fn projected_element(&self, node: NodeId) -> Option<HtmlElement> {
        self.parked.as_ref().filter(|parked| parked.node == node).map(|parked| parked.stand_in.clone()).or_else(|| self.dom.element(node))
    }

    fn element_bounds(&self, element: &HtmlElement) -> Option<Rect> {
        if !element.is_connected() || element.get_client_rects().length() == 0 {
            return None;
        }
        let origin = self.surface.get_bounding_client_rect();
        let rect = element.get_bounding_client_rect();
        Some(Rect::new((rect.x() - origin.x()) as f32, (rect.y() - origin.y()) as f32, rect.width() as f32, rect.height() as f32))
    }

    fn pending(&mut self) -> &mut Pending {
        self.pending.as_mut().expect("an open DOM projection")
    }

    fn remember(&mut self, node: NodeId) {
        let pending = self.pending.as_mut().expect("an open DOM projection");
        pending.entries.entry(node).or_insert_with(|| self.entries.get(&node).cloned());
    }

    fn register_indexes(&mut self, node: NodeId) {
        let entry = self.entries.get(&node);
        set_member(&mut self.demands, node, entry.is_some_and(|entry| entry.bounds_demand));
        set_member(&mut self.scrollports, node, entry.is_some_and(|entry| entry.scrollport));
        set_member(
            &mut self.images,
            node,
            entry.is_some_and(|entry| {
                entry.image.is_some()
                    || entry.media.as_ref().is_some_and(assets::MediaFrame::is_pixels)
                    || entry.inline_media.as_deref().is_some_and(|inline| inline.values().any(assets::MediaFrame::is_pixels))
            }),
        );
        set_member(
            &mut self.canvas_images,
            node,
            entry.and_then(|entry| entry.canvas.as_ref()).is_some_and(|frame| !frame.images.is_empty()),
        );
        set_member(&mut self.inline_images, node, entry.is_some_and(|entry| entry.inline_images));
        set_member(&mut self.relations, node, entry.is_some_and(|entry| entry.relates));
        if let Some(clip) = entry.and_then(|entry| entry.text_clip) {
            self.clipped.insert(node, clip);
        } else {
            self.clipped.remove(&node);
        }
    }

    fn project_round(&mut self, tree: &Tree, delta: &TreeDelta, viewport: WindowViewport, now: f64) -> Result<ProjectFeedback, JsValue> {
        if self.pending.is_none() {
            self.pending = Some(Pending::default());
        }
        let resized = self.dom.take_resized();
        self.pending().observed.extend(resized.iter().copied());

        let modal = tree.front_modal();
        let old_modal = self.pending.as_ref().and_then(|pending| pending.viewport.map(|_| pending.modal)).unwrap_or(self.modal);
        let last_viewport = self.pending.as_ref().and_then(|pending| pending.viewport).or(self.viewport);
        let previous_keyboard_focus = self
            .pending
            .as_ref()
            .filter(|pending| pending.viewport.is_some())
            .map_or(self.keyboard_focus, |pending| pending.keyboard_focus);
        let keyboard_focus = tree.focus().filter(|&node| tree.node(node).is_some_and(|core| core.state.contains(State::FOCUS_VISIBLE)));
        let viewport_changed = last_viewport != Some(viewport);
        let modal_changed = old_modal != modal;
        self.pending().viewport = Some(viewport);
        self.pending().modal = modal;
        self.pending().focus = tree.focus();
        self.pending().keyboard_focus = keyboard_focus;

        let full_measure = self.dom.take_full_measure();
        self.pending().full_measure |= full_measure;
        let measure_all = full_measure || viewport_changed || self.dom.measure_mode() == MeasureMode::VerifyFullCssom;
        let mut geometry = self.prepare_geometry(tree, delta, &resized, measure_all);
        let mut feedback = ProjectFeedback::default();
        let exits = self.capture_exits(tree, delta, now);
        let (created, moved) = self.patch_structure(tree, delta, exits, &mut feedback)?;
        let mut styles = self.patch_semantics(tree, delta, &created, &moved, modal_changed)?;
        let last_reduced_motion = self.pending().reduced_motion.unwrap_or(self.published_reduced_motion);
        if last_reduced_motion != self.reduced_motion {
            styles.extend(self.entries.keys().copied().filter(|&node| !tree.visual(node).motion.is_none()));
            self.pending().reduced_motion = Some(self.reduced_motion);
        }
        self.patch_styles(tree, delta, viewport, viewport_changed, &mut styles)?;
        self.patch_editors(tree, delta, &created, &styles)?;
        let changed_images = self.patch_images(tree, &styles)?;

        let anchors: Vec<_> = tree.anchored_nodes().collect();
        geometry.include_moved(&moved);
        geometry.include_images(self, tree, changed_images);
        let reveal_focus = keyboard_focus.filter(|&node| Some(node) != previous_keyboard_focus);
        let measured = self.read_geometry(tree, delta, &created, &anchors, reveal_focus, geometry)?;
        self.update_canvases(tree, viewport, &measured, &delta.paint_dirty)?;
        self.report_feedback(tree, viewport, &anchors, &measured, &mut feedback);
        #[cfg(feature = "webgpu")]
        self.stage_gpu(tree, delta, viewport, now, &created, &measured);
        self.report_exits(tree, now, &mut feedback);
        feedback.next_deadline = feedback.next_deadline.into_iter().chain(self.stage_reveals(tree, now)).min();
        Ok(feedback)
    }
}

impl HostEngine for DomEngine {
    fn platform_text_selection(&self) -> bool {
        true
    }

    fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    fn shortcut_modifier(&self) -> octara_model::input::Modifiers {
        self.document.default_view().as_ref().map_or(octara_model::input::Modifiers::CTRL, crate::services::shortcut_modifier)
    }

    fn project(&mut self, tree: &Tree, delta: &TreeDelta, viewport: WindowViewport, now: f64) -> Result<ProjectFeedback, ProjectError> {
        let _guard = self.dom.project_guard();
        self.project_round(tree, delta, viewport, now)
            .map_err(|error| ProjectError(error.as_string().unwrap_or_else(|| format!("DOM projection failed: {error:?}"))))
    }

    fn commit(&mut self) -> CommitOutput {
        let _guard = self.dom.project_guard();
        if self.pending.is_none() {
            return CommitOutput::default();
        }
        if let Err(error) = self.release_editor() {
            web_sys::console::error_1(&error);
        }
        let mut pending = self.pending.take().expect("an open DOM projection");
        pending.undo.clear();
        for node in pending.retired {
            self.exits.remove(&node);
            self.dom.remove(node);
            self.entries.remove(&node);
            self.register_indexes(node);
        }
        for node in pending.created {
            self.dom.observe(node);
        }
        self.fallback_focus = None;
        let active = self.document.active_element();
        if let Some(next) = pending.focus.and_then(|node| self.dom.element(node)) {
            if active.as_ref() != Some(next.as_ref()) && active.as_ref().and_then(dom::native_view_host).as_ref() != Some(next.as_ref()) {
                let _ = undo::focus(&next);
                if let Some(selection) = pending.focus.and_then(|node| self.entries.get(&node)?.rich_selection) {
                    let _ = crate::rich::select(&next, selection);
                }
            }
        } else if let Some(active) = active.and_then(|active| active.dyn_into::<HtmlElement>().ok())
            && self.surface.contains(Some(&active))
            && dom::native_view_host(&active).is_none()
        {
            let _ = active.blur();
        }
        self.publish_exits(pending.exits);
        self.publish_reveals(pending.reveals);
        self.viewport = pending.viewport;
        self.modal = pending.modal;
        self.keyboard_focus = pending.keyboard_focus;
        if let Some(reduced_motion) = pending.reduced_motion {
            self.published_reduced_motion = reduced_motion;
        }
        if let Some(generation) = pending.inline_pixel_generation {
            self.inline_pixel_generation = generation;
        }
        #[cfg(feature = "webgpu")]
        self.gpu.publish(self.frame + 1, pending.gpu);
        self.frame += 1;
        CommitOutput { draw: None, frame: Some(self.frame) }
    }

    fn rollback(&mut self) {
        let Some(mut pending) = self.pending.take() else { return };
        let _guard = self.dom.project_guard();
        pending.undo.rollback();
        self.parked = None;
        if let Some(FocusSnapshot { element, input, rich_selection, composing }) = self.fallback_focus.take() {
            let _ = undo::focus(&element);
            if let Some(input) = input {
                let _ = input.restore(&element);
            }
            if let Some(selection) = rich_selection {
                let _ = crate::rich::select(&element, selection);
            }
            self.dom.set_composing(composing);
        }
        for (node, previous) in pending.actions {
            self.dom.set_action(node, previous);
        }
        for node in pending.created {
            self.dom.remove(node);
        }
        for (node, previous) in pending.entries {
            if let Some(entry) = previous {
                self.entries.insert(node, entry);
            } else {
                self.entries.remove(&node);
            }
            self.register_indexes(node);
        }
        self.dom.resize_nodes(&pending.observed.into_iter().collect::<Vec<_>>());
        if pending.full_measure {
            self.dom.request_full_measure();
        }
    }

    fn hit_test(&self, tree: &Tree, pos: Point) -> Option<ContactTarget> {
        let surface = self.surface.get_bounding_client_rect();
        let element = self.document.element_from_point(pos.x + surface.x() as f32, pos.y + surface.y() as f32)?;
        self.dom.target(tree, &element)
    }

    fn geometry(&self, node: NodeId) -> Option<LayoutBox> {
        let entry = self.entries.get(&node)?;
        Some(entry.geometry?.layout(entry.parent.and_then(|parent| self.sample(parent))))
    }

    fn interaction_bounds(&self, node: NodeId) -> Option<Rect> {
        self.element_bounds(&self.dom.element(node)?)
    }

    fn inline_attachment_bounds(&self, node: NodeId, index: usize) -> Option<Rect> {
        let owner = self.dom.element(node)?;
        self.element_bounds(&crate::rich::media_element(&owner, index)?)
    }

    fn caret_rect(&self, tree: &Tree, node: NodeId) -> Option<Rect> {
        (tree.focus() == Some(node)).then(|| self.text_anchor_rect(tree, node, true)).flatten()
    }

    fn selection_rect(&self, tree: &Tree, node: NodeId) -> Option<Rect> {
        self.text_anchor_rect(tree, node, false)
    }

    fn clipped_text(&self, tree: &Tree) -> Vec<(NodeId, TextClip)> {
        self.clipped.iter().filter(|&(&node, _)| tree.is_live(node)).map(|(&node, &clip)| (node, clip)).collect()
    }

    fn scroll_range(&self, node: NodeId) -> Option<ScrollRange> {
        self.scrollports.contains(&node).then(|| self.sample(node)).flatten().map(|geometry| ScrollRange { max: geometry.scroll_max() })
    }

    fn scroll_anchor(&self, tree: &Tree, node: NodeId) -> Option<NodeId> {
        if !tree.maintains_scroll_anchor(node) {
            return None;
        }
        let port = self.sample(node)?;
        let viewport = Rect::new(port.bounds.x + port.border.left, port.bounds.y + port.border.top, port.client.width, port.client.height);
        tree.children(node).iter().copied().find(|&child| {
            tree.is_live(child)
                && tree.node(child).is_some_and(|core| !core.flags.contains(NodeFlags::HIDDEN))
                && self.sample(child).is_some_and(|sample| !sample.bounds.intersect(&viewport).is_empty())
        })
    }
}

impl Drop for DomEngine {
    fn drop(&mut self) {
        reveal::octara_clear_reveals(&self.surface);
        if let Some(root) = self.root.and_then(|node| self.dom.element(node)) {
            root.remove();
        }
        self.parking.remove();
    }
}

fn subtrees(tree: &Tree, roots: impl IntoIterator<Item = NodeId>) -> FxHashSet<NodeId> {
    let mut seen = FxHashSet::default();
    let mut pending: Vec<_> = roots.into_iter().collect();
    while let Some(node) = pending.pop() {
        if tree.is_live(node) && seen.insert(node) {
            pending.extend(tree.logical_children(node));
        }
    }
    seen
}

fn set_member(set: &mut FxHashSet<NodeId>, node: NodeId, belongs: bool) {
    if belongs {
        set.insert(node);
    } else {
        set.remove(&node);
    }
}
