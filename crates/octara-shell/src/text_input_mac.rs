//! The focused Octara field as an AppKit text document. AppKit queries and edits a local
//! mirror synchronously; application handlers run later, when the shell drains the edits.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, Sel};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{NSEvent, NSTextInputClient, NSTextInputContext, NSView};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString,
};
use octara::app::{TextDocument, TextEdit};
use octara::kernel::NodeId;
use octara::platform::{TextGeometry, TextInput, TextInputLayout};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::Range;
use winit::platform::scancode::PhysicalKeyExtScancode;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// The expected document prevents an old native edit from overwriting a newer app edit.
pub(crate) struct NativeEdit {
    pub(crate) owner: NodeId,
    pub(crate) expected: TextDocument,
    pub(crate) edit: TextEdit,
    epoch: u64,
}

/// Retaining the original event lets the shell route application key handlers before asking
/// AppKit to insert ordinary text. winit can dispatch that route inline from the parent's
/// keyDown; native text callbacks themselves only update the owned document and queue edits.
pub(crate) struct NativeKey {
    event: Retained<NSEvent>,
    owner: Option<NodeId>,
    epoch: u64,
}

struct Session {
    owner: NodeId,
    document: TextDocument,
    geometry: Option<TextGeometry>,
    geometry_document: TextDocument,
    layout: Option<TextInputLayout>,
    geometry_status: &'static str,
}

impl Session {
    fn update_frame(&mut self, input: &TextInput, document: &TextDocument) {
        // An intermediate application frame may still describe an earlier native edit.
        // Keep the fresh native layout while both its complete document and recipe match.
        let keep =
            self.layout == input.layout && self.geometry.is_some() && self.geometry_document == self.document && self.document != *document;
        self.layout = input.layout.clone();
        if !keep {
            self.geometry = input.geometry.clone();
            self.geometry_document = document.clone();
            self.geometry_status = "published";
        }
    }

    fn geometry(&mut self) -> Result<&TextGeometry, &'static str> {
        if self.geometry.is_none() || self.geometry_document != self.document {
            let Some(layout) = &self.layout else {
                self.geometry_status = "unavailable";
                return Err("missing_layout");
            };
            let Some(geometry) = layout.layout(&self.document) else {
                self.geometry_status = "busy";
                return Err("layout_busy");
            };
            self.geometry = Some(geometry);
            self.geometry_document = self.document.clone();
            self.geometry_status = "recomputed";
        }
        self.geometry.as_ref().ok_or("missing_geometry")
    }
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    latest: Option<TextInput>,
    pending: Vec<NativeEdit>,
    inflight: bool,
    epoch: u64,
    notified: Option<TextDocument>,
}

impl State {
    fn should_retire(&self) -> bool {
        let input = self.latest.as_ref().filter(|i| i.document.is_some());
        if self.session.as_ref().map(|s| s.owner) != input.map(|i| i.id) {
            return true;
        }
        // Outside a native batch, a changed draft or selection came from the application.
        // Retire IMKit as well as the mirror so the next syllable starts a fresh composer.
        self.pending.is_empty()
            && !self.inflight
            && self
                .session
                .as_ref()
                .is_some_and(|s| s.document.marked().is_some() && input.and_then(|i| i.document.as_ref()) != Some(&s.document))
    }

    fn edit(&mut self, edit: TextEdit) -> bool {
        let Some(session) = &mut self.session else { return false };
        let expected = session.document.clone();
        if !session.document.apply(&edit) {
            return false;
        }
        self.pending.push(NativeEdit { owner: session.owner, expected, edit, epoch: self.epoch });
        true
    }

    fn synchronize(&mut self) {
        let Some(input) = &self.latest else {
            self.session = None;
            return;
        };
        let Some(document) = &input.document else {
            self.session = None;
            return;
        };
        self.session = Some(Session {
            owner: input.id,
            document: document.clone(),
            geometry: input.geometry.clone(),
            geometry_document: document.clone(),
            layout: input.layout.clone(),
            geometry_status: "published",
        });
    }

