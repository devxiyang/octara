//! UIKit owns the soft keyboard and marked draft. The Rust editor receives the
//! resulting document at the next safe event-loop boundary.

use super::draft::{self, Change, Draft, PendingInputs, byte_offset};
use crate::os::ActiveEditorView;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint, NSRange, NSRect, NSSize, NSString};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIColor, UIControlEvents, UIEdgeInsets, UIEvent, UIKeyboardDidChangeFrameNotification, UIKeyboardType,
    UIKeyboardWillChangeFrameNotification, UIKeyboardWillHideNotification, UIReturnKeyType, UIScrollViewDelegate,
    UITextAutocapitalizationType, UITextAutocorrectionType, UITextContentTypeEmailAddress, UITextContentTypeNewPassword,
    UITextContentTypeOneTimeCode, UITextContentTypePassword, UITextContentTypeTelephoneNumber, UITextContentTypeURL,
    UITextContentTypeUsername, UITextField, UITextFieldDelegate, UITextInput, UITextInputTraits, UITextPosition, UITextRange, UITextView,
    UITextViewDelegate, UIView,
};
use octara::EditorRope;
use octara::runtime::Input;
use octara_model::ask::ClipboardKinds;
use octara_model::geometry::Rect;
use octara_model::identity::EditorSessionId;
use octara_model::input::{HostReply, PasteEvent};
use octara_model::text::{AutoCapitalize, Autocorrect, InputTraits, KeyboardType, ReturnKeyType, TextContentType, TextGeometry};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

struct Signals {
    wake: Rc<dyn Fn()>,
    pending: RefCell<PendingInputs>,
    muted: Cell<bool>,
    mutating: Cell<bool>,
}

impl Signals {
    fn changed(&self, read: impl FnOnce() -> Draft) {
        if !self.mutating.get() {
            self.capture(read);
        }
    }

    fn capture(&self, read: impl FnOnce() -> Draft) {
        if self.muted.get() {
            return;
        }
        let draft = read();
        let wake = self.pending.borrow().is_empty();
        self.pending.borrow_mut().record(draft);
        if wake {
            (self.wake)();
        }
    }

    /// A native mark can produce delegate calls before UIKit publishes its effective range.
    fn composition_changed(&self, mutate: impl FnOnce(), read: impl FnOnce() -> Draft) {
        let outer = self.mutating.replace(true);
        mutate();
        self.mutating.set(outer);
        self.capture(read);
    }

    fn return_pressed(&self) {
        if !self.muted.get() {
            let wake = self.pending.borrow().is_empty();
            self.pending.borrow_mut().record_return();
            if wake {
                (self.wake)();
            }
        }
    }

    fn paste(&self, read: impl FnOnce() -> Draft) {
        if self.muted.get() {
            return;
        }
        let items = super::read_clipboard(ClipboardKinds::all());
        if !items.is_empty() {
            self.capture(read);
            self.pending.borrow_mut().record_paste(PasteEvent { items });
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
            self.ivars().changed(|| self.draft());
        }

        #[unsafe(method(paste:))]
        fn paste_action(&self, _: Option<&AnyObject>) {
            self.ivars().paste(|| self.draft());
        }

        #[unsafe(method(canPerformAction:withSender:))]
        fn can_perform_action(&self, action: Sel, sender: Option<&AnyObject>) -> bool {
            if action == sel!(paste:) {
                super::has_paste_items()
            } else {
                // SAFETY: Forward the inherited responder action validation unchanged.
                let allowed: Bool = unsafe { msg_send![super(self), canPerformAction: action, withSender: sender] };
                allowed.as_bool()
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:))]
        fn set_mark(&self, text: Option<&NSString>, selected: NSRange) {
            self.ivars().composition_changed(
                // SAFETY: Implementation of the inherited UITextInput selector.
                || unsafe { msg_send![super(self), setMarkedText: text, selectedRange: selected] },
                || self.draft(),
            );
        }

