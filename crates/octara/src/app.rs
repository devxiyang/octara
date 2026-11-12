//! The app runtime: mounts a root view, dispatches events, lays out, and rebuilds for hot reload.

mod behavior;
mod drive;
mod edit;
mod event;
mod focus;
mod input;
mod keys;
mod pick;
mod pointer;
mod time;

pub use edit::{TextField, byte_offset, utf16_offset};
pub use event::{InputEvent, Recorded, Recording};
pub use keys::{BUILTIN_COMMANDS, Binding, Chord, Key, KeyEvent, Keymap};
pub use pointer::{Button, Drag, DragPhase, Modifiers, Phase, PointerEvent, PointerKind, Tuning};

use crate::diagnostics::{self as codes, Diagnostic};
use crate::kernel::{Breakpoint, HandlerKind, Kernel, NodeId, State, Theme};
use crate::reactive::graph::Signal;
use crate::reactive::graph::{Ctx, Owner, Runtime, ScopeId};
use crate::view::{Handler, Parent, Slots, View, mount};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;

/// The window size an app starts with; change it with [`Window::resize`].
pub const DEFAULT_VIEWPORT: (f32, f32) = (800.0, 600.0);

/// An input event, addressed by test id (or by name for external events).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Press(String),
    Input(String, String),
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
    pub(crate) rt: Rc<Runtime>,
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
    keymap: RefCell<keys::Keymap>,
    /// The system the app runs on (clipboard, waking the loop, windows).
    system: RefCell<Box<dyn crate::platform::Platform>>,
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
    pub(crate) handlers: RefCell<rustc_hash::FxHashMap<u64, Handler>>,
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
    text_input: Cell<Option<crate::platform::TextInput>>,
    /// What the platform was last told of each native view: its props version and placement.
    native_sent: RefCell<rustc_hash::FxHashMap<NodeId, (u64, Option<crate::native::Placement>)>>,
    /// Native changes not presented yet.
    native_changes: RefCell<Vec<crate::native::NativeChange>>,
    /// The paint generation last presented.
    presented: Cell<u64>,
    /// A capture in progress: the snapshots asked for.
    capturing: RefCell<Option<crate::capture::Capturing>>,
    /// Picking an element to hand to an agent.
    picking: RefCell<pick::Picking>,
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

fn enter_app<R>(app: &Rc<AppState>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Rc<AppState>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let prev = self.0.take();
            APP.with(|a| *a.borrow_mut() = prev);
        }
    }
    let r = {
        let _restore = Restore(APP.with(|a| a.replace(Some(app.clone()))));
        app.depth.set(app.depth.get() + 1);
        let r = app.rt.enter(f);
        app.depth.set(app.depth.get() - 1);
        r
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

/// Runtime-owned interaction states. A hot reload carries them over by test id.
#[derive(Default)]
struct Interaction {
    /// The node under the mouse; it and its ancestors (`hover_chain`) are hovered.
    hovered: Option<NodeId>,
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
    reload: (Option<String>, Option<String>),
    forced: Vec<(String, State, bool)>,
    /// Scroll offsets by test id, carried over a hot reload.
    scrolls: Vec<(String, (f32, f32))>,
    /// A text field whose caret moved: scroll containers around it bring the caret into view
    /// once the frame is laid out.
    reveal: Option<NodeId>,
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
        WindowState {
            id,
            kernel: RefCell::new(Kernel::new(app.res.clone())),
            handlers: RefCell::default(),
            commands: RefCell::default(),
            overlay_slots: Slots::default(),
            viewport: app.rt.enter(|| Signal::detached(size)),
            breakpoint: app.rt.enter(|| Signal::detached(Theme::default().breakpoint_of(size.0))),
            scrolls: RefCell::default(),
            sizes: RefCell::default(),
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
            text_input: Cell::new(None),
            native_sent: RefCell::default(),
            native_changes: RefCell::default(),
            presented: Cell::new(u64::MAX),
            capturing: RefCell::default(),
            picking: RefCell::default(),
        }
    }
}

