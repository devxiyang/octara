//! The platform layer ([design](../../../docs/design/platform.md)): what an app needs from the
//! system it runs on, as traits a backend implements. The app and the kernel never see a
//! windowing library's types; headless runs (tests, agents, `octara run`) implement the same
//! traits with [`HeadlessPlatform`] and [`HeadlessWindow`].
//!
//! A backend sends its windows' input as [`InputEvent`](crate::app::InputEvent)s
//! ([`Window::event`](crate::app::Window::event)) and draws each window's published frame. The app
//! presents a [`Frame`] to platform windows when their drawing, frame demand or native state
//! changes, and asks the platform for the clipboard, waking the loop and new windows.

use crate::capture::{PaintFrame, PaintOrigin, PaintReceipt, PaintSample, PaintSite};
use crate::kernel::{NodeId, TextGeometry, TextInputLayout};
use crate::native::{NativeChange, NativeId};
use crate::paint::Rect;
use octara_primitives::input::CursorStyle;
use rustc_hash::FxHashMap;
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The system an app runs on (one per app).
pub trait Platform {
    /// The clipboard's text, if it holds text.
    fn read_text(&mut self) -> Option<String>;
    fn write_text(&mut self, text: &str);
    /// The clipboard's image, if it holds one.
    fn read_image(&mut self) -> Option<crate::image::Image> {
        None
    }
    fn write_image(&mut self, image: &crate::image::Image) {
        let _ = image;
    }
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
    /// Shows the app's menus (a menu bar: on macOS between the app's menu and the Window menu),
    /// each item's state and keys filled in; a pick comes back as input
    /// (`InputEvent::Command`) to the active window.
    fn set_menus(&mut self, menus: &[crate::menu::Menu]) {
        let _ = menus;
    }
    /// Opens window `id` and reports its initial viewport (the id names it in the input it sends back).
    fn open_window(&mut self, id: crate::app::WindowId, options: &WindowOptions) -> Result<OpenedWindow, String>;
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
    /// Its requested content size in logical pixels.
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

/// The viewport reported by the host before the root view mounts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowViewport {
    /// Content size in logical pixels.
    pub size: (f32, f32),
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// Usable area for anchored overlays, in logical window coordinates.
    pub safe_area: Option<Rect>,
}

/// An opened platform window and the viewport the App uses for its first layout.
pub struct OpenedWindow {
    pub window: Box<dyn PlatformWindow>,
    pub viewport: WindowViewport,
}

/// One snapshot request issued to a platform window. Echo it in the completion event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NativeSnapshotRequest {
    /// The target native view.
    pub id: NativeId,
    // Older recordings contain only the view ID. Zero names no issued request.
    #[serde(default)]
    token: u64,
}

impl NativeSnapshotRequest {
    pub(crate) fn new(id: NativeId) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let token = NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1)).expect("snapshot identity exhausted");
        Self { id, token }
    }
}

/// One window on that system. The app hands over changes after settling an operation. The backend
/// draws the [latest settled drawing](crate::app::Window::published_frame) at its own pace.
pub trait PlatformWindow {
    /// Consume each handoff in order. Apply native and input changes and record drawing demand.
    /// Synchronize staged native placements with the corresponding drawing when presenting it.
    /// A redraw can be coalesced with a newer drawing after the handoff returns.
    fn present(&mut self, frame: Frame);
    /// Whether the platform shows `ask` itself (a native menu, alert or file dialog); else the
    /// app draws menus and prompts in the window, and other requests wait for an answer.
    fn shows_ask(&self, ask: &crate::ask::Ask) -> bool {
        let _ = ask;
        false
    }
    /// Whether the platform shows native views of `kind` in `slot` (else they draw as
    /// placeholders, and an underlay view gets no cutout).
    fn shows_native(&self, kind: &str, slot: crate::native::Slot) -> bool {
        let _ = (kind, slot);
        false
    }
    /// Requests a snapshot. `Ok` lists what the snapshot cannot show (WebKit: hardware video
    /// and WebGL). Complete it asynchronously with `InputEvent::NativeSnapshot`, echoing
    /// `request` and providing an image if available. `Err` explains why no request started.
    fn snapshot_native(&mut self, request: NativeSnapshotRequest) -> Result<Vec<String>, String> {
        let _ = request;
        Err("this platform takes no snapshots".into())
    }
    /// What view `id` is busy with (a page loading), so a capture waits for it.
    fn native_busy(&self, id: crate::native::NativeId) -> Option<String> {
        let _ = id;
        None
    }
    fn as_any(&self) -> &dyn Any;
}

