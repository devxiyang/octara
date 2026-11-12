//! Desktop windows for Octara apps (winit): window events become [`App`] input by position and
//! key, and frames are the app's display list drawn by the GPU renderer into the window.
//!
//! ```no_run
//! octara_shell::run("Counter", octara_examples_counter);
//! # fn octara_examples_counter() -> octara::view::View { unimplemented!() }
//! ```

use octara::app::{App, Button, Key, KeyEvent, Modifiers, Phase, PointerEvent};
use octara::view::View;
use octara_render::GpuRenderer;
use octara_render::wgpu;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowId};

#[cfg(target_os = "macos")]
mod native_mac;

/// Events sent to the loop from elsewhere: AccessKit's requests, and the app's waker (another
/// thread finished background work or sent on a channel).
#[derive(Debug)]
pub enum ShellEvent {
    Accessibility(accesskit_winit::Event),
    Wake,
    /// `OCTARA_CAPTURE`'s time came: take snapshots, then write the capture (a second one
    /// writes it with what arrived).
    Capture,
}

impl From<accesskit_winit::Event> for ShellEvent {
    fn from(e: accesskit_winit::Event) -> Self {
        ShellEvent::Accessibility(e)
    }
}

/// The platform's clipboard (text).
struct SystemClipboard(arboard::Clipboard);

