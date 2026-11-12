//! The winit backend's side of the platform layer ([`octara::platform`]): the system's clipboard
//! and the event loop's waker, and one winit window's native views and text input.

use super::ShellEvent;
use octara::platform::{Frame, Platform, PlatformWindow, TextInput};
use std::any::Any;
use std::sync::Arc;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

/// The desktop the app runs on: arboard's clipboard, the event loop's proxy as the waker, and
/// the window the shell made (winit makes windows inside its loop; opening more comes with the
/// backend's several windows).
pub(crate) struct WinitPlatform {
    clipboard: Option<arboard::Clipboard>,
    proxy: EventLoopProxy<ShellEvent>,
    first: Option<WinitWindow>,
}

impl WinitPlatform {
    pub(crate) fn new(proxy: EventLoopProxy<ShellEvent>, first: WinitWindow) -> Self {
        WinitPlatform { clipboard: arboard::Clipboard::new().ok(), proxy, first: Some(first) }
    }
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
    fn open_window(&mut self, _: octara::app::WindowId, _: &octara::platform::WindowOptions) -> Result<Box<dyn PlatformWindow>, String> {
        let w = self.first.take().ok_or("this backend shows one window for now")?;
        Ok(Box::new(w))
    }
    fn waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let proxy = self.proxy.clone();
        Some(Arc::new(move || {
            let _ = proxy.send_event(ShellEvent::Wake);
        }))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// One winit window: its native views (macOS: AppKit's; elsewhere none, so they draw as
/// placeholders) and the input method.
pub(crate) struct WinitWindow {
    window: Arc<Window>,
    #[cfg(target_os = "macos")]
    host: Option<super::native_mac::Host>,
    /// A secure field has focus: the input method is off and (macOS) secure input on.
    secure: bool,
}

impl WinitWindow {
    pub(crate) fn new(window: Arc<Window>, #[cfg(target_os = "macos")] host: Option<super::native_mac::Host>) -> Self {
        WinitWindow {
            window,
            #[cfg(target_os = "macos")]
            host,
            secure: false,
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
        #[cfg(target_os = "macos")]
        if let Some(host) = &self.host {
            for change in frame.native {
                host.apply(change);
            }
        }
        self.text_input(frame.text_input.as_ref());
        if let Some(p) = &frame.picked {
            eprintln!("octara-shell: picked {} (on the clipboard)", p.text);
            println!("{}", octara::serde_json::to_string(p).expect("a pick serializes"));
        }
        if frame.redraw {
            self.window.request_redraw();
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

impl Drop for WinitWindow {
    fn drop(&mut self) {
        if self.secure {
            secure_input::set(false);
        }
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
