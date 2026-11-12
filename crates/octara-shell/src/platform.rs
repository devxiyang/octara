//! The winit backend's side of the platform layer ([`octara::platform`]): the system's clipboard
//! and the event loop's waker, and one winit window's native views and text input.

use super::ShellEvent;
use octara::native::NativeHost;
use octara::platform::{Platform, PlatformWindow, TextInput};
use std::any::Any;
use std::sync::Arc;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

/// The desktop the app runs on: arboard's clipboard, the event loop's proxy as the waker.
pub(crate) struct WinitPlatform {
    clipboard: Option<arboard::Clipboard>,
    proxy: EventLoopProxy<ShellEvent>,
}

impl WinitPlatform {
    pub(crate) fn new(proxy: EventLoopProxy<ShellEvent>) -> Self {
        WinitPlatform { clipboard: arboard::Clipboard::new().ok(), proxy }
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

/// One winit window: its native views (macOS: AppKit's; elsewhere placeholders, as headless)
/// and the input method.
pub(crate) struct WinitWindow {
    window: Arc<Window>,
    host: Box<dyn NativeHost>,
    /// A secure field has focus: the input method is off and (macOS) secure input on.
    secure: bool,
}

impl WinitWindow {
    pub(crate) fn new(window: Arc<Window>, host: Box<dyn NativeHost>) -> Self {
        WinitWindow { window, host, secure: false }
    }
}

impl PlatformWindow for WinitWindow {
    fn native_host(&self) -> &dyn NativeHost {
        &*self.host
    }
    fn native_host_mut(&mut self) -> &mut dyn NativeHost {
        &mut *self.host
    }
    /// The input method's candidate window follows the caret; a secure field turns the input
    /// method off and the platform's secure input on.
    fn set_text_input(&mut self, input: Option<&TextInput>) {
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
    fn as_any(&self) -> &dyn Any {
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
