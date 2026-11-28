//! AppKit responder and Metal-backed content view.

use super::super::{FileDropPhase, HostWake, PlatformEvent, PlatformKey, PlatformSender, PointerPhase, WindowAccessibility};
use super::editor::TextInput;
use super::input::{GestureState, cursor, key as platform_key, modifiers, wheel};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSCursor, NSDragOperation, NSDraggingContext, NSDraggingDestination, NSDraggingInfo, NSDraggingSession,
    NSDraggingSource, NSEvent, NSEventModifierFlags, NSEventType, NSPasteboardTypeFileURL, NSTextInputClient, NSTextInputContext,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSAttributedString, NSAttributedStringKey, NSPoint, NSRange, NSRangePointer, NSRect, NSURL};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use octara_model::ask::ClipboardKinds;
use octara_model::geometry::Point;
use octara_model::identity::{EditorSessionId, TargetGuard, WindowId};
use octara_model::input::{Button, CursorStyle, DragItem, HostReply, InteractionEvent, PasteEvent, PointerId, PointerKind};
use octara_model::semantics::A11yAction;
use octara_pipeline::NativePublication;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, OsStr};
use std::mem::DropGuard;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::rc::Rc;

pub(super) type Dispatch = dyn Fn(PlatformEvent, Option<DragEvent<'_>>) -> bool;

pub(super) struct DragEvent<'a> {
    view: &'a OctaraView,
    event: &'a NSEvent,
}

impl DragEvent<'_> {
    pub(super) fn dispatch<T>(self, deliver: impl FnOnce() -> T) -> T {
        // Only the matching direct input may start an AppKit session. The host can drain
        // queued events before this callback and run idle work after it.
        let pending = &self.view.ivars().pending_drag_event;
        let previous = pending.replace(Some(self.event.retain()));
        let _restore = DropGuard::new(previous, |previous| {
            pending.replace(previous);
        });
        deliver()
    }
}

pub(super) struct ViewIvars {
    id: WindowId,
    dispatch: Rc<Dispatch>,
    pub(super) text_input: TextInput,
    // The editor fallback runs inside the synchronous key dispatch.
    pending_key: Cell<Option<PendingKey>>,
    tried_equivalent: Cell<Option<Retained<NSEvent>>>,
    has_tracking_area: Cell<bool>,
    pointer_inside: Cell<bool>,
    cursor: RefCell<(CursorStyle, Retained<NSCursor>)>,
    gestures: RefCell<GestureState>,
    pending_drag_event: RefCell<Option<Retained<NSEvent>>>,
    accepted_drag: Cell<Option<isize>>,
    accessibility: RefCell<Option<AccessibilityState>>,
}

struct PendingKey {
    event: Retained<NSEvent>,
    session: Option<EditorSessionId>,
}

