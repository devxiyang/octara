//! Platform services and window capabilities used by the shared event loop.

use crate::{ShellConfig, ShellEvent};
use octara::WindowOptions;
use octara::module::{HostSurface, HostSurfaceKind};
use octara::runtime::{Input, WindowId};
use octara::tree::editor::{Composition, EditorSlot};
use octara_gpu::DrawTarget;
use octara_model::ask::{Ask, AskId, ClipboardItem, ClipboardKinds, Menu, ServiceError, ServiceId, ServiceRequest};
use octara_model::geometry::{Point, Rect, Size, WindowViewport};
use octara_model::identity::{EditorSessionId, NativeId, TargetGuard};
use octara_model::input::{EditorEvent, GestureEvent, HostReply, Modifiers, WheelPhase};
use octara_model::native::{NativeChange, NativeCommand, NativeMeasureRequest, NativeSlot, NativeSnapshotRequest};
use octara_model::paint::Image;
use octara_model::semantics::A11yAction;
use octara_model::text::TextGeometry;
use octara_model::theme::ColorScheme;
use octara_model::window::{StatusBarStyle, UserAttention, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowMaterial};
use octara_pipeline::NativePublication;
use octara_render::{gpu::OutputColor, wgpu};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use winit::event::{Ime, KeyEvent, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::EventLoopProxy;
use winit::window::{Fullscreen, Theme, UserAttentionType, Window, WindowAttributes};

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_frame_clock;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_media;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_metal;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_preferences;
#[cfg(target_os = "macos")]
mod corners;
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod desktop;
#[cfg(not(target_os = "ios"))]
mod gestures;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod scheduling;

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
use desktop as platform;
#[cfg(target_os = "ios")]
use ios as platform;
#[cfg(target_os = "macos")]
use macos as platform;

pub fn create_os_host() -> Box<dyn OsHost> {
    platform::create()
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) fn decode_avif(bytes: &[u8]) -> Result<Image, &'static str> {
    apple_media::decode_avif(bytes).ok_or("Unsupported, oversized, or damaged AVIF image")
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(crate) fn decode_avif(_: &[u8]) -> Result<Image, &'static str> {
    Err("The built-in decoder on this platform does not support AVIF")
}

pub struct OsWindowContext<'a> {
    pub window: &'a Arc<Window>,
    pub id: WindowId,
    pub backend: wgpu::Backend,
    pub config: &'a ShellConfig,
    pub proxy: EventLoopProxy<ShellEvent>,
}

pub struct HostEvent {
    pub window: WindowId,
    pub input: Input,
}

pub type WindowHostEvent = Input;
pub type ServiceCleanup = Box<dyn FnOnce()>;

/// Current editor state is borrowed from the tree only for the duration of synchronization.
pub struct ActiveEditorView<'a> {
    pub session: EditorSessionId,
    pub document_rev: u64,
    pub value: &'a str,
    pub selection: (usize, usize),
    pub composition: Option<&'a Composition>,
    pub editor: &'a EditorSlot,
    pub multiline: bool,
    pub secure: bool,
    pub caret: Option<Rect>,
    pub geometry: Option<&'a TextGeometry>,
}

pub enum KeyDispatch {
    Forward,
    Consumed,
    Input(Input),
}

