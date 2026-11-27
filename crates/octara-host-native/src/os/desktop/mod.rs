//! The Winit desktop adapter for Linux and Windows.

mod cursor;
mod events;
mod gestures;
mod ime;
mod keys;
#[cfg(target_os = "linux")]
mod linux_clipboard;
#[cfg(target_os = "linux")]
mod linux_preferences;
mod run_loop;
mod services;
mod wheel;
#[cfg(target_os = "windows")]
mod windows_preferences;

use crate::os::{
    ActiveEditorView, HostWake, OsHost, OsWindow, OsWindowInit, PlatformEvent, ServiceCleanup, WindowAccessibility, WindowTextInput,
};
use accesskit_winit::Adapter;
use events::{WindowInput, color_scheme};
use octara::WindowOptions;
use octara::module::{HostSurface, HostSurfaceKind, RawNativeSurface};
use octara::runtime::WindowId;
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceRequest};
use octara_model::geometry::{Point, WindowViewport};
use octara_model::identity::TargetGuard;
use octara_model::input::{CursorStyle, HostReply};
use octara_model::paint::Image;
use octara_model::semantics::A11yAction;
use octara_model::theme::ColorScheme;
use octara_model::window::{StatusBarStyle, UserAttention, WindowGeometry, WindowGeometryRequest, WindowMaterial};
use octara_pipeline::NativePublication;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use run_loop::DesktopProxy;
use services::{Clipboard, Services};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::{Arc, OnceLock};
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Theme, UserAttentionType, Window, WindowId as NativeId};

#[cfg(target_os = "windows")]
pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Windows;
#[cfg(not(target_os = "windows"))]
pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Linux;

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(DesktopHost {
        event_loop: None,
        state: Rc::new(DesktopState {
            clipboard: Rc::new(Clipboard::new()),
            proxy: Arc::new(OnceLock::new()),
            windows: RefCell::new(HashMap::new()),
        }),
    })
}

struct DesktopHost<'a> {
    event_loop: Option<&'a ActiveEventLoop>,
    state: Rc<DesktopState>,
}

struct DesktopState {
    clipboard: Rc<Clipboard>,
    proxy: Arc<OnceLock<DesktopProxy>>,
    windows: RefCell<HashMap<NativeId, Weak<DesktopWindowState>>>,
}

impl OsHost for DesktopHost<'_> {
    fn run(self: Box<Self>, driver: Box<dyn crate::os::ShellDriver>) -> Result<(), String> {
        run_loop::run(self.state, driver)
    }

    fn wake(&self) -> HostWake {
        let proxy = self.state.proxy.clone();
        Arc::new(move || {
            if let Some(proxy) = proxy.get() {
                proxy.send(PlatformEvent::UserWake);
            }
        })
    }

    fn follows_window_focus(&self) -> bool {
        true
    }

    fn color_scheme(&self) -> Option<ColorScheme> {
        self.event_loop?.system_theme().map(color_scheme)
    }

    #[cfg(target_os = "linux")]
    fn locale(&self) -> Option<Arc<str>> {
        linux_preferences::locale()
    }

    #[cfg(target_os = "windows")]
    fn reduced_motion(&self) -> Option<bool> {
        windows_preferences::reduced_motion()
    }

    fn create_window(&mut self, id: WindowId, options: &WindowOptions, context: OsWindowInit<'_>) -> Result<Box<dyn OsWindow>, String> {
        let event_loop = self.event_loop.ok_or("Desktop windows require a running event loop")?;
        let size = options.viewport.size;
        let mut attributes = Window::default_attributes()
            .with_title(options.presentation.title.as_ref())
            .with_inner_size(LogicalSize::new(size.width, size.height))
            .with_resizable(options.resizable)
            .with_maximized(options.maximized)
            .with_visible(false)
            .with_transparent(context.transparent);
        if let Some(position) = options.position {
            attributes = attributes.with_position(LogicalPosition::new(position.x, position.y));
        }
        if let Some(size) = options.min_size {
            attributes = attributes.with_min_inner_size(LogicalSize::new(size.width, size.height));
        }
        if let Some(size) = options.max_size {
            attributes = attributes.with_max_inner_size(LogicalSize::new(size.width, size.height));
        }
        let window = event_loop.create_window(attributes).map_err(|error| error.to_string())?;
        let proxy = self.state.proxy.get().expect("desktop event loop proxy").native.clone();
        let adapter = Adapter::with_event_loop_proxy(event_loop, &window, proxy);
        let sender = context.sender;
        #[cfg(target_os = "windows")]
        let preferences = windows_preferences::Observer::new(&window, sender.clone());
        if let Some(theme) = window.theme() {
            sender.send_event(events::theme_changed(theme));
        }
        let native = Rc::new(DesktopWindowState {
            id,
            #[cfg(target_os = "windows")]
            _preferences: preferences,
            accessibility: RefCell::new(DesktopAccessibility { adapter, published: None }),
            window,
            input: RefCell::new(WindowInput::default()),
        });
        self.state.windows.borrow_mut().insert(native.window.id(), Rc::downgrade(&native));
        let reply = move |reply| sender.send_event(PlatformEvent::HostReply { window: id, reply });
        Ok(Box::new(DesktopWindow {
            native,
            desktop: Rc::downgrade(&self.state),
            services: Services::new(self.state.clipboard.clone(), reply),
            cursor_visible: true,
        }))
    }

    fn open_url(&mut self, url: &str) {
        let _ = open::that_detached(url);
    }

    fn read_clipboard(&mut self, accept: ClipboardKinds) -> Option<Vec<ClipboardItem>> {
        self.state.clipboard.with_native(|native| services::read_clipboard(native, accept))?.ok()
    }

    fn write_clipboard(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        self.state.clipboard.write(items)
    }

    fn write_clipboard_image(&mut self, image: &Image) {
        let frame = image.frame();
        let data = arboard::ImageData { width: frame.width as usize, height: frame.height as usize, bytes: (&*frame.rgba).into() };
        let _ = self.state.clipboard.with_native(|clipboard| clipboard.set_image(data));
    }
}