    fn selection_notification(&mut self) -> bool {
        let Some(session) = &self.session else { return false };
        if !self.pending.is_empty() || self.inflight || session.geometry.is_none() || session.geometry_document != session.document {
            return false;
        }
        let changed = self
            .notified
            .as_ref()
            .is_none_or(|old| old.text() != session.document.text() || old.selection() != session.document.selection());
        if changed {
            self.notified = Some(session.document.clone());
        }
        changed
    }
}

#[derive(Default)]
struct Queries {
    selected: Cell<u64>,
    marked: Cell<u64>,
    substring: Cell<u64>,
    first_rect: Cell<u64>,
    index: Cell<u64>,
    set_marked: Cell<u64>,
    insert: Cell<u64>,
    unmark: Cell<u64>,
}

fn increment(count: &Cell<u64>) {
    count.set(count.get().saturating_add(1));
}

struct InputIvars {
    state: RefCell<State>,
    wake: Box<dyn Fn()>,
    queries: Queries,
    handled: Cell<bool>,
    forward: Cell<bool>,
    discarding: Cell<bool>,
    in_key_down: Cell<bool>,
    last_interpret_inline: Cell<Option<bool>>,
    first_rect_request: Cell<Option<NSRange>>,
    first_rect_failure: Cell<Option<&'static str>>,
    keys: RefCell<VecDeque<NativeKey>>,
    anchor: Cell<Option<NSRect>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements. The view owns no application borrow.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraTextInput"]
    #[ivars = InputIvars]
    struct InputView;

    unsafe impl NSObjectProtocol for InputView {}