impl octara::app::Clipboard for SystemClipboard {
    fn read_text(&mut self) -> Option<String> {
        self.0.get_text().ok()
    }
    fn write_text(&mut self, text: &str) {
        let _ = self.0.set_text(text);
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
    /// Exit after drawing this many frames (smoke tests).
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

/// Opens a window showing `root` and runs until it is closed.
pub fn run(title: &str, root: impl Fn() -> View + 'static) {
    run_with(Options { title: title.into(), ..Options::default() }, root)
}

pub fn run_with(options: Options, root: impl Fn() -> View + 'static) {
    let event_loop = EventLoop::<ShellEvent>::with_user_event().build().expect("event loop");
    let proxy = event_loop.create_proxy();
    let mut shell = Shell {
        options,
        root: Some(Box::new(root)),
        app: None,
        gpu: None,
        a11y: None,
        proxy,
        cursor: (0.0, 0.0),
        modifiers: ModifiersState::empty(),
        frames: 0,
        started: Instant::now(),
        trace: std::env::var_os("OCTARA_TRACE_INPUT").is_some_and(|v| v != "0"),
        unshown: None,
        presented: None,
        done: false,
        secure: false,
        resized: false,
        capture: std::env::var("OCTARA_CAPTURE").ok(),
        awaiting: None,
        #[cfg(target_os = "macos")]
        native: None,
    };
    event_loop.run_app(&mut shell).expect("event loop");
}

/// The platform's secure keyboard input (macOS: other processes cannot read the keys typed
/// into a password field). Calls are balanced: on when a secure field gains focus, off when it
/// loses it and when the shell ends.
#[cfg(target_os = "macos")]
mod secure_input {
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn EnableSecureEventInput() -> i32;
        fn DisableSecureEventInput() -> i32;
    }
    pub fn set(on: bool) {
        // SAFETY: no arguments; the system keeps a count, which the shell keeps balanced.
        unsafe {
            if on {
                EnableSecureEventInput();
            } else {
                DisableSecureEventInput();
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod secure_input {
    pub fn set(_: bool) {}
}

impl Drop for Shell {
    fn drop(&mut self) {
        if self.secure {
            secure_input::set(false);
        }
    }
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: GpuRenderer,
}

struct Shell {
    options: Options,
    root: Option<Box<dyn Fn() -> View>>,
    app: Option<App>,
    gpu: Option<Gpu>,
    a11y: Option<accesskit_winit::Adapter>,
    proxy: EventLoopProxy<ShellEvent>,
    /// Pointer position in logical pixels.
    cursor: (f32, f32),
    modifiers: ModifiersState,
    frames: u32,
    /// The app's clock starts at launch.
    started: Instant,
    /// `OCTARA_TRACE_INPUT=1`: each input's latency to the frame that shows it, on stderr.
    trace: bool,
    /// The oldest input not yet shown: when it arrived, what it was, and when handling ended.
    unshown: Option<(Instant, &'static str, Instant)>,
    /// When the last frame was presented.
    presented: Option<Instant>,
    /// `exit_after_frames` was reached.
    done: bool,
    /// A secure field has focus: the input method is off and (macOS) secure input on.
    secure: bool,
    /// The window resized since the last frame (native views present with the frame).
    resized: bool,
    /// `OCTARA_CAPTURE=out.png`: after `OCTARA_CAPTURE_AFTER_MS` (default 1500), the frame
    /// with every native view's snapshot in its place is written there, and the app exits;
    /// `OCTARA_CAPTURE_SCROLL=px` first scrolls that far down at the window's center.
    capture: Option<String>,
    /// Snapshots asked for and not yet back.
    awaiting: Option<Vec<octara::native::NativeId>>,
    /// The window's native views (web views), placed with each frame.
    #[cfg(target_os = "macos")]
    native: Option<native_mac::Native>,
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
    /// Records an input for latency tracing (the first one not yet shown).
    fn traced(&mut self, at: Instant, what: &'static str) {
        if self.trace && self.unshown.is_none() {
            self.unshown = Some((at, what, Instant::now()));
        }
    }

    /// Brings the app's clock to now (before each event and frame).
    fn tick(&mut self) {
        let now = self.started.elapsed();
        if let Some(app) = &mut self.app {
            app.advance_to(now);
        }
    }

    /// One pointer event at the current position (mouse) or the touch's (finger).
    fn pointer(&mut self, ev: PointerEvent) {
        let at = Instant::now();
        self.tick();
        if let Some(app) = &mut self.app {
            app.pointer(ev);
        }
        self.place_ime();
        // Moves come in bursts, and a cancel (the window losing focus) shows nothing new.
        match ev.phase {
            Phase::Down => self.traced(at, "pointer down"),
            Phase::Up => self.traced(at, "pointer up"),
            Phase::Move | Phase::Cancel => {}
        }
        self.redraw();
    }

    fn scale(&self) -> f32 {
        self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor() as f32)
    }

    /// Something may have changed: draw again, and tell assistive technology if it listens.
    /// Something changed: show it. After a quiet spell the frame is drawn now, rather than at
    /// the display's next refresh callback (which an idle ProMotion display slows to ~24 Hz, so
    /// the first keystroke after a pause waited ~45 ms); within a burst, changes coalesce into
    /// the next refresh.
    fn redraw(&mut self) {
        let recent = self.presented.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(8));
        if recent {
            if let Some(g) = &self.gpu {
                g.window.request_redraw();
            }
        } else {
            self.draw();
        }
        let scale = self.scale() as f64;
        if let (Some(a11y), Some(app)) = (&mut self.a11y, &self.app) {
            a11y.update_if_active(|| app.accessibility_update(scale));
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        let scale = self.scale();
        if let Some(g) = &mut self.gpu {
            g.config.width = width.max(1);
            g.config.height = height.max(1);
            g.surface.configure(g.renderer.device(), &g.config);
        }
        if let Some(app) = &mut self.app {
            app.resize(width as f32 / scale, height as f32 / scale);
        }
        self.resized = true;
        self.redraw();
    }

    fn draw(&mut self) {
        let scale = self.scale();
        let (Some(g), Some(app)) = (&mut self.gpu, &self.app) else { return };
        // Settling the frame hands native views their placements; they are applied with this
        // frame's present, in one transaction.
        let list = app.display_list();
        #[cfg(target_os = "macos")]
        let transaction = self.native.as_ref().is_some_and(|n| n.begin_frame(&g.surface, self.resized));
        self.resized = false;
        let frame = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                g.surface.configure(g.renderer.device(), &g.config);
                g.window.request_redraw();
                None
            }
            _ => None,
        };
        let drawing = Instant::now();
        if let Some(frame) = &frame {
            let view = frame.texture.create_view(&Default::default());
            g.renderer.draw(&list, scale, &view, (g.config.width, g.config.height));
        }
        #[cfg(target_os = "macos")]
        if let Some(n) = &self.native {
            n.place();
        }
        let shown = frame.is_some();
        if let Some(frame) = frame {
            g.window.pre_present_notify();
            g.renderer.queue().present(frame);
        }
        #[cfg(target_os = "macos")]
        if transaction {
            native_mac::Native::end_frame();
        }
        if !shown {
            return;
        }
        self.presented = Some(Instant::now());
        if let Some((at, what, handled)) = self.unshown.take() {
            let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
            eprintln!(
                "octara-shell: {what}: shown after {:.1} ms (handling {:.1}, waiting for the frame {:.1}, drawing {:.1})",
                ms(at.elapsed()),
                ms(handled - at),
                ms(drawing - handled),
                ms(drawing.elapsed()),
            );
        }
        self.frames += 1;
        if let Some(n) = self.options.exit_after_frames {
            if self.frames >= n {
                eprintln!("octara-shell: drew {} frames at {}×{} (scale {scale}), exiting", self.frames, g.config.width, g.config.height);
                self.done = true;
            } else {
                g.window.request_redraw();
            }
        }
    }

    /// Writes the capture once every snapshot asked for is back.
    fn capture_if_ready(&mut self) {
        if self.awaiting.as_ref().is_some_and(|a| a.is_empty()) {
            self.write_capture();
        }
    }

    /// The frame drawn by the reference renderer at the window's scale, native views as their
    /// snapshots; then the app exits.
    fn write_capture(&mut self) {
        let (Some(path), Some(app)) = (self.capture.take(), &self.app) else { return };
        let scale = self.scale();
        match octara_render::screenshot(app, scale).save_png(&path) {
            Ok(()) => eprintln!("octara-shell: captured {path}"),
            Err(e) => eprintln!("octara-shell: capture {path}: {e}"),
        }
        #[cfg(target_os = "macos")]
        if let Some(n) = &self.native {
            for line in n.frames() {
                eprintln!("octara-shell: {line}");
            }
        }
        self.awaiting = None;
        self.done = true;
    }

    /// Tells the input method where the caret is, so its candidate window opens next to it; a
    /// secure field turns the input method off and the platform's secure input on.
    fn place_ime(&mut self) {
        let (Some(g), Some(app)) = (&self.gpu, &self.app) else { return };
        if let Some((x, y, w, h)) = app.caret() {
            g.window.set_ime_cursor_area(LogicalPosition::new(x, y), LogicalSize::new(w.max(1.0), h));
        }
        let secure = app.text_field().is_some_and(|f| f.secure);
        if secure != self.secure {
            self.secure = secure;
            g.window.set_ime_allowed(!secure);
            secure_input::set(secure);
        }
    }

    /// A key press: the app's key routing first (focus, keymap, commands); if nothing took it,
    /// the text it typed goes to the focused field.
    fn key(&mut self, event: &winit::event::KeyEvent) {
        let Some(app) = &mut self.app else { return };
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
            app.key_event(KeyEvent { key, code, modifiers: modifiers(self.modifiers), repeat: event.repeat })
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
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // A deadline the app asked for (a long press coming due, momentum's next frame).
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.tick();
            self.redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.done {
            event_loop.exit();
            return;
        }
        let deadline = self.app.as_ref().and_then(|a| a.next_deadline());
        event_loop.set_control_flow(match deadline {
            Some(d) => ControlFlow::WaitUntil(self.started + d),
            None => ControlFlow::Wait,
        });
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: ShellEvent) {
        let e = match event {
            ShellEvent::Wake => {
                self.tick();
                if let Some(app) = &mut self.app {
                    app.poll();
                    // Events native views sent (a page loaded, its title changed).
                    #[cfg(target_os = "macos")]
                    if let Some(n) = &self.native {
                        for (id, event) in n.inbox.take() {
                            app.native_event(id, event);
                        }
                        for (id, image) in n.inbox.take_snapshots() {
                            app.native_snapshot(id, image);
                            if let Some(a) = &mut self.awaiting {
                                a.retain(|x| *x != id);
                            }
                        }
                    }
                }
                self.capture_if_ready();
                self.redraw();
                return;
            }
            ShellEvent::Capture => {
                if self.awaiting.is_some() {
                    self.write_capture();
                } else if self.app.is_some() {
                    if let Some(dy) = std::env::var("OCTARA_CAPTURE_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
                        let (w, h) = self.options.size;
                        if let Some(app) = &mut self.app {
                            app.wheel(w / 2.0, h / 2.0, 0.0, dy);
                        }
                        // Drawn, so the views are placed where the page now is.
                        self.draw();
                    }
                    let Some(app) = &mut self.app else { return };
                    let ids = app.snapshot_native();
                    self.awaiting = Some(ids);
                    self.capture_if_ready();
                }
                return;
            }
            ShellEvent::Accessibility(e) => e,
        };
        let scale = self.scale() as f64;
        match e.window_event {
            accesskit_winit::WindowEvent::InitialTreeRequested => {
                if let (Some(a11y), Some(app)) = (&mut self.a11y, &self.app) {
                    app.restart_accessibility();
                    a11y.update_if_active(|| app.accessibility_update(scale));
                }
            }
            accesskit_winit::WindowEvent::ActionRequested(req) => {
                if let Some(app) = &mut self.app {
                    app.accessibility_action(req.target_node, req.action, req.data);
                }
                self.redraw();
            }
            accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let (w, h) = self.options.size;
        // Hidden until the accessibility adapter is attached (AccessKit requires it).
        let attrs =
            Window::default_attributes().with_title(&self.options.title).with_inner_size(LogicalSize::new(w, h)).with_visible(false);
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        self.a11y = Some(accesskit_winit::Adapter::with_event_loop_proxy(event_loop, &window, self.proxy.clone()));
        window.set_visible(true);
        window.set_ime_allowed(true);
        // An app started from a terminal comes to the front.
        window.focus_window();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() }),
        )
        .expect("a GPU adapter for the window");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
        let caps = surface.get_capabilities(&adapter);
        // Colors are blended in sRGB-encoded space: present a non-sRGB format.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        let renderer = GpuRenderer::with_device(device, queue, format);
        let scale = window.scale_factor() as f32;
        let root = self.root.take().expect("root view");
        let mut app = App::new(root);
        let proxy = self.proxy.clone();
        app.set_waker(move || {
            let _ = proxy.send_event(ShellEvent::Wake);
        });
        #[cfg(target_os = "macos")]
        {
            let proxy = self.proxy.clone();
            if let Some((native, host)) = native_mac::Native::new(&window, move || {
                let _ = proxy.send_event(ShellEvent::Wake);
            }) {
                app.set_native_host(host);
                self.native = Some(native);
            }
        }
        if let Ok(c) = arboard::Clipboard::new() {
            app.set_clipboard(Box::new(SystemClipboard(c)));
        }
        if self.options.system_fonts {
            app.use_system_fonts();
        }
        app.resize(size.width as f32 / scale, size.height as f32 / scale);
        if self.capture.is_some() {
            let after = std::env::var("OCTARA_CAPTURE_AFTER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500u64);
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(after));
                let _ = proxy.send_event(ShellEvent::Capture);
                // Snapshots that never come do not hold the capture forever.
                std::thread::sleep(std::time::Duration::from_secs(5));
                let _ = proxy.send_event(ShellEvent::Capture);
            });
        }
        self.app = Some(app);
        self.gpu = Some(Gpu { window, surface, config, renderer });
        self.redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(a11y), Some(g)) = (&mut self.a11y, &self.gpu) {
            a11y.process_event(&g.window, &event);
        }
        let scale = self.scale();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::RedrawRequested => self.draw(),
            // Hidden windows get no drawables; draw again once visible.
            WindowEvent::Occluded(false) => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32 / scale, position.y as f32 / scale);
                let (x, y) = self.cursor;
                self.pointer(PointerEvent::mouse(Phase::Move, x, y).modifiers(modifiers(self.modifiers)));
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(app) = &mut self.app {
                    app.pointer_leave();
                }
                self.redraw();
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
                let (x, y) = self.cursor;
                self.pointer(PointerEvent::mouse(phase, x, y).button(button).modifiers(modifiers(self.modifiers)));
            }
            WindowEvent::Touch(Touch { phase, location, id, .. }) => {
                let phase = match phase {
                    TouchPhase::Started => Phase::Down,
                    TouchPhase::Moved => Phase::Move,
                    TouchPhase::Ended => Phase::Up,
                    TouchPhase::Cancelled => Phase::Cancel,
                };
                let (x, y) = (location.x as f32 / scale, location.y as f32 / scale);
                self.pointer(PointerEvent::touch(id, phase, x, y));
            }
            WindowEvent::Focused(false) => {
                // The window lost the pointer: a held press or drag is cancelled.
                let (x, y) = self.cursor;
                self.pointer(PointerEvent::mouse(Phase::Cancel, x, y));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x * LINE, -y * LINE),
                    MouseScrollDelta::PixelDelta(p) => (-p.x as f32 / scale, -p.y as f32 / scale),
                };
                let (x, y) = self.cursor;
                self.tick();
                if let Some(app) = &mut self.app {
                    app.wheel(x, y, dx, dy);
                }
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let at = Instant::now();
                self.tick();
                self.key(&event);
                self.traced(at, if event.repeat { "key (repeat)" } else { "key" });
                self.redraw();
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                self.traced(Instant::now(), "input method composition");
                if let Some(app) = &mut self.app {
                    app.set_composition(&text);
                }
                self.place_ime();
                self.redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                self.traced(Instant::now(), "input method commit");
                if let Some(app) = &mut self.app {
                    app.type_text(&text);
                }
                self.place_ime();
                self.redraw();
            }
            _ => {}
        }
    }
}
