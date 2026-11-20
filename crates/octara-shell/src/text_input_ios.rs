//! UIKit owns the soft keyboard and marked draft. The Rust editor receives the
//! resulting document at the next safe event-loop boundary.

use super::ime_draft::Draft;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRect, NSSize, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIColor, UIControlEvents, UIEdgeInsets, UIEvent, UIKeyboardDidChangeFrameNotification,
    UIKeyboardDidHideNotification, UIScrollViewDelegate, UITextField, UITextFieldDelegate, UITextInput, UITextPosition, UITextRange,
    UITextView, UITextViewDelegate, UIView,
};
use octara::app::{byte_offset, utf16_offset};
use octara::kernel::{TextDocument, TextGeometry};
use octara::platform::TextInput;
use octara::serde_json::{Value, json};
use octara_primitives::identity::NodeId;
use octara_primitives::text::TextEdit;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

struct Signals {
    wake: Rc<dyn Fn()>,
    changed: Cell<bool>,
    submit: Cell<bool>,
    muted: Cell<bool>,
}

impl Signals {
    fn changed(&self) {
        if !self.muted.get() {
            self.changed.set(true);
            (self.wake)();
        }
    }

    fn return_pressed(&self) {
        if !self.muted.get() {
            self.submit.set(true);
            (self.wake)();
        }
    }
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
            self.ivars().changed();
        }
    }

    unsafe impl NSObjectProtocol for Field {}

    unsafe impl UITextFieldDelegate for Field {
        #[unsafe(method(textFieldDidChangeSelection:))]
        fn selection_changed(&self, _: &UITextField) {
            self.ivars().changed();
        }

        #[unsafe(method(textFieldShouldReturn:))]
        fn should_return(&self, field: &UITextField) -> bool {
            if field.markedTextRange().is_none() {
                self.ivars().return_pressed();
            }
            false
        }
    }
);

struct AreaState {
    signals: Rc<Signals>,
    frame: RefCell<TextInput>,
    pasting: Cell<bool>,
    last_geometry: Cell<Option<GeometryQuery>>,
}

#[derive(Clone, Copy)]
struct GeometryQuery {
    range: [usize; 2],
    rect: [f32; 4],
}

define_class!(
    #[unsafe(super(UITextView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraUIKitTextArea"]
    #[ivars = AreaState]
    struct Area;

    impl Area {
        #[unsafe(method(pointInside:withEvent:))]
        fn point_inside(&self, _: NSPoint, _: Option<&UIEvent>) -> bool {
            false
        }

        #[unsafe(method(insertText:))]
        fn insert_text(&self, text: &NSString) {
            if !self.ivars().pasting.get() && self.markedTextRange().is_none() && text.length() == 1 && text.characterAtIndex(0) == 10 {
                self.ivars().signals.return_pressed();
            } else {
                // SAFETY: UITextView owns the native edit, including the marked-text session.
                let _: () = unsafe { msg_send![super(self), insertText: text] };
            }
        }

        #[unsafe(method(paste:))]
        fn paste_text(&self, sender: Option<&AnyObject>) {
            let pasting = &self.ivars().pasting;
            let previous = pasting.replace(true);
            // SAFETY: This is the inherited UIResponder paste action.
            let _: () = unsafe { msg_send![super(self), paste: sender] };
            pasting.set(previous);
        }

        #[unsafe(method(setMarkedText:selectedRange:))]
        fn set_mark(&self, text: Option<&NSString>, selected: objc2_foundation::NSRange) {
            // SAFETY: Implementation of the inherited UITextInput selector.
            let _: () = unsafe { msg_send![super(self), setMarkedText: text, selectedRange: selected] };
            self.ivars().signals.changed();
        }

        #[unsafe(method(unmarkText))]
        fn unmark(&self) {
            // SAFETY: Implementation of the inherited UITextInput selector.
            let _: () = unsafe { msg_send![super(self), unmarkText] };
            self.ivars().signals.changed();
        }

        #[unsafe(method(firstRectForRange:))]
        fn first_rect(&self, range: &UITextRange) -> NSRect {
            if let Some(rect) = self.octara_rect(&range.start(), &range.end()) {
                return rect;
            }
            // SAFETY: This is the inherited UITextInput query.
            unsafe { msg_send![super(self), firstRectForRange: range] }
        }

        #[unsafe(method(caretRectForPosition:))]
        fn caret_rect(&self, position: &UITextPosition) -> NSRect {
            if let Some(mut rect) = self.octara_rect(position, position) {
                rect.size.width = 1.0;
                return rect;
            }
            // SAFETY: This is the inherited UITextInput query.
            unsafe { msg_send![super(self), caretRectForPosition: position] }
        }
    }

    unsafe impl NSObjectProtocol for Area {}

    unsafe impl UIScrollViewDelegate for Area {}

    unsafe impl UITextViewDelegate for Area {
        #[unsafe(method(textViewDidChange:))]
        fn editing_changed(&self, _: &UITextView) {
            self.ivars().signals.changed();
        }

        #[unsafe(method(textViewDidChangeSelection:))]
        fn selection_changed(&self, _: &UITextView) {
            self.ivars().signals.changed();
        }
    }
);

