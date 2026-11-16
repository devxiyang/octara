//! The app runtime: mounts a root view, dispatches events, lays out, and rebuilds for hot reload.

mod asks;
mod behavior;
mod drive;
mod edit;
mod event;
mod focus;
mod frame;
mod guard;
mod input;
mod keys;
mod pick;
mod pointer;
mod presentation;
mod selection;
mod text_input;
mod time;
mod tooltip;

pub use asks::menu_surface;
pub use edit::{TextField, byte_offset, utf16_offset};
pub use event::{FileDrag, InputEvent, Recorded, Recording};
pub use guard::TargetGuard;
pub use keys::{BUILTIN_COMMANDS, Binding, Chord, Key, KeyEvent, Keymap};
pub use pointer::{Button, Drag, DragPhase, Modifiers, Phase, PointerEvent, PointerKind, Tuning};
pub(crate) use selection::ListOrder;
pub use selection::TextPoint;
pub(crate) use text_input::Composition;
pub use text_input::{TextDocument, TextEdit};

use crate::diagnostics::{self as codes, Diagnostic};
use crate::kernel::{Breakpoint, HandlerKind, Kernel, NodeId, State, Theme};
use crate::reactive::graph::{Ctx, Owner, Runtime, ScopeId};
use crate::reactive::graph::{Signal, untrack};
use crate::view::{Handler, Parent, ScopedHandler, Slots, View, mount};
use std::cell::{Cell, Ref, RefCell};
use std::rc::Rc;

/// The window size an app starts with; change it with [`Window::resize`].
pub const DEFAULT_VIEWPORT: (f32, f32) = (800.0, 600.0);

/// An input event, addressed by test id (or by name for external events).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Press(String),
    Input(String, String),
    /// A finite number set by test id, through the control's `on_numeric_input` handler.
    NumericInput(String, f64),
    Toggle(String),
    /// The pointer entered (`true`) or left (`false`) the element.
    Hover(String, bool),
    /// Keyboard focus moved to the element; it must be `focusable` and not disabled.
    Focus(String),
    /// Keyboard focus left the focused element.
    Blur,
    /// Scrolls a scroll container by `(dx, dy)`, like a wheel or trackpad.
    Scroll(String, f32, f32),
    /// Scrolls a scroll container to `(x, y)` (CSS `scrollTo`), clamped to its range.
    ScrollTo(String, f32, f32),
    /// Text sent to the channel named by `Receiver::named` (as `App::send`): data arriving
    /// from outside.
    Send(String, String),
}

/// The platform an app runs on, for its conventions: the shortcut modifier (Command on Apple
/// platforms, Control elsewhere), Control-click as a right click on macOS.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetPlatform {
    MacOs,
    Ios,
    Windows,
    Linux,
    Android,
}

impl TargetPlatform {
    /// The platform this build targets.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            TargetPlatform::MacOs
        } else if cfg!(target_os = "ios") {
            TargetPlatform::Ios
        } else if cfg!(target_os = "windows") {
            TargetPlatform::Windows
        } else if cfg!(target_os = "android") {
            TargetPlatform::Android
        } else {
            TargetPlatform::Linux
        }
    }
    pub fn is_apple(self) -> bool {
        matches!(self, TargetPlatform::MacOs | TargetPlatform::Ios)
    }
    /// Input is a finger: controls take their touch sizes (`When::Touch`), and `octara check`
    /// asks 44 × 44 targets.
    pub fn is_touch(self) -> bool {
        matches!(self, TargetPlatform::Ios | TargetPlatform::Android)
    }
}

/// How the user last gave input: focus rings show only after the keyboard (Rune, GPUI).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Modality {
    #[default]
    Pointer,
    Keyboard,
}

/// Work the runtime did, beyond what the kernel counts.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Work {
    /// Bindings, memos and watches run.
    pub closures_run: u64,
    /// Live reactive nodes.
    pub reactive_nodes: u64,
}

slotmap::new_key_type! {
    /// A window's identity while it is open.
    pub struct WindowId;
}