define_class!(
    // SAFETY: NSView has no additional subclass requirements. AppKit and the host
    // access the responder and its state on the main thread.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraView"]
    #[ivars = ViewIvars]
    pub(super) struct OctaraView;

    unsafe impl NSObjectProtocol for OctaraView {}

    impl OctaraView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<CALayer> {
            let layer = CAMetalLayer::new();
            layer.setZPosition(1.0);
            Retained::into_super(layer)
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _: Option<&NSEvent>) -> bool {
            self.window().is_some_and(|window| window.styleMask().contains(NSWindowStyleMask::NonactivatingPanel))
        }

        #[unsafe(method_id(inputContext))]
        fn input_context(&self) -> Option<Retained<NSTextInputContext>> { self.editor_context() }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let already_tried = self.ivars().tried_equivalent.take().is_some_and(|previous| std::ptr::eq(&*previous, event));
            if !already_tried {
                self.dispatch_key(event);
            }
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.try_key_equivalent(event)
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            if self.ivars().pointer_inside.get() {
                self.cursor_moved(event);
            }
        }

        #[unsafe(method(paste:))]
        fn paste_action(&self, _: Option<&AnyObject>) {
            if !self.paste() {
                let items = super::services::read_clipboard(ClipboardKinds::all());
                if !items.is_empty() {
                    let reply = HostReply::Paste(PasteEvent { items });
                    self.emit(PlatformEvent::HostReply { window: self.ivars().id, reply });
                }
            }
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // SAFETY: NSView's implementation updates its tracking bookkeeping.
            unsafe { msg_send![super(self), updateTrackingAreas] }
            if !self.ivars().has_tracking_area.replace(true) {
                self.install_tracking_area();
            }
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            // SAFETY: NSView owns the cursor rect list and updates it on this callback.
            unsafe { msg_send![super(self), resetCursorRects] }
            let cursor = self.ivars().cursor.borrow().1.clone();
            self.addCursorRect_cursor(self.visibleRect(), &cursor);
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.ivars().pointer_inside.set(true);
            self.cursor_moved(event);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.ivars().pointer_inside.set(false);
            self.emit(PlatformEvent::CursorLeft(self.ivars().id));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) { self.cursor_moved(event); }
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) { self.button(event, PointerPhase::Moved); }
        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) { self.cursor_moved(event); }
        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) { self.cursor_moved(event); }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) { self.button(event, PointerPhase::Started); }
        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) { self.button(event, PointerPhase::Started); }
        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) { self.button(event, PointerPhase::Started); }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) { self.button(event, PointerPhase::Ended); }
        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) { self.button(event, PointerPhase::Ended); }
        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) { self.button(event, PointerPhase::Ended); }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.interaction(wheel(event, self.position(event.locationInWindow())));
        }

        #[unsafe(method(magnifyWithEvent:))]
        fn magnify(&self, event: &NSEvent) {
            let point = self.position(event.locationInWindow());
            let gesture = self.ivars().gestures.borrow_mut().pinch(event, point);
            if let Some(gesture) = gesture {
                self.interaction(InteractionEvent::Gesture(gesture));
            }
        }

        #[unsafe(method(rotateWithEvent:))]
        fn rotate(&self, event: &NSEvent) {
            let point = self.position(event.locationInWindow());
            let gesture = self.ivars().gestures.borrow_mut().rotate(event, point);
            if let Some(gesture) = gesture {
                self.interaction(InteractionEvent::Gesture(gesture));
            }
        }

        #[unsafe(method_id(accessibilityChildren))]
        fn accessibility_children(&self) -> Option<Retained<NSArray<AnyObject>>> {
            let children = self.ivars().accessibility.borrow_mut().as_mut().and_then(AccessibilityState::children);
            // SAFETY: Preserve AppKit's children alongside the virtual tree.
            let native: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![super(self), accessibilityChildren] };
            match (children, native) {
                (Some(children), Some(native)) if !native.is_empty() => {
                    Some(NSArray::from_retained_slice(&[children.to_vec(), native.to_vec()].concat()))
                }
                (children, native) => children.or(native),
            }
        }

        #[unsafe(method_id(accessibilityFocusedUIElement))]
        fn accessibility_focus(&self) -> Option<Retained<AnyObject>> {
            let focus = self.ivars().accessibility.borrow_mut().as_mut().and_then(AccessibilityState::focus);
            // SAFETY: The default behavior applies when the virtual tree has no focus.
            focus.or_else(|| unsafe { msg_send![super(self), accessibilityFocusedUIElement] })
        }

        #[unsafe(method_id(accessibilityHitTest:))]
        fn accessibility_hit_test(&self, point: NSPoint) -> Option<Retained<AnyObject>> {
            let hit = self.ivars().accessibility.borrow_mut().as_mut().and_then(|state| state.hit_test(point));
            // SAFETY: The default behavior applies without an active virtual element.
            hit.or_else(|| unsafe { msg_send![super(self), accessibilityHitTest: point] })
        }
    }

    unsafe impl NSDraggingSource for OctaraView {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn drag_source_operation(&self, _: &NSDraggingSession, _: NSDraggingContext) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }

    unsafe impl NSDraggingDestination for OctaraView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let paths = file_paths(sender);
            if paths.is_empty() || !sender.draggingSourceOperationMask().contains(NSDragOperation::Copy) {
                return NSDragOperation::None;
            }
            self.ivars().accepted_drag.set(Some(sender.draggingSequenceNumber()));
            self.file_drop(FileDropPhase::Entered, Some(self.position(sender.draggingLocation())), paths);
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.drag_operation(sender)
        }

        #[unsafe(method(prepareForDragOperation:))]
        fn prepare_for_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.drag_operation(sender) == NSDragOperation::Copy
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            if self.ivars().accepted_drag.take().is_some() {
                let position = sender.map(|sender| self.position(sender.draggingLocation()));
                self.file_drop(FileDropPhase::Left, position, Vec::new());
            }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.perform_file_drop(sender)
        }
    }
    unsafe impl NSTextInputClient for OctaraView {
        #[unsafe(method(insertText:replacementRange:))]
        unsafe fn insert_text(&self, string: &AnyObject, replacement: NSRange) {
            self.editor_insert_text(string, replacement)
        }

        #[unsafe(method(doCommandBySelector:))]
        unsafe fn do_command(&self, selector: Sel) {
            self.editor_do_command(selector)
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        unsafe fn set_marked_text(&self, string: &AnyObject, selection: NSRange, replacement: NSRange) {
            self.editor_set_marked_text(string, selection, replacement)
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            self.editor_unmark_text()
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            self.editor_selected_range()
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            self.editor_marked_range()
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            self.editor_has_marked_text()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        unsafe fn substring(&self, proposed: NSRange, actual: NSRangePointer) -> Option<Retained<NSAttributedString>> {
            unsafe { self.editor_substring(proposed, actual) }
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> { NSArray::new() }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        unsafe fn first_rect(&self, proposed: NSRange, actual: NSRangePointer) -> NSRect {
            unsafe { self.editor_first_rect(proposed, actual) }
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, point: NSPoint) -> usize {
            self.editor_character_index(point)
        }
    }
);

impl OctaraView {
    pub(super) fn new(mtm: MainThreadMarker, frame: NSRect, id: WindowId, dispatch: Rc<Dispatch>, wake: HostWake) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            id,
            dispatch,
            text_input: TextInput::new(wake),
            pending_key: Cell::default(),
            tried_equivalent: Cell::default(),
            has_tracking_area: Cell::default(),
            pointer_inside: Cell::default(),
            cursor: RefCell::new((CursorStyle::Arrow, NSCursor::arrowCursor())),
            gestures: RefCell::default(),
            pending_drag_event: RefCell::default(),
            accepted_drag: Cell::default(),
            accessibility: RefCell::default(),
        });
        // SAFETY: The NSView initializer returns the allocated subclass with its ivars initialized.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
        view.setWantsLayer(true);
        // SAFETY: AppKit provides this pasteboard type for the lifetime of the process.
        view.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
        view
    }

    pub(super) fn set_cursor(&self, style: CursorStyle) {
        if self.ivars().cursor.borrow().0 != style {
            let cursor = cursor(style);
            self.ivars().cursor.replace((style, cursor));
            if let Some(window) = self.window() {
                window.invalidateCursorRectsForView(self);
            }
        }
    }

    pub(super) fn set_window_focused(&self, active: bool) {
        if !active {
            self.ivars().pointer_inside.set(false);
            self.ivars().tried_equivalent.take();
        }
        let events = self.ivars().accessibility.borrow_mut().as_mut().and_then(|state| state.adapter.update_view_focus_state(active));
        if let Some(events) = events {
            events.raise();
        }
    }

    fn emit(&self, event: PlatformEvent) -> bool {
        // Dispatch can close the window while AppKit is still in this callback.
        let _keep_alive = self.retain();
        (self.ivars().dispatch)(event, None)
    }

    fn interaction(&self, event: InteractionEvent) {
        self.emit(PlatformEvent::Interaction { window: self.ivars().id, event });
    }

    fn position(&self, window_point: NSPoint) -> Point {
        let point = self.convertPoint_fromView(window_point, None);
        Point::new(point.x as f32, point.y as f32)
    }

    fn cursor_moved(&self, event: &NSEvent) {
        self.emit(PlatformEvent::CursorMoved {
            window: self.ivars().id,
            position: self.position(event.locationInWindow()),
            modifiers: modifiers(event.modifierFlags()),
        });
    }

    fn button(&self, event: &NSEvent, phase: PointerPhase) {
        let button = match event.buttonNumber() {
            0 => Button::Primary,
            1 => Button::Secondary,
            2 => Button::Middle,
            _ => return,
        };
        let input = PlatformEvent::Pointer {
            window: self.ivars().id,
            phase,
            position: Some(self.position(event.locationInWindow())),
            id: PointerId(0),
            kind: PointerKind::Mouse,
            button,
            modifiers: modifiers(event.modifierFlags()),
        };
        if phase == PointerPhase::Moved {
            let _keep_alive = self.retain();
            (self.ivars().dispatch)(input, Some(DragEvent { view: self, event }));
        } else {
            self.emit(input);
        }
    }

    pub(super) fn start_drag_out(&self, items: &[DragItem]) -> bool {
        let event = self.ivars().pending_drag_event.borrow().clone();
        let Some(event) = event else { return false };
        let Some(window) = self.window() else { return false };
        if event.r#type() != NSEventType::LeftMouseDragged || event.windowNumber() != window.windowNumber() || items.is_empty() {
            return false;
        }
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let Some(items) = super::drag::dragging_items(items, point) else { return false };
        self.ivars().pending_drag_event.take();
        let source = ProtocolObject::<dyn NSDraggingSource>::from_ref(self);
        // SAFETY: These are AppKit dragging items, and the exact mouse event is still being dispatched to this view.
        // AppKit documents a nil return when the session cannot start.
        let session: Option<Retained<NSDraggingSession>> =
            unsafe { msg_send![self, beginDraggingSessionWithItems: &*items, event: &*event, source: source] };
        session.is_some()
    }

    fn try_key_equivalent(&self, event: &NSEvent) -> bool {
        let _keep_alive = self.retain();
        // SAFETY: Offer the equivalent to subviews using the superclass implementation.
        let handled: bool = unsafe { msg_send![super(self), performKeyEquivalent: event] };
        if handled {
            return true;
        }
        if event.r#type() != NSEventType::KeyDown
            || !event.modifierFlags().intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Control)
            || self
                .window()
                .and_then(|window| window.firstResponder())
                .is_none_or(|responder| !std::ptr::eq(Retained::as_ptr(&responder).cast::<Self>(), self))
        {
            return false;
        }
        if self.dispatch_key(event) {
            return true;
        }
        // The menu can still handle it. If AppKit subsequently delivers keyDown:,
        // skip the duplicate runtime event.
        self.ivars().tried_equivalent.set(Some(event.retain()));
        false
    }

    fn dispatch_key(&self, event: &NSEvent) -> bool {
        let _keep_alive = self.retain();
        let composing = self.composing();
        if composing && self.interpret_editor_key(event) {
            return true;
        }
        let pending = &self.ivars().pending_key;
        let previous = pending.replace((!composing).then(|| PendingKey { event: event.retain(), session: self.editor_session() }));
        let _restore = DropGuard::new(previous, |previous| pending.set(previous));
        self.emit(PlatformEvent::Key { window: self.ivars().id, event: platform_key(event) })
    }

    pub(super) fn interpret_pending_key(&self, key: &PlatformKey) -> bool {
        let Some(pending) = self.ivars().pending_key.take() else { return false };
        let native = platform_key(&pending.event);
        if native.event != key.event || native.text != key.text {
            self.ivars().pending_key.set(Some(pending));
            return false;
        }
        pending.session != self.editor_session() || self.interpret_editor_key(&pending.event)
    }

    fn install_tracking_area(&self) {
        use NSTrackingAreaOptions as Options;
        // SAFETY: The owner is this view, which implements all tracking callbacks requested here.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                Options::InVisibleRect | Options::ActiveInKeyWindow | Options::MouseEnteredAndExited | Options::MouseMoved,
                Some(self),
                None,
            )
        };
        self.addTrackingArea(&area);
    }

    fn drag_operation(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
        if self.ivars().accepted_drag.get() == Some(sender.draggingSequenceNumber())
            && sender.draggingSourceOperationMask().contains(NSDragOperation::Copy)
        {
            NSDragOperation::Copy
        } else {
            NSDragOperation::None
        }
    }

    fn perform_file_drop(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
        if self.drag_operation(sender) != NSDragOperation::Copy {
            return false;
        }
        self.ivars().accepted_drag.take();
        let paths = file_paths(sender);
        let position = Some(self.position(sender.draggingLocation()));
        if paths.is_empty() {
            self.file_drop(FileDropPhase::Left, position, Vec::new());
            return false;
        }
        self.file_drop(FileDropPhase::Dropped, position, paths);
        true
    }

    fn file_drop(&self, phase: FileDropPhase, position: Option<Point>, paths: Vec<PathBuf>) {
        self.emit(PlatformEvent::FileDrop { window: self.ivars().id, phase, position, paths });
    }
}

