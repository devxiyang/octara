//! The app runtime: mounts a root view, dispatches events, lays out, and rebuilds for hot reload.

mod asks;
mod back;
mod command;
mod config;
mod drive;
mod edit;
mod event;
mod feedback;
mod focus;
mod frame;
mod guard;
mod input;
mod keys;
mod pick;
mod pointer;
mod presentation;
mod resources;
mod selection;
mod time;
mod tooltip;
mod virtual_list;

pub(crate) use crate::kernel::TextDocument;
pub use asks::menu_surface;
pub use back::BackPhase;
pub use config::AppConfig;
pub use edit::{TextField, byte_offset, utf16_offset};
pub use event::{FileDrag, InputEvent, Recorded, Recording, WheelPhase};
pub(crate) use guard::GuardTarget;
pub use guard::TargetGuard;
pub use keys::{BUILTIN_COMMANDS, Binding, Chord, Key, KeyEvent, Keymap};
pub(crate) use octara_primitives::text::TextEdit;
pub use pointer::{Button, Drag, DragPhase, Modifiers, Phase, PointerEvent, PointerKind, TouchScrollStage, Tuning};
pub use selection::TextPoint;
use virtual_list::VirtualLists;
pub(crate) use virtual_list::{ListOrder, RowReveal};

use crate::diagnostics::{self as codes, Diagnostic};
use crate::kernel::{Breakpoint, HandlerKind, Kernel, NodeId, State, Theme};
use crate::reactive::graph::{Ctx, Owner, Runtime, ScopeId};
use crate::reactive::graph::{Signal, untrack};
use crate::view::{Handler, IntoView, Parent, ScopedHandler, Slots, View, build_content, mount};
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

