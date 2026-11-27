//! Capability contexts for construction, derivation, mutation, and teardown.

mod access;
mod binding;
mod close;
pub use access::{PreparedRead, Readable, Writable};
pub use binding::Binding;
pub use close::CloseHandle;

use octara_model::geometry::Size;
use octara_model::identity::WindowId;
use octara_model::theme::Breakpoint;
use octara_model::window::{SystemPreferences, WindowChrome, WindowGeometry};
use rustc_hash::FxHashMap;
use serde::{Serialize, de::DeserializeOwned};
use smallvec::SmallVec;

use crate::app::AppCommands;
use crate::arena::{NodeKind, ReactiveId, ReactiveWorld, ScopeId, Source};
use crate::authoring::WindowAuthoring;
use crate::graph::value::{ReadSignal, Signal, Trigger, Watch};
use crate::task::TaskSpawner;

/// A tracked read of a reactive computation, binding, or watch source.
///
/// `WRITE_IN_DERIVE`
///
/// ```compile_fail,E0599
/// use octara_runtime::ReactiveWorld;
/// let mut world = ReactiveWorld::new();
/// let mut build = world.build(world.root_scope(), None);
/// let signal = build.signal(0);
/// let _ = build.memo(move |r| r.set(signal, 1));
/// ```
///
/// `VIEW_READS_STATE`
///
/// ```compile_fail,E0599
/// use octara_runtime::Signal;
/// fn project_row(row: &i32, multiplier: Signal<i32>) -> i32 {
///     row * multiplier.get()
/// }
/// ```
pub struct ReadCx<'a> {
    pub(crate) world: &'a mut ReactiveWorld,
    observer: Option<ReactiveId>,
    sources: SmallVec<[Source; 4]>,
    source_indices: Option<FxHashMap<ReactiveId, usize>>,
}

impl<'a> ReadCx<'a> {
    pub(crate) fn new(world: &'a mut ReactiveWorld) -> Self {
        Self { world, observer: None, sources: SmallVec::new(), source_indices: None }
    }

    pub(crate) fn tracked(world: &'a mut ReactiveWorld, observer: ReactiveId) -> Self {
        Self { observer: Some(observer), ..Self::new(world) }
    }

    pub(crate) fn observer(&self) -> Option<ReactiveId> {
        self.observer
    }

    pub(crate) fn into_sources(self) -> SmallVec<[Source; 4]> {
        self.sources
    }

    #[track_caller]
    pub(crate) fn track_id(&mut self, id: ReactiveId) {
        self.world.ensure_clean(id);
        self.record(id, false);
    }

    pub(crate) fn record(&mut self, id: ReactiveId, fixed: bool) {
        if self.observer().is_none() {
            return;
        }
        let existing = if let Some(indices) = &self.source_indices {
            indices.get(&id).copied()
        } else {
            self.sources.iter().position(|source| source.id == id)
        };
        let revision = self.world.revision(id);
        if let Some(index) = existing {
            let fixed = self.sources[index].fixed() || fixed;
            self.sources[index] = Source::new(id, revision, fixed);
            return;
        }
        if let Some(indices) = &mut self.source_indices {
            indices.insert(id, self.sources.len());
        }
        self.sources.push(Source::new(id, revision, fixed));
        if self.sources.len() == 5 {
            self.source_indices = Some(self.sources.iter().enumerate().map(|(index, source)| (source.id, index)).collect());
        }
    }

    /// Subscribe to a payload-free notification.
    #[track_caller]
    pub fn track(&mut self, trigger: Trigger) {
        self.track_id(trigger.id());
    }

