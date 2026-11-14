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
    pub document: Option<crate::app::TextDocument>,
    /// Displayed text geometry from this frame; omitted with a secure document.
    pub geometry: Option<TextGeometry>,
    /// Frozen field configuration for laying out newer native text before its queued edit
    /// reaches the app. It owns no window or application callback; secure fields omit it.
    pub layout: Option<TextInputLayout>,
}

/// A focused field's published layout configuration. Native text queries can shape their
/// current document with the same fonts, wrapping and caret scrolling as the field, without
/// reentering a window. Only the shared text engine's bounded caches may change.
#[derive(Clone)]
pub struct TextInputLayout {
    pub(crate) resources: std::rc::Rc<crate::kernel::Resources>,
    pub(crate) style: crate::kernel::text::TextStyle,
    pub(crate) content: Rect,
    pub(crate) multiline: bool,
    pub(crate) scroll: f32,
    pub(crate) kept: Option<(usize, usize)>,
    pub(crate) fonts: u64,
}

impl PartialEq for TextInputLayout {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.resources, &other.resources)
            && self.style == other.style
            && self.content == other.content
            && self.multiline == other.multiline
            && self.scroll == other.scroll
            && self.kept == other.kept
            && self.fonts == other.fonts
    }
}

impl std::fmt::Debug for TextInputLayout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextInputLayout")
            .field("style", &self.style)
            .field("content", &self.content)
            .field("multiline", &self.multiline)
            .field("scroll", &self.scroll)
            .field("kept", &self.kept)
            .field("fonts", &self.fonts)
            .finish_non_exhaustive()
    }
}

/// An owned layout for platform text queries, with displayed UTF-8 byte offsets (including
/// marked text). A host may use it only while its document text still matches this frame.
#[derive(Clone, PartialEq, Debug)]
pub struct TextGeometry {
    /// The text's origin in logical window coordinates, including its current scroll.
    pub origin: (f32, f32),
    pub lines: Vec<crate::kernel::text::VisualLine>,
}

impl TextGeometry {
    /// The first line fragment of a displayed range: its actual byte range and logical
    /// window rect. The actual range expands to available caret stops. An insertion point
    /// has zero width; the host converts the rect to screen coordinates.
    pub fn first_rect(&self, range: std::ops::Range<usize>) -> Option<(std::ops::Range<usize>, Rect)> {
        let last = self.lines.last()?.end;
        let start = range.start.min(range.end).min(last);
        let end = range.start.max(range.end).min(last);
        let index = self.lines.iter().rposition(|l| l.start <= start).unwrap_or(0);
        let line = &self.lines[index];
        let (s, x) = line.stops.iter().rev().find(|(b, _)| *b <= start).or(line.stops.first()).copied()?;
        let (e, right) =
            if start == end { (s, x) } else { line.stops.iter().find(|(b, _)| *b >= end.min(line.end)).or(line.stops.last()).copied()? };
        // Line ranges omit their newline. Include that gap in a nonempty actual range so
        // a caller iterating line fragments advances even across an otherwise empty line.
        let e = if start != end && end > line.end {
            self.lines.get(index + 1).filter(|next| next.start > line.end).map_or(e, |next| e.max(end.min(next.start)))
        } else {
            e
        };
        Some((s..e, Rect { x: self.origin.0 + x.min(right), y: self.origin.1 + line.top, w: (right - x).abs(), h: line.height }))
    }

    /// The displayed byte index nearest a point in logical window coordinates. Unlike an
    /// editing caret's value offset, indices inside marked text are kept distinct.
    pub fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        let line = self.lines.iter().rev().find(|l| l.top <= y - self.origin.1).or(self.lines.first())?;
        let x = x - self.origin.0;
        line.stops.iter().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map(|s| s.0)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::text::VisualLine;

    #[test]
    fn native_layout_handles_a_busy_engine_and_an_empty_document() {
        let layout = TextInputLayout {
            resources: std::rc::Rc::new(crate::kernel::Resources::default()),
            style: crate::kernel::text::TextStyle::default(),
            content: Rect { x: 10.0, y: 20.0, w: 80.0, h: 24.0 },
            multiline: false,
            scroll: 0.0,
            kept: None,
            fonts: 0,
        };
        let document = crate::app::TextDocument::new("nihao".into(), 5, 5);
        let engine = layout.resources.text.borrow_mut();
        assert!(layout.layout(&document).is_none(), "a reentrant native query must not panic or enter the app");
        let empty = layout.layout(&crate::app::TextDocument::new(String::new(), 0, 0)).unwrap();
        assert_eq!(empty.first_rect(0..0).unwrap(), (0..0, Rect { x: 10.0, y: 20.0, w: 0.0, h: 24.0 }));
        drop(engine);
        assert!(layout.layout(&document).is_some());
    }

    #[test]
    fn actual_text_rect_ranges_advance_across_newlines_and_empty_lines() {
        let geometry = TextGeometry {
            origin: (10.0, 20.0),
            lines: vec![
                VisualLine { start: 0, end: 1, top: 0.0, height: 10.0, stops: vec![(0, 0.0), (1, 8.0)] },
                VisualLine { start: 2, end: 2, top: 10.0, height: 10.0, stops: vec![(2, 0.0)] },
                VisualLine { start: 3, end: 4, top: 20.0, height: 10.0, stops: vec![(3, 0.0), (4, 8.0)] },
            ],
        };
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
        assert_eq!((newline.x, newline.y, newline.w), (18.0, 20.0, 0.0));
        assert_eq!(geometry.first_rect(1..1).unwrap().0, 1..1, "an insertion-point query remains empty");
    }

    #[test]
    fn actual_text_rect_ranges_expand_to_complete_surrogates_and_graphemes() {
        let text = "A😀e\u{301}B";
        let geometry = TextGeometry {
            origin: (0.0, 0.0),
            lines: vec![VisualLine {
                start: 0,
                end: text.len(),
                top: 0.0,
                height: 10.0,
                stops: vec![(0, 0.0), (1, 8.0), (5, 20.0), (8, 28.0), (9, 36.0)],
            }],
        };
        let (actual, rect) = geometry.first_rect(2..3).unwrap();
        assert_eq!(actual, 1..5);
        assert_eq!(crate::app::utf16_offset(text, actual.start)..crate::app::utf16_offset(text, actual.end), 1..3);
        assert_eq!(rect.w, 12.0);
        assert_eq!(geometry.first_rect(6..7).unwrap().0, 5..8, "a combining mark shares the base character's rect");
        assert_eq!(geometry.first_rect(2..2).unwrap().0, 1..1, "a split-surrogate insertion point floors to the previous caret stop");
    }
}