        #[unsafe(method(unmarkText))]
        fn unmark(&self) {
            self.ivars().composition_changed(
                // SAFETY: Implementation of the inherited UITextInput selector.
                || unsafe { msg_send![super(self), unmarkText] },
                || self.draft(),
            );
        }
    }

    unsafe impl NSObjectProtocol for Field {}

    unsafe impl UITextFieldDelegate for Field {
        #[unsafe(method(textFieldDidChangeSelection:))]
        fn selection_changed(&self, _: &UITextField) {
            self.ivars().changed(|| self.draft());
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

impl Field {
    fn draft(&self) -> Draft {
        read_draft(&**self, self.text().map(|s| s.to_string()).unwrap_or_default())
    }
}

struct AreaState {
    signals: Rc<Signals>,
    projection: RefCell<GeometryProjection>,
}

// objc2 stores these values inline in Objective-C ivars.
const _: () = assert!(std::mem::align_of::<AreaState>() <= 8);

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
            if self.markedTextRange().is_none() && text.length() == 1 && text.characterAtIndex(0) == 10 {
                self.ivars().signals.return_pressed();
            } else {
                // SAFETY: UITextView owns the native edit, including the marked-text session.
                let _: () = unsafe { msg_send![super(self), insertText: text] };
            }
        }

        #[unsafe(method(paste:))]
        fn paste_action(&self, _: Option<&AnyObject>) {
            self.ivars().signals.paste(|| self.draft());
        }

        #[unsafe(method(canPerformAction:withSender:))]
        fn can_perform_action(&self, action: Sel, sender: Option<&AnyObject>) -> bool {
            if action == sel!(paste:) {
                super::has_paste_items()
            } else {
                // SAFETY: Forward the inherited responder action validation unchanged.
                let allowed: Bool = unsafe { msg_send![super(self), canPerformAction: action, withSender: sender] };
                allowed.as_bool()
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:))]
        fn set_mark(&self, text: Option<&NSString>, selected: NSRange) {
            self.ivars().signals.composition_changed(
                // SAFETY: Implementation of the inherited UITextInput selector.
                || unsafe { msg_send![super(self), setMarkedText: text, selectedRange: selected] },
                || self.draft(),
            );
        }

        #[unsafe(method(unmarkText))]
        fn unmark(&self) {
            self.ivars().signals.composition_changed(
                // SAFETY: Implementation of the inherited UITextInput selector.
                || unsafe { msg_send![super(self), unmarkText] },
                || self.draft(),
            );
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
            self.ivars().signals.changed(|| self.draft());
        }

        #[unsafe(method(textViewDidChangeSelection:))]
        fn selection_changed(&self, _: &UITextView) {
            self.ivars().signals.changed(|| self.draft());
        }
    }
);

impl Area {
    fn draft(&self) -> Draft {
        read_draft(&**self, self.text().to_string())
    }

    /// Candidate and caret queries use the published text layout where it matches
    /// the native document. A fresh native draft can still use the last caret anchor.
    fn octara_rect(&self, start: &UITextPosition, end: &UITextPosition) -> Option<NSRect> {
        let parent = self.superview()?;
        let draft = self.draft();
        let start = position_byte(&**self, &draft.text, start);
        let end = position_byte(&**self, &draft.text, end);
        let projection = self.ivars().projection.borrow();
        let is_caret =
            draft.selection.is_some_and(|(_, focus)| focus == start) || draft.marked.as_ref().is_some_and(|marked| marked.start == start);
        let geometry = projection.geometry.as_ref().filter(|_| draft.text == projection.draft.text);
        let rect = if start == end && is_caret {
            projection.caret.or_else(|| geometry.and_then(|geometry| first_rect(geometry, start, end)))
        } else {
            geometry.and_then(|geometry| first_rect(geometry, start, end)).or_else(|| is_caret.then_some(projection.caret).flatten())
        }?;
        drop(projection);
        Some(self.textInputView().convertRect_fromView(to_native_rect(rect), Some(&parent)))
    }
}

struct GeometryProjection {
    draft: Rc<Draft>,
    geometry: Option<TextGeometry>,
    caret: Option<Rect>,
}