pub trait OsHost {
    fn initialize(&mut self, _proxy: EventLoopProxy<ShellEvent>) {}
    fn module_surface_kind(&self) -> HostSurfaceKind {
        platform::SURFACE_KIND
    }
    fn take_launch_items(&mut self) -> (Vec<String>, Vec<PathBuf>) {
        (Vec::new(), Vec::new())
    }
    fn application_active(&self) -> Option<bool> {
        None
    }
    fn follows_window_focus(&self) -> bool {
        false
    }
    fn is_ready(&self) -> bool {
        true
    }
    fn reduced_motion(&self) -> Option<bool> {
        None
    }
    fn locale(&self) -> Option<Arc<str>> {
        None
    }
    fn text_scale(&self) -> Option<f32> {
        None
    }
    fn high_contrast(&self) -> Option<bool> {
        None
    }
    /// Demand reported directly by the OS. Other platforms use AccessKit's demand events.
    fn platform_accessibility_active(&self) -> Option<bool> {
        None
    }
    fn supports_window_vibrancy(&self) -> bool {
        false
    }
    fn window_attributes(&self, attributes: WindowAttributes, _options: &WindowOptions) -> WindowAttributes {
        attributes
    }
    fn prepare_window(&self, _window: &Window) -> Result<(), String> {
        Ok(())
    }
    fn report_error(&self, error: &str) {
        eprintln!("Octara: {error}");
    }
    fn create_window(&mut self, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String>;
    fn set_menus(&mut self, _menus: &[Menu]) {}
    fn set_app_badge(&mut self, _badge: Option<&str>) {}
    fn request_user_attention(&mut self, window: Option<&Window>, attention: UserAttention) {
        if let Some(window) = window {
            let attention = match attention {
                UserAttention::Informational => UserAttentionType::Informational,
                UserAttention::Critical => UserAttentionType::Critical,
            };
            window.request_user_attention(Some(attention));
        }
    }
    fn open_url(&mut self, _url: &str) {}
    fn read_clipboard(&mut self, _accept: ClipboardKinds) -> Option<Vec<ClipboardItem>> {
        None
    }
    fn write_clipboard(&mut self, _items: &[ClipboardItem]) -> Result<(), ServiceError> {
        Err(ServiceError::Unavailable)
    }
    fn write_clipboard_image(&mut self, _image: &Image) {}
    fn drain_platform_events(&mut self, _out: &mut Vec<HostEvent>) {}
    /// Configure the thread that encodes and submits interactive frames.
    fn frame_thread_setup(&self) -> Option<fn()> {
        None
    }
    /// Configure workers for asynchronous media transfers, decoding and resizing.
    fn utility_thread_setup(&self) -> Option<fn()> {
        None
    }
    fn background_thread_setup(&self) -> Option<fn()> {
        None
    }
}

pub trait OsWindow {
    fn text_input(&mut self) -> &mut dyn WindowTextInput;
    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        None
    }
    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup>;
    fn on_winit_gesture(&mut self, _event: &WindowEvent, _anchor: Option<Point>, _modifiers: Modifiers) -> Option<GestureEvent> {
        None
    }

    /// Supplies the native phase for the matching winit wheel event when available.
    fn on_winit_wheel(&mut self, _delta: MouseScrollDelta, _phase: TouchPhase) -> Option<WheelPhase> {
        None
    }

    /// Supplies handles valid while the host borrows the live window.
    fn module_surface<'a>(&'a self, window: &'a Window, kind: HostSurfaceKind, id: WindowId) -> HostSurface<'a> {
        let surface = HostSurface::new(kind, Some(id));
        match platform::raw_module_surface(window) {
            // SAFETY: The platform backend obtains the live native handles
            // from the borrowed winit window on the UI thread.
            Some(raw) => unsafe { surface.with_raw_native(raw) },
            None => surface,
        }
    }

    fn asks(&mut self) -> Option<&mut dyn WindowAsks> {
        None
    }
    fn native_views(&mut self) -> Option<&mut dyn WindowNativeViews> {
        None
    }
    fn presentation(&mut self) -> Option<&mut dyn WindowPresentation> {
        None
    }
    fn scene(&mut self) -> Option<&mut dyn WindowScene> {
        None
    }

    fn configure_chrome(&mut self, _window: &Window, _options: &WindowOptions) {}

    fn window_is_vibrant(&self) -> bool {
        false
    }

    fn window_chrome(&mut self) -> WindowChrome {
        WindowChrome::default()
    }

    fn prepare_window_metrics(&mut self, _window: &Window) {}

    fn surface_size(&self, window: &Window) -> PhysicalSize<u32> {
        window.inner_size()
    }

    fn window_geometry(&mut self, window: &Window, viewport: WindowViewport) -> WindowGeometry {
        read_window_geometry(window, viewport, window.is_maximized(), window.fullscreen().is_some())
    }

    fn set_window_geometry(&mut self, window: &Window, geometry: WindowGeometryRequest) {
        apply_window_geometry(window, geometry);
    }