impl WindowId {
    /// The number agents and the inspector protocol name it by.
    pub fn number(self) -> u64 {
        slotmap::Key::data(&self).as_ffi()
    }
    /// The window `number` names.
    pub fn from_number(number: u64) -> WindowId {
        slotmap::KeyData::from_ffi(number).into()
    }
    /// The reactive owner code of this window runs as.
    fn owner(self) -> Owner {
        Owner(slotmap::Key::data(&self).as_ffi())
    }
    fn of(owner: Owner) -> WindowId {
        slotmap::KeyData::from_ffi(owner.0).into()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Teardown {
    Pending,
    Running,
    Complete,
}

/// What the whole app shares ([design](https://github.com/octoii-works/octara/blob/main/docs/design/core.md)):
/// the reactive runtime, the scheduler and its clock, settings and the platform, and the
/// windows.
pub(crate) struct AppState {
    /// A process-random namespace and local App incarnation, shared by its window identities.
    namespace: u128,
    pub(crate) rt: Rc<Runtime>,
    root_scope: ScopeId,
    /// A one-way gate: teardown may still run internally, but public work cannot restart.
    shutdown: Cell<bool>,
    teardown: Cell<Teardown>,
    failed: Cell<bool>,
    /// Tasks, timers and channels, and the clock: one for all the windows.
    pub(crate) sched: Rc<crate::task::Scheduler>,
    /// The theme, the text system and input conventions every window's kernel reads.
    pub(crate) res: Rc<crate::kernel::Resources>,
    windows: RefCell<slotmap::SlotMap<WindowId, Rc<WindowState>>>,
    /// The open windows, in the order they opened.
    order: RefCell<Vec<WindowId>>,
    /// Windows opened from app code, mounted at the end of the frame; windows to close then.
    opening: RefCell<Vec<WindowId>>,
    closing: RefCell<Vec<WindowId>>,
    next_cause: Cell<u64>,
    tuning: Cell<Tuning>,
    platform: Cell<TargetPlatform>,
    /// The app's own bindings (after the platform's standard ones).
    pub(crate) keymap: RefCell<keys::Keymap>,
    /// The platform's standard bindings then the app's, built once until either changes.
    keymap_full: RefCell<Option<(TargetPlatform, Rc<keys::Keymap>)>>,
    /// The system the app runs on (clipboard, waking the loop, windows).
    pub(crate) system: RefCell<Box<dyn crate::platform::Platform>>,
    /// The app's menus as declared (`set_menus`), as last presented to the platform (each
    /// item's state and keys filled in), and the active window (whose commands they run).
    menus: RefCell<Vec<crate::menu::Menu>>,
    menus_presented: RefCell<Option<Vec<crate::menu::Menu>>>,
    active: Cell<Option<WindowId>>,
    /// What paints `capture` requests (the harness installs the reference renderer).
    painter: RefCell<Option<Box<dyn crate::capture::Painter>>>,
    /// How deep the app's entry points are nested: an operation ends when it returns to 0.
    depth: Cell<u32>,
    /// Check every window against a from-scratch rebuild after each operation (tests).
    check: Cell<bool>,
    /// A check is running (its rebuild is an operation too).
    checking: Cell<bool>,
    /// The inputs recorded, while recording.
    recording: RefCell<Option<Recording>>,
}

/// What is one window's own: its kernel and view context (handlers, commands, overlay slots,
/// followed scroll states and sizes, after-layout hooks), its root view and scope, interaction
/// state, size and the signals derived from it, its platform window and what it was last sent,
/// and a capture in progress.
pub(crate) struct WindowState {
    pub(crate) id: WindowId,
    pub(crate) kernel: RefCell<Kernel>,
    pub(crate) handlers: RefCell<rustc_hash::FxHashMap<u64, ScopedHandler>>,
    /// The window's command handlers (`on_command`), in registration order.
    pub(crate) commands: RefCell<Vec<(String, u64)>>,
    /// Regions of the kernel's overlay layer, one per `overlay` of a mounted element.
    pub(crate) overlay_slots: Slots,
    pub(crate) viewport: Signal<(f32, f32)>,
    pub(crate) breakpoint: Signal<Breakpoint>,
    /// Scroll containers someone follows (virtual lists), with their state as a signal.
    pub(crate) scrolls: RefCell<rustc_hash::FxHashMap<NodeId, Signal<crate::view::ScrollState>>>,
    /// Element sizes someone follows (`size_of`), by test id.
    pub(crate) sizes: RefCell<rustc_hash::FxHashMap<String, Signal<(f32, f32)>>>,
    /// Whether scroll containers follow their tail, as someone reads it (`following_tail`), by
    /// test id.
    pub(crate) tails: RefCell<rustc_hash::FxHashMap<String, Signal<bool>>>,
    /// Clocks someone reads (`clock`), by period in nanoseconds.
    pub(crate) clocks: RefCell<rustc_hash::FxHashMap<u128, Clock>>,
    /// Paced texts (`paced`) in this window, by number.
    pub(crate) paced: RefCell<Vec<(u64, Rc<crate::view::Paced>)>>,
    /// Run after each layout; return whether they changed anything (then the window lays out again).
    pub(crate) after_layout: RefCell<Vec<(u64, crate::view::AfterLayout)>>,
    pub(crate) next: Cell<u64>,
    options: crate::platform::WindowOptions,
    /// Closed: its scope is disposed and it left the app.
    closed: Cell<bool>,
    root: Rc<dyn Fn() -> View>,
    root_scope: Cell<Option<ScopeId>>,
    root_slots: Slots,
    interaction: RefCell<Interaction>,
    /// Invalidates virtual residency when the actual focus or pointer owner changes.
    pub(crate) interaction_revision: Signal<u64>,
    size: Cell<(f32, f32)>,
    /// Device pixels per logical pixel.
    scale: Cell<f32>,
    /// The window on the system: its native views and text input (headless: a recording host).
    platform: RefCell<Box<dyn crate::platform::PlatformWindow>>,
    /// The text input state last sent to it.
    text_input: RefCell<Option<crate::platform::TextInput>>,
    /// The pointer's look last presented.
    cursor: Cell<crate::platform::CursorStyle>,
    /// What the platform was last told of each native view: its props version and placement.
    native_sent: RefCell<rustc_hash::FxHashMap<NodeId, (u64, Option<crate::native::Placement>)>>,
    /// Native changes not presented yet.
    native_changes: RefCell<Vec<crate::native::NativeChange>>,
    /// The paint generation last presented.
    presented: Cell<u64>,
    /// The kernel generation last laid out and presented, before final focus and hover cleanup.
    settled_generation: Cell<Option<u64>>,
    /// Frames presented to the platform window.
    frames: Cell<u64>,
    /// A capture in progress: the snapshots asked for.
    capturing: RefCell<Option<crate::capture::Capturing>>,
    /// Picking an element to hand to an agent.
    picking: RefCell<pick::Picking>,
    /// The document selection (a selectable region's), and the texts it marked last.
    selection: RefCell<Option<selection::DocSelection>>,
    selected_texts: RefCell<Vec<NodeId>>,
    /// The region whose selection assistive technology was given.
    selection_region: Cell<Option<NodeId>>,
    /// Requests waiting for an answer, those to present, those withdrawn, the next number.
    asks: RefCell<Vec<asks::Pending>>,
    asks_new: RefCell<Vec<(crate::ask::AskId, crate::ask::Ask)>>,
    asks_withdrawn: RefCell<Vec<crate::ask::AskId>>,
    ask_next: Cell<crate::ask::AskId>,
    /// The requests the window draws itself: its asks layer's menus and prompts.
    drawn: RefCell<Option<asks::Drawn>>,
    /// The tooltip the window draws (one at most).
    tip: RefCell<Option<crate::reactive::list::StateList<u64, tooltip::Shown>>>,
    /// How lists order their rows, by container (for selections through them).
    pub(crate) lists: RefCell<std::collections::HashMap<NodeId, selection::ListOrder>>,
}

thread_local! {
    static APP: RefCell<Option<Rc<AppState>>> = const { RefCell::new(None) };
}

/// The app whose code is running (panics outside one).
pub(crate) fn app_state() -> Rc<AppState> {
    APP.with(|a| a.borrow().clone()).expect("no app: call inside an app's frame")
}

/// The window whose code is running: the reactive owner of the running binding, watch, task or
/// handler (panics outside one).
pub(crate) fn window() -> Rc<WindowState> {
    let owner = crate::reactive::graph::current_owner().expect("no window: this runs only inside a window's code");
    let app = app_state();
    let w = app.windows.borrow().get(WindowId::of(owner)).cloned();
    w.expect("the window this code belongs to is closed")
}

/// A handle on the window whose code is running.
pub(crate) fn current() -> Window {
    Window { app: app_state(), w: window() }
}

fn enter_app<R>(app: &Rc<AppState>, f: impl FnOnce() -> R) -> R {
    app.assert_usable();
    enter_app_context(app, || {
        if !app.shutdown.get() {
            app.sched.resume_background_failure();
        }
        f()
    })
}

fn enter_app_context<R>(app: &Rc<AppState>, f: impl FnOnce() -> R) -> R {
    let _failure = std::mem::DropGuard::new(app, |app| {
        if std::thread::panicking() && !app.failed.replace(true) {
            app.rt.diag(
                &codes::APP_FAILED,
                "panic escaped an App operation; shut down this App and create a new one".into(),
                std::panic::Location::caller(),
            );
            for window in app.windows.borrow().values() {
                window.settled_generation.set(None);
            }
        }
    });
    if app.depth.get() == 0 && !app.checking.get() {
        app.sched.begin_turn();
        app.rt.resume_propagation();
    }
    struct Restore(Option<Rc<AppState>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            APP.with(|a| *a.borrow_mut() = prev);
        }
    }
    struct Depth<'a>(&'a Cell<u32>);
    impl Drop for Depth<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }
    let r = {
        let _restore = Restore(APP.with(|a| a.replace(Some(app.clone()))));
        app.depth.set(app.depth.get() + 1);
        let _depth = Depth(&app.depth);
        app.rt.enter(f)
    };
    if app.depth.get() == 0 && !app.failed.get() {
        app.sched.retire_names();
        app.check_consistency();
        app.sched.finish_turn();
    }
    r
}

/// An app: one reactive runtime and clock, its settings and platform, and its windows. Cloning
/// it gives another handle on the same app.
#[derive(Clone)]
pub struct App {
    app: Rc<AppState>,
}

/// One window of an app: drive it (pointer, keys, text, test ids), read what it shows (the
/// display list, the semantic tree, the kernel). Cloning it gives another handle.
#[derive(Clone)]
pub struct Window {
    app: Rc<AppState>,
    w: Rc<WindowState>,
}

/// One bounded update attempt. An incomplete window stays pending, but is not retried
/// inside this same App frame merely because another window changed.
#[derive(Clone, Copy)]
enum WindowUpdate {
    Ready(u64),
    Incomplete,
}

impl WindowUpdate {
    fn invalidated(self, generation: u64) -> bool {
        matches!(self, Self::Ready(previous) if previous != generation)
    }
}

