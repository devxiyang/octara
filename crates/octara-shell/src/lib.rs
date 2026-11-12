//! Desktop windows for Octara apps (winit): window events become [`App`] input by position and
//! key, and frames are each window's display list drawn by the GPU renderer into it. The app
//! opens and closes windows (`open_window`, `close_window`); each one has its own surface,
//! accessibility adapter, native views and frame clock (macOS: its view's display link), and the
//! loop ends when the last one closes.
//!
//! Conformance: `OCTARA_CONFORMANCE=1` runs the platform contract's checks
//! ([`octara::conformance`]) in the loop once the first window is open, prints them (JSON) on
//! stdout and exits, with status 1 if one failed.
//!
//! Picking: in debug builds `mod+shift+c` (as browsers' "inspect element") starts picking, and
//! `OCTARA_PICK=1` starts the window picking. Click an element: its line (number, role, test id,
//! source, the component that made it) goes to the clipboard and to stderr, its JSON to stdout.
//!
//! ```no_run
//! octara_shell::run("Counter", octara_examples_counter);
//! # fn octara_examples_counter() -> octara::view::View { unimplemented!() }
//! ```

use octara::app::{App, Button, FileDrag, InputEvent, Key, KeyEvent, Modifiers, Phase, PointerEvent, Window as AppWindow};
use octara::view::View;
use platform::WinitWindow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::WindowId;

mod capture;
#[cfg(target_os = "macos")]
mod native_mac;
#[cfg(target_os = "macos")]
mod pace_mac;
mod platform;

/// Events sent to the loop from elsewhere: AccessKit's requests, the app's waker (another
/// thread finished background work or sent on a channel), a window's frame clock ticking.
#[derive(Debug)]
pub enum ShellEvent {
    Accessibility(accesskit_winit::Event),
    Wake,
    /// A capture in progress looks again (`OCTARA_CAPTURE`).
    Capture,
    /// A window's frame clock ticked (a display refresh).
    Frame(WindowId),
    /// A person answered a request a window's platform showed (a file dialog, a menu).
    Answer {
        window: WindowId,
        id: octara::ask::AskId,
        answer: octara::ask::Answer,
    },
}

impl From<accesskit_winit::Event> for ShellEvent {
    fn from(e: accesskit_winit::Event) -> Self {
        ShellEvent::Accessibility(e)
    }
}

/// Pixels per wheel "line".
const LINE: f32 = 40.0;

/// Options for [`run_with`].
#[derive(Clone, Debug)]
pub struct Options {
    pub title: String,
    /// Initial size in logical pixels.
    pub size: (f32, f32),
    /// Exit after the first window drew this many frames (smoke tests).
    pub exit_after_frames: Option<u32>,
    /// Use the platform's fonts and fallback (CJK, emoji, …), and its UI font when the app's
    /// theme keeps the default sans family. On by default.
    pub system_fonts: bool,
}

impl Default for Options {
    fn default() -> Self {
        let exit_after_frames = std::env::var("OCTARA_EXIT_AFTER_FRAMES").ok().and_then(|v| v.parse().ok());
        Options { title: "Octara".into(), size: (800.0, 600.0), exit_after_frames, system_fonts: true }
    }
}

/// Opens a window showing `root` and runs until the app's last window is closed.
pub fn run(title: &str, root: impl Fn() -> View + 'static) {
    run_with(Options { title: title.into(), ..Options::default() }, root)
}

pub fn run_with(options: Options, root: impl Fn() -> View + 'static) {
    let event_loop = EventLoop::<ShellEvent>::with_user_event().build().expect("event loop");
    let shared = Rc::new(Shared {
        proxy: event_loop.create_proxy(),
        active: Cell::new(std::ptr::null()),
        gpu: RefCell::new(None),
        windows: RefCell::new(HashMap::new()),
        trace: std::env::var_os("OCTARA_TRACE_INPUT").is_some_and(|v| v != "0"),
    });
    let mut shell = Shell {
        options,
        root: Some(Box::new(root)),
        shared,
        app: None,
        main: None,
        modifiers: ModifiersState::empty(),
        started: Instant::now(),
        done: false,
        failed: false,
        capture: None,
    };
    event_loop.run_app(&mut shell).expect("event loop");
    if shell.failed {
        std::process::exit(1);
    }
}

