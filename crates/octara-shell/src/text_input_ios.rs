//! UIKit owns the soft keyboard and marked draft. The Rust editor receives the
//! resulting document at the next safe event-loop boundary.

use super::ime_draft::Draft;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIColor, UIControlEvents, UIEvent, UIKeyboardDidChangeFrameNotification, UIKeyboardDidHideNotification,
    UITextField, UITextFieldDelegate, UITextInput, UITextRange, UIView,
};
use octara::app::{byte_offset, utf16_offset};
use octara::kernel::TextDocument;
use octara::platform::TextInput;
use octara::serde_json::{Value, json};
use octara_primitives::identity::NodeId;
use octara_primitives::text::TextEdit;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use winit::window::Window;

struct Signals {
    wake: Rc<dyn Fn()>,
    changed: Cell<bool>,
    submit: Cell<bool>,
    muted: Cell<bool>,
}

define_class!(
    #[unsafe(super(UITextField))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraUIKitEditor"]
    #[ivars = Rc<Signals>]
    struct Field;

    impl Field {
        #[unsafe(method(pointInside:withEvent:))]
        fn point_inside(&self, _: NSPoint, _: Option<&UIEvent>) -> bool {
            false
        }

        #[unsafe(method(editingChanged))]
        fn editing_changed(&self) {
            self.queue_change();
        }
    }

    unsafe impl NSObjectProtocol for Field {}

    unsafe impl UITextFieldDelegate for Field {
        #[unsafe(method(textFieldDidChangeSelection:))]
        fn selection_changed(&self, _: &UITextField) {
            self.queue_change();
        }

        #[unsafe(method(textFieldShouldReturn:))]
        fn should_return(&self, field: &UITextField) -> bool {
            let signals = self.ivars();
            if !signals.muted.get() && field.markedTextRange().is_none() {
                signals.submit.set(true);
                (signals.wake)();
            }
            false
        }
    }
);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraKeyboardObserver"]
    #[ivars = Rc<dyn Fn()>]
    struct KeyboardObserver;

    impl KeyboardObserver {
        #[unsafe(method(keyboardChanged:))]
        fn changed(&self, _: &NSNotification) {
            (self.ivars())();
        }
    }
);

impl Field {
    fn queue_change(&self) {
        let signals = self.ivars();
        if !signals.muted.get() {
            signals.changed.set(true);
            (signals.wake)();
        }
    }

    fn read(&self) -> Draft {
        let text = self.text().map(|s| s.to_string()).unwrap_or_default();
        let beginning = self.beginningOfDocument();
        let offsets = |range: Retained<UITextRange>| {
            let offset = |p| byte_offset(&text, self.offsetFromPosition_toPosition(&beginning, p).max(0) as usize);
            offset(&range.start())..offset(&range.end())
        };
        let marked = self.markedTextRange().map(offsets).filter(|r| !r.is_empty());
        let selection = self.selectedTextRange().map(offsets);
        Draft { text, marked, selection }
    }

    fn select(&self, text: &str, selection: Range<usize>) {
        let beginning = self.beginningOfDocument();
        let start = self.positionFromPosition_offset(&beginning, utf16_offset(text, selection.start) as isize);
        let end = self.positionFromPosition_offset(&beginning, utf16_offset(text, selection.end) as isize);
        if let Some((start, end)) = start.zip(end) {
            self.setSelectedTextRange(self.textRangeFromPosition_toPosition(&start, &end).as_deref());
        }
    }

    fn synchronize(&self, document: &TextDocument) {
        let current = self.read();
        let signals = self.ivars();
        signals.muted.set(true);
        if current.text != document.text() || current.marked != document.marked() {
            self.setText(Some(&NSString::from_str(document.text())));
        }
        if let Some(selection) = document.selection()
            && self.read().selection.as_ref() != Some(&selection)
        {
            self.select(document.text(), selection);
        }
        signals.muted.set(false);
    }
}

