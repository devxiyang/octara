//! Per-window tree, staging queues and callback lifetimes.

use crate::app::AppCommands;
use crate::arena::{ReactiveId, ReactiveWorld, ScopeId};
use crate::authoring::WindowAuthoring;
use crate::cx::{BuildCx, ReadCx, WriteCx};
use crate::cycle::TextInteraction;
use crate::cycle::scroll::ScrollMotion;
use crate::engine::{AccessibilityConsumer, CommitOutput, HostEngine};
use crate::graph::value::{ReadSignal, Signal};
use crate::task::TaskSpawner;
use crate::tooltip::{HoverTooltip, HoveredTooltip};
use octara_model::geometry::{LayoutBox, Point, Size, Vec2, WindowViewport};
pub use octara_model::identity::WindowId;
use octara_model::identity::{TreeId, ViewportRev, WindowRev};
use octara_model::input::{ContactTarget, DroppedFile, GestureEvent, KeyEvent, Modifiers};
use octara_model::native::{NativeEvent, TypedNativeEvent};
use octara_model::semantics::{A11yAction, TextClip};
use octara_model::theme::Breakpoint;
pub use octara_model::window::{
    SystemPreferences, TitlebarStyle, UserAttention, VibrancyMaterial, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowMaterial,
    WindowPresentation,
};
use octara_tree::arbiter::{InteractionArbiter, InteractionPayload, ScrollbarKey};
use octara_tree::delta::RetiredNode;
use octara_tree::editor::{EditorProposal, EditorRichText, EditorState};
use octara_tree::selection::DocSelection;
use octara_tree::tables::{CommandSlot, HandlerId, HandlerKind};
use octara_tree::transaction::{PendingNode, RejectedTransaction, RetirementCache};
use octara_tree::{NodeChange, NodeId, NodeRef, NodeSpec, Retirement, RuntimeOp, TargetGuard, Tree, TreeDelta, TreeReceipt, TxBuilder};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

