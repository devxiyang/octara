//! The winit backend's side of the platform layer ([`octara::platform`]): the system's clipboard
//! and the event loop's waker, and its windows, each with what shows it: its GPU surface (all
//! windows share one renderer), its accessibility adapter, its native views, its text input and
//! its frame clock.

use super::{Shared, ShellEvent, trace};
use crate::gpu::{DeviceGeneration, FrameSubmission, Gpu, RenderCompletion};
#[cfg(target_os = "macos")]
use crate::presentation::DisplayTick;
use crate::presentation::{DrawableGeneration, Opportunity, Presentation};
#[cfg(target_os = "macos")]
use crate::{
    metal_acquire::AcquireWorker,
    metal_frame::{MetalFrame, MetalLayer},
};
use octara::capture::PaintFrame;
#[cfg(target_os = "ios")]
use octara::kernel::HandlerId;
use octara::platform::{Frame, NativeSnapshotRequest, OpenedWindow, Platform, PlatformWindow, PublishedFrame, TextInput, WindowViewport};
use octara::serde_json::{Value, json};
use octara_primitives::input::CursorStyle;
use octara_render::gpu::PendingPresentation;
use octara_render::wgpu;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
#[cfg(not(any(target_os = "ios", target_os = "macos")))]
use winit::dpi::LogicalPosition;
#[cfg(not(target_os = "ios"))]
use winit::dpi::LogicalSize;
use winit::dpi::PhysicalSize;
use winit::window::Window;

/// The desktop the app runs on: arboard's clipboard, the event loop's proxy as the waker, and
/// windows made in the loop's callback the app runs in.
pub(crate) struct WinitPlatform {
    #[cfg(not(target_os = "ios"))]
    clipboard: Option<arboard::Clipboard>,
    shared: Rc<Shared>,
    /// The menu bar (macOS), made with the first menus.
    #[cfg(target_os = "macos")]
    menubar: Option<super::menubar_mac::MenuBar>,
}

impl WinitPlatform {
    pub(crate) fn new(shared: Rc<Shared>) -> Self {
        WinitPlatform {
            #[cfg(not(target_os = "ios"))]
            clipboard: arboard::Clipboard::new().ok(),
            shared,
            #[cfg(target_os = "macos")]
            menubar: None,
        }
    }
}

// COPY_SRC allows a backdrop to sample a local region of this surface between rendering passes.
const SURFACE_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::COPY_SRC);