/// The offset a scroll event starts from: the current one for `Scroll`, the origin for `ScrollTo`.
fn n_offset(k: &Kernel, n: crate::kernel::NodeId, relative: bool) -> (f32, f32) {
    if relative { k.scroll_offset(n) } else { (0.0, 0.0) }
}

/// States the runtime drives; `SELECTED` and `DISABLED` come from element props.
const RUNTIME_STATES: State = State::HOVER.union(State::FOCUSED).union(State::FOCUS_VISIBLE).union(State::PRESSED);

impl App {
    /// An app on `platform` (a backend's; `HeadlessPlatform` for tests and agents), with no
    /// window yet.
    pub fn new(platform: impl crate::platform::Platform + 'static) -> App {
        let state = AppState {
            rt: Runtime::new(),
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
            system: RefCell::new(Box::new(platform)),
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
        self.app.closing.borrow_mut().push(id);
        self.app.frame(std::panic::Location::caller());
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

    /// Runs `f` in the app's runtime, outside any window: makes or reads state the windows
    /// share (a signal made here belongs to the app), then lets every window settle.
    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        let r = self.enter(f);
        self.app.frame(std::panic::Location::caller());
        r
    }
}

impl AppState {
    /// Makes window `id` with its platform window; `root` is mounted by the caller.
    fn insert_window(&self, options: crate::platform::WindowOptions, root: Rc<dyn Fn() -> View>) -> Result<WindowId, String> {
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
        if !self.check.get() || self.checking.replace(true) {
            return;
        }
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
            if w.w.breakpoint.get_untracked() != bp {
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
        self.closing.borrow_mut().push(id);
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

    /// [`frame`](Self::frame) for every window but `settled` (which just settled).
    fn frame_except(self: &Rc<Self>, settled: Option<WindowId>, at: &'static std::panic::Location<'static>) {
        let mut skip = settled;
        loop {
            for id in self.order.borrow().clone() {
                if skip == Some(id) {
                    continue;
                }
                if let Some(w) = self.handle(id) {
                    w.enter(|| w.settle_window(at));
                }
            }
            let opening = std::mem::take(&mut *self.opening.borrow_mut());
            let closing = std::mem::take(&mut *self.closing.borrow_mut());
            if opening.is_empty() && closing.is_empty() {
                return;
            }
            skip = None;
            for id in opening {
                if let Some(w) = self.handle(id) {
                    w.enter(|| w.mount_root());
                }
            }
            for id in closing {
                self.close(id);
            }
        }
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
        });
        w.w.closed.set(true);
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
        let s = self.app.rt.create_scope(None, "app");
        self.w.root_scope.set(Some(s));
        let parent = Parent { node: self.w.kernel.borrow().root(), root: self.w.root_slots.clone() };
        let root = self.w.root.clone();
        self.app.rt.with_frame(Ctx::Build, None, Some(Some(s)), || mount(root(), &parent, &self.w.root_slots));
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
        self.settle_window(at);
        self.app.frame_except(Some(self.w.id), at);
    }