struct DesktopWindowState {
    id: WindowId,
    #[cfg(target_os = "windows")]
    _preferences: Option<windows_preferences::Observer>,
    accessibility: RefCell<DesktopAccessibility>,
    window: Window,
    input: RefCell<WindowInput>,
}

struct DesktopWindow {
    native: Rc<DesktopWindowState>,
    desktop: Weak<DesktopState>,
    services: Services,
    cursor_visible: bool,
}

impl DesktopWindow {
    fn unregister(&self) {
        if let Some(desktop) = self.desktop.upgrade() {
            desktop.windows.borrow_mut().remove(&self.native.window.id());
        }
    }
}

impl Drop for DesktopWindow {
    fn drop(&mut self) {
        self.unregister();
    }
}

// SAFETY: The desktop window owns the Winit Window and its native display and window handles.
// Closing hides the window and stops event routing while retaining its native resources until drop.
unsafe impl OsWindow for DesktopWindow {
    fn raw_handles(&self) -> Result<(RawDisplayHandle, RawWindowHandle), String> {
        let window = &self.native.window;
        let display = window.display_handle().map_err(|error| error.to_string())?.as_raw();
        let window = window.window_handle().map_err(|error| error.to_string())?.as_raw();
        Ok((display, window))
    }

    fn host_surface(&self) -> HostSurface<'_> {
        let surface = HostSurface::new(SURFACE_KIND, Some(self.native.id));
        if let Some(handles) = self.raw_handles().ok().and_then(module_handles) {
            // SAFETY: These native handles are borrowed from the live Winit window.
            return unsafe { surface.with_raw_native(handles) };
        }
        surface
    }

    fn physical_size(&self) -> [u32; 2] {
        let size = self.native.window.inner_size();
        [size.width, size.height]
    }

    fn scale_factor(&self) -> f32 {
        self.native.window.scale_factor() as f32
    }

    fn set_cursor(&mut self, style: CursorStyle) {
        let cursor = cursor::cursor_icon(style);
        if let Some(icon) = cursor {
            self.native.window.set_cursor(icon);
        }
        let visible = cursor.is_some();
        if self.cursor_visible != visible {
            self.native.window.set_cursor_visible(visible);
            self.cursor_visible = visible;
        }
    }

    fn request_redraw(&mut self) {
        self.native.window.request_redraw();
    }

    fn pre_present_notify(&mut self) {
        self.native.window.pre_present_notify();
    }

    fn is_focused(&self) -> bool {
        self.native.window.has_focus()
    }

    fn set_transparent(&mut self, transparent: bool) {
        self.native.window.set_transparent(transparent);
    }

    fn request_user_attention(&mut self, attention: UserAttention) {
        let attention = match attention {
            UserAttention::Informational => UserAttentionType::Informational,
            UserAttention::Critical => UserAttentionType::Critical,
        };
        self.native.window.request_user_attention(Some(attention));
    }

    fn apply_presentation(&mut self, title: &str, color_scheme: ColorScheme, _material: WindowMaterial, _status_bar: StatusBarStyle) {
        self.native.window.set_title(title);
        self.native.window.set_theme(Some(match color_scheme {
            ColorScheme::Light => Theme::Light,
            ColorScheme::Dark => Theme::Dark,
        }));
    }

    fn window_geometry(&mut self, viewport: WindowViewport) -> WindowGeometry {
        let window = &self.native.window;
        let position = window.outer_position().ok().map(|position| {
            let position = position.to_logical::<f32>(window.scale_factor());
            Point::new(position.x, position.y)
        });
        WindowGeometry {
            position,
            size: viewport.size,
            scale: viewport.scale,
            maximized: window.is_maximized(),
            fullscreen: window.fullscreen().is_some(),
        }
    }

    fn set_window_geometry(&mut self, geometry: WindowGeometryRequest) {
        let window = &self.native.window;
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

    fn focus(&mut self) {
        self.native.window.focus_window();
    }

    fn minimize(&mut self) {
        self.native.window.set_minimized(true);
    }

    fn toggle_maximize(&mut self) {
        self.native.window.set_maximized(!self.native.window.is_maximized());
    }

    fn start_window_drag(&mut self) {
        let _ = self.native.window.drag_window();
    }

    fn show(&mut self) {
        self.native.window.set_visible(true);
    }

    fn close(&mut self) {
        self.unregister();
        self.native.window.set_ime_allowed(false);
        self.native.window.set_visible(false);
    }

    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        self
    }

    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        Some(self)
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        self.services.request(id, request)
    }
}