/// Where a platform's answer goes (from any thread).
pub type Reply<T> = Box<dyn FnOnce(T) + Send>;

/// What the clipboard holds: text, an image, or both (a copied picture with its description).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipboardItem {
    text: Option<String>,
    image: Option<crate::image::Image>,
}

impl ClipboardItem {
    pub fn new_string(text: impl Into<String>) -> ClipboardItem {
        ClipboardItem { text: Some(text.into()), image: None }
    }
    pub fn new_image(image: &crate::image::Image) -> ClipboardItem {
        ClipboardItem { text: None, image: Some(image.clone()) }
    }
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    pub fn image(&self) -> Option<&crate::image::Image> {
        self.image.as_ref()
    }
    pub(crate) fn parts(text: Option<String>, image: Option<crate::image::Image>) -> Option<ClipboardItem> {
        (text.is_some() || image.is_some()).then_some(ClipboardItem { text, image })
    }
}

impl From<&str> for ClipboardItem {
    fn from(s: &str) -> Self {
        ClipboardItem::new_string(s)
    }
}
impl From<String> for ClipboardItem {
    fn from(s: String) -> Self {
        ClipboardItem::new_string(s)
    }
}
impl From<&String> for ClipboardItem {
    fn from(s: &String) -> Self {
        ClipboardItem::new_string(s.clone())
    }
}
impl From<crate::image::Image> for ClipboardItem {
    fn from(i: crate::image::Image) -> Self {
        ClipboardItem { text: None, image: Some(i) }
    }
}

/// Ordered window updates and drawing demand from a settled app operation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// Native views created, updated, placed and destroyed, in order.
    pub native: Vec<NativeChange>,
    /// The focused text field (none: no field has focus): where its caret is, so the input
    /// method's window opens next to it, and whether it is secure (the input method off and, on
    /// macOS, secure keyboard input on) or multi-line.
    pub text_input: Option<TextInput>,
    /// A new drawing will be available once the handoff commits.
    pub redraw: bool,
    /// The window needs display callbacks while its motion or scrolling continues.
    pub continuous: bool,
    /// How the pointer looks now (over the element it is on).
    pub cursor: CursorStyle,
    /// Requests opened since the last frame that the platform shows ([`shows_ask`]).
    ///
    /// [`shows_ask`]: PlatformWindow::shows_ask
    pub asks: Vec<(crate::ask::AskId, crate::ask::Ask)>,
    /// Requests shown that the app no longer waits for (answered otherwise, or withdrawn): the
    /// platform closes them.
    pub withdrawn: Vec<crate::ask::AskId>,
    /// The element a person picked ([`Window::set_picking`](crate::app::Window::set_picking)),
    /// its line already on the clipboard: a backend may print or show it.
    pub picked: Option<crate::inspect::Picked>,
}

/// A settled window drawing with its sampled resources and paint attribution. Hosts can retain
/// it across later app operations and hand it to another thread to render.
#[derive(Clone, Debug)]
pub struct PublishedFrame {
    pub window: crate::app::WindowId,
    /// A drawing revision local to this window, increasing when it publishes changed content.
    pub revision: u64,
    /// Paint sample time in the app's time coordinates. Display hosts can sample analytic
    /// paint motion at their target time while the app is still at its settled time.
    pub sampled_at: Duration,
    /// Device pixels per logical pixel at preparation.
    pub scale: f32,
    /// Drawing data in logical pixels, including the viewport size.
    pub list: Arc<crate::paint::DisplayList>,
    pub(crate) paint_sites: Arc<FxHashMap<NodeId, PaintSite>>,
}

