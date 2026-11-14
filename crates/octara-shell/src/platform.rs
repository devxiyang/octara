//! The winit backend's side of the platform layer ([`octara::platform`]): the system's clipboard
//! and the event loop's waker, and its windows, each with what shows it: its GPU surface (all
//! windows share one renderer), its accessibility adapter, its native views, its text input and
//! its frame clock.

use super::{Shared, ShellEvent};
use octara::paint::DisplayList;
use octara::platform::{CursorStyle, Frame, Platform, PlatformWindow, TextInput};
use octara::serde_json::{Value, json};
use octara_render::{GpuRenderer, wgpu};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
#[cfg(not(target_os = "macos"))]
use winit::dpi::LogicalPosition;
use winit::dpi::LogicalSize;
use winit::window::Window;

/// The desktop the app runs on: arboard's clipboard, the event loop's proxy as the waker, and
/// windows made in the loop's callback the app runs in.
pub(crate) struct WinitPlatform {
    clipboard: Option<arboard::Clipboard>,
    shared: Rc<Shared>,
    /// The menu bar (macOS), made with the first menus.
    #[cfg(target_os = "macos")]
    menubar: Option<super::menubar_mac::MenuBar>,
}

impl WinitPlatform {
    pub(crate) fn new(shared: Rc<Shared>) -> Self {
        WinitPlatform {
            clipboard: arboard::Clipboard::new().ok(),
            shared,
            #[cfg(target_os = "macos")]
            menubar: None,
        }
    }
}

/// The GPU all windows draw with: the adapter, one renderer, the surfaces' format.
pub(crate) struct Gpu {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    pub(crate) renderer: GpuRenderer,
    format: wgpu::TextureFormat,
}