/// What the shell and its platform share: the loop's proxy, the loop running a callback now
/// (windows are made inside one), the GPU, and which app window each winit window shows.
pub(crate) struct Shared {
    pub(crate) proxy: EventLoopProxy<ShellEvent>,
    active: Cell<*const ActiveEventLoop>,
    pub(crate) gpu: RefCell<Option<platform::Gpu>>,
    pub(crate) windows: RefCell<HashMap<WindowId, octara::app::WindowId>>,
    /// `OCTARA_TRACE_INPUT=1`: each input's latency to the frame that shows it, on stderr.
    pub(crate) trace: bool,
}

impl Shared {
    /// The loop whose callback runs now.
    pub(crate) fn event_loop(&self) -> Option<&ActiveEventLoop> {
        let p = self.active.get();
        // SAFETY: set only for the duration of a callback, which holds the loop.
        (!p.is_null()).then(|| unsafe { &*p })
    }
}

/// While a callback of the loop runs: windows can be made in it.
struct InLoop(Rc<Shared>);

impl InLoop {
    fn enter(shared: &Rc<Shared>, event_loop: &ActiveEventLoop) -> InLoop {
        shared.active.set(event_loop);
        InLoop(shared.clone())
    }
}

impl Drop for InLoop {
    fn drop(&mut self) {
        self.0.active.set(std::ptr::null());
    }
}

struct Shell {
    options: Options,
    root: Option<Box<dyn Fn() -> View>>,
    shared: Rc<Shared>,
    app: Option<App>,
    /// The first window (captures and `exit_after_frames` are its).
    main: Option<octara::app::WindowId>,
    modifiers: ModifiersState,
    /// The app's clock starts at launch.
    started: Instant,
    /// `exit_after_frames` was reached, a capture or the conformance checks finished.
    done: bool,
    /// A conformance check failed: the process exits with status 1.
    failed: bool,
    /// A capture in progress (`OCTARA_CAPTURE`, see [`capture`]).
    capture: Option<capture::Job>,
}

/// `f` on the winit window behind app window `w`.
fn ww<R>(w: &AppWindow, f: impl FnOnce(&mut WinitWindow) -> R) -> Option<R> {
    let mut pw = w.platform_window_mut();
    pw.as_any_mut().downcast_mut::<WinitWindow>().map(f)
}

fn modifiers(m: ModifiersState) -> Modifiers {
    let mut out = Modifiers::empty();
    out.set(Modifiers::SHIFT, m.shift_key());
    out.set(Modifiers::CTRL, m.control_key());
    out.set(Modifiers::ALT, m.alt_key());
    out.set(Modifiers::META, m.super_key());
    out
}

