//! Winit windows for Octara apps: window events become [`App`] input by position and
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
//! Live inspection: `OCTARA_INSPECT=/absolute/path.sock` enables a local Unix socket on macOS,
//! Linux and the iOS simulator. Requests use the Inspector JSON-lines protocol on the UI thread.
//!
//! `OCTARA_TRACE_GPU=1` adds CPU phase and GPU timestamp samples on stderr when the device
//! supports timestamp queries. `OCTARA_TRACE_INPUT=1` enables the CPU and input trace alone.
//! On macOS, `OCTARA_ASYNC_METAL=1` enables experimental drawable acquisition on a worker.
//! `OCTARA_METAL_ACQUIRE_DELAY_MS` injects an acquisition delay for live comparison.
//! `OCTARA_METAL_FRAME_LATENCY=1` requests two drawables instead of three for that experiment.
//! `OCTARA_RENDER_WORKER=1` also records and submits plain Metal frames and capture work on
//! the render worker. Readback completion is asynchronous on all supported hosts.
//! `OCTARA_TRACE_PRESENTED=1` reports hardware presentation timestamps for the Metal path.
//! `OCTARA_TRACE_FRAME_CLOCK=1` reports macOS display-link transitions.
//! `OCTARA_TRACE_NATIVE_INPUT=1` times macOS input dispatch and associates it with a drawing revision.
//!
//! ```no_run
//! octara_shell::run("Counter", octara_examples_counter);
//! # fn octara_examples_counter() -> octara::view::El { octara::view::text("Counter") }
//! ```

#[cfg(target_os = "ios")]
use octara::app::BackPhase;
use octara::app::{
    App, AppDefinition, AppMode, Button, FileDrag, InputEvent, Key, KeyEvent, Modifiers, Phase, PointerEvent, Window as AppWindow,
};
#[cfg(target_os = "ios")]
use octara::kernel::HandlerId;
use octara::platform::WindowOptions;
use octara::view::IntoView;
use platform::WinitWindow;
use presentation::Opportunity;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Instant;
use winit::application::ApplicationHandler;
#[cfg(not(target_os = "ios"))]
use winit::event::Ime;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, StartCause, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::WindowId;

#[cfg(target_os = "macos")]
mod ask_mac;
#[cfg(target_os = "ios")]
mod back_ios;
mod capture;
mod capture_output;
mod gesture;
mod gpu;
#[cfg(any(test, target_os = "ios"))]
mod ime_draft;
mod inspect;
#[cfg(target_os = "macos")]
mod menubar_mac;
#[cfg(target_os = "macos")]
mod metal_acquire;
#[cfg(target_os = "macos")]
mod metal_frame;
#[cfg(target_os = "macos")]
mod native_mac;
#[cfg(target_os = "macos")]
mod pace_mac;
mod platform;
mod presentation;
mod render_worker;
mod resize;
#[cfg(target_os = "ios")]
mod scene_ios;
#[cfg(target_os = "macos")]
mod scroll_mac;
#[cfg(target_os = "ios")]
mod text_input_ios;
#[cfg(target_os = "macos")]
mod text_input_mac;
mod trace;