impl GeometryProjection {
    fn new(editor: &ActiveEditorView<'_>, draft: Rc<Draft>) -> Self {
        Self { draft, geometry: editor.geometry.cloned().filter(|_| !editor.secure), caret: editor.caret }
    }
}

fn first_rect(geometry: &TextGeometry, start: usize, end: usize) -> Option<Rect> {
    let index = geometry.lines.partition_point(|line| line.range.start <= start).saturating_sub(1);
    let line = geometry.lines.get(index)?;
    let caret = line.fragments.iter().find(|fragment| fragment.range.contains(&start)).map_or_else(
        || {
            line.fragments
                .iter()
                .find(|fragment| fragment.range.end == start)
                .map_or(line.rect.x, |fragment| if fragment.rtl { fragment.rect.x } else { fragment.rect.right() })
        },
        |fragment| if fragment.rtl { fragment.rect.right() } else { fragment.rect.x },
    );
    let bounds = line
        .fragments
        .iter()
        .filter(|fragment| start < fragment.range.end && end > fragment.range.start)
        .map(|fragment| (fragment.rect.x, fragment.rect.right()))
        .reduce(|(left, right), (x, edge)| (left.min(x), right.max(edge)));
    let (left, right) = if start == end { (caret, caret) } else { bounds.unwrap_or((caret, caret)) };
    Some(Rect::new(geometry.origin.x + left, geometry.origin.y + line.rect.y, right - left, line.rect.h))
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraKeyboardObserver"]
    #[ivars = Box<dyn Fn()>]
    struct KeyboardObserver;

    impl KeyboardObserver {
        #[unsafe(method(keyboardChanged:))]
        fn changed(&self, _: &NSNotification) {
            (self.ivars())();
        }
    }
);

fn position_byte(input: &(impl UITextInput + Message), text: &EditorRope, position: &UITextPosition) -> usize {
    let beginning = input.beginningOfDocument();
    byte_offset(text, input.offsetFromPosition_toPosition(&beginning, position).max(0) as usize)
}

fn read_draft(input: &(impl UITextInput + Message), text: String) -> Draft {
    let text = EditorRope::new(text);
    let offsets = |range: Retained<UITextRange>| position_byte(input, &text, &range.start())..position_byte(input, &text, &range.end());
    let marked = input.markedTextRange().map(offsets).filter(|r| !r.is_empty());
    let selection = input.selectedTextRange().map(offsets).map(|range| (range.start, range.end));
    Draft { text, marked, selection }
}