impl Platform for WinitPlatform {
    fn read_text(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.get_text().ok()
    }
    fn write_text(&mut self, text: &str) {
        if let Some(c) = &mut self.clipboard {
            let _ = c.set_text(text);
        }
    }
    fn read_image(&mut self) -> Option<octara::image::Image> {
        let i = self.clipboard.as_mut()?.get_image().ok()?;
        Some(octara::image::Image::rgba(i.width as u32, i.height as u32, i.bytes.into_owned()))
    }
    fn write_image(&mut self, image: &octara::image::Image) {
        let f = image.frame();
        let data = arboard::ImageData { width: f.width as usize, height: f.height as usize, bytes: std::borrow::Cow::Borrowed(&f.rgba) };
        if let Some(c) = &mut self.clipboard {
            let _ = c.set_image(data);
        }
    }
    fn set_menus(&mut self, menus: &[octara::menu::Menu]) {
        #[cfg(target_os = "macos")]
        {
            if self.menubar.is_none() {
                let proxy = self.shared.proxy.clone();
                self.menubar = super::menubar_mac::MenuBar::new(move |name, chord| {
                    let _ = proxy.send_event(ShellEvent::Command { name, chord });
                });
            }
            if let Some(bar) = &mut self.menubar {
                bar.set(menus);
            }
        }
        let _ = menus;
    }
    fn open_url(&mut self, url: &str) {
        if let Err(e) = open::that_detached(url) {
            eprintln!("octara-shell: cannot open {url}: {e}");
        }
    }
    fn open_window(
        &mut self,
        id: octara::app::WindowId,
        options: &octara::platform::WindowOptions,
    ) -> Result<Box<dyn PlatformWindow>, String> {
        let shared = &self.shared;
        let event_loop = shared.event_loop().ok_or("windows open while the event loop runs")?;
        let (w, h) = options.size;
        // Hidden until the accessibility adapter is attached (AccessKit requires it).
        let attrs = Window::default_attributes().with_title(&options.title).with_inner_size(LogicalSize::new(w, h)).with_visible(false);
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let a11y = accesskit_winit::Adapter::with_event_loop_proxy(event_loop, &window, shared.proxy.clone());
        window.set_visible(true);
        // An app started from a terminal comes to the front.
        window.focus_window();
        let mut gpu = shared.gpu.borrow_mut();
        let instance = match &*gpu {
            Some(g) => g.instance.clone(),
            None => wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env()),
        };
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        if gpu.is_none() {
            *gpu = Some(new_gpu(instance, &surface)?);
        }
        let g = gpu.as_ref().expect("the GPU");
        let caps = surface.get_capabilities(&g.adapter);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: g.format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(g.renderer.device(), &config);
        drop(gpu);
        let wid = window.id();
        #[cfg(target_os = "macos")]
        let text_input = {
            let proxy = shared.proxy.clone();
            super::text_input_mac::TextInputMac::new(&window, shared.trace, move || {
                let _ = proxy.send_event(ShellEvent::Wake);
            })
        };
        #[cfg(target_os = "macos")]
        let (native, host, link) = {
            let proxy = shared.proxy.clone();
            let (native, host) = super::native_mac::Native::new(&window, move || {
                let _ = proxy.send_event(ShellEvent::Wake);
            })
            .map_or((None, None), |(n, h)| (Some(n), Some(h)));
            let proxy = shared.proxy.clone();
            let link = super::pace_mac::DisplayLink::new(&window, move || {
                let _ = proxy.send_event(ShellEvent::Frame(wid));
            });
            (native, host, link)
        };
        shared.windows.borrow_mut().insert(wid, id);
        Ok(Box::new(WinitWindow {
            window,
            shared: shared.clone(),
            surface,
            config,
            a11y,
            #[cfg(target_os = "macos")]
            text_input,
            #[cfg(target_os = "macos")]
            native,
            #[cfg(target_os = "macos")]
            host,
            #[cfg(target_os = "macos")]
            link,
            #[cfg(target_os = "macos")]
            shown: Rc::default(),
            secure: false,
            ime_allowed: false,
            input_owner: None,
            ime_owner: None,
            cursor_style: CursorStyle::Arrow,
            cursor: (0.0, 0.0),
            hovering: Vec::new(),
            dropped: Vec::new(),
            pending: true,
            presented: None,
            resized: false,
            sized: false,
            unshown: None,
            occluded: None,
            frames: 0,
        }))
    }
    fn waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let proxy = self.shared.proxy.clone();
        Some(Arc::new(move || {
            let _ = proxy.send_event(ShellEvent::Wake);
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The adapter for the first window's surface, and the renderer.
fn new_gpu(instance: wgpu::Instance, surface: &wgpu::Surface<'static>) -> Result<Gpu, String> {
    let adapter = pollster::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(surface), ..Default::default() }),
    )
    .map_err(|e| format!("no GPU adapter for the window: {e}"))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
    let caps = surface.get_capabilities(&adapter);
    // Colors are blended in sRGB-encoded space: present a non-sRGB format.
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
        .unwrap_or(caps.formats[0]);
    let renderer = GpuRenderer::with_device(device, queue, format);
    Ok(Gpu { instance, adapter, renderer, format })
}