impl AppState {
    /// Admission for fallible external operations, before allocating or delivering work.
    pub(crate) fn check_running(&self) -> Result<(), String> {
        if self.failed.get() {
            return Err(format!("[{}] App failed after a panic; shut it down", codes::APP_FAILED));
        }
        if self.shutdown.get() {
            return Err("App has shut down".into());
        }
        Ok(())
    }

    pub(crate) fn assert_usable(&self) {
        assert!(!self.failed.get() || self.shutdown.get(), "[{}] App failed after a panic; shut it down", codes::APP_FAILED);
    }

    /// The keymap in use: the platform's standard bindings, then the app's (cached).
    pub(crate) fn full_keymap(&self) -> Rc<keys::Keymap> {
        let platform = self.platform.get();
        if let Some((p, k)) = self.keymap_full.borrow().as_ref()
            && *p == platform
        {
            return k.clone();
        }
        let mut k = keys::Keymap::standard(platform);
        k.bindings.extend(self.keymap.borrow().bindings.iter().cloned());
        let k = Rc::new(k);
        *self.keymap_full.borrow_mut() = Some((platform, k.clone()));
        k
    }
    /// The app's bindings changed: the keymap in use is built again when next read.
    pub(crate) fn keymap_changed(&self) {
        self.keymap_full.borrow_mut().take();
    }
}

impl WindowState {
    pub(crate) fn interaction_changed(&self) {
        if self.closed.get() {
            return;
        }
        self.interaction_revision.set_internal(untrack(|| self.interaction_revision.get()).wrapping_add(1));
    }

    /// Current interaction allocations, including callbacks finishing a captured pointer.
    /// The interaction state remains their only owner; lists retain no separate pin registry.
    pub(crate) fn interaction_nodes(&self) -> Vec<NodeId> {
        let interaction = self.interaction.borrow();
        let mut nodes: Vec<_> = interaction.focused.into_iter().collect();
        interaction.pointer.resident_nodes(&mut nodes);
        nodes
    }

    /// Scope teardown cancels ownership without invoking callbacks of the retired scope.
    pub(crate) fn retire_interactions(&self, node: NodeId) {
        let mut kernel = self.kernel.borrow_mut();
        let within = |target| kernel.is_in_interaction_scope(target, node);
        let mut interaction = self.interaction.borrow_mut();
        let focused = interaction.focused.filter(|&target| within(target));
        if focused.is_some() {
            interaction.focused = None;
        }
        if interaction.reveal.is_some_and(within) {
            interaction.reveal = None;
        }
        interaction.edits.retain(|&target, _| !within(target));
        let held = interaction.pointer.retire(&within);
        let changed = focused.is_some() || held.is_some();
        if let Some(target) = focused.filter(|&target| kernel.is_alive(target)) {
            kernel.set_preedit(target, "");
            kernel.set_state(target, State::FOCUSED | State::FOCUS_VISIBLE, false);
        }
        if let Some(held) = held {
            held.unpress(&mut kernel);
        }
        drop((interaction, kernel));
        if changed {
            self.interaction_changed();
        }
    }

    pub(crate) fn interaction_request_command(&self, command: &str, at: &'static std::panic::Location<'static>) {
        self.interaction.borrow_mut().after_update.push_back((command.to_owned(), at));
    }

    pub(crate) fn interaction_request_focus(&self, id: &str) {
        self.interaction.borrow_mut().focus_request = Some(id.to_owned());
    }

    pub(crate) fn interaction_request_scroll_into_view(&self, id: &str) {
        self.interaction.borrow_mut().reveal_request = Some(id.to_owned());
    }
}

/// A clock someone reads: its signal, the timer moving it, and how many scopes read it.
pub(crate) struct Clock {
    pub(crate) signal: Signal<std::time::Duration>,
    pub(crate) task: crate::task::Task,
    pub(crate) readers: usize,
}

/// Runtime-owned interaction states. A hot reload carries them over by test id.
#[derive(Default)]
struct Interaction {
    /// The node under the mouse; it and its ancestors (`hover_chain`) are hovered.
    hovered: Option<NodeId>,
    /// What files dragged from the system are over (it takes them: `on_drop`).
    drop_target: Option<NodeId>,
    hover_chain: Vec<NodeId>,
    /// Hover held still while layout moved under a still pointer (Rune).
    hover_held: bool,
    focused: Option<NodeId>,
    modality: Modality,
    pointer: pointer::PointerState,
    /// Open modals, outermost first.
    modals: Vec<focus::OpenModal>,
    /// Text fields' editing history.
    edits: rustc_hash::FxHashMap<NodeId, edit::EditState>,
    /// Test ids of the hovered and focused elements across a hot reload.
    reload: (Option<crate::kernel::Address>, Option<crate::kernel::Address>),
    /// Modal identities and their original focus targets across a hot reload.
    reload_modals: Vec<focus::SavedModal>,
    forced: Vec<(String, State, bool)>,
    /// Scroll offsets by test id, carried over a hot reload.
    scrolls: Vec<(String, (f32, f32))>,
    /// Containers following their tail that had let go of it, by test id.
    released: Vec<String>,
    /// A text field whose caret moved: scroll containers around it bring the caret into view
    /// once the frame is laid out.
    reveal: Option<NodeId>,
    /// An element requested by test id, resolved after this frame's content is mounted.
    reveal_request: Option<String>,
    /// The window's tooltip.
    tip: tooltip::Tip,
    /// The test id of an element the app asked to focus (`focus`), once the frame is built.
    focus_request: Option<String>,
    /// Commands routed after updates, modal restoration and explicit focus requests settle.
    after_update: std::collections::VecDeque<(String, &'static std::panic::Location<'static>)>,
}

impl WindowState {
    fn new(
        app: &AppState,
        id: WindowId,
        options: crate::platform::WindowOptions,
        root: Rc<dyn Fn() -> View>,
        platform: Box<dyn crate::platform::PlatformWindow>,
    ) -> WindowState {
        let size = options.size;
        let mut kernel = Kernel::new(app.res.clone());
        kernel.bind_identity(app.namespace, id.number());
        WindowState {
            id,
            kernel: RefCell::new(kernel),
            handlers: RefCell::default(),
            commands: RefCell::default(),
            overlay_slots: Slots::default(),
            viewport: app.rt.enter(|| Signal::detached(size)),
            breakpoint: app.rt.enter(|| Signal::detached(Theme::default().breakpoint_of(size.0))),
            scrolls: RefCell::default(),
            sizes: RefCell::default(),
            tails: RefCell::default(),
            clocks: RefCell::default(),
            paced: RefCell::default(),
            after_layout: RefCell::default(),
            next: Cell::new(0),
            options,
            closed: Cell::new(false),
            root,
            root_scope: Cell::new(None),
            root_slots: Slots::default(),
            interaction: RefCell::default(),
            interaction_revision: app.rt.enter(|| Signal::detached(0)),
            size: Cell::new(size),
            scale: Cell::new(1.0),
            platform: RefCell::new(platform),
            text_input: RefCell::new(None),
            cursor: Cell::new(crate::platform::CursorStyle::Arrow),
            native_sent: RefCell::default(),
            native_changes: RefCell::default(),
            presented: Cell::new(u64::MAX),
            settled_generation: Cell::new(None),
            frames: Cell::new(0),
            capturing: RefCell::default(),
            picking: RefCell::default(),
            selection: RefCell::default(),
            selected_texts: RefCell::default(),
            selection_region: Cell::new(None),
            asks: RefCell::default(),
            asks_new: RefCell::default(),
            asks_withdrawn: RefCell::default(),
            ask_next: Cell::new(1),
            drawn: RefCell::new(None),
            tip: RefCell::new(None),
            lists: RefCell::default(),
        }
    }
}

