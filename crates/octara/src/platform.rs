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
    /// Opens window `id` as `options` asks (the id names it in the input it sends back).
    fn open_window(&mut self, id: crate::app::WindowId, options: &WindowOptions) -> Result<Box<dyn PlatformWindow>, String>;
    fn as_any(&self) -> &dyn Any;
}

/// What kind of window: a document or tool window, or a dialog over its parent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    #[default]
    Normal,
    Dialog,
}

/// A window to open.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct WindowOptions {
    pub title: String,
    /// Its content size in logical pixels.
    pub size: (f32, f32),
    pub kind: WindowKind,
}

impl Default for WindowOptions {
    fn default() -> Self {
        WindowOptions { title: String::new(), size: (800.0, 600.0), kind: WindowKind::Normal }
    }
}

impl WindowOptions {
    pub fn new(title: impl Into<String>) -> Self {
        WindowOptions { title: title.into(), ..WindowOptions::default() }
    }
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = (width, height);
        self
    }
    pub fn kind(mut self, kind: WindowKind) -> Self {
        self.kind = kind;
        self
    }
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

/// The platform of headless runs: a clipboard in memory, no waker; its windows are
/// [`HeadlessWindow`]s.
#[derive(Default)]
pub struct HeadlessPlatform {
    pub clipboard: Option<String>,
    /// The native host the next window opened gets (a test's fake platform views).
    pub next_host: Option<Box<dyn NativeHost>>,
}

impl HeadlessPlatform {
    /// A headless platform whose next window's native views go to `host`.
    pub fn with_host(host: impl NativeHost + 'static) -> HeadlessPlatform {
        HeadlessPlatform { clipboard: None, next_host: Some(Box::new(host)) }
    }
}

impl Platform for HeadlessPlatform {
    fn read_text(&mut self) -> Option<String> {
        self.clipboard.clone()
    }
    fn write_text(&mut self, text: &str) {
        self.clipboard = Some(text.to_owned());
    }
    fn open_window(&mut self, _: crate::app::WindowId, _: &WindowOptions) -> Result<Box<dyn PlatformWindow>, String> {
        Ok(Box::new(match self.next_host.take() {
            Some(host) => HeadlessWindow { host, text_input: None },
            None => HeadlessWindow::default(),
        }))
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