/// The character a physical key types on a US layout (letters and digits), for shortcuts under
/// other layouts.
fn physical_char(key: PhysicalKey) -> Option<char> {
    let PhysicalKey::Code(code) = key else { return None };
    const LETTERS: [KeyCode; 26] = [
        KeyCode::KeyA,
        KeyCode::KeyB,
        KeyCode::KeyC,
        KeyCode::KeyD,
        KeyCode::KeyE,
        KeyCode::KeyF,
        KeyCode::KeyG,
        KeyCode::KeyH,
        KeyCode::KeyI,
        KeyCode::KeyJ,
        KeyCode::KeyK,
        KeyCode::KeyL,
        KeyCode::KeyM,
        KeyCode::KeyN,
        KeyCode::KeyO,
        KeyCode::KeyP,
        KeyCode::KeyQ,
        KeyCode::KeyR,
        KeyCode::KeyS,
        KeyCode::KeyT,
        KeyCode::KeyU,
        KeyCode::KeyV,
        KeyCode::KeyW,
        KeyCode::KeyX,
        KeyCode::KeyY,
        KeyCode::KeyZ,
    ];
    const DIGITS: [KeyCode; 10] = [
        KeyCode::Digit0,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    if let Some(i) = LETTERS.iter().position(|k| *k == code) {
        return Some((b'a' + i as u8) as char);
    }
    DIGITS.iter().position(|k| *k == code).map(|i| (b'0' + i as u8) as char)
}

impl Shell {
    /// The app window winit window `id` shows.
    fn window_of(&self, id: WindowId) -> Option<AppWindow> {
        let w = *self.shared.windows.borrow().get(&id)?;
        self.app.as_ref()?.window(w)
    }

    fn main_window(&self) -> Option<AppWindow> {
        self.app.as_ref()?.window(self.main?)
    }

    /// Brings the app's clock to now (before each event and frame).
    fn tick(&self) {
        if let Some(app) = &self.app {
            app.advance_to(self.started.elapsed());
        }
    }

    /// One pointer event at the window's pointer (mouse) or the touch's (finger).
    fn pointer(&mut self, w: &mut AppWindow, ev: PointerEvent) {
        let at = Instant::now();
        self.tick();
        w.event(InputEvent::Pointer(ev));
        // Moves come in bursts, and a cancel (the window losing focus) shows nothing new.
        let what = match ev.phase {
            Phase::Down => "pointer down",
            Phase::Up => "pointer up",
            Phase::Move | Phase::Cancel => return,
        };
        ww(w, |x| x.traced(at, what));
    }

    /// After anything: each window learns its size once, shows what the app has new (now after
    /// a quiet spell, else on its clock's next tick), and tells assistive technology if it
    /// listens.
    fn show(&mut self) {
        let Some(app) = self.app.clone() else { return };
        for id in app.windows() {
            let Some(mut w) = app.window(id) else { continue };
            if let Some((size, scale)) = ww(&w, |x| (!x.sized).then(|| (x.window.inner_size(), x.scale()))).flatten() {
                ww(&w, |x| x.sized = true);
                w.event(InputEvent::Resize { width: size.width as f32 / scale, height: size.height as f32 / scale, scale });
            }
            match ww(&w, |x| (x.pending, x.rested())) {
                Some((true, true)) => self.draw(&w),
                Some((true, false)) => {
                    ww(&w, |x| x.want_frame());
                }
                _ => {}
            }
            ww(&w, |x| x.a11y.update_if_active(|| w.accessibility_update()));
        }
    }

    /// Draws window `w` now.
    fn draw(&mut self, w: &AppWindow) {
        let list = w.display_list();
        let shown = {
            let mut gpu = self.shared.gpu.borrow_mut();
            let Some(g) = gpu.as_mut() else { return };
            ww(w, |x| x.draw(&list, &mut g.renderer)).unwrap_or(false)
        };
        if shown
            && Some(w.id()) == self.main
            && let Some(n) = self.options.exit_after_frames
        {
            let (frames, (width, height), scale) = ww(w, |x| (x.frames, (x.config.width, x.config.height), x.scale())).unwrap_or_default();
            if frames >= n {
                eprintln!("octara-shell: drew {frames} frames at {width}×{height} (scale {scale}), exiting");
                self.done = true;
            } else {
                ww(w, |x| {
                    x.pending = true;
                    x.want_frame();
                });
            }
        }
    }

    /// A key press: the app's key routing first (focus, keymap, commands); if nothing took it,
    /// the text it typed goes to the focused field.
    fn key(&mut self, app: &mut AppWindow, event: &winit::event::KeyEvent) {
        // `mod+shift+c` starts and ends picking in debug builds.
        let command = if cfg!(target_os = "macos") { self.modifiers.super_key() } else { self.modifiers.control_key() };
        if cfg!(debug_assertions)
            && command
            && self.modifiers.shift_key()
            && physical_char(event.physical_key) == Some('c')
            && !event.repeat
        {
            let on = !app.picking();
            app.set_picking(on);
            return;
        }
        let key = match &event.logical_key {
            WKey::Named(n) => match n {
                NamedKey::Tab => Some(Key::Tab),
                NamedKey::Enter => Some(Key::Enter),
                NamedKey::Escape => Some(Key::Escape),
                NamedKey::Backspace => Some(Key::Backspace),
                NamedKey::Delete => Some(Key::Delete),
                NamedKey::Space => Some(Key::Space),
                NamedKey::ArrowUp => Some(Key::Up),
                NamedKey::ArrowDown => Some(Key::Down),
                NamedKey::ArrowLeft => Some(Key::Left),
                NamedKey::ArrowRight => Some(Key::Right),
                NamedKey::Home => Some(Key::Home),
                NamedKey::End => Some(Key::End),
                NamedKey::PageUp => Some(Key::PageUp),
                NamedKey::PageDown => Some(Key::PageDown),
                NamedKey::ContextMenu => Some(Key::Menu),
                NamedKey::F1 => Some(Key::F(1)),
                NamedKey::F2 => Some(Key::F(2)),
                NamedKey::F3 => Some(Key::F(3)),
                NamedKey::F4 => Some(Key::F(4)),
                NamedKey::F5 => Some(Key::F(5)),
                NamedKey::F6 => Some(Key::F(6)),
                NamedKey::F7 => Some(Key::F(7)),
                NamedKey::F8 => Some(Key::F(8)),
                NamedKey::F9 => Some(Key::F(9)),
                NamedKey::F10 => Some(Key::F(10)),
                NamedKey::F11 => Some(Key::F(11)),
                NamedKey::F12 => Some(Key::F(12)),
                _ => None,
            },
            WKey::Character(s) => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Some(Key::Char(c)),
                    _ => None,
                }
            }
            _ => None,
        };
        let handled = key.is_some_and(|key| {
            let code = physical_char(event.physical_key);
            app.event(InputEvent::Key(KeyEvent { key, code, modifiers: modifiers(self.modifiers), repeat: event.repeat }))
        });
        if handled {
            return;
        }
        // Option (Alt) types characters on macOS; Control and Command make shortcuts.
        let shortcut = self.modifiers.control_key() || self.modifiers.super_key();
        if let Some(t) = event.text.as_deref().filter(|t| !shortcut && !t.chars().any(char::is_control)) {
            app.type_text(t);
        }
    }
}