pub type WindowRoot = Box<dyn FnOnce(&mut MountCx<'_>)>;
pub type EngineFactory = Box<dyn Fn(WindowId, &WindowOptions) -> Box<dyn HostEngine>>;

#[derive(Clone)]
pub struct WindowOptions {
    pub presentation: WindowPresentation,
    pub viewport: WindowViewport,
    pub touch: bool,
    pub position: Option<Point>,
    pub min_size: Option<Size>,
    pub max_size: Option<Size>,
    pub resizable: bool,
    pub maximized: bool,
    pub titlebar: TitlebarStyle,
    pub persistence_key: Option<String>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            presentation: WindowPresentation::default(),
            viewport: WindowViewport { size: Size::new(800.0, 600.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
            touch: false,
            position: None,
            min_size: None,
            max_size: None,
            resizable: true,
            maximized: false,
            titlebar: TitlebarStyle::default(),
            persistence_key: None,
        }
    }
}

impl WindowOptions {
    pub(crate) fn initial_geometry(&self) -> WindowGeometry {
        WindowGeometry {
            position: self.position,
            size: self.viewport.size,
            scale: self.viewport.scale,
            maximized: self.maximized,
            fullscreen: false,
        }
    }
}

pub enum HandlerEvent<'a> {
    Interaction { target: TargetGuard, kind: HandlerKind, payload: InteractionPayload, local_pos: Option<Point> },
    Gesture(&'a GestureEvent),
    FileDrop(&'a [DroppedFile]),
    Paste(&'a octara_model::input::PasteEvent),
    EditorInput { node: NodeId, value: &'a str },
    Key(KeyEvent),
    Command(&'a str),
    Focus { node: NodeId, focused: bool },
    Hover(bool),
    Link(&'a octara_model::input::LinkEvent),
    ContextMenu(&'a octara_model::input::ContextMenuEvent),
    Native(&'a NativeEvent),
    TypedNative(&'a TypedNativeEvent),
    Accessibility(&'a A11yAction),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandlerOutcome {
    pub consumed: bool,
    pub scroll: Vec2,
}

pub(crate) type Handler = Box<dyn Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) -> HandlerOutcome>;
pub(crate) type Reveal = Box<dyn Fn(&mut WriteCx<'_>, &str)>;
pub(crate) type CopyRows = Box<dyn Fn(&mut ReadCx<'_>, NodeId, NodeId) -> Option<Result<Vec<CopyRow>, ()>>>;

/// Content to copy from a virtual row between the selection endpoints.
pub enum CopyRow {
    Mounted(NodeId),
    Unmounted(String),
}

pub(crate) struct ScopedCopyRows {
    pub node: NodeRef,
    pub scope: ScopeId,
    pub call: CopyRows,
}

/// Selection metadata from one retained text node.
pub struct CopiedText {
    /// Outer-to-inner row keys between the selectable region and the text.
    pub rows: Vec<String>,
    /// The entire text node is selected.
    pub whole: bool,
    /// Every selectable text with content in the outermost keyed row is fully selected.
    pub whole_row: bool,
}

type FormatSelection = Box<dyn Fn(&mut ReadCx<'_>, &[CopiedText]) -> Option<String>>;

pub(crate) struct ScopedCopyFormat {
    node: NodeRef,
    scope: ScopeId,
    kind: Option<octara_model::input::CopyFormat>,
    format: FormatSelection,
}

pub(crate) type Binding = Box<dyn FnMut(&mut ReadCx<'_>) -> Vec<NodeChange>>;
type BeforeCommit = Box<dyn FnOnce(&mut MountCx<'_>)>;
type AfterCommit = Box<dyn FnOnce(&TreeReceipt)>;

pub(crate) trait Structure {
    fn check(&mut self, read: &mut ReadCx<'_>) -> bool;
    fn build(&mut self, mount: &mut MountCx<'_>);
}

struct Region<S, R, F> {
    previous: Option<S>,
    read: R,
    build: F,
}

impl<S: PartialEq, R: FnMut(&mut ReadCx<'_>) -> S, F: FnMut(&mut MountCx<'_>, &S)> Structure for Region<S, R, F> {
    fn check(&mut self, read: &mut ReadCx<'_>) -> bool {
        let next = (self.read)(read);
        if self.previous.as_ref() == Some(&next) {
            return false;
        }
        self.previous = Some(next);
        true
    }

    fn build(&mut self, mount: &mut MountCx<'_>) {
        (self.build)(mount, self.previous.as_ref().expect("region is read before build"));
    }
}

pub(crate) enum FeedbackSignal {
    Media(Signal<octara_model::image::ImageState>),
    Geometry(Signal<Option<LayoutBox>>),
    Composition(Signal<bool>),
    Editor(Signal<Option<EditorState>>),
    Scroll(Signal<Vec2>),
    Overscroll(Signal<f32>),
    ScrollAnchor(Signal<Option<NodeId>>),
    Pinned(Signal<bool>),
}

pub(crate) struct ScopedHandler {
    pub node: NodeRef,
    pub kind: HandlerKind,
    pub scope: ScopeId,
    pub call: Handler,
}

pub(crate) struct ScopedBinding {
    pub node: NodeRef,
    pub call: Binding,
}

pub(crate) struct ScopedFeedback {
    pub node: NodeRef,
    pub scope: ScopeId,
    pub signal: FeedbackSignal,
}

pub(crate) struct ScopedReveal {
    pub node: NodeRef,
    pub scope: ScopeId,
    pub call: Reveal,
}

#[derive(Default)]
pub(crate) struct MountWork {
    pub bindings: Vec<(ScopeId, ScopedBinding)>,
    pub structures: Vec<(ScopeId, Box<dyn Structure>)>,
    pub before: Vec<(ScopeId, BeforeCommit)>,
    pub after: Vec<AfterCommit>,
    pub retire_scopes: Vec<ScopeId>,
    pub created: Vec<(ScopeId, PendingNode)>,
    pub retire_nodes: FxHashSet<NodeRef>,
    retirement_cache: RefCell<RetirementCache>,
    pub parents: FxHashMap<NodeRef, NodeRef>,
    pub portal_owners: FxHashMap<NodeRef, Option<NodeRef>>,
    pub places: Vec<(NodeRef, usize, NodeRef)>,
    pub place_by_child: FxHashMap<NodeRef, usize>,
    pub commands: FxHashMap<NodeRef, CommandSlot>,
}

pub struct MountCx<'a> {
    pub(crate) world: &'a mut ReactiveWorld,
    pub(crate) scope: ScopeId,
    pub(crate) window: &'a mut WindowRuntime,
    pub(crate) tasks: &'a TaskSpawner,
    pub(crate) system_preferences: ReadSignal<SystemPreferences>,
    pub(crate) current_structure: Option<ReactiveId>,
}

impl MountCx<'_> {
    pub fn scope(&self) -> ScopeId {
        self.scope
    }
    pub fn tree_id(&self) -> TreeId {
        self.window.tree.id()
    }
    pub fn root(&self) -> NodeRef {
        self.window.tree.node_ref(self.window.tree.root())
    }
    pub fn overlay_root(&self) -> NodeRef {
        self.window.tree.node_ref(self.window.tree.overlay_root())
    }

    /// Resolve a mounted node that will survive the current transaction.
    pub fn live_node(&self, id: NodeId) -> Option<NodeRef> {
        let node = self.window.tree.node_ref(id);
        (self.window.tree.is_live(id) && !self.window.is_retiring(node)).then_some(node)
    }

    pub fn build<R>(&mut self, f: impl FnOnce(&mut BuildCx<'_>) -> R) -> R {
        let mut cx = self
            .world
            .build(self.scope, Some(self.window.id))
            .with_tasks(self.tasks.clone())
            .with_viewport(self.window.viewport_source.read_only(), self.window.breakpoint)
            .with_window_metrics(self.window.chrome_source.read_only(), self.window.geometry_source.read_only())
            .with_hovered_tooltip(self.window.hovered_tooltip.read_only())
            .with_system_preferences(self.system_preferences)
            .with_authoring(&mut self.window.authoring);
        f(&mut cx)
    }

    pub fn with_scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> (ScopeId, R) {
        let child = self.world.new_scope(self.scope);
        self.in_scope(child, f)
    }

    pub fn with_named_scope<R>(&mut self, key: &str, f: impl FnOnce(&mut Self) -> R) -> (ScopeId, R) {
        let child = self.world.new_keyed_scope(self.scope, key);
        self.in_scope(child, f)
    }

    fn in_scope<R>(&mut self, child: ScopeId, f: impl FnOnce(&mut Self) -> R) -> (ScopeId, R) {
        let outer = std::mem::replace(&mut self.scope, child);
        let result = f(self);
        self.scope = outer;
        (child, result)
    }

    pub fn retire_scope(&mut self, scope: ScopeId) {
        self.window.work.retire_scopes.push(scope);
    }

    pub fn create(&mut self, spec: NodeSpec) -> PendingNode {
        let node = self.window.tx.create(spec);
        self.window.work.created.push((self.scope, node));
        node
    }

    pub fn place(&mut self, parent: impl Into<NodeRef>, index: usize, child: impl Into<NodeRef>) {
        let (parent, child) = (parent.into(), child.into());
        let work = &mut self.window.work;
        if work.parents.insert(child, parent) != Some(parent) {
            work.retirement_cache.get_mut().changed_ancestry(child);
        }
        if let Some(&position) = work.place_by_child.get(&child) {
            work.places[position] = (parent, index, child);
        } else {
            work.place_by_child.insert(child, work.places.len());
            work.places.push((parent, index, child));
        }
    }

    pub fn change(&mut self, node: impl Into<NodeRef>, change: NodeChange) {
        self.window.stage_change(node.into(), change);
    }

    pub fn retire(&mut self, node: impl Into<NodeRef>, mode: Retirement) {
        let node = node.into();
        let work = &mut self.window.work;
        let inserted = work.retire_nodes.insert(node);
        if inserted {
            work.retirement_cache.get_mut().clear();
        }
        if inserted || mode == Retirement::Destroy {
            self.window.tx.retire(node, mode);
        }
    }

    pub fn cancel_exit(&mut self, node: NodeRef) {
        let exiting = matches!(node, NodeRef::Existing { tree, id }
            if tree == self.window.tree.id() && self.window.tree.node(id).is_some_and(|core| core.flags.contains(octara_tree::NodeFlags::EXITING)));
        if exiting || self.window.work.retire_nodes.contains(&node) {
            self.retire(node, Retirement::Destroy);
        }
    }

    pub fn bind(&mut self, node: impl Into<NodeRef>, mut read: impl FnMut(&mut ReadCx<'_>) -> NodeChange + 'static) {
        self.bind_changes(node, move |cx| vec![read(cx)]);
    }

    pub fn bind_changes(&mut self, node: impl Into<NodeRef>, read: impl FnMut(&mut ReadCx<'_>) -> Vec<NodeChange> + 'static) {
        self.window.work.bindings.push((self.scope, ScopedBinding { node: node.into(), call: Box::new(read) }));
    }

    pub fn structure<S: PartialEq + 'static>(
        &mut self,
        read: impl FnMut(&mut ReadCx<'_>) -> S + 'static,
        build: impl FnMut(&mut MountCx<'_>, &S) + 'static,
    ) {
        self.window.work.structures.push((self.scope, Box::new(Region { previous: None, read, build })));
    }

    /// Make a structural source subscribe to reactive values allocated by its current build.
    pub fn refresh_current_structure(&mut self) {
        if let Some(observer) = self.current_structure {
            self.world.invalidate_observer(observer);
        }
    }

    pub fn before_commit(&mut self, f: impl FnOnce(&mut MountCx<'_>) + 'static) {
        self.before_commit_in(self.scope, f);
    }

    pub fn before_commit_in(&mut self, owner: ScopeId, f: impl FnOnce(&mut MountCx<'_>) + 'static) {
        self.window.work.before.push((owner, Box::new(f)));
    }

    pub fn after_commit(&mut self, f: impl FnOnce(&TreeReceipt) + 'static) {
        self.window.work.after.push(Box::new(f));
    }

    pub fn handle(
        &mut self,
        node: impl Into<NodeRef>,
        kind: HandlerKind,
        call: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) -> HandlerOutcome + 'static,
    ) -> HandlerId {
        let node = node.into();
        let id = self.window.next_handler();
        self.window.handlers.insert(id, ScopedHandler { node, kind, scope: self.scope, call: Box::new(call) });
        self.change(node, NodeChange::Handler { kind, id: Some(id) });
        id
    }

    pub fn on(
        &mut self,
        node: impl Into<NodeRef>,
        kind: HandlerKind,
        call: impl Fn(&mut WriteCx<'_>, &HandlerEvent<'_>) + 'static,
    ) -> HandlerId {
        self.handle(node, kind, move |write, event| {
            call(write, event);
            HandlerOutcome::default()
        })
    }

    pub fn on_command(
        &mut self,
        node: impl Into<NodeRef>,
        name: impl Into<Arc<str>>,
        call: impl Fn(&mut WriteCx<'_>) + 'static,
    ) -> HandlerId {
        self.handle_command(node, name, move |write| {
            call(write);
            HandlerOutcome { consumed: true, ..Default::default() }
        })
    }

    /// Registers a named command whose handler reports whether it consumed the invocation.
    pub fn handle_command(
        &mut self,
        node: impl Into<NodeRef>,
        name: impl Into<Arc<str>>,
        call: impl Fn(&mut WriteCx<'_>) -> HandlerOutcome + 'static,
    ) -> HandlerId {
        let node = node.into();
        let name = name.into();
        let id = self.window.next_handler();
        self.window
            .handlers
            .insert(id, ScopedHandler { node, scope: self.scope, kind: HandlerKind::Command, call: Box::new(move |write, _| call(write)) });
        let slot = self.window.work.commands.entry(node).or_default();
        slot.commands.retain(|(candidate, _)| *candidate != name);
        slot.commands.push((name, id));
        id
    }

    pub fn key_context(&mut self, node: impl Into<NodeRef>, name: impl Into<Arc<str>>) {
        self.window.work.commands.entry(node.into()).or_default().key_context = Some(name.into());
    }

    pub fn observe_geometry(&mut self, node: impl Into<NodeRef>, signal: Signal<Option<LayoutBox>>) {
        let node = node.into();
        self.change(node, NodeChange::DemandBounds);
        self.window.feedback.push(ScopedFeedback { node, scope: self.scope, signal: FeedbackSignal::Geometry(signal) });
    }

    pub fn observe_image_state(&mut self, node: impl Into<NodeRef>, signal: Signal<octara_model::image::ImageState>) {
        self.window.feedback.push(ScopedFeedback { node: node.into(), scope: self.scope, signal: FeedbackSignal::Media(signal) });
    }

    pub fn observe_composition(&mut self, node: impl Into<NodeRef>, signal: Signal<bool>) {
        self.window.feedback.push(ScopedFeedback { node: node.into(), scope: self.scope, signal: FeedbackSignal::Composition(signal) });
    }

    pub fn observe_editor(&mut self, node: impl Into<NodeRef>, signal: Signal<Option<EditorState>>) {
        self.window.feedback.push(ScopedFeedback { node: node.into(), scope: self.scope, signal: FeedbackSignal::Editor(signal) });
    }

    pub fn observe_scroll(&mut self, node: impl Into<NodeRef>, signal: Signal<Vec2>) {
        self.window.feedback.push(ScopedFeedback { node: node.into(), scope: self.scope, signal: FeedbackSignal::Scroll(signal) });
    }

    pub fn observe_overscroll(&mut self, node: impl Into<NodeRef>, signal: Signal<f32>) {
        self.window.feedback.push(ScopedFeedback { node: node.into(), scope: self.scope, signal: FeedbackSignal::Overscroll(signal) });
    }

    pub fn observe_scroll_anchor(&mut self, node: impl Into<NodeRef>, signal: Signal<Option<NodeId>>) {
        self.window.feedback.push(ScopedFeedback { node: node.into(), scope: self.scope, signal: FeedbackSignal::ScrollAnchor(signal) });
    }

    pub fn observe_pinned(&mut self, node: impl Into<NodeRef>, signal: Signal<bool>) {
        self.window.feedback.push(ScopedFeedback { node: node.into(), scope: self.scope, signal: FeedbackSignal::Pinned(signal) });
    }

    /// Register a virtual section's ordered interior rows for document selection copy.
    /// Return `None` when the endpoints belong to another section, or an error when
    /// any unmounted interior row has no copy source.
    pub fn on_copy_rows(
        &mut self,
        node: impl Into<NodeRef>,
        copy: impl Fn(&mut ReadCx<'_>, NodeId, NodeId) -> Option<Result<Vec<CopyRow>, ()>> + 'static,
    ) {
        self.window.copy_rows.push(ScopedCopyRows { node: node.into(), scope: self.scope, call: Box::new(copy) });
    }

    pub fn on_copy_selection(
        &mut self,
        node: impl Into<NodeRef>,
        format: impl Fn(&mut ReadCx<'_>, &[CopiedText]) -> Option<String> + 'static,
    ) {
        self.on_copy_format(node, None, format);
    }

    pub fn on_copy_format(
        &mut self,
        node: impl Into<NodeRef>,
        kind: Option<octara_model::input::CopyFormat>,
        format: impl Fn(&mut ReadCx<'_>, &[CopiedText]) -> Option<String> + 'static,
    ) {
        self.window.copy_formats.push(ScopedCopyFormat { node: node.into(), scope: self.scope, kind, format: Box::new(format) });
    }

    pub fn on_reveal(&mut self, node: impl Into<NodeRef>, reveal: impl Fn(&mut WriteCx<'_>, &str) + 'static) {
        self.window.reveal.push(ScopedReveal { node: node.into(), scope: self.scope, call: Box::new(reveal) });
    }
}

struct PendingEdit {
    node: NodeId,
    proposal: EditorProposal,
    staged: Option<String>,
    rich: Option<Box<EditorRichText>>,
}

pub struct WindowRuntime {
    pub(crate) id: WindowId,
    pub(crate) options: WindowOptions,
    pub(crate) pending_presentation: Option<WindowPresentation>,
    pub(crate) presentation_published: bool,
    pub(crate) scope: ScopeId,
    pub(crate) tree: Tree,
    pub(crate) engine: Box<dyn HostEngine>,
    pub(crate) authoring: WindowAuthoring,
    pub(crate) shortcut_modifier: Modifiers,
    pub(crate) root: Option<WindowRoot>,
    pub(crate) tx: TxBuilder,
    pub(crate) work: MountWork,
    pub(crate) arbiter: InteractionArbiter,
    pub(crate) tooltip: HoverTooltip,
    pub(crate) hovered_tooltip: Signal<Option<HoveredTooltip>>,
    pub(crate) scrollbar_hovered: Option<ScrollbarKey>,
    pub(crate) viewport_rev: ViewportRev,
    pub(crate) viewport_source: Signal<WindowViewport>,
    pub(crate) breakpoint: ReadSignal<Breakpoint>,
    pub(crate) breakpoint_source: Signal<Breakpoint>,
    pub(crate) chrome_source: Signal<WindowChrome>,
    pub(crate) geometry_source: Signal<WindowGeometry>,
    pub(crate) geometry: WindowGeometry,
    pub(crate) handlers: FxHashMap<HandlerId, ScopedHandler>,
    pub(crate) structures: FxHashMap<ReactiveId, Box<dyn Structure>>,
    pub(crate) bindings: FxHashMap<ReactiveId, ScopedBinding>,
    node_scopes: FxHashMap<NodeId, ScopeId>,
    pub(crate) feedback: Vec<ScopedFeedback>,
    pub(crate) reveal: Vec<ScopedReveal>,
    pub(crate) copy_rows: Vec<ScopedCopyRows>,
    copy_formats: Vec<ScopedCopyFormat>,
    pub(crate) pending: TreeDelta,
    pub(crate) staged: TreeDelta,
    pub(crate) projection_open: bool,
    pub(crate) force_project: bool,
    pub(crate) next_deadline: Option<Duration>,
    pub(crate) settled: Option<WindowRev>,
    pub(crate) last_commit: Option<CommitOutput>,
    pub(crate) committed_at: Option<Duration>,
    pub(crate) observation_rev: u64,
    pub(crate) transaction_count: u64,
    pub(crate) handler_seq: u64,
    pub(crate) keyboard_open: bool,
    pub(crate) scroll_motion: ScrollMotion,
    pub(crate) text: TextInteraction,
    pub(crate) back: Option<(u64, HandlerId)>,
    edit: Option<PendingEdit>,
}

impl WindowRuntime {
    pub(crate) fn write<'a>(
        &'a self,
        world: &'a mut ReactiveWorld,
        scope: ScopeId,
        tasks: TaskSpawner,
        commands: &'a mut AppCommands,
    ) -> WriteCx<'a> {
        let mut write = world.write(scope, Some(self.id)).with_tasks(tasks).with_commands(commands);
        write.tree = Some(&self.tree);
        write
    }

    pub(crate) fn new(
        world: &mut ReactiveWorld,
        id: WindowId,
        tree_id: TreeId,
        options: WindowOptions,
        restored: Option<WindowGeometry>,
        engine: Box<dyn HostEngine>,
        root: WindowRoot,
    ) -> Self {
        let scope = world.new_scope(world.root_scope());
        let mut build = world.build(scope, Some(id));
        let breakpoint_value = options.presentation.theme.breakpoint_of(options.viewport.size.width);
        let viewport_source = build.signal(options.viewport);
        let breakpoint = build.signal(breakpoint_value);
        let geometry = restored.unwrap_or_else(|| options.initial_geometry());
        let geometry_source = build.signal(geometry);
        let chrome_source = build.signal(WindowChrome::default());
        let hovered_tooltip = build.signal(None);
        let mut tree = Tree::new(tree_id, options.presentation.theme.clone());
        let mut pending = tree.initial_delta();
        pending.merge(tree.apply_runtime(RuntimeOp::SetViewportContext {
            breakpoint: breakpoint_value,
            width: options.viewport.size.width,
            touch: options.touch,
        }));
        Self {
            id,
            pending_presentation: None,
            presentation_published: false,
            options,
            scope,
            tx: tree.transaction(),
            tree,
            shortcut_modifier: engine.shortcut_modifier(),
            engine,
            authoring: WindowAuthoring::default(),
            root: Some(root),
            work: MountWork::default(),
            arbiter: InteractionArbiter::default(),
            tooltip: HoverTooltip::default(),
            hovered_tooltip,
            scrollbar_hovered: None,
            viewport_rev: ViewportRev::default(),
            viewport_source,
            breakpoint: breakpoint.read_only(),
            breakpoint_source: breakpoint,
            chrome_source,
            geometry_source,
            geometry,
            handlers: FxHashMap::default(),
            structures: FxHashMap::default(),
            bindings: FxHashMap::default(),
            node_scopes: FxHashMap::default(),
            feedback: Vec::new(),
            reveal: Vec::new(),
            copy_rows: Vec::new(),
            copy_formats: Vec::new(),
            pending,
            staged: TreeDelta::default(),
            projection_open: false,
            force_project: true,
            next_deadline: None,
            settled: None,
            last_commit: None,
            committed_at: None,
            observation_rev: 0,
            transaction_count: 0,
            handler_seq: 0,
            keyboard_open: false,
            scroll_motion: ScrollMotion::default(),
            text: TextInteraction::default(),
            back: None,
            edit: None,
        }
    }

    /// Whether a key has an authored shortcut in the settled window.
    pub fn has_shortcut(&self, event: KeyEvent) -> bool {
        self.settled.is_some() && self.authoring.has_shortcut(event, self.shortcut_modifier)
    }

    /// Register a window-owned handler for host close requests.
    pub fn on_close_request(&self, run: impl Fn(&mut WriteCx<'_>) -> bool + 'static) {
        self.authoring.set_close_request(self.scope, run);
    }

    pub fn id(&self) -> WindowId {
        self.id
    }
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// Request or release the host accessibility projection for a consumer.
    pub fn set_accessibility_active(&self, consumer: AccessibilityConsumer, active: bool) {
        self.engine.set_accessibility_active(&self.tree, consumer, active);
    }

    pub fn options(&self) -> &WindowOptions {
        &self.options
    }
    pub fn presentation(&self) -> &WindowPresentation {
        &self.options.presentation
    }
    pub fn window_geometry(&self) -> WindowGeometry {
        self.geometry
    }
    pub fn settled(&self) -> Option<WindowRev> {
        self.settled
    }
    pub fn observation_revision(&self) -> u64 {
        self.observation_rev
    }
    pub fn transaction_count(&self) -> u64 {
        self.transaction_count
    }
    pub fn back_target(&self) -> Option<TargetGuard> {
        self.named_command("navigation.back").map(|(node, _)| self.tree.guard(node))
    }

    pub(crate) fn named_command(&self, name: &str) -> Option<(NodeId, HandlerId)> {
        let command = |node| {
            let (_, handler) = self.tree.commands(node)?.commands.iter().find(|(candidate, _)| candidate.as_ref() == name)?;
            self.tree.is_shown(node).then_some((node, *handler))
        };
        let tree = &self.tree;
        std::iter::successors(tree.focus(), |&node| tree.logical_parent(node))
            .find_map(command)
            .or_else(|| tree.subtree(tree.front_modal().unwrap_or(tree.root())).find_map(command))
    }

    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }
    /// Check that published tree handlers and live closures agree.
    pub fn validate_handler_integrity(&self, world: &ReactiveWorld) -> bool {
        let mut count = 0;
        for (node, _) in self.tree.nodes().filter(|(node, _)| self.tree.is_live(*node)) {
            let handlers = self.tree.handlers(node).chain(
                self.tree.commands(node).into_iter().flat_map(|slot| slot.commands.iter()).map(|(_, id)| (HandlerKind::Command, *id)),
            );
            for (kind, id) in handlers {
                count += 1;
                if !self.handlers.get(&id).is_some_and(|handler| {
                    self.tree.node_ref(node) == handler.node && handler.kind == kind && world.is_scope_alive(handler.scope)
                }) {
                    return false;
                }
            }
        }
        count == self.handlers.len()
    }

    pub fn last_commit(&self) -> Option<&CommitOutput> {
        self.last_commit.as_ref()
    }

    pub fn committed_at(&self) -> Option<Duration> {
        self.committed_at
    }

    pub(crate) fn copy_selection(&self, world: &mut ReactiveWorld, region: NodeId) -> Option<String> {
        self.copy_selection_as(world, region, None)
    }

    pub(crate) fn copy_selection_as(
        &self,
        world: &mut ReactiveWorld,
        region: NodeId,
        kind: Option<&octara_model::input::CopyFormat>,
    ) -> Option<String> {
        use octara_model::input::CopyFormat;
        let ranges = self.tree.selected_text_ranges(region, self.tree.doc_selection(region)?)?;
        if let Some(formatted) = self.format_selection(world, region, &ranges, kind) {
            return Some(formatted);
        }
        if matches!(kind, Some(CopyFormat::Custom(_))) {
            return None;
        }
        let first = ranges.first()?.0;
        let last = ranges.last()?.0;
        let mut intersects_virtual_rows = false;
        for copy in &self.copy_rows {
            let NodeRef::Existing { id: container, .. } = copy.node else { continue };
            if !world.is_scope_alive(copy.scope) {
                continue;
            }
            let row_of = |node| {
                std::iter::successors(Some(node), |&node| self.tree.physical_parent(node))
                    .find(|&node| self.tree.physical_parent(node) == Some(container))
            };
            let (Some(first_row), Some(last_row)) = (row_of(first), row_of(last)) else {
                intersects_virtual_rows |= ranges.iter().any(|&(node, _)| self.tree.is_within(node, container));
                continue;
            };
            if first_row == last_row {
                continue;
            }
            let Some(rows) = (copy.call)(&mut world.read(), first_row, last_row) else {
                intersects_virtual_rows = true;
                continue;
            };
            let mut result = Vec::new();
            let add_endpoint = |row, result: &mut Vec<String>| {
                for (node, range) in &ranges {
                    if self.tree.is_within(*node, row) {
                        result.push(self.tree.text(*node)?.text.get(range.clone())?.to_owned());
                    }
                }
                Some(())
            };
            add_endpoint(first_row, &mut result)?;
            for row in rows.ok()? {
                match row {
                    CopyRow::Unmounted(text) => result.push(text),
                    CopyRow::Mounted(node) => {
                        for node in self.tree.subtree(node).filter(|&text| self.tree.selectable_in(region, text)) {
                            result.push(self.tree.text(node)?.text.to_string());
                        }
                    }
                }
            }
            add_endpoint(last_row, &mut result)?;
            return Some(result.join("\n"));
        }
        if intersects_virtual_rows {
            return None;
        }
        self.tree.selected_text(region)
    }

    pub(crate) fn format_document_selection(&self, world: &mut ReactiveWorld, region: NodeId, selection: DocSelection) -> Option<String> {
        let ranges = self.tree.selected_text_ranges(region, selection)?;
        self.format_selection(world, region, &ranges, None)
    }

    pub(crate) fn copy_formats(&self, world: &ReactiveWorld, region: NodeId) -> Vec<octara_model::input::CopyFormat> {
        use octara_model::input::CopyFormat;
        let mut formats = vec![CopyFormat::Plain];
        for copy in &self.copy_formats {
            if matches!(copy.node, NodeRef::Existing { id, .. } if id == region)
                && world.is_scope_alive(copy.scope)
                && let Some(format) = &copy.kind
                && !formats.contains(format)
            {
                formats.push(format.clone());
            }
        }
        formats
    }

    fn format_selection(
        &self,
        world: &mut ReactiveWorld,
        region: NodeId,
        ranges: &[(NodeId, std::ops::Range<usize>)],
        kind: Option<&octara_model::input::CopyFormat>,
    ) -> Option<String> {
        let copy = self.copy_formats.iter().rev().find(|copy| {
            copy.kind.as_ref() == kind
                && matches!(copy.node, NodeRef::Existing { id, .. } if id == region)
                && world.is_scope_alive(copy.scope)
        })?;
        let copied = self.copy_selection_metadata(region, ranges);
        (copy.format)(&mut world.read(), &copied)
    }

    fn copy_selection_metadata(&self, region: NodeId, ranges: &[(NodeId, std::ops::Range<usize>)]) -> Vec<CopiedText> {
        let mut selected = ranges
            .iter()
            .filter_map(|(node, range)| {
                let len = self.tree.text(*node)?.text.len();
                let mut outer_row = None;
                let mut rows: Vec<_> = std::iter::successors(Some(*node), |&node| self.tree.physical_parent(node))
                    .take_while(|&node| node != region)
                    .filter_map(|node| {
                        let key = self.tree.provenance(node)?.row_key.as_ref()?;
                        outer_row = Some(node);
                        Some(key.to_string())
                    })
                    .collect();
                rows.reverse();
                let text = CopiedText { rows, whole: range.start == 0 && range.end == len, whole_row: false };
                Some((outer_row, len > 0, text))
            })
            .collect::<Vec<_>>();
        for row in selected.chunk_by_mut(|a, b| a.0 == b.0) {
            let Some(root) = row[0].0 else { continue };
            if !row.iter().all(|(_, _, text)| text.whole) {
                continue;
            }
            let included = row.iter().filter(|(_, has_text, _)| *has_text).count();
            let total = self
                .tree
                .subtree(root)
                .filter(|&node| self.tree.text(node).is_some_and(|slot| !slot.text.is_empty()) && self.tree.selectable_in(region, node))
                .count();
            if included > 0 && included == total {
                for (_, _, text) in row {
                    text.whole_row = true;
                }
            }
        }
        selected.into_iter().map(|(_, _, text)| text).collect()
    }

    pub fn hit_test(&self, at: Point) -> Option<ContactTarget> {
        self.settled?;
        self.engine.hit_test(&self.tree, at)
    }

    /// Whether a burst at a fixed point can share one wheel hit test.
    pub fn can_coalesce_wheel(&self, pos: Point) -> bool {
        let Some(hit) = self.hit_test(pos).and_then(|target| target.hit.node()) else { return false };
        let mut chain =
            std::iter::successors(Some(hit), |&node| self.tree.physical_parent(node)).filter(|&node| self.tree.scroll(node).is_some());
        let Some(port) = chain.next() else { return true };
        let inside = |node| std::iter::successors(Some(node), |&node| self.tree.physical_parent(node)).any(|node| node == port);
        // Nested scrollports can move under the pointer after the first delta.
        // Scroll feedback can also change the tree between two deltas.
        chain.next().is_none()
            && !self.tree.scroll_nodes().any(|node| node != port && inside(node))
            && !self.feedback.iter().any(|binding| {
                matches!(
                    binding.signal,
                    FeedbackSignal::Scroll(_) | FeedbackSignal::ScrollAnchor(_) | FeedbackSignal::Geometry(_) | FeedbackSignal::Pinned(_)
                ) && matches!(binding.node, NodeRef::Existing { id, .. } if inside(id))
            })
    }

    /// Find the nearest interactive drop receiver at a point in window coordinates.
    /// Use its guard when dispatching a host file drop.
    pub fn file_drop_target(&self, at: Point) -> Option<TargetGuard> {
        let hit = self.hit_test(at)?.hit;
        let node = hit.node().filter(|_| self.tree.contains_target(hit))?;
        std::iter::successors(Some(node), |&node| self.tree.logical_parent(node))
            .map(|node| self.tree.guard(node))
            .find(|&target| self.file_drop_handler(target).is_some())
    }

    pub(crate) fn file_drop_handler(&self, target: TargetGuard) -> Option<HandlerId> {
        self.settled?;
        let node = target.node().filter(|_| self.tree.contains_target(target))?;
        self.tree.handler(node, HandlerKind::FileDrop).filter(|_| self.tree.is_interactive(node))
    }

    pub fn caret_rect(&self, node: NodeId) -> Option<octara_model::geometry::Rect> {
        self.settled?;
        self.engine.caret_rect(&self.tree, node)
    }

    pub fn selection_rect(&self, node: NodeId) -> Option<octara_model::geometry::Rect> {
        self.settled?;
        self.engine.selection_rect(&self.tree, node)
    }

    pub fn geometry(&self, node: NodeId) -> Option<LayoutBox> {
        self.settled?;
        self.engine.geometry(node)
    }

    /// Text truncation reported by the current settled host layout.
    pub fn clipped_text(&self) -> Vec<(NodeId, TextClip)> {
        self.settled.map_or_else(Vec::new, |_| self.engine.clipped_text(&self.tree))
    }

    /// Resolve context for a live node and identify the scope that provided the value.
    pub fn node_context<T: Clone + 'static>(&self, world: &ReactiveWorld, node: NodeId) -> Option<(ScopeId, T)> {
        if !self.tree.is_live(node) {
            return None;
        }
        let node = self.tree.link(node).and_then(|_| self.tree.physical_parent(node)).unwrap_or(node);
        let scope = self.node_scopes.get(&node).copied().unwrap_or(self.scope);
        world.context_with_owner(scope)
    }

    pub(crate) fn stage_change(&mut self, node: NodeRef, change: NodeChange) {
        if let NodeChange::Overlay(overlay) = &change {
            let owner = overlay.mount.map(|(owner, _)| owner);
            if self.work.portal_owners.insert(node, owner) != Some(owner) {
                self.work.retirement_cache.get_mut().changed_ancestry(node);
            }
        }
        if let NodeRef::Existing { id, .. } = node
            && let Some(edit) = &mut self.edit
            && edit.node == id
        {
            match change {
                NodeChange::EditorValue(value) => {
                    edit.staged = Some(value);
                    return;
                }
                NodeChange::EditorRichText(rich) => {
                    edit.rich = Some(rich);
                    return;
                }
                _ => {}
            }
        }
        self.tx.change(node, change);
    }

    pub(crate) fn propose_edit(&mut self, node: NodeId, proposal: EditorProposal) {
        self.edit = Some(PendingEdit { node, proposal, staged: None, rich: None });
    }

    pub(crate) fn is_retiring(&self, node: NodeRef) -> bool {
        if self.work.retire_nodes.is_empty() {
            return false;
        }
        self.work
            .retirement_cache
            .borrow_mut()
            .find(
                node,
                |node| self.work.retire_nodes.contains(&node),
                |node| {
                    let physical = match node {
                        NodeRef::Existing { id, .. } => self.tree.physical_parent(id).map(|id| self.tree.node_ref(id)),
                        NodeRef::Pending(_) => None,
                    };
                    let owner = self.work.portal_owners.get(&node).copied().unwrap_or_else(|| match node {
                        NodeRef::Existing { id, .. } => {
                            self.tree.overlay(id).and_then(|slot| slot.mount).map(|mount| self.tree.node_ref(mount.owner))
                        }
                        NodeRef::Pending(_) => None,
                    });
                    [self.work.parents.get(&node).copied().or(physical), owner]
                },
            )
            .is_some()
    }

    pub(crate) fn scope_is_retiring(&self, world: &ReactiveWorld, scope: ScopeId) -> bool {
        self.work.retire_scopes.iter().any(|&ancestor| world.scope_is_within(scope, ancestor))
    }

    pub(crate) fn discard_retiring_allocations(&mut self, world: &ReactiveWorld) {
        let mut created = std::mem::take(&mut self.work.created);
        created.retain(|&(scope, node)| {
            let retiring_node = self.is_retiring(node.into());
            if retiring_node || self.scope_is_retiring(world, scope) {
                self.tx.discard(node);
                if self.work.retire_nodes.insert(node.into()) && !retiring_node {
                    self.work.retirement_cache.get_mut().clear();
                }
                return false;
            }
            scope != self.scope
        });
        self.work.created = created;
    }

    pub(crate) fn commit_tree(&mut self) -> Result<(), RejectedTransaction> {
        let edit = self.edit.take();
        let mut editor_update = None;
        if let Some(PendingEdit { node, mut proposal, staged, mut rich }) = edit {
            let accepted = match staged.as_deref() {
                Some(value) => proposal.matches_value(value),
                None => self.tree.editor(node).is_some_and(|editor| proposal.matches_retained(editor)),
            };
            if accepted && let Some(rich) = rich.take() {
                proposal.set_rich_text(*rich);
            }
            editor_update = Some((node, proposal.into_update(accepted)));
            if !accepted && !self.is_retiring(self.tree.node_ref(node)) {
                if let Some(value) = staged {
                    self.tx.change(self.tree.node_ref(node), NodeChange::EditorValue(value));
                }
                if let Some(rich) = rich {
                    self.tx.change(self.tree.node_ref(node), NodeChange::EditorRichText(rich));
                }
            }
        }
        for (parent, index, child) in std::mem::take(&mut self.work.places) {
            if !self.is_retiring(child) {
                self.tx.place(parent, index, child);
            }
        }
        for (node, slot) in std::mem::take(&mut self.work.commands) {
            if !self.is_retiring(node) {
                self.tx.change(node, NodeChange::Commands(Box::new(slot)));
            }
        }
        self.work.parents.clear();
        self.work.portal_owners.clear();
        self.work.retire_nodes.clear();
        self.work.retirement_cache.get_mut().clear();
        self.work.place_by_child.clear();

        if !self.tx.is_empty() {
            let tx = std::mem::replace(&mut self.tx, self.tree.transaction()).finish();
            let (receipt, delta) = self.tree.apply(tx)?;
            self.transaction_count = self.transaction_count.checked_add(1).expect("transaction count exhausted");
            self.tx = self.tree.transaction();
            self.merge_delta(delta);
            self.activate(receipt);
        }
        if let Some((node, update)) = editor_update {
            self.apply(RuntimeOp::EditorApply { node, update });
        }
        Ok(())
    }

    fn activate(&mut self, receipt: TreeReceipt) {
        for (scope, pending) in self.work.created.drain(..) {
            if let Some(node) = receipt.node(pending) {
                self.node_scopes.insert(node, scope);
            }
        }
        let resolve = |node: &mut NodeRef| {
            if let NodeRef::Pending(pending) = *node
                && let Some(id) = receipt.node(pending)
            {
                *node = self.tree.node_ref(id);
            }
        };
        self.handlers.retain(|&id, handler| {
            resolve(&mut handler.node);
            matches!(handler.node, NodeRef::Existing { id: node, .. } if self.tree.is_live(node) && if handler.kind == HandlerKind::Command {
                self.tree.commands(node).is_some_and(|slot| slot.commands.iter().any(|entry| entry.1 == id))
            } else {
                self.tree.handler(node, handler.kind) == Some(id)
            })
        });
        for binding in self.bindings.values_mut() {
            resolve(&mut binding.node);
        }
        self.feedback.retain_mut(|feedback| {
            resolve(&mut feedback.node);
            matches!(feedback.signal, FeedbackSignal::Editor(_))
                || matches!(feedback.node, NodeRef::Existing { id, .. } if self.tree.is_live(id))
        });
        self.reveal.retain_mut(|reveal| {
            resolve(&mut reveal.node);
            matches!(reveal.node, NodeRef::Existing { id, .. } if self.tree.is_live(id))
        });
        self.copy_rows.retain_mut(|copy| {
            resolve(&mut copy.node);
            matches!(copy.node, NodeRef::Existing { id, .. } if self.tree.is_live(id))
        });
        self.copy_formats.retain_mut(|copy| {
            resolve(&mut copy.node);
            matches!(copy.node, NodeRef::Existing { id, .. } if self.tree.is_live(id))
        });
        for callback in self.work.after.drain(..) {
            callback(&receipt);
        }
    }

    pub(crate) fn apply(&mut self, op: RuntimeOp) {
        let delta = self.tree.apply_runtime(op);
        self.merge_delta(delta);
    }

    pub(crate) fn set_style_preferences(&mut self, preferences: &SystemPreferences) {
        let delta = self.tree.set_system_preferences(preferences);
        self.merge_delta(delta);
    }

    fn merge_delta(&mut self, mut delta: TreeDelta) {
        for retired in &delta.retired {
            if let RetiredNode::Destroyed(node) = retired {
                self.node_scopes.remove(node);
            }
        }
        drop(delta.take_retired_assets());
        drop(delta.take_retired_native_props());
        self.pending.merge(delta);
    }

    fn next_handler(&mut self) -> HandlerId {
        self.handler_seq = self.handler_seq.checked_add(1).expect("handler id exhausted");
        HandlerId(self.handler_seq)
    }
}