    /// One window's part of a frame: tasks, layout, native views, text input, focus and hover.
    fn settle_window(&self, at: &'static std::panic::Location<'static>) {
        // Tasks woken (or spawned) by what just happened run to their next wait first; what they
        // change is laid out with the rest. They may wake or spawn more, so repeat (bounded).
        for _ in 0..16 {
            self.app.sched.run_ready();
            self.settle_layout(at);
            // Modals that opened or closed move focus (which restyles).
            if self.sync_modals() {
                self.settle_layout(at);
            }
            if !self.app.sched.has_ready() {
                break;
            }
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
        self.sync_native();
        self.present();
        // An `instant` transaction covers the frame it was made in.
        self.w.kernel.borrow_mut().set_instant(false);
        // What is on its way out can no longer be focused or hovered.
        let (focused, hovered) = {
            let i = self.w.interaction.borrow();
            (i.focused, i.hovered)
        };
        let gone = |n: Option<NodeId>| n.is_some_and(|n| !self.w.kernel.borrow().is_alive(n) || self.w.kernel.borrow().is_exiting(n));
        if gone(focused) {
            let _ = self.set_focus(None);
        }
        // What leaves the tree lets go of hover at once (even while hover holds still).
        if gone(hovered) {
            self.set_hover(None);
        }
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
        let hovered = hover.and_then(|id| k.find(&id));
        i.hovered = hovered;
        i.hover_chain = k.hover_chain(hovered);
        for n in i.hover_chain.clone() {
            k.set_state(n, State::HOVER, true);
        }
        i.focused = focus.and_then(|id| k.find(&id));
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
        let mut i = self.w.interaction.borrow_mut();
        let mut k = self.w.kernel.borrow_mut();
        if let Some(n) = n {
            let name = k.test_id(n).unwrap_or("element").to_owned();
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
        if let Some(p) = i.focused.take()
            && k.is_alive(p)
        {
            k.set_state(p, State::FOCUSED, false);
            k.set_state(p, State::FOCUS_VISIBLE, false);
        }
        if let Some(n) = n {
            k.set_state(n, State::FOCUSED, true);
            k.set_state(n, State::FOCUS_VISIBLE, i.modality == Modality::Keyboard);
            k.focused_in_group(n);
        }
        i.focused = n;
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

    fn handler_for(&self, ev: &Event) -> Result<Handler, String> {
        let (id, kind) = match ev {
            Event::Press(id) => (id, HandlerKind::Press),
            Event::Input(id, _) => (id, HandlerKind::Input),
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
                if self.w.breakpoint.get_untracked() != bp {
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
        crate::inspect::Snapshot {
            schema: crate::inspect::SCHEMA.into(),
            octara: env!("CARGO_PKG_VERSION").into(),
            viewport: [w, h],
            theme: k.theme().name.into(),
            breakpoint: crate::inspect::snake(&format!("{:?}", self.breakpoint())),
            time_ms: self.app().now().as_millis() as u64,
            focus,
            pending: self.app().pending().len(),
            window: self.w.id.number(),
            windows: self
                .app()
                .each()
                .iter()
                .map(|w| crate::inspect::WindowInfo {
                    window: w.w.id.number(),
                    title: w.w.options.title.clone(),
                    size: [w.w.size.get().0, w.w.size.get().1],
                })
                .collect(),
            root: k.inspect_tree(),
        }
    }
    /// What the current frame draws, in order (see [`crate::paint`]).
    pub fn display_list(&self) -> std::rc::Rc<crate::paint::DisplayList> {
        let list = self.w.kernel.borrow().display_list();
        match self.picking_overlay() {
            Some(over) => {
                let mut list = (*list).clone();
                list.items.extend(over);
                std::rc::Rc::new(list)
            }
            None => list,
        }
    }
    pub fn kernel(&self) -> Ref<'_, Kernel> {
        self.w.kernel.borrow()
    }
    pub fn kernel_mut(&mut self) -> RefMut<'_, Kernel> {
        self.w.kernel.borrow_mut()
    }
    /// Runs the event's handler, applies every update and lays out, like one frame.
    pub fn dispatch(&mut self, ev: Event) -> Result<(), String> {
        self.app.record(self.w.id, &InputEvent::Semantic(ev.clone()));
        self.on_semantic(ev)
    }
    pub(crate) fn on_semantic(&mut self, ev: Event) -> Result<(), String> {
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
            self.app.rt.with_frame(Ctx::Handler, None, Some(None), || match (h, ev) {
                (Handler::Press(f), _) | (Handler::Toggle(f), _) => f(),
                (Handler::Input(f), Event::Input(_, v)) => f(v),
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
    /// Rebuilds this window's scope from the current state, keeping hover, focus and scroll by
    /// test id.
    fn reload(&self) {
        self.enter(|| {
            {
                let mut i = self.w.interaction.borrow_mut();
                let k = self.w.kernel.borrow();
                i.scrolls = k.scroll_offsets();
                let id = |n: Option<NodeId>| n.filter(|n| k.is_alive(*n)).and_then(|n| k.test_id(n).map(str::to_owned));
                // The innermost hovered element with a test id (the node under the mouse may be
                // a text inside it).
                let hover = i.hover_chain.iter().copied().find(|n| k.is_alive(*n) && k.test_id(*n).is_some());
                i.reload = (id(hover), id(i.focused));
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
    /// Uses the platform's installed fonts and its fallback (CJK, emoji, …) besides the
    /// bundled ones, and its UI font where the theme keeps the default sans family: what a
    /// window shows. Headless screenshots for people and agents use it; deterministic tests
    /// do not (results then depend on the machine).
    #[cfg(feature = "system-fonts")]
    pub fn use_system_fonts(&self) {
        self.set_text_engine(Box::new(crate::kernel::text::ParleyEngine::system()));
        let theme = self.app.res.theme.borrow().clone();
        if theme.font_sans == Theme::default().font_sans {
            self.set_theme(theme.font_sans("system-ui, Inter"));
        }
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
    pub(crate) fn on_native_event(&mut self, id: crate::native::NativeId, event: crate::native::NativeEvent) -> bool {
        let Some(n) = self.native_node(id) else { return false };
        let handler = {
            let k = self.w.kernel.borrow();
            k.handler(n, HandlerKind::Native).and_then(|h| self.w.handlers.borrow().get(&h.0).cloned())
        };
        let Some(crate::view::Handler::Native(f)) = handler else { return false };
        self.input_frame(|| self.app.rt.with_frame(Ctx::Handler, None, Some(None), || f(&event)));
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
    /// which run for good) and native views that are busy (a page loading). A capture waits for
    /// this to be empty.
    pub fn unsettled(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .app()
            .pending()
            .into_iter()
            .filter(|p| p.kind != crate::task::WorkKind::Subscription)
            .map(|p| if p.at.is_empty() { p.label } else { format!("{} ({})", p.label, p.at) })
            .collect();
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
        self.w.kernel.borrow().pick(x, y)
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

    /// The main window on the platform (downcast with `as_any`; headless, a
    /// [`HeadlessWindow`](crate::platform::HeadlessWindow)).
    pub fn platform_window(&self) -> std::cell::Ref<'_, dyn crate::platform::PlatformWindow> {
        std::cell::Ref::map(self.w.platform.borrow(), |w| &**w)
    }

    /// The window on the platform, to change (a headless window's pretences in tests).
    pub fn platform_window_mut(&self) -> std::cell::RefMut<'_, dyn crate::platform::PlatformWindow> {
        std::cell::RefMut::map(self.w.platform.borrow_mut(), |w| &mut **w)
    }

    /// Tells the window's platform about the focused text field when it changed: where its
    /// caret is, whether it is secure or multi-line.
    /// Presents what changed to the window's platform: native view changes, the text input,
    /// whether to draw again (after every settle; nothing when nothing changed).
    fn present(&self) {
        let text_input = self.focused_input().and_then(|(n, _)| {
            let k = self.w.kernel.borrow();
            let caret = k.caret(n)?;
            Some(crate::platform::TextInput { caret, secure: k.is_secure(n), multiline: k.is_multiline(n) })
        });
        let generation = self.w.kernel.borrow().paint_generation();
        let (picking, picked) = self.picking_frame();
        let redraw = picking || generation != self.w.presented.get() || self.w.kernel.borrow_mut().in_motion();
        let native = std::mem::take(&mut *self.w.native_changes.borrow_mut());
        if native.is_empty() && !redraw && picked.is_none() && text_input == self.w.text_input.get() {
            return;
        }
        self.w.text_input.set(text_input);
        self.w.presented.set(generation);
        self.w.platform.borrow_mut().present(crate::platform::Frame { native, text_input, redraw, picked });
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

    /// The system the app runs on (downcast with `as_any`; headless, a
    /// [`HeadlessPlatform`](crate::platform::HeadlessPlatform), whose clipboard tests set).
    pub fn platform(&self) -> std::cell::RefMut<'_, dyn crate::platform::Platform> {
        std::cell::RefMut::map(self.app.system.borrow_mut(), |p| &mut **p)
    }
}