impl WindowTextInput for DesktopWindow {
    fn sync_text_input(&mut self, editor: Option<&ActiveEditorView<'_>>) {
        self.native.input.borrow_mut().ime.sync(&self.native.window, editor);
    }

    fn inspect_text_input(&self) -> serde_json::Value {
        self.native.input.borrow().ime.inspect(&self.native.window)
    }
}

struct DesktopAccessibility {
    adapter: Adapter,
    published: Option<u64>,
}

impl WindowAccessibility for DesktopWindow {
    fn synchronize(&mut self, publication: &NativePublication) {
        let mut accessibility = self.native.accessibility.borrow_mut();
        let DesktopAccessibility { adapter, published } = &mut *accessibility;
        if let Some(previous) = *published {
            // Deliver every tree delta even when the renderer skips a frame.
            for (version, update) in publication.take_accessibility_updates() {
                if version > previous {
                    adapter.update_if_active(|| update.as_ref().clone());
                    *published = Some(version);
                }
            }
        } else if let Some(frame) = publication.latest()
            && let Some(snapshot) = publication.snapshot_full()
        {
            adapter.update_if_active(|| {
                publication.take_accessibility_updates();
                *published = Some(frame.number);
                snapshot
            });
        }
    }

    fn clear(&mut self) {
        self.native.accessibility.borrow_mut().published = None;
    }

    fn accepts_action(&self, _publication: &NativePublication, _target: TargetGuard, _action: &A11yAction) -> bool {
        // Desktop actions are routed and validated as AccessKit action requests.
        false
    }
}

#[cfg(target_os = "windows")]
fn module_handles((_, window): (RawDisplayHandle, RawWindowHandle)) -> Option<RawNativeSurface> {
    use std::ptr::NonNull;

    let RawWindowHandle::Win32(handle) = window else { return None };
    Some(RawNativeSurface::Win32 {
        hwnd: NonNull::new(handle.hwnd.get() as *mut _)?,
        hinstance: handle.hinstance.and_then(|instance| NonNull::new(instance.get() as *mut _)),
    })
}

#[cfg(not(target_os = "windows"))]
fn module_handles((display, window): (RawDisplayHandle, RawWindowHandle)) -> Option<RawNativeSurface> {
    match (window, display) {
        (RawWindowHandle::Wayland(window), RawDisplayHandle::Wayland(display)) => {
            Some(RawNativeSurface::Wayland { display: display.display, surface: window.surface })
        }
        (RawWindowHandle::Xlib(window), RawDisplayHandle::Xlib(display)) => {
            #[allow(clippy::unnecessary_cast, reason = "Xlib's unsigned long can be 32-bit")]
            let id = window.window as u64;
            Some(RawNativeSurface::Xlib { display: display.display?, window: id })
        }
        _ => None,
    }
}

type Reply = Arc<dyn Fn(HostReply) + Send + Sync>;