struct Session {
    owner: NodeId,
    field: Retained<Field>,
    expected: TextDocument,
}

pub(crate) struct Change {
    pub(crate) owner: NodeId,
    pub(crate) expected: TextDocument,
    pub(crate) edit: Option<TextEdit>,
    pub(crate) selection: Option<Range<usize>>,
    pub(crate) submit: bool,
    signals: Rc<Signals>,
}

pub(crate) struct TextInputIos {
    content: Retained<UIView>,
    observer: Retained<KeyboardObserver>,
    wake: Rc<dyn Fn()>,
    latest: Option<TextInput>,
    session: Option<Session>,
    active: bool,
    inflight: bool,
}

impl TextInputIos {
    pub(crate) fn new(window: &Window, wake: impl Fn() + 'static) -> Option<Self> {
        let content = super::scene_ios::ui_view(window).ok()?;
        let wake: Rc<dyn Fn()> = Rc::new(wake);
        let mtm = MainThreadMarker::new()?;
        // SAFETY: NSObject initialization on the UIKit thread.
        let observer: Retained<KeyboardObserver> =
            unsafe { msg_send![super(mtm.alloc::<KeyboardObserver>().set_ivars(wake.clone())), init] };
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: UIKit posts these notifications on the main thread. The selector accepts NSNotification.
        unsafe {
            for name in [UIKeyboardDidChangeFrameNotification, UIKeyboardDidHideNotification] {
                center.addObserver_selector_name_object(&observer, sel!(keyboardChanged:), Some(name), None);
            }
        }
        Some(Self { content, observer, wake, latest: None, session: None, active: true, inflight: false })
    }

    pub(crate) fn update(&mut self, input: Option<&TextInput>) {
        self.latest = input.filter(|input| !input.multiline && input.document.is_some()).cloned();
        let next = self.latest.as_ref().filter(|_| self.active);
        let replaced_draft = !self.inflight
            && self.session.as_ref().zip(next).is_some_and(|(session, input)| {
                !session.field.ivars().changed.get()
                    && input.document.as_ref().is_some_and(|document| document != &session.expected && document.marked().is_none())
                    && session.field.markedTextRange().is_some()
            });
        if self.session.as_ref().map(|s| s.owner) != next.map(|i| i.id) || replaced_draft {
            self.retire();
            if self.active {
                self.install();
            }
        } else if let (Some(session), Some(input)) = (&mut self.session, next) {
            let document = input.document.as_ref().expect("eligible input has a document");
            if !self.inflight && !session.field.ivars().changed.get() && document != &session.expected {
                session.field.synchronize(document);
                session.expected = document.clone();
            }
        }
        if input.is_none() {
            self.content.endEditing(true);
        }
        if let (Some(s), Some(input)) = (&self.session, &self.latest) {
            let frame = self.anchor(input);
            if s.field.frame() != frame {
                s.field.setFrame(frame);
            }
            if !s.field.isFirstResponder() {
                s.field.becomeFirstResponder();
            }
        }
    }

    fn anchor(&self, input: &TextInput) -> NSRect {
        NSRect::new(
            NSPoint::new(0.0, f64::from(input.caret.y)),
            NSSize::new(self.content.bounds().size.width, f64::from(input.caret.h.max(1.0))),
        )
    }

