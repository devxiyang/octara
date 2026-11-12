//! The app runtime: mounts a root view, dispatches events, lays out, and rebuilds for hot reload.

mod behavior;
mod drive;
mod edit;
mod focus;
mod input;
mod keys;
mod pointer;
mod time;

pub use edit::{Clipboard, MemoryClipboard, TextField, byte_offset, utf16_offset};
pub use keys::{BUILTIN_COMMANDS, Binding, Chord, Key, KeyEvent, Keymap};
pub use pointer::{Button, Drag, DragPhase, Modifiers, Phase, PointerEvent, PointerKind, Tuning};

use crate::diagnostics::{self as codes, Diagnostic};
use crate::kernel::{Breakpoint, HandlerKind, Kernel, NodeId, State, Theme};
use crate::reactive::graph::Signal;
use crate::reactive::graph::{Ctx, Runtime, ScopeId};
use crate::view::{Handler, Parent, Slots, Ui, View, enter_ui, mount};
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;

/// The window size an app starts with; change it with [`App::resize`].
pub const DEFAULT_VIEWPORT: (f32, f32) = (800.0, 600.0);

/// An input event, addressed by test id (or by name for external events).
#[derive(Clone, Debug)]
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
pub enum Platform {
    MacOs,
    Ios,
    Windows,
    Linux,
    Android,
}