fn file_paths(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    // SAFETY: NSURL implements NSPasteboardReading. A successful downcast establishes the
    // type before we read each URL and copy its null-terminated filesystem representation.
    let items = unsafe { sender.draggingPasteboard().readObjectsForClasses_options(&NSArray::from_slice(&[NSURL::class()]), None) };
    items
        .into_iter()
        .flat_map(|items| items.to_vec())
        .filter_map(|object| {
            let url = object.downcast::<NSURL>().ok()?;
            if !url.isFileURL() {
                return None;
            }
            let path = unsafe { CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) };
            Some(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
        })
        .collect()
}

struct AccessibilityHandler {
    window: WindowId,
    sender: PlatformSender,
}

impl accesskit::ActivationHandler for AccessibilityHandler {
    fn request_initial_tree(&mut self) -> Option<accesskit::TreeUpdate> {
        self.sender.send_event(PlatformEvent::AccessibilityEnabledChanged { window: self.window, enabled: true });
        None
    }
}

impl accesskit::ActionHandler for AccessibilityHandler {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        self.sender.send_event(PlatformEvent::AccessKitAction { window: self.window, request });
    }
}

struct AccessibilityState {
    adapter: accesskit_macos::Adapter,
    activation: AccessibilityHandler,
    published: Option<u64>,
}