impl Area {
    fn geometry(&self, draft: &Draft) -> Option<TextGeometry> {
        let layout = {
            let frame = self.ivars().frame.borrow();
            let current = frame.document.as_ref()?;
            if current.text() == draft.text && current.selection() == draft.selection {
                return frame.geometry.clone();
            }
            frame.layout.clone()?
        };
        // A native query can arrive before the corresponding edit reached Octara. Shape that
        // draft using the issued field configuration, independently of the mounted window.
        let selection = draft.selection.clone().unwrap_or(draft.text.len()..draft.text.len());
        layout.layout(&TextDocument::new(draft.text.clone(), selection.start, selection.end))
    }

    fn octara_rect(&self, start: &UITextPosition, end: &UITextPosition) -> Option<NSRect> {
        let parent = self.superview()?;
        let draft = read_draft(&**self, self.text().to_string());
        let range = position_byte(&**self, &draft.text, start)..position_byte(&**self, &draft.text, end);
        let (actual, rect) = self.geometry(&draft)?.first_rect(range)?;
        self.ivars().last_geometry.set(Some(GeometryQuery { range: [actual.start, actual.end], rect: [rect.x, rect.y, rect.w, rect.h] }));
        let rect = NSRect::new(NSPoint::new(f64::from(rect.x), f64::from(rect.y)), NSSize::new(f64::from(rect.w), f64::from(rect.h)));
        Some(self.textInputView().convertRect_fromView(rect, Some(&parent)))
    }
}

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

fn position_byte(input: &(impl UITextInput + Message), text: &str, position: &UITextPosition) -> usize {
    let beginning = input.beginningOfDocument();
    byte_offset(text, input.offsetFromPosition_toPosition(&beginning, position).max(0) as usize)
}

fn read_draft(input: &(impl UITextInput + Message), text: String) -> Draft {
    let offsets = |range: Retained<UITextRange>| position_byte(input, &text, &range.start())..position_byte(input, &text, &range.end());
    let marked = input.markedTextRange().map(offsets).filter(|r| !r.is_empty());
    let selection = input.selectedTextRange().map(offsets);
    Draft { text, marked, selection }
}

fn select(input: &(impl UITextInput + Message), text: &str, selection: Range<usize>) {
    let beginning = input.beginningOfDocument();
    let start = input.positionFromPosition_offset(&beginning, utf16_offset(text, selection.start) as isize);
    let end = input.positionFromPosition_offset(&beginning, utf16_offset(text, selection.end) as isize);
    if let Some((start, end)) = start.zip(end) {
        input.setSelectedTextRange(input.textRangeFromPosition_toPosition(&start, &end).as_deref());
    }
}

enum Editor {
    Field(Retained<Field>),
    Area(Retained<Area>),
}

impl Editor {
    fn view(&self) -> &UIView {
        match self {
            Self::Field(field) => field,
            Self::Area(area) => area,
        }
    }

    fn input(&self) -> &ProtocolObject<dyn UITextInput> {
        match self {
            Self::Field(field) => ProtocolObject::from_ref::<UITextField>(field),
            Self::Area(area) => ProtocolObject::from_ref::<UITextView>(area),
        }
    }

    fn update_geometry(&self, input: &TextInput) {
        if let Self::Area(view) = self {
            *view.ivars().frame.borrow_mut() = input.clone();
        }
    }

    fn read(&self) -> Draft {
        let text = match self {
            Self::Field(field) => field.text().map(|s| s.to_string()).unwrap_or_default(),
            Self::Area(area) => area.text().to_string(),
        };
        read_draft(self.input(), text)
    }

    fn synchronize(&self, document: &TextDocument, signals: &Signals) {
        let current = self.read();
        signals.muted.set(true);
        if current.text != document.text() || current.marked != document.marked() {
            let text = NSString::from_str(document.text());
            match self {
                Self::Field(field) => field.setText(Some(&text)),
                Self::Area(area) => area.setText(Some(&text)),
            }
        }
        if let Some(selection) = document.selection()
            && self.read().selection.as_ref() != Some(&selection)
        {
            select(self.input(), document.text(), selection);
        }
        signals.muted.set(false);
    }
}

struct Session {
    owner: NodeId,
    editor: Editor,
    signals: Rc<Signals>,
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
    pub(crate) fn new(content: Retained<UIView>, wake: impl Fn() + 'static) -> Self {
        let wake: Rc<dyn Fn()> = Rc::new(wake);
        let mtm = MainThreadMarker::from(&*content);
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
        Self { content, observer, wake, latest: None, session: None, active: true, inflight: false }
    }