/// The offset a scroll event starts from: the current one for `Scroll`, the origin for `ScrollTo`.
fn n_offset(k: &Kernel, n: crate::kernel::NodeId, relative: bool) -> (f32, f32) {
    if relative { k.scroll_offset(n) } else { (0.0, 0.0) }
}

/// States the runtime drives; `SELECTED` and `DISABLED` come from element props.
const RUNTIME_STATES: State = State::HOVER.union(State::FOCUSED).union(State::FOCUS_VISIBLE).union(State::PRESSED).union(State::DRAG_OVER);

impl App {
    /// The app whose code is running. Panics outside an app entry.
    /// Use `App::current().run(..)` for work that must outlive the initiating UI scope;
    /// it has no current window, and its domain owner must cancel it when no longer needed.
    pub fn current() -> Self {
        Self { app: app_state() }
    }

    /// An app on `platform` (a backend's; `HeadlessPlatform` for tests and agents), with no
    /// window yet.
    pub fn new(platform: impl crate::platform::Platform + 'static) -> App {
        let rt = Runtime::new();
        let root_scope = rt.create_app_scope();
        let state = AppState {
            namespace: crate::identity::new_namespace(),
            rt,
            root_scope,
            shutdown: Cell::new(false),
            teardown: Cell::new(Teardown::Pending),
            failed: Cell::new(false),
            sched: Rc::new(crate::task::Scheduler::default()),
            res: Rc::default(),
            windows: RefCell::default(),
            order: RefCell::default(),
            opening: RefCell::default(),
            closing: RefCell::default(),
            next_cause: Cell::new(0),
            tuning: Cell::new(Tuning::default()),
            platform: Cell::new(TargetPlatform::current()),
            keymap: RefCell::new(keys::Keymap::default()),
            keymap_full: RefCell::new(None),
            system: RefCell::new(Box::new(platform)),
            menus: RefCell::new(vec![crate::menu::Menu::edit()]),
            menus_presented: RefCell::new(None),
            active: Cell::new(None),
            painter: RefCell::default(),
            depth: Cell::new(0),
            check: Cell::new(false),
            checking: Cell::new(false),
            recording: RefCell::default(),
        };
        if let Some(wake) = state.system.borrow().waker() {
            state.sched.set_waker(wake);
        }
        App { app: Rc::new(state) }
    }

    /// Opens a window showing `root`, built and laid out now.
    pub fn open_window(&self, options: crate::platform::WindowOptions, root: impl Fn() -> View + 'static) -> Result<Window, String> {
        let id = self.app.insert_window(options, Rc::new(root))?;
        let w = self.app.handle(id).expect("just opened");
        w.enter(|| w.mount_root());
        Ok(w)
    }

    /// Window `id`, while it is open.
    pub fn window(&self, id: WindowId) -> Option<Window> {
        self.app.handle(id)
    }

    /// The open windows, in the order they opened.
    pub fn windows(&self) -> Vec<WindowId> {
        self.app.order.borrow().clone()
    }

    /// Closes window `id` now: its scope (state, watches, tasks) is disposed, its platform
    /// window let go, and the other windows settle.
    pub fn close_window(&self, id: WindowId) {
        if self.app.shutdown.get() {
            return;
        }
        self.app.closing.borrow_mut().push(id);
        self.app.frame(std::panic::Location::caller());
    }

    /// Ends this App, even when other handles remain. Closes windows, runs scope cleanup and
    /// cancels tasks. Completed shutdown is idempotent; interrupted teardown can be resumed.
    /// Closing the last window alone does not end the App.
    /// Hosts must call this before exiting; dropping Rc handles is not a shutdown protocol.
    /// A user cleanup panic propagates and can interrupt resource release. The entry gate
    /// remains closed. A later shutdown resumes unstarted teardown work without replaying
    /// a cleanup callback that already began.
    #[track_caller]
    pub fn shutdown(&self) {
        let at = std::panic::Location::caller();
        enter_app_context(&self.app, || {
            if self.app.teardown.get() != Teardown::Pending {
                return;
            }
            if !self.app.rt.write_allowed(at) || !self.app.rt.effect_allowed(at) {
                return;
            }
            self.app.shutdown.set(true);
            self.app.teardown.set(Teardown::Running);
            let _progress = std::mem::DropGuard::new(&self.app.teardown, |progress| {
                if progress.get() == Teardown::Running {
                    progress.set(Teardown::Pending);
                }
            });
            self.app.rt.set_reloading(false);
            // Revoke task execution before user cleanup. If a future destructor panics,
            // the closed scheduler retains its remaining slots for the next shutdown.
            self.app.sched.shutdown();
            let windows = self.app.order.borrow().clone();
            for id in windows {
                self.app.close(id);
            }
            self.app.opening.borrow_mut().clear();
            self.app.closing.borrow_mut().clear();
            self.app.rt.shutdown();
            self.app.teardown.set(Teardown::Complete);
        });
    }

    /// A panic escaped an App operation. Ordinary execution is rejected; shutdown and
    /// diagnostic reads remain available. User state is not rolled back.
    pub fn is_failed(&self) -> bool {
        self.app.failed.get()
    }

    /// Whether execution is permanently stopped. If cleanup was interrupted, `pending()`
    /// reports unfinished teardown and another `shutdown()` resumes it.
    pub fn is_shutdown(&self) -> bool {
        self.app.shutdown.get()
    }

    /// Runs `f` as the app (not as one of its windows).
    pub(crate) fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        enter_app(&self.app, f)
    }

    /// Checks every window against a from-scratch rebuild after each operation (an event, a
    /// clock step, a theme change): the tree an operation updated incrementally must equal the
    /// tree built again from the same state, as a hot reload builds it (self-adjusting
    /// computation, Nominal Adapton). A difference panics with both trees. The rebuild drops
    /// what a hot reload drops (undo history, the modal stack): for tests of the runtime itself.
    pub fn set_check_consistency(&self, on: bool) {
        self.app.check.set(on);
    }

    /// Runs `f` in the App root scope, without a current window, then settles its windows.
    /// Named state persists across calls; tasks and cleanup belong to this root until shutdown.
    /// Preserves the caller's execution phase: a pure computation gains no write permission.
    /// Panics after shutdown, without calling `f`.
    #[track_caller]
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        let at = std::panic::Location::caller();
        self.enter(|| {
            if self.app.shutdown.get() {
                self.app.rt.diag(&codes::WRITE_DISPOSED, "cannot enter a shut down App".into(), at);
                panic!("[{}] App has shut down", codes::WRITE_DISPOSED);
            }
            let r = self.app.rt.with_owner(None, || self.app.rt.with_scope(Some(self.app.root_scope), f));
            // A nested call belongs to its enclosing operation; in particular, a memo or
            // binding must finish computing before that operation flushes and lays out.
            if self.app.depth.get() == 1 {
                self.app.frame(std::panic::Location::caller());
            }
            r
        })
    }
}

impl AppState {
    pub(crate) fn set_menus(self: &Rc<Self>, menus: Vec<crate::menu::Menu>) {
        *self.menus.borrow_mut() = menus;
    }

    /// The window whose commands the menus run: the active one, else the first.
    pub(crate) fn command_window(self: &Rc<Self>) -> Option<Window> {
        self.active.get().and_then(|id| self.handle(id)).or_else(|| self.order.borrow().first().and_then(|id| self.handle(*id)))
    }