    fn apply_presentation(
        &mut self,
        window: &Window,
        title: &str,
        color_scheme: ColorScheme,
        _material: WindowMaterial,
        _status_bar: StatusBarStyle,
    ) {
        window.set_title(title);
        window.set_theme(Some(match color_scheme {
            ColorScheme::Light => Theme::Light,
            ColorScheme::Dark => Theme::Dark,
        }));
    }

    fn focus_window(&mut self, window: &Window) {
        window.focus_window();
    }

    fn minimize_window(&mut self, window: &Window) {
        window.set_minimized(true);
    }

    fn toggle_maximize(&mut self, window: &Window) {
        window.set_maximized(!window.is_maximized());
    }

    fn start_window_drag(&mut self, window: &Window) {
        let _ = window.drag_window();
    }

    fn file_drop_position(&self) -> Option<Point> {
        None
    }

    fn show(&mut self, window: &Window) {
        window.set_visible(true);
    }
}

/// Direct delivery for platform accessibility consumers.
pub trait WindowAccessibility {
    fn synchronize(&mut self, publication: &NativePublication);
    fn clear(&mut self);
    fn accepts_action(&self, publication: &NativePublication, target: TargetGuard, action: &A11yAction) -> bool;
}

fn read_window_geometry(window: &Window, viewport: WindowViewport, maximized: bool, fullscreen: bool) -> WindowGeometry {
    let position = window.outer_position().ok().map(|position| {
        let position = position.to_logical::<f32>(window.scale_factor());
        Point::new(position.x, position.y)
    });
    WindowGeometry { position, size: viewport.size, scale: viewport.scale, maximized, fullscreen }
}

fn apply_window_geometry(window: &Window, geometry: WindowGeometryRequest) {
    if geometry.fullscreen == Some(false) {
        window.set_fullscreen(None);
    }
    if geometry.maximized == Some(false) {
        window.set_maximized(false);
    }
    if let Some(size) =
        geometry.size.filter(|size| size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0)
    {
        let _ = window.request_inner_size(LogicalSize::new(f64::from(size.width), f64::from(size.height)));
    }
    if let Some(position) = geometry.position.filter(|position| position.x.is_finite() && position.y.is_finite()) {
        window.set_outer_position(LogicalPosition::new(f64::from(position.x), f64::from(position.y)));
    }
    if geometry.maximized == Some(true) {
        window.set_maximized(true);
    }
    if geometry.fullscreen == Some(true) {
        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
    }
}