const _: fn() = || {
    fn require_owned_render_data<T: Send + Sync + 'static>() {}
    require_owned_render_data::<PublishedFrame>();
};

impl PublishedFrame {
    pub fn paint_sample(&self) -> PaintSample {
        PaintSample { revision: self.revision, sampled_at: self.sampled_at }
    }

    /// Attributes receipts from this drawing using its sampled source metadata. A host can
    /// retain this drawing through submission and publish the evidence once submission succeeds.
    pub fn paint_frame(&self, origin: PaintOrigin, receipts: &[PaintReceipt]) -> PaintFrame {
        PaintFrame::from_receipts(self.window, self.paint_sample(), origin, &self.paint_sites, receipts)
    }
}

/// The focused text field, as the platform's text input sees it.
#[derive(Clone, PartialEq, Debug)]
pub struct TextInput {
    /// The focused field's allocation in this window. A host retires its input-method context
    /// when this changes, even if the next field accepts the same kind of input.
    pub id: crate::kernel::NodeId,
    /// The caret's rect in logical window coordinates.
    pub caret: Rect,
    pub secure: bool,
    pub multiline: bool,
    /// Owned editing state for synchronous platform queries. Secure fields omit the entire
    /// document, including their length and selection.
    pub document: Option<crate::kernel::TextDocument>,
    /// Displayed text geometry from this frame; omitted with a secure document.
    pub geometry: Option<TextGeometry>,
    /// Frozen field configuration for laying out newer native text before its queued edit
    /// reaches the app. It owns no window or application callback; secure fields omit it.
    pub layout: Option<TextInputLayout>,
}