    /// At the end of a frame: the menus as they stand now (each item enabled while the active
    /// window handles its command, with the keys the keymap binds to it there) go to the
    /// platform when they changed.
    fn present_menus(self: &Rc<Self>) {
        let Some(w) = self.command_window() else { return };
        let mut menus = self.menus.borrow().clone();
        for m in &mut menus {
            for item in m.actions_mut() {
                if let crate::menu::MenuItem::Action { id, enabled, shortcut, .. } = item {
                    *enabled = *enabled && w.command_enabled(id);
                    if shortcut.is_none() {
                        *shortcut = w.chord_for(id);
                    }
                }
            }
        }
        if self.menus_presented.borrow().as_ref() != Some(&menus) {
            self.system.borrow_mut().set_menus(&menus);
            *self.menus_presented.borrow_mut() = Some(menus);
        }
    }

    pub(crate) fn open_url(&self, url: &str) {
        self.system.borrow_mut().open_url(url);
    }

    pub(crate) fn write_clipboard(&self, item: &crate::platform::ClipboardItem) {
        let mut system = self.system.borrow_mut();
        match (item.image(), item.text()) {
            (Some(image), _) => system.write_image(image),
            (None, Some(text)) => system.write_text(text),
            (None, None) => {}
        }
    }

    pub(crate) fn read_clipboard(&self) -> Option<crate::platform::ClipboardItem> {
        let mut system = self.system.borrow_mut();
        let (text, image) = (system.read_text(), system.read_image());
        crate::platform::ClipboardItem::parts(text, image)
    }

    /// Makes window `id` with its platform window; `root` is mounted by the caller.
    fn insert_window(&self, options: crate::platform::WindowOptions, root: Rc<dyn Fn() -> View>) -> Result<WindowId, String> {
        self.check_running()?;
        let id = self.windows.borrow_mut().try_insert_with_key(|id| {
            let platform = self.system.borrow_mut().open_window(id, &options)?;
            Ok::<_, String>(Rc::new(WindowState::new(self, id, options, root, platform)))
        })?;
        self.order.borrow_mut().push(id);
        Ok(id)
    }

    /// After an operation: with consistency checks on, every window's tree must equal its
    /// from-scratch rebuild.
    fn check_consistency(self: &Rc<Self>) {
        if self.shutdown.get() || !self.check.get() || self.checking.get() {
            return;
        }
        // A budget cutoff leaves unfinished work visible to the caller. There is no settled
        // incremental baseline to compare until a later operation completes that work.
        let unfinished = self.rt.has_pending()
            || self
                .windows
                .borrow()
                .values()
                .any(|w| !w.closed.get() && w.settled_generation.get() != Some(w.kernel.borrow().mutation_generation()));
        if unfinished {
            return;
        }
        self.checking.set(true);
        for id in self.order.borrow().clone() {
            let Some(w) = self.handle(id) else { continue };
            let incremental = w.w.kernel.borrow().tree_dump();
            w.reload();
            let scratch = w.w.kernel.borrow().tree_dump();
            if incremental != scratch {
                self.checking.set(false);
                panic!(
                    "window `{}`: incremental and from-scratch trees differ\n--- incremental\n{incremental}--- scratch\n{scratch}",
                    w.w.options.title
                );
            }
        }
        self.checking.set(false);
    }

    /// Switches the theme every window shows; each window's kernel restyles and its breakpoint
    /// follows (the theme may move the breakpoints).
    pub(crate) fn set_theme(self: &Rc<Self>, theme: Theme) {
        if theme == *self.res.theme.borrow() {
            return;
        }
        *self.res.theme.borrow_mut() = theme;
        for id in self.order.borrow().clone() {
            let Some(w) = self.handle(id) else { continue };
            w.w.kernel.borrow_mut().theme_changed();
            let bp = self.res.theme.borrow().breakpoint_of(w.w.size.get().0);
            if untrack(|| w.w.breakpoint.get()) != bp {
                w.enter(|| w.w.breakpoint.set(bp));
            }
        }
    }

    /// The shared text system's fonts changed: every window measures its text again.
    fn fonts_changed(self: &Rc<Self>) {
        for id in self.order.borrow().clone() {
            if let Some(w) = self.handle(id) {
                w.w.kernel.borrow_mut().fonts_changed();
            }
        }
        self.frame(std::panic::Location::caller());
    }

    /// Opens a window from app code: made now, built at the end of the frame.
    pub(crate) fn request_open(&self, options: crate::platform::WindowOptions, root: Rc<dyn Fn() -> View>) -> Result<WindowId, String> {
        let id = self.insert_window(options, root)?;
        self.opening.borrow_mut().push(id);
        Ok(id)
    }

    /// Closes window `id` at the end of the frame.
    pub(crate) fn request_close(&self, id: WindowId) {
        if !self.shutdown.get() {
            self.closing.borrow_mut().push(id);
        }
    }

    pub(crate) fn open_windows(&self) -> Vec<WindowId> {
        self.order.borrow().clone()
    }

    fn handle(self: &Rc<Self>, id: WindowId) -> Option<Window> {
        let w = self.windows.borrow().get(id).cloned()?;
        Some(Window { app: self.clone(), w })
    }

    /// Revokes window input, disposes its resources, then removes it from the App. Keep its
    /// root scope and registry entry until each stage succeeds so shutdown can resume it.
    fn close(self: &Rc<Self>, id: WindowId) {
        let Some(w) = self.handle(id) else { return };
        w.w.closed.set(true);
        if let Some(s) = w.w.root_scope.get() {
            w.enter(|| self.rt.dispose_scope(s));
            w.w.root_scope.set(None);
        }
        // The signals the window itself holds (no scope owns them).
        w.enter(|| {
            w.w.viewport.dispose();
            w.w.breakpoint.dispose();
            w.w.interaction_revision.dispose();
            for (_, s) in w.w.scrolls.borrow_mut().drain() {
                s.dispose();
            }
            for (_, s) in w.w.sizes.borrow_mut().drain() {
                s.dispose();
            }
            for (_, s) in w.w.tails.borrow_mut().drain() {
                s.dispose();
            }
            for (_, c) in w.w.clocks.borrow_mut().drain() {
                c.task.cancel();
                c.signal.dispose();
            }
        });
        *w.w.interaction.borrow_mut() = Interaction::default();
        w.w.root_slots.borrow_mut().clear();
        let platform = std::mem::replace(&mut *w.w.platform.borrow_mut(), Box::new(crate::platform::HeadlessWindow::default()));
        drop(platform);
        self.order.borrow_mut().retain(|x| *x != id);
        self.windows.borrow_mut().remove(id);
    }
}

impl Window {
    pub(crate) fn check_running(&self) -> Result<(), String> {
        self.app.check_running()?;
        if self.w.closed.get() {
            return Err("window is closed".into());
        }
        Ok(())
    }

    /// `root` in an 800 × 600 window of a new headless app: what tests and agents drive. The
    /// app is [`app`](Self::app)'s (its clock, theme, fonts, other windows).
    pub fn headless(root: impl Fn() -> View + 'static) -> Window {
        Window::headless_on(crate::platform::HeadlessPlatform::default(), root)
    }

    /// [`headless`](Self::headless) on `platform` (a fake native host, a clipboard).
    pub fn headless_on(platform: crate::platform::HeadlessPlatform, root: impl Fn() -> View + 'static) -> Window {
        let app = App::new(platform);
        app.open_window(crate::platform::WindowOptions::new("Headless"), root).expect("a headless window opens")
    }

    /// Its id.
    pub fn id(&self) -> WindowId {
        self.w.id
    }

    /// This window's opaque Inspector identity, including its current tree incarnation.
    pub fn target(&self) -> String {
        crate::inspect::window_target(&self.w.kernel.borrow())
    }

    pub(crate) fn focused_node(&self) -> Option<NodeId> {
        let node = self.w.interaction.borrow().focused?;
        let k = self.w.kernel.borrow();
        (k.is_alive(node) && !k.is_exiting(node)).then_some(node)
    }