pub trait WindowTextInput {
    fn sync_text_input(&mut self, session: Option<&ActiveEditorView<'_>>);
    fn on_winit_key(&mut self, _event: &KeyEvent) -> KeyDispatch {
        KeyDispatch::Forward
    }
    fn on_unhandled_winit_key(&mut self, _event: &KeyEvent) -> bool {
        false
    }
    fn on_winit_ime(&mut self, _event: &Ime) -> Option<EditorEvent> {
        None
    }
    fn focused(&mut self, _active: bool) {}
    fn drain_text_input(&mut self, _out: &mut Vec<WindowHostEvent>) {}
    fn inspect_text_input(&self) -> serde_json::Value;
}

pub trait WindowNativeViews {
    fn sync_native_views(&mut self, changes: &[NativeChange]);
    fn send_native_command(&mut self, _command: NativeCommand) {}
    fn shows_native(&self, kind: &str, slot: NativeSlot) -> bool;
    fn measure_native(&mut self, request: NativeMeasureRequest) -> Option<Size>;
    fn snapshot_native(&mut self, request: NativeSnapshotRequest);
    fn native_busy(&self, id: NativeId) -> Option<String>;
    fn has_underlay(&self) -> bool;
    fn drain_native_events(&mut self, out: &mut Vec<WindowHostEvent>);
    fn inspect_native_views(&self) -> serde_json::Value;
}

pub trait WindowAsks {
    /// Returns the ask for host fallback when this backend cannot display it.
    fn show_ask(&mut self, id: AskId, ask: Ask) -> Option<Ask>;
    fn withdraw_ask(&mut self, id: AskId);
}

pub trait WindowScene {
    fn viewport(&self, viewport: WindowViewport) -> WindowViewport;
    fn keyboard_open(&self) -> bool;
    fn sync_back(&mut self, owner: Option<TargetGuard>);
    fn did_present(&mut self);
    fn set_suspended(&mut self, suspended: bool);
    fn drain_scene_events(&mut self, out: &mut Vec<WindowHostEvent>);
}

/// The predicted scanout time and current interval reported by the platform clock.
#[derive(Clone, Copy, Debug)]
pub struct FrameTick {
    pub target: Instant,
    pub interval: Duration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameClockActivity {
    /// Stop issuing ticks immediately, for example when a window is suspended.
    Paused,
    /// Keep ticking at a low rate through a short idle grace period.
    Idle,
    /// Keep ticking at a low rate while motion is active.
    Low,
    /// Request the screen's fastest available cadence.
    Interactive,
}

pub trait WindowPresentation {
    fn externally_paced(&self) -> bool;
    fn set_frame_clock_activity(&mut self, activity: FrameClockActivity);
    fn surface_will_reconfigure(&mut self) -> bool;
    fn set_transactional_presentation(&mut self, _native_rev: u64, _transactional: bool) {}
    fn abandon_native_view_frame(&mut self) {}
    fn try_acquire_drawable(&mut self, surface: &Arc<wgpu::Surface<'static>>, color: OutputColor, from_clock: bool) -> DrawableAcquisition;
    fn present(&mut self, drawable: Box<dyn Drawable>, queue: &wgpu::Queue) -> Result<(), String>;

    /// Present the drawable and commit native view placements for its revision.
    /// Backends with a platform transaction call the callback inside that transaction.
    fn present_with_native_views(
        &mut self,
        drawable: Box<dyn Drawable>,
        queue: &wgpu::Queue,
        _native_rev: u64,
        commit_native_views: &mut dyn FnMut(),
    ) -> Result<(), String> {
        self.present(drawable, queue)?;
        commit_native_views();
        Ok(())
    }
}

pub trait Drawable: DrawTarget {
    fn present(self: Box<Self>, queue: &wgpu::Queue) -> Result<(), String>;
    fn suboptimal(&self) -> bool {
        false
    }
}

impl DrawTarget for Box<dyn Drawable> {
    fn texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<(wgpu::Texture, OutputColor), String> {
        self.as_mut().texture(device, queue)
    }
}

pub enum DrawableAcquisition {
    Ready(Box<dyn Drawable>),
    WaitingForClock,
    Retry,
    Reconfigure,
    Lost,
    Suspended,
}

struct SurfaceDrawable {
    frame: wgpu::SurfaceTexture,
    _surface: Arc<wgpu::Surface<'static>>,
    color: OutputColor,
    suboptimal: bool,
}

impl DrawTarget for SurfaceDrawable {
    fn texture(&mut self, _: &wgpu::Device, _: &wgpu::Queue) -> Result<(wgpu::Texture, OutputColor), String> {
        Ok((self.frame.texture.clone(), self.color))
    }
}

impl Drawable for SurfaceDrawable {
    fn present(self: Box<Self>, queue: &wgpu::Queue) -> Result<(), String> {
        queue.present(self.frame);
        Ok(())
    }

    fn suboptimal(&self) -> bool {
        self.suboptimal
    }
}

pub fn surface_drawable(surface: &Arc<wgpu::Surface<'static>>, color: OutputColor) -> DrawableAcquisition {
    use wgpu::CurrentSurfaceTexture as Status;
    let (frame, suboptimal) = match surface.get_current_texture() {
        Status::Success(frame) => (frame, false),
        Status::Suboptimal(frame) => (frame, true),
        Status::Timeout | Status::Occluded => return DrawableAcquisition::Retry,
        Status::Outdated => return DrawableAcquisition::Reconfigure,
        Status::Lost | Status::Validation => return DrawableAcquisition::Lost,
    };
    DrawableAcquisition::Ready(Box::new(SurfaceDrawable { frame, _surface: surface.clone(), color, suboptimal }))
}

pub fn reply(proxy: &EventLoopProxy<ShellEvent>, window: WindowId, reply: HostReply) {
    let _ = proxy.send_event(ShellEvent::Input { window, input: Input::Host(reply) });
}