/// Events sent to the loop from elsewhere: AccessKit's requests, the app's waker (another
/// thread finished background work or sent on a channel), a window's frame clock ticking.
#[derive(Debug)]
pub enum ShellEvent {
    Accessibility(accesskit_winit::Event),
    Wake,
    #[doc(hidden)]
    GpuLost(u64),
    #[doc(hidden)]
    RenderReady(u64),
    #[cfg(target_os = "ios")]
    IosSceneConnected,
    #[cfg(target_os = "ios")]
    IosBack {
        window: WindowId,
        id: u64,
        owner: Option<HandlerId>,
        phase: BackPhase,
        interactive: bool,
    },
    /// A capture in progress looks again (`OCTARA_CAPTURE`).
    Capture,
    /// A local Inspector request, returned only to the connection that submitted it.
    Inspect {
        line: String,
        reply: std::sync::mpsc::Sender<String>,
        connected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    },
    /// A window's frame clock ticked (a display refresh).
    Frame(WindowId),
    #[cfg(target_os = "macos")]
    #[doc(hidden)]
    DrawableReady(WindowId),
    /// A menu bar item was picked (by its keys: the chord): its command for the active window.
    Command {
        name: String,
        chord: Option<String>,
    },
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
pub struct Options {
    /// Requested properties of the first window.
    pub window: WindowOptions,
    /// Exit after the first window drew this many frames (smoke tests).
    pub exit_after_frames: Option<u32>,
    /// Resources prepared before the root is built. Defaults to system fonts and fallback.
    /// Use `AppConfig::default()` here for bundled fonts instead.
    pub app: octara::app::AppConfig,
}

impl Default for Options {
    fn default() -> Self {
        let exit_after_frames = std::env::var("OCTARA_EXIT_AFTER_FRAMES").ok().and_then(|v| v.parse().ok());
        Options { window: WindowOptions::new("Octara"), exit_after_frames, app: octara::app::AppConfig::default().with_system_fonts() }
    }
}

/// Opens a window showing `root` and runs until the app's last window is closed.
#[track_caller]
pub fn run<V: IntoView>(title: &str, root: impl Fn() -> V + 'static) {
    run_with(Options { window: WindowOptions::new(title), ..Options::default() }, root)
}

/// Runs with explicit window and App options.
#[track_caller]
pub fn run_with<V: IntoView>(options: Options, root: impl Fn() -> V + 'static) {
    run_definition(AppDefinition::new(options.app, options.window, root), options.exit_after_frames);
}

/// Runs the same application entry used by headless inspection, with native resources.
pub fn run_app(entry: impl FnOnce(AppMode, octara::app::AppConfig) -> Result<AppDefinition, String>) -> Result<(), String> {
    let options = Options::default();
    let definition = entry(AppMode::Native, options.app)?;
    run_definition(definition, options.exit_after_frames);
    Ok(())
}

fn run_definition(definition: AppDefinition, exit_after_frames: Option<u32>) {
    let event_loop = EventLoop::<ShellEvent>::with_user_event().build().expect("event loop");
    #[cfg(target_os = "ios")]
    scene_ios::register(event_loop.create_proxy());
    let trace_gpu = std::env::var_os("OCTARA_TRACE_GPU").is_some_and(|v| v != "0");
    let started = Instant::now();
    let shared = Rc::new(Shared {
        proxy: event_loop.create_proxy(),
        active: Cell::new(std::ptr::null()),
        gpu: RefCell::new(None),
        windows: RefCell::new(HashMap::new()),
        #[cfg(target_os = "macos")]
        menu_commands: menubar_mac::PendingCommands::default(),
        #[cfg(target_os = "macos")]
        async_metal: std::env::var_os("OCTARA_ASYNC_METAL").is_some_and(|v| v == "1"),
        #[cfg(target_os = "macos")]
        metal_frame_latency: if std::env::var_os("OCTARA_METAL_FRAME_LATENCY").is_some_and(|v| v == "1") { 1 } else { 2 },
        #[cfg(target_os = "macos")]
        metal_acquire_delay: std::time::Duration::from_millis(
            std::env::var("OCTARA_METAL_ACQUIRE_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0).min(1000),
        ),
        #[cfg(debug_assertions)]
        render_worker_hold: std::time::Duration::from_millis(
            std::env::var("OCTARA_RENDER_WORKER_HOLD_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0).min(1500),
        ),
        trace: trace_gpu || std::env::var_os("OCTARA_TRACE_INPUT").is_some_and(|v| v != "0"),
        trace_native_input: cfg!(target_os = "macos") && std::env::var_os("OCTARA_TRACE_NATIVE_INPUT").is_some_and(|v| v == "1"),
        trace_gpu,
        #[cfg(target_os = "macos")]
        trace_presented: std::env::var_os("OCTARA_TRACE_PRESENTED").is_some_and(|v| v == "1"),
        #[cfg(target_os = "macos")]
        trace_frame_clock: std::env::var_os("OCTARA_TRACE_FRAME_CLOCK").is_some_and(|v| v == "1"),
        trace_epoch: started,
    });
    let mut shell = Shell {
        definition: Some(definition),
        exit_after_frames,
        shared,
        app: None,
        main: None,
        active: None,
        modifiers: ModifiersState::empty(),
        started,
        gpu_poll_wake: None,
        deferred_gpu_loss: None,
        last_rendered: None,
        #[cfg(debug_assertions)]
        destroy_device_after_capture: std::env::var_os("OCTARA_DESTROY_DEVICE_AFTER_CAPTURE").is_some_and(|v| v == "1"),
        done: false,
        failed: false,
        capture: None,
        capture_writer: None,
        inspector: None,
        inspect_pending: None,
    };
    let result = event_loop.run_app(&mut shell);
    if let Some(app) = shell.app.take() {
        app.shutdown();
    }
    // `process::exit` below bypasses Drop, so shut down and clean the endpoint first.
    drop(shell.capture.take());
    if let Some(writer) = shell.capture_writer.take() {
        writer.shutdown();
    }
    drop(shell.inspector.take());
    result.expect("event loop");
    if shell.failed {
        std::process::exit(1);
    }
}

/// What the shell and its platform share: the loop's proxy, the loop running a callback now
/// (windows are made inside one), the GPU, and which app window each winit window shows.
pub(crate) struct Shared {
    pub(crate) proxy: EventLoopProxy<ShellEvent>,
    active: Cell<*const ActiveEventLoop>,
    pub(crate) gpu: RefCell<Option<gpu::Gpu>>,
    pub(crate) windows: RefCell<HashMap<WindowId, octara::app::WindowId>>,
    #[cfg(target_os = "macos")]
    pub(crate) menu_commands: menubar_mac::PendingCommands,
    #[cfg(target_os = "macos")]
    pub(crate) async_metal: bool,
    #[cfg(target_os = "macos")]
    pub(crate) metal_frame_latency: u32,
    #[cfg(target_os = "macos")]
    pub(crate) metal_acquire_delay: std::time::Duration,
    #[cfg(debug_assertions)]
    pub(crate) render_worker_hold: std::time::Duration,
    /// Optional CPU frame/input timings as JSON on stderr.
    pub(crate) trace: bool,
    trace_native_input: bool,
    /// `OCTARA_TRACE_GPU=1`: add renderer counts and GPU timestamp queries when supported.
    pub(crate) trace_gpu: bool,
    #[cfg(target_os = "macos")]
    pub(crate) trace_presented: bool,
    #[cfg(target_os = "macos")]
    pub(crate) trace_frame_clock: bool,
    pub(crate) trace_epoch: Instant,
}

impl Shared {
    pub(crate) fn gpu_available(&self) -> bool {
        self.gpu.borrow().as_ref().is_some_and(gpu::Gpu::available)
    }

    pub(crate) fn renderer_idle(&self) -> bool {
        self.gpu.borrow().as_ref().is_some_and(gpu::Gpu::idle)
    }

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
    definition: Option<AppDefinition>,
    exit_after_frames: Option<u32>,
    shared: Rc<Shared>,
    app: Option<App>,
    /// The first window (captures and `exit_after_frames` are its).
    main: Option<octara::app::WindowId>,
    /// The window that last became active (the menu bar's commands go to it).
    active: Option<octara::app::WindowId>,
    modifiers: ModifiersState,
    /// The app's clock starts at launch.
    started: Instant,
    /// A wake scheduled solely for GPU polling, outside the App's clock.
    gpu_poll_wake: Option<Instant>,
    deferred_gpu_loss: Option<u64>,
    last_rendered: Option<octara::app::WindowId>,
    #[cfg(debug_assertions)]
    destroy_device_after_capture: bool,
    /// `exit_after_frames` was reached, a capture or the conformance checks finished.
    done: bool,
    /// A terminal failure exits the process with status 1.
    failed: bool,
    /// A capture in progress (`OCTARA_CAPTURE`, see [`capture`]).
    capture: Option<capture::Job>,
    capture_writer: Option<capture_output::CaptureWriter>,
    inspector: Option<inspect::Server>,
    inspect_pending: Option<inspect::Pending>,
}

/// `f` on the winit window behind app window `w`.
fn ww<R>(w: &AppWindow, f: impl FnOnce(&mut WinitWindow) -> R) -> Option<R> {
    let pw = w.platform_window();
    pw.as_any().downcast_ref::<platform::WindowHost>().map(|host| f(&mut host.0.borrow_mut()))
}

#[cfg(target_os = "macos")]
fn ready_drawable_position(app: &App, windows: &[octara::app::WindowId]) -> Option<usize> {
    let now = Instant::now();
    windows
        .iter()
        .position(|id| app.window(*id).is_some_and(|w| w.frame_is_settled() && ww(&w, |host| host.ready_to_draw(now)).unwrap_or(false)))
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
    /// A menu key equivalent can post its command, then a later key reaches winit inline
    /// before that proxy event. Apply the earlier menu intent before interpreting that key.
    #[cfg(target_os = "macos")]
    fn menu_commands(&mut self) {
        let shared = self.shared.clone();
        shared.menu_commands.drain(|name, chord| {
            self.tick();
            let w = self.active.and_then(|id| self.app.as_ref()?.window(id)).or_else(|| self.main_window());
            if let Some(mut w) = w {
                w.event(InputEvent::Command { name, chord });
            }
        });
    }

    /// Native text callbacks only edit an owned mirror. Apply their ordered intents before
    /// the next framework input, then reconcile with what application handlers accepted.
    #[cfg(target_os = "macos")]
    fn native_text(&mut self) {
        let Some(app) = self.app.clone() else { return };
        for id in app.windows() {
            let Some(mut w) = app.window(id) else { continue };
            let (edits, input) = ww(&w, |x| x.text_input.as_ref().map(|i| i.take())).flatten().unwrap_or_default();
            if edits.is_empty() {
                continue;
            }
            let started = self.shared.trace.then(Instant::now);
            let publication = input.filter(|_| self.shared.trace_native_input).and_then(|input| input.publication(&w));
            for edit in edits {
                let current = ww(&w, |x| x.text_input.as_ref().is_some_and(|i| i.is_current(&edit))).unwrap_or(false);
                if !current {
                    break;
                }
                if !w.apply_text_edit(edit.owner, &edit.expected, &edit.edit) {
                    ww(&w, |x| {
                        if let Some(input) = &x.text_input
                            && input.is_current(&edit)
                        {
                            input.reject();
                        }
                    });
                    break;
                }
            }
            if let Some((publication, kind)) = publication {
                publication.log(self.shared.trace_epoch, &w, kind, None);
            }
            ww(&w, |x| {
                if let Some(input) = &x.text_input {
                    input.reconcile();
                }
                if let Some((input, started)) = input.zip(started) {
                    x.record_input(trace::Input::native(input, started));
                }
            });
        }
    }

    #[cfg(target_os = "ios")]
    fn native_text_ios(&mut self) {
        let Some(app) = self.app.clone() else { return };
        for id in app.windows() {
            let Some(mut window) = app.window(id) else { continue };
            while let Some(change) = ww(&window, |host| host.ios_input.take()).flatten() {
                let delivery = change.input.deliver(&mut window, change.owner, &change.expected, |window| {
                    ww(window, |host| host.ios_input.document(&change).cloned()).flatten()
                });
                if !ww(&window, |host| host.ios_input.reconcile(&change, delivery)).unwrap_or(false) {
                    break;
                }
            }
        }
    }

    #[cfg(target_os = "ios")]
    fn ios_back(&mut self, window: WindowId, id: u64, owner: Option<HandlerId>, phase: BackPhase, interactive: bool) {
        let Some(mut window) = self.window_of(window) else { return };
        let Some(permitted) = ww(&window, |host| host.ios_back.as_ref()?.permits_delivery(id)).flatten() else {
            return;
        };
        self.tick();
        // The recognizer saw the frame that was drawn before this proxy event. A timer,
        // native edit or other input can move the current command before dispatch.
        let accepted = permitted
            && (phase != BackPhase::Started || owner.is_some() && owner == window.back_gesture_target())
            && window.event(InputEvent::BackGesture { id, phase });
        if matches!(phase, BackPhase::Completed | BackPhase::Cancelled) {
            let eligible = window.back_gesture_target().is_some();
            ww(&window, |host| {
                if let Some(back) = &mut host.ios_back
                    && back.settle(id, accepted, eligible)
                {
                    host.request_draw();
                }
            });
        }
        if self.shared.trace {
            let (phase, progress) = match phase {
                BackPhase::Started => ("started", None),
                BackPhase::Progress(progress) => ("progress", Some(progress)),
                BackPhase::Cancelled => ("cancelled", None),
                BackPhase::Completed => ("completed", None),
            };
            eprintln!(
                "{}",
                octara::serde_json::json!({
                    "schema": "octara.back_trace/1",
                    "source": "uikit_navigation_controller",
                    "window": window.id().number(),
                    "id": id,
                    "phase": phase,
                    "progress": progress,
                    "interactive": interactive,
                    "accepted": accepted,
                })
            );
        }
        self.show();
    }

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
        if self.app_failed() {
            return;
        }
        if let Some(app) = &self.app {
            if let Some(pending) = &self.inspect_pending {
                pending.cancel_disconnected(app);
            }
            let started = self.shared.trace.then(Instant::now);
            app.advance_to(self.started.elapsed());
            if let Some(started) = started {
                eprintln!(
                    "{}",
                    octara::serde_json::json!({
                        "schema": "octara.update_trace/1",
                        "phase": "advance_to",
                        "app_clock_ms": app.now().as_millis() as u64,
                        "elapsed_us": started.elapsed().as_secs_f64() * 1_000_000.0,
                    })
                );
            }
        }
    }

    /// Apply an actual native client-area size through the same path as a resize event.
    fn resize_window(&self, w: &mut AppWindow, size: winit::dpi::PhysicalSize<u32>) {
        let changed = ww(w, |host| (host.config.width, host.config.height) != (size.width, size.height)) == Some(true);
        let mut traced_window = None;
        if changed && let Some(g) = &*self.shared.gpu.borrow() {
            let limit = g.device().limits().max_texture_dimension_2d;
            if resize::validate_physical(size, limit).is_ok() {
                ww(w, |host| {
                    host.resize(size);
                    traced_window = self.shared.trace.then(|| host.window.id());
                });
            }
        }
        self.sync_viewport(w, size);
        if let Some(window) = traced_window {
            let at = Instant::now();
            trace::resize(self.shared.trace_epoch, window, size, at, trace::ResizePhase::Viewport);
            ww(w, |host| host.trace_resize_eligibility(at));
        }
    }

    fn sync_viewport(&self, w: &mut AppWindow, size: winit::dpi::PhysicalSize<u32>) {
        let Some(viewport) = ww(w, |host| {
            host.viewport_checked = true;
            host.viewport(size)
        }) else {
            return;
        };
        let (width, height) = viewport.size;
        if w.size() != viewport.size || w.scale() != viewport.scale || w.kernel().safe_area() != viewport.safe_area {
            w.event(InputEvent::Resize { width, height, scale: viewport.scale, safe_area: viewport.safe_area });
        }
    }

    /// One pointer event at the window's pointer (mouse) or the touch's (finger).
    fn pointer(&mut self, w: &mut AppWindow, ev: PointerEvent, received: Option<Instant>) {
        let at = self.shared.trace.then(Instant::now);
        self.tick();
        let publication = trace::WindowInput::start(received, w).map(|publication| {
            let kind = match ev.phase {
                Phase::Down => "down",
                Phase::Up => "up",
                Phase::Move if w.held_pointer().is_some() => "drag",
                Phase::Move => "move",
                Phase::Cancel => "cancel",
            };
            (publication, kind)
        });
        w.event(InputEvent::Pointer(ev));
        if let Some((publication, kind)) = publication {
            publication.log(self.shared.trace_epoch, w, kind, (kind == "drag").then_some((ev.x, ev.y)));
        }
        #[cfg(target_os = "macos")]
        if ev.phase == Phase::Down {
            ww(w, |x| {
                if let Some(input) = &x.text_input {
                    input.focus();
                }
            });
        }
        // Moves come in bursts, and a cancel (the window losing focus) shows nothing new.
        let what = match ev.phase {
            Phase::Down => "pointer down",
            Phase::Up => "pointer up",
            Phase::Move | Phase::Cancel => return,
        };
        ww(w, |x| x.traced(at, what));
    }

    fn show(&mut self) {
        self.show_frames(false);
    }

    /// Reserves the renderer before waiting for the device. The worker returns it with a
    /// device-wide result, so newer window requests can use the same drain.
    fn drain_for_surfaces(&mut self) {
        let epoch = self.shared.trace.then_some(self.shared.trace_epoch);
        let result = self
            .shared
            .gpu
            .borrow_mut()
            .as_mut()
            .filter(|gpu| gpu.idle() && gpu.available())
            .map_or(Ok(()), |gpu| gpu.start_surface_drain(epoch));
        if let Err(error) = result {
            self.gpu_failed(&error);
        }
    }

    /// Applies only the requests that still belong to open windows with free drawables.
    /// Run this before reopening admission for drawing, captures or profiler submits.
    fn apply_surface_configurations(&self) -> Result<(), String> {
        let Some(app) = &self.app else { return Ok(()) };
        for mut window in app.windows().into_iter().filter_map(|id| app.window(id)) {
            let Some(size) = ww(&window, |host| host.ready_to_configure().then(|| host.drawable_size())).flatten() else { continue };
            // The native window may already reflect a size whose resize event is still queued.
            self.resize_window(&mut window, size);
            let gpu = self.shared.gpu.borrow();
            let Some(gpu) = gpu.as_ref().filter(|gpu| gpu.available()) else { break };
            ww(&window, |host| host.apply_surface_configuration(gpu)).transpose()?;
        }
        Ok(())
    }

    /// Schedules each settled window on its own clock and updates assistive technology.
    fn show_frames(&mut self, clock_sampled: bool) {
        #[cfg(target_os = "macos")]
        let mut clock_sampled = clock_sampled;
        if self.done || self.app_failed() {
            return;
        }
        if let Some(generation) = self.deferred_gpu_loss.take() {
            self.on_gpu_lost(generation);
            return;
        }
        let Some(app) = self.app.clone() else { return };
        let mut windows = app.windows();
        if self.shared.renderer_idle()
            && windows.iter().any(|id| app.window(*id).is_some_and(|w| ww(&w, |host| host.ready_to_configure()).unwrap_or(false)))
        {
            self.drain_for_surfaces();
            if self.done {
                return;
            }
        }
        #[cfg(target_os = "macos")]
        {
            // Publish all the opportunities the main run loop already delivered before any
            // window can block in a synchronous acquire. Their wakes can arrive in either order.
            let handled_at = Instant::now();
            let ticks: Vec<_> = windows
                .iter()
                .filter_map(|id| {
                    let w = app.window(*id)?;
                    let opportunity = ww(&w, |host| host.take_display_tick(handled_at)).flatten()?;
                    Some((w, opportunity))
                })
                .collect();
            if !ticks.is_empty() && !clock_sampled && self.shared.renderer_idle() {
                self.tick();
                clock_sampled = true;
            }
            let now = Instant::now();
            for (w, opportunity) in ticks {
                ww(&w, |host| host.offer_display_tick(opportunity, now));
            }
        }
        if let Some(position) = windows.iter().position(|id| Some(*id) == self.last_rendered) {
            windows.rotate_left(position + 1);
        }
        // A ready drawable can use the device now. Prefer the first ready window in the
        // same rotation, before work that would take a fresh surface acquisition.
        #[cfg(target_os = "macos")]
        if self.shared.renderer_idle()
            && let Some(ready) = ready_drawable_position(&app, &windows)
        {
            windows[..=ready].rotate_right(1);
        }
        let mut frames = VecDeque::new();
        for &id in &windows {
            let Some(mut w) = app.window(id) else { continue };
            if let Some(size) = ww(&w, |host| (cfg!(target_os = "ios") || !host.viewport_checked).then(|| host.drawable_size())).flatten() {
                self.resize_window(&mut w, size);
                if ww(&w, |host| host.ready_to_configure()) == Some(true) {
                    self.drain_for_surfaces();
                }
            }
            if !w.frame_is_settled() {
                ww(&w, |x| {
                    x.pause_frame_clock();
                    #[cfg(target_os = "macos")]
                    x.discard_metal_drawable();
                });
                continue;
            }
            #[cfg(target_os = "ios")]
            {
                let back_eligible = w.back_gesture_target().is_some();
                ww(&w, |x| x.sync_ios_back(back_eligible));
            }
            let draw = ww(&w, |x| x.schedule_frame(Instant::now())).flatten();
            if let Some((opportunity, continuous)) = draw {
                frames.push_back((w, opportunity, continuous));
            }
        }
        self.admit_render_work(frames, clock_sampled);
        for w in windows.into_iter().filter_map(|id| app.window(id)).filter(AppWindow::frame_is_settled) {
            ww(&w, |x| x.a11y.update_if_active(|| w.accessibility_update()));
        }
    }

    fn admit_render_work(&mut self, mut frames: VecDeque<(AppWindow, Opportunity, bool)>, clock_sampled: bool) {
        // Sample an animation that reached its fallback deadline before choosing GPU work.
        if !clock_sampled && frames.iter().any(|(_, _, continuous)| *continuous) {
            self.tick();
        }
        // Native snapshots can finish even while the renderer belongs to another job.
        let mut capture = self.capture.as_mut().and_then(capture::Job::ready_to_render);
        while !self.done && self.shared.renderer_idle() {
            let now = Instant::now();
            let next_window = match capture {
                Some((_, ready_at)) => frames.iter().position(|(window, opportunity, _)| {
                    window.frame_is_settled() && !presentation::capture_precedes_frame(ready_at, *opportunity, now)
                }),
                None => Some(0),
            };
            let Some(next_window) = next_window else {
                let capture = capture.take().expect("a capture is ready");
                let competing_frame = frames
                    .iter()
                    .find(|(window, opportunity, _)| opportunity.valid(now) && window.frame_is_settled())
                    .map(|(window, opportunity, _)| (window.id(), *opportunity));
                if self.submit_ready_capture() {
                    self.trace_admission(now, "capture", capture, competing_frame);
                }
                continue;
            };
            let Some((window, opportunity, _)) = frames.remove(next_window) else { break };
            if self.draw_for(&window, opportunity)
                && let Some(capture) = capture
            {
                self.trace_admission(now, "window", capture, Some((window.id(), opportunity)));
            }
            // Acquisition can request configuration during a draw.
            if ww(&window, |host| host.ready_to_configure()) == Some(true) {
                self.drain_for_surfaces();
            }
        }
    }

    fn trace_admission(
        &self,
        at: Instant,
        selected: &'static str,
        capture: (octara::app::WindowId, Instant),
        frame: Option<(octara::app::WindowId, Opportunity)>,
    ) {
        if self.shared.trace
            && let Some(gpu) = self.shared.gpu.borrow().as_ref()
        {
            trace::device_admission(self.shared.trace_epoch, at, gpu.generation.number, selected, capture, frame);
        }
    }

    /// Draws the latest publication and reports whether a frame submitted or took the worker.
    fn draw_for(&mut self, w: &AppWindow, opportunity: Opportunity) -> bool {
        if self.app_failed() || !w.frame_is_settled() || !self.shared.renderer_idle() || !ww(w, |x| x.can_present()).unwrap_or(false) {
            return false;
        }
        let attempted = Instant::now();
        if !opportunity.valid(attempted) {
            ww(w, |x| x.skip_opportunity(opportunity, "expired_display_opportunity"));
            return false;
        }
        let prepared =
            self.shared.gpu.borrow().as_ref().is_some_and(|g| ww(w, |x| x.prepare_draw(w.scale(), g, opportunity)).unwrap_or(false));
        if !prepared {
            return false;
        }
        let now = Instant::now();
        if !opportunity.valid(now) {
            ww(w, |x| x.skip_opportunity(opportunity, "expired_display_opportunity"));
            return false;
        }
        let started = self.shared.trace.then_some(attempted);
        let target = ww(w, |x| x.visual_target(opportunity)).flatten();
        let frame = match target {
            Some(target) => w.published_frame_for_display(target.saturating_duration_since(self.started)),
            None => w.published_frame(),
        };
        let Some(frame) = frame else {
            ww(w, |x| {
                x.request_draw();
                x.pause_frame_clock();
                #[cfg(target_os = "macos")]
                x.discard_metal_drawable();
                if self.shared.trace {
                    opportunity.skipped(x.window.id(), self.started, "no_settled_drawing");
                }
            });
            return false;
        };
        #[cfg(target_os = "ios")]
        let back_owner = w.back_gesture_target();
        #[cfg(target_os = "ios")]
        ww(w, |x| x.sync_ios_back(back_owner.is_some()));
        let trace = started.map(|at| trace::Frame::new(at, w.app().now().as_millis() as u64, opportunity));
        self.last_rendered = Some(w.id());
        let submitted = {
            let mut gpu = self.shared.gpu.borrow_mut();
            let Some(g) = gpu.as_mut() else { return false };
            ww(w, |x| x.draw(&frame, g, attempted, trace)).unwrap_or(false)
        };
        #[cfg(target_os = "ios")]
        if submitted {
            ww(w, |x| x.did_draw_ios_back(back_owner));
        }
        self.finish_window_draw(w, submitted);
        submitted || !self.shared.renderer_idle()
    }

    fn finish_window_draw(&mut self, w: &AppWindow, submitted: bool) {
        if submitted
            && Some(w.id()) == self.main
            && let Some(n) = self.exit_after_frames
        {
            let (frames, (width, height), scale) = ww(w, |x| (x.frames, (x.config.width, x.config.height), x.scale())).unwrap_or_default();
            if frames >= n {
                eprintln!("octara-shell: drew {frames} frames at {width}×{height} (scale {scale}), exiting");
                self.done = true;
            } else {
                ww(w, |x| x.request_draw());
            }
        }
        ww(w, |x| x.sync_frame_clock());
    }

    #[cfg(target_os = "macos")]
    fn drawable_ready(&mut self, id: WindowId) {
        if let Some(w) = self.window_of(id) {
            self.tick();
            {
                let gpu = self.shared.gpu.borrow();
                if let Some(gpu) = gpu.as_ref() {
                    ww(&w, |host| {
                        host.take_metal_drawable(gpu);
                        if self.shared.trace {
                            host.trace_resize_eligibility(Instant::now());
                        }
                    });
                }
            }
            self.show_frames(true);
        } else {
            self.show();
        }
    }

    fn render_ready(&mut self, generation: u64) {
        let completed = {
            let mut gpu = self.shared.gpu.borrow_mut();
            let Some(gpu) = gpu.as_mut().filter(|gpu| gpu.generation.number == generation) else { return };
            gpu.finish_work()
        };
        match completed {
            Ok(Some(gpu::RenderCompletion::Window(frame))) => {
                if let Some(w) = self.window_of(frame.window) {
                    let latest = w.published_revision();
                    let submitted = {
                        let mut gpu = self.shared.gpu.borrow_mut();
                        gpu.as_mut().is_some_and(|gpu| ww(&w, |host| host.rendered(gpu, *frame, latest)).unwrap_or(false))
                    };
                    self.finish_window_draw(&w, submitted);
                }
            }
            Ok(Some(gpu::RenderCompletion::Capture)) => {}
            Ok(Some(gpu::RenderCompletion::SurfaceDrain(result))) => {
                let result = match result {
                    Ok(true) => self.apply_surface_configurations(),
                    Ok(false) => {
                        // A long GPU submission may take more than one bounded wait. Retry
                        // the latest configuration before admitting another GPU submission.
                        self.show();
                        return;
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    self.gpu_failed(&error);
                    return;
                }
            }
            Ok(None) => return,
            Err(error) => {
                self.gpu_failed(&error);
                return;
            }
        }
        self.show();
    }

    /// A key press: the app's key routing first (focus, keymap, commands); if nothing took it,
    /// the text it typed goes to the focused field.
    fn key(&mut self, app: &mut AppWindow, event: &winit::event::KeyEvent, text: bool) -> bool {
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
            return true;
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
            return true;
        }
        if !text {
            return false;
        }
        // Option (Alt) types characters on macOS; Control and Command make shortcuts.
        let shortcut = self.modifiers.control_key() || self.modifiers.super_key();
        if let Some(t) = event.text.as_deref().filter(|t| !shortcut && !t.chars().any(char::is_control)) {
            app.type_text(t);
            return true;
        }
        false
    }
}

impl Shell {
    fn on_gpu_lost(&mut self, generation: u64) {
        if self.done {
            return;
        }
        let Some(app) = self.app.clone() else { return };
        let mut windows = app.windows().into_iter().filter_map(|id| app.window(id)).collect::<Vec<_>>();
        let Some(first) = windows.first() else { return };
        if self.shared.gpu.borrow().as_ref().is_some_and(|gpu| gpu.generation.loss_for(generation).is_some()) {
            let waiting = !self.shared.renderer_idle();
            #[cfg(target_os = "macos")]
            let mut waiting = waiting;
            #[cfg(target_os = "macos")]
            for window in &windows {
                waiting |= ww(window, WinitWindow::retire_metal_drawable).unwrap_or(false);
            }
            if waiting {
                self.deferred_gpu_loss = Some(generation);
                return;
            }
            self.deferred_gpu_loss = None;
        }
        let recover = || {
            let mut shared_gpu = self.shared.gpu.borrow_mut();
            let Some(gpu) = shared_gpu.as_ref() else { return Ok(false) };
            let Some((reason, message)) = gpu.generation.loss_for(generation) else { return Ok(false) };
            eprintln!("octara-shell: GPU device generation {generation} lost ({reason:?}) {message}");
            let replacement =
                ww(first, |host| gpu.replacement(&host.surface, &self.shared.proxy)).ok_or("the app has no GPU surface")??;
            for window in &windows {
                ww(window, |host| host.recover_surface(&replacement));
            }
            if !replacement.available() {
                return Err("the replacement GPU device was lost during surface recovery".into());
            }
            *shared_gpu = Some(replacement);
            Ok::<_, String>(true)
        };
        match recover() {
            Ok(true) => {
                self.gpu_poll_wake = None;
                for window in &mut windows {
                    if let Some(size) = ww(window, |host| host.drawable_size()) {
                        self.sync_viewport(window, size);
                    }
                }
                self.show();
            }
            Ok(false) => {}
            Err(why) => self.gpu_failed(&why),
        }
    }

    fn gpu_failed(&mut self, why: &str) {
        eprintln!("octara-shell: cannot recover the GPU device: {why}");
        self.failed = true;
        self.done = true;
        self.inspect_pending.take();
        self.capture.take();
    }

    fn on_new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if self.app_failed() {
            return;
        }
        // A task or input deadline, or a frame for a window without display callbacks.
        if let StartCause::ResumeTimeReached { requested_resume, .. } = cause
            && self.gpu_poll_wake.take() != Some(requested_resume)
        {
            let _in = InLoop::enter(&self.shared, event_loop);
            self.tick();
            self.show_frames(true);
        }
    }

    fn on_about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.app_failed() {
            self.failed = true;
            if let Some(gpu) = self.shared.gpu.borrow_mut().as_mut() {
                gpu.cancel_work();
            }
            if self.app.as_ref().is_some_and(|app| app.is_shutdown()) {
                event_loop.exit();
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            return;
        }
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
        let now = Instant::now();
        let deadline = self.app.as_ref().and_then(|a| a.next_timer_deadline()).map(|d| self.started + d);
        let fallback = self.app.as_ref().and_then(|app| {
            app.windows()
                .into_iter()
                .filter_map(|id| {
                    let w = app.window(id)?;
                    let d = ww(&w, |x| x.fallback_frame_deadline(now)).flatten()?;
                    w.frame_is_settled().then_some(d)
                })
                .min()
        });
        let follow = dragging.then(|| now + std::time::Duration::from_millis(16));
        let deadline = deadline.into_iter().chain(fallback).chain(follow).min();
        self.gpu_poll_wake = None;
        let poll = self.shared.gpu.borrow_mut().as_mut().map_or(Ok(false), |gpu| gpu.poll(self.shared.trace_gpu));
        match poll {
            Ok(true) => {
                let poll = Instant::now() + std::time::Duration::from_millis(2);
                self.gpu_poll_wake = deadline.is_none_or(|app| poll < app).then_some(poll);
            }
            Ok(false) => {}
            Err(why) => {
                self.gpu_failed(&why);
                event_loop.exit();
                return;
            }
        }
        event_loop.set_control_flow(match self.gpu_poll_wake.or(deadline) {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }

    fn on_user_event(&mut self, event_loop: &ActiveEventLoop, event: ShellEvent) {
        let inspect_entered = (self.shared.trace && matches!(event, ShellEvent::Inspect { .. })).then(Instant::now);
        if self.app_failed() {
            if let ShellEvent::Inspect { line, reply, connected } = event {
                self.inspect_request(line, reply, connected, inspect_entered);
            }
            return;
        }
        let _in = InLoop::enter(&self.shared, event_loop);
        if let ShellEvent::GpuLost(generation) = event {
            self.on_gpu_lost(generation);
            return;
        }
        #[cfg(target_os = "ios")]
        self.native_text_ios();
        #[cfg(target_os = "macos")]
        {
            self.native_text();
            self.menu_commands();
        }
        let e = match event {
            ShellEvent::GpuLost(_) => unreachable!("handled before app events"),
            ShellEvent::RenderReady(generation) => {
                self.render_ready(generation);
                return;
            }
            #[cfg(target_os = "ios")]
            ShellEvent::IosSceneConnected => {
                if !scene_ios::connected() {
                    return;
                }
                if self.app.is_none() {
                    self.start_app();
                } else if let Some(window) = self.main_window()
                    && let Some(Err(error)) = ww(&window, |host| {
                        scene_ios::attach(&host.window).map(|native| {
                            native.makeKeyAndVisible();
                            host.set_occluded(false);
                        })
                    })
                {
                    eprintln!("octara-shell: {error}");
                }
                self.show();
                return;
            }
            #[cfg(target_os = "ios")]
            ShellEvent::IosBack { window, id, owner, phase, interactive } => {
                self.ios_back(window, id, owner, phase, interactive);
                return;
            }
            ShellEvent::Wake => {
                self.tick();
                if let Some(app) = self.app.clone() {
                    let started = self.shared.trace.then(Instant::now);
                    app.poll();
                    if let Some(started) = started {
                        eprintln!(
                            "{}",
                            octara::serde_json::json!({
                                "schema": "octara.update_trace/1",
                                "phase": "poll",
                                "app_clock_ms": app.now().as_millis() as u64,
                                "elapsed_us": started.elapsed().as_secs_f64() * 1_000_000.0,
                            })
                        );
                    }
                    // Events native views sent (a page loaded, its title changed).
                    #[cfg(target_os = "macos")]
                    for id in app.windows() {
                        let Some(mut w) = app.window(id) else { continue };
                        let (events, snapshots) =
                            ww(&w, |x| x.native.as_ref().map(|n| (n.inbox.take(), n.inbox.take_snapshots()))).flatten().unwrap_or_default();
                        for (id, event) in events {
                            w.event(InputEvent::Native { id, event });
                        }
                        for (request, image) in snapshots {
                            w.event(InputEvent::NativeSnapshot { request, image });
                        }
                    }
                }
                self.show();
                return;
            }
            ShellEvent::Frame(id) => {
                #[cfg(target_os = "macos")]
                if let Some(w) = self.window_of(id)
                    && ww(&w, |host| host.acknowledge_display_wake()) == Some(true)
                {
                    self.show();
                }
                #[cfg(not(target_os = "macos"))]
                let _ = id;
                return;
            }
            #[cfg(target_os = "macos")]
            ShellEvent::DrawableReady(id) => {
                self.drawable_ready(id);
                return;
            }
            ShellEvent::Capture => {
                self.inspect_poll();
                self.capture_step();
                self.show();
                return;
            }
            ShellEvent::Inspect { line, reply, connected } => {
                self.inspect_request(line, reply, connected, inspect_entered);
                return;
            }
            ShellEvent::Command { name, chord } => {
                self.tick();
                let w = self.active.and_then(|id| self.app.as_ref()?.window(id)).or_else(|| self.main_window());
                if let Some(mut w) = w {
                    w.event(InputEvent::Command { name, chord });
                }
                self.show();
                return;
            }
            ShellEvent::Answer { window, id, answer } => {
                self.tick();
                if let Some(mut w) = self.window_of(window) {
                    // No move came while a menu or a sheet had the pointer: where it is now.
                    if let Some((x, y)) = ww(&w, |x| x.pointer()) {
                        w.event(InputEvent::Pointer(PointerEvent::mouse(Phase::Move, x, y).modifiers(modifiers(self.modifiers))));
                    }
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
                // AccessKit waits for a full tree. An unsettled window sends it on the next eligible update.
                w.restart_accessibility();
                if w.frame_is_settled() {
                    ww(&w, |x| x.a11y.update_if_active(|| w.accessibility_update()));
                }
            }
            accesskit_winit::WindowEvent::ActionRequested(req) => {
                w.event(InputEvent::Accessibility { target: req.target_node, action: req.action, data: req.data });
                self.show();
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
    }

    fn on_resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.is_some() {
            #[cfg(target_os = "ios")]
            self.ios_active(event_loop, true);
            return;
        }
        #[cfg(target_os = "ios")]
        if !scene_ios::connected() {
            return;
        }
        let _in = InLoop::enter(&self.shared, event_loop);
        self.start_app();
    }

    fn start_app(&mut self) {
        let definition = self.definition.take().expect("initial application definition");
        let mut w = definition.start(platform::WinitPlatform::new(self.shared.clone()), |_| {}).expect("the window");
        let app = w.app();
        self.capture = capture::Job::from_env(w.id(), &self.shared.proxy);
        if std::env::var("OCTARA_PICK").is_ok_and(|v| v == "1") {
            w.set_picking(true);
        }
        self.main = Some(w.id());
        self.app = Some(app.clone());
        self.show();
        match inspect::Server::from_env(self.shared.proxy.clone(), self.shared.trace) {
            Ok(server) => self.inspector = server,
            Err(why) => {
                eprintln!("octara-shell: inspector: {why}");
                self.failed = true;
                self.done = true;
                return;
            }
        }
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

    #[cfg(target_os = "ios")]
    fn ios_active(&mut self, event_loop: &ActiveEventLoop, active: bool) {
        let Some(mut window) = self.main_window().filter(|_| !self.app_failed()) else { return };
        let _in = InLoop::enter(&self.shared, event_loop);
        // End the contact before the next clock tick can run a held gesture's deadline.
        let held = window.held_pointer();
        let edit_to_cancel = ww(&window, |host| host.ios_input.set_active(active)).flatten();
        Self::set_window_active(&mut window, active);
        if let Some((owner, expected)) = edit_to_cancel {
            window.apply_text_edit(owner, &expected, &octara_primitives::text::TextEdit::Cancel);
        }
        ww(&window, |host| host.set_occluded(!active));
        if self.shared.trace {
            eprintln!(
                "{}",
                octara::serde_json::json!({
                    "schema": "octara.lifecycle_trace/1",
                    "window": window.id().number(),
                    "active": active,
                    "held_before": held.map(|(id, _)| id),
                    "held_after": window.held_pointer().map(|(id, _)| id),
                })
            );
        }
        if active {
            self.tick();
            self.show();
        }
    }

    fn set_window_active(window: &mut AppWindow, active: bool) {
        #[cfg(target_os = "ios")]
        if !active {
            ww(window, |host| {
                if let Some(back) = &host.ios_back {
                    back.interrupt();
                }
            });
        }
        window.event(InputEvent::Active(active));
    }

    fn on_window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let resize_event = match &event {
            WindowEvent::Resized(size) if self.shared.trace => Some((Instant::now(), *size)),
            _ => None,
        };
        let received = self.shared.trace_native_input.then(Instant::now);
        let _in = InLoop::enter(&self.shared, event_loop);
        if self.app_failed() {
            if matches!(event, WindowEvent::CloseRequested) {
                self.app.as_ref().expect("failed App").shutdown();
                self.done = true;
                event_loop.exit();
            }
            return;
        }
        #[cfg(target_os = "ios")]
        self.native_text_ios();
        #[cfg(target_os = "macos")]
        {
            self.native_text();
            self.menu_commands();
        }
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
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                // A queued event can describe an older request; configure what the native
                // window actually has now, just as Inspector feedback observes it.
                if let Some(size) = ww(&w, |host| host.drawable_size()) {
                    if let Some((at, reported)) = resize_event {
                        trace::resize(self.shared.trace_epoch, id, size, at, trace::ResizePhase::Event(reported));
                    }
                    self.resize_window(&mut w, size);
                }
            }
            // The system asks for exposure. Retry a surface that was occluded.
            WindowEvent::RedrawRequested => {
                ww(&w, |x| x.redraw());
                self.tick();
                self.show_frames(true);
                return;
            }
            // Hidden windows get no drawables; draw again once visible.
            WindowEvent::Occluded(occluded) => {
                ww(&w, |x| x.set_occluded(occluded));
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let at = (position.x as f32 / scale, position.y as f32 / scale);
                ww(&w, |x| x.cursor = at);
                // Winit also samples the cursor before button and wheel delivery. Those
                // identity samples are part of those events, not extra drag movements.
                let moved = received.filter(|_| ww(&w, |x| x.last_traced_motion.replace(at) != Some(at)) == Some(true));
                self.pointer(&mut w, PointerEvent::mouse(Phase::Move, at.0, at.1).modifiers(modifiers(self.modifiers)), moved);
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
                self.pointer(&mut w, PointerEvent::mouse(phase, x, y).button(button).modifiers(modifiers(self.modifiers)), received);
            }
            WindowEvent::Touch(Touch { phase, location, id, .. }) => {
                let phase = match phase {
                    TouchPhase::Started => Phase::Down,
                    TouchPhase::Moved => Phase::Move,
                    TouchPhase::Ended => Phase::Up,
                    TouchPhase::Cancelled => Phase::Cancel,
                };
                let (x, y) = (location.x as f32 / scale, location.y as f32 / scale);
                self.pointer(&mut w, PointerEvent::touch(id, phase, x, y), received);
                if self.shared.trace {
                    eprintln!(
                        "{}",
                        octara::serde_json::json!({
                            "schema": "octara.touch_trace/1",
                            "window": w.id().number(),
                            "contact": id,
                            "phase": phase,
                            "position": [x, y],
                            "held": w.held_pointer().map(|(id, _)| id),
                        })
                    );
                }
            }
            WindowEvent::Focused(on) => {
                if on {
                    self.active = Some(w.id());
                }
                Self::set_window_active(&mut w, on);
                if !on {
                    self.tick();
                }
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                let at = self.shared.trace.then(Instant::now);
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x * LINE, -y * LINE),
                    MouseScrollDelta::PixelDelta(p) => (-p.x as f32 / scale, -p.y as f32 / scale),
                };
                let (x, y) = cursor;
                self.tick();
                let publication = trace::WindowInput::start(received, &w);
                #[cfg(target_os = "macos")]
                match ww(&w, |host| host.scroll.as_ref().map(scroll_mac::Bridge::next)).flatten() {
                    Some(Some(delivery)) => delivery.deliver(&mut w, phase),
                    Some(None) => {}
                    None => w.wheel(x, y, dx, dy),
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let _ = phase;
                    w.wheel(x, y, dx, dy);
                }
                if let Some(publication) = publication {
                    publication.log(self.shared.trace_epoch, &w, "wheel", None);
                }
                ww(&w, |x| x.traced(at, "wheel"));
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                #[cfg(target_os = "ios")]
                if matches!(event.physical_key, PhysicalKey::Unidentified(_)) {
                    return;
                }
                let at = self.shared.trace.then(Instant::now);
                self.tick();
                let publication = trace::WindowInput::start(received, &w);
                #[cfg(target_os = "macos")]
                let native_key = ww(&w, |x| x.text_input.as_ref().and_then(|i| i.take_key(&event))).flatten();
                #[cfg(target_os = "macos")]
                if let Some(native_key) = native_key {
                    let marked = ww(&w, |x| x.text_input.as_ref().is_some_and(|i| i.has_marked_text())).unwrap_or(false);
                    if marked {
                        let handled = ww(&w, |x| x.text_input.as_ref().is_some_and(|i| i.interpret_key(native_key))).unwrap_or(false);
                        self.native_text();
                        if !handled {
                            self.key(&mut w, &event, false);
                        }
                    } else if !self.key(&mut w, &event, false) {
                        ww(&w, |x| {
                            if let Some(input) = &x.text_input {
                                input.interpret_key(native_key);
                            }
                        });
                        self.native_text();
                    }
                } else {
                    self.key(&mut w, &event, true);
                }
                #[cfg(not(target_os = "macos"))]
                self.key(&mut w, &event, !cfg!(target_os = "ios"));
                if let Some(publication) = publication {
                    publication.log(self.shared.trace_epoch, &w, "key", None);
                }
                ww(&w, |x| x.traced(at, if event.repeat { "key (repeat)" } else { "key" }));
            }
            #[cfg(not(target_os = "ios"))]
            WindowEvent::Ime(Ime::Enabled) => {
                ww(&w, |x| x.ime_enabled());
            }
            #[cfg(not(target_os = "ios"))]
            WindowEvent::Ime(Ime::Preedit(text, selection)) => {
                let at = self.shared.trace.then(Instant::now);
                self.tick();
                if ww(&w, |x| x.accepts_ime()).unwrap_or(false) {
                    w.event(InputEvent::Preedit { text, selection });
                    ww(&w, |x| x.traced(at, "input method composition"));
                }
            }
            #[cfg(not(target_os = "ios"))]
            WindowEvent::Ime(Ime::Disabled) => {
                self.tick();
                if ww(&w, |x| x.ime_disabled()).unwrap_or(false) {
                    w.event(InputEvent::Preedit { text: String::new(), selection: None });
                }
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
            #[cfg(not(target_os = "ios"))]
            WindowEvent::Ime(Ime::Commit(text)) => {
                let at = self.shared.trace.then(Instant::now);
                self.tick();
                if ww(&w, |x| x.accepts_ime()).unwrap_or(false) {
                    w.type_text(&text);
                    ww(&w, |x| x.traced(at, "input method commit"));
                }
            }
            _ => return,
        }
        self.show();
    }
}

impl Shell {
    fn app_failed(&self) -> bool {
        self.app.as_ref().is_some_and(App::is_failed)
    }

    /// Contain only a runtime-certified failed App. Host/GPU bugs still unwind.
    fn host_entry(&mut self, f: impl FnOnce(&mut Self)) {
        let was_failed = self.app_failed();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))) {
            Ok(()) => {}
            Err(payload) if !was_failed && self.app_failed() => drop(payload),
            Err(payload) => std::panic::resume_unwind(payload),
        }
        if self.app_failed() {
            self.failed = true;
            if let Some(app) = &self.app {
                for id in app.windows() {
                    if let Some(window) = app.window(id) {
                        ww(&window, |host| host.cancel_frames());
                    }
                }
            }
            // Dropping these jobs stops their wake sources and replies with failure.
            self.inspect_pending.take();
            self.capture.take();
        }
    }
}