    /// The app it belongs to.
    pub fn app(&self) -> App {
        App { app: self.app.clone() }
    }

    /// Whether it was closed (a handle outlives its window).
    pub fn is_closed(&self) -> bool {
        self.w.closed.get()
    }

    /// What it was opened with.
    pub fn options(&self) -> crate::platform::WindowOptions {
        self.w.options.clone()
    }

    /// Runs `f` as this window's code.
    pub(crate) fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        enter_app(&self.app, || self.app.rt.with_owner(Some(self.w.id.owner()), f))
    }

    fn restore_interaction(&self) {
        let mut i = self.w.interaction.borrow_mut();
        let mut k = self.w.kernel.borrow_mut();
        let (hover, focus) = std::mem::take(&mut i.reload);
        let hovered = hover.and_then(|a| k.find_address(&a));
        i.hovered = hovered;
        i.hover_chain = k.hover_chain(hovered);
        for n in i.hover_chain.clone() {
            k.set_state(n, State::HOVER, true);
        }
        i.focused = focus.and_then(|a| k.find_address(&a));
        if let Some(n) = i.focused {
            k.set_state(n, State::FOCUSED, true);
            k.set_state(n, State::FOCUS_VISIBLE, i.modality == Modality::Keyboard);
        }
        let mut apply = |id: &str, state: State, on: bool| {
            if let Some(n) = k.find(id) {
                k.set_state(n, state, on);
            }
        };
        for (id, state, on) in &i.forced {
            apply(id, *state, *on);
        }
        // The new content is not laid out yet: clamping happens after the next layout.
        for (id, (x, y)) in &i.scrolls {
            if let Some(n) = k.find(id) {
                k.scroll_to_unclamped(n, *x, *y);
            }
        }
        for id in &i.released {
            if let Some(n) = k.find(id) {
                k.release_tail(n);
            }
        }
        drop((i, k));
        self.w.interaction_changed();
    }

    /// The mouse is over `n` (the topmost node there): it and its ancestors are hovered, up to
    /// the innermost `hover_alone` one (GPUI, Rune, CSS).
    pub(crate) fn set_hover(&self, n: Option<NodeId>) {
        let mut i = self.w.interaction.borrow_mut();
        if i.hovered == n {
            return;
        }
        let mut k = self.w.kernel.borrow_mut();
        let chain = k.hover_chain(n);
        for p in std::mem::take(&mut i.hover_chain) {
            if k.is_alive(p) && !chain.contains(&p) {
                k.set_state(p, State::HOVER, false);
            }
        }
        // Outermost first, so a group is hovered before its members resolve.
        for c in chain.iter().rev() {
            k.set_state(*c, State::HOVER, true);
        }
        i.hovered = n;
        i.hover_chain = chain;
        drop((i, k));
        self.tip_hovered();
    }

    /// Records how input last arrived; the focus ring follows it.
    pub(crate) fn set_modality(&self, m: Modality) {
        let mut i = self.w.interaction.borrow_mut();
        if i.modality == m {
            return;
        }
        i.modality = m;
        if let Some(n) = i.focused {
            let mut k = self.w.kernel.borrow_mut();
            if k.is_alive(n) {
                k.set_state(n, State::FOCUS_VISIBLE, m == Modality::Keyboard);
            }
        }
    }

    /// Moves keyboard focus to `n`, which must be focusable and enabled (`None` blurs). The
    /// ring shows if the last input was the keyboard.
    pub(crate) fn set_focus(&self, n: Option<NodeId>) -> Result<(), String> {
        let modal = self.top_modal();
        let mut i = self.w.interaction.borrow_mut();
        let mut k = self.w.kernel.borrow_mut();
        if let Some(n) = n {
            let name = k.test_id(n).unwrap_or("element").to_owned();
            k.check_activation(n, modal).map_err(|why| format!("`{name}` {why}"))?;
            if !k.is_focusable(n) {
                return Err(format!("`{name}` is not focusable"));
            }
        }
        if i.focused == n {
            return Ok(());
        }
        let mut blurred = None;
        if let Some(p) = i.focused.take()
            && k.is_alive(p)
        {
            // Uncommitted text belongs to this field. Moving focus cancels its displayed
            // composition while keeping the original committed value and selection.
            k.set_preedit(p, "");
            k.set_state(p, State::FOCUSED, false);
            k.set_state(p, State::FOCUS_VISIBLE, false);
            blurred = (!k.is_exiting(p)).then_some(p);
        }
        if let Some(n) = n {
            k.set_state(n, State::FOCUSED, true);
            k.set_state(n, State::FOCUS_VISIBLE, i.modality == Modality::Keyboard);
            k.focused_in_group(n);
        }
        i.focused = n;
        drop((i, k));
        self.w.interaction_changed();
        // What lost focus hears it first, then what got it.
        if let Some(p) = blurred {
            self.notify_lifecycle(p, HandlerKind::Blur);
        }
        if let Some(n) = n
            && self.w.interaction.borrow().focused == Some(n)
        {
            self.run(n, &[HandlerKind::Focus], None);
        }
        Ok(())
    }

    /// Applies a runtime-driven state change; `Err` if the target cannot take it.
    fn interact(&self, ev: &Event) -> Result<(), String> {
        let find = |id: &str| self.w.kernel.borrow().find(id).ok_or(format!("no node with test id `{id}`"));
        match ev {
            Event::Hover(id, on) => {
                let n = find(id)?;
                if *on {
                    self.set_hover(Some(n));
                } else if self.w.interaction.borrow().hover_chain.contains(&n) {
                    self.set_hover(None);
                }
            }
            Event::Focus(id) => {
                let n = find(id)?;
                self.set_modality(Modality::Keyboard);
                self.set_focus(Some(n))?
            }
            Event::Blur => self.set_focus(None)?,
            Event::Scroll(id, dx, dy) | Event::ScrollTo(id, dx, dy) => {
                let mut k = self.w.kernel.borrow_mut();
                let n = k.find(id).ok_or(format!("no node with test id `{id}`"))?;
                let (sx, sy) = k.scrollable(n);
                let o = n_offset(&k, n, matches!(ev, Event::Scroll(..)));
                if (*dx != 0.0 && !sx) || (*dy != 0.0 && !sy) || (!sx && !sy) {
                    return Err(format!("`{id}` does not scroll on that axis: give it `overflow_y_scroll()` (or `overflow_x_scroll()`)"));
                }
                if matches!(ev, Event::ScrollTo(..)) {
                    // A program's request: it glides where layout moves.
                    k.glide_scroll(n, o.0 + dx, o.1 + dy);
                } else {
                    k.stop_glide(n);
                    k.scroll_to(n, o.0 + dx, o.1 + dy);
                }
            }
            _ => unreachable!("only interaction events"),
        }
        Ok(())
    }

    fn handler_for(&self, ev: &Event) -> Result<ScopedHandler, String> {
        let (id, kind) = match ev {
            Event::Press(id) => (id, HandlerKind::Press),
            Event::Input(id, _) => (id, HandlerKind::Input),
            Event::NumericInput(id, _) => (id, HandlerKind::NumericInput),
            Event::Toggle(id) => (id, HandlerKind::Toggle),
            Event::Hover(..) | Event::Focus(_) | Event::Blur | Event::Scroll(..) | Event::ScrollTo(..) => {
                unreachable!("interaction events have no handler")
            }
            Event::Send(..) => unreachable!("sends go to channels"),
        };
        let k = self.w.kernel.borrow();
        let node = k.find(id).ok_or(format!("no node with test id `{id}`"))?;
        k.check_activation(node, self.top_modal()).map_err(|why| format!("`{id}` {why}"))?;
        if kind == HandlerKind::NumericInput && !k.preedit(node).is_empty() {
            return Err(format!("`{id}` has an active input-method composition"));
        }
        // A link's press is the `on_link` above it, given its target.
        if kind == HandlerKind::Press
            && k.handler(node, kind).is_none()
            && let Some(target) = k.link_target(node)
        {
            // No `on_link` takes it: the system opens it.
            let Some(at) = k.handler_above(k.parent(node), HandlerKind::Link) else {
                let app = self.app.clone();
                return Ok(ScopedHandler::capture(Handler::Press(std::rc::Rc::new(move || app.open_url(&target)))));
            };
            let hid = k.handler(at, HandlerKind::Link).expect("found by kind");
            let Some(mut handler) = self.w.handlers.borrow().get(&hid.0).cloned() else { return Err("on_link was disposed".into()) };
            let Handler::Link(f) = handler.handler else { return Err("on_link was disposed".into()) };
            handler.handler = Handler::Press(std::rc::Rc::new(move || f(&target)));
            return Ok(handler);
        }
        let hid = k.handler(node, kind).ok_or(format!("`{id}` has no {kind:?} handler"))?;
        self.w.handlers.borrow().get(&hid.0).cloned().ok_or(format!("handler of `{id}` was disposed"))
    }
}