fn select(input: &(impl UITextInput + Message), text: &EditorRope, selection: Range<usize>) {
    let Some(start) = text.byte_to_utf16(selection.start) else { return };
    let Some(end) = text.byte_to_utf16(selection.end) else { return };
    let beginning = input.beginningOfDocument();
    let start = input.positionFromPosition_offset(&beginning, start as isize);
    let end = input.positionFromPosition_offset(&beginning, end as isize);
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

    fn apply_input_traits(&self, previous: InputTraits, traits: InputTraits) {
        let input = self.input();
        if previous.keyboard_type != traits.keyboard_type {
            input.setKeyboardType(match traits.keyboard_type {
                KeyboardType::Default => UIKeyboardType::Default,
                KeyboardType::Email => UIKeyboardType::EmailAddress,
                KeyboardType::Number => UIKeyboardType::NumberPad,
                KeyboardType::Decimal => UIKeyboardType::DecimalPad,
                KeyboardType::Phone => UIKeyboardType::PhonePad,
                KeyboardType::Url => UIKeyboardType::URL,
                KeyboardType::Search => UIKeyboardType::WebSearch,
            });
        }
        if previous.return_key != traits.return_key {
            input.setReturnKeyType(match traits.return_key {
                ReturnKeyType::Default => UIReturnKeyType::Default,
                ReturnKeyType::Done => UIReturnKeyType::Done,
                ReturnKeyType::Go => UIReturnKeyType::Go,
                ReturnKeyType::Next => UIReturnKeyType::Next,
                ReturnKeyType::Search => UIReturnKeyType::Search,
                ReturnKeyType::Send => UIReturnKeyType::Send,
            });
        }
        if previous.autocapitalize != traits.autocapitalize {
            input.setAutocapitalizationType(match traits.autocapitalize.unwrap_or_default() {
                AutoCapitalize::Sentences => UITextAutocapitalizationType::Sentences,
                AutoCapitalize::Words => UITextAutocapitalizationType::Words,
                AutoCapitalize::Characters => UITextAutocapitalizationType::AllCharacters,
                AutoCapitalize::None => UITextAutocapitalizationType::None,
            });
        }
        if previous.autocorrect != traits.autocorrect {
            input.setAutocorrectionType(match traits.autocorrect {
                Autocorrect::Default => UITextAutocorrectionType::Default,
                Autocorrect::On => UITextAutocorrectionType::Yes,
                Autocorrect::Off => UITextAutocorrectionType::No,
            });
        }
        if previous.content_type != traits.content_type {
            // SAFETY: These immutable UIKit constants are available on all supported iOS versions.
            let content_type = unsafe {
                match traits.content_type {
                    TextContentType::None => None,
                    TextContentType::Username => Some(UITextContentTypeUsername),
                    TextContentType::Password => Some(UITextContentTypePassword),
                    TextContentType::NewPassword => Some(UITextContentTypeNewPassword),
                    TextContentType::OneTimeCode => Some(UITextContentTypeOneTimeCode),
                    TextContentType::Email => Some(UITextContentTypeEmailAddress),
                    TextContentType::Url => Some(UITextContentTypeURL),
                    TextContentType::Telephone => Some(UITextContentTypeTelephoneNumber),
                }
            };
            input.setTextContentType(content_type);
        }
    }

    fn read(&self) -> Draft {
        match self {
            Self::Field(field) => field.draft(),
            Self::Area(area) => area.draft(),
        }
    }

    fn synchronize(&self, expected: &Draft, signals: &Signals) {
        let current = self.read();
        if &current == expected {
            return;
        }
        signals.muted.set(true);
        if current.text != expected.text || current.marked != expected.marked {
            self.input().unmarkText();
            let text = NSString::from_str(expected.text.as_str());
            match self {
                Self::Field(field) => field.setText(Some(&text)),
                Self::Area(area) => area.setText(Some(&text)),
            }
            if let Some(marked) = &expected.marked {
                select(self.input(), &expected.text, marked.clone());
                let text = NSString::from_str(&expected.text.as_str()[marked.clone()]);
                let (start, end) = expected.selection.unwrap_or((marked.end, marked.end));
                let marked_start = expected.text.byte_to_utf16(marked.start).expect("marked range begins at a character boundary");
                let range = |offset: usize| {
                    expected.text.byte_to_utf16(offset.clamp(marked.start, marked.end)).unwrap_or(marked_start) - marked_start
                };
                let begin = range(start);
                self.input().setMarkedText_selectedRange(Some(&text), NSRange::new(begin, range(end) - begin));
            }
        }
        if let Some((start, end)) = expected.selection
            && self.read().selection != expected.selection
        {
            select(self.input(), &expected.text, start..end);
        }
        signals.muted.set(false);
    }
}

struct Session {
    id: EditorSessionId,
    rev: u64,
    editor: Editor,
    signals: Rc<Signals>,
    committed: Draft,
    expected: Rc<Draft>,
    inflight: Option<Change>,
    multiline: bool,
    secure: bool,
    input_traits: InputTraits,
}

pub(super) struct TextInput {
    content: Retained<UIView>,
    observer: Retained<KeyboardObserver>,
    wake: Rc<dyn Fn()>,
    session: Option<Session>,
    active: bool,
}