impl Event {
    fn test_id(&self) -> Option<&str> {
        match self {
            Self::Press(id)
            | Self::Input(id, _)
            | Self::NumericInput(id, _)
            | Self::Toggle(id)
            | Self::Hover(id, _)
            | Self::Focus(id)
            | Self::Scroll(id, ..)
            | Self::ScrollTo(id, ..) => Some(id),
            Self::Blur | Self::Send(..) => None,
        }
    }
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
    failed: Rc<Cell<bool>>,
    /// Tasks, timers and channels, and the clock: one for all the windows.
    pub(crate) sched: Rc<crate::task::Scheduler>,
    /// The theme, the text system and input conventions every window's kernel reads.
    pub(crate) res: Rc<crate::kernel::Resources>,
    windows: RefCell<slotmap::SlotMap<WindowId, Rc<WindowState>>>,
    /// The open windows, in the order they opened.
    order: RefCell<Vec<WindowId>>,
    /// Allocated windows to mount and retired windows to dispose in the App update.
    opening: RefCell<Vec<WindowId>>,
    closing: RefCell<Vec<WindowId>>,
    /// Window requests share one allowance across the operation's preparation checkpoints.
    window_rounds_left: Cell<u8>,
    updating: Cell<bool>,
    /// A nested poll leaves ready work to the enclosing update, after its borrows end.
    poll_requested: Cell<bool>,
    next_cause: Cell<u64>,
    tuning: Cell<Tuning>,
    platform: Cell<TargetPlatform>,
    /// The app's own bindings (after the platform's standard ones).
    keymap: RefCell<keys::Keymap>,
    /// The platform's standard bindings then the app's, built once until either changes.
    keymap_effective: RefCell<Option<(TargetPlatform, Rc<[Binding]>)>>,
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
    /// Settled command availability when a view binding observes it.
    command_availability: Cell<Option<Signal<Vec<String>>>>,
    /// Regions of the kernel's overlay layer, one per `overlay` of a mounted element.
    pub(crate) overlay_slots: Slots,
    pub(crate) viewport: Signal<(f32, f32)>,
    pub(crate) breakpoint: Signal<Breakpoint>,
    /// Scroll containers someone follows (virtual lists), with their state as a signal.
    scrolls: RefCell<rustc_hash::FxHashMap<NodeId, Signal<feedback::ScrollState>>>,
    /// Element sizes someone follows (`size_of`), by test id.
    sizes: RefCell<rustc_hash::FxHashMap<String, Signal<(f32, f32)>>>,
    /// Whether scroll containers follow their tail, as someone reads it (`following_tail`), by
    /// test id.
    tails: RefCell<rustc_hash::FxHashMap<String, Signal<bool>>>,
    /// Clocks someone reads (`clock`), by period in nanoseconds.
    pub(crate) clocks: RefCell<rustc_hash::FxHashMap<u128, Clock>>,
    /// Paced texts (`paced`) in this window, by number.
    pub(crate) paced: RefCell<Vec<(u64, Rc<crate::view::Paced>)>>,
    /// Run after each layout; return whether they changed anything (then the window lays out again).
    after_layout: RefCell<Vec<(u64, feedback::AfterLayout)>>,
    pub(crate) next: Cell<u64>,
    options: crate::platform::WindowOptions,
    /// Closed: its scope is disposed and it left the app.
    closed: Cell<bool>,
    root: Rc<dyn Fn() -> View>,
    root_scope: Cell<Option<ScopeId>>,
    rebuild_pending: Cell<bool>,
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
    cursor: Cell<octara_primitives::input::CursorStyle>,
    /// What the platform was last told of each native view: its props version and placement.
    native_sent: RefCell<rustc_hash::FxHashMap<NodeId, (u64, Option<crate::native::Placement>)>>,
    /// Native changes not presented yet.
    native_changes: RefCell<Vec<crate::native::NativeChange>>,
    /// The paint generation last presented.
    presented: Cell<u64>,
    /// The revision accepted at the App's final publication boundary.
    settled_revision: Cell<Option<WindowRevision>>,
    /// Frames presented to the platform window.
    frames: Cell<u64>,
    /// The active painted capture and its native responses.
    capturing: RefCell<Option<crate::capture::Capturing>>,
    /// Picking an element to hand to an agent.
    picking: RefCell<pick::Picking>,
    /// The document selection and its projection at the last presentation.
    selection: RefCell<Option<selection::DocSelection>>,
    marked_selection: RefCell<selection::MarkedSelection>,
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
    pub(crate) lists: RefCell<VirtualLists>,
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
        if std::thread::panicking() {
            app.rt.fail("panic escaped an App operation; shut down this App and create a new one", std::panic::Location::caller());
        }
        if app.failed.get() {
            for window in app.windows.borrow().values() {
                window.settled_revision.set(None);
                if let Ok(mut capture) = window.capturing.try_borrow_mut() {
                    capture.take();
                }
                if let Ok(mut kernel) = window.kernel.try_borrow_mut() {
                    kernel.cancel_pending_edits();
                }
            }
        }
    });
    if app.depth.get() == 0 && !app.checking.get() {
        app.sched.begin_turn();
        app.rt.resume_propagation();
        app.window_rounds_left.set(frame::WINDOW_ROUNDS);
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
        app.rt.enter(|| {
            let result = f();
            if app.depth.get() == 1 && !app.failed.get() {
                app.rt.prune_keys();
            }
            result
        })
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
    Ready(WindowRevision),
    Incomplete,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowRevision {
    mutation: u64,
    paint: u64,
    time: Option<u64>,
    viewport: [u32; 3],
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
    fn effective_keymap(&self) -> Rc<[Binding]> {
        let platform = self.platform.get();
        if let Some((p, k)) = self.keymap_effective.borrow().as_ref()
            && *p == platform
        {
            return k.clone();
        }
        let mut bindings = keys::Keymap::standard(platform).bindings;
        bindings.extend(self.keymap.borrow().bindings.iter().cloned());
        let k: Rc<[Binding]> = bindings.into();
        *self.keymap_effective.borrow_mut() = Some((platform, k.clone()));
        k
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
        for work in &interaction.after_update {
            if let frame::AfterUpdate::TouchPress(press) = work {
                press.resident_nodes(&mut nodes);
            }
        }
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
        let held = interaction.pointer.retire(&within);
        kernel.retire_editors(node);
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
        self.interaction.borrow_mut().after_update.push_back(frame::AfterUpdate::Command(command.to_owned(), at));
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
    /// Refresh stationary hover after this input's layout and focus changes.
    refresh_hover: bool,
    focused: Option<NodeId>,
    modality: Modality,
    pointer: pointer::PointerState,
    back: back::BackInteraction,
    /// Open modals, outermost first.
    modals: Vec<focus::OpenModal>,
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
    /// Input work that needs settled geometry or focus before delivery.
    after_update: std::collections::VecDeque<frame::AfterUpdate>,
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
        let breakpoint = kernel.theme().breakpoint_of(size.0);
        WindowState {
            id,
            kernel: RefCell::new(kernel),
            handlers: RefCell::default(),
            commands: RefCell::default(),
            command_availability: Cell::new(None),
            overlay_slots: Slots::default(),
            viewport: app.rt.enter(|| Signal::detached(size)),
            breakpoint: app.rt.enter(|| Signal::detached(breakpoint)),
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
            rebuild_pending: Cell::new(false),
            root_slots: Slots::default(),
            interaction: RefCell::default(),
            interaction_revision: app.rt.enter(|| Signal::detached(0)),
            size: Cell::new(size),
            scale: Cell::new(1.0),
            platform: RefCell::new(platform),
            text_input: RefCell::new(None),
            cursor: Cell::new(octara_primitives::input::CursorStyle::Arrow),
            native_sent: RefCell::default(),
            native_changes: RefCell::default(),
            presented: Cell::new(u64::MAX),
            settled_revision: Cell::new(None),
            frames: Cell::new(0),
            capturing: RefCell::default(),
            picking: RefCell::default(),
            selection: RefCell::default(),
            marked_selection: RefCell::default(),
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
        Self::with_config(platform, AppConfig::default())
    }

    /// Creates an App with its resources ready before the first window or view is created.
    pub fn with_config(platform: impl crate::platform::Platform + 'static, config: AppConfig) -> App {
        let AppConfig { theme, text_engine, target_platform } = config;
        let rt = Runtime::new();
        let root_scope = rt.create_app_scope();
        let failed = rt.failure_flag();
        let res = resources::text_resources(theme, text_engine, target_platform.is_touch(), &rt);
        let state = AppState {
            namespace: crate::identity::new_namespace(),
            rt,
            root_scope,
            shutdown: Cell::new(false),
            teardown: Cell::new(Teardown::Pending),
            failed,
            sched: Rc::new(crate::task::Scheduler::default()),
            res,
            windows: RefCell::default(),
            order: RefCell::default(),
            opening: RefCell::default(),
            closing: RefCell::default(),
            window_rounds_left: Cell::new(0),
            updating: Cell::new(false),
            poll_requested: Cell::new(false),
            next_cause: Cell::new(0),
            tuning: Cell::new(Tuning::default()),
            platform: Cell::new(target_platform),
            keymap: RefCell::new(keys::Keymap::default()),
            keymap_effective: RefCell::new(None),
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

    /// Opens a window showing `root`. A host call mounts and lays it out before returning.
    /// Calls during an App update mount it when that update processes its window requests.
    /// The factory runs in the window's scope and can return any [`IntoView`].
    #[track_caller]
    pub fn open_window<V: IntoView>(
        &self,
        options: crate::platform::WindowOptions,
        root: impl Fn() -> V + 'static,
    ) -> Result<Window, String> {
        let at = std::panic::Location::caller();
        let id = self.app.request_open(options, root, at)?;
        let w = self.app.handle(id).expect("just opened");
        if self.app.depth.get() == 0 {
            self.app.frame(Some(id), at);
        }
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

    /// Retires window `id` immediately. The App update disposes its scope and platform window.
    #[track_caller]
    pub fn close_window(&self, id: WindowId) {
        let at = std::panic::Location::caller();
        if self.app.request_close(id, at) && self.app.depth.get() == 0 {
            self.app.frame(None, at);
        }
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
        if self.app.teardown.get() != Teardown::Pending {
            return;
        }
        if !Runtime::caller_allows(crate::reactive::graph::Operation::Shutdown, at) {
            return;
        }
        enter_app_context(&self.app, || {
            if self.app.teardown.get() != Teardown::Pending {
                return;
            }
            if !self.app.rt.allows(crate::reactive::graph::Operation::Shutdown, at) {
                return;
            }
            self.app.shutdown.set(true);
            self.app.res.text.retire();
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
            let owner = crate::reactive::graph::ExecutionOwner { scope: Some(self.app.root_scope), window: None };
            let r = self.app.rt.with_owned(owner, f);
            // A nested call belongs to its enclosing operation; in particular, a memo or
            // binding must finish computing before that operation flushes and lays out.
            if self.app.depth.get() == 1 {
                self.app.frame(None, at);
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
        self.active
            .get()
            .and_then(|id| self.handle(id))
            .filter(Window::can_prepare)
            .or_else(|| self.order.borrow().iter().filter_map(|id| self.handle(*id)).find(Window::can_prepare))
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
                    *shortcut = w.chord_for(id);
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

    /// After an operation: with consistency checks on, every window's tree must equal its
    /// from-scratch rebuild.
    fn check_consistency(self: &Rc<Self>) {
        if self.shutdown.get() || !self.check.get() || self.checking.get() {
            return;
        }
        // A budget cutoff leaves unfinished work visible to the caller. There is no settled
        // incremental baseline to compare until a later operation completes that work.
        let unfinished = self.has_window_requests()
            || self.rt.has_pending()
            || self.order.borrow().iter().any(|&id| self.handle(id).is_some_and(|w| !w.frame_is_settled()));
        if unfinished {
            return;
        }
        self.checking.set(true);
        let windows = self.order.borrow().clone();
        for id in windows {
            let Some(w) = self.handle(id).filter(Window::can_prepare) else { continue };
            let incremental = w.w.kernel.borrow().tree_dump();
            w.reload();
            self.frame(Some(id), std::panic::Location::caller());
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

    /// Allocates the host window now and queues its root for the App update.
    pub(crate) fn request_open<V: IntoView>(
        &self,
        options: crate::platform::WindowOptions,
        root: impl Fn() -> V + 'static,
        at: &'static std::panic::Location<'static>,
    ) -> Result<WindowId, String> {
        use crate::reactive::graph::Operation;
        self.check_running()?;
        if !Runtime::caller_allows(Operation::Write, at) || !Runtime::caller_allows(Operation::Register, at) {
            return Err("window creation is not allowed in this execution phase".into());
        }
        let id = self.windows.borrow_mut().try_insert_with_key(|id| {
            let platform = self.system.borrow_mut().open_window(id, &options)?;
            let root = Rc::new(move || build_content(at, &root));
            Ok::<_, String>(Rc::new(WindowState::new(self, id, options, root, platform)))
        })?;
        self.order.borrow_mut().push(id);
        self.opening.borrow_mut().push(id);
        Ok(id)
    }

    /// Revoke delivery before resuming the current callback. The App drain owns disposal.
    pub(crate) fn request_close(&self, id: WindowId, at: &'static std::panic::Location<'static>) -> bool {
        if self.check_running().is_err() || !Runtime::caller_allows(crate::reactive::graph::Operation::Write, at) {
            return false;
        }
        let Some(w) = self.windows.borrow().get(id).cloned() else { return false };
        if w.closed.replace(true) {
            return false;
        }
        w.capturing.borrow_mut().take();
        if let Some(scope) = w.root_scope.get() {
            self.rt.retire_scope_tree(scope);
        }
        self.closing.borrow_mut().push(id);
        true
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
        w.w.capturing.borrow_mut().take();
        if let Some(s) = w.w.root_scope.get() {
            w.enter(|| self.rt.dispose_scope(s));
            w.w.root_scope.set(None);
        }
        // The signals the window itself holds (no scope owns them).
        w.enter(|| {
            self.rt.settle_named_state(Owner(w.w.id.number()));
            w.w.viewport.dispose();
            w.w.breakpoint.dispose();
            w.w.interaction_revision.dispose();
            if let Some(signal) = w.w.command_availability.take() {
                signal.dispose();
            }
            w.w.release_feedback();
            for (_, c) in w.w.clocks.borrow_mut().drain() {
                c.task.cancel_owned();
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
    #[track_caller]
    pub fn headless<V: IntoView>(root: impl Fn() -> V + 'static) -> Window {
        Window::headless_on(crate::platform::HeadlessPlatform::default(), root)
    }

    /// [`headless`](Self::headless) on `platform` (a fake native host, a clipboard).
    #[track_caller]
    pub fn headless_on<V: IntoView>(platform: crate::platform::HeadlessPlatform, root: impl Fn() -> V + 'static) -> Window {
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
        self.change_focus(n, None)
    }

    fn change_focus(&self, n: Option<NodeId>, modality: Option<Modality>) -> Result<(), String> {
        if let Some(n) = n {
            let k = self.kernel();
            let name = if k.is_alive(n) { k.test_id(n).unwrap_or("element") } else { "element" };
            k.check_activation(n, self.top_modal()).map_err(|why| format!("`{name}` {}", why.message(&k)))?;
            if !k.is_focusable(n) {
                return Err(format!("`{name}` is not focusable"));
            }
        }
        if let Some(modality) = modality {
            self.set_modality(modality);
        }
        let mut i = self.w.interaction.borrow_mut();
        let mut k = self.w.kernel.borrow_mut();
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
    fn interact(&self, ev: &Event, node: Option<NodeId>) -> Result<(), String> {
        let target = || node.expect("a node event was resolved");
        match ev {
            Event::Hover(id, on) => {
                let n = target();
                if *on {
                    {
                        let k = self.kernel();
                        k.check_interaction_scope(n, self.top_modal()).map_err(|why| format!("`{id}` {}", why.message(&k)))?;
                    }
                    self.set_hover(Some(n));
                } else if self.w.interaction.borrow().hover_chain.contains(&n) {
                    self.set_hover(None);
                }
            }
            Event::Focus(_) => {
                self.change_focus(Some(target()), Some(Modality::Keyboard))?;
            }
            Event::Blur => self.set_focus(None)?,
            Event::Scroll(id, dx, dy) | Event::ScrollTo(id, dx, dy) => {
                let mut k = self.w.kernel.borrow_mut();
                let n = target();
                if !k.accepts_scroll(n, *dx, *dy) {
                    return Err(format!("`{id}` does not scroll on that axis: give it `overflow_y_scroll()` (or `overflow_x_scroll()`)"));
                }
                if matches!(ev, Event::ScrollTo(..)) {
                    k.glide_scroll(n, *dx, *dy);
                } else {
                    let (x, y) = k.scroll_offset(n);
                    k.stop_glide(n);
                    k.scroll_to(n, x + dx, y + dy);
                }
            }
            _ => unreachable!("only interaction events"),
        }
        Ok(())
    }

    fn handler_for(&self, ev: &Event, node: NodeId) -> Result<ScopedHandler, String> {
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
        k.check_activation(node, self.top_modal()).map_err(|why| format!("`{id}` {}", why.message(&k)))?;
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
        self.event(InputEvent::Resize { width, height, scale, safe_area: None });
    }
    /// Its scale: device pixels per logical pixel (what the last resize said).
    pub fn scale(&self) -> f32 {
        self.w.scale.get()
    }
    pub(crate) fn on_resize(&mut self, width: f32, height: f32, scale: f32, safe_area: Option<crate::paint::Rect>) {
        if self.size() != (width, height) || self.scale() != scale {
            self.capture_resized();
        }
        self.w.scale.set(scale);
        self.w.size.set((width, height));
        self.w.kernel.borrow_mut().set_safe_area(safe_area);
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
    /// Reads the semantic tree and app state for the Inspector protocol without settling the
    /// window or evaluating application callbacks. See [`crate::inspect`].
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
            commands: self.commands(),
            text_input,
            touch_scroll: self.w.interaction.borrow().pointer.last_touch_scroll.clone(),
            back_gesture: self.w.interaction.borrow().back.last,
            pending: self.app().pending().len(),
            window: self.w.id.number(),
            target: crate::inspect::window_target(&k),
            selection: self.selection_preview(&k),
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
            root: crate::inspect::tree_for_window(&k, self.submit_target()),
        }
    }
    /// What the current frame draws, in order (see [`crate::paint`]).
    pub fn display_list(&self) -> Rc<crate::paint::DisplayList> {
        self.with_picking_overlay(self.w.kernel.borrow().display_list())
    }

    fn with_picking_overlay(&self, mut list: Rc<crate::paint::DisplayList>) -> Rc<crate::paint::DisplayList> {
        if let Some(over) = self.picking_overlay() {
            Rc::make_mut(&mut list).parts.extend(over.into_iter().map(crate::paint::Part::Item));
        }
        list
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
        self.dispatch_semantic(ev, None)
    }

    pub(crate) fn dispatch_to(&mut self, ev: Event, node: NodeId) -> Result<(), String> {
        self.dispatch_semantic(ev, Some(node))
    }

    fn dispatch_semantic(&mut self, ev: Event, node: Option<NodeId>) -> Result<(), String> {
        self.check_running()?;
        self.app.record(self.w.id, || match node {
            Some(node) => InputEvent::SemanticAt { node, event: ev.clone() },
            None => InputEvent::Semantic(ev.clone()),
        });
        self.on_semantic(ev, node)
    }

    fn on_semantic(&mut self, ev: Event, bound: Option<NodeId>) -> Result<(), String> {
        self.check_running()?;
        if let Event::NumericInput(_, value) = &ev
            && !value.is_finite()
        {
            return Err("numeric input requires a finite value".into());
        }
        self.enter(|| {
            let node = match (ev.test_id(), bound) {
                (Some(_), Some(node)) if self.kernel().is_mounted(node) => Some(node),
                (Some(_), Some(_)) => return Err("the semantic target is no longer mounted".into()),
                (Some(id), None) => Some(self.kernel().named_node(id).map_err(|error| error.message(id))?),
                (None, Some(_)) => return Err("the semantic event targets the window".into()),
                (None, None) => None,
            };
            let cause = self.app.next_cause.get() + 1;
            if matches!(ev, Event::Hover(..) | Event::Focus(_) | Event::Blur | Event::Scroll(..) | Event::ScrollTo(..)) {
                self.w.kernel.borrow_mut().set_cause(Some(cause));
                let r = self.interact(&ev, node);
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
            let node = node.expect("a handler event resolved its target");
            let h = self.handler_for(&ev, node)?;
            self.app.next_cause.set(cause);
            self.w.kernel.borrow_mut().set_cause(Some(cause));
            if let Event::Input(_, value) = ev {
                let delivered = self.set_value_of(node, value);
                self.settle_frame();
                self.w.kernel.borrow_mut().set_cause(None);
                return delivered.then_some(()).ok_or_else(|| "input target has an unresolved edit".into());
            }
            h.run(&self.app.rt, |h| match (h, ev) {
                (Handler::Press(f), _) | (Handler::Toggle(f), _) => f(),
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
        if self.is_closed() || self.w.root_scope.get().is_none() {
            return;
        }
        self.w.capturing.borrow_mut().take();
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
            }
            if let Some(s) = self.w.root_scope.get() {
                self.app.rt.set_reloading(true);
                let _reload = std::mem::DropGuard::new(&self.app.rt, |rt| rt.set_reloading(false));
                self.app.rt.dispose_scope(s);
                self.w.root_scope.set(None);
            }
            if self.is_closed() || self.app.shutdown.get() {
                return;
            }
            self.w.root_slots.borrow_mut().clear();
            // A rebuild is not content arriving: nothing enters.
            self.w.kernel.borrow_mut().set_quiet(true);
            self.w.rebuild_pending.set(true);
            self.mount_root();
        });
    }
}

impl App {
    /// The platform whose conventions input follows (the build's by default).
    pub fn target_platform(&self) -> TargetPlatform {
        self.app.platform.get()
    }
    /// The theme every window shows.
    pub fn theme(&self) -> Theme {
        self.app.res.theme.borrow().clone()
    }
    /// Rebuilds every window's scope from the current state, as after a code patch. Named state
    /// survives for owner paths present in the rebuilt tree, even if a handler looks it up
    /// later. State under absent paths is released after rebuilding; closing a window releases
    /// all of its retained state. Rejected during construction, pure computations and shutdown.
    #[track_caller]
    pub fn hot_reload(&self) {
        use crate::reactive::graph::Operation;

        let at = std::panic::Location::caller();
        // Check the caller before restoring the destination App's execution context.
        if !Runtime::caller_allows(Operation::Write, at) {
            return;
        }
        if self.app.shutdown.get() {
            self.app.rt.diag(&codes::WRITE_DISPOSED, "cannot hot reload a shut down App".into(), at);
            return;
        }
        self.enter(|| {
            if !self.app.rt.allows(Operation::Write, at) {
                return;
            }
            for w in self.each() {
                w.reload();
            }
            if self.app.depth.get() == 1 {
                self.app.frame(None, at);
            }
        });
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