impl Window {
    /// The current window size.
    pub fn size(&self) -> (f32, f32) {
        self.w.size.get()
    }
    /// Resizes the window: bindings that read `viewport()` update, then everything is laid out
    /// again at the new size (text re-breaks; nothing else reruns).
    pub fn resize(&mut self, width: f32, height: f32) {
        let scale = self.w.scale.get();
        self.event(InputEvent::Resize { width, height, scale });
    }
    /// Its scale: device pixels per logical pixel (what the last resize said).
    pub fn scale(&self) -> f32 {
        self.w.scale.get()
    }
    pub(crate) fn on_resize(&mut self, width: f32, height: f32, scale: f32) {
        self.w.scale.set(scale);
        self.w.size.set((width, height));
        self.enter(|| {
            self.app.rt.with_frame(Ctx::Handler, None, Some(None), || {
                self.w.viewport.set((width, height));
                let bp = self.w.kernel.borrow().theme().breakpoint_of(width);
                if untrack(|| self.w.breakpoint.get()) != bp {
                    self.w.breakpoint.set(bp);
                }
            });
            self.settle_frame();
        })
    }
    /// The window's breakpoint (Tailwind `sm` … `2xl`).
    pub fn breakpoint(&self) -> Breakpoint {
        self.kernel().theme().breakpoint_of(self.w.size.get().0)
    }
    /// Runs `check` at each window size in turn (e.g. phone and desktop), then restores the size.
    /// Stops at the first error, naming the size.
    pub fn at_sizes(&mut self, sizes: &[(f32, f32)], mut check: impl FnMut(&mut Window) -> Result<(), String>) -> Result<(), String> {
        self.check_running()?;
        let before = self.size();
        let mut result = Ok(());
        for &(w, h) in sizes {
            self.resize(w, h);
            if let Err(e) = check(self) {
                result = Err(format!("at {w}×{h}: {e}"));
                break;
            }
        }
        self.resize(before.0, before.1);
        result
    }
    /// Layout problems in the current frame (overflow, collapsed elements, truncated text,
    /// clamped overlays, missing tokens). Tests and agents can assert there are none.
    pub fn layout_issues(&self) -> Vec<crate::kernel::LayoutIssue> {
        self.w.kernel.borrow().layout_issues()
    }
    /// The semantic tree and app state as the Inspector protocol gives them (see
    /// [`crate::inspect`]).
    pub fn inspect(&self) -> crate::inspect::Snapshot {
        let (w, h) = self.size();
        let k = self.w.kernel.borrow();
        let focus = self.w.interaction.borrow().focused.filter(|n| k.is_alive(*n)).and_then(|n| k.test_id(n).map(str::to_owned));
        let text_input = self.text_field().and_then(|field| {
            let id = self.w.interaction.borrow().focused?;
            crate::inspect::TextInputState::from_field(field, k.text_document(id)?)
        });
        crate::inspect::Snapshot {
            failed: self.app.failed.get(),
            schema: crate::inspect::SCHEMA.into(),
            octara: env!("CARGO_PKG_VERSION").into(),
            viewport: [w, h],
            theme: k.theme().name.into(),
            breakpoint: crate::inspect::snake(&format!("{:?}", self.breakpoint())),
            time_ms: self.app().now().as_millis() as u64,
            focus,
            text_input,
            pending: self.app().pending().len(),
            window: self.w.id.number(),
            target: crate::inspect::window_target(&k),
            // Selection extraction may build unmounted virtual rows; never execute it after failure.
            selection: (!self.app.failed.get()).then(|| self.selected_text()).flatten().map(|t| t.chars().take(2000).collect()),
            asks: self.asks().into_iter().map(|(id, ask)| crate::inspect::AskInfo { id, ask }).collect(),
            windows: self
                .app()
                .each()
                .iter()
                .map(|w| crate::inspect::WindowInfo {
                    window: w.w.id.number(),
                    target: w.target(),
                    title: w.w.options.title.clone(),
                    size: [w.w.size.get().0, w.w.size.get().1],
                })
                .collect(),
            root: crate::inspect::tree(&k),
        }
    }
    /// What the current frame draws, in order (see [`crate::paint`]).
    pub fn display_list(&self) -> std::rc::Rc<crate::paint::DisplayList> {
        let list = self.w.kernel.borrow().display_list();
        match self.picking_overlay() {
            Some(over) => {
                let mut list = (*list).clone();
                list.parts.extend(over.into_iter().map(crate::paint::Part::Item));
                std::rc::Rc::new(list)
            }
            None => list,
        }
    }
    /// Read the retained tree and its measurements. Update UI through state and events.
    pub fn kernel(&self) -> Ref<'_, Kernel> {
        self.w.kernel.borrow()
    }
    /// Resets this window's kernel work counters without changing the tree. Reactive work is
    /// counted across the App; reset it separately with [`App::reset_work`].
    pub fn reset_counters(&self) {
        self.w.kernel.borrow_mut().reset_counters();
    }
    /// Benchmark control: disable layout to measure state/tree updates separately. Geometry
    /// remains stale while disabled; re-enabling takes effect on the next frame.
    pub fn set_layout_enabled(&self, on: bool) {
        self.w.kernel.borrow_mut().set_layout_enabled(on);
    }
    /// Reference-rendering control: disable subtree culling to check painting and hit testing
    /// against a full traversal. Takes effect on the next display-list read or hit test.
    pub fn set_culling(&self, on: bool) {
        self.w.kernel.borrow_mut().set_culling(on);
    }
    /// Runs the event's handler, applies every update and lays out, like one frame.
    pub fn dispatch(&mut self, ev: Event) -> Result<(), String> {
        self.check_running()?;
        self.app.record(self.w.id, &InputEvent::Semantic(ev.clone()));
        self.on_semantic(ev)
    }
    pub(crate) fn on_semantic(&mut self, ev: Event) -> Result<(), String> {
        self.check_running()?;
        if let Event::NumericInput(_, value) = &ev
            && !value.is_finite()
        {
            return Err("numeric input requires a finite value".into());
        }
        self.enter(|| {
            let cause = self.app.next_cause.get() + 1;
            if matches!(ev, Event::Hover(..) | Event::Focus(_) | Event::Blur | Event::Scroll(..) | Event::ScrollTo(..)) {
                self.w.kernel.borrow_mut().set_cause(Some(cause));
                let r = self.interact(&ev);
                if r.is_err() {
                    self.w.kernel.borrow_mut().set_cause(None);
                    return r;
                }
                self.app.next_cause.set(cause);
                self.settle_frame();
                self.w.kernel.borrow_mut().set_cause(None);
                return Ok(());
            }
            if let Event::Send(name, payload) = &ev {
                self.w.kernel.borrow_mut().set_cause(Some(cause));
                let r = self.app.sched.send_named(name, payload);
                if r.is_ok() {
                    self.app.next_cause.set(cause);
                    self.settle_frame();
                }
                self.w.kernel.borrow_mut().set_cause(None);
                return r;
            }
            let h = self.handler_for(&ev)?;
            self.app.next_cause.set(cause);
            self.w.kernel.borrow_mut().set_cause(Some(cause));
            h.run(&self.app.rt, |h| match (h, ev) {
                (Handler::Press(f), _) | (Handler::Toggle(f), _) => f(),
                (Handler::Input(f), Event::Input(_, v)) => f(v),
                (Handler::NumericInput(f), Event::NumericInput(_, v)) => f(v),
                _ => unreachable!("handler kind matches event kind"),
            });
            self.settle_frame();
            self.w.kernel.borrow_mut().set_cause(None);
            Ok(())
        })
    }
    /// The platform whose conventions input follows (the build's by default).
    /// Holds an interaction state (`HOVER`, `FOCUSED`, `FOCUS_VISIBLE`, `PRESSED`) on or off, e.g. to check
    /// pressed styles; kept across hot reload. `SELECTED` and `DISABLED` come from props.
    pub fn force_state(&mut self, test_id: &str, state: State, on: bool) -> Result<(), String> {
        self.check_running()?;
        if !RUNTIME_STATES.contains(state) {
            return Err(format!("{state:?} is set by element props, not forced"));
        }
        {
            let mut k = self.w.kernel.borrow_mut();
            let n = k.find(test_id).ok_or(format!("no node with test id `{test_id}`"))?;
            k.set_state(n, state, on);
        }
        self.enter(|| self.settle_frame());
        let mut i = self.w.interaction.borrow_mut();
        i.forced.retain(|(id, s, _)| !(id == test_id && *s == state));
        i.forced.push((test_id.to_owned(), state, on));
        Ok(())
    }
    /// Drops every forced state (the nodes keep their current state until it next changes).
    pub fn clear_forced(&mut self) {
        self.w.interaction.borrow_mut().forced.clear();
    }
    /// Rebuilds this window's scope from the current state, keeping hover and focus by address
    /// (the nearest test id and the child indices below it) and scroll by test id.
    fn reload(&self) {
        self.enter(|| {
            self.w.kernel.borrow_mut().advance_identity();
            let modals = self.save_modals();
            {
                let mut i = self.w.interaction.borrow_mut();
                let k = self.w.kernel.borrow();
                i.scrolls = k.scroll_offsets();
                i.released = k.released_tails();
                let address = |n: Option<NodeId>| n.filter(|n| k.is_alive(*n)).map(|n| k.address(n));
                i.reload = (address(i.hovered), address(i.focused));
                i.reload_modals = modals;
                i.hover_chain.clear();
                i.pointer = pointer::PointerState::default();
                i.modals.clear();
                i.edits.clear();
            }
            if let Some(s) = self.w.root_scope.get() {
                self.app.rt.set_reloading(true);
                let _reload = std::mem::DropGuard::new(&self.app.rt, |rt| rt.set_reloading(false));
                self.app.rt.dispose_scope(s);
                self.w.root_scope.set(None);
            }
            self.w.root_slots.borrow_mut().clear();
            // A rebuild is not content arriving: nothing enters.
            self.w.kernel.borrow_mut().set_quiet(true);
            self.mount_root();
            self.w.kernel.borrow_mut().set_quiet(false);
        });
    }
}

