//! The platform layer ([design](../../../docs/design/platform.md)): what an app needs from the
//! system it runs on, as traits a backend implements. The app and the kernel never see a
//! windowing library's types; headless runs (tests, agents, `octara run`) implement the same
//! traits with [`HeadlessPlatform`] and [`HeadlessWindow`].
//!
//! A backend drives the app with its input methods ([`App::pointer`](crate::app::App::pointer),
//! [`App::key_event`](crate::app::App::key_event), …) and draws its display list; the app
//! calls back through these traits for the clipboard, waking the loop, native views and the
//! state of text input.

use crate::native::{NativeHost, RecordingHost};
use crate::paint::Rect;
use std::any::Any;
use std::sync::Arc;

/// The system an app runs on (one per app).
pub trait Platform {
    /// The clipboard's text, if it holds text.
    fn read_text(&mut self) -> Option<String>;
    fn write_text(&mut self, text: &str);
    /// What wakes the event loop from another thread (background work finished, a channel
    /// sent); the app then [polls](crate::app::App::poll). `None` headless, where the app is
    /// driven step by step.
    fn waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        None
    }
    fn as_any(&self) -> &dyn Any;
}

/// One window on that system.
pub trait PlatformWindow {
    /// Where the window's native views live.
    fn native_host(&self) -> &dyn NativeHost;
    fn native_host_mut(&mut self) -> &mut dyn NativeHost;
    /// The focused text field changed (or none has focus): where its caret is, so the input
    /// method's window opens next to it, and whether it is secure (the input method off and,
    /// on macOS, secure keyboard input on) or multi-line. Sent when it changes, after a frame.
    fn set_text_input(&mut self, input: Option<&TextInput>);
    fn as_any(&self) -> &dyn Any;
}

/// The focused text field, as the platform's text input sees it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextInput {
    /// The caret's rect in logical window coordinates.
    pub caret: Rect,
    pub secure: bool,
    pub multiline: bool,
}

/// The platform of headless runs: a clipboard in memory, no waker.
#[derive(Default, Debug)]
pub struct HeadlessPlatform {
    pub clipboard: Option<String>,
}

impl Platform for HeadlessPlatform {
    fn read_text(&mut self) -> Option<String> {
        self.clipboard.clone()
    }
    fn write_text(&mut self, text: &str) {
        self.clipboard = Some(text.to_owned());
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The window of headless runs: native views in a [`RecordingHost`] (or the host given), the
/// text input state kept for tests.
pub struct HeadlessWindow {
    pub host: Box<dyn NativeHost>,
    pub text_input: Option<TextInput>,
}

impl Default for HeadlessWindow {
    fn default() -> Self {
        HeadlessWindow { host: Box::<RecordingHost>::default(), text_input: None }
    }
}

impl HeadlessWindow {
    /// A headless window whose native views go to `host` (a test's fake platform views).
    pub fn with_host(host: impl NativeHost + 'static) -> HeadlessWindow {
        HeadlessWindow { host: Box::new(host), text_input: None }
    }
}

impl PlatformWindow for HeadlessWindow {
    fn native_host(&self) -> &dyn NativeHost {
        &*self.host
    }
    fn native_host_mut(&mut self) -> &mut dyn NativeHost {
        &mut *self.host
    }
    fn set_text_input(&mut self, input: Option<&TextInput>) {
        self.text_input = input.copied();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