    fn install(&mut self) {
        let Some(input) = &self.latest else { return };
        let Some(document) = &input.document else { return };
        let Some(mtm) = MainThreadMarker::new() else { return };
        let signals =
            Rc::new(Signals { wake: self.wake.clone(), changed: Cell::new(false), submit: Cell::new(false), muted: Cell::new(false) });
        // SAFETY: UITextField's initializer is called on the UIKit thread with its view frame.
        let field: Retained<Field> =
            unsafe { msg_send![super(mtm.alloc::<Field>().set_ivars(signals)), initWithFrame: self.anchor(input)] };
        field.setDelegate(Some(ProtocolObject::from_ref(&*field)));
        // SAFETY: editingChanged takes no arguments. The color is a UIKit UIColor on the main thread.
        unsafe {
            field.addTarget_action_forControlEvents(Some(&field), sel!(editingChanged), UIControlEvents::EditingChanged);
            field.setTintColor(Some(&UIColor::clearColor()));
        }
        field.setTextColor(Some(&UIColor::clearColor()));
        field.setIsAccessibilityElement(false, mtm);
        self.content.addSubview(&field);
        field.synchronize(document);
        self.session = Some(Session { owner: input.id, field, expected: document.clone() });
    }

    fn retire(&mut self) {
        self.inflight = false;
        if let Some(session) = self.session.take() {
            session.field.ivars().muted.set(true);
            session.field.resignFirstResponder();
            session.field.removeFromSuperview();
        }
    }

    pub(crate) fn set_active(&mut self, active: bool) -> Option<(NodeId, TextDocument)> {
        self.active = active;
        if active {
            return None;
        }
        let cancel = self.latest.as_ref().and_then(|input| {
            let document = input.document.as_ref()?;
            document.marked().map(|_| (input.id, document.clone()))
        });
        self.retire();
        self.content.endEditing(true);
        cancel
    }

    pub(crate) fn take(&mut self) -> Option<Change> {
        let session = self.session.as_ref()?;
        let signals = session.field.ivars();
        let changed = signals.changed.replace(false);
        let submit = signals.submit.replace(false);
        if !changed && !submit {
            return None;
        }
        let draft = session.field.read();
        let cancelled = session.expected.marked().is_some_and(|mark| {
            draft.marked.is_none()
                && draft.text.len() + mark.len() == session.expected.text().len()
                && draft.text.starts_with(&session.expected.text()[..mark.start])
                && draft.text.ends_with(&session.expected.text()[mark.end..])
        });
        let edit = draft.edit(&session.expected, cancelled);
        self.inflight = true;
        Some(Change {
            owner: session.owner,
            expected: session.expected.clone(),
            edit,
            selection: draft.selection,
            submit,
            signals: signals.clone(),
        })
    }

    pub(crate) fn is_current(&self, change: &Change) -> bool {
        self.session.as_ref().is_some_and(|s| Rc::ptr_eq(s.field.ivars(), &change.signals))
    }

    pub(crate) fn reconcile(&mut self, change: &Change) {
        if self.is_current(change) {
            self.inflight = false;
            let input = self.latest.clone();
            self.update(input.as_ref());
            if let Some(session) = &self.session
                && self.is_current(change)
                && !session.field.ivars().changed.get()
            {
                session.field.synchronize(&session.expected);
            }
        }
    }

    pub(crate) fn keyboard_top(&self) -> Option<f32> {
        let view = &self.content;
        let guide = view.keyboardLayoutGuide().layoutFrame();
        let top = guide.origin.y;
        let height = view.bounds().size.height;
        (top > 0.0 && height - top > view.safeAreaInsets().bottom + 1.0).then_some(top as f32)
    }

    pub(crate) fn inspect(&self) -> Value {
        json!({
            "allowed": self.session.is_some(),
            "native_focus": self.session.as_ref().is_some_and(|s| s.field.isFirstResponder()),
            "can_focus": self.session.as_ref().is_some_and(|s| s.field.canBecomeFirstResponder()),
            "key_window": self.content.window().is_some_and(|window| window.isKeyWindow()),
            "content_focus": self.content.isFirstResponder(),
            "has_marked": self.session.as_ref().is_some_and(|s| s.field.markedTextRange().is_some()),
            "keyboard_top": self.keyboard_top(),
        })
    }
}

impl Drop for TextInputIos {
    fn drop(&mut self) {
        self.retire();
        // SAFETY: this is the observer registered by this UI-thread adapter.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(&self.observer) };
    }
}