impl AccessibilityState {
    fn new(view: &OctaraView, sender: PlatformSender) -> Self {
        let window = view.ivars().id;
        let actions = AccessibilityHandler { window, sender: sender.clone() };
        let focused = view.window().is_some_and(|window| window.isKeyWindow());
        // SAFETY: objc2 0.2 and 0.3 use the same Objective-C object pointers. The view is
        // a live NSView on the main thread, and the adapter keeps a weak reference to it.
        let adapter = unsafe { accesskit_macos::Adapter::new(std::ptr::from_ref(view).cast_mut().cast(), focused, actions) };
        Self { adapter, activation: AccessibilityHandler { window, sender }, published: None }
    }

    fn children(&mut self) -> Option<Retained<NSArray<AnyObject>>> {
        let children = self.adapter.view_children(&mut self.activation);
        // SAFETY: AccessKit returns an autoreleased NSArray of Objective-C objects.
        unsafe { Retained::retain(children.cast()) }
    }

    fn focus(&mut self) -> Option<Retained<AnyObject>> {
        let focus = self.adapter.focus(&mut self.activation);
        // SAFETY: AccessKit returns an autoreleased Objective-C object or null.
        unsafe { Retained::retain(focus.cast()) }
    }

    fn hit_test(&mut self, point: NSPoint) -> Option<Retained<AnyObject>> {
        let point = accesskit_macos::NSPoint::new(point.x, point.y);
        let hit = self.adapter.hit_test(point, &mut self.activation);
        // SAFETY: AccessKit returns an autoreleased Objective-C object or null.
        unsafe { Retained::retain(hit.cast()) }
    }
}

