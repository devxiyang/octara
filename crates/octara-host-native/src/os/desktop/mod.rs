//! Portable desktop window integration for Windows and Linux.

mod ime;
#[cfg(target_os = "linux")]
mod linux_clipboard;
#[cfg(target_os = "linux")]
mod linux_preferences;
mod services;
#[cfg(target_os = "windows")]
mod windows_preferences;

use super::gestures::TrackpadGestures;
use crate::ShellEvent;
use crate::os::{ActiveEditorView, OsHost, OsWindow, OsWindowContext, ServiceCleanup, WindowTextInput};
use ime::WinitIme;
use octara::module::{HostSurfaceKind, RawNativeSurface};
use octara::runtime::Input;
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceId, ServiceRequest};
use octara_model::geometry::Point;
use octara_model::input::{EditorEvent, GestureEvent, HostReply, Modifiers};
use octara_model::paint::Image;
use serde_json::Value;
use services::{Clipboard, Services};
use std::rc::Rc;
use std::sync::Arc;
use winit::event::{Ime, WindowEvent};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

#[cfg(target_os = "windows")]
pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Windows;
#[cfg(not(target_os = "windows"))]
pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Linux;

#[cfg(target_os = "windows")]
pub(super) fn raw_module_surface(window: &Window) -> Option<RawNativeSurface> {
    use std::ptr::NonNull;

    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else { return None };
    Some(RawNativeSurface::Win32 {
        hwnd: NonNull::new(handle.hwnd.get() as *mut _)?,
        hinstance: handle.hinstance.and_then(|instance| NonNull::new(instance.get() as *mut _)),
    })
}

#[cfg(not(target_os = "windows"))]
pub(super) fn raw_module_surface(window: &Window) -> Option<RawNativeSurface> {
    use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

    let handles = (window.window_handle().ok()?.as_raw(), window.display_handle().ok()?.as_raw());
    match handles {
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

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(DesktopHost {
        clipboard: Rc::new(Clipboard::new()),
        #[cfg(target_os = "linux")]
        preferences: None,
    })
}

struct DesktopHost {
    clipboard: Rc<Clipboard>,
    #[cfg(target_os = "linux")]
    preferences: Option<linux_preferences::Observer>,
}

impl OsHost for DesktopHost {
    fn follows_window_focus(&self) -> bool {
        true
    }

    #[cfg(target_os = "linux")]
    fn initialize(&mut self, proxy: winit::event_loop::EventLoopProxy<ShellEvent>) {
        self.preferences = linux_preferences::Observer::start(proxy).ok();
    }

    #[cfg(target_os = "linux")]
    fn locale(&self) -> Option<Arc<str>> {
        linux_preferences::locale()
    }

    #[cfg(target_os = "windows")]
    fn reduced_motion(&self) -> Option<bool> {
        windows_preferences::reduced_motion()
    }

    fn create_window(&mut self, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        #[cfg(target_os = "windows")]
        let preferences = windows_preferences::Observer::new(context.window, context.proxy.clone());
        let reply = move |reply| {
            let _ = context.proxy.send_event(ShellEvent::Input { window: context.id, input: Input::Host(reply) });
        };
        Ok(Box::new(DesktopWindow {
            #[cfg(target_os = "windows")]
            _preferences: preferences,
            window: context.window.clone(),
            ime: WinitIme::default(),
            gestures: TrackpadGestures::default(),
            services: Services::new(self.clipboard.clone(), reply),
        }))
    }

    fn open_url(&mut self, url: &str) {
        let _ = open::that_detached(url);
    }

    fn read_clipboard(&mut self, accept: ClipboardKinds) -> Option<Vec<ClipboardItem>> {
        self.clipboard.with_native(|native| services::read_clipboard(native, accept))?.ok()
    }

    fn write_clipboard(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        self.clipboard.write(items)
    }

    fn write_clipboard_image(&mut self, image: &Image) {
        let frame = image.frame();
        let data = arboard::ImageData { width: frame.width as usize, height: frame.height as usize, bytes: (&*frame.rgba).into() };
        let _ = self.clipboard.with_native(|clipboard| clipboard.set_image(data));
    }
}

struct DesktopWindow {
    #[cfg(target_os = "windows")]
    _preferences: Option<windows_preferences::Observer>,
    window: Arc<Window>,
    ime: WinitIme,
    gestures: TrackpadGestures,
    services: Services,
}

impl OsWindow for DesktopWindow {
    fn on_winit_gesture(&mut self, event: &WindowEvent, anchor: Option<Point>, modifiers: Modifiers) -> Option<GestureEvent> {
        self.gestures.update(event, anchor, modifiers)
    }

    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        self
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        self.services.request(id, request)
    }
}

impl WindowTextInput for DesktopWindow {
    fn sync_text_input(&mut self, editor: Option<&ActiveEditorView<'_>>) {
        self.ime.sync(&self.window, editor);
    }

    fn on_winit_ime(&mut self, event: &Ime) -> Option<EditorEvent> {
        self.ime.event(event)
    }

    fn inspect_text_input(&self) -> Value {
        self.ime.inspect(&self.window)
    }
}

type Reply = Arc<dyn Fn(HostReply) + Send + Sync>;