impl Platform {
    /// The platform this build targets.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "ios") {
            Platform::Ios
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "android") {
            Platform::Android
        } else {
            Platform::Linux
        }
    }
    pub fn is_apple(self) -> bool {
        matches!(self, Platform::MacOs | Platform::Ios)
    }
    /// Input is a finger: controls take their touch sizes (`When::Touch`), and `octara check`
    /// asks 44 × 44 targets.
    pub fn is_touch(self) -> bool {
        matches!(self, Platform::Ios | Platform::Android)
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

pub struct App {
    rt: Rc<Runtime>,
    ui: Rc<Ui>,
    root: Rc<dyn Fn() -> View>,
    root_scope: Cell<Option<ScopeId>>,
    root_slots: Slots,
    next_cause: Cell<u64>,
    interaction: RefCell<Interaction>,
    viewport: Signal<(f32, f32)>,
    breakpoint: Signal<Breakpoint>,
    size: Cell<(f32, f32)>,
    tuning: Cell<Tuning>,
    platform: Cell<Platform>,
    /// The app's own bindings (after the platform's standard ones).
    keymap: RefCell<keys::Keymap>,
    clipboard: RefCell<Box<dyn edit::Clipboard>>,
    /// Where native views live: the platform's in a shell, a recording one headless.
    native_host: RefCell<Box<dyn crate::native::NativeHost>>,
    /// What the host was last told of each native view: its props version and placement.
    native_sent: RefCell<rustc_hash::FxHashMap<NodeId, (u64, Option<crate::native::Placement>)>>,
    /// What paints `capture` requests (the harness installs the reference renderer).
    painter: RefCell<Option<Box<dyn crate::capture::Painter>>>,
    /// A capture in progress: the snapshots asked for.
    capturing: RefCell<Option<crate::capture::Capturing>>,
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

/// The offset a scroll event starts from: the current one for `Scroll`, the origin for `ScrollTo`.
fn n_offset(k: &Kernel, n: crate::kernel::NodeId, relative: bool) -> (f32, f32) {
    if relative { k.scroll_offset(n) } else { (0.0, 0.0) }
}

/// States the runtime drives; `SELECTED` and `DISABLED` come from element props.
const RUNTIME_STATES: State = State::HOVER.union(State::FOCUSED).union(State::FOCUS_VISIBLE).union(State::PRESSED);

impl App {
    pub fn new(root: impl Fn() -> View + 'static) -> Self {
        let rt = Runtime::new();
        let app = App {
            rt: rt.clone(),
            ui: Rc::new(Ui::default()),
            root: Rc::new(root),
            root_scope: Cell::new(None),
            root_slots: Slots::default(),
            next_cause: Cell::new(0),
            interaction: RefCell::default(),
            viewport: rt.enter(|| Signal::detached(DEFAULT_VIEWPORT)),
            breakpoint: rt.enter(|| Signal::detached(Theme::default().breakpoint_of(DEFAULT_VIEWPORT.0))),
            size: Cell::new(DEFAULT_VIEWPORT),
            tuning: Cell::new(Tuning::default()),
            platform: Cell::new(Platform::current()),
            native_host: RefCell::new(Box::<crate::native::RecordingHost>::default()),
            native_sent: RefCell::default(),
            painter: RefCell::default(),
            capturing: RefCell::default(),
            keymap: RefCell::new(keys::Keymap::default()),
            clipboard: RefCell::new(Box::new(edit::MemoryClipboard::default())),
        };
        app.ui.viewport.set(Some(app.viewport));
        app.ui.breakpoint.set(Some(app.breakpoint));
        app.ui.kernel.borrow_mut().set_touch(app.platform.get().is_touch());
        app.enter(|| app.mount_root());
        app
    }

    pub(crate) fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        enter_ui(&self.ui, || self.rt.enter(f))
    }

    fn mount_root(&self) {
        let s = self.rt.create_scope(None, "app");
        self.root_scope.set(Some(s));
        let parent = Parent { node: self.ui.kernel.borrow().root(), root: self.root_slots.clone() };
        let root = self.root.clone();
        self.rt.with_frame(Ctx::Build, None, Some(Some(s)), || mount(root(), &parent, &self.root_slots));
        self.rt.flush();
        self.restore_interaction();
        self.settle_frame();
    }

    /// Applies pending updates and lays out until nothing changes: after a layout, virtual
    /// lists measure their rows and scroll containers report their state, which may change the
    /// view again. Gives up after a few rounds and reports `LAYOUT_LOOP`.
    #[track_caller]
    fn settle_frame(&self) {
        let at = std::panic::Location::caller();
        // Tasks woken (or spawned) by what just happened run to their next wait first; what they
        // change is laid out with the rest. They may wake or spawn more, so repeat (bounded).
        for _ in 0..16 {
            self.ui.sched.run_ready();
            self.settle_layout(at);
            // Modals that opened or closed move focus (which restyles).
            if self.sync_modals() {
                self.settle_layout(at);
            }
            if !self.ui.sched.has_ready() {
                break;
            }
        }
        // A caret that moved shows: the scroll containers around its field follow it.
        let reveal = self.interaction.borrow_mut().reveal.take();
        if let Some(n) = reveal {
            let caret = self.ui.kernel.borrow().caret(n);
            if let Some(r) = caret
                && self.ui.kernel.borrow_mut().reveal(n, r)
            {
                self.settle_layout(at);
            }
        }
        self.sync_native();
        // An `instant` transaction covers the frame it was made in.
        self.ui.kernel.borrow_mut().set_instant(false);
        // What is on its way out can no longer be focused or hovered.
        let (focused, hovered) = {
            let i = self.interaction.borrow();
            (i.focused, i.hovered)
        };
        let gone = |n: Option<NodeId>| n.is_some_and(|n| !self.ui.kernel.borrow().is_alive(n) || self.ui.kernel.borrow().is_exiting(n));
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
            self.rt.flush();
            self.ui.kernel.borrow_mut().layout(self.size().0, self.size().1);
            let changed = self.rt.with_frame(Ctx::Handler, None, Some(None), || self.ui.sync_after_layout());
            if !changed {
                return;
            }
        }
        self.rt.flush();
        self.ui.kernel.borrow_mut().layout(self.size().0, self.size().1);
        self.rt.diag(
            &codes::LAYOUT_LOOP,
            "layout did not settle in 4 rounds: something keeps changing what it measures (a size read feeding the size it reads)".into(),
            at,
        );
    }

    fn restore_interaction(&self) {
        let mut i = self.interaction.borrow_mut();
        let mut k = self.ui.kernel.borrow_mut();
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
        let mut i = self.interaction.borrow_mut();
        if i.hovered == n {
            return;
        }
        let mut k = self.ui.kernel.borrow_mut();
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
        let mut i = self.interaction.borrow_mut();
        if i.modality == m {
            return;
        }
        i.modality = m;
        if let Some(n) = i.focused {
            let mut k = self.ui.kernel.borrow_mut();
            if k.is_alive(n) {
                k.set_state(n, State::FOCUS_VISIBLE, m == Modality::Keyboard);
            }
        }
    }

    /// Moves keyboard focus to `n`, which must be focusable and enabled (`None` blurs). The
    /// ring shows if the last input was the keyboard.
    pub(crate) fn set_focus(&self, n: Option<NodeId>) -> Result<(), String> {
        let mut i = self.interaction.borrow_mut();
        let mut k = self.ui.kernel.borrow_mut();
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
        let find = |id: &str| self.ui.kernel.borrow().find(id).ok_or(format!("no node with test id `{id}`"));
        match ev {
            Event::Hover(id, on) => {
                let n = find(id)?;
                if *on {
                    self.set_hover(Some(n));
                } else if self.interaction.borrow().hover_chain.contains(&n) {
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
                let mut k = self.ui.kernel.borrow_mut();
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
        let k = self.ui.kernel.borrow();
        let node = k.find(id).ok_or(format!("no node with test id `{id}`"))?;
        if k.state(node).contains(State::DISABLED) {
            return Err(format!("`{id}` is disabled"));
        }
        let hid = k.handler(node, kind).ok_or(format!("`{id}` has no {kind:?} handler"))?;
        self.ui.handlers.borrow().get(&hid.0).cloned().ok_or(format!("handler of `{id}` was disposed"))
    }
}

impl App {
    /// The current window size.
    pub fn size(&self) -> (f32, f32) {
        self.size.get()
    }
    /// Resizes the window: bindings that read `viewport()` update, then everything is laid out
    /// again at the new size (text re-breaks; nothing else reruns).
    pub fn resize(&mut self, width: f32, height: f32) {
        self.size.set((width, height));
        self.enter(|| {
            self.rt.with_frame(Ctx::Handler, None, Some(None), || {
                self.viewport.set((width, height));
                let bp = self.ui.kernel.borrow().theme().breakpoint_of(width);
                if self.breakpoint.get_untracked() != bp {
                    self.breakpoint.set(bp);
                }
            });
            self.settle_frame();
        })
    }
    /// The window's breakpoint (Tailwind `sm` … `2xl`).
    pub fn breakpoint(&self) -> Breakpoint {
        self.kernel().theme().breakpoint_of(self.size.get().0)
    }
    /// Runs `check` at each window size in turn (e.g. phone and desktop), then restores the size.
    /// Stops at the first error, naming the size.
    pub fn at_sizes(&mut self, sizes: &[(f32, f32)], mut check: impl FnMut(&mut App) -> Result<(), String>) -> Result<(), String> {
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
        self.ui.kernel.borrow().layout_issues()
    }
    /// The semantic tree and app state as the Inspector protocol gives them (see
    /// [`crate::inspect`]).
    pub fn inspect(&self) -> crate::inspect::Snapshot {
        let (w, h) = self.size();
        let k = self.ui.kernel.borrow();
        let focus = self.interaction.borrow().focused.filter(|n| k.is_alive(*n)).and_then(|n| k.test_id(n).map(str::to_owned));
        crate::inspect::Snapshot {
            schema: crate::inspect::SCHEMA.into(),
            octara: env!("CARGO_PKG_VERSION").into(),
            viewport: [w, h],
            theme: k.theme().name.into(),
            breakpoint: crate::inspect::snake(&format!("{:?}", self.breakpoint())),
            time_ms: self.now().as_millis() as u64,
            focus,
            pending: self.pending().len(),
            root: k.inspect_tree(),
        }
    }
    /// What the current frame draws, in order (see [`crate::paint`]).
    pub fn display_list(&self) -> std::rc::Rc<crate::paint::DisplayList> {
        self.ui.kernel.borrow().display_list()
    }
    pub fn kernel(&self) -> Ref<'_, Kernel> {
        self.ui.kernel.borrow()
    }
    pub fn kernel_mut(&mut self) -> RefMut<'_, Kernel> {
        self.ui.kernel.borrow_mut()
    }
    /// Runs the event's handler, applies every update and lays out, like one frame.
    pub fn dispatch(&mut self, ev: Event) -> Result<(), String> {
        self.enter(|| {
            let cause = self.next_cause.get() + 1;
            if matches!(ev, Event::Hover(..) | Event::Focus(_) | Event::Blur | Event::Scroll(..) | Event::ScrollTo(..)) {
                self.ui.kernel.borrow_mut().set_cause(Some(cause));
                let r = self.interact(&ev);
                if r.is_err() {
                    self.ui.kernel.borrow_mut().set_cause(None);
                    return r;
                }
                self.next_cause.set(cause);
                self.settle_frame();
                self.ui.kernel.borrow_mut().set_cause(None);
                return Ok(());
            }
            if let Event::Send(name, payload) = &ev {
                self.ui.kernel.borrow_mut().set_cause(Some(cause));
                let r = self.ui.sched.send_named(name, payload);
                if r.is_ok() {
                    self.next_cause.set(cause);
                    self.settle_frame();
                }
                self.ui.kernel.borrow_mut().set_cause(None);
                return r;
            }
            let h = self.handler_for(&ev)?;
            self.next_cause.set(cause);
            self.ui.kernel.borrow_mut().set_cause(Some(cause));
            self.rt.with_frame(Ctx::Handler, None, Some(None), || match (h, ev) {
                (Handler::Press(f), _) | (Handler::Toggle(f), _) => f(),
                (Handler::Input(f), Event::Input(_, v)) => f(v),
                _ => unreachable!("handler kind matches event kind"),
            });
            self.settle_frame();
            self.ui.kernel.borrow_mut().set_cause(None);
            Ok(())
        })
    }
    /// The platform whose conventions input follows (the build's by default).
    pub fn platform(&self) -> Platform {
        self.platform.get()
    }
    /// The platform whose conventions input follows: its keymap, and on touch platforms the
    /// controls' touch sizes (`when_touch`).
    pub fn set_platform(&mut self, p: Platform) {
        self.platform.set(p);
        self.ui.kernel.borrow_mut().set_touch(p.is_touch());
        self.enter(|| self.settle_frame());
    }
    /// Holds an interaction state (`HOVER`, `FOCUSED`, `FOCUS_VISIBLE`, `PRESSED`) on or off, e.g. to check
    /// pressed styles; kept across hot reload. `SELECTED` and `DISABLED` come from props.
    pub fn force_state(&mut self, test_id: &str, state: State, on: bool) -> Result<(), String> {
        if !RUNTIME_STATES.contains(state) {
            return Err(format!("{state:?} is set by element props, not forced"));
        }
        {
            let mut k = self.ui.kernel.borrow_mut();
            let n = k.find(test_id).ok_or(format!("no node with test id `{test_id}`"))?;
            k.set_state(n, state, on);
        }
        self.enter(|| self.settle_frame());
        let mut i = self.interaction.borrow_mut();
        i.forced.retain(|(id, s, _)| !(id == test_id && *s == state));
        i.forced.push((test_id.to_owned(), state, on));
        Ok(())
    }
    /// Drops every forced state (the nodes keep their current state until it next changes).
    pub fn clear_forced(&mut self) {
        self.interaction.borrow_mut().forced.clear();
    }
    pub fn set_theme(&mut self, theme: Theme) {
        self.enter(|| {
            self.rt.with_frame(Ctx::Handler, None, Some(None), || crate::view::set_theme(theme));
            self.settle_frame();
        })
    }
    /// Adds font data (TTF/OTF); its families can then be named by the theme or `font_family`.
    pub fn register_font(&mut self, data: &'static [u8]) {
        self.ui.kernel.borrow_mut().register_font(data);
        self.enter(|| self.settle_frame());
    }
    /// Adds a font text falls back to for `scripts` (ISO 15924 tags: `*b"Hani"` for hanzi,
    /// `*b"Hira"`, `*b"Kana"`, `*b"Hang"`, …) when its own family lacks a character.
    pub fn register_fallback_font(&mut self, data: &'static [u8], scripts: &[[u8; 4]]) {
        self.ui.kernel.borrow_mut().register_fallback_font(data, scripts);
        self.enter(|| self.settle_frame());
    }
    /// Uses the platform's installed fonts and its fallback (CJK, emoji, …) besides the
    /// bundled ones, and its UI font where the theme keeps the default sans family: what a
    /// window shows. Headless screenshots for people and agents use it; deterministic tests
    /// do not (results then depend on the machine).
    #[cfg(feature = "system-fonts")]
    pub fn use_system_fonts(&mut self) {
        self.ui.kernel.borrow_mut().set_text_engine(Box::new(crate::kernel::text::ParleyEngine::system()));
        let theme = self.kernel().theme().clone();
        if theme.font_sans == Theme::default().font_sans {
            self.set_theme(theme.font_sans("system-ui, Inter"));
        } else {
            self.enter(|| self.settle_frame());
        }
    }
    /// Rebuilds every scope from the current state, as after a code patch; named state survives.
    pub fn hot_reload(&mut self) {
        self.enter(|| {
            {
                let mut i = self.interaction.borrow_mut();
                let k = self.ui.kernel.borrow();
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
            if let Some(s) = self.root_scope.take() {
                self.rt.set_reloading(true);
                self.rt.dispose_scope(s);
                self.rt.set_reloading(false);
            }
            self.root_slots.borrow_mut().clear();
            // A rebuild is not content arriving: nothing enters.
            self.ui.kernel.borrow_mut().set_quiet(true);
            self.mount_root();
            self.ui.kernel.borrow_mut().set_quiet(false);
        });
    }
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.rt.diagnostics()
    }
    /// Each stored diagnostic's code with how many times it occurred (repeats are not stored).
    pub fn diagnostic_counts(&self) -> Vec<(&'static str, u32)> {
        self.rt.diagnostic_counts()
    }
    pub fn work(&self) -> Work {
        Work { closures_run: self.rt.closures_run(), reactive_nodes: self.rt.live_nodes() }
    }
    pub fn reset_work(&mut self) {
        self.rt.reset_closures_run();
    }
}

// ---- native views ----

impl App {
    /// Tells the native host what changed this frame: views created (with their props), props
    /// changed, placements changed, views gone.
    fn sync_native(&self) {
        let k = self.ui.kernel.borrow();
        let mut sent = self.native_sent.borrow_mut();
        if k.native_nodes().is_empty() && sent.is_empty() {
            return;
        }
        let mut host = self.native_host.borrow_mut();
        let placements = k.native_placements();
        let mut moved = Vec::new();
        for (n, p) in &placements {
            let Some(v) = k.native(*n) else { continue };
            match sent.get_mut(n) {
                None => {
                    host.create(p.id, &v.kind, &v.props);
                    sent.insert(*n, (v.version, None));
                }
                Some((version, _)) if *version != v.version => {
                    host.update(p.id, &v.props);
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
            host.place(&moved);
        }
        let live: Vec<NodeId> = placements.iter().map(|(n, _)| *n).collect();
        sent.retain(|n, _| {
            let keep = live.contains(n);
            if !keep {
                host.destroy(crate::native::NativeId(crate::kernel::a11y::a11y_id(*n).0));
            }
            keep
        });
    }

    /// The node of a native view's id, if it still exists.
    fn native_node(&self, id: crate::native::NativeId) -> Option<NodeId> {
        let k = self.ui.kernel.borrow();
        k.node_of_a11y(accesskit::NodeId(id.0)).filter(|n| k.native(*n).is_some())
    }

    /// An event from native view `id` (the host calls this): runs its `on_native`.
    pub fn native_event(&mut self, id: crate::native::NativeId, event: crate::native::NativeEvent) -> bool {
        let Some(n) = self.native_node(id) else { return false };
        let handler = {
            let k = self.ui.kernel.borrow();
            k.handler(n, HandlerKind::Native).and_then(|h| self.ui.handlers.borrow().get(&h.0).cloned())
        };
        let Some(crate::view::Handler::Native(f)) = handler else { return false };
        self.input_frame(|| self.rt.with_frame(Ctx::Handler, None, Some(None), || f(&event)));
        true
    }

    /// The natural size native view `id` reports (the host calls this): used where no size is
    /// authored.
    pub fn native_size(&mut self, id: crate::native::NativeId, width: f32, height: f32) {
        let Some(n) = self.native_node(id) else { return };
        self.input_frame(|| self.ui.kernel.borrow_mut().set_native_size(n, (width, height)));
    }

    /// What keeps the app from being settled now: tasks, timers and motion (not subscriptions,
    /// which run for good) and native views that are busy (a page loading). A capture waits for
    /// this to be empty.
    pub fn unsettled(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .pending()
            .into_iter()
            .filter(|p| p.kind != crate::task::WorkKind::Subscription)
            .map(|p| if p.at.is_empty() { p.label } else { format!("{} ({})", p.label, p.at) })
            .collect();
        let placements = self.ui.kernel.borrow().native_placements();
        let host = self.native_host.borrow();
        for (n, p) in placements.into_iter().filter(|(_, p)| p.visible) {
            if let Some(what) = host.busy(p.id) {
                out.push(format!("{}: {what}", self.native_name(n)));
            }
        }
        out
    }

    /// A native view as reports name it: its test id, else its kind.
    fn native_name(&self, n: NodeId) -> String {
        let k = self.ui.kernel.borrow();
        let kind = k.native(n).map(|v| v.kind.clone()).unwrap_or_default();
        match k.test_id(n) {
            Some(id) => format!("{kind} view #{id}"),
            None => format!("{kind} view"),
        }
    }

    /// Installs what paints `capture` requests headless (`octara-render`'s reference renderer).
    pub fn set_painter(&mut self, painter: Box<dyn crate::capture::Painter>) {
        *self.painter.borrow_mut() = Some(painter);
    }

    /// Starts a capture: asks the host for a snapshot of every native view shown. Returns the
    /// views whose snapshot is coming (through [`native_snapshot`](Self::native_snapshot)).
    pub fn capture_begin(&mut self) -> Vec<crate::native::NativeId> {
        let placements = self.ui.kernel.borrow().native_placements();
        let mut host = self.native_host.borrow_mut();
        let asked: Vec<_> = placements.into_iter().filter(|(_, p)| p.visible).map(|(_, p)| (p.id, host.snapshot(p.id))).collect();
        let coming = asked.iter().filter(|(_, r)| r.is_ok()).map(|(id, _)| *id).collect();
        *self.capturing.borrow_mut() = Some(crate::capture::Capturing { asked });
        coming
    }

    /// The snapshots asked for that have not come yet.
    pub fn capture_awaiting(&self) -> Vec<crate::native::NativeId> {
        let capturing = self.capturing.borrow();
        let Some(c) = capturing.as_ref() else { return Vec::new() };
        let k = self.ui.kernel.borrow();
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
        let asked = self.capturing.borrow_mut().take().map(|c| c.asked).unwrap_or_default();
        let mut native = Vec::new();
        for (id, result) in &asked {
            let Some(n) = self.native_node(*id) else { continue };
            let k = self.ui.kernel.borrow();
            let Some(v) = k.native(n) else { continue };
            let (x, y, w, h) = k.rect(n);
            let (shown, reason, misses) = match result {
                Ok(misses) if v.snapshot.is_some() => (Shown::Snapshot, None, misses.clone()),
                Ok(misses) => (Shown::Failed, Some("its snapshot did not come in time".to_owned()), misses.clone()),
                Err(why) => (Shown::Placeholder, Some(why.clone()), Vec::new()),
            };
            native.push(NativeShot { id: k.test_id(n).map(str::to_owned), kind: v.kind.clone(), rect: [x, y, w, h], shown, reason, misses });
        }
        let painted = painter.paint(&self.display_list(), scale).and_then(|p| p.save_png(path).map(|()| p));
        for (id, _) in &asked {
            self.native_snapshot(*id, None);
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

    /// Takes the installed painter out while it paints (see [`set_painter`](Self::set_painter)).
    pub(crate) fn take_painter(&self) -> Option<Box<dyn crate::capture::Painter>> {
        self.painter.borrow_mut().take()
    }

    /// An image of what native view `id` shows (the host calls this): drawn in its place
    /// instead of the placeholder, until `None` clears it.
    pub fn native_snapshot(&mut self, id: crate::native::NativeId, image: Option<crate::image::Image>) {
        let Some(n) = self.native_node(id) else { return };
        self.input_frame(|| self.ui.kernel.borrow_mut().set_native_snapshot(n, image));
    }

    /// Replaces the native host (a shell installs the platform's). Views already shown are
    /// created again on the new one.
    pub fn set_native_host(&mut self, host: Box<dyn crate::native::NativeHost>) {
        *self.native_host.borrow_mut() = host;
        self.native_sent.borrow_mut().clear();
        self.sync_native();
    }

    /// The native host (downcast with `as_any`; headless, a [`RecordingHost`]).
    ///
    /// [`RecordingHost`]: crate::native::RecordingHost
    pub fn native_host(&self) -> std::cell::Ref<'_, Box<dyn crate::native::NativeHost>> {
        self.native_host.borrow()
    }
}