impl App {
    /// The platform whose conventions input follows (the build's by default).
    pub fn target_platform(&self) -> TargetPlatform {
        self.app.platform.get()
    }
    /// The platform whose conventions input follows: its keymap, and on touch platforms the
    /// controls' touch sizes (`when_touch`), in every window.
    pub fn set_target_platform(&self, p: TargetPlatform) {
        self.app.platform.set(p);
        self.app.res.touch.set(p.is_touch());
        for w in self.each() {
            w.w.kernel.borrow_mut().touch_changed();
        }
        self.app.frame(std::panic::Location::caller());
    }
    /// Switches the theme of every window. Token colors resolve in the kernels, so no closure
    /// reruns; only nodes whose resolved colors change are repainted.
    pub fn set_theme(&self, theme: Theme) {
        self.enter(|| self.app.set_theme(theme));
        self.app.frame(std::panic::Location::caller());
    }
    /// Adds font data (TTF/OTF); its families can then be named by the theme or `font_family`,
    /// in every window.
    pub fn register_font(&self, data: &'static [u8]) {
        self.app.res.text.borrow_mut().register_font(data);
        self.app.fonts_changed();
    }
    /// Adds a font text falls back to for `scripts` (ISO 15924 tags: `*b"Hani"` for hanzi,
    /// `*b"Hira"`, `*b"Kana"`, `*b"Hang"`, …) when its own family lacks a character.
    pub fn register_fallback_font(&self, data: &'static [u8], scripts: &[[u8; 4]]) {
        self.app.res.text.borrow_mut().register_fallback_font(data, scripts);
        self.app.fonts_changed();
    }
    /// Replaces the text engine (e.g. `MonoEngine` for benchmarks): every window measures again
    /// (fonts registered with the old one are not carried over).
    pub fn set_text_engine(&self, engine: Box<dyn crate::kernel::text::TextEngine>) {
        self.app.res.set_text_engine(engine);
        self.app.fonts_changed();
    }
    /// Uses the platform's fonts: `system-ui` and `ui-monospace` (the theme's families) become
    /// its UI and monospace fonts (San Francisco and Menlo, Segoe UI and Cascadia Mono, …), with
    /// its fallback for scripts (CJK, emoji, …): what a window shows. Headless screenshots meant
    /// to match a window use it; deterministic tests do not (results then depend on the machine).
    #[cfg(feature = "system-fonts")]
    pub fn use_system_fonts(&self) {
        self.set_text_engine(Box::new(crate::kernel::text::ParleyEngine::system()));
    }
    /// The theme every window shows.
    pub fn theme(&self) -> Theme {
        self.app.res.theme.borrow().clone()
    }
    /// Rebuilds every window's scope from the current state, as after a code patch; named state
    /// survives.
    pub fn hot_reload(&self) {
        for w in self.each() {
            w.reload();
        }
    }
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut diagnostics = self.app.rt.diagnostics();
        let late = self.app.sched.late_completion_failures();
        if late > 0 {
            diagnostics.push(Diagnostic {
                code: &codes::LATE_COMPLETION_PANIC,
                message: format!("{late} external completion panic(s) after shutdown; see the panic hook output for details"),
                location: None,
            });
        }
        diagnostics
    }
    /// Each stored diagnostic's code with how many times it occurred (repeats are not stored).
    pub fn diagnostic_counts(&self) -> Vec<(&'static str, u32)> {
        let mut counts = self.app.rt.diagnostic_counts();
        let late = self.app.sched.late_completion_failures();
        if late > 0 {
            counts.push((codes::LATE_COMPLETION_PANIC.id, late));
        }
        counts
    }
    pub fn work(&self) -> Work {
        Work { closures_run: self.app.rt.closures_run(), reactive_nodes: self.app.rt.live_nodes() }
    }
    pub fn reset_work(&self) {
        self.app.rt.reset_closures_run();
    }
}