/// One winit window and what shows it.
pub(crate) struct WinitWindow {
    pub(crate) window: Arc<Window>,
    shared: Rc<Shared>,
    surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
    pub(crate) a11y: accesskit_winit::Adapter,
    #[cfg(target_os = "macos")]
    pub(crate) text_input: Option<super::text_input_mac::TextInputMac>,
    /// Its native views (macOS: AppKit's; elsewhere none, so they draw as placeholders).
    #[cfg(target_os = "macos")]
    pub(crate) native: Option<super::native_mac::Native>,
    #[cfg(target_os = "macos")]
    host: Option<super::native_mac::Host>,
    /// Its frame clock (macOS); elsewhere winit's redraw requests, paced by the compositor
    /// (Wayland) or the swap chain's blocking present.
    #[cfg(target_os = "macos")]
    link: Option<super::pace_mac::DisplayLink>,
    /// The requests it shows natively now (macOS: menus and alerts).
    #[cfg(target_os = "macos")]
    shown: Rc<super::ask_mac::Shown>,
    /// A secure field has focus: the input method is off and (macOS) secure input on.
    secure: bool,
    /// Whether the focused field currently accepts input-method composition.
    ime_allowed: bool,
    /// Current logical field, and the field that enabled the native composition session.
    input_owner: Option<octara::kernel::NodeId>,
    ime_owner: Option<octara::kernel::NodeId>,
    /// How the pointer looks.
    cursor_style: CursorStyle,
    /// The pointer, in logical pixels.
    pub(crate) cursor: (f32, f32),
    /// Files dragged from the system over it, and those dropped (sent once the drop's events
    /// are all in).
    pub(crate) hovering: Vec<std::path::PathBuf>,
    pub(crate) dropped: Vec<std::path::PathBuf>,
    /// The app has something new to show.
    pub(crate) pending: bool,
    /// When the last frame was presented.
    pub(crate) presented: Option<Instant>,
    /// It resized since the last frame (native views present with the frame).
    pub(crate) resized: bool,
    /// The app was told its size.
    pub(crate) sized: bool,
    /// `OCTARA_TRACE_INPUT`: only the oldest input and bounded aggregate counts.
    unshown: Option<super::trace::Input>,
    pub(crate) occluded: Option<bool>,
    pub(crate) frames: u32,
}

impl WinitWindow {
    pub(crate) fn scale(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    /// One refresh of the display it is on (60 Hz when that is unknown).
    fn refresh(&self) -> Duration {
        let mhz = self.window.current_monitor().and_then(|m| m.refresh_rate_millihertz()).filter(|r| *r > 0).unwrap_or(60_000);
        Duration::from_secs_f64(1000.0 / f64::from(mhz))
    }

    /// Where the pointer is: asked of AppKit on macOS (it sends no moves while files are dragged
    /// over the window), else the last move's place.
    pub(crate) fn pointer(&mut self) -> (f32, f32) {
        #[cfg(target_os = "macos")]
        if let Some(p) = super::native_mac::pointer_in(&self.window) {
            self.cursor = p;
        }
        self.cursor
    }

    /// Nothing was shown for a refresh: a new frame is drawn at once, not at the next refresh
    /// (which an idle ProMotion display slows to ~24 Hz).
    pub(crate) fn rested(&self) -> bool {
        self.presented.is_none_or(|t| t.elapsed() >= self.refresh())
    }

    /// Asks for the next frame on the clock.
    pub(crate) fn want_frame(&self) {
        #[cfg(target_os = "macos")]
        if let Some(link) = &self.link {
            link.start();
            return;
        }
        self.window.request_redraw();
    }

    /// A tick of the clock: whether to draw now (else the clock pauses while nothing waits).
    pub(crate) fn on_tick(&self) -> bool {
        #[cfg(target_os = "macos")]
        if let Some(link) = &self.link {
            if !self.pending {
                link.stop();
                return false;
            }
            return super::pace_mac::tick_draws(self.presented);
        }
        self.pending
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32, renderer: &GpuRenderer) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(renderer.device(), &self.config);
        self.resized = true;
        self.pending = true;
    }