    impl InputView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> { None }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            // Forwarding can run a window-close handler before keyDown returns.
            let _keep_alive = self.retain();
            let ivars = self.ivars();
            let previous = ivars.in_key_down.replace(true);
            let key = {
                let state = ivars.state.borrow();
                NativeKey { event: event.retain(), owner: state.session.as_ref().map(|s| s.owner), epoch: state.epoch }
            };
            ivars.keys.borrow_mut().push_back(key);
            // winit's IME is disabled. It translates the physical key once and the shell
            // subsequently decides whether this retained event should enter AppKit.
            // SAFETY: this attached view and its parent are used only on the main thread.
            if let Some(parent) = unsafe { self.superview() } {
                parent.keyDown(event);
            }
            ivars.in_key_down.set(previous);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            // SAFETY: this attached view and its parent are used only on the main thread.
            if let Some(parent) = unsafe { self.superview() } {
                parent.keyUp(event);
            }
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            // SAFETY: this attached view and its parent are used only on the main thread.
            if let Some(parent) = unsafe { self.superview() } {
                parent.flagsChanged(event);
            }
        }
    }

    unsafe impl NSTextInputClient for InputView {
        #[unsafe(method(insertText:replacementRange:))]
        unsafe fn insert_text(&self, string: &AnyObject, replacement: NSRange) {
            increment(&self.ivars().queries.insert);
            let Some(text) = native_string(string) else { return };
            let replacement = self.ivars().state.borrow().session.as_ref()
                .and_then(|s| replacement_range(&s.document, replacement));
            self.enqueue(TextEdit::Insert { text, replacement });
        }

        #[unsafe(method(doCommandBySelector:))]
        unsafe fn do_command(&self, selector: Sel) {
            if selector.name().to_bytes() == b"cancelOperation:" && self.composing() {
                self.enqueue(TextEdit::Cancel);
                self.discard_composer();
                return;
            }
            // If an input method committed and then emitted a command (Enter is common),
            // the same key must not reach the application's submit handler as well.
            if !self.ivars().handled.get() {
                self.ivars().forward.set(true);
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        unsafe fn set_marked_text(&self, string: &AnyObject, selected: NSRange, replacement: NSRange) {
            increment(&self.ivars().queries.set_marked);
            let Some(text) = native_string(string) else { return };
            if text.is_empty() {
                self.enqueue(TextEdit::Cancel);
                return;
            }
            let selection = byte_range(&text, selected);
            // Both explicit replacement ranges address the displayed document. This matches
            // NSTextView on the host OS, including reconversion away from an existing mark.
            let replacement = self.ivars().state.borrow().session.as_ref()
                .and_then(|s| replacement_range(&s.document, replacement));
            self.enqueue(TextEdit::Mark { text, selection, replacement });
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            increment(&self.ivars().queries.unmark);
            self.enqueue(TextEdit::Unmark);
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            increment(&self.ivars().queries.selected);
            let state = self.ivars().state.borrow();
            state.session.as_ref().and_then(|s| s.document.selection().map(|r| native_range(s.document.text(), r)))
                .unwrap_or_else(not_found)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            increment(&self.ivars().queries.marked);
            let state = self.ivars().state.borrow();
            state.session.as_ref().and_then(|s| s.document.marked().map(|r| native_range(s.document.text(), r)))
                .unwrap_or_else(not_found)
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            self.ivars().state.borrow().session.as_ref().is_some_and(|s| s.document.marked().is_some())
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        unsafe fn substring(&self, proposed: NSRange, actual: NSRangePointer) -> Option<Retained<NSAttributedString>> {
            increment(&self.ivars().queries.substring);
            // SAFETY: AppKit provides a writable out pointer or null.
            unsafe { write_range(actual, not_found()) };
            self.substring_value(proposed).map(|(text, range)| {
                unsafe { write_range(actual, range) };
                NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(&text))
            })
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> { NSArray::new() }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        unsafe fn first_rect(&self, proposed: NSRange, actual: NSRangePointer) -> NSRect {
            increment(&self.ivars().queries.first_rect);
            self.ivars().first_rect_request.set(Some(proposed));
            self.ivars().first_rect_failure.set(None);
            unsafe { write_range(actual, not_found()) };
            let found = {
                let mut state = self.ivars().state.borrow_mut();
                state.session.as_mut().ok_or("no_session").and_then(|s| {
                    let range = query_range(s.document.text(), proposed).ok_or("invalid_range")?;
                    let (range, rect) = s.geometry()?.first_rect(range).ok_or("invalid_range")?;
                    Ok((native_range(s.document.text(), range), rect))
                })
            };
            let (range, rect) = match found {
                Ok(found) => found,
                Err(reason) => {
                    self.ivars().first_rect_failure.set(Some(reason));
                    return NSRect::ZERO;
                }
            };
            let Some(window) = self.window() else {
                self.ivars().first_rect_failure.set(Some("no_window"));
                return NSRect::ZERO;
            };
            let rect = NSRect::new(NSPoint::new(rect.x as f64, rect.y as f64), NSSize::new(rect.w as f64, rect.h as f64));
            let screen = window.convertRectToScreen(self.convertRect_toView(rect, None));
            self.ivars().anchor.set(Some(screen));
            unsafe { write_range(actual, range) };
            screen
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, point: NSPoint) -> usize {
            increment(&self.ivars().queries.index);
            let Some(window) = self.window() else { return NSNotFound as usize };
            let local = self.convertPoint_fromView(window.convertPointFromScreen(point), None);
            let mut state = self.ivars().state.borrow_mut();
            let Some(session) = &mut state.session else { return NSNotFound as usize };
            let index = session.geometry().ok().and_then(|g| g.index_at(local.x as f32, local.y as f32));
            index.map_or(NSNotFound as usize, |b| utf16_offset(session.document.text(), b))
        }
    }
);

impl InputView {
    fn substring_value(&self, proposed: NSRange) -> Option<(String, NSRange)> {
        let state = self.ivars().state.borrow();
        let document = &state.session.as_ref()?.document;
        let range = query_range(document.text(), proposed)?;
        Some((document.text()[range.clone()].to_owned(), native_range(document.text(), range)))
    }

    fn composing(&self) -> bool {
        self.ivars().state.borrow().session.as_ref().is_some_and(|s| s.document.marked().is_some())
    }

    fn enqueue(&self, edit: TextEdit) {
        // Even an idempotent insert/unmark consumed the native key.
        self.ivars().handled.set(true);
        if self.ivars().discarding.get() {
            return;
        }
        let changed = self.ivars().state.borrow_mut().edit(edit);
        if changed {
            (self.ivars().wake)();
        }
    }

    fn discard_composer(&self) {
        let previous = self.ivars().discarding.replace(true);
        if let Some(context) = self.inputContext() {
            context.discardMarkedText();
        }
        self.ivars().discarding.set(previous);
    }
}

/// A per-window, non-hittable child view. Its parent continues handling mouse and ordinary
/// framework key events; this view exclusively owns native text interpretation.
pub(crate) struct TextInputMac {
    view: Retained<InputView>,
    parent: Retained<NSView>,
}

impl TextInputMac {
    pub(crate) fn new(window: &Window, wake: impl Fn() + 'static) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else { return None };
        // SAFETY: winit's AppKit handle is a live NSView, used only on the main thread.
        let parent: Retained<NSView> = unsafe { Retained::retain(handle.ns_view.as_ptr().cast()) }?;
        let this = InputView::alloc(mtm).set_ivars(InputIvars {
            state: RefCell::new(State::default()),
            wake: Box::new(wake),
            queries: Queries::default(),
            handled: Cell::new(false),
            forward: Cell::new(false),
            discarding: Cell::new(false),
            in_key_down: Cell::new(false),
            last_interpret_inline: Cell::new(None),
            first_rect_request: Cell::new(None),
            first_rect_failure: Cell::new(None),
            keys: RefCell::new(VecDeque::new()),
            anchor: Cell::new(None),
        });
        let view: Retained<InputView> = unsafe { msg_send![super(this), initWithFrame: parent.bounds()] };
        view.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        parent.addSubview(&view);
        Some(Self { view, parent })
    }

    /// Called with each core frame. Intermediate frames may update layout, but cannot erase
    /// native edits that the shell has not yet applied.
    pub(crate) fn update(&self, input: Option<&TextInput>) {
        let changed = {
            let mut state = self.view.ivars().state.borrow_mut();
            state.latest = input.cloned();
            state.should_retire()
        };
        if changed {
            self.retire();
            self.view.ivars().state.borrow_mut().synchronize();
            self.focus();
        } else {
            let mut state = self.view.ivars().state.borrow_mut();
            if state.pending.is_empty() && !state.inflight {
                state.synchronize();
            } else if let Some(input) = input
                && let (Some(session), Some(document)) = (&mut state.session, &input.document)
            {
                session.update_frame(input, document);
            }
        }
        self.invalidate_coordinates();
    }

    /// Restore after a framework pointer event: winit's mouseDown may have made its view the
    /// first responder. Do not call this for native-island pointer events.
    pub(crate) fn focus(&self) {
        let has_session = self.view.ivars().state.borrow().session.is_some();
        if has_session && let Some(window) = self.view.window() {
            window.makeFirstResponder(Some(&self.view));
        }
    }

    pub(crate) fn take(&self) -> Vec<NativeEdit> {
        let mut state = self.view.ivars().state.borrow_mut();
        let edits = std::mem::take(&mut state.pending);
        state.inflight |= !edits.is_empty();
        edits
    }

    pub(crate) fn take_key(&self, event: &winit::event::KeyEvent) -> Option<NativeKey> {
        let mut keys = self.view.ivars().keys.borrow_mut();
        // flagsChanged can enqueue modifier KeyboardInput ahead of an already retained
        // keyDown. Matching its physical code keeps that modifier from stealing the event.
        if keys.front().is_some_and(|k| event.physical_key.to_scancode() == Some(k.event.keyCode() as u32)) {
            keys.pop_front()
        } else {
            None
        }
    }

    pub(crate) fn has_marked_text(&self) -> bool {
        self.view.composing()
    }

    /// `true` means the input method consumed this event. With marked text, call this before
    /// application shortcuts; otherwise first let application key handlers consume the key.
    pub(crate) fn interpret_key(&self, key: NativeKey) -> bool {
        {
            let state = self.view.ivars().state.borrow();
            let owner = state.session.as_ref().map(|s| s.owner);
            if key.epoch != state.epoch || key.owner != owner {
                // A focus transition retired this event together with its old composer.
                return true;
            }
            if owner.is_none() {
                return false;
            }
        }
        let ivars = self.view.ivars();
        ivars.last_interpret_inline.set(Some(ivars.in_key_down.get()));
        ivars.handled.set(false);
        ivars.forward.set(false);
        // Offer the retained event to this view's Cocoa input context before falling back
        // to the responder's key bindings.
        let consumed = self.view.inputContext().is_some_and(|context| context.handleEvent(&key.event));
        if !consumed && !ivars.handled.get() && !ivars.forward.get() {
            self.view.interpretKeyEvents(&NSArray::from_slice(&[&*key.event]));
        }
        ivars.handled.get() || !ivars.forward.get()
    }

    pub(crate) fn is_current(&self, edit: &NativeEdit) -> bool {
        let state = self.view.ivars().state.borrow();
        state.epoch == edit.epoch && state.session.as_ref().is_some_and(|s| s.owner == edit.owner)
    }

    /// The batch has reached the safe application boundary. A reentrant native edit remains
    /// pending for the next drain instead of being overwritten by the last core frame.
    pub(crate) fn reconcile(&self) {
        let mut state = self.view.ivars().state.borrow_mut();
        state.inflight = false;
        if state.pending.is_empty() {
            state.synchronize();
        }
        let selection_changed = state.selection_notification();
        drop(state);
        self.invalidate_coordinates();
        if selection_changed
            && let Some(context) = self.view.inputContext()
            && context.respondsToSelector(sel!(textInputClientDidUpdateSelection))
        {
            context.textInputClientDidUpdateSelection();
        }
    }

    /// A handler changed owner or document while applying a batch. Retire the stale composer
    /// and resume from the accepted core document, never replay its remaining native edits.
    pub(crate) fn reject(&self) {
        let focused = self.is_focused();
        self.retire();
        self.view.ivars().state.borrow_mut().synchronize();
        if focused {
            self.focus();
        }
        self.invalidate_coordinates();
    }

    fn retire(&self) {
        {
            let mut state = self.view.ivars().state.borrow_mut();
            state.session = None;
            state.pending.clear();
            state.inflight = false;
            state.epoch = state.epoch.wrapping_add(1);
            state.notified = None;
        }
        self.view.ivars().anchor.set(None);
        self.view.ivars().first_rect_request.set(None);
        self.view.ivars().first_rect_failure.set(None);
        // No session is exposed while IMKit synchronously retires its composer. This is a
        // cancellation, not a request to commit the previous field into the next one.
        self.view.discard_composer();
        if let Some(window) = self.view.window() {
            let is_ours =
                window.firstResponder().is_some_and(|r| std::ptr::eq(&*r as *const _, &*self.view as *const InputView as *const _));
            if is_ours {
                window.makeFirstResponder(Some(&self.parent));
            }
        }
    }

    fn invalidate_coordinates(&self) {
        if let Some(context) = self.view.inputContext() {
            context.invalidateCharacterCoordinates();
        }
    }

    fn is_focused(&self) -> bool {
        self.view
            .window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| std::ptr::eq(&*r as *const _, &*self.view as *const InputView as *const _))
    }

    pub(crate) fn inspect_state(&self) -> Value {
        let (allowed, secure, selected, marked, pending, inflight, geometry_status) = {
            let state = self.view.ivars().state.borrow();
            let session = state.session.as_ref();
            let selected = session.and_then(|s| s.document.selection().map(|r| native_range(s.document.text(), r)));
            let marked = session.and_then(|s| s.document.marked().map(|r| native_range(s.document.text(), r)));
            (
                session.is_some(),
                state.latest.as_ref().is_some_and(|i| i.secure),
                selected,
                marked,
                state.pending.len(),
                state.inflight,
                session.map(|s| s.geometry_status),
            )
        };
        let queries = &self.view.ivars().queries;
        let window_key = self.view.window().is_some_and(|w| w.isKeyWindow());
        let focus = window_key && self.is_focused();
        let context = allowed.then(|| self.view.inputContext()).flatten();
        let current = MainThreadMarker::new().and_then(NSTextInputContext::currentInputContext);
        let context_current = context.as_ref().zip(current.as_ref()).is_some_and(|(a, b)| std::ptr::eq(&**a, &**b));
        let source = context.and_then(|c| c.selectedKeyboardInputSource()).map(|s| s.to_string());
        let range = |r: NSRange| [r.location, r.length];
        let anchor = self.view.ivars().anchor.get().map(|r| [r.origin.x, r.origin.y, r.size.width, r.size.height]);
        json!({
            "allowed": allowed, "secure": secure,
            "native_focus": focus, "context_source": source, "context_current": context_current,
            "window_key": window_key, "last_interpret_inline": self.view.ivars().last_interpret_inline.get(),
            "selected_range_utf16": selected.map(range), "marked_range_utf16": marked.map(range),
            "screen_anchor": anchor, "pending": pending, "inflight": inflight,
            "first_rect_request_utf16": self.view.ivars().first_rect_request.get().map(range),
            "first_rect_failure": self.view.ivars().first_rect_failure.get(),
            "geometry_status": geometry_status,
            "queries": { "selected": queries.selected.get(), "marked": queries.marked.get(),
                "substring": queries.substring.get(), "first_rect": queries.first_rect.get(),
                "character_index": queries.index.get(), "set_marked": queries.set_marked.get(),
                "insert": queries.insert.get(), "unmark": queries.unmark.get() }
        })
    }
}

impl Drop for TextInputMac {
    fn drop(&mut self) {
        self.retire();
        self.view.removeFromSuperview();
    }
}

fn native_string(string: &AnyObject) -> Option<String> {
    if let Some(string) = string.downcast_ref::<NSString>() {
        Some(string.to_string())
    } else {
        string.downcast_ref::<NSAttributedString>().map(|s| s.string().to_string())
    }
}

fn not_found() -> NSRange {
    NSRange::new(NSNotFound as usize, 0)
}

fn replacement_range(document: &TextDocument, range: NSRange) -> Option<Range<usize>> {
    byte_range(document.text(), range)
}

fn query_range(text: &str, range: NSRange) -> Option<Range<usize>> {
    let length = text.encode_utf16().count();
    if range.location > length || (range.location == length && range.length != 0) {
        return None;
    }
    byte_range(text, range)
}

/// UTF-16 document offsets to UTF-8 char boundaries, expanding a split surrogate pair only
/// for nonempty ranges. Overflowing or out-of-document proposals are safely clamped.
fn byte_range(text: &str, range: NSRange) -> Option<Range<usize>> {
    if range.location == NSNotFound as usize {
        return None;
    }
    let end = range.location.saturating_add(range.length);
    let mut utf16 = 0;
    let mut start_byte = text.len();
    let mut end_byte = text.len();
    for (byte, ch) in text.char_indices() {
        let next = utf16 + ch.len_utf16();
        if range.location >= utf16 && range.location < next {
            start_byte = byte;
        }
        if end >= utf16 && end < next {
            end_byte = if end == utf16 { byte } else { byte + ch.len_utf8() };
        }
        utf16 = next;
    }
    Some(start_byte..if range.length == 0 { start_byte } else { end_byte.max(start_byte) })
}

fn utf16_offset(text: &str, byte: usize) -> usize {
    text.char_indices().take_while(|(i, _)| *i < byte).map(|(_, ch)| ch.len_utf16()).sum()
}

fn native_range(text: &str, range: Range<usize>) -> NSRange {
    let start = utf16_offset(text, range.start);
    NSRange::new(start, utf16_offset(text, range.end).saturating_sub(start))
}

unsafe fn write_range(pointer: NSRangePointer, range: NSRange) {
    if !pointer.is_null() {
        // SAFETY: the NSTextInputClient caller promises writable storage or null.
        unsafe { *pointer = range };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_ranges_preserve_surrogates_and_combining_marks() {
        let text = "A😀e\u{301}中";
        assert_eq!(byte_range(text, NSRange::new(1, 2)), Some(1..5));
        assert_eq!(byte_range(text, NSRange::new(2, 0)), Some(1..1));
        assert_eq!(byte_range(text, NSRange::new(2, 1)), Some(1..5));
        assert_eq!(byte_range(text, NSRange::new(3, 2)), Some(5..8));
        assert_eq!(native_range(text, 1..8), NSRange::new(1, 4));
    }

    #[test]
    fn native_ranges_handle_missing_and_oversized_proposals() {
        assert_eq!(byte_range("hi", not_found()), None);
        assert_eq!(byte_range("hi", NSRange::new(1, usize::MAX)), Some(1..2));
        assert_eq!(byte_range("hi", NSRange::new(40, 4)), Some(2..2));
        assert_eq!(byte_range("", NSRange::new(0, 0)), Some(0..0));
        assert_eq!(query_range("hi", NSRange::new(40, 4)), None, "a query outside the document has no actual range");
    }

    #[test]
    fn explicit_replacement_is_document_global_while_marked() {
        let mut document = TextDocument::new("0123456789abcdZ".into(), 10, 14);
        document.apply(&TextEdit::Mark { text: "abcd".into(), selection: Some(4..4), replacement: None });
        let replacement = replacement_range(&document, NSRange::new(1, 1));
        document.apply(&TextEdit::Mark { text: "X".into(), selection: Some(1..1), replacement });
        assert_eq!(document.text(), "0X23456789abcdZ");
        assert_eq!(document.marked(), Some(1..2));
        assert_eq!(replacement_range(&document, not_found()), None, "missing ranges retain the input method's default replacement");
    }

    #[test]
    fn only_idle_external_document_changes_retire_a_same_owner_composer() {
        let original = TextDocument::new("A😀Z".into(), 1, 5);
        let mut state = State {
            latest: Some(TextInput {
                id: NodeId::default(),
                caret: Default::default(),
                secure: false,
                multiline: false,
                document: Some(original.clone()),
                geometry: None,
                layout: None,
            }),
            ..State::default()
        };
        state.synchronize();
        state.edit(TextEdit::Mark { text: "ni".into(), selection: Some(2..2), replacement: None });
        assert!(!state.should_retire(), "a pending native edit must survive an older core frame");
        state.pending.clear();
        state.inflight = true;
        assert!(!state.should_retire(), "an intermediate frame must not retire the batch being applied");
        state.inflight = false;
        let draft = state.session.as_ref().unwrap().document.clone();
        state.latest.as_mut().unwrap().document = Some(draft);
        assert!(!state.should_retire(), "an accepted native draft keeps its composer");
        state.latest.as_mut().unwrap().document = Some(original);
        assert!(state.should_retire(), "an external cancellation must retire IMKit's internal draft too");
    }

    #[test]
    fn an_intermediate_frame_preserves_fresh_native_geometry() {
        let old = TextDocument::new("A😀Z".into(), 1, 5);
        let mut current = old.clone();
        current.apply(&TextEdit::Mark { text: "ni hao".into(), selection: Some(6..6), replacement: None });
        let fresh = TextGeometry { origin: (32.0, 64.0), lines: Vec::new() };
        let mut session = Session {
            owner: NodeId::default(),
            document: current.clone(),
            geometry: Some(fresh.clone()),
            geometry_document: current.clone(),
            layout: None,
            geometry_status: "recomputed",
        };
        let mut input = TextInput {
            id: session.owner,
            caret: Default::default(),
            secure: false,
            multiline: false,
            document: Some(old.clone()),
            geometry: None,
            layout: None,
        };
        session.update_frame(&input, &old);
        assert_eq!(session.geometry, Some(fresh));
        assert_eq!(session.geometry_document, current);
        input.document = Some(current.clone());
        session.update_frame(&input, &current);
        assert!(session.geometry.is_none(), "an accepted core frame becomes authoritative");
    }

    #[test]
    fn selection_changes_invalidate_layout_even_when_displayed_text_matches() {
        let mut before = TextDocument::new("A😀Z".into(), 1, 5);
        before.apply(&TextEdit::Mark { text: "ni hao".into(), selection: Some(6..6), replacement: None });
        let mut session = Session {
            owner: NodeId::default(),
            document: before.clone(),
            geometry: Some(TextGeometry { origin: (0.0, 0.0), lines: Vec::new() }),
            geometry_document: before,
            layout: None,
            geometry_status: "published",
        };
        session.document.apply(&TextEdit::Mark { text: "ni hao".into(), selection: Some(0..0), replacement: None });
        assert_eq!(session.document.text(), session.geometry_document.text());
        assert_eq!(session.geometry().unwrap_err(), "missing_layout", "scroll reveal must be recomputed for the new caret");
    }
}