impl Platform for WinitPlatform {
    #[cfg(target_os = "ios")]
    fn read_text(&mut self) -> Option<String> {
        None
    }
    #[cfg(target_os = "ios")]
    fn write_text(&mut self, _: &str) {}
    #[cfg(not(target_os = "ios"))]
    fn read_text(&mut self) -> Option<String> {
        self.clipboard.as_mut()?.get_text().ok()
    }
    #[cfg(not(target_os = "ios"))]
    fn write_text(&mut self, text: &str) {
        if let Some(c) = &mut self.clipboard {
            let _ = c.set_text(text);
        }
    }
    #[cfg(not(target_os = "ios"))]
    fn read_image(&mut self) -> Option<octara_primitives::image::Image> {
        let i = self.clipboard.as_mut()?.get_image().ok()?;
        Some(octara_primitives::image::Image::rgba(i.width as u32, i.height as u32, i.bytes.into_owned()))
    }
    #[cfg(not(target_os = "ios"))]
    fn write_image(&mut self, image: &octara_primitives::image::Image) {
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
                let shared = self.shared.clone();
                self.menubar = super::menubar_mac::MenuBar::new(move |name, chord| {
                    shared.menu_commands.push(name, chord);
                    let _ = shared.proxy.send_event(ShellEvent::Wake);
                });
            }
            if let Some(bar) = &mut self.menubar {
                bar.set(menus);
            }
        }
        let _ = menus;
    }
    #[cfg(not(target_os = "ios"))]
    fn open_url(&mut self, url: &str) {
        if let Err(e) = open::that_detached(url) {
            eprintln!("octara-shell: cannot open {url}: {e}");
        }
    }
    fn open_window(&mut self, id: octara::app::WindowId, options: &octara::platform::WindowOptions) -> Result<OpenedWindow, String> {
        let shared = &self.shared;
        let event_loop = shared.event_loop().ok_or("windows open while the event loop runs")?;
        // Hidden until the accessibility adapter is attached (AccessKit requires it).
        let attrs = Window::default_attributes().with_title(&options.title).with_visible(false);
        #[cfg(not(target_os = "ios"))]
        let attrs = attrs.with_inner_size(LogicalSize::new(options.size.0, options.size.1));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        #[cfg(target_os = "ios")]
        let native_window = super::scene_ios::attach(&window)?;
        #[cfg(target_os = "ios")]
        let ios_input = {
            let proxy = shared.proxy.clone();
            let content = super::scene_ios::ui_view(&window)?;
            super::text_input_ios::TextInputIos::new(content, move || {
                let _ = proxy.send_event(ShellEvent::Wake);
            })
        };
        // AccessKit subclasses the content view and restores its parent class at teardown.
        // Install the scroll receiver first so the accessibility adapter owns the outer layer.
        #[cfg(target_os = "macos")]
        let scroll = super::scroll_mac::Bridge::attach(&window);
        let a11y = accesskit_winit::Adapter::with_event_loop_proxy(event_loop, &window, shared.proxy.clone());
        #[cfg(target_os = "ios")]
        native_window.makeKeyAndVisible();
        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        {
            window.set_visible(true);
            // An app started from a terminal comes to the front.
            window.focus_window();
        }
        let mut gpu = shared.gpu.borrow_mut();
        let instance = match &*gpu {
            Some(g) => g.instance.clone(),
            None => wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env()),
        };
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        if gpu.is_none() {
            *gpu = Some(Gpu::new(instance, &surface, shared.trace_gpu, &shared.proxy)?);
        }
        let g = gpu.as_ref().expect("the GPU");
        #[cfg(target_os = "macos")]
        let metal = (shared.async_metal && g.adapter.get_info().backend == wgpu::Backend::Metal).then(MetalPresentation::default);
        let caps = surface.get_capabilities(&g.adapter);
        if !caps.formats.contains(&g.format) {
            return Err("the window surface does not support the shared GPU format".into());
        }
        let size = drawable_size(&window);
        let desired_maximum_frame_latency = 2;
        #[cfg(target_os = "macos")]
        let desired_maximum_frame_latency = if metal.is_some() { shared.metal_frame_latency } else { desired_maximum_frame_latency };
        let config = wgpu::SurfaceConfiguration {
            usage: caps.usages & SURFACE_USAGE,
            format: g.format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        let wid = window.id();
        drop(gpu);
        #[cfg(target_os = "macos")]
        let text_input = {
            let proxy = shared.proxy.clone();
            super::text_input_mac::TextInputMac::new(&window, shared.trace || shared.trace_native_input, move || {
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
            let link = super::pace_mac::DisplayLink::new(&window, shared.trace, move || proxy.send_event(ShellEvent::Frame(wid)).is_ok());
            (native, host, link)
        };
        shared.windows.borrow_mut().insert(wid, id);
        let host = WinitWindow {
            window,
            shared: shared.clone(),
            surface,
            config,
            surface_generation: 0,
            presentation: Presentation::new(id),
            #[cfg(target_os = "macos")]
            metal,
            surface_occluded: false,
            pending_surface_configuration: Some(SurfaceConfigurationRequest {
                reason: "open",
                attempt: None,
                recreate: None,
                wait_for_redraw: false,
            }),
            a11y,
            #[cfg(target_os = "ios")]
            ios_back: None,
            #[cfg(target_os = "ios")]
            ios_back_preview: false,
            #[cfg(target_os = "ios")]
            ios_input,
            #[cfg(target_os = "macos")]
            text_input,
            #[cfg(target_os = "macos")]
            scroll,
            #[cfg(target_os = "macos")]
            native,
            #[cfg(target_os = "macos")]
            host,
            #[cfg(target_os = "macos")]
            link,
            #[cfg(target_os = "macos")]
            shown: Rc::default(),
            #[cfg(target_os = "macos")]
            show_on_first_commit: true,
            #[cfg(not(target_os = "ios"))]
            secure: false,
            #[cfg(not(target_os = "ios"))]
            ime_allowed: false,
            #[cfg(not(target_os = "ios"))]
            input_owner: None,
            #[cfg(not(target_os = "ios"))]
            ime_owner: None,
            cursor_style: CursorStyle::Arrow,
            cursor: (0.0, 0.0),
            last_traced_motion: None,
            hovering: Vec::new(),
            dropped: Vec::new(),
            last_submission_at: None,
            resized: false,
            viewport_checked: false,
            unshown: None,
            occluded: None,
            frames: 0,
            attempts: 0,
        };
        let viewport = host.viewport(host.drawable_size());
        Ok(OpenedWindow { window: Box::new(WindowHost(RefCell::new(host))), viewport })
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

fn drawable_size(window: &Window) -> PhysicalSize<u32> {
    #[cfg(target_os = "ios")]
    {
        super::scene_ios::drawable_size(window)
    }
    #[cfg(not(target_os = "ios"))]
    {
        window.inner_size()
    }
}

/// The runtime owns this adapter and drops it on window close, even if an AppWindow handle
/// survives. Only this backend can access its mutable native state; application code sees the
/// PlatformWindow protocol. No extra strong handle can keep a closed OS window alive.
pub(crate) struct WindowHost(pub(crate) RefCell<WinitWindow>);

impl PlatformWindow for WindowHost {
    fn present(&mut self, frame: Frame) {
        self.0.get_mut().present(frame);
    }
    #[cfg(not(target_os = "ios"))]
    fn shows_ask(&self, ask: &octara::ask::Ask) -> bool {
        self.0.borrow().shows_ask(ask)
    }
    fn shows_native(&self, kind: &str, slot: octara_primitives::native::Slot) -> bool {
        self.0.borrow().shows_native(kind, slot)
    }
    fn snapshot_native(&mut self, request: NativeSnapshotRequest) -> Result<Vec<String>, String> {
        self.0.get_mut().snapshot_native(request)
    }
    fn native_busy(&self, id: octara_primitives::native::NativeId) -> Option<String> {
        self.0.borrow().native_busy(id)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One winit window and what shows it.
pub(crate) struct WinitWindow {
    pub(crate) window: Arc<Window>,
    shared: Rc<Shared>,
    pub(crate) surface: wgpu::Surface<'static>,
    pub(crate) config: wgpu::SurfaceConfiguration,
    /// Invalidates evidence collected across surface recreation or reconfiguration.
    pub(crate) surface_generation: u64,
    presentation: Presentation,
    #[cfg(target_os = "macos")]
    metal: Option<MetalPresentation>,
    surface_occluded: bool,
    pending_surface_configuration: Option<SurfaceConfigurationRequest>,
    pub(crate) a11y: accesskit_winit::Adapter,
    #[cfg(target_os = "ios")]
    pub(crate) ios_back: Option<super::back_ios::BackNavigation>,
    /// A frame without a back command can serve as the preview when a detail page opens.
    #[cfg(target_os = "ios")]
    ios_back_preview: bool,
    #[cfg(target_os = "ios")]
    pub(crate) ios_input: super::text_input_ios::TextInputIos,
    #[cfg(target_os = "macos")]
    pub(crate) text_input: Option<super::text_input_mac::TextInputMac>,
    #[cfg(target_os = "macos")]
    pub(crate) scroll: Option<super::scroll_mac::Bridge>,
    /// Its native views (macOS: AppKit's; elsewhere none, so they draw as placeholders).
    #[cfg(target_os = "macos")]
    pub(crate) native: Option<super::native_mac::Native>,
    #[cfg(target_os = "macos")]
    host: Option<super::native_mac::Host>,
    /// Its native frame clock when available. Other windows use monitor-rate deadlines.
    #[cfg(target_os = "macos")]
    link: Option<super::pace_mac::DisplayLink>,
    /// The requests it shows natively now (macOS: menus and alerts).
    #[cfg(target_os = "macos")]
    shown: Rc<super::ask_mac::Shown>,
    /// A mount interrupted before its first committed frame leaves its native window hidden.
    #[cfg(target_os = "macos")]
    show_on_first_commit: bool,
    /// A secure field has focus: the input method is off and (macOS) secure input on.
    #[cfg(not(target_os = "ios"))]
    secure: bool,
    /// Whether the focused field currently accepts input-method composition.
    #[cfg(not(target_os = "ios"))]
    ime_allowed: bool,
    /// Current logical field, and the field that enabled the native composition session.
    #[cfg(not(target_os = "ios"))]
    input_owner: Option<octara_primitives::identity::NodeId>,
    #[cfg(not(target_os = "ios"))]
    ime_owner: Option<octara_primitives::identity::NodeId>,
    /// How the pointer looks.
    cursor_style: CursorStyle,
    /// The pointer, in logical pixels.
    pub(crate) cursor: (f32, f32),
    pub(crate) last_traced_motion: Option<(f32, f32)>,
    /// Files dragged from the system over it, and those dropped (sent once the drop's events
    /// are all in).
    pub(crate) hovering: Vec<std::path::PathBuf>,
    pub(crate) dropped: Vec<std::path::PathBuf>,
    /// End of the last submission and native transaction on the host thread.
    last_submission_at: Option<Instant>,
    /// It resized since the last frame (native views present with the frame).
    pub(crate) resized: bool,
    /// The actual viewport has been checked after creating the window.
    pub(crate) viewport_checked: bool,
    /// `OCTARA_TRACE_INPUT`: only the oldest input and bounded aggregate counts.
    unshown: Option<super::trace::Input>,
    pub(crate) occluded: Option<bool>,
    pub(crate) frames: u32,
    attempts: u64,
}

struct SurfaceConfigurationRequest {
    reason: &'static str,
    attempt: Option<u64>,
    recreate: Option<SurfaceRecreation>,
    /// A failed recreation can be retried on exposure without spinning the event loop.
    wait_for_redraw: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SurfaceRecreation {
    Lost,
    DeviceRecovery,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct MetalPresentation {
    worker: Option<AcquireWorker>,
    ready: Option<ReadyMetalDrawable>,
}

#[cfg(target_os = "macos")]
struct ReadyMetalDrawable {
    frame: MetalFrame,
    acquisition_request: u64,
}

enum WindowDrawable {
    Surface(wgpu::SurfaceTexture),
    #[cfg(target_os = "macos")]
    Metal {
        frame: MetalFrame,
        texture: wgpu::Texture,
    },
}

enum DrawAttempt {
    HandedOff { paint: PaintFrame, presentation: PendingPresentation, generation: DrawableGeneration, number: u32 },
    Recording,
    Failed,
}

pub(crate) struct RenderedWindow {
    pub(crate) window: winit::window::WindowId,
    drawable: WindowDrawable,
    generation: DrawableGeneration,
    number: u32,
    revision: u64,
    scale: f32,
    trace: Option<trace::Frame>,
    submission: Option<FrameSubmission>,
}

impl WindowDrawable {
    fn can_render_on_worker(&self) -> bool {
        match self {
            Self::Surface(_) => false,
            #[cfg(target_os = "macos")]
            Self::Metal { .. } => true,
        }
    }

    fn texture(&self) -> &wgpu::Texture {
        match self {
            Self::Surface(surface) => &surface.texture,
            #[cfg(target_os = "macos")]
            Self::Metal { texture, .. } => texture,
        }
    }

    fn present(self, gpu: &Gpu) -> Result<(), String> {
        match self {
            Self::Surface(surface) => {
                gpu.present(surface);
                Ok(())
            }
            #[cfg(target_os = "macos")]
            Self::Metal { frame, texture } => {
                drop(texture);
                frame.present(gpu.queue())
            }
        }
    }
}

impl WinitWindow {
    #[cfg(target_os = "macos")]
    fn can_acquire_metal(&self) -> bool {
        self.metal.is_some()
            // The first frame goes through the normal window-show path.
            && self.frames != 0
            && self.config.format == wgpu::TextureFormat::Bgra8Unorm
            && self.native.as_ref().is_some_and(|native| !native.has_hosted_views())
    }

    #[cfg(target_os = "macos")]
    fn metal_window_visible(&self) -> bool {
        self.occluded != Some(true) && !self.surface_occluded && self.native.as_ref().is_some_and(|native| native.window_visible())
    }

    #[cfg(target_os = "macos")]
    fn disable_metal(&mut self, error: impl std::fmt::Display) {
        self.stop_metal();
        self.request_draw();
        eprintln!("octara-shell: background drawable acquisition disabled: {error}");
    }

    #[cfg(target_os = "macos")]
    fn request_metal_drawable(&mut self, generation: DrawableGeneration) -> Result<(), String> {
        let layer = MetalLayer::from_surface(&self.surface)?;
        let metal = self.metal.as_mut().expect("the experimental Metal path is enabled");
        if metal.worker.is_none() {
            let proxy = self.shared.proxy.clone();
            let id = self.window.id();
            metal.worker = Some(
                AcquireWorker::spawn(move || {
                    let _ = proxy.send_event(ShellEvent::DrawableReady(id));
                })
                .map_err(|e| e.to_string())?,
            );
        }
        let acquisition = self.presentation.begin(generation).expect("one available acquisition per window");
        let started = self.shared.trace.then(Instant::now);
        if let Some(started) = started {
            trace::acquire(self.shared.trace_epoch, acquisition, "worker", started, None);
        }
        let worker = metal.worker.as_mut().expect("the Metal worker was started");
        if let Err(error) = worker.request(acquisition, layer, self.shared.metal_acquire_delay) {
            self.presentation.finish(acquisition, generation);
            if let Some(started) = started {
                trace::acquire(self.shared.trace_epoch, acquisition, "worker", started, Some((Instant::now(), "failed")));
            }
            return Err(error.to_string());
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn take_metal_drawable(&mut self, gpu: &Gpu) {
        let generation = self.drawable_generation(gpu.generation.number);
        let Some(worker) = self.metal.as_mut().and_then(|metal| metal.worker.as_mut()) else { return };
        let completion = match worker.take_ready() {
            Ok(Some(completion)) => completion,
            Ok(None) => return,
            Err(acquisition) => {
                self.presentation.finish(acquisition, generation);
                self.disable_metal("the Metal worker disconnected");
                return;
            }
        };
        let accepted = self.presentation.finish(completion.token, generation)
            && gpu.available()
            && self.pending_surface_configuration.is_none()
            && self.can_acquire_metal()
            && self.metal_window_visible();
        if self.shared.trace {
            let status = if !accepted {
                "stale"
            } else if completion.frame.is_some() {
                "ready"
            } else {
                "timeout"
            };
            trace::acquire(self.shared.trace_epoch, completion.token, "worker", completion.started, Some((completion.finished, status)));
        }
        if accepted && let Some(frame) = completion.frame {
            self.metal.as_mut().expect("the Metal path is still active").ready =
                Some(ReadyMetalDrawable { frame, acquisition_request: completion.token.serial() });
        } else if accepted {
            self.presentation.schedule.retry(Instant::now());
        }
    }

    #[cfg(target_os = "macos")]
    fn has_ready_drawable(&self) -> bool {
        self.metal.as_ref().is_some_and(|metal| metal.ready.is_some())
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn discard_metal_drawable(&mut self) {
        if let Some(metal) = &mut self.metal {
            metal.ready = None;
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn retire_metal_drawable(&mut self) -> bool {
        self.presentation.invalidate();
        self.discard_metal_drawable();
        self.presentation.waiting()
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn waiting_for_drawable(&self) -> bool {
        self.presentation.waiting()
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn stop_metal(&mut self) {
        self.presentation.invalidate();
        // Disconnect first. A ready drawable is released here and an active acquisition
        // can finish on its own without retaining this native window.
        self.metal = None;
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn sync_ios_back(&mut self, eligible: bool) {
        if !eligible {
            if let Some(back) = &mut self.ios_back {
                back.disarm();
            }
            return;
        }
        if !self.ios_back_preview || self.ios_back.as_ref().is_some_and(super::back_ios::BackNavigation::armed) {
            return;
        }
        self.ios_back_preview = false;
        let result = (|| {
            if self.ios_back.is_none() {
                self.ios_back = Some(super::back_ios::BackNavigation::new(&self.window, self.shared.proxy.clone())?);
            }
            self.ios_back.as_mut().expect("iOS back bridge initialized").arm()
        })();
        if let Err(error) = result {
            eprintln!("octara-shell: {error}");
        }
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn did_draw_ios_back(&mut self, owner: Option<HandlerId>) {
        self.ios_back_preview = owner.is_none();
        if let Some(back) = &mut self.ios_back {
            back.did_draw(owner);
        }
    }

    pub(crate) fn drawable_size(&self) -> PhysicalSize<u32> {
        drawable_size(&self.window)
    }

    pub(crate) fn scale(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    pub(crate) fn viewport(&self, pixels: PhysicalSize<u32>) -> WindowViewport {
        let scale = self.scale();
        let width = pixels.width as f32 / scale;
        let height = pixels.height as f32 / scale;
        #[cfg(target_os = "ios")]
        let (height, safe_area) = (self.ios_input.keyboard_top().unwrap_or(height), super::scene_ios::safe_area(&self.window));
        #[cfg(not(target_os = "ios"))]
        let safe_area = None;
        WindowViewport { size: (width, height), scale, safe_area }
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

    pub(crate) fn can_present(&self) -> bool {
        self.occluded != Some(true) && !self.surface_occluded && self.shared.gpu_available()
    }

    fn can_draw(&self) -> bool {
        self.pending_surface_configuration.is_none()
            && self.presentation.schedule.wants_frame()
            && self.can_present()
            && !self.presentation.waiting()
            && self.shared.renderer_idle()
    }

    pub(crate) fn request_draw(&mut self) {
        self.presentation.schedule.request();
    }

    pub(crate) fn redraw(&mut self) {
        self.surface_occluded = false;
        if let Some(request) = &mut self.pending_surface_configuration {
            request.wait_for_redraw = false;
        }
        self.presentation.schedule.redraw(Instant::now());
    }

    pub(crate) fn set_occluded(&mut self, occluded: bool) {
        self.occluded = Some(occluded);
        if occluded {
            self.presentation.invalidate();
            #[cfg(target_os = "macos")]
            self.discard_metal_drawable();
            self.pause_frame_clock();
        } else {
            self.surface_occluded = false;
            if let Some(request) = &mut self.pending_surface_configuration {
                request.wait_for_redraw = false;
            }
            self.request_draw();
        }
    }

    /// A timer-driven window keeps its deadline anchored to the last draw attempt. Events
    /// between frames can replace pending content without shifting the next opportunity.
    pub(crate) fn fallback_frame_deadline(&self, now: Instant) -> Option<Instant> {
        #[cfg(target_os = "macos")]
        if self.link.is_some() {
            return None;
        }
        self.can_draw().then(|| self.next_frame_at(now))
    }

    fn next_frame_at(&self, now: Instant) -> Instant {
        self.presentation.schedule.attempted().map_or(now, |t| t + self.refresh())
    }

    fn drawing_opportunity(&self, now: Instant) -> Option<Opportunity> {
        if !self.can_draw() {
            return None;
        }
        if let Some(opportunity) = self.presentation.schedule.opportunity(now) {
            return Some(opportunity);
        }
        if self.presentation.schedule.attempted().is_none() {
            return Some(Opportunity::Immediate);
        }
        #[cfg(target_os = "macos")]
        if let Some(link) = &self.link {
            // A one-shot after a quiet period can draw without waiting for the idle display
            // to ramp up. Once a burst starts the display link owns its cadence.
            return (!link.running() && self.next_frame_at(now) <= now).then_some(Opportunity::Immediate);
        }
        let deadline = self.next_frame_at(now);
        (deadline <= now).then_some(Opportunity::Timer(deadline))
    }

    /// This also runs when the renderer or drawable is busy, so a display callback can
    /// update the opportunity while either resource is outstanding.
    pub(crate) fn schedule_frame(&mut self, now: Instant) -> Option<(Opportunity, bool)> {
        let next = self.drawing_opportunity(now);
        self.sync_frame_clock();
        let opportunity = next?;
        self.presentation.schedule.retain(opportunity);
        Some((opportunity, self.presentation.schedule.continuous()))
    }

    pub(crate) fn visual_target(&self, opportunity: Opportunity) -> Option<Instant> {
        #[cfg(target_os = "macos")]
        if self.native.as_ref().is_some_and(|native| native.has_hosted_views()) {
            return None;
        }
        opportunity.target()
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn ready_to_draw(&self, now: Instant) -> bool {
        self.has_ready_drawable() && self.drawing_opportunity(now).is_some()
    }

    pub(crate) fn sync_frame_clock(&mut self) {
        #[cfg(target_os = "macos")]
        if self.link.is_some() {
            if self.pending_surface_configuration.is_some() || !self.can_present() {
                self.presentation.schedule.pause();
                self.set_frame_clock(false, "surface_unavailable");
            } else if self.presentation.schedule.wants_frame() {
                self.set_frame_clock(true, "frame_demand");
            }
        }
    }

    pub(crate) fn pause_frame_clock(&mut self) {
        self.presentation.schedule.pause();
        self.set_frame_clock(false, "pause_requested");
    }

    pub(crate) fn cancel_frames(&mut self) {
        self.presentation.schedule.cancel();
        #[cfg(target_os = "macos")]
        self.stop_metal();
        self.set_frame_clock(false, "pause_requested");
    }

    fn set_frame_clock(&self, _running: bool, _reason: &'static str) {
        #[cfg(target_os = "macos")]
        if let Some(link) = &self.link
            && link.running() != _running
        {
            if self.shared.trace_frame_clock {
                trace::frame_clock(self.shared.trace_epoch, self.window.id(), _running, _reason, link.has_queued_tick());
            }
            if _running {
                link.start();
            } else {
                link.stop();
            }
        }
    }

    /// Reads one queued display timing. The host advances the app to its actual time
    /// before deciding whether this opportunity can draw or should let the window rest.
    #[cfg(target_os = "macos")]
    pub(crate) fn take_display_tick(&self, handled_at: Instant) -> Option<Opportunity> {
        let tick = self.link.as_ref()?.take_tick()?;
        let expires_at = tick.target.unwrap_or_else(|| handled_at + self.refresh());
        Some(Opportunity::DisplayLink { tick, handled_at, expires_at })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn acknowledge_display_wake(&self) -> bool {
        self.link.as_ref().is_some_and(super::pace_mac::DisplayLink::acknowledge_wake)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn offer_display_tick(&mut self, opportunity: Opportunity, now: Instant) {
        let reason = if self.pending_surface_configuration.is_some() || !self.can_present() {
            self.sync_frame_clock();
            "surface_unavailable"
        } else if !opportunity.valid(now) {
            "expired_display_opportunity"
        } else if !super::pace_mac::tick_draws(self.last_submission_at) {
            "low_power_pacing"
        } else {
            match self.presentation.schedule.display_tick(opportunity) {
                DisplayTick::Offered if self.presentation.schedule.wants_frame() => return,
                DisplayTick::Offered => "no_pending_drawing",
                DisplayTick::Repeated => "repeated_display_target",
                DisplayTick::Rest => {
                    self.set_frame_clock(false, "no_frame_demand");
                    "no_pending_drawing"
                }
            }
        };
        self.skip_opportunity(opportunity, reason);
    }

    pub(crate) fn skip_opportunity(&self, opportunity: Opportunity, reason: &'static str) {
        if self.shared.trace {
            opportunity.skipped(self.window.id(), self.shared.trace_epoch, reason);
        }
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        if (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.request_surface_configuration("resize", None, None);
    }

    fn request_surface_configuration(&mut self, reason: &'static str, attempt: Option<u64>, recreate: Option<SurfaceRecreation>) {
        let recreate = recreate.or_else(|| self.pending_surface_configuration.as_ref().and_then(|request| request.recreate));
        self.pending_surface_configuration = Some(SurfaceConfigurationRequest { reason, attempt, recreate, wait_for_redraw: false });
        self.resized = true;
        self.request_draw();
        self.presentation.invalidate();
        self.presentation.clear_submission();
        #[cfg(target_os = "macos")]
        self.discard_metal_drawable();
        self.pause_frame_clock();
    }

    pub(crate) fn ready_to_configure(&self) -> bool {
        self.pending_surface_configuration.as_ref().is_some_and(|request| !request.wait_for_redraw) && !self.presentation.waiting()
    }

    /// Called by the window thread after the renderer finished draining the shared GPU queue.
    pub(crate) fn apply_surface_configuration(&mut self, gpu: &Gpu) -> Result<(), String> {
        if !self.ready_to_configure() {
            return Ok(());
        }
        let mut request = self.pending_surface_configuration.take().expect("a configuration request is ready");
        let configure_started = self.shared.trace.then(Instant::now);
        if request.recreate.is_some()
            && let Err(error) = self.recreate_surface(gpu)
        {
            if request.recreate == Some(SurfaceRecreation::DeviceRecovery) {
                return Err(error);
            }
            if !gpu.generation.is_lost() {
                eprintln!("octara-shell: cannot recreate the window surface: {error}");
                self.surface_occluded = true;
                request.wait_for_redraw = true;
            }
            self.pending_surface_configuration = Some(request);
            return Ok(());
        }
        let configured = gpu.apply_surface_configuration(&self.surface, &self.config);
        if configured {
            self.surface_generation = self.surface_generation.checked_add(1).expect("surface generation exhausted");
            self.presentation.clear_submission();
            self.resized = true;
            if request.recreate.is_some() {
                self.surface_occluded = false;
            }
            self.request_draw();
        }
        if let Some(started) = configure_started {
            trace::SurfaceConfiguration {
                window: self.window.id(),
                attempt: request.attempt,
                surface_generation: self.surface_generation,
                device_generation: gpu.generation.number,
                size: [self.config.width, self.config.height],
                desired_maximum_frame_latency: self.config.desired_maximum_frame_latency,
                reason: request.reason,
                configured,
            }
            .log(self.shared.trace_epoch, started, &self.surface);
        }
        if !configured {
            let recovering_device = request.recreate == Some(SurfaceRecreation::DeviceRecovery);
            self.pending_surface_configuration = Some(request);
            if recovering_device || gpu.available() {
                return Err("the GPU could not configure the window surface after draining".into());
            }
        }
        Ok(())
    }

    pub(crate) fn recover_surface(&mut self, gpu: &Gpu) {
        let size = self.drawable_size();
        if super::resize::validate_physical(size, gpu.device().limits().max_texture_dimension_2d).is_ok() {
            self.config.width = size.width;
            self.config.height = size.height;
        }
        self.last_submission_at = None;
        self.presentation.schedule.restart();
        self.surface_occluded = false;
        // A lost device can retain an acquired texture in the old surface. Replacing the
        // surface also gives its old swapchain the correct device for unconfiguration.
        self.request_surface_configuration("device_recovery", None, Some(SurfaceRecreation::DeviceRecovery));
    }

    fn recreate_surface(&mut self, gpu: &Gpu) -> Result<(), String> {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        let metal_layer = if gpu.adapter.get_info().backend == wgpu::Backend::Metal {
            // SAFETY: retain the layer on the main thread without taking ownership of the
            // HAL surface. The replacement stays in its existing place in the layer tree.
            unsafe { self.surface.as_hal::<wgpu::hal::api::Metal>() }.map(|hal| hal.render_layer().lock().clone())
        } else {
            None
        };
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        let surface = if let Some(layer) = &metal_layer {
            let ptr = objc2::rc::Retained::as_ptr(layer).cast_mut().cast();
            // SAFETY: the retained CAMetalLayer stays alive during this call and the Metal
            // backend retains it for the lifetime of the replacement surface.
            unsafe { gpu.instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(ptr)) }
        } else {
            gpu.instance.create_surface(self.window.clone())
        };
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        let surface = gpu.instance.create_surface(self.window.clone());
        let surface = surface.map_err(|error| error.to_string())?;
        let capabilities = surface.get_capabilities(&gpu.adapter);
        if !capabilities.formats.contains(&self.config.format) || !capabilities.alpha_modes.contains(&self.config.alpha_mode) {
            return Err("the replacement surface does not support the active format and alpha mode".into());
        }
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            native.surface_replaced();
        }
        self.config.usage = capabilities.usages & SURFACE_USAGE;
        self.surface = surface;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn place_occluded_native_views(&self) {
        // A painted capture still needs native layout while display acquisition is paused.
        if !self.can_present()
            && !self.presentation.waiting()
            && let Some(native) = &self.native
        {
            let _transaction = native.begin_frame(&self.surface, self.resized);
            native.place();
        }
    }

    /// Start any blocking acquisition before building the display list. An acquisition is
    /// independent of the drawing published when it begins.
    pub(crate) fn prepare_draw(&mut self, scale: f32, _gpu: &Gpu, opportunity: Opportunity) -> bool {
        if self.pending_surface_configuration.is_some() {
            self.skip_opportunity(opportunity, "surface_not_configured");
            return false;
        }
        if !self.matches_drawable(scale) {
            self.presentation.schedule.retry(Instant::now());
            self.skip_opportunity(opportunity, "drawable_mismatch");
            return false;
        }
        #[cfg(target_os = "macos")]
        if self.can_acquire_metal() {
            if !self.metal_window_visible() {
                self.discard_metal_drawable();
                self.surface_occluded = true;
                self.sync_frame_clock();
                self.skip_opportunity(opportunity, "surface_unavailable");
                return false;
            }
            if !self.has_ready_drawable() {
                match self.request_metal_drawable(self.drawable_generation(_gpu.generation.number)) {
                    Ok(()) => return false,
                    Err(error) => self.disable_metal(error),
                }
            }
        } else {
            self.discard_metal_drawable();
        }
        true
    }

    /// Submits a published drawing and its native placements in the same transaction.
    pub(crate) fn draw(&mut self, published: &PublishedFrame, gpu: &mut Gpu, started: Instant, mut trace: Option<trace::Frame>) -> bool {
        self.presentation.schedule.begin_draw(started);
        self.attempts += 1;
        if let Some(t) = &mut trace {
            t.surface_generation = self.surface_generation;
        }
        let generation = gpu.generation.clone();
        let result = generation.run(|| self.draw_frame(published, gpu, &mut trace));
        let recording = matches!(&result, Some(DrawAttempt::Recording));
        let submitted = if let Some(DrawAttempt::HandedOff { paint, presentation, generation, number }) = result {
            self.record_present(gpu, paint, presentation, generation, number)
        } else {
            false
        };
        if !submitted && !recording {
            self.presentation.schedule.retry(Instant::now());
        }
        self.log_draw(published.revision, published.scale, trace, &generation, submitted);
        submitted
    }

    fn log_draw(&mut self, revision: u64, scale: f32, trace: Option<trace::Frame>, generation: &DeviceGeneration, submitted: bool) {
        if let Some(t) = trace {
            let finished = Instant::now();
            let status = t.acquisition.map(|(_, status)| status);
            let epoch = self.shared.trace_epoch;
            eprintln!(
                "{}",
                json!({
                    "schema": "octara.frame_trace/1",
                    "window": format!("{:?}", self.window.id()),
                    "attempt": self.attempts,
                    "frame": self.frames,
                    "revision": revision,
                    "surface_generation": t.surface_generation,
                    "device_generation": generation.number,
                    "app_clock_ms": t.app_clock_ms,
                    "visible": self.window.is_visible(),
                    "occluded": self.occluded,
                    "surface_acquired": status.is_some_and(trace::AcquireStatus::acquired),
                    "acquisition_request": t.acquisition_request,
                    "acquire_status": status.map(trace::AcquireStatus::as_str).or(t.surface_start.map(|_| "interrupted")),
                    "skip_reason": t.skip_reason,
                    "submitted": submitted,
                    "device_lost": generation.is_lost(),
                    "size_px": [self.config.width, self.config.height],
                    "scale": scale,
                    "opportunity": t.opportunity.json(epoch),
                    "timeline_us": t.timeline_json(epoch),
                    "presents_with_transaction": t.presents_with_transaction,
                    "cpu_us": t.cpu_json(finished),
                    "render": t.render.map(|(profile, submit)| trace::render(profile, submit)),
                    "render_worker": t.render_worker,
                    "paint": self.presentation.submitted_paint(self.drawable_generation(generation.number)).filter(|_| submitted),
                    "input": self.unshown.as_ref().map(|i| i.json(t.started, t.present_returned)),
                    "gpu_execution_us": null,
                    "physical_display_us": null,
                })
            );
        }
        if submitted {
            self.unshown = None;
        }
    }

    fn draw_frame(&mut self, published: &PublishedFrame, gpu: &mut Gpu, trace: &mut Option<trace::Frame>) -> DrawAttempt {
        let scale = published.scale;
        let configuring = self.pending_surface_configuration.is_some();
        if configuring || !self.matches_drawable(scale) {
            if let Some(t) = trace.as_mut() {
                t.skip_reason = Some(if configuring { "surface_not_configured" } else { "drawable_mismatch" });
            }
            return DrawAttempt::Failed;
        }
        let attempt = trace.as_ref().map(|_| self.attempts);
        let generation = self.drawable_generation(gpu.generation.number);
        #[cfg(target_os = "macos")]
        let prepared =
            self.metal.as_mut().and_then(|metal| metal.ready.take()).and_then(|ReadyMetalDrawable { mut frame, acquisition_request }| {
                let size = (self.config.width, self.config.height);
                match frame.texture(gpu, self.config.format, size) {
                    Ok(texture) => {
                        if let Some(t) = trace.as_mut() {
                            t.acquisition_request = Some(acquisition_request);
                        }
                        Some(WindowDrawable::Metal { frame, texture })
                    }
                    Err(error) => {
                        self.disable_metal(error);
                        None
                    }
                }
            });
        #[cfg(not(target_os = "macos"))]
        let prepared: Option<WindowDrawable> = None;
        #[cfg(target_os = "macos")]
        let transaction = self.native.as_ref().and_then(|n| n.begin_frame(&self.surface, self.resized));
        if let Some(t) = trace.as_mut() {
            #[cfg(target_os = "macos")]
            {
                t.presents_with_transaction = transaction.as_ref().is_some_and(|transaction| transaction.synchronizes_present);
            }
            t.surface_generation = self.surface_generation;
            t.surface_start = Some(Instant::now());
        }
        let drawable = if let Some(drawable) = prepared {
            if let Some(t) = trace.as_mut() {
                t.acquisition = Some((Instant::now(), trace::AcquireStatus::Success));
            }
            drawable
        } else {
            let acquisition = self.presentation.begin(generation).expect("surface is free to acquire");
            #[cfg(target_os = "macos")]
            if let Some(t) = trace.as_mut() {
                t.acquisition_request = Some(acquisition.serial());
            }
            #[cfg(target_os = "macos")]
            let started = self.shared.trace.then(Instant::now);
            #[cfg(target_os = "macos")]
            if let Some(started) = started {
                trace::acquire(self.shared.trace_epoch, acquisition, "sync", started, None);
            }
            #[cfg(target_os = "macos")]
            if !self.shared.metal_acquire_delay.is_zero() {
                std::thread::sleep(self.shared.metal_acquire_delay);
            }
            let texture = gpu.generation.run(|| self.surface.get_current_texture());
            let accepted = self.presentation.finish(acquisition, generation);
            #[cfg(target_os = "macos")]
            if let Some(started) = started {
                let status = texture.as_ref().map_or("failed", |texture| trace::AcquireStatus::from_texture(texture).as_str());
                trace::acquire(self.shared.trace_epoch, acquisition, "sync", started, Some((Instant::now(), status)));
            }
            let Some(texture) = texture.filter(|_| accepted) else { return DrawAttempt::Failed };
            if let Some(t) = trace.as_mut() {
                t.acquisition = Some((Instant::now(), trace::AcquireStatus::from_texture(&texture)));
            }
            if gpu.generation.is_lost() {
                return DrawAttempt::Failed;
            }
            let frame = match texture {
                wgpu::CurrentSurfaceTexture::Success(f) => f,
                wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                    // This texture is still usable. Reconfigure before the next acquisition.
                    self.request_surface_configuration("suboptimal", attempt, None);
                    f
                }
                status => {
                    #[cfg(target_os = "macos")]
                    drop(transaction);
                    self.acquire_failed(gpu, status, attempt);
                    return DrawAttempt::Failed;
                }
            };
            WindowDrawable::Surface(frame)
        };
        let number = self.frames.checked_add(1).expect("window submission identity exhausted");
        let sample = published.paint_sample();
        let report: Option<Box<dyn FnOnce(octara_render::gpu::GpuTimings) + Send>> = if self.shared.trace_gpu {
            let window = format!("{:?}", self.window.id());
            let attempt = self.attempts;
            let generation = self.surface_generation;
            let device = gpu.generation.clone();
            Some(Box::new(move |timings| {
                if !device.is_lost() {
                    eprintln!("{}", trace::gpu(&window, attempt, number, sample.revision, generation, device.number, timings));
                }
            }))
        } else {
            None
        };
        if gpu.threaded() && drawable.can_render_on_worker() {
            let window = self.window.id();
            let published = published.clone();
            let device = gpu.generation.clone();
            #[cfg(debug_assertions)]
            let (hold, epoch) = (self.shared.render_worker_hold, self.shared.trace_epoch);
            let mut trace = trace.take();
            if let Some(t) = trace.as_mut() {
                t.render_worker = true;
            }
            let recording = gpu.start_work(Box::new(move |renderer| {
                let submission =
                    crate::gpu::submit_frame(renderer, &device, &published, drawable.texture(), number, generation.surface, report);
                if let Some(t) = trace.as_mut() {
                    t.encoded = Some(Instant::now());
                }
                #[cfg(debug_assertions)]
                if !hold.is_zero()
                    && let Some(submission) = &submission
                {
                    eprintln!(
                        "{}",
                        json!({
                            "schema": "octara.pending_presentation/1",
                            "at_us": epoch.elapsed().as_secs_f64() * 1_000_000.0,
                            "paint": &submission.paint,
                        })
                    );
                    std::thread::sleep(hold);
                }
                RenderCompletion::Window(Box::new(RenderedWindow {
                    window,
                    drawable,
                    generation,
                    number,
                    revision: published.revision,
                    scale,
                    trace,
                    submission,
                }))
            }));
            return if recording { DrawAttempt::Recording } else { DrawAttempt::Failed };
        }
        let Some(mut submission) = gpu.submit_frame(published, drawable.texture(), number, self.surface_generation, report) else {
            return DrawAttempt::Failed;
        };
        if let Some(t) = trace.as_mut() {
            t.render = submission.profile.take();
            t.encoded = Some(Instant::now());
        }
        if gpu.generation.is_lost() {
            return DrawAttempt::Failed;
        }
        let presented = self.present_target(gpu, drawable, &submission.paint, trace);
        #[cfg(target_os = "macos")]
        drop(transaction);
        if presented {
            DrawAttempt::HandedOff { paint: submission.paint, presentation: submission.presentation, generation, number }
        } else {
            DrawAttempt::Failed
        }
    }

    pub(crate) fn rendered(&mut self, gpu: &mut Gpu, completed: RenderedWindow, latest: Option<u64>) -> bool {
        let RenderedWindow { drawable, generation, number, revision, scale, mut trace, submission, .. } = completed;
        let current = gpu.generation.clone();
        let eligible = gpu.available()
            && generation == self.drawable_generation(current.number)
            && self.pending_surface_configuration.is_none()
            && self.matches_drawable(scale);
        #[cfg(target_os = "macos")]
        let eligible = eligible && self.can_acquire_metal() && self.metal_window_visible();
        let submitted = if eligible && let Some(mut submission) = submission {
            if let Some(t) = trace.as_mut() {
                t.render = submission.profile.take();
            }
            current.run(|| self.present_target(gpu, drawable, &submission.paint, &mut trace)) == Some(true)
                && self.record_present(gpu, submission.paint, submission.presentation, generation, number)
        } else {
            drop(drawable);
            if let Some(t) = trace.as_mut() {
                t.skip_reason = Some("stale_render");
            }
            false
        };
        if !submitted || latest != Some(revision) {
            // The next opportunity may already have arrived while this frame was recording.
            self.request_draw();
        }
        self.log_draw(revision, scale, trace, &current, submitted);
        submitted
    }

    fn present_target(&mut self, gpu: &Gpu, drawable: WindowDrawable, _paint: &PaintFrame, trace: &mut Option<trace::Frame>) -> bool {
        #[cfg(target_os = "macos")]
        if let Some(n) = &self.native {
            n.place();
        }
        if let Some(t) = trace.as_mut() {
            t.native_placed = Some(Instant::now());
        }
        self.window.pre_present_notify();
        #[cfg(target_os = "macos")]
        if self.shared.trace_presented
            && let WindowDrawable::Metal { frame, .. } = &drawable
        {
            trace::metal_presented(frame, _paint, self.shared.trace_epoch, [self.config.width, self.config.height]);
        }
        if let Some(t) = trace.as_mut() {
            t.present_start = Some(Instant::now());
        }
        if let Err(error) = drawable.present(gpu) {
            #[cfg(target_os = "macos")]
            self.disable_metal(error);
            #[cfg(not(target_os = "macos"))]
            eprintln!("octara-shell: {error}");
            return false;
        }
        if let Some(t) = trace.as_mut() {
            t.present_returned = Some(Instant::now());
        }
        true
    }

    fn record_present(
        &mut self,
        gpu: &mut Gpu,
        paint: PaintFrame,
        presentation: PendingPresentation,
        generation: DrawableGeneration,
        number: u32,
    ) -> bool {
        if gpu.generation.is_lost() || !self.presentation.record_submission(paint, generation) {
            return false;
        }
        gpu.accept_presentation(presentation);
        self.resized = false;
        self.surface_occluded = false;
        self.last_submission_at = Some(Instant::now());
        self.frames = number;
        true
    }

    fn acquire_failed(&mut self, gpu: &Gpu, status: wgpu::CurrentSurfaceTexture, attempt: Option<u64>) {
        match status {
            wgpu::CurrentSurfaceTexture::Outdated => self.request_surface_configuration("outdated", attempt, None),
            wgpu::CurrentSurfaceTexture::Lost => {
                self.request_surface_configuration("surface_lost", attempt, Some(SurfaceRecreation::Lost));
            }
            wgpu::CurrentSurfaceTexture::Occluded => self.surface_occluded = true,
            wgpu::CurrentSurfaceTexture::Validation => {
                gpu.instance.poll_all(false);
                if !gpu.generation.is_lost() {
                    eprintln!("octara-shell: cannot acquire the window surface after validation failed");
                    self.surface_occluded = true;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn matches_drawable(&self, scale: f32) -> bool {
        let drawable = self.drawable_size();
        (drawable.width, drawable.height) == (self.config.width, self.config.height) && self.scale() == scale
    }

    pub(crate) fn configured_size(&self) -> Option<PhysicalSize<u32>> {
        self.pending_surface_configuration.is_none().then(|| PhysicalSize::new(self.config.width, self.config.height))
    }

    pub(crate) fn has_submitted(&self, revision: u64) -> bool {
        self.submitted_paint().is_some_and(|paint| paint.sample.revision == revision)
    }

    pub(crate) fn submitted_paint(&self) -> Option<&PaintFrame> {
        if self.pending_surface_configuration.is_some() {
            return None;
        }
        let gpu = self.shared.gpu.borrow();
        let gpu = gpu.as_ref().filter(|g| g.available())?;
        self.presentation.submitted_paint(self.drawable_generation(gpu.generation.number))
    }

    fn drawable_generation(&self, device: u64) -> DrawableGeneration {
        DrawableGeneration { surface: self.surface_generation, device }
    }

    pub(crate) fn last_submission(&self) -> Option<u32> {
        (self.shared.gpu_available() && self.last_submission_at.is_some()).then_some(self.frames)
    }

    /// Shows a request the platform takes (file dialogs: rfd's, made here on the main thread so
    /// the panel shows now, awaited on a thread of their own); its answer comes back as input.
    #[cfg(not(target_os = "ios"))]
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

    #[cfg(not(target_os = "ios"))]
    pub(crate) fn ime_enabled(&mut self) {
        self.ime_owner = self.input_owner.filter(|_| self.ime_allowed);
    }

    #[cfg(not(target_os = "ios"))]
    pub(crate) fn accepts_ime(&self) -> bool {
        self.ime_allowed && self.ime_owner.is_some() && self.ime_owner == self.input_owner
    }

    #[cfg(not(target_os = "ios"))]
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
        #[cfg(target_os = "ios")]
        let ime = self.ios_input.inspect();
        #[cfg(not(target_os = "ios"))]
        let ime = json!({
            "allowed": self.ime_allowed,
            "secure": self.secure,
            "native_focus": self.window.has_focus(),
        });
        let submission_age_ms =
            self.last_submission_at.filter(|_| self.shared.gpu_available()).map(|at| at.elapsed().as_secs_f64() * 1000.0);
        let state = json!({
            "ime": ime,
            "window": {
                "frames": self.frames,
                "pending": self.presentation.schedule.pending(),
                "surface_generation": self.surface_generation,
                "device_generation": self.shared.gpu.borrow().as_ref().map(|gpu| gpu.generation.number),
                "submission_age_ms": submission_age_ms,
                "presented_age_ms": submission_age_ms,
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

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
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
        #[cfg(target_os = "ios")]
        self.ios_input.update(input);
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
        #[cfg(not(any(target_os = "ios", target_os = "macos")))]
        self.winit_text_input(input);
    }

    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
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

impl WinitWindow {
    fn present(&mut self, frame: Frame) {
        // Placements are applied with the next frame: one is needed.
        let placed = !frame.native.is_empty();
        #[cfg(target_os = "macos")]
        if placed {
            self.retire_metal_drawable();
        }
        #[cfg(target_os = "macos")]
        if let Some(host) = &self.host {
            for change in frame.native {
                host.apply(change);
            }
        }
        // Underlays show through cutouts once the surface uses the renderer's alpha mode.
        #[cfg(target_os = "macos")]
        if placed
            && self.native.as_ref().is_some_and(|native| native.transparent())
            && self.config.alpha_mode != wgpu::CompositeAlphaMode::PostMultiplied
        {
            self.config.alpha_mode = wgpu::CompositeAlphaMode::PostMultiplied;
            self.request_surface_configuration("alpha_mode", None, None);
        }
        #[cfg(target_os = "macos")]
        if std::mem::take(&mut self.show_on_first_commit) {
            self.window.set_visible(true);
            self.window.focus_window();
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
        self.presentation.schedule.publish(frame.redraw || placed, frame.continuous);
        for id in frame.withdrawn {
            #[cfg(target_os = "macos")]
            super::ask_mac::withdraw(&self.shown, id);
            let _ = id;
        }
        #[cfg(not(target_os = "ios"))]
        for (id, ask) in frame.asks {
            self.show_ask(id, ask);
        }
    }
    #[cfg(not(target_os = "ios"))]
    fn shows_ask(&self, ask: &octara::ask::Ask) -> bool {
        #[cfg(target_os = "macos")]
        if self.host.is_some() && super::ask_mac::shows(ask) {
            return true;
        }
        matches!(ask, octara::ask::Ask::Paths(_) | octara::ask::Ask::NewPath { .. })
    }
    fn shows_native(&self, kind: &str, slot: octara_primitives::native::Slot) -> bool {
        #[cfg(target_os = "macos")]
        if self.host.is_some() {
            return super::native_mac::Host::shows(kind, slot);
        }
        let _ = (kind, slot);
        false
    }
    fn snapshot_native(&mut self, request: NativeSnapshotRequest) -> Result<Vec<String>, String> {
        #[cfg(target_os = "macos")]
        if let Some(host) = &self.host {
            return host.snapshot(request);
        }
        let _ = request;
        Err("this platform takes no snapshots of native views yet".into())
    }
    fn native_busy(&self, id: octara_primitives::native::NativeId) -> Option<String> {
        #[cfg(target_os = "macos")]
        if let Some(host) = &self.host {
            return host.busy(id);
        }
        let _ = id;
        None
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
        #[cfg(target_os = "macos")]
        self.stop_metal();
        self.pause_frame_clock();
        #[cfg(target_os = "macos")]
        drop(self.scroll.take());
        #[cfg(not(target_os = "ios"))]
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

#[cfg(not(any(target_os = "ios", target_os = "macos")))]
mod secure_input {
    pub fn set(_: bool) {}
}