    #[track_caller]
    pub fn get<T: Clone + 'static>(&mut self, handle: impl Readable<T>) -> T {
        self.try_get(handle).expect("reactive value is absent")
    }

    /// Read a value that may be absent, such as a removed list row.
    #[track_caller]
    pub fn try_get<T: Clone + 'static>(&mut self, handle: impl Readable<T>) -> Option<T> {
        handle.prepare(self).into_value(self.world)
    }

    #[track_caller]
    pub fn with<T: 'static, R>(&mut self, handle: impl Readable<T>, f: impl FnOnce(&T) -> R) -> R {
        self.try_with(handle, f).expect("reactive value is absent")
    }

    /// Borrow a value if it is present.
    #[track_caller]
    pub fn try_with<T: 'static, R>(&mut self, handle: impl Readable<T>, f: impl FnOnce(&T) -> R) -> Option<R> {
        handle.prepare(self).value(self.world).map(f)
    }

    #[track_caller]
    pub fn with2<A: 'static, B: 'static, R>(&mut self, a: impl Readable<A>, b: impl Readable<B>, f: impl FnOnce(&A, &B) -> R) -> R {
        let a = a.prepare(self);
        let b = b.prepare(self);
        f(a.value(self.world).expect("first reactive value is absent"), b.value(self.world).expect("second reactive value is absent"))
    }

    /// Reads current values without subscribing the surrounding computation.
    pub fn untrack<R>(&mut self, f: impl FnOnce(&mut ReadCx<'_>) -> R) -> R {
        f(&mut ReadCx::new(self.world))
    }
}

/// Construction capabilities for one scope and, when mounting, one window.
///
/// `WRITE_IN_BUILD`
///
/// ```compile_fail,E0599
/// use octara_runtime::ReactiveWorld;
/// let mut world = ReactiveWorld::new();
/// let mut build = world.build(world.root_scope(), None);
/// let signal = build.signal(0);
/// build.set(signal, 1);
/// ```
///
/// `STRICT_READ_UNTRACKED`
///
/// ```compile_fail,E0599
/// use octara_runtime::ReactiveWorld;
/// let mut world = ReactiveWorld::new();
/// let mut build = world.build(world.root_scope(), None);
/// let signal = build.signal(0);
/// let _ = build.get(signal);
/// ```
pub struct BuildCx<'a> {
    pub(crate) world: &'a mut ReactiveWorld,
    pub(crate) scope: ScopeId,
    pub(crate) window: Option<WindowId>,
    pub(crate) tasks: Option<TaskSpawner>,
    viewport: Option<(ReadSignal<Size>, ReadSignal<Breakpoint>)>,
    window_metrics: Option<(ReadSignal<WindowChrome>, ReadSignal<WindowGeometry>)>,
    system_preferences: Option<ReadSignal<SystemPreferences>>,
    pub(crate) authoring: Option<&'a mut WindowAuthoring>,
}

impl<'a> BuildCx<'a> {
    pub(crate) fn new(world: &'a mut ReactiveWorld, scope: ScopeId, window: Option<WindowId>) -> Self {
        Self { world, scope, window, tasks: None, viewport: None, window_metrics: None, system_preferences: None, authoring: None }
    }

    pub fn with_tasks(mut self, tasks: TaskSpawner) -> Self {
        self.tasks = Some(tasks);
        self
    }

    pub(crate) fn with_authoring(mut self, authoring: &'a mut WindowAuthoring) -> Self {
        self.authoring = Some(authoring);
        self
    }

    pub fn with_viewport(mut self, size: ReadSignal<Size>, breakpoint: ReadSignal<Breakpoint>) -> Self {
        self.viewport = Some((size, breakpoint));
        self
    }

    pub(crate) fn with_window_metrics(mut self, chrome: ReadSignal<WindowChrome>, geometry: ReadSignal<WindowGeometry>) -> Self {
        self.window_metrics = Some((chrome, geometry));
        self
    }

    pub(crate) fn with_system_preferences(mut self, preferences: ReadSignal<SystemPreferences>) -> Self {
        self.system_preferences = Some(preferences);
        self
    }

    pub fn scope(&self) -> ScopeId {
        self.scope
    }

    pub fn window_id(&self) -> Option<WindowId> {
        self.window
    }

    #[track_caller]
    pub fn window(&self) -> WindowId {
        self.window.expect("build context has no window")
    }

    #[track_caller]
    pub fn viewport(&self) -> ReadSignal<Size> {
        self.viewport.expect("build context has no viewport").0
    }

    #[track_caller]
    pub fn breakpoint(&self) -> ReadSignal<Breakpoint> {
        self.viewport.expect("build context has no viewport").1
    }

    #[track_caller]
    pub fn window_chrome(&self) -> ReadSignal<WindowChrome> {
        self.window_metrics.expect("build context has no window chrome").0
    }

    #[track_caller]
    pub fn window_geometry(&self) -> ReadSignal<WindowGeometry> {
        self.window_metrics.expect("build context has no window geometry").1
    }

