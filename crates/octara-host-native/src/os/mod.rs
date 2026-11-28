//! Platform services and window capabilities used by the shared event loop.

use crate::ShellConfig;
use octara::WindowOptions;
use octara::module::{HostSurface, HostSurfaceKind};
use octara::runtime::{Input, WindowId};
use octara::tree::editor::{Composition, EditorSlot};
#[cfg(all(any(target_os = "macos", target_os = "ios"), not(feature = "wgpu")))]
use octara_gpu::DrawableTarget;
use octara_gpu::{BackendCapabilities, DeviceDrawnFrame, GpuBackend, GpuDevice};
use octara_model::ask::{Ask, AskId, ClipboardItem, ClipboardKinds, Menu, ServiceError, ServiceId, ServiceRequest};
use octara_model::geometry::{Rect, Size, WindowViewport};
use octara_model::identity::{EditorSessionId, NativeId, TargetGuard};
use octara_model::input::{CursorStyle, DragItem, HostReply};
use octara_model::native::{NativeChange, NativeCommand, NativeMeasureRequest, NativeSlot, NativeSnapshotRequest};
use octara_model::paint::Image;
use octara_model::semantics::A11yAction;
use octara_model::text::TextGeometry;
use octara_model::theme::ColorScheme;
use octara_model::window::{
    StatusBarStyle, UserAttention, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowLevel, WindowMaterial,
};
use octara_pipeline::NativePublication;
use octara_render::OutputColor;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod events;
pub use events::{FileDropPhase, HostWake, LoopState, PlatformEvent, PlatformKey, PlatformSender, PointerPhase, ShellDriver};

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_frame_clock;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod apple_media;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_metal;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple_preferences;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(crate) mod apple_transport;
#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod scheduling;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
mod windows;

#[cfg(target_os = "ios")]
use ios as platform;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
use windows as platform;

pub fn create_os_host() -> Box<dyn OsHost> {
    platform::create()
}

#[cfg(all(any(target_os = "macos", target_os = "ios"), not(feature = "wgpu")))]
pub(crate) type NativeGpuBackend = octara_gpu::MetalBackend;
#[cfg(any(feature = "wgpu", not(any(target_os = "macos", target_os = "ios"))))]
pub(crate) type NativeGpuBackend = octara_gpu::WgpuBackend;
pub(crate) type NativeGpuDevice = <NativeGpuBackend as GpuBackend>::Device;
pub type NativeSurfaceRendererFactory = <NativeGpuBackend as GpuBackend>::RendererFactory;
pub(crate) type NativeSurfaceLease = <NativeGpuDevice as GpuDevice>::SurfaceLease;
pub(crate) type NativeDrawnFrame = DeviceDrawnFrame<NativeGpuDevice>;
pub type NativeSurfaceSession = <NativeGpuBackend as GpuBackend>::Surface;
pub type Drawable = <NativeGpuDevice as GpuDevice>::Drawable;
pub type DrawableAcquisition = octara_gpu::DrawableAcquisition<Drawable>;

pub struct OsWindowContext<'a> {
    pub backend: BackendCapabilities,
    pub config: &'a ShellConfig,
    pub sender: PlatformSender,
    pub transparent: bool,
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