impl TextInput {
    pub(super) fn new(content: Retained<UIView>, wake: impl Fn() + 'static, refresh: impl Fn() + 'static) -> Self {
        let mtm = MainThreadMarker::from(&*content);
        // SAFETY: NSObject initialization on the UIKit thread.
        let observer: Retained<KeyboardObserver> =
            unsafe { msg_send![super(mtm.alloc::<KeyboardObserver>().set_ivars(Box::new(refresh))), init] };
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: UIKit posts these notifications on the main thread and the selector accepts NSNotification.
        unsafe {
            center.addObserver_selector_name_object(&observer, sel!(keyboardChanged:), Some(UIKeyboardWillChangeFrameNotification), None);
            center.addObserver_selector_name_object(&observer, sel!(keyboardChanged:), Some(UIKeyboardDidChangeFrameNotification), None);
            center.addObserver_selector_name_object(&observer, sel!(keyboardChanged:), Some(UIKeyboardWillHideNotification), None);
        }
        Self { content, observer, wake: Rc::new(wake), session: None, active: true }
    }

    pub(super) fn sync(&mut self, input: Option<&ActiveEditorView<'_>>) {
        let Some(input) = input.filter(|_| self.active) else {
            self.retire();
            return;
        };
        let frame = self.anchor(input);
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.id != input.session || session.multiline != input.multiline || session.secure != input.secure)
        {
            self.retire();
        }
        let Some(session) = &mut self.session else {
            self.install(input, Rc::new(Draft::from_editor(input)), frame);
            return;
        };
        let updated = input.document_rev != session.rev;
        if updated {
            session.expected = Rc::new(Draft::from_editor(input));
            session.committed = Draft::committed(input);
            session.rev = input.document_rev;
        }
        let changed = match session.inflight.take() {
            Some(Change::Document(native)) => native != *session.expected,
            Some(Change::Return | Change::Paste(_)) | None => updated,
        };
        if changed {
            session.signals.pending.borrow_mut().clear();
        }