    /// Observe the appearance and accessibility preferences shared by the application's windows.
    #[track_caller]
    pub fn system_preferences(&self) -> ReadSignal<SystemPreferences> {
        self.system_preferences.expect("system preferences require an application build context")
    }

    #[track_caller]
    pub fn signal<T: 'static>(&mut self, init: T) -> Signal<T> {
        Signal::new(self.world.alloc_value(NodeKind::ValueSource, self.scope, self.window, init))
    }

    #[track_caller]
    pub fn persisted<T: Serialize + DeserializeOwned + 'static>(&mut self, key: &'static str, init: impl FnOnce() -> T) -> Signal<T> {
        Signal::new(self.world.persisted(self.scope, self.window, key, init))
    }

    /// Derives a value from tracked reads. The computation can retain a private cache.
    #[track_caller]
    pub fn memo<T: PartialEq + 'static>(&mut self, f: impl FnMut(&mut ReadCx<'_>) -> T + 'static) -> ReadSignal<T> {
        self.world.memo(self.scope, self.window, f)
    }

    /// Derive a value by reusing its existing storage. Return true to publish a change.
    #[track_caller]
    pub fn memo_with<T: 'static>(&mut self, init: T, compute: impl FnMut(&mut ReadCx<'_>, &mut T) -> bool + 'static) -> ReadSignal<T> {
        self.world.memo_with(self.scope, self.window, init, compute)
    }

    /// Create a scope-owned notification with no value slot.
    #[track_caller]
    pub fn trigger(&mut self) -> Trigger {
        Trigger::new(self.world.alloc_node(NodeKind::ValueSource, self.scope, self.window))
    }

    /// Runs an effect after a tracked value changes. Both callbacks can retain private state.
    #[track_caller]
    pub fn watch<T: PartialEq + 'static>(
        &mut self,
        source: impl FnMut(&mut ReadCx<'_>) -> T + 'static,
        effect: impl FnMut(&mut WriteCx<'_>, &T, Option<&T>) + 'static,
    ) -> Watch {
        self.world.watch(self.scope, self.window, source, effect)
    }

    #[track_caller]
    pub fn provide<T: Clone + 'static>(&mut self, value: T) {
        self.world.provide(self.scope, value);
    }

    #[track_caller]
    pub fn context<T: Clone + 'static>(&self) -> Option<T> {
        self.world.context(self.scope)
    }

    /// Read current state once during construction without creating an observer.
    pub fn read<R>(&mut self, sample: impl FnOnce(&mut ReadCx<'_>) -> R) -> R {
        sample(&mut self.world.read())
    }

    #[track_caller]
    pub fn sample<T: Clone + 'static>(&mut self, handle: impl Readable<T>) -> T {
        self.read(|read| read.get(handle))
    }

    #[track_caller]
    pub fn on_cleanup(&mut self, f: impl FnOnce(&mut CleanupCx<'_>) + 'static) {
        self.world.on_cleanup(self.scope, Box::new(f));
    }

    #[track_caller]
    pub fn create_observer(&mut self, kind: NodeKind) -> ReactiveId {
        self.world.create_observer(kind, self.scope, self.window)
    }
}

/// Synchronous mutation capabilities for handlers and tasks.
///
/// `SIGNAL_BORROW`
///
/// ```compile_fail,E0499
/// use octara_runtime::ReactiveWorld;
/// let mut world = ReactiveWorld::new();
/// let scope = world.root_scope();
/// let signal = world.build(scope, None).signal(0);
/// let mut write = world.write(scope, None);
/// write.update(signal, |value| {
///     write.set(signal, *value + 1);
/// });
/// ```
pub struct WriteCx<'a> {
    pub(crate) world: &'a mut ReactiveWorld,
    pub(crate) scope: ScopeId,
    pub(crate) window: Option<WindowId>,
    pub(crate) commands: Option<&'a mut AppCommands>,
    pub(crate) tasks: Option<TaskSpawner>,
    pub(crate) tree: Option<&'a octara_tree::Tree>,
}

impl<'a> WriteCx<'a> {
    pub(crate) fn new(world: &'a mut ReactiveWorld, scope: ScopeId, window: Option<WindowId>) -> Self {
        Self { world, scope, window, commands: None, tasks: None, tree: None }
    }