    /// Draws `list` and presents it (native views placed with it, in one transaction): whether a
    /// frame was shown.
    pub(crate) fn draw(&mut self, list: &DisplayList, renderer: &mut GpuRenderer, mut trace: Option<super::trace::Frame>) -> bool {
        let scale = self.scale();
        // An underlay view shows through the frame's cutouts: the layer becomes non-opaque
        // (premultiplied, as the renderer draws).
        #[cfg(target_os = "macos")]
        if self.native.as_ref().is_some_and(|n| n.transparent()) && self.config.alpha_mode != wgpu::CompositeAlphaMode::PostMultiplied {
            self.config.alpha_mode = wgpu::CompositeAlphaMode::PostMultiplied;
            self.surface.configure(renderer.device(), &self.config);
        }
        #[cfg(target_os = "macos")]
        let transaction = self.native.as_ref().is_some_and(|n| n.begin_frame(&self.surface, self.resized));
        self.resized = false;
        if let Some(t) = &mut trace {
            t.surface_start = Instant::now();
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(renderer.device(), &self.config);
                self.want_frame();
                None
            }
            _ => None,
        };
        if let Some(t) = &mut trace {
            t.surface_ready = Instant::now();
        }
        if let Some(frame) = &frame {
            let view = frame.texture.create_view(&Default::default());
            renderer.draw(list, scale, &view, (self.config.width, self.config.height));
        }
        if let Some(t) = &mut trace {
            t.encoded = Instant::now();
        }
        #[cfg(target_os = "macos")]
        if let Some(n) = &self.native {
            n.place();
        }
        if let Some(t) = &mut trace {
            t.native_placed = Instant::now();
        }
        let shown = frame.is_some();
        if let Some(frame) = frame {
            self.window.pre_present_notify();
            if let Some(t) = &mut trace {
                t.present_start = Some(Instant::now());
            }
            renderer.queue().present(frame);
            if let Some(t) = &mut trace {
                t.present_returned = Some(Instant::now());
            }
        }
        #[cfg(target_os = "macos")]
        if transaction {
            super::native_mac::Native::end_frame();
        }
        if !shown && trace.is_none() {
            return false;
        }
        let finished = Instant::now();
        if shown {
            self.pending = false;
            self.presented = Some(finished);
            self.frames += 1;
        }
        if let Some(t) = trace {
            eprintln!(
                "{}",
                json!({
                    "schema": "octara.frame_trace/1",
                    "window": format!("{:?}", self.window.id()),
                    "frame": self.frames,
                    "app_clock_ms": t.app_clock_ms,
                    "visible": self.window.is_visible(),
                    "occluded": self.occluded,
                    "surface_acquired": shown,
                    "size_px": [self.config.width, self.config.height],
                    "scale": scale,
                    "cpu_us": t.cpu_json(finished),
                    "input": self.unshown.as_ref().map(|i| i.json(t.started, t.present_returned)),
                    "gpu_execution_us": null,
                    "physical_display_us": null,
                })
            );
        }
        if shown {
            self.unshown = None;
        }
        shown
    }

    /// Shows a request the platform takes (file dialogs: rfd's, made here on the main thread so
    /// the panel shows now, awaited on a thread of their own); its answer comes back as input.
    fn show_ask(&self, id: octara::ask::AskId, ask: octara::ask::Ask) {
        use octara::ask::{Answer, Ask};
        use std::future::Future;
        use std::pin::Pin;
        let (proxy, window) = (self.shared.proxy.clone(), self.window.id());
        #[cfg(target_os = "macos")]
        if super::ask_mac::shows(&ask) {
            let send: super::ask_mac::Send = Rc::new(move |id, answer| {
                let _ = proxy.send_event(ShellEvent::Answer { window, id, answer });
            });
            super::ask_mac::show(&self.window, &self.shown, id, ask, send);
            return;
        }
        let answer: Pin<Box<dyn Future<Output = Answer> + Send>> = match ask {
            Ask::Paths(o) => {
                // A sheet on the window where the platform has them.
                let mut d = rfd::AsyncFileDialog::new().set_parent(&*self.window);
                if let Some(p) = &o.prompt {
                    d = d.set_title(p);
                }
                let to_paths =
                    |hs: Option<Vec<rfd::FileHandle>>| Answer::Paths(hs.map(|hs| hs.iter().map(|h| h.path().to_owned()).collect()));
                match (o.directories && !o.files, o.multiple) {
                    (true, true) => Box::pin(async move { to_paths(d.pick_folders().await) }),
                    (true, false) => Box::pin(async move { to_paths(d.pick_folder().await.map(|h| vec![h])) }),
                    (false, true) => Box::pin(async move { to_paths(d.pick_files().await) }),
                    (false, false) => Box::pin(async move { to_paths(d.pick_file().await.map(|h| vec![h])) }),
                }
            }
            Ask::NewPath { directory, name } => {
                let mut d = rfd::AsyncFileDialog::new().set_parent(&*self.window).set_directory(directory);
                if let Some(n) = name {
                    d = d.set_file_name(n);
                }
                let path = d.save_file();
                Box::pin(async move { Answer::NewPath(path.await.map(|h| h.path().to_owned())) })
            }
            _ => return,
        };
        std::thread::spawn(move || {
            let answer = pollster::block_on(answer);
            let _ = proxy.send_event(ShellEvent::Answer { window, id, answer });
        });
    }

    /// Records an input for latency tracing (the first one not yet shown).
    pub(crate) fn traced(&mut self, at: Option<Instant>, what: &'static str) {
        if let Some(at) = at {
            self.record_input(super::trace::Input::handled(at, what));
        }
    }

    pub(crate) fn record_input(&mut self, input: super::trace::Input) {
        if let Some(pending) = &mut self.unshown {
            pending.merge(input);
        } else {
            self.unshown = Some(input);
        }
    }

    pub(crate) fn ime_enabled(&mut self) {
        self.ime_owner = self.input_owner.filter(|_| self.ime_allowed);
    }

    pub(crate) fn accepts_ime(&self) -> bool {
        self.ime_allowed && self.ime_owner.is_some() && self.ime_owner == self.input_owner
    }

    pub(crate) fn ime_disabled(&mut self) -> bool {
        // A context retired on A can report Disabled after B already has focus. Its empty
        // preedit belongs to A; core focus cleanup has already cleared that field.
        let current = self.ime_owner.is_some() && self.ime_owner == self.input_owner;
        self.ime_owner = None;
        current
    }

    /// Current UI-thread input and presentation facts. A submitted frame does not establish
    /// that the window server displayed it; visibility is reported separately.
    pub(crate) fn inspect_state(&self) -> Value {
        let state = json!({
            "ime": {
                "allowed": self.ime_allowed,
                "secure": self.secure,
                "native_focus": self.window.has_focus(),
            },
            "window": {
                "frames": self.frames,
                "pending": self.pending,
                "presented_age_ms": self.presented.map(|at| at.elapsed().as_secs_f64() * 1000.0),
            },
        });
        #[cfg(target_os = "macos")]
        {
            let mut state = state;
            if let Some(native) = super::native_mac::inspect_state(&self.window) {
                for (key, fields) in native {
                    state[&key]
                        .as_object_mut()
                        .expect("host state is an object")
                        .extend(fields.as_object().expect("native state is an object").clone());
                }
            }
            if let Some(input) = &self.text_input {
                state["ime"] = input.inspect_state();
            }
            state
        }
        #[cfg(not(target_os = "macos"))]
        {
            state
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn set_ime_allowed(&mut self, allowed: bool) {
        if allowed == self.ime_allowed {
            return;
        }
        self.window.set_ime_allowed(allowed);
        self.ime_allowed = allowed;
    }

    /// The input method's candidate window follows the caret. Only a focused, non-secure field
    /// enables composition; secure fields also enable the platform's secure keyboard input.
    fn text_input(&mut self, input: Option<&TextInput>) {
        #[cfg(target_os = "macos")]
        {
            // The owned native client interprets text; winit only translates physical keys.
            self.ime_allowed = input.is_some_and(|i| !i.secure);
            self.input_owner = input.map(|i| i.id);
            let secure = input.is_some_and(|i| i.secure);
            if secure != self.secure {
                self.secure = secure;
                secure_input::set(secure);
            }
            if let Some(client) = &self.text_input {
                client.update(input);
            }
        }
        #[cfg(not(target_os = "macos"))]
        self.winit_text_input(input);
    }

    #[cfg(not(target_os = "macos"))]
    fn winit_text_input(&mut self, input: Option<&TextInput>) {
        let owner = input.map(|t| t.id);
        if owner != self.input_owner {
            self.input_owner = owner;
            // The old session keeps its owner until queued Disabled has been handled.
            self.set_ime_allowed(false);
        }
        if let Some(t) = input {
            let c = t.caret;
            self.window.set_ime_cursor_area(LogicalPosition::new(c.x, c.y), LogicalSize::new(c.w.max(1.0), c.h));
        }
        let secure = input.is_some_and(|t| t.secure);
        if secure != self.secure {
            self.secure = secure;
            secure_input::set(secure);
        }
        let allowed = input.is_some_and(|t| !t.secure);
        self.set_ime_allowed(allowed);
    }
}

impl PlatformWindow for WinitWindow {
    fn present(&mut self, frame: Frame) {
        // Placements are applied with the next frame: one is needed.
        let placed = !frame.native.is_empty();
        #[cfg(target_os = "macos")]
        if let Some(host) = &self.host {
            for change in frame.native {
                host.apply(change);
            }
        }
        self.text_input(frame.text_input.as_ref());
        if frame.cursor != self.cursor_style {
            self.cursor_style = frame.cursor;
            self.window.set_cursor(cursor_icon(frame.cursor));
        }
        if let Some(p) = &frame.picked {
            eprintln!("octara-shell: picked {} (on the clipboard)", p.text);
            println!("{}", octara::serde_json::to_string(p).expect("a pick serializes"));
        }
        if frame.redraw || placed {
            self.pending = true;
        }
        for id in frame.withdrawn {
            #[cfg(target_os = "macos")]
            super::ask_mac::withdraw(&self.shown, id);
            let _ = id;
        }
        for (id, ask) in frame.asks {
            self.show_ask(id, ask);
        }
    }
    fn shows_ask(&self, ask: &octara::ask::Ask) -> bool {
        #[cfg(target_os = "macos")]
        if self.host.is_some() && super::ask_mac::shows(ask) {
            return true;
        }
        matches!(ask, octara::ask::Ask::Paths(_) | octara::ask::Ask::NewPath { .. })
    }
    fn shows_native(&self, kind: &str, slot: octara::native::Slot) -> bool {
        #[cfg(target_os = "macos")]
        if self.host.is_some() {
            return super::native_mac::Host::shows(kind, slot);
        }
        let _ = (kind, slot);
        false
    }
    fn snapshot_native(&mut self, id: octara::native::NativeId) -> Result<Vec<String>, String> {
        #[cfg(target_os = "macos")]
        if let Some(host) = &self.host {
            return host.snapshot(id);
        }
        let _ = id;
        Err("this platform takes no snapshots of native views yet".into())
    }
    fn native_busy(&self, id: octara::native::NativeId) -> Option<String> {
        #[cfg(target_os = "macos")]
        if let Some(host) = &self.host {
            return host.busy(id);
        }
        let _ = id;
        None
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn cursor_icon(c: CursorStyle) -> winit::window::CursorIcon {
    use winit::window::CursorIcon as I;
    match c {
        CursorStyle::Arrow => I::Default,
        CursorStyle::IBeam => I::Text,
        CursorStyle::PointingHand => I::Pointer,
        CursorStyle::Crosshair => I::Crosshair,
        CursorStyle::OpenHand => I::Grab,
        CursorStyle::ClosedHand => I::Grabbing,
        CursorStyle::ResizeLeftRight => I::EwResize,
        CursorStyle::ResizeUpDown => I::NsResize,
        CursorStyle::ResizeColumn => I::ColResize,
        CursorStyle::ResizeRow => I::RowResize,
        CursorStyle::OperationNotAllowed => I::NotAllowed,
    }
}

impl Drop for WinitWindow {
    fn drop(&mut self) {
        if self.secure {
            secure_input::set(false);
        }
        self.shared.windows.borrow_mut().remove(&self.window.id());
    }
}

/// The platform's secure keyboard input (macOS: other processes cannot read the keys typed
/// into a password field). Calls are balanced: on when a secure field gains focus, off when it
/// loses it and when the window goes.
#[cfg(target_os = "macos")]
mod secure_input {
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn EnableSecureEventInput() -> i32;
        fn DisableSecureEventInput() -> i32;
    }
    pub fn set(on: bool) {
        // SAFETY: no arguments; the system keeps a count, which the window keeps balanced.
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