/// The platform of headless runs: a clipboard in memory, no waker; its windows are
/// [`HeadlessWindow`]s.
#[derive(Default)]
pub struct HeadlessPlatform {
    pub clipboard: Option<String>,
    /// The clipboard's image (writing text clears it, and writing an image clears the text).
    pub clipboard_image: Option<crate::image::Image>,
    /// Every URL opened, in order.
    pub opened_urls: Vec<String>,
    /// The menus last shown.
    pub menus: Vec<crate::menu::Menu>,
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
    fn set_menus(&mut self, menus: &[crate::menu::Menu]) {
        self.menus = menus.to_vec();
    }
    fn read_image(&mut self) -> Option<crate::image::Image> {
        self.clipboard_image.clone()
    }
    fn write_image(&mut self, image: &crate::image::Image) {
        self.clipboard = None;
        self.clipboard_image = Some(image.clone());
    }
    fn write_text(&mut self, text: &str) {
        self.clipboard = Some(text.to_owned());
        self.clipboard_image = None;
    }
    fn open_window(&mut self, _: crate::app::WindowId, options: &WindowOptions) -> Result<OpenedWindow, String> {
        Ok(OpenedWindow {
            window: Box::new(self.next_window.take().unwrap_or_default()),
            viewport: WindowViewport { size: options.size, scale: 1.0, safe_area: None },
        })
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
    /// Every snapshot request received, in order.
    pub snapshot_requests: Vec<NativeSnapshotRequest>,
    /// Pretend every native view is busy with this. Keep a clone before opening the window
    /// to simulate native work starting or completing without borrowing the host mutably.
    pub busy: std::rc::Rc<std::cell::RefCell<Option<String>>>,
    /// Every element picked, in order.
    pub picked: Vec<crate::inspect::Picked>,
    /// How the pointer looks.
    pub cursor: CursorStyle,
    /// Pretend the platform shows every request (it waits for an answer as input).
    pub shows_asks: bool,
    /// Every request presented, and those withdrawn.
    pub asks: Vec<(crate::ask::AskId, crate::ask::Ask)>,
    pub withdrawn: Vec<crate::ask::AskId>,
}

impl PlatformWindow for HeadlessWindow {
    fn present(&mut self, frame: Frame) {
        self.native.extend(frame.native);
        self.text_input = frame.text_input;
        self.cursor = frame.cursor;
        self.asks.extend(frame.asks);
        self.withdrawn.extend(frame.withdrawn);
        self.picked.extend(frame.picked);
        self.frames += 1;
    }
    fn shows_ask(&self, _: &crate::ask::Ask) -> bool {
        self.shows_asks
    }
    fn shows_native(&self, _: &str, _: crate::native::Slot) -> bool {
        self.shows_native
    }
    fn snapshot_native(&mut self, request: NativeSnapshotRequest) -> Result<Vec<String>, String> {
        self.snapshot_requests.push(request);
        self.snapshots.clone().ok_or_else(|| "headless: no platform view to take a snapshot of".into())
    }
    fn native_busy(&self, _: crate::native::NativeId) -> Option<String> {
        self.busy.borrow().clone()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_layout(multiline: bool) -> TextInputLayout {
        let resources = std::rc::Rc::new(crate::kernel::Resources::default());
        *resources.text.borrow_mut() = Box::new(octara_text::MonoEngine);
        TextInputLayout {
            resources,
            style: octara_text::TextStyle::default(),
            content: Rect { x: 10.0, y: 20.0, w: 80.0, h: 24.0 },
            multiline,
            scroll: 0.0,
            kept: None,
            fonts: 0,
        }
    }

    #[test]
    fn native_layout_handles_a_busy_engine_and_an_empty_document() {
        let layout = text_layout(false);
        let document = crate::kernel::TextDocument::new("nihao".into(), 5, 5);
        let engine = layout.resources.text.borrow_mut();
        assert!(layout.layout(&document).is_none(), "a reentrant native query must not panic or enter the app");
        let empty = layout.layout(&crate::kernel::TextDocument::new(String::new(), 0, 0)).unwrap();
        assert_eq!(empty.first_rect(0..0).unwrap(), (0..0, Rect { x: 10.0, y: 20.0, w: 0.0, h: 24.0 }));
        drop(engine);
        assert!(layout.layout(&document).is_some());
    }

    #[test]
    fn actual_text_rect_ranges_advance_across_newlines_and_empty_lines() {
        let geometry = text_layout(true).layout(&crate::kernel::TextDocument::new("A\n\nB".into(), 0, 0)).unwrap();
        let mut ranges = Vec::new();
        let mut start = 0;
        while start < 4 {
            let (actual, _) = geometry.first_rect(start..4).unwrap();
            assert!(actual.end > start, "a nonempty line-fragment query must make progress");
            start = actual.end;
            ranges.push(actual);
        }
        assert_eq!(ranges, [0..2, 2..3, 3..4]);
        let (actual, newline) = geometry.first_rect(1..2).unwrap();
        assert_eq!(actual, 1..2);
        assert_eq!((newline.x, newline.y, newline.w), (18.0, 20.0, 6.0));
        assert_eq!(geometry.first_rect(1..1).unwrap().0, 1..1, "an insertion-point query remains empty");
    }

    #[test]
    fn actual_text_rect_ranges_expand_to_complete_surrogates_and_graphemes() {
        let text = "A😀e\u{301}B";
        let geometry = text_layout(false).layout(&crate::kernel::TextDocument::new(text.into(), 0, 0)).unwrap();
        let (actual, rect) = geometry.first_rect(2..3).unwrap();
        assert_eq!(actual, 1..5);
        assert_eq!(crate::app::utf16_offset(text, actual.start)..crate::app::utf16_offset(text, actual.end), 1..3);
        assert_eq!(rect.w, 8.0);
        assert_eq!(geometry.first_rect(6..7).unwrap().0, 5..8, "a combining mark shares the base character's rect");
        assert_eq!(geometry.first_rect(2..2).unwrap().0, 1..1, "a split-surrogate insertion point floors to the previous caret stop");
    }
}