    pub(crate) fn update(&mut self, input: Option<&TextInput>) {
        self.latest = input.filter(|input| input.document.is_some()).cloned();
        let next = self.latest.as_ref().filter(|_| self.active);
        let replaced_draft = !self.inflight
            && self.session.as_ref().zip(next).is_some_and(|(session, input)| {
                !session.signals.changed.get()
                    && input.document.as_ref().is_some_and(|document| document != &session.expected && document.marked().is_none())
                    && session.editor.input().markedTextRange().is_some()
            });
        let current = self.session.as_ref().map(|s| (s.owner, matches!(s.editor, Editor::Area(_))));
        if current != next.map(|i| (i.id, i.multiline)) || replaced_draft {
            self.retire();
            if self.active {
                self.install();
            }
        } else if let (Some(session), Some(input)) = (&mut self.session, next) {
            let document = input.document.as_ref().expect("eligible input has a document");
            session.editor.update_geometry(input);
            if !self.inflight && !session.signals.changed.get() && document != &session.expected {
                session.editor.synchronize(document, &session.signals);
                session.expected = document.clone();
            }
        }
        if input.is_none() {
            self.content.endEditing(true);
        }
        if let (Some(s), Some(input)) = (&self.session, &self.latest) {
            let frame = self.anchor(input);
            let view = s.editor.view();
            if view.frame() != frame {
                view.setFrame(frame);
            }
            if !view.isFirstResponder() {
                view.becomeFirstResponder();
            }
        }
    }

    fn anchor(&self, input: &TextInput) -> NSRect {
        if input.multiline {
            return self.content.bounds();
        }
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
        let frame = self.anchor(input);
        let clear = UIColor::clearColor();
        let editor = if input.multiline {
            let state = AreaState {
                signals: signals.clone(),
                frame: RefCell::new(input.clone()),
                pasting: Cell::new(false),
                last_geometry: Cell::new(None),
            };
            // SAFETY: UIKit initialization on the main thread, with the default text container.
            let area: Retained<Area> =
                unsafe { msg_send![super(mtm.alloc::<Area>().set_ivars(state)), initWithFrame: frame, textContainer: None::<&AnyObject>] };
            // SAFETY: The view is its own delegate and lives for the duration of the session.
            unsafe { area.setDelegate(Some(ProtocolObject::from_ref(&*area))) };
            area.setScrollEnabled(false);
            area.setTextContainerInset(UIEdgeInsets { top: 0.0, left: 0.0, bottom: 0.0, right: 0.0 });
            area.setTextColor(Some(&clear));
            Editor::Area(area)
        } else {
            // SAFETY: UITextField initialization on the main thread with its view frame.
            let field: Retained<Field> = unsafe { msg_send![super(mtm.alloc::<Field>().set_ivars(signals.clone())), initWithFrame: frame] };
            field.setDelegate(Some(ProtocolObject::from_ref(&*field)));
            // SAFETY: The editingChanged target is this live field and the selector accepts no arguments.
            unsafe { field.addTarget_action_forControlEvents(Some(&field), sel!(editingChanged), UIControlEvents::EditingChanged) };
            field.setTextColor(Some(&clear));
            Editor::Field(field)
        };
        let view = editor.view();
        view.setBackgroundColor(Some(&clear));
        // SAFETY: The color is a UIKit UIColor on the main thread.
        unsafe { view.setTintColor(Some(&clear)) };
        view.setIsAccessibilityElement(false, mtm);
        self.content.addSubview(view);
        editor.synchronize(document, &signals);
        self.session = Some(Session { owner: input.id, editor, signals, expected: document.clone() });
    }

    fn retire(&mut self) {
        self.inflight = false;
        if let Some(session) = self.session.take() {
            session.signals.muted.set(true);
            session.editor.view().resignFirstResponder();
            session.editor.view().removeFromSuperview();
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
        let signals = &session.signals;
        let changed = signals.changed.replace(false);
        let submit = signals.submit.replace(false);
        if !changed && !submit {
            return None;
        }
        let draft = session.editor.read();
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
        self.session.as_ref().is_some_and(|s| Rc::ptr_eq(&s.signals, &change.signals))
    }

    pub(crate) fn reconcile(&mut self, change: &Change) {
        if self.is_current(change) {
            self.inflight = false;
            let input = self.latest.clone();
            self.update(input.as_ref());
            if let Some(session) = &self.session
                && self.is_current(change)
                && !session.signals.changed.get()
            {
                session.editor.synchronize(&session.expected, &session.signals);
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
        let geometry = self.session.as_ref().and_then(|session| match &session.editor {
            Editor::Area(area) => area.ivars().last_geometry.get(),
            Editor::Field(_) => None,
        });
        json!({
            "allowed": self.session.is_some(),
            "native_focus": self.session.as_ref().is_some_and(|s| s.editor.view().isFirstResponder()),
            "can_focus": self.session.as_ref().is_some_and(|s| s.editor.view().canBecomeFirstResponder()),
            "key_window": self.content.window().is_some_and(|window| window.isKeyWindow()),
            "content_focus": self.content.isFirstResponder(),
            "has_marked": self.session.as_ref().is_some_and(|s| s.editor.input().markedTextRange().is_some()),
            "keyboard_top": self.keyboard_top(),
            "last_geometry_query": geometry.map(|q| json!({ "range": q.range, "rect": q.rect })),
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
