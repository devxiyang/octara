//! Owns reactive state, window lifetimes, and queued platform requests.

use crate::HostEngine;
use crate::arena::{PersistedState, ReactiveWorld, ScopeId};
use crate::cx::{BuildCx, WriteCx};
use crate::graph::value::Signal;
use crate::media::{MediaCacheBackend, MediaLoader, MediaScheduler};
use crate::task::TaskScheduler;
use crate::turn::{TurnError, TurnReceipt};
use crate::window::{EngineFactory, MountCx, WindowOptions, WindowRoot, WindowRuntime};
use octara_model::ask::{Answer, Ask, AskId, Menu, ServiceId, ServiceOutcome, ServiceRequest};
use octara_model::diagnostics::Diagnostic;
use octara_model::geometry::Vec2;
use octara_model::identity::{NativeId, TreeId, WindowId};
use octara_model::input::{EditorEvent, HostReply, InteractionEvent, PasteEvent, TextEdit};
use octara_model::native::NativeCommand;
use octara_model::theme::{ColorScheme, Theme};
use octara_model::window::{SystemPreferences, UserAttention, WindowGeometry, WindowGeometryRequest, WindowMaterial, WindowPresentation};
use octara_tree::NodeId;
use octara_tree::editor::{EditorSession, EditorState};
use octara_tree::selection::DocSelection;
use serde::{Deserialize, Serialize};
use slotmap::{Key, SlotMap};
use std::collections::{BTreeMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Input {
    Interaction(InteractionEvent),
    Editor(EditorEvent),
    Host(HostReply),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RestartState {
    pub state_schema: String,
    pub values: PersistedState,
    pub window_geometry: BTreeMap<String, WindowGeometry>,
    pub normal_window_geometry: BTreeMap<String, WindowGeometry>,
}

type AnswerCallback = Box<dyn FnOnce(&mut WriteCx<'_>, Answer)>;
type ServiceCallback = Box<dyn FnOnce(&mut WriteCx<'_>, ServiceOutcome)>;

#[derive(Debug)]
pub enum HostCommand {
    Opened(WindowId),
    Closed(WindowId),
    Menus(Vec<Menu>),
    Ask { window: WindowId, id: AskId, ask: Ask },
    WithdrawAsk { window: WindowId, id: AskId },
    Service { window: WindowId, id: ServiceId, request: ServiceRequest },
    OpenUrl { url: String },
    UpdateWindowPresentation { window: WindowId, title: Arc<str>, color_scheme: ColorScheme, material: WindowMaterial },
    SetAppBadge { badge: Option<Arc<str>> },
    RequestUserAttention { window: Option<WindowId>, attention: UserAttention },
    SetWindowGeometry { window: WindowId, geometry: WindowGeometryRequest },
    FocusWindow { window: WindowId },
    MinimizeWindow { window: WindowId },
    ToggleMaximizeWindow { window: WindowId },
    StartWindowDrag { window: WindowId },
    NativeCommand { window: WindowId, command: NativeCommand },
}

pub(crate) enum AppCommand {
    Open { id: WindowId, options: Box<WindowOptions>, root: WindowRoot },
    Close(WindowId),
    Focus { window: WindowId, test_id: Option<String> },
    WindowControl { window: WindowId, command: HostCommand },
    WindowPresentation { window: WindowId, update: PresentationUpdate },
    SetAppBadge(Option<Arc<str>>),
    RequestUserAttention { window: Option<WindowId>, attention: UserAttention },
    Command { window: WindowId, name: String, after_commit: bool },
    Scroll { window: WindowId, node: NodeId, offset: Vec2 },
    ScrollEnd { window: WindowId, container: String },
    Reveal { window: WindowId, container: String, key: String },
    Ask { window: WindowId, scope: ScopeId, ask: Ask, done: AnswerCallback },
    Service { window: WindowId, scope: ScopeId, request: ServiceRequest, done: ServiceCallback },
    EditorEdit { window: WindowId, event: EditorEvent },
    Paste { window: WindowId, target: crate::turn::PasteTarget, event: PasteEvent },
    NativeCommand { window: WindowId, command: NativeCommand },
}

pub(crate) enum PresentationUpdate {
    Title(Arc<str>),
    Theme(Box<Theme>),
    Material(WindowMaterial),
    All(Box<WindowPresentation>),
}

impl PresentationUpdate {
    pub(crate) fn apply(self, presentation: &mut WindowPresentation) {
        match self {
            Self::Title(title) => presentation.title = title,
            Self::Theme(theme) => presentation.theme = *theme,
            Self::Material(material) => presentation.material = material,
            Self::All(next) => *presentation = *next,
        }
    }
}

#[derive(Default)]
pub struct AppCommands {
    pub(crate) queued: VecDeque<AppCommand>,
    pub(crate) windows: SlotMap<WindowId, ()>,
}

pub(crate) struct PendingAsk {
    pub window: WindowId,
    pub scope: ScopeId,
    pub ask: Ask,
    pub done: AnswerCallback,
}

pub(crate) struct PendingService {
    pub window: WindowId,
    pub scope: ScopeId,
    pub done: ServiceCallback,
}

pub struct AppRuntime {
    pub(crate) world: ReactiveWorld,
    pub(crate) system_preferences: Signal<SystemPreferences>,
    host_preferences: Option<SystemPreferences>,
    pub(crate) windows: BTreeMap<WindowId, WindowRuntime>,
    pub(crate) tasks: TaskScheduler,
    pub(crate) media: MediaScheduler,
    text_stream_ready_at: Duration,
    text_stream_deadline: Option<Duration>,
    pub(crate) commands: AppCommands,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) failed: bool,
    pub(crate) engine: EngineFactory,
    pub(crate) namespace: [u64; 2],
    pub(crate) asks: BTreeMap<u64, PendingAsk>,
    pub(crate) services: BTreeMap<u64, PendingService>,
    pub(crate) request_seq: u64,
    pub(crate) staged_host: Vec<HostCommand>,
    pub(crate) published_host: Vec<HostCommand>,
    pub(crate) menu_window: Option<WindowId>,
    pub(crate) published_menus: Vec<Menu>,
    pub(crate) app_badge: Option<Arc<str>>,
    pub(crate) published_badge: Option<Arc<str>>,
    pub(crate) window_geometry: BTreeMap<String, WindowGeometry>,
    pub(crate) normal_window_geometry: BTreeMap<String, WindowGeometry>,
    state_schema: String,
}

impl AppRuntime {
    pub fn new(engine: impl Fn(WindowId, &WindowOptions) -> Box<dyn HostEngine> + 'static) -> Self {
        Self::with_namespace(TreeId::fresh_namespace(), engine)
    }

    pub fn with_namespace(namespace: [u64; 2], engine: impl Fn(WindowId, &WindowOptions) -> Box<dyn HostEngine> + 'static) -> Self {
        let mut world = ReactiveWorld::new();
        let system_preferences = world.build(world.root_scope(), None).signal(SystemPreferences::default());
        Self {
            world,
            system_preferences,
            host_preferences: None,
            windows: BTreeMap::new(),
            tasks: TaskScheduler::new(),
            media: MediaScheduler::default(),
            text_stream_ready_at: Duration::ZERO,
            text_stream_deadline: None,
            commands: AppCommands::default(),
            diagnostics: Vec::new(),
            failed: false,
            engine: Box::new(engine),
            namespace,
            asks: BTreeMap::new(),
            services: BTreeMap::new(),
            request_seq: 0,
            staged_host: Vec::new(),
            published_host: Vec::new(),
            menu_window: None,
            published_menus: Vec::new(),
            app_badge: None,
            published_badge: None,
            window_geometry: BTreeMap::new(),
            normal_window_geometry: BTreeMap::new(),
            state_schema: String::new(),
        }
    }

    /// Installs the transport and decoder before opening the application's windows.
    pub fn set_media_loader(&mut self, loader: impl MediaLoader) {
        self.media.loader = Arc::new(loader);
    }

    /// Installs the optional public HTTP response cache.
    pub fn set_media_cache_backend(&mut self, backend: Arc<dyn MediaCacheBackend>) {
        self.media.disk_cache = Some(backend);
    }

    pub fn set_state_schema(&mut self, schema: impl Into<String>) {
        self.state_schema = schema.into();
    }

    pub fn export_persisted(&self) -> RestartState {
        RestartState {
            state_schema: self.state_schema.clone(),
            values: self.world.export_persisted(),
            window_geometry: self.window_geometry.clone(),
            normal_window_geometry: self.normal_window_geometry.clone(),
        }
    }

    pub fn import_persisted(&mut self, saved: RestartState) -> bool {
        if saved.state_schema != self.state_schema {
            return false;
        }
        self.world.import_persisted(saved.values);
        self.window_geometry = saved.window_geometry;
        self.normal_window_geometry = saved.normal_window_geometry;
        true
    }

    pub fn world(&self) -> &ReactiveWorld {
        &self.world
    }

    /// Supplies the current preferences before the first native or web window builds.
    pub fn set_system_preferences(&mut self, preferences: SystemPreferences) -> Result<(), TurnError> {
        self.require_active()?;
        self.guarded(|app| app.update_system_preferences(preferences));
        self.run_turn(None)?;
        Ok(())
    }

    pub fn system_preferences(&self) -> SystemPreferences {
        self.host_preferences.unwrap_or_default()
    }

    pub(crate) fn update_system_preferences(&mut self, preferences: SystemPreferences) {
        let previous = self.host_preferences.replace(preferences);
        self.world.write(self.world.root_scope(), None).set_if_changed(self.system_preferences, preferences);
        if previous.is_none_or(|previous| previous.reduced_motion != preferences.reduced_motion) {
            for window in self.windows.values_mut() {
                window.engine.set_reduced_motion(preferences.reduced_motion);
                window.force_project = true;
            }
        }
    }

    pub fn window(&self, id: WindowId) -> Option<&WindowRuntime> {
        self.windows.get(&id)
    }

    /// Format a host-owned selection when the region supplies a custom clipboard representation.
    pub fn format_document_selection(&mut self, id: WindowId, region: NodeId, selection: DocSelection) -> Option<String> {
        let window = self.windows.get(&id)?;
        window.settled?;
        window.format_document_selection(&mut self.world, region, selection)
    }
    pub fn windows(&self) -> impl Iterator<Item = (&WindowId, &WindowRuntime)> {
        self.windows.iter()
    }
    pub fn is_failed(&self) -> bool {
        self.failed
    }
    pub fn now(&self) -> Duration {
        self.tasks.now()
    }

    pub fn next_deadline(&self) -> Option<Duration> {
        self.tasks
            .next_deadline()
            .into_iter()
            .chain(self.text_stream_deadline)
            .chain(self.windows.values().flat_map(|window| window.next_deadline.into_iter().chain(window.tooltip.deadline())))
            .min()
    }

    pub(crate) fn coalesce_text_streams(&mut self, now: Duration) {
        self.text_stream_deadline = None;
        if !self.world.has_pending_text_stream() {
            return;
        }
        if now < self.text_stream_ready_at {
            self.text_stream_deadline = Some(self.text_stream_ready_at);
        } else if self.world.drain_text_streams() {
            // Respect the app clock in tests and publish at most once per frame.
            self.text_stream_ready_at = now.saturating_add(Duration::from_nanos(1_000_000_000 / 60));
        }
    }

    pub fn set_wake(&mut self, wake: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.tasks.set_wake(wake);
    }

    pub fn take_host_commands(&mut self) -> Vec<HostCommand> {
        std::mem::take(&mut self.published_host)
    }

    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.diagnostics.append(&mut self.world.take_diagnostics());
        std::mem::take(&mut self.diagnostics)
    }

    pub fn open_window(&mut self, options: WindowOptions, root: impl FnOnce(&mut MountCx<'_>) + 'static) -> Result<WindowId, TurnError> {
        self.require_active()?;
        let id = self.commands.windows.insert(());
        self.commands.queued.push_back(AppCommand::Open { id, options: Box::new(options), root: Box::new(root) });
        if let Err(error) = self.run_turn(None) {
            self.remove_window(id);
            self.staged_host
                .retain(|command| !matches!(command, HostCommand::Opened(window) | HostCommand::Closed(window) if *window == id));
            return Err(error);
        }
        Ok(id)
    }

    pub(crate) fn insert_window(&mut self, id: WindowId, mut options: WindowOptions, root: WindowRoot) {
        let restored = options.persistence_key.as_ref().and_then(|key| self.window_geometry.get(key)).copied();
        if let Some(geometry) = restored {
            let normal = options.persistence_key.as_ref().and_then(|key| self.normal_window_geometry.get(key));
            let bounds = normal.filter(|_| geometry.maximized || geometry.fullscreen).copied().unwrap_or(geometry);
            options.position = bounds.position.or(options.position);
            options.viewport.size = bounds.size;
            options.maximized = geometry.maximized;
        }
        if let Some(key) = &options.persistence_key
            && options.maximized
        {
            self.normal_window_geometry
                .entry(key.clone())
                .or_insert_with(|| WindowGeometry { maximized: false, ..options.initial_geometry() });
        }
        let mut engine = (self.engine)(id, &options);
        engine.set_window_material(options.presentation.material);
        if let Some(preferences) = self.host_preferences {
            engine.set_reduced_motion(preferences.reduced_motion);
        }
        let tree_id = TreeId::new(self.namespace, id.data().as_ffi());
        let window = WindowRuntime::new(&mut self.world, id, tree_id, options, restored, engine, root);
        self.windows.insert(id, window);
        self.staged_host.push(HostCommand::Opened(id));
    }

    pub fn close_window(&mut self, window: WindowId) -> Result<(), TurnError> {
        self.require_active()?;
        self.commands.queued.push_back(AppCommand::Close(window));
        self.run_turn(None)?;
        Ok(())
    }

    pub fn shutdown(&mut self) {
        let ids: Vec<_> = self.windows.keys().copied().collect();
        for id in ids {
            self.remove_window(id);
        }
        self.world.dispose(self.world.root_scope());
        self.media.clear(&mut self.tasks);
        self.tasks.shutdown();
        self.commands.queued.clear();
        self.failed = true;
        self.published_host.append(&mut self.staged_host);
    }

    pub(crate) fn remove_window(&mut self, id: WindowId) {
        if let Some(mut window) = self.windows.remove(&id) {
            if window.projection_open {
                window.engine.rollback();
            }
            self.commands.windows.remove(id);
            self.world.dispose(window.scope);
            self.tasks.cancel_retired_scopes(&self.world);
            self.cancel_platform_work();
            self.staged_host.push(HostCommand::Closed(id));
        }
    }

    pub(crate) fn cancel_platform_work(&mut self) {
        self.asks.retain(|&id, pending| {
            if self.world.is_scope_alive(pending.scope) {
                return true;
            }
            self.staged_host.push(HostCommand::WithdrawAsk { window: pending.window, id: AskId(id) });
            false
        });
        self.services.retain(|_, pending| self.world.is_scope_alive(pending.scope));
    }

    pub fn dispatch(&mut self, window: WindowId, input: Input) -> Result<TurnReceipt, TurnError> {
        self.run_turn(Some((window, input)))
    }

    pub fn set_keyboard_open(&mut self, window: WindowId, open: bool) {
        if let Some(state) = self.windows.get_mut(&window) {
            state.keyboard_open = open;
        }
    }

    pub fn settle(&mut self) -> Result<TurnReceipt, TurnError> {
        self.run_turn(None)
    }

    /// Schedule a fresh host projection after externally changed geometry.
    pub fn invalidate_projection(&mut self, window: WindowId) -> Result<(), TurnError> {
        self.require_active()?;
        let window = self.windows.get_mut(&window).ok_or(TurnError::UnknownWindow(window))?;
        window.force_project = true;
        Ok(())
    }

    pub fn advance_to(&mut self, deadline: Duration) -> Result<TurnReceipt, TurnError> {
        self.require_active()?;
        self.guarded(|app| app.tasks.advance_to(deadline));
        self.run_turn(None)
    }

    pub fn build<R>(&mut self, f: impl FnOnce(&mut BuildCx<'_>) -> R) -> Result<R, TurnError> {
        self.require_active()?;
        let scope = self.world.root_scope();
        let value = self.guarded(|app| {
            let mut cx =
                app.world.build(scope, None).with_tasks(app.tasks.spawner()).with_system_preferences(app.system_preferences.read_only());
            f(&mut cx)
        });
        self.run_turn(None)?;
        Ok(value)
    }

    pub fn update<R>(&mut self, window: Option<WindowId>, f: impl FnOnce(&mut WriteCx<'_>) -> R) -> Result<R, TurnError> {
        self.require_active()?;
        let scope = match window {
            Some(id) => self.windows.get(&id).ok_or(TurnError::UnknownWindow(id))?.scope,
            None => self.world.root_scope(),
        };
        let value = self.guarded(|app| {
            let mut cx = app.world.write(scope, window).with_tasks(app.tasks.spawner()).with_commands(&mut app.commands);
            cx.tree = window.and_then(|id| app.windows.get(&id)).map(|window| &window.tree);
            f(&mut cx)
        });
        self.run_turn(None)?;
        Ok(value)
    }

    pub(crate) fn next_request(&mut self) -> u64 {
        self.request_seq = self.request_seq.checked_add(1).expect("platform request identity exhausted");
        self.request_seq
    }
}

impl WriteCx<'_> {
    pub fn request_window(&mut self, options: WindowOptions, root: impl FnOnce(&mut MountCx<'_>) + 'static) -> WindowId {
        let commands = self.commands();
        let id = commands.windows.insert(());
        commands.queued.push_back(AppCommand::Open { id, options: Box::new(options), root: Box::new(root) });
        id
    }

    pub fn close_window(&mut self, id: WindowId) {
        self.commands().queued.push_back(AppCommand::Close(id));
    }

    pub fn set_window_title(&mut self, title: impl Into<Arc<str>>) {
        self.update_window_presentation(PresentationUpdate::Title(title.into()));
    }

    pub fn set_window_theme(&mut self, theme: Theme) {
        self.update_window_presentation(PresentationUpdate::Theme(Box::new(theme)));
    }

    pub fn set_window_material(&mut self, material: WindowMaterial) {
        self.update_window_presentation(PresentationUpdate::Material(material));
    }

    pub fn set_window_presentation(&mut self, presentation: WindowPresentation) {
        self.update_window_presentation(PresentationUpdate::All(Box::new(presentation)));
    }

    fn update_window_presentation(&mut self, update: PresentationUpdate) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::WindowPresentation { window, update });
        }
    }

    /// Set the application's platform badge label, or clear it with `None`.
    pub fn set_app_badge(&mut self, badge: Option<Arc<str>>) {
        self.commands().queued.push_back(AppCommand::SetAppBadge(badge));
    }

    /// Request platform attention for the current window, or the application's active window.
    pub fn request_user_attention(&mut self, attention: UserAttention) {
        let window = self.window_id();
        self.commands().queued.push_back(AppCommand::RequestUserAttention { window, attention });
    }

    pub fn set_window_geometry(&mut self, geometry: WindowGeometryRequest) {
        self.window_control(|window| HostCommand::SetWindowGeometry { window, geometry });
    }

    pub fn focus_window(&mut self, window: WindowId) {
        self.commands().queued.push_back(AppCommand::WindowControl { window, command: HostCommand::FocusWindow { window } });
    }

    pub fn minimize_window(&mut self) {
        self.window_control(|window| HostCommand::MinimizeWindow { window });
    }

    pub fn toggle_maximize_window(&mut self) {
        self.window_control(|window| HostCommand::ToggleMaximizeWindow { window });
    }

    fn window_control(&mut self, command: impl FnOnce(WindowId) -> HostCommand) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::WindowControl { window, command: command(window) });
        }
    }

    pub fn focus(&mut self, test_id: &str) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::Focus { window, test_id: Some(test_id.into()) });
        }
    }

    pub fn blur(&mut self) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::Focus { window, test_id: None });
        }
    }

    /// Queues an undoable edit against the current focused editor revision.
    /// The normal controlled input handler receives the resulting value.
    pub fn replace_editor_text(&mut self, state: EditorState, range: Range<usize>, text: impl Into<String>) -> bool {
        let (Some(window), Some(tree)) = (self.window_id(), self.tree) else { return false };
        let Some(editor) = EditorSession::active(tree) else { return false };
        let value = editor.editor().rope();
        if state.composing
            || editor.state() != Some(state)
            || range.start > range.end
            || !value.is_char_boundary(range.start)
            || !value.is_char_boundary(range.end)
        {
            return false;
        }
        let event = EditorEvent::ProposeEdit {
            session: state.session,
            expected_rev: state.revision,
            edit: TextEdit::Insert { text: text.into(), replacement: Some(range) },
        };
        self.commands().queued.push_back(AppCommand::EditorEdit { window, event });
        true
    }

    pub fn request_command(&mut self, name: &str) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::Command { window, name: name.into(), after_commit: false });
        }
    }

    /// Queues a command for the unique live native view with this test ID in the current window.
    /// A true result means the request was queued. Any result from the view arrives via its native event handler.
    pub fn send_native_command(&mut self, target_test_id: &str, name: impl Into<String>, data: serde_json::Value) -> bool {
        let (Some(window), Some(tree)) = (self.window_id(), self.tree) else { return false };
        let [node] = tree.test_ids(target_test_id) else { return false };
        if !tree.is_live(*node) || !tree.native(*node).is_some_and(|native| native.live) {
            return false;
        }
        let command = NativeCommand { id: NativeId::from(*node), name: name.into(), data };
        self.commands().queued.push_back(AppCommand::NativeCommand { window, command });
        true
    }

    /// Resolves the command after this update's view and focus changes have committed.
    pub fn request_command_after_commit(&mut self, name: &str) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::Command { window, name: name.into(), after_commit: true });
        }
    }

    pub fn scroll_to(&mut self, node: NodeId, offset: Vec2) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::Scroll { window, node, offset });
        }
    }

    /// Brings the bottom of a scroll container into view and resumes its authored tail following.
    pub fn scroll_to_end(&mut self, container: &str) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::ScrollEnd { window, container: container.into() });
        }
    }

    pub fn reveal_row(&mut self, container: &str, key: impl std::fmt::Display) {
        if let Some(window) = self.window_id() {
            self.commands().queued.push_back(AppCommand::Reveal { window, container: container.into(), key: key.to_string() });
        }
    }

    pub fn ask(&mut self, ask: Ask, done: impl FnOnce(&mut WriteCx<'_>, Answer) + 'static) {
        if let Some(window) = self.window_id() {
            let scope = self.scope();
            self.commands().queued.push_back(AppCommand::Ask { window, scope, ask, done: Box::new(done) });
        }
    }

    pub fn request_service(&mut self, request: ServiceRequest, done: impl FnOnce(&mut WriteCx<'_>, ServiceOutcome) + 'static) {
        if let Some(window) = self.window_id() {
            let scope = self.scope();
            self.commands().queued.push_back(AppCommand::Service { window, scope, request, done: Box::new(done) });
        }
    }
}