    pub fn with_tasks(mut self, tasks: TaskSpawner) -> Self {
        self.tasks = Some(tasks);
        self
    }

    pub fn with_commands(mut self, commands: &'a mut AppCommands) -> Self {
        self.commands = Some(commands);
        self
    }

    pub(crate) fn commands(&mut self) -> &mut AppCommands {
        self.commands.as_deref_mut().expect("write context has no application commands")
    }

    /// Whether a previously captured scope still owns mounted resources.
    pub fn is_scope_alive(&self, scope: ScopeId) -> bool {
        self.world.is_scope_alive(scope)
    }

    /// Queries a mounted name in the current window during event handling.
    /// An unmounted name or a context without a window tree returns `None`.
    pub fn named_interactive(&self, name: &str) -> Option<bool> {
        let tree = self.tree?;
        match tree.named_nodes(name) {
            [] => None,
            [node] => Some(tree.is_interactive(*node)),
            _ => Some(false),
        }
    }

    pub fn scope(&self) -> ScopeId {
        self.scope
    }

    pub fn window_id(&self) -> Option<WindowId> {
        self.window
    }

    #[track_caller]
    pub fn get<T: Clone + 'static>(&mut self, handle: impl Readable<T>) -> T {
        self.world.read().get(handle)
    }

    #[track_caller]
    pub fn with<T: 'static, R>(&mut self, handle: impl Readable<T>, f: impl FnOnce(&T) -> R) -> R {
        self.world.read().with(handle, f)
    }

    #[track_caller]
    pub fn try_get<T: Clone + 'static>(&mut self, handle: impl Readable<T>) -> Option<T> {
        self.world.read().try_get(handle)
    }

    #[track_caller]
    pub fn try_with<T: 'static, R>(&mut self, handle: impl Readable<T>, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.world.read().try_with(handle, f)
    }

    #[track_caller]
    pub fn with2<A: 'static, B: 'static, R>(&mut self, a: impl Readable<A>, b: impl Readable<B>, f: impl FnOnce(&A, &B) -> R) -> R {
        self.world.read().with2(a, b, f)
    }

    /// Evaluate reads without creating an observer.
    pub fn read<R>(&mut self, f: impl FnOnce(&mut ReadCx<'_>) -> R) -> R {
        f(&mut self.world.read())
    }

    #[track_caller]
    pub fn set<T: 'static>(&mut self, target: impl Writable<T>, value: T) {
        target.write(self, value);
    }

    #[track_caller]
    pub fn set_if_changed<T: PartialEq + 'static>(&mut self, target: impl Writable<T>, value: T) -> bool {
        target.write_if_changed(self, value)
    }

    #[track_caller]
    pub fn update<T: 'static, R>(&mut self, target: impl Writable<T>, f: impl FnOnce(&mut T) -> R) -> R {
        target.edit(self, |value| (f(value), true))
    }

    /// Edit a value and notify its observers only when the edit returns true.
    /// The callback must leave the observable value unchanged when it returns false.
    #[track_caller]
    pub fn mutate<T: 'static>(&mut self, target: impl Writable<T>, f: impl FnOnce(&mut T) -> bool) -> bool {
        target.edit(self, |value| {
            let changed = f(value);
            (changed, changed)
        })
    }

    /// Notify computations tracking this payload-free trigger.
    #[track_caller]
    pub fn notify(&mut self, trigger: Trigger) {
        self.world.mark_changed(trigger.id());
    }
}

/// Untracked observation during scope teardown.
///
/// `REGISTER_IN_CLEANUP`
///
/// ```compile_fail,E0599
/// use octara_runtime::ReactiveWorld;
/// let mut world = ReactiveWorld::new();
/// world.build(world.root_scope(), None).on_cleanup(|cx| {
///     let _ = cx.signal(0);
/// });
/// ```
pub struct CleanupCx<'a> {
    world: &'a mut ReactiveWorld,
}

impl<'a> CleanupCx<'a> {
    pub(crate) fn new(world: &'a mut ReactiveWorld) -> Self {
        Self { world }
    }

    #[track_caller]
    pub fn sample<T: Clone + 'static>(&mut self, handle: impl Readable<T>) -> T {
        self.world.read().get(handle)
    }
}
