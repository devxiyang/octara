//! The winit backend's side of the platform layer ([`octara::platform`]): the system's clipboard
//! and the event loop's waker, and its windows, each with what shows it: its GPU surface (all
//! windows share one renderer), its accessibility adapter, its native views, its text input and
//! its frame clock.

use super::{Shared, ShellEvent};
use octara::paint::DisplayList;
use octara::platform::{CursorStyle, Frame, Platform, PlatformWindow, TextInput};
use octara_render::{GpuRenderer, wgpu};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::window::Window;

/// The desktop the app runs on: arboard's clipboard, the event loop's proxy as the waker, and
/// windows made in the loop's callback the app runs in.
pub(crate) struct WinitPlatform {
    clipboard: Option<arboard::Clipboard>,
    shared: Rc<Shared>,
}

impl WinitPlatform {
    pub(crate) fn new(shared: Rc<Shared>) -> Self {
        WinitPlatform { clipboard: arboard::Clipboard::new().ok(), shared }
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
    fn prompt_for_paths(
        &mut self,
        o: &octara::platform::PathPromptOptions,
        reply: octara::platform::Reply<Option<Vec<std::path::PathBuf>>>,
    ) {
        let mut d = rfd::AsyncFileDialog::new();
        if let Some(p) = &o.prompt {
            d = d.set_title(p);
        }
        // Made here (the main thread: the panel shows now), awaited on a thread of its own.
        let paths: std::pin::Pin<Box<dyn std::future::Future<Output = Option<Vec<std::path::PathBuf>>> + Send>> =
            match (o.directories && !o.files, o.multiple) {
                (true, true) => Box::pin(async move { Some(d.pick_folders().await?.iter().map(|h| h.path().to_owned()).collect()) }),
                (true, false) => Box::pin(async move { Some(vec![d.pick_folder().await?.path().to_owned()]) }),
                (false, true) => Box::pin(async move { Some(d.pick_files().await?.iter().map(|h| h.path().to_owned()).collect()) }),
                (false, false) => Box::pin(async move { Some(vec![d.pick_file().await?.path().to_owned()]) }),
            };
        std::thread::spawn(move || reply(pollster::block_on(paths)));
    }
    fn prompt_for_new_path(
        &mut self,
        directory: &std::path::Path,
        name: Option<&str>,
        reply: octara::platform::Reply<Option<std::path::PathBuf>>,
    ) {
        let mut d = rfd::AsyncFileDialog::new().set_directory(directory);
        if let Some(n) = name {
            d = d.set_file_name(n);
        }
        let path = d.save_file();
        std::thread::spawn(move || reply(pollster::block_on(path).map(|h| h.path().to_owned())));
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
        window.set_ime_allowed(true);
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
            native,
            #[cfg(target_os = "macos")]
            host,
            #[cfg(target_os = "macos")]
            link,
            secure: false,
            cursor_style: CursorStyle::Arrow,
            cursor: (0.0, 0.0),
            pending: true,
            presented: None,
            resized: false,
            sized: false,
            unshown: None,
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
    /// Its native views (macOS: AppKit's; elsewhere none, so they draw as placeholders).
    #[cfg(target_os = "macos")]
    pub(crate) native: Option<super::native_mac::Native>,
    #[cfg(target_os = "macos")]
    host: Option<super::native_mac::Host>,
    /// Its frame clock (macOS); elsewhere winit's redraw requests, paced by the compositor
    /// (Wayland) or the swap chain's blocking present.
    #[cfg(target_os = "macos")]
    link: Option<super::pace_mac::DisplayLink>,
    /// A secure field has focus: the input method is off and (macOS) secure input on.
    secure: bool,
    /// How the pointer looks.
    cursor_style: CursorStyle,
    /// The pointer, in logical pixels.
    pub(crate) cursor: (f32, f32),
    /// The app has something new to show.
    pub(crate) pending: bool,
    /// When the last frame was presented.
    pub(crate) presented: Option<Instant>,
    /// It resized since the last frame (native views present with the frame).
    pub(crate) resized: bool,
    /// The app was told its size.
    pub(crate) sized: bool,
    /// `OCTARA_TRACE_INPUT`: the oldest input not yet shown, when it arrived, what it was, and
    /// when handling ended.
    pub(crate) unshown: Option<(Instant, &'static str, Instant)>,
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
    pub(crate) fn draw(&mut self, list: &DisplayList, renderer: &mut GpuRenderer) -> bool {
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
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(renderer.device(), &self.config);
                self.want_frame();
                None
            }
            _ => None,
        };
        let drawing = Instant::now();
        if let Some(frame) = &frame {
            let view = frame.texture.create_view(&Default::default());
            renderer.draw(list, scale, &view, (self.config.width, self.config.height));
        }
        #[cfg(target_os = "macos")]
        if let Some(n) = &self.native {
            n.place();
        }
        let shown = frame.is_some();
        if let Some(frame) = frame {
            self.window.pre_present_notify();
            renderer.queue().present(frame);
        }
        #[cfg(target_os = "macos")]
        if transaction {
            super::native_mac::Native::end_frame();
        }
        if !shown {
            return false;
        }
        self.pending = false;
        self.presented = Some(Instant::now());
        self.frames += 1;
        if let Some((at, what, handled)) = self.unshown.take() {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            eprintln!(
                "octara-shell: {what}: shown after {:.1} ms (handling {:.1}, waiting for the frame {:.1}, drawing {:.1})",
                ms(at.elapsed()),
                ms(handled - at),
                ms(drawing - handled),
                ms(drawing.elapsed()),
            );
        }
        true
    }

    /// Records an input for latency tracing (the first one not yet shown).
    pub(crate) fn traced(&mut self, at: Instant, what: &'static str) {
        if self.shared.trace && self.unshown.is_none() {
            self.unshown = Some((at, what, Instant::now()));
        }
    }

    /// The input method's candidate window follows the caret; a secure field turns the input
    /// method off and the platform's secure input on.
    fn text_input(&mut self, input: Option<&TextInput>) {
        if let Some(t) = input {
            let c = t.caret;
            self.window.set_ime_cursor_area(LogicalPosition::new(c.x, c.y), LogicalSize::new(c.w.max(1.0), c.h));
        }
        let secure = input.is_some_and(|t| t.secure);
        if secure != self.secure {
            self.secure = secure;
            self.window.set_ime_allowed(!secure);
            secure_input::set(secure);
        }
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