        if let Editor::Area(area) = &session.editor {
            *area.ivars().projection.borrow_mut() = GeometryProjection::new(input, session.expected.clone());
        }
        let view = session.editor.view();
        if view.frame() != frame {
            view.setFrame(frame);
        }
        if !view.isFirstResponder() {
            view.becomeFirstResponder();
        }
        if session.signals.pending.borrow().is_empty() {
            session.editor.synchronize(&session.expected, &session.signals);
            let traits = input.editor.input_traits;
            // Keep UIKit's marked text alive until the input method finishes editing it.
            if session.input_traits != traits && session.expected.marked.is_none() && session.editor.input().markedTextRange().is_none() {
                session.editor.apply_input_traits(session.input_traits, traits);
                session.input_traits = traits;
                view.reloadInputViews();
            }
        } else {
            (self.wake)();
        }
    }

    fn anchor(&self, input: &ActiveEditorView<'_>) -> NSRect {
        let bounds = self.content.bounds();
        if input.multiline {
            return bounds;
        }
        let caret = input.caret.unwrap_or_default();
        NSRect::new(
            NSPoint::new(f64::from(caret.x), f64::from(caret.y)),
            NSSize::new((bounds.size.width - f64::from(caret.x)).max(1.0), f64::from(caret.h.max(1.0))),
        )
    }

    fn install(&mut self, input: &ActiveEditorView<'_>, expected: Rc<Draft>, frame: NSRect) {
        let mtm = MainThreadMarker::from(&*self.content);
        let signals =
            Rc::new(Signals { wake: self.wake.clone(), pending: RefCell::default(), muted: Cell::new(true), mutating: Cell::new(false) });
        let clear = UIColor::clearColor();
        let editor = if input.multiline {
            let state = AreaState { signals: signals.clone(), projection: RefCell::new(GeometryProjection::new(input, expected.clone())) };
            // SAFETY: UITextView initialization on the UIKit thread with a default text container.
            let area: Retained<Area> =
                unsafe { msg_send![super(mtm.alloc::<Area>().set_ivars(state)), initWithFrame: frame, textContainer: None::<&AnyObject>] };
            // SAFETY: The view is its own delegate and lives for the duration of the session.
            unsafe { area.setDelegate(Some(ProtocolObject::from_ref(&*area))) };
            area.setScrollEnabled(false);
            area.setTextContainerInset(UIEdgeInsets { top: 0.0, left: 0.0, right: 0.0, bottom: 0.0 });
            area.setTextColor(Some(&clear));
            area.setSecureTextEntry(input.secure);
            Editor::Area(area)
        } else {
            // SAFETY: UITextField initialization on the UIKit thread with its view frame.
            let field: Retained<Field> = unsafe { msg_send![super(mtm.alloc::<Field>().set_ivars(signals.clone())), initWithFrame: frame] };
            field.setDelegate(Some(ProtocolObject::from_ref(&*field)));
            // SAFETY: The target remains alive and the editingChanged selector accepts no arguments.
            unsafe { field.addTarget_action_forControlEvents(Some(&field), sel!(editingChanged), UIControlEvents::EditingChanged) };
            field.setTextColor(Some(&clear));
            field.setSecureTextEntry(input.secure);
            Editor::Field(field)
        };
        let input_traits = input.editor.input_traits;
        editor.apply_input_traits(InputTraits::default(), input_traits);
        let view = editor.view();
        view.setBackgroundColor(Some(&clear));
        // SAFETY: The color is a UIKit UIColor on the main thread.
        unsafe { view.setTintColor(Some(&clear)) };
        view.setIsAccessibilityElement(false, mtm);
        self.content.addSubview(view);
        editor.synchronize(&expected, &signals);
        signals.muted.set(false);
        view.becomeFirstResponder();
        self.session = Some(Session {
            id: input.session,
            rev: input.document_rev,
            editor,
            signals,
            committed: Draft::committed(input),
            expected,
            inflight: None,
            multiline: input.multiline,
            secure: input.secure,
            input_traits,
        });
    }

    fn retire(&mut self) {
        if let Some(session) = self.session.take() {
            session.signals.muted.set(true);
            session.editor.view().resignFirstResponder();
            session.editor.view().removeFromSuperview();
            // UIKit can hand the keyboard back to winit's UIKeyInput view.
            if self.content.isFirstResponder() {
                self.content.resignFirstResponder();
            }
        }
    }

    pub(super) fn focus(&self) {
        if self.active
            && let Some(session) = &self.session
        {
            let view = session.editor.view();
            if !view.isFirstResponder() {
                view.becomeFirstResponder();
            }
        }
    }

    pub(super) fn set_active(&mut self, active: bool) {
        self.active = active;
        if !active {
            self.retire();
        }
    }

    pub(super) fn take(&mut self) -> Option<Input> {
        let session = self.session.as_mut()?;
        if session.inflight.is_some() {
            return None;
        }
        loop {
            let change = session.signals.pending.borrow_mut().take()?;
            if let Change::Paste(event) = change {
                return Some(Input::Host(HostReply::Paste(event)));
            }
            match draft::proposal(&change, &session.expected, &session.committed, session.id, session.rev) {
                Ok(Some(event)) => {
                    session.inflight = Some(change);
                    return Some(Input::Editor(event));
                }
                Ok(None) => {}
                Err(()) => {
                    session.signals.pending.borrow_mut().clear();
                    session.editor.synchronize(&session.expected, &session.signals);
                    return None;
                }
            }
        }
    }

    pub(super) fn inspect(&self) -> Value {
        json!({
            "allowed": self.session.is_some(),
            "native_focus": self.session.as_ref().is_some_and(|session| session.editor.view().isFirstResponder()),
            "key_window": self.content.window().is_some_and(|window| window.isKeyWindow()),
            "has_marked": self.session.as_ref().is_some_and(|session| session.expected.marked.is_some()),
            "keyboard_inset": super::scene::keyboard_inset(&self.content),
        })
    }
}

impl Drop for TextInput {
    fn drop(&mut self) {
        self.retire();
        // SAFETY: This is the observer registered by this UI-thread adapter.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(&self.observer) };
    }
}

fn to_native_rect(rect: Rect) -> NSRect {
    NSRect::new(NSPoint::new(f64::from(rect.x), f64::from(rect.y)), NSSize::new(f64::from(rect.w), f64::from(rect.h)))
}