pub trait OsHost {
    fn run(self: Box<Self>, driver: Box<dyn ShellDriver>) -> Result<(), String>;
    fn wake(&self) -> HostWake;

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
    /// Whether the platform can attach new windows, such as when a mobile scene is connected.
    fn is_ready(&self) -> bool {
        true
    }
    fn color_scheme(&self) -> Option<ColorScheme> {
        None
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
    fn report_error(&self, error: &str) {
        eprintln!("Octara: {error}");
    }
    fn create_window(&mut self, id: WindowId, options: &WindowOptions, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String>;
    fn set_menus(&mut self, _menus: &[Menu]) {}
    fn set_app_badge(&mut self, _badge: Option<&str>) {}
    fn request_user_attention(&mut self, window: Option<&mut dyn OsWindow>, attention: UserAttention) {
        if let Some(window) = window {
            window.request_user_attention(attention);
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

/// A native window owned by the host on the UI thread.
///
/// # Safety
///
/// The display and window handles must stay valid until the owner is dropped, including after
/// `close`. GPU work may still hold a surface made from these handles after the window is closed.
pub unsafe trait OsWindow {
    fn raw_handles(&self) -> Result<(RawDisplayHandle, RawWindowHandle), String>;
    fn host_surface(&self) -> HostSurface<'_>;
    fn physical_size(&self) -> [u32; 2];
    fn scale_factor(&self) -> f32;
    fn request_redraw(&mut self) {}
    fn pre_present_notify(&mut self) {}
    fn set_cursor(&mut self, _style: CursorStyle) {}
    fn set_transparent(&mut self, _transparent: bool) {}
    fn is_focused(&self) -> bool {
        false
    }
    fn request_user_attention(&mut self, _attention: UserAttention) {}
    fn text_input(&mut self) -> &mut dyn WindowTextInput;
    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        None
    }
    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup>;
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
    fn configure_chrome(&mut self, _options: &WindowOptions) {}
    fn window_is_vibrant(&self) -> bool {
        false
    }
    fn window_chrome(&mut self) -> WindowChrome {
        WindowChrome::default()
    }
    fn prepare_window_metrics(&mut self) {}
    fn window_geometry(&mut self, viewport: WindowViewport) -> WindowGeometry {
        WindowGeometry { size: viewport.size, scale: viewport.scale, ..WindowGeometry::default() }
    }
    fn set_window_geometry(&mut self, _geometry: WindowGeometryRequest) {}
    fn apply_presentation(
        &mut self,
        _title: &str,
        _color_scheme: ColorScheme,
        _material: WindowMaterial,
        _status_bar: StatusBarStyle,
        _level: WindowLevel,
    ) {
    }
    fn focus(&mut self) {}
    fn minimize(&mut self) {}
    fn toggle_maximize(&mut self) {}
    fn start_window_drag(&mut self) {}
    fn start_drag_out(&mut self, _items: Vec<DragItem>) -> bool {
        false
    }
    fn show(&mut self) {}
    fn close(&mut self) {}
}

/// Direct delivery for platform accessibility consumers.
pub trait WindowAccessibility {
    fn synchronize(&mut self, publication: &NativePublication);
    fn clear(&mut self);
    fn accepts_action(&self, publication: &NativePublication, target: TargetGuard, action: &A11yAction) -> bool;
}

pub trait WindowTextInput {
    /// A native input method owns keys while it has marked text.
    fn is_composing(&self) -> bool {
        false
    }

    fn sync_text_input(&mut self, session: Option<&ActiveEditorView<'_>>);
    /// Give the native text system a key that Octara has not consumed.
    fn on_unhandled_key(&mut self, _event: &PlatformKey) -> bool {
        false
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
    /// Resizing this surface would retire drawable storage still queued for display.
    fn resize_waits_for_display(&self) -> bool {
        false
    }
    fn set_transactional_presentation(&mut self, _native_rev: u64, _transactional: bool, _resizing: bool) {}
    fn abandon_native_view_frame(&mut self) {}
    fn try_acquire_drawable(&mut self, surface: &NativeSurfaceSession, color: OutputColor, from_clock: bool) -> DrawableAcquisition;

    /// Present the drawable and commit native view placements for its revision.
    /// Backends with a platform transaction call the callback inside that transaction.
    fn present_with_native_views(
        &mut self,
        drawable: Box<Drawable>,
        _native_rev: u64,
        commit_native_views: &mut dyn FnMut(),
    ) -> Result<(), String> {
        drawable.present()?;
        commit_native_views();
        Ok(())
    }
}

pub fn reply(sender: &PlatformSender, window: WindowId, reply: HostReply) {
    sender.send_event(PlatformEvent::HostReply { window, reply });
}