impl ApplicationHandler<ShellEvent> for Shell {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        // A deadline the app asked for (a long press coming due, momentum's next frame).
        if let StartCause::ResumeTimeReached { .. } = cause {
            let _in = InLoop::enter(&self.shared, event_loop);
            self.tick();
            self.show();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let last_closed = self.app.as_ref().is_some_and(|a| a.windows().is_empty());
        if self.done || last_closed {
            event_loop.exit();
            return;
        }
        // Files dropped (one event per file): the drop, once all are in; files still dragged
        // over a window follow the pointer.
        let mut dragging = false;
        if let Some(app) = self.app.clone() {
            let _in = InLoop::enter(&self.shared, event_loop);
            for id in app.windows() {
                let Some(mut w) = app.window(id) else { continue };
                let (dropped, hover) = ww(&w, |x| {
                    let dropped = std::mem::take(&mut x.dropped);
                    if !dropped.is_empty() {
                        x.hovering.clear();
                    }
                    let (px, py) = x.pointer();
                    (dropped, (!x.hovering.is_empty()).then(|| (px, py, x.hovering.clone())))
                })
                .unwrap_or_default();
                if !dropped.is_empty() {
                    let (x, y) = ww(&w, |x| x.cursor).unwrap_or_default();
                    w.event(InputEvent::FileDrag(FileDrag::Drop { x, y, paths: dropped }));
                }
                if let Some((x, y, paths)) = hover {
                    dragging = true;
                    w.event(InputEvent::FileDrag(FileDrag::Over { x, y, paths }));
                }
            }
            self.show();
        }
        let deadline = self.app.as_ref().and_then(|a| a.next_deadline()).map(|d| self.started + d);
        let follow = dragging.then(|| Instant::now() + std::time::Duration::from_millis(16));
        event_loop.set_control_flow(match deadline.into_iter().chain(follow).min() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ShellEvent) {
        let _in = InLoop::enter(&self.shared, event_loop);
        let e = match event {
            ShellEvent::Wake => {
                self.tick();
                if let Some(app) = self.app.clone() {
                    app.poll();
                    // Events native views sent (a page loaded, its title changed).
                    #[cfg(target_os = "macos")]
                    for id in app.windows() {
                        let Some(mut w) = app.window(id) else { continue };
                        let (events, snapshots) =
                            ww(&w, |x| x.native.as_ref().map(|n| (n.inbox.take(), n.inbox.take_snapshots()))).flatten().unwrap_or_default();
                        for (id, event) in events {
                            w.event(InputEvent::Native { id, event });
                        }
                        for (id, image) in snapshots {
                            w.event(InputEvent::NativeSnapshot { id, image });
                        }
                    }
                }
                self.show();
                return;
            }
            ShellEvent::Frame(id) => {
                self.tick();
                if let Some(w) = self.window_of(id)
                    && ww(&w, |x| x.on_tick()).unwrap_or(false)
                {
                    self.draw(&w);
                }
                return;
            }
            ShellEvent::Capture => {
                self.capture_step();
                return;
            }
            ShellEvent::Answer { window, id, answer } => {
                self.tick();
                if let Some(mut w) = self.window_of(window) {
                    w.event(InputEvent::Answer { id, answer });
                }
                self.show();
                return;
            }
            ShellEvent::Accessibility(e) => e,
        };
        let Some(mut w) = self.window_of(e.window_id) else { return };
        match e.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                w.restart_accessibility();
                ww(&w, |x| x.a11y.update_if_active(|| w.accessibility_update()));
            }
            accesskit_winit::WindowEvent::ActionRequested(req) => {
                w.event(InputEvent::Accessibility { target: req.target_node, action: req.action, data: req.data });
                self.show();
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }
        let _in = InLoop::enter(&self.shared, event_loop);
        let root = self.root.take().expect("root view");
        let app = App::new(platform::WinitPlatform::new(self.shared.clone()));
        let options = octara::platform::WindowOptions::new(self.options.title.clone()).size(self.options.size.0, self.options.size.1);
        let mut w = app.open_window(options, root).expect("the window");
        if self.options.system_fonts {
            app.use_system_fonts();
        }
        self.capture = capture::Job::from_env(&self.shared.proxy);
        if std::env::var("OCTARA_PICK").is_ok_and(|v| v == "1") {
            w.set_picking(true);
        }
        self.main = Some(w.id());
        self.app = Some(app.clone());
        self.show();
        if std::env::var("OCTARA_CONFORMANCE").is_ok_and(|v| v == "1") {
            let checks = octara::conformance::run(&app);
            for c in &checks {
                match &c.failure {
                    None => eprintln!("octara-shell: conformance {}: ok", c.name),
                    Some(why) => eprintln!("octara-shell: conformance {}: FAILED: {why}", c.name),
                }
            }
            println!("{}", octara::serde_json::to_string(&checks).expect("checks serialize"));
            self.failed = checks.iter().any(|c| c.failure.is_some());
            self.done = true;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let _in = InLoop::enter(&self.shared, event_loop);
        let Some(mut w) = self.window_of(id) else { return };
        ww(&w, |x| x.a11y.process_event(&x.window, &event));
        let scale = ww(&w, |x| x.scale()).unwrap_or(1.0);
        let cursor = ww(&w, |x| x.cursor).unwrap_or_default();
        match event {
            WindowEvent::CloseRequested => {
                if let Some(app) = &self.app {
                    app.close_window(w.id());
                }
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = &*self.shared.gpu.borrow() {
                    ww(&w, |x| x.resize(size.width, size.height, &g.renderer));
                }
                w.event(InputEvent::Resize { width: size.width as f32 / scale, height: size.height as f32 / scale, scale });
            }
            // The system asks (exposure), or the clock does where it is winit's.
            WindowEvent::RedrawRequested => {
                self.tick();
                self.draw(&w);
            }
            // Hidden windows get no drawables; draw again once visible.
            WindowEvent::Occluded(false) => {
                ww(&w, |x| x.pending = true);
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let at = (position.x as f32 / scale, position.y as f32 / scale);
                ww(&w, |x| x.cursor = at);
                self.pointer(&mut w, PointerEvent::mouse(Phase::Move, at.0, at.1).modifiers(modifiers(self.modifiers)));
            }
            WindowEvent::CursorLeft { .. } => {
                w.event(InputEvent::PointerLeave);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Button::Primary,
                    MouseButton::Right => Button::Secondary,
                    MouseButton::Middle => Button::Middle,
                    _ => return,
                };
                let phase = match state {
                    ElementState::Pressed => Phase::Down,
                    ElementState::Released => Phase::Up,
                };
                let (x, y) = cursor;
                self.pointer(&mut w, PointerEvent::mouse(phase, x, y).button(button).modifiers(modifiers(self.modifiers)));
            }
            WindowEvent::Touch(Touch { phase, location, id, .. }) => {
                let phase = match phase {
                    TouchPhase::Started => Phase::Down,
                    TouchPhase::Moved => Phase::Move,
                    TouchPhase::Ended => Phase::Up,
                    TouchPhase::Cancelled => Phase::Cancel,
                };
                let (x, y) = (location.x as f32 / scale, location.y as f32 / scale);
                self.pointer(&mut w, PointerEvent::touch(id, phase, x, y));
            }
            WindowEvent::Focused(on) => {
                if !on {
                    // The window lost the pointer: a held press or drag is cancelled.
                    let (x, y) = cursor;
                    self.pointer(&mut w, PointerEvent::mouse(Phase::Cancel, x, y));
                }
                w.event(InputEvent::Active(on));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x * LINE, -y * LINE),
                    MouseScrollDelta::PixelDelta(p) => (-p.x as f32 / scale, -p.y as f32 / scale),
                };
                let (x, y) = cursor;
                self.tick();
                w.wheel(x, y, dx, dy);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let at = Instant::now();
                self.tick();
                self.key(&mut w, &event);
                ww(&w, |x| x.traced(at, if event.repeat { "key (repeat)" } else { "key" }));
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                ww(&w, |x| x.traced(Instant::now(), "input method composition"));
                w.event(InputEvent::Composition(text));
            }
            WindowEvent::HoveredFile(path) => {
                let (x, y, paths) = ww(&w, |win| {
                    win.hovering.push(path);
                    let (x, y) = win.pointer();
                    (x, y, win.hovering.clone())
                })
                .unwrap_or_default();
                w.event(InputEvent::FileDrag(FileDrag::Over { x, y, paths }));
            }
            WindowEvent::DroppedFile(path) => {
                ww(&w, |win| win.dropped.push(path));
            }
            WindowEvent::HoveredFileCancelled => {
                ww(&w, |win| win.hovering.clear());
                w.event(InputEvent::FileDrag(FileDrag::Leave));
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                ww(&w, |x| x.traced(Instant::now(), "input method commit"));
                w.type_text(&text);
            }
            _ => return,
        }
        self.show();
    }
}
