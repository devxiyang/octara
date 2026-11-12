//! The platform layer ([design](../../../docs/design/platform.md)): what an app needs from the
//! system it runs on, as traits a backend implements. The app and the kernel never see a
//! windowing library's types; headless runs (tests, agents, `octara run`) implement the same
//! traits with [`HeadlessPlatform`] and [`HeadlessWindow`].
//!
//! A backend sends its windows' input as [`InputEvent`](crate::app::InputEvent)s
//! ([`Window::event`](crate::app::Window::event)) and draws each window's display list; the app
//! presents a [`Frame`] to each platform window after every operation (native view changes, the
//! text input) and asks the platform for the clipboard, waking the loop and new windows.

use crate::native::NativeChange;
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
    /// Opens `url` with the system (the browser, the mail app).
    fn open_url(&mut self, url: &str) {
        let _ = url;
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

/// One window on that system. The app presents a [`Frame`] after each operation; the backend
/// draws the window's display list ([`Window::display_list`](crate::app::Window::display_list))
/// on its own pacing. Questions that need an answer are methods.
pub trait PlatformWindow {
    /// What changed since the last frame: apply native view changes (placements with the next
    /// present), follow the text input, draw again when asked.
    fn present(&mut self, frame: Frame);
    /// Whether the platform shows native views of `kind` in `slot` (else they draw as
    /// placeholders, and an underlay view gets no cutout).
    fn shows_native(&self, kind: &str, slot: crate::native::Slot) -> bool {
        let _ = (kind, slot);
        false
    }
    /// Takes a snapshot of view `id`, sent back as input (`NativeSnapshot`): `Ok` with what the
    /// snapshot cannot show (WebKit: hardware video and WebGL come out black), or `Err` with why
    /// this platform takes none.
    fn snapshot_native(&mut self, id: crate::native::NativeId) -> Result<Vec<String>, String> {
        let _ = id;
        Err("this platform takes no snapshots".into())
    }
    /// What view `id` is busy with (a page loading), so a capture waits for it.
    fn native_busy(&self, id: crate::native::NativeId) -> Option<String> {
        let _ = id;
        None
    }
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// How the pointer looks (GPUI's names).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorStyle {
    #[default]
    Arrow,
    /// Over text that can be selected or typed in.
    IBeam,
    /// Over a link.
    PointingHand,
    Crosshair,
    /// Over something that can be dragged.
    OpenHand,
    /// While dragging it.
    ClosedHand,
    ResizeLeftRight,
    ResizeUpDown,
    ResizeColumn,
    ResizeRow,
    OperationNotAllowed,
}

/// What a window's platform is told after an operation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// Native views created, updated, placed and destroyed, in order.
    pub native: Vec<NativeChange>,
    /// The focused text field (none: no field has focus): where its caret is, so the input
    /// method's window opens next to it, and whether it is secure (the input method off and, on
    /// macOS, secure keyboard input on) or multi-line.
    pub text_input: Option<TextInput>,
    /// What the window shows changed or moves: draw again.
    pub redraw: bool,
    /// How the pointer looks now (over the element it is on).
    pub cursor: CursorStyle,
    /// The element a person picked ([`Window::set_picking`](crate::app::Window::set_picking)),
    /// its line already on the clipboard: a backend may print or show it.
    pub picked: Option<crate::inspect::Picked>,
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
    /// Every URL opened, in order.
    pub opened_urls: Vec<String>,
    /// The window the next one opened is (a test's fake platform views).
    pub next_window: Option<HeadlessWindow>,
}

impl HeadlessPlatform {
    /// A headless platform whose next window is `window`.
    pub fn with_window(window: HeadlessWindow) -> HeadlessPlatform {
        HeadlessPlatform { next_window: Some(window), ..HeadlessPlatform::default() }
    }
}

impl Platform for HeadlessPlatform {
    fn read_text(&mut self) -> Option<String> {
        self.clipboard.clone()
    }
    fn open_url(&mut self, url: &str) {
        self.opened_urls.push(url.to_owned());
    }
    fn write_text(&mut self, text: &str) {
        self.clipboard = Some(text.to_owned());
    }
    fn open_window(&mut self, _: crate::app::WindowId, _: &WindowOptions) -> Result<Box<dyn PlatformWindow>, String> {
        Ok(Box::new(self.next_window.take().unwrap_or_default()))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The window of headless runs: keeps what it was presented (native changes, the text input,
/// how many frames) for tests. By default it shows no native view (they draw as placeholders);
/// a test can pretend it does, take snapshots, or be busy.
#[derive(Default, Debug)]
pub struct HeadlessWindow {
    /// Every native change presented, in order.
    pub native: Vec<NativeChange>,
    /// The last text input state presented.
    pub text_input: Option<TextInput>,
    /// Frames presented.
    pub frames: u64,
    /// Pretend the platform shows every native view.
    pub shows_native: bool,
    /// Pretend snapshots are taken, missing these (`None`: no snapshots).
    pub snapshots: Option<Vec<String>>,
    /// Pretend every native view is busy with this.
    pub busy: Option<String>,
    /// Every element picked, in order.
    pub picked: Vec<crate::inspect::Picked>,
    /// How the pointer looks.
    pub cursor: CursorStyle,
}

impl PlatformWindow for HeadlessWindow {
    fn present(&mut self, frame: Frame) {
        self.native.extend(frame.native);
        self.text_input = frame.text_input;
        self.cursor = frame.cursor;
        self.picked.extend(frame.picked);
        self.frames += 1;
    }
    fn shows_native(&self, _: &str, _: crate::native::Slot) -> bool {
        self.shows_native
    }
    fn snapshot_native(&mut self, _: crate::native::NativeId) -> Result<Vec<String>, String> {
        self.snapshots.clone().ok_or_else(|| "headless: no platform view to take a snapshot of".into())
    }
    fn native_busy(&self, _: crate::native::NativeId) -> Option<String> {
        self.busy.clone()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