pub(super) struct Accessibility(Retained<OctaraView>);

impl Accessibility {
    pub(super) fn new(view: &OctaraView, sender: PlatformSender) -> Self {
        view.ivars().accessibility.replace(Some(AccessibilityState::new(view, sender)));
        Self(view.retain())
    }
}

impl WindowAccessibility for Accessibility {
    fn synchronize(&mut self, publication: &NativePublication) {
        let mut slot = self.0.ivars().accessibility.borrow_mut();
        let Some(state) = slot.as_mut() else { return };
        let mut queued = Vec::new();
        if let Some(previous) = state.published {
            for (version, update) in publication.take_accessibility_updates() {
                if version > previous {
                    queued.extend(state.adapter.update_if_active(|| update.as_ref().clone()));
                    state.published = Some(version);
                }
            }
        } else if let Some(frame) = publication.latest()
            && let Some(snapshot) = publication.snapshot_full()
        {
            queued.extend(state.adapter.update_if_active(|| {
                publication.take_accessibility_updates();
                state.published = Some(frame.number);
                snapshot
            }));
        }
        drop(slot);
        for events in queued {
            events.raise();
        }
    }

    fn clear(&mut self) {
        let sender = self.0.ivars().accessibility.borrow().as_ref().map(|state| state.activation.sender.clone());
        if let Some(sender) = sender {
            let state = AccessibilityState::new(&self.0, sender);
            let old = self.0.ivars().accessibility.replace(Some(state));
            // AccessKit may send destroyed notifications as it drops the old tree.
            drop(old);
        }
    }

    fn accepts_action(&self, _: &NativePublication, _: TargetGuard, _: &A11yAction) -> bool {
        // The driver validates AccessKit action requests against the published tree.
        false
    }
}