impl ApplicationHandler<ShellEvent> for Shell {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.host_entry(|shell| shell.on_new_events(event_loop, cause));
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.host_entry(|shell| shell.on_about_to_wait(event_loop));
    }
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ShellEvent) {
        self.host_entry(|shell| shell.on_user_event(event_loop, event));
    }
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.host_entry(|shell| shell.on_resumed(event_loop));
    }
    #[cfg(target_os = "ios")]
    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.host_entry(|shell| shell.ios_active(event_loop, false));
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.host_entry(|shell| shell.on_window_event(event_loop, id, event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::capture::PaintOrigin;
    use octara::inspect::{Completion, LayoutState};
    use octara::platform::HeadlessPlatform;
    use octara::prelude::{El, Role, Styled, col, el, state, text};
    use presentation::{DrawableGeneration, Presentation};

    #[test]
    fn application_definition_keeps_the_window_scope_and_source() {
        let owner = Rc::new(Cell::new(None));
        let observed = owner.clone();
        let line = line!() + 1;
        let definition = AppDefinition::new(Default::default(), WindowOptions::default(), move || {
            observed.set(Some(octara::window::current_window()));
            "Ready"
        });
        assert_eq!(owner.get(), None);

        let window = definition.start(HeadlessPlatform::default(), |_| {}).unwrap();
        assert_eq!(owner.get(), Some(window.id()));
        let kernel = window.kernel();
        let node = kernel.children(kernel.root())[0];
        let source = kernel.origin(node).unwrap();
        assert_eq!(kernel.text(node), "Ready");
        assert_eq!((source.file(), source.line()), (file!(), line));
    }

    fn counter() -> El {
        let count = state("count", || 0);
        col()
            .child(text(move || count.get().to_string()).id("value"))
            .child(el().id("increment").role(Role::Button).label("Increment").size_8().on_press(move || count.update(|value| *value += 1)))
    }

    #[test]
    fn a_waiting_surface_does_not_hold_app_updates_or_another_surface() {
        let app = App::new(HeadlessPlatform::default());
        let mut a = app.open_window(WindowOptions::new("A"), counter).unwrap();
        let mut b = app.open_window(WindowOptions::new("B"), counter).unwrap();
        let mut surface_a = Presentation::new(a.id());
        let mut surface_b = Presentation::new(b.id());
        let initial = DrawableGeneration { surface: 0, device: 0 };
        let old_revision = a.published_revision().unwrap();
        let waiting_a = surface_a.begin(initial).unwrap();

        for _ in 0..3 {
            a.tap("increment").unwrap();
            b.tap("increment").unwrap();
        }
        for window in [&a, &b] {
            assert_eq!(window.kernel().text_of("value").as_deref(), Some("3"));
            assert_eq!(Completion::observe(window).layout, LayoutState::Settled);
        }
        assert!(surface_a.begin(initial).is_none(), "one request is outstanding for A");
        assert!(surface_a.submitted_paint(initial).is_none());

        let ready_b = surface_b.begin(initial).unwrap();
        assert!(surface_b.finish(ready_b, initial));
        let origin = PaintOrigin::Submitted { frame: 1, surface_generation: 0, device_generation: 0 };
        assert!(surface_b.record_submission(b.published_frame().unwrap().paint_frame(origin, &[]), initial));
        assert!(surface_a.waiting());

        assert!(surface_a.finish(waiting_a, initial));
        let latest = a.published_frame().unwrap();
        assert!(latest.revision > old_revision);
        assert!(surface_a.record_submission(latest.paint_frame(origin, &[]), initial));
        assert_eq!(surface_a.submitted_paint(initial).unwrap().sample.revision, latest.revision);

        let old_acquire = surface_a.begin(initial).unwrap();
        a.event(InputEvent::Resize { width: 320.0, height: 240.0, scale: 2.0, safe_area: None });
        surface_a.invalidate();
        let resized = DrawableGeneration { surface: 1, device: 0 };
        assert!(surface_a.begin(resized).is_none(), "the old acquisition still owns the surface");
        assert!(!surface_a.finish(old_acquire, resized));
        assert!(surface_a.submitted_paint(resized).is_none());
        assert_eq!(a.published_frame().unwrap().list.size, (320.0, 240.0));

        let closing_acquire = surface_a.begin(resized).unwrap();
        app.close_window(a.id());
        let reopened = app.open_window(WindowOptions::new("A"), counter).unwrap();
        let mut reopened_surface = Presentation::new(reopened.id());
        let reopened_acquire = reopened_surface.begin(initial).unwrap();
        assert!(a.published_frame().is_none());
        assert!(!reopened_surface.finish(closing_acquire, initial));
        assert!(reopened_surface.finish(reopened_acquire, initial));
        app.shutdown();
    }
}
