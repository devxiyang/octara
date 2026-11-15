//! The app runtime: mounts a root view, dispatches events, lays out, and rebuilds for hot reload.

mod asks;
mod behavior;
mod drive;
mod edit;
mod event;
mod focus;
mod guard;
mod input;
mod keys;
mod pick;
mod pointer;
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
    settled_generation: Cell<u64>,
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
    if app.depth.get() == 0 && !app.checking.get() {
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
    if app.depth.get() == 0 {
        app.check_consistency();
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

impl AppState {
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
            size: Cell::new(size),
            scale: Cell::new(1.0),
            platform: RefCell::new(platform),
            text_input: RefCell::new(None),
            cursor: Cell::new(crate::platform::CursorStyle::Arrow),
            native_sent: RefCell::default(),
            native_changes: RefCell::default(),
            presented: Cell::new(u64::MAX),
            settled_generation: Cell::new(u64::MAX),
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
    /// cancels tasks. Repeated calls do nothing. Closing the last window alone does not end it.
    /// Hosts must call this before exiting; dropping Rc handles is not a shutdown protocol.
    #[track_caller]
    pub fn shutdown(&self) {
        let at = std::panic::Location::caller();
        self.enter(|| {
            if self.app.shutdown.get() {
                return;
            }
            if !self.app.rt.write_allowed(at) || !self.app.rt.effect_allowed(at) {
                return;
            }
            self.app.shutdown.set(true);
            // Close the gate before callbacks run. Even if user cleanup unwinds, stop the
            // scheduler rather than allowing late wakes to resume partially retired work.
            struct Stop<'a>(&'a crate::task::Scheduler);
            impl Drop for Stop<'_> {
                fn drop(&mut self) {
                    self.0.shutdown();
                }
            }
            let _stop = Stop(&self.app.sched);
            let windows = self.app.order.borrow().clone();
            for id in windows {
                self.app.close(id);
            }
            self.app.opening.borrow_mut().clear();
            self.app.closing.borrow_mut().clear();
            self.app.rt.dispose_scope(self.app.root_scope);
        });
    }

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
        if self.shutdown.get() {
            return Err("App has shut down".into());
        }
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
                .any(|w| !w.closed.get() && w.settled_generation.get() != w.kernel.borrow().mutation_generation());
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

    /// A frame for the whole app: every window settles (shared state may have changed what any
    /// of them shows), then windows asked for open and windows asked to close go.
    fn frame(self: &Rc<Self>, at: &'static std::panic::Location<'static>) {
        self.frame_except(None, at);
    }

    /// [`frame`](Self::frame), retaining the generation of a window that just settled. A later
    /// window's measurements or callbacks can invalidate an earlier window, including this one.
    fn frame_except(self: &Rc<Self>, settled: Option<(WindowId, u64)>, at: &'static std::panic::Location<'static>) {
        if self.shutdown.get() {
            return;
        }
        enter_app(self, || {
            let mut generations = rustc_hash::FxHashMap::default();
            generations.extend(settled);
            for _ in 0..16 {
                for id in self.order.borrow().clone() {
                    if let Some(w) = self.handle(id) {
                        if generations.get(&id) == Some(&w.w.kernel.borrow().mutation_generation()) {
                            continue;
                        }
                        let generation = w.enter(|| w.settle_window(at));
                        generations.insert(id, generation);
                    }
                }
                // The last window's focus or hover cleanup can queue reactive work too.
                self.rt.flush();
                let opening = std::mem::take(&mut *self.opening.borrow_mut());
                let closing = std::mem::take(&mut *self.closing.borrow_mut());
                if opening.is_empty() && closing.is_empty() {
                    let invalidated = self.order.borrow().iter().any(|id| {
                        self.handle(*id).is_some_and(|w| generations.get(id) != Some(&w.w.kernel.borrow().mutation_generation()))
                    });
                    if !invalidated {
                        self.present_menus();
                        return;
                    }
                }
                for id in opening {
                    if let Some(w) = self.handle(id) {
                        w.enter(|| w.mount_root());
                    }
                }
                for id in closing {
                    self.close(id);
                }
            }
            // Keep every window's invalidation intact; another operation can settle it once the
            // feedback stops. Do not schedule an immediate retry of the same runaway frame.
            self.rt.diag(
                &codes::LAYOUT_LOOP,
                "window updates did not settle in 16 rounds: windows keep changing each other's layout or presentation".into(),
                at,
            );
            self.present_menus();
        });
    }

    /// Closes window `id`: its scope is disposed, it leaves the app, its platform window is let
    /// go when the last handle on it goes.
    fn close(self: &Rc<Self>, id: WindowId) {
        let Some(w) = self.handle(id) else { return };
        if let Some(s) = w.w.root_scope.take() {
            w.enter(|| self.rt.dispose_scope(s));
        }
        // The signals the window itself holds (no scope owns them).
        w.enter(|| {
            w.w.viewport.dispose();
            w.w.breakpoint.dispose();
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
        w.w.closed.set(true);
        w.w.interaction.borrow_mut().after_update.clear();
        w.w.root_slots.borrow_mut().clear();
        *w.w.platform.borrow_mut() = Box::new(crate::platform::HeadlessWindow::default());
        self.order.borrow_mut().retain(|x| *x != id);
        self.windows.borrow_mut().remove(id);
    }
}

impl Window {
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

    fn mount_root(&self) {
        // Named by the window: state is kept by its scope's path (across a hot reload), so two
        // windows' roots never share a key.
        let s = self.app.rt.create_scope(Some(self.app.root_scope), &format!("window{}", self.w.id.number()));
        self.w.root_scope.set(Some(s));
        let parent = Parent { node: self.w.kernel.borrow().root(), root: self.w.root_slots.clone() };
        let root = self.w.root.clone();
        self.app.rt.with_frame(Ctx::Build, None, Some(Some(s)), || {
            mount(root(), &parent, &self.w.root_slots);
            // What the window draws of its requests, above the app's view.
            mount(asks::layer(), &parent, &self.w.root_slots);
        });
        self.app.rt.flush();
        self.restore_interaction();
        self.settle_frame();
    }

    /// Applies pending updates and lays out until nothing changes: after a layout, virtual
    /// lists measure their rows and scroll containers report their state, which may change the
    /// view again. Gives up after a few rounds and reports `LAYOUT_LOOP`.
    /// This window's frame, then the app's: the other windows settle (shared state may have
    /// changed them) and windows asked for open or close.
    #[track_caller]
    fn settle_frame(&self) {
        let at = std::panic::Location::caller();
        let generation = self.settle_window(at);
        self.app.frame_except(Some((self.w.id, generation)), at);
    }

    /// One window's part of a frame: tasks, layout, native views, text input, focus and hover.
    fn settle_window(&self, at: &'static std::panic::Location<'static>) -> u64 {
        // Tasks woken (or spawned) by what just happened run to their next wait first; what they
        // change is laid out with the rest. They may wake or spawn more, so repeat (bounded).
        let mut command_ran = false;
        for _ in 0..16 {
            if self.app.shutdown.get() || self.w.closed.get() || self.app.closing.borrow().contains(&self.w.id) {
                self.w.interaction.borrow_mut().after_update.clear();
                return self.w.kernel.borrow().mutation_generation();
            }
            self.app.sched.run_ready();
            self.settle_layout(at);
            // Modals that opened or closed move focus (which restyles).
            if self.sync_modals() {
                self.settle_layout(at);
            }
            if self.settle_requested_focus(at) || self.reconcile_focus() {
                // Focus callbacks can change visibility, open a modal or close the window.
                // Let those changes land before any queued command sees its focus path.
                command_ran = false;
                continue;
            }
            // Route one command, then let its updates and focus changes settle before the
            // next. This uses the same round bound as tasks and never nests an input frame.
            let command = self.w.interaction.borrow_mut().after_update.pop_front();
            if let Some((command, source)) = command {
                command_ran = self.run_after_update_command(&command, source);
            } else {
                command_ran = false;
            }
            if !command_ran && !self.app.sched.has_ready() && self.w.interaction.borrow().after_update.is_empty() {
                break;
            }
        }
        // The last command may have changed state even when it queued no successor.
        if command_ran {
            self.settle_layout(at);
            if self.sync_modals() {
                self.settle_layout(at);
            }
            self.settle_requested_focus(at);
            self.reconcile_focus();
        }
        let pending = std::mem::take(&mut self.w.interaction.borrow_mut().after_update);
        if let Some((command, source)) = pending.front() {
            self.app.rt.diag(
                &codes::COMMAND_QUEUE_LIMIT,
                format!(
                    "deferred commands did not settle in 16 rounds; discarded {} pending command(s), starting with `{command}`",
                    pending.len()
                ),
                source,
            );
        }
        // A caret that moved shows: the scroll containers around its field follow it.
        let reveal = self.w.interaction.borrow_mut().reveal.take();
        if let Some(n) = reveal {
            let caret = self.w.kernel.borrow().caret(n);
            if let Some(r) = caret
                && self.w.kernel.borrow_mut().reveal(n, r)
            {
                self.settle_layout(at);
            }
        }
        // Highlighting an option can reveal it without moving keyboard focus from its field.
        let request = self.w.interaction.borrow_mut().reveal_request.take();
        if let Some(id) = request {
            let mut k = self.w.kernel.borrow_mut();
            let moved = k.find(&id).filter(|n| k.is_shown(*n)).is_some_and(|n| {
                let (x, y, w, h) = k.shown_rect(n);
                k.reveal(n, crate::paint::Rect { x, y, w, h })
            });
            drop(k);
            if moved {
                self.settle_layout(at);
            }
        }
        self.sync_native();
        self.present();
        let generation = self.w.kernel.borrow().mutation_generation();
        self.w.settled_generation.set(generation);
        // An `instant` transaction covers the frame it was made in.
        self.w.kernel.borrow_mut().set_instant(false);
        // What is on its way out can no longer be focused or hovered.
        let (focused, hovered) = {
            let i = self.w.interaction.borrow();
            (i.focused, i.hovered)
        };
        let gone = |n: Option<NodeId>| n.is_some_and(|n| !self.w.kernel.borrow().is_shown(n));
        if gone(focused) {
            self.reconcile_focus();
        }
        // What leaves the tree lets go of hover at once (even while hover holds still).
        if gone(hovered) {
            self.set_hover(None);
        }
        generation
    }

    fn settle_requested_focus(&self, at: &'static std::panic::Location<'static>) -> bool {
        let request = self.w.interaction.borrow_mut().focus_request.take();
        if let Some(id) = request {
            let node = self.w.kernel.borrow().find(&id);
            if let Some(node) = node
                && self.set_focus(Some(node)).is_ok()
            {
                self.settle_layout(at);
            }
            return true;
        }
        false
    }

    fn settle_layout(&self, at: &'static std::panic::Location<'static>) {
        for _ in 0..4 {
            self.app.rt.flush();
            self.w.kernel.borrow_mut().layout(self.size().0, self.size().1);
            let changed = self.app.rt.with_frame(Ctx::Handler, None, Some(None), || self.w.sync_after_layout());
            if !changed {
                return;
            }
        }
        self.app.rt.flush();
        self.w.kernel.borrow_mut().layout(self.size().0, self.size().1);
        self.app.rt.diag(
            &codes::LAYOUT_LOOP,
            "layout did not settle in 4 rounds: something keeps changing what it measures (a size read feeding the size it reads)".into(),
            at,
        );
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
            if !k.is_shown(n) {
                return Err(format!("`{name}` is hidden or leaving the tree"));
            }
            if modal.is_some_and(|modal| !k.is_in_modal_scope(n, modal)) {
                return Err(format!("`{name}` is behind the open modal"));
            }
            if !k.is_focusable(n) {
                return Err(format!("`{name}` is not focusable"));
            }
            if k.state(n).contains(State::DISABLED) {
                return Err(format!("`{name}` is disabled"));
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
        // What lost focus hears it first, then what got it.
        if let Some(p) = blurred {
            self.run(p, &[HandlerKind::Blur], None);
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
        if k.state(node).contains(State::DISABLED) {
            return Err(format!("`{id}` is disabled"));
        }
        if kind == HandlerKind::NumericInput && (!k.is_shown(node) || self.live(Some(node)) != Some(node)) {
            return Err(format!("`{id}` is hidden, leaving or outside the active modal"));
        }
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
            selection: self.selected_text().map(|t| t.chars().take(2000).collect()),
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
        self.app.record(self.w.id, &InputEvent::Semantic(ev.clone()));
        self.on_semantic(ev)
    }
    pub(crate) fn on_semantic(&mut self, ev: Event) -> Result<(), String> {
        if self.app.shutdown.get() || self.w.closed.get() {
            return Err("window is closed".into());
        }
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
            if let Some(s) = self.w.root_scope.take() {
                self.app.rt.set_reloading(true);
                self.app.rt.dispose_scope(s);
                self.app.rt.set_reloading(false);
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
        self.app.rt.diagnostics()
    }
    /// Each stored diagnostic's code with how many times it occurred (repeats are not stored).
    pub fn diagnostic_counts(&self) -> Vec<(&'static str, u32)> {
        self.app.rt.diagnostic_counts()
    }
    pub fn work(&self) -> Work {
        Work { closures_run: self.app.rt.closures_run(), reactive_nodes: self.app.rt.live_nodes() }
    }
    pub fn reset_work(&self) {
        self.app.rt.reset_closures_run();
    }
}

// ---- native views ----

impl Window {
    /// Tells the native host what changed this frame: views created (with their props), props
    /// changed, placements changed, views gone.
    fn sync_native(&self) {
        let k = self.w.kernel.borrow();
        let mut sent = self.w.native_sent.borrow_mut();
        if k.native_nodes().is_empty() && sent.is_empty() {
            return;
        }
        let window = self.w.platform.borrow();
        let mut changes = self.w.native_changes.borrow_mut();
        let placements = k.native_placements();
        let mut moved = Vec::new();
        let mut live = Vec::new();
        for (n, p) in &placements {
            let Some(v) = k.native(*n) else { continue };
            match sent.get_mut(n) {
                None => {
                    live.push((*n, window.shows_native(&v.kind, v.slot)));
                    changes.push(crate::native::NativeChange::Create {
                        id: p.id,
                        kind: v.kind.clone(),
                        slot: v.slot,
                        props: v.props.clone(),
                    });
                    sent.insert(*n, (v.version, None));
                }
                Some((version, _)) if *version != v.version => {
                    changes.push(crate::native::NativeChange::Update { id: p.id, props: v.props.clone() });
                    *version = v.version;
                }
                Some(_) => {}
            }
            let entry = sent.get_mut(n).expect("sent");
            if entry.1.as_ref() != Some(p) {
                entry.1 = Some(p.clone());
                moved.push(p.clone());
            }
        }
        if !moved.is_empty() {
            changes.push(crate::native::NativeChange::Place { placements: moved });
        }
        let mounted: Vec<NodeId> = placements.iter().map(|(n, _)| *n).collect();
        sent.retain(|n, _| {
            let keep = mounted.contains(n);
            if !keep {
                changes.push(crate::native::NativeChange::Destroy { id: crate::native::NativeId(crate::kernel::a11y::a11y_id(*n).0) });
            }
            keep
        });
        drop((k, window, sent, changes));
        // Painted next as the platform shows them: no placeholder, a cutout for the underlay.
        for (n, shown) in live {
            self.w.kernel.borrow_mut().set_native_live(n, shown);
        }
    }

    /// The node of a native view's id, if it still exists.
    fn native_node(&self, id: crate::native::NativeId) -> Option<NodeId> {
        let k = self.w.kernel.borrow();
        k.node_of_a11y(accesskit::NodeId(id.0)).filter(|n| k.native(*n).is_some())
    }

    /// An event from native view `id` (`InputEvent::Native`): runs its `on_native`.
    /// Files dragged from the system: the element under them that takes them (`on_drop`, the
    /// nearest at or above the point) is `DRAG_OVER`; dropped, it gets their paths.
    pub(crate) fn on_file_drag(&mut self, ev: crate::app::FileDrag) -> bool {
        let target = |x: f32, y: f32| {
            let k = self.w.kernel.borrow();
            k.hit_test(x, y).and_then(|n| k.handler_above(Some(n), HandlerKind::Drop))
        };
        let (over, dropped) = match ev {
            crate::app::FileDrag::Over { x, y, .. } => (target(x, y), None),
            crate::app::FileDrag::Drop { x, y, paths } => (None, target(x, y).map(|t| (t, paths))),
            crate::app::FileDrag::Leave => (None, None),
        };
        let handler = dropped.as_ref().and_then(|(t, _)| {
            let k = self.w.kernel.borrow();
            k.handler(*t, HandlerKind::Drop).and_then(|h| self.w.handlers.borrow().get(&h.0).cloned())
        });
        self.input_frame(|| {
            let was = std::mem::replace(&mut self.w.interaction.borrow_mut().drop_target, over);
            if was != over {
                let mut k = self.w.kernel.borrow_mut();
                if let Some(n) = was.filter(|n| k.is_alive(*n)) {
                    k.set_state(n, State::DRAG_OVER, false);
                }
                if let Some(n) = over {
                    k.set_state(n, State::DRAG_OVER, true);
                }
            }
            if let (Some(handler), Some((_, paths))) = (handler, &dropped) {
                handler.run(&self.app.rt, |handler| {
                    if let Handler::Drop(f) = handler {
                        f(paths);
                    }
                });
            }
        });
        true
    }

    pub(crate) fn on_native_event(&mut self, id: crate::native::NativeId, event: crate::native::NativeEvent) -> bool {
        let Some(n) = self.native_node(id) else { return false };
        let handler = {
            let k = self.w.kernel.borrow();
            k.handler(n, HandlerKind::Native).and_then(|h| self.w.handlers.borrow().get(&h.0).cloned())
        };
        let Some(handler) = handler else { return false };
        self.input_frame(|| {
            handler.run(&self.app.rt, |handler| {
                if let Handler::Native(f) = handler {
                    f(&event);
                }
            });
        });
        true
    }

    /// The natural size native view `id` reports (`InputEvent::NativeSize`): used where no size
    /// is authored.
    pub(crate) fn on_native_size(&mut self, id: crate::native::NativeId, width: f32, height: f32) -> bool {
        let Some(n) = self.native_node(id) else { return false };
        self.input_frame(|| self.w.kernel.borrow_mut().set_native_size(n, (width, height)));
        true
    }

    /// What keeps the app from being settled now: tasks, timers and motion (not subscriptions,
    /// which run for good), pending reactive updates, and native views that are busy (a page
    /// loading). A capture waits for this to be empty.
    pub fn unsettled(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .app()
            .pending()
            .into_iter()
            .filter(|p| p.kind != crate::task::WorkKind::Subscription)
            .map(|p| if p.at.is_empty() { p.label } else { format!("{} ({})", p.label, p.at) })
            .collect();
        if self.app.rt.has_pending() {
            out.push("reactive updates are still pending".into());
        }
        if !self.w.closed.get() && self.w.settled_generation.get() != self.w.kernel.borrow().mutation_generation() {
            out.push("window updates are still pending".into());
        }
        let placements = self.w.kernel.borrow().native_placements();
        let window = self.w.platform.borrow();
        for (n, p) in placements.into_iter().filter(|(_, p)| p.visible) {
            if let Some(what) = window.native_busy(p.id) {
                out.push(format!("{}: {what}", self.native_name(n)));
            }
        }
        out
    }

    /// A native view as reports name it: its test id, else its kind.
    fn native_name(&self, n: NodeId) -> String {
        let k = self.w.kernel.borrow();
        let kind = k.native(n).map(|v| v.kind.clone()).unwrap_or_default();
        match k.test_id(n) {
            Some(id) => format!("{kind} view #{id}"),
            None => format!("{kind} view"),
        }
    }

    /// The topmost element at `(x, y)` (logical pixels), with its number, test id, source and
    /// the elements around it (the inspector's `pick`).
    pub fn pick(&self, x: f32, y: f32) -> Option<crate::inspect::Picked> {
        crate::inspect::pick(&self.w.kernel.borrow(), x, y)
    }

    /// Starts a capture: asks the platform window for a snapshot of every native view shown.
    /// Returns the views whose snapshot is coming (as [`InputEvent::NativeSnapshot`]s).
    pub fn capture_begin(&mut self) -> Vec<crate::native::NativeId> {
        let placements = self.w.kernel.borrow().native_placements();
        let mut window = self.w.platform.borrow_mut();
        let asked: Vec<_> = placements.into_iter().filter(|(_, p)| p.visible).map(|(_, p)| (p.id, window.snapshot_native(p.id))).collect();
        let coming = asked.iter().filter(|(_, r)| r.is_ok()).map(|(id, _)| *id).collect();
        *self.w.capturing.borrow_mut() = Some(crate::capture::Capturing { asked });
        coming
    }

    /// The snapshots asked for that have not come yet.
    pub fn capture_awaiting(&self) -> Vec<crate::native::NativeId> {
        let capturing = self.w.capturing.borrow();
        let Some(c) = capturing.as_ref() else { return Vec::new() };
        let k = self.w.kernel.borrow();
        c.asked
            .iter()
            .filter(|(id, r)| r.is_ok() && self.native_node(*id).is_some_and(|n| k.native(n).is_some_and(|v| v.snapshot.is_none())))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Ends a capture: paints the frame (snapshots in their views' places) with `painter`,
    /// writes it to `path` as a PNG, clears the snapshots, and reports. `pending` is what was
    /// still running when the wait ended ([`unsettled`](Self::unsettled)); `fallback` why a
    /// `display` capture was asked and this painted one given.
    pub fn capture_finish(
        &mut self,
        painter: &mut dyn crate::capture::Painter,
        path: &str,
        scale: f32,
        pending: Vec<String>,
        fallback: Option<String>,
    ) -> Result<crate::capture::Capture, String> {
        use crate::capture::{NativeShot, Shown};
        let asked = self.w.capturing.borrow_mut().take().map(|c| c.asked).unwrap_or_default();
        let mut native = Vec::new();
        for (id, result) in &asked {
            let Some(n) = self.native_node(*id) else { continue };
            let k = self.w.kernel.borrow();
            let Some(v) = k.native(n) else { continue };
            let (x, y, w, h) = k.rect(n);
            let (shown, reason, misses) = match result {
                Ok(misses) if v.snapshot.is_some() => (Shown::Snapshot, None, misses.clone()),
                Ok(misses) => (Shown::Failed, Some("its snapshot did not come in time".to_owned()), misses.clone()),
                Err(why) => (Shown::Placeholder, Some(why.clone()), Vec::new()),
            };
            native.push(NativeShot {
                id: k.test_id(n).map(str::to_owned),
                kind: v.kind.clone(),
                rect: [x, y, w, h],
                shown,
                reason,
                misses,
            });
        }
        let painted = painter.paint(&self.display_list(), scale).and_then(|p| p.save_png(path).map(|()| p));
        for (id, _) in &asked {
            self.on_native_snapshot(*id, None);
        }
        let picture = painted?;
        Ok(crate::capture::Capture {
            path: path.to_owned(),
            width: picture.width,
            height: picture.height,
            scale,
            pixels: crate::capture::Pixels::Painted,
            fallback,
            renderer: painter.renderer().to_owned(),
            settled: pending.is_empty(),
            pending,
            native,
        })
    }

    /// A `display` capture: the window server's `picture` of the window, written to `path` as a
    /// PNG, with the native views reported live.
    pub fn capture_display(
        &mut self,
        picture: &crate::capture::Picture,
        path: &str,
        scale: f32,
        pending: Vec<String>,
    ) -> Result<crate::capture::Capture, String> {
        use crate::capture::{NativeShot, Shown};
        picture.save_png(path)?;
        let placements = self.w.kernel.borrow().native_placements();
        let k = self.w.kernel.borrow();
        let native = placements
            .into_iter()
            .filter(|(_, p)| p.visible)
            .filter_map(|(n, p)| {
                let v = k.native(n)?;
                let r = p.rect;
                Some(NativeShot {
                    id: k.test_id(n).map(str::to_owned),
                    kind: v.kind.clone(),
                    rect: [r.x, r.y, r.w, r.h],
                    shown: Shown::Live,
                    reason: None,
                    misses: Vec::new(),
                })
            })
            .collect();
        Ok(crate::capture::Capture {
            path: path.to_owned(),
            width: picture.width,
            height: picture.height,
            scale,
            pixels: crate::capture::Pixels::Display,
            fallback: None,
            renderer: "display".to_owned(),
            settled: pending.is_empty(),
            pending,
            native,
        })
    }

    /// An image of what native view `id` shows (`InputEvent::NativeSnapshot`): drawn in its place
    /// instead of the placeholder, until `None` clears it.
    pub(crate) fn on_native_snapshot(&mut self, id: crate::native::NativeId, image: Option<crate::image::Image>) -> bool {
        let Some(n) = self.native_node(id) else { return false };
        self.input_frame(|| self.w.kernel.borrow_mut().set_native_snapshot(n, image));
        true
    }

    /// Read the backend adapter (downcast with `as_any`; headless, a
    /// [`HeadlessWindow`](crate::platform::HeadlessWindow)). Mutation belongs to the runtime
    /// and backend; tests configure a headless host before opening its window.
    pub fn platform_window(&self) -> std::cell::Ref<'_, dyn crate::platform::PlatformWindow> {
        std::cell::Ref::map(self.w.platform.borrow(), |w| &**w)
    }

    /// Tells the window's platform about the focused text field when it changed: where its
    /// caret is, whether it is secure or multi-line.
    /// How many frames the window presented to its platform window (each operation that changed
    /// something presents one).
    pub fn frames_presented(&self) -> u64 {
        self.w.frames.get()
    }

    /// Whether the window is the active one (key on macOS; GPUI's `is_window_active`): its
    /// focused field shows a caret. Headless windows are active.
    pub fn is_active(&self) -> bool {
        self.w.kernel.borrow().window_active()
    }

    /// How the pointer looks now: over the element it is on (an arrow off the window).
    pub fn cursor(&self) -> crate::platform::CursorStyle {
        let hovered = self.w.interaction.borrow().hovered;
        let k = self.w.kernel.borrow();
        hovered.filter(|n| k.is_alive(*n)).map_or(crate::platform::CursorStyle::Arrow, |n| k.cursor_over(n))
    }

    /// Presents what changed to the window's platform: native view changes, the text input,
    /// whether to draw again (after every settle; nothing when nothing changed).
    fn present(&self) {
        self.mark_selection();
        let text_input = self.focused_input().and_then(|(n, _)| {
            let k = self.w.kernel.borrow();
            let caret = k.caret(n)?;
            let secure = k.is_secure(n);
            let document = (!secure).then(|| k.text_document(n)).flatten();
            let (geometry, layout) = if secure {
                (None, None)
            } else {
                let field = k.field(n)?;
                let layout = k.input_layout(n, &field);
                (
                    Some(crate::platform::TextGeometry {
                        origin: (field.x - field.scroll.0, field.y - field.scroll.1),
                        lines: field.lines,
                    }),
                    Some(layout),
                )
            };
            Some(crate::platform::TextInput { id: n, caret, secure, multiline: k.is_multiline(n), document, geometry, layout })
        });
        let generation = self.w.kernel.borrow().paint_generation();
        let (picking, picked) = self.picking_frame();
        let redraw = picking || generation != self.w.presented.get() || self.w.kernel.borrow_mut().in_motion();
        let native = std::mem::take(&mut *self.w.native_changes.borrow_mut());
        let cursor = self.cursor();
        let asks = std::mem::take(&mut *self.w.asks_new.borrow_mut());
        let withdrawn = std::mem::take(&mut *self.w.asks_withdrawn.borrow_mut());
        if native.is_empty()
            && !redraw
            && picked.is_none()
            && text_input == *self.w.text_input.borrow()
            && cursor == self.w.cursor.get()
            && asks.is_empty()
            && withdrawn.is_empty()
        {
            return;
        }
        *self.w.text_input.borrow_mut() = text_input.clone();
        self.w.cursor.set(cursor);
        self.w.presented.set(generation);
        self.w.frames.set(self.w.frames.get() + 1);
        self.w.platform.borrow_mut().present(crate::platform::Frame { native, text_input, redraw, cursor, picked, asks, withdrawn });
    }
}

impl App {
    /// Installs what paints `capture` requests headless (`octara-render`'s reference renderer).
    pub fn set_painter(&self, painter: Box<dyn crate::capture::Painter>) {
        *self.app.painter.borrow_mut() = Some(painter);
    }

    /// Takes the installed painter out while it paints (see [`set_painter`](Self::set_painter)).
    pub(crate) fn take_painter(&self) -> Option<Box<dyn crate::capture::Painter>> {
        self.app.painter.borrow_mut().take()
    }

    /// Read the system adapter (downcast with `as_any`; headless, a
    /// [`HeadlessPlatform`](crate::platform::HeadlessPlatform)). Clipboard operations use
    /// [`crate::view::read_from_clipboard`] / [`crate::view::write_to_clipboard`] inside
    /// [`Self::run`] or a handler; open windows through [`Self::open_window`].
    pub fn platform(&self) -> std::cell::Ref<'_, dyn crate::platform::Platform> {
        std::cell::Ref::map(self.app.system.borrow(), |p| &**p)
    }
}
