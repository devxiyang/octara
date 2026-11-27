//! The rendering view is the UIKit text client. Native draft changes are queued
//! as guarded editor proposals and reconciled with the retained document.

use super::draft::{self, Change, Draft, PendingInputs, byte_offset};
use super::view::{NativeWindow, ViewState};
use crate::os::{ActiveEditorView, PlatformEvent, PointerPhase};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{
    NSArray, NSAttributedStringKey, NSComparisonResult, NSDictionary, NSNotification, NSNotificationCenter, NSPoint, NSRange, NSRect,
    NSSet, NSSize, NSString,
};
use objc2_quartz_core::CAMetalLayer;
use objc2_ui_kit::{
    NSWritingDirection, UIEvent, UIKeyInput, UIKeyboardDidChangeFrameNotification, UIKeyboardType, UIKeyboardWillChangeFrameNotification,
    UIKeyboardWillHideNotification, UIPress, UIPressPhase, UIPressesEvent, UIReturnKeyType, UITextAutocapitalizationType,
    UITextAutocorrectionType, UITextContentTypeEmailAddress, UITextContentTypeNewPassword, UITextContentTypeOneTimeCode,
    UITextContentTypePassword, UITextContentTypeTelephoneNumber, UITextContentTypeURL, UITextContentTypeUsername, UITextInput,
    UITextInputDelegate, UITextInputStringTokenizer, UITextInputTokenizer, UITextInputTraits, UITextLayoutDirection, UITextPosition,
    UITextRange, UITextSelectionRect, UITextStorageDirection, UITouch, UIView,
};
use octara::runtime::Input;
use octara_model::ask::{ClipboardItem, ClipboardKinds};
use octara_model::geometry::Rect;
use octara_model::identity::EditorSessionId;
use octara_model::input::{HostReply, PasteEvent};
use octara_model::text::{AutoCapitalize, Autocorrect, InputTraits, KeyboardType, ReturnKeyType, TextContentType, TextGeometry};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::ops::Range;

define_class!(
    #[unsafe(super(UITextPosition))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraTextPosition"]
    #[ivars = usize]
    struct Position;

    unsafe impl NSObjectProtocol for Position {}
);

impl Position {
    fn make(main: MainThreadMarker, offset: usize) -> Retained<UITextPosition> {
        // SAFETY: Initialize UIKit's abstract position with a concrete UTF-16 offset.
        let position: Retained<Self> = unsafe { msg_send![super(main.alloc::<Self>().set_ivars(offset)), init] };
        Retained::into_super(position)
    }
}

fn position(native: &UITextPosition) -> Option<usize> {
    Some(*native.downcast_ref::<Position>()?.ivars())
}

define_class!(
    #[unsafe(super(UITextRange))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraTextRange"]
    #[ivars = Range<usize>]
    struct TextRange;

    unsafe impl NSObjectProtocol for TextRange {}

    impl TextRange {
        #[unsafe(method_id(start))]
        fn start_position(&self) -> Retained<UITextPosition> {
            Position::make(self.mtm(), self.ivars().start)
        }

        #[unsafe(method_id(end))]
        fn end_position(&self) -> Retained<UITextPosition> {
            Position::make(self.mtm(), self.ivars().end)
        }

        #[unsafe(method(isEmpty))]
        fn empty(&self) -> bool {
            self.ivars().is_empty()
        }
    }
);

impl TextRange {
    fn make(main: MainThreadMarker, offsets: Range<usize>) -> Retained<UITextRange> {
        // SAFETY: Initialize UIKit's abstract range with a concrete UTF-16 range.
        let range: Retained<Self> = unsafe { msg_send![super(main.alloc::<Self>().set_ivars(offsets)), init] };
        Retained::into_super(range)
    }
}

fn offsets(native: &UITextRange) -> Option<Range<usize>> {
    Some(native.downcast_ref::<TextRange>()?.ivars().clone())
}

struct SelectionState {
    rect: NSRect,
    rtl: bool,
    start: bool,
    end: bool,
}

define_class!(
    #[unsafe(super(UITextSelectionRect))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraTextSelectionRect"]
    #[ivars = SelectionState]
    struct SelectionRect;

    unsafe impl NSObjectProtocol for SelectionRect {}

    impl SelectionRect {
        #[unsafe(method(rect))]
        fn rect(&self) -> NSRect {
            self.ivars().rect
        }

        #[unsafe(method(writingDirection))]
        fn direction(&self) -> NSWritingDirection {
            if self.ivars().rtl { NSWritingDirection::RightToLeft } else { NSWritingDirection::LeftToRight }
        }

        #[unsafe(method(containsStart))]
        fn contains_start(&self) -> bool {
            self.ivars().start
        }

        #[unsafe(method(containsEnd))]
        fn contains_end(&self) -> bool {
            self.ivars().end
        }

        #[unsafe(method(isVertical))]
        fn vertical(&self) -> bool {
            false
        }
    }
);

impl SelectionRect {
    fn make(main: MainThreadMarker, state: SelectionState) -> Retained<UITextSelectionRect> {
        // SAFETY: Initialize the selection rectangle on UIKit's thread with view coordinates.
        let rect: Retained<Self> = unsafe { msg_send![super(main.alloc::<Self>().set_ivars(state)), init] };
        Retained::into_super(rect)
    }
}

#[derive(Default)]
pub(super) struct EditorState {
    session: RefCell<Option<Session>>,
    delegate: RefCell<Weak<ProtocolObject<dyn UITextInputDelegate>>>,
    marked_style: RefCell<Option<Retained<NSDictionary<NSAttributedStringKey, AnyObject>>>>,
}

struct Session {
    id: EditorSessionId,
    rev: u64,
    displayed: Draft,
    expected: Draft,
    committed: Draft,
    pending: PendingInputs,
    inflight: Option<Change>,
    geometry: Option<TextGeometry>,
    caret: Option<Rect>,
    traits: InputTraits,
    secure: bool,
}

define_class!(
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraView"]
    #[ivars = ViewState]
    pub(super) struct ContentView;

    unsafe impl NSObjectProtocol for ContentView {}

    impl ContentView {
        #[unsafe(method(layerClass))]
        fn layer_class() -> &'static AnyClass {
            CAMetalLayer::class()
        }

        #[unsafe(method(layoutSubviews))]
        fn layout(&self) {
            // SAFETY: Call UIView's layout before reading the resulting bounds.
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            self.publish_geometry();
        }

        #[unsafe(method(didMoveToWindow))]
        fn moved_to_window(&self) {
            // SAFETY: Preserve UIView's window-attachment callback.
            let _: () = unsafe { msg_send![super(self), didMoveToWindow] };
            if let Some(scene) = self.window().and_then(|window| window.windowScene()) {
                let scale = scene.screen().scale();
                if self.contentScaleFactor() != scale {
                    self.setContentScaleFactor(scale);
                }
            }
            self.publish_geometry();
        }

        #[unsafe(method(safeAreaInsetsDidChange))]
        fn safe_area_changed(&self) {
            // SAFETY: Preserve UIView's safe-area callback.
            let _: () = unsafe { msg_send![super(self), safeAreaInsetsDidChange] };
            self.refresh_viewport();
        }

        #[unsafe(method(touchesBegan:withEvent:))]
        fn began(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            self.touches(touches, event, PointerPhase::Started);
        }

        #[unsafe(method(touchesMoved:withEvent:))]
        fn moved(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            self.touches(touches, event, PointerPhase::Moved);
        }

        #[unsafe(method(touchesEnded:withEvent:))]
        fn ended(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            self.touches(touches, event, PointerPhase::Ended);
        }

        #[unsafe(method(touchesCancelled:withEvent:))]
        fn cancelled(&self, touches: &NSSet<UITouch>, event: Option<&UIEvent>) {
            self.touches(touches, event, PointerPhase::Cancelled);
        }

        #[unsafe(method(pressesBegan:withEvent:))]
        fn presses_began(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            self.route_presses(presses, UIPressPhase::Began, |presses| {
                // SAFETY: UIKit's text system handles unconsumed keys for this responder.
                unsafe { msg_send![super(self), pressesBegan: presses, withEvent: event] }
            });
        }

        #[unsafe(method(pressesEnded:withEvent:))]
        fn presses_ended(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            self.route_presses(presses, UIPressPhase::Ended, |presses| {
                // SAFETY: Forward UIKit's matching release.
                unsafe { msg_send![super(self), pressesEnded: presses, withEvent: event] }
            });
        }

        #[unsafe(method(pressesCancelled:withEvent:))]
        fn presses_cancelled(&self, presses: &NSSet<UIPress>, event: Option<&UIPressesEvent>) {
            self.route_presses(presses, UIPressPhase::Cancelled, |presses| {
                // SAFETY: Forward UIKit's matching cancellation.
                unsafe { msg_send![super(self), pressesCancelled: presses, withEvent: event] }
            });
        }

        #[unsafe(method(isAccessibilityElement))]
        fn accessible(&self) -> bool {
            // The platform bridge supplies the semantic elements for this view.
            false
        }

        #[unsafe(method_id(accessibilityElements))]
        fn accessibility_elements(&self) -> Retained<NSArray> {
            self.ivars().accessibility_elements.borrow().clone().unwrap_or_default()
        }

        #[unsafe(method(setAccessibilityElements:))]
        fn set_accessibility_elements(&self, elements: Option<&NSArray>) {
            *self.ivars().accessibility_elements.borrow_mut() = elements.map(Message::retain);
        }

        #[unsafe(method(accessibilityElementCount))]
        fn accessibility_element_count(&self) -> isize {
            self.ivars().accessibility_elements.borrow().as_ref().map_or(0, |elements| elements.count() as isize)
        }

        #[unsafe(method_id(accessibilityElementAtIndex:))]
        fn accessibility_element(&self, index: isize) -> Option<Retained<AnyObject>> {
            self.ivars().accessibility_elements.borrow().as_ref().and_then(|elements| {
                usize::try_from(index).ok().filter(|&i| i < elements.count()).map(|i| elements.objectAtIndex(i))
            })
        }

        #[unsafe(method(indexOfAccessibilityElement:))]
        fn index_of_accessibility_element(&self, element: &AnyObject) -> isize {
            self.ivars().accessibility_elements.borrow().as_ref().and_then(|elements| {
                elements.iter().position(|entry| std::ptr::eq(&*entry, element)).map(|i| i as isize)
            }).unwrap_or(isize::MAX)
        }

        #[unsafe(method(canBecomeFirstResponder))]
        fn can_focus(&self) -> bool {
            self.editor().session.borrow().is_some()
        }

        #[unsafe(method(paste:))]
        fn paste_action(&self, _: Option<&AnyObject>) {
            let items = super::read_clipboard(ClipboardKinds::all());
            if items.is_empty() {
                return;
            }
            if let Some(session) = self.editor().session.borrow_mut().as_mut() {
                session.pending.record_paste(PasteEvent { items });
                self.wake_editor();
            }
        }

        #[unsafe(method(copy:))]
        fn copy_action(&self, _: Option<&AnyObject>) {
            if let Some(value) = self.selected_value() {
                let _ = super::write_clipboard(&[ClipboardItem::Text(value)]);
            }
        }

        #[unsafe(method(cut:))]
        fn cut_action(&self, _: Option<&AnyObject>) {
            if let Some(value) = self.selected_value()
                && super::write_clipboard(&[ClipboardItem::Text(value)]).is_ok()
            {
                self.insert("");
            }
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _: Option<&AnyObject>) {
            self.edit(|draft| draft.selection = Some((0, draft.text.len())));
        }

        #[unsafe(method(canPerformAction:withSender:))]
        fn can_perform_action(&self, action: Sel, sender: Option<&AnyObject>) -> bool {
            if action == sel!(paste:) {
                self.canBecomeFirstResponder() && super::has_paste_items()
            } else if action == sel!(copy:) || action == sel!(cut:) {
                self.selected_value().is_some()
            } else if action == sel!(selectAll:) {
                self.document_length() > 0
            } else {
                // SAFETY: Forward validation of UIKit's other responder actions.
                let result: Bool = unsafe { msg_send![super(self), canPerformAction: action, withSender: sender] };
                result.as_bool()
            }
        }
    }

    unsafe impl UITextInputTraits for ContentView {
        #[unsafe(method(keyboardType))]
        fn keyboard_type(&self) -> UIKeyboardType {
            match self.traits().keyboard_type {
                KeyboardType::Default => UIKeyboardType::Default,
                KeyboardType::Email => UIKeyboardType::EmailAddress,
                KeyboardType::Number => UIKeyboardType::NumberPad,
                KeyboardType::Decimal => UIKeyboardType::DecimalPad,
                KeyboardType::Phone => UIKeyboardType::PhonePad,
                KeyboardType::Url => UIKeyboardType::URL,
                KeyboardType::Search => UIKeyboardType::WebSearch,
            }
        }

        #[unsafe(method(returnKeyType))]
        fn return_key(&self) -> UIReturnKeyType {
            match self.traits().return_key {
                ReturnKeyType::Default => UIReturnKeyType::Default,
                ReturnKeyType::Done => UIReturnKeyType::Done,
                ReturnKeyType::Go => UIReturnKeyType::Go,
                ReturnKeyType::Next => UIReturnKeyType::Next,
                ReturnKeyType::Search => UIReturnKeyType::Search,
                ReturnKeyType::Send => UIReturnKeyType::Send,
            }
        }

        #[unsafe(method(autocapitalizationType))]
        fn autocapitalize(&self) -> UITextAutocapitalizationType {
            match self.traits().autocapitalize.unwrap_or_default() {
                AutoCapitalize::Sentences => UITextAutocapitalizationType::Sentences,
                AutoCapitalize::Words => UITextAutocapitalizationType::Words,
                AutoCapitalize::Characters => UITextAutocapitalizationType::AllCharacters,
                AutoCapitalize::None => UITextAutocapitalizationType::None,
            }
        }

        #[unsafe(method(autocorrectionType))]
        fn autocorrect(&self) -> UITextAutocorrectionType {
            match self.traits().autocorrect {
                Autocorrect::Default => UITextAutocorrectionType::Default,
                Autocorrect::On => UITextAutocorrectionType::Yes,
                Autocorrect::Off => UITextAutocorrectionType::No,
            }
        }

        #[unsafe(method(isSecureTextEntry))]
        fn secure(&self) -> bool {
            self.editor().session.borrow().as_ref().is_some_and(|session| session.secure)
        }

        #[unsafe(method_id(textContentType))]
        fn content_type(&self) -> Option<Retained<NSString>> {
            // SAFETY: UIKit exports these immutable content type constants.
            let native = unsafe {
                match self.traits().content_type {
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
            native.map(Message::retain)
        }
    }

    unsafe impl UIKeyInput for ContentView {
        #[unsafe(method(hasText))]
        fn has_text(&self) -> bool {
            self.editor().session.borrow().as_ref().is_some_and(|session| !session.displayed.text.is_empty())
        }

        #[unsafe(method(insertText:))]
        fn insert_text(&self, text: &NSString) {
            let text = text.to_string();
            if matches!(text.as_str(), "\n" | "\r") {
                let mut session = self.editor().session.borrow_mut();
                let Some(session) = session.as_mut() else { return };
                if session.displayed.marked.is_none() {
                    session.pending.record_return();
                    self.wake_editor();
                    return;
                }
            }
            self.insert(&text);
        }

        #[unsafe(method(deleteBackward))]
        fn backspace(&self) {
            self.edit(|draft| {
                let mut range = selection(draft);
                if range.is_empty() {
                    range.start = draft.text.grapheme_edge(range.start, true).unwrap_or(range.start);
                }
                if !range.is_empty() {
                    replace(draft, range, "");
                }
            });
        }
    }

    unsafe impl UITextInput for ContentView {
        #[unsafe(method_id(textInRange:))]
        fn text_in_range(&self, range: &UITextRange) -> Option<Retained<NSString>> {
            self.editor().session.borrow().as_ref().zip(offsets(range)).and_then(|(session, range)| {
                let draft = &session.displayed;
                draft.text.slice(bytes(draft, range)).map(|text| NSString::from_str(&text))
            })
        }

        #[unsafe(method(replaceRange:withText:))]
        fn replace_range(&self, range: &UITextRange, text: &NSString) {
            if let Some(range) = offsets(range) {
                let text = text.to_string();
                self.edit(|draft| replace(draft, bytes(draft, range), &text));
            }
        }

        #[unsafe(method_id(selectedTextRange))]
        fn selected_range(&self) -> Option<Retained<UITextRange>> {
            self.editor().session.borrow().as_ref().and_then(|session| {
                let draft = &session.displayed;
                let (a, b) = draft.selection?;
                self.range(draft, a..b)
            })
        }

        #[unsafe(method(setSelectedTextRange:))]
        fn set_selection(&self, range: Option<&UITextRange>) {
            let range = range.and_then(offsets);
            self.edit(|draft| {
                draft.selection = range.map(|range| {
                    let range = bytes(draft, range);
                    (range.start, range.end)
                });
            });
        }

        #[unsafe(method_id(markedTextRange))]
        fn marked_range(&self) -> Option<Retained<UITextRange>> {
            self.editor().session.borrow().as_ref().and_then(|session| {
                let draft = &session.displayed;
                self.range(draft, draft.marked.clone()?)
            })
        }

        #[unsafe(method_id(markedTextStyle))]
        fn marked_style(&self) -> Option<Retained<NSDictionary<NSAttributedStringKey, AnyObject>>> {
            self.editor().marked_style.borrow().clone()
        }

        #[unsafe(method(setMarkedTextStyle:))]
        unsafe fn set_marked_style(&self, style: Option<&NSDictionary<NSAttributedStringKey, AnyObject>>) {
            *self.editor().marked_style.borrow_mut() = style.map(Message::retain);
        }

        #[unsafe(method(setMarkedText:selectedRange:))]
        fn set_marked(&self, text: Option<&NSString>, selected: NSRange) {
            let text = text.map(ToString::to_string).unwrap_or_default();
            self.edit(|draft| {
                let range = draft.marked.clone().unwrap_or_else(|| selection(draft));
                let start = range.start;
                replace(draft, range, &text);
                let end = start + text.len();
                draft.marked = (start < end).then_some(start..end);
                let start_utf16 = draft.text.byte_to_utf16(start).unwrap_or(0);
                let end_utf16 = draft.text.byte_to_utf16(end).unwrap_or(start_utf16);
                let a = start_utf16.saturating_add(selected.location).min(end_utf16);
                let b = a.saturating_add(selected.length).min(end_utf16);
                draft.selection = Some((byte_offset(&draft.text, a), byte_offset(&draft.text, b)));
            });
        }

        #[unsafe(method(unmarkText))]
        fn unmark(&self) {
            self.edit(|draft| draft.marked = None);
        }

        #[unsafe(method_id(beginningOfDocument))]
        fn begin_document(&self) -> Retained<UITextPosition> {
            Position::make(self.mtm(), 0)
        }

        #[unsafe(method_id(endOfDocument))]
        fn end_document(&self) -> Retained<UITextPosition> {
            Position::make(self.mtm(), self.document_length())
        }

        #[unsafe(method_id(textRangeFromPosition:toPosition:))]
        fn text_range(&self, from: &UITextPosition, to: &UITextPosition) -> Option<Retained<UITextRange>> {
            position(from).zip(position(to)).and_then(|(a, b)| {
                (a.max(b) <= self.document_length()).then(|| TextRange::make(self.mtm(), a.min(b)..a.max(b)))
            })
        }

        #[unsafe(method_id(positionFromPosition:offset:))]
        fn offset_position(&self, native: &UITextPosition, offset: isize) -> Option<Retained<UITextPosition>> {
            self.position_offset(native, offset)
        }

        #[unsafe(method_id(positionFromPosition:inDirection:offset:))]
        fn offset_direction(&self, native: &UITextPosition, direction: UITextLayoutDirection, offset: isize) -> Option<Retained<UITextPosition>> {
            self.position_offset(native, if backward(direction) { offset.saturating_neg() } else { offset })
        }

        #[unsafe(method(comparePosition:toPosition:))]
        fn compare(&self, from: &UITextPosition, to: &UITextPosition) -> NSComparisonResult {
            use std::cmp::Ordering;
            match position(from).cmp(&position(to)) {
                Ordering::Less => NSComparisonResult::Ascending,
                Ordering::Equal => NSComparisonResult::Same,
                Ordering::Greater => NSComparisonResult::Descending,
            }
        }

        #[unsafe(method(offsetFromPosition:toPosition:))]
        fn distance(&self, from: &UITextPosition, to: &UITextPosition) -> isize {
            (position(to).unwrap_or_default() as isize) - (position(from).unwrap_or_default() as isize)
        }

        #[unsafe(method_id(inputDelegate))]
        fn input_delegate(&self) -> Option<Retained<ProtocolObject<dyn UITextInputDelegate>>> {
            self.editor().delegate.borrow().load()
        }

        #[unsafe(method(setInputDelegate:))]
        fn set_delegate(&self, delegate: Option<&ProtocolObject<dyn UITextInputDelegate>>) {
            *self.editor().delegate.borrow_mut() = delegate.map(Weak::new).unwrap_or_default();
        }

        #[unsafe(method_id(tokenizer))]
        fn tokenizer(&self) -> Retained<ProtocolObject<dyn UITextInputTokenizer>> {
            // SAFETY: This view implements UITextInput and is a UIResponder.
            let tokenizer = unsafe { UITextInputStringTokenizer::initWithTextInput(self.mtm().alloc(), self) };
            ProtocolObject::from_retained(tokenizer)
        }

        #[unsafe(method_id(positionWithinRange:farthestInDirection:))]
        fn farthest(&self, range: &UITextRange, direction: UITextLayoutDirection) -> Option<Retained<UITextPosition>> {
            offsets(range).map(|range| Position::make(self.mtm(), if backward(direction) { range.start } else { range.end }))
        }

        #[unsafe(method_id(characterRangeByExtendingPosition:inDirection:))]
        fn extend_character(&self, native: &UITextPosition, direction: UITextLayoutDirection) -> Option<Retained<UITextRange>> {
            position(native).and_then(|offset| self.grapheme_range(offset, backward(direction)))
        }

        #[unsafe(method(baseWritingDirectionForPosition:inDirection:))]
        fn writing_direction(&self, native: &UITextPosition, _: UITextStorageDirection) -> NSWritingDirection {
            let session = self.editor().session.borrow();
            let rtl = (|| {
                let session = session.as_ref()?;
                let byte = byte_offset(&session.displayed.text, position(native)?);
                let geometry = session.geometry.as_ref()?;
                geometry.lines.iter().flat_map(|line| line.fragments.iter()).find(|part| part.range.contains(&byte)).map(|part| part.rtl)
            })();
            match rtl {
                Some(true) => NSWritingDirection::RightToLeft,
                Some(false) => NSWritingDirection::LeftToRight,
                None => NSWritingDirection::Natural,
            }
        }

        #[unsafe(method(setBaseWritingDirection:forRange:))]
        fn set_writing_direction(&self, _: NSWritingDirection, _: &UITextRange) {}

        #[unsafe(method(firstRectForRange:))]
        fn first_rect(&self, range: &UITextRange) -> NSRect {
            offsets(range).map_or(NSRect::ZERO, |range| self.project(range))
        }

        #[unsafe(method(caretRectForPosition:))]
        fn caret_rect(&self, native: &UITextPosition) -> NSRect {
            let Some(at) = position(native) else { return NSRect::ZERO };
            let mut rect = self.project(at..at);
            rect.size.width = 1.0;
            rect
        }

        #[unsafe(method_id(selectionRectsForRange:))]
        fn selection_rects(&self, range: &UITextRange) -> Retained<NSArray<UITextSelectionRect>> {
            offsets(range).map_or_else(NSArray::new, |range| self.project_selection(range))
        }

        #[unsafe(method_id(closestPositionToPoint:))]
        fn closest_position(&self, point: NSPoint) -> Option<Retained<UITextPosition>> {
            self.text_offset_at(point).map(|offset| Position::make(self.mtm(), offset))
        }

        #[unsafe(method_id(closestPositionToPoint:withinRange:))]
        fn closest_in_range(&self, point: NSPoint, range: &UITextRange) -> Option<Retained<UITextPosition>> {
            offsets(range).zip(self.text_offset_at(point))
                .map(|(range, offset)| Position::make(self.mtm(), offset.clamp(range.start, range.end)))
        }

        #[unsafe(method_id(characterRangeAtPoint:))]
        fn character_range(&self, point: NSPoint) -> Option<Retained<UITextRange>> {
            self.text_offset_at(point).and_then(|offset| self.grapheme_range(offset, false))
        }

        #[unsafe(method_id(textInputView))]
        fn input_view(&self) -> Retained<UIView> {
            Retained::into_super(self.retain())
        }
    }
);

fn backward(direction: UITextLayoutDirection) -> bool {
    matches!(direction, UITextLayoutDirection::Left | UITextLayoutDirection::Up)
}

fn selection(draft: &Draft) -> Range<usize> {
    let (start, end) = draft.selection.unwrap_or((draft.text.len(), draft.text.len()));
    start..end
}

fn bytes(draft: &Draft, range: Range<usize>) -> Range<usize> {
    byte_offset(&draft.text, range.start)..byte_offset(&draft.text, range.end)
}

fn replace(draft: &mut Draft, range: Range<usize>, text: &str) {
    let end = range.start + text.len();
    if draft.text.replace(range, text) {
        draft.selection = Some((end, end));
        draft.marked = None;
    }
}

impl ContentView {
    fn route_presses(&self, presses: &NSSet<UIPress>, phase: UIPressPhase, forward: impl FnOnce(&NSSet<UIPress>)) {
        if let Some(window) = self.window().and_then(|window| window.downcast::<NativeWindow>().ok()) {
            window.route_presses(presses, phase, forward);
        } else {
            forward(presses);
        }
    }

    fn editor(&self) -> &EditorState {
        &self.ivars().editor
    }

    fn traits(&self) -> InputTraits {
        self.editor().session.borrow().as_ref().map_or_else(InputTraits::default, |session| session.traits)
    }

    fn position_offset(&self, native: &UITextPosition, offset: isize) -> Option<Retained<UITextPosition>> {
        let offset = position(native)?.checked_add_signed(offset)?;
        (offset <= self.document_length()).then(|| Position::make(self.mtm(), offset))
    }

    fn grapheme_range(&self, offset: usize, backward: bool) -> Option<Retained<UITextRange>> {
        let session = self.editor().session.borrow();
        let draft = &session.as_ref()?.displayed;
        let start = byte_offset(&draft.text, offset);
        let end = draft.text.grapheme_edge(start, backward)?;
        self.range(draft, start.min(end)..start.max(end))
    }

    fn document_length(&self) -> usize {
        self.editor().session.borrow().as_ref().map_or(0, |session| session.displayed.text.len_utf16())
    }

    fn range(&self, draft: &Draft, range: Range<usize>) -> Option<Retained<UITextRange>> {
        let (start, end) = (draft.text.byte_to_utf16(range.start)?, draft.text.byte_to_utf16(range.end)?);
        Some(TextRange::make(self.mtm(), start..end))
    }

    fn selected_value(&self) -> Option<String> {
        let session = self.editor().session.borrow();
        let session = session.as_ref()?;
        let range = selection(&session.displayed);
        (!session.secure && !range.is_empty()).then(|| session.displayed.text.slice(range)).flatten()
    }

    fn wake_editor(&self) {
        self.ivars().sender.send_event(PlatformEvent::UserWake);
    }

    fn edit(&self, edit: impl FnOnce(&mut Draft)) {
        let mut session = self.editor().session.borrow_mut();
        if let Some(session) = session.as_mut() {
            let previous = session.displayed.clone();
            edit(&mut session.displayed);
            if session.displayed != previous {
                session.pending.record(session.displayed.clone());
                self.wake_editor();
            }
        }
    }

    fn insert(&self, text: &str) {
        self.edit(|draft| {
            let range = draft.marked.clone().unwrap_or_else(|| selection(draft));
            replace(draft, range, text);
        });
    }

    fn project(&self, range: Range<usize>) -> NSRect {
        let session = self.editor().session.borrow();
        let Some(session) = session.as_ref() else { return NSRect::ZERO };
        let draft = &session.displayed;
        let range = bytes(draft, range);
        let at_caret = range.is_empty() && selection(draft).end == range.start;
        let geometry = session.geometry.as_ref().filter(|_| draft.text == session.expected.text);
        let caret = at_caret.then_some(session.caret).flatten();
        let rect = caret.or_else(|| geometry.and_then(|geometry| first_rect(geometry, range.start, range.end))).or(session.caret);
        rect.map_or(NSRect::ZERO, to_native_rect)
    }

    fn project_selection(&self, offsets: Range<usize>) -> Retained<NSArray<UITextSelectionRect>> {
        let session = self.editor().session.borrow();
        let Some(session) = session.as_ref() else { return NSArray::new() };
        let Some(geometry) = session.geometry.as_ref().filter(|_| session.displayed.text == session.expected.text) else {
            return NSArray::new();
        };
        let range = bytes(&session.displayed, offsets);
        let fragments = geometry.lines.iter().flat_map(|line| line.fragments.iter());
        let rects: Vec<_> = fragments
            .filter(|part| range.start < part.range.end && range.end > part.range.start && !range.is_empty())
            .map(|part| {
                SelectionRect::make(
                    self.mtm(),
                    SelectionState {
                        rect: to_native_rect(Rect::new(
                            geometry.origin.x + part.rect.x,
                            geometry.origin.y + part.rect.y,
                            part.rect.w,
                            part.rect.h,
                        )),
                        rtl: part.rtl,
                        start: part.range.contains(&range.start),
                        end: part.range.start < range.end && range.end <= part.range.end,
                    },
                )
            })
            .collect();
        NSArray::from_retained_slice(&rects)
    }

    fn text_offset_at(&self, point: NSPoint) -> Option<usize> {
        let session = self.editor().session.borrow();
        let session = session.as_ref()?;
        let draft = &session.displayed;
        let mut closest = selection(draft).end;
        if let Some(geometry) = session.geometry.as_ref().filter(|_| draft.text == session.expected.text) {
            let x = point.x as f32 - geometry.origin.x;
            let y = point.y as f32 - geometry.origin.y;
            if let Some(line) = geometry.lines.iter().min_by(|a, b| {
                let distance = |rect: Rect| (rect.y - y).max(y - rect.bottom()).max(0.0);
                distance(a.rect).total_cmp(&distance(b.rect))
            }) {
                closest = line
                    .fragments
                    .iter()
                    .flat_map(|part| {
                        let (left, right) = if part.rtl { (part.range.end, part.range.start) } else { (part.range.start, part.range.end) };
                        [(part.rect.x, left), (part.rect.right(), right)]
                    })
                    .min_by(|a, b| (a.0 - x).abs().total_cmp(&(b.0 - x).abs()))
                    .map_or(line.range.start, |(_, byte)| byte);
            }
        }
        draft.text.byte_to_utf16(closest)
    }

    fn install(&self, input: &ActiveEditorView<'_>) {
        let expected = Draft::from_editor(input);
        *self.editor().session.borrow_mut() = Some(Session {
            id: input.session,
            rev: input.document_rev,
            displayed: expected.clone(),
            expected,
            committed: Draft::committed(input),
            pending: PendingInputs::default(),
            inflight: None,
            traits: input.editor.input_traits,
            geometry: input.geometry.cloned().filter(|_| !input.secure),
            caret: input.caret,
            secure: input.secure,
        });
    }

    fn sync_session(&self, input: &ActiveEditorView<'_>) -> bool {
        let mut current = self.editor().session.borrow_mut();
        let session = current.as_mut().expect("active UIKit editor");
        let updated = input.document_rev != session.rev;
        if updated {
            session.expected = Draft::from_editor(input);
            session.committed = Draft::committed(input);
            session.rev = input.document_rev;
        }
        let rejected = match session.inflight.take() {
            Some(Change::Document(native)) => native != session.expected,
            Some(Change::Return | Change::Paste(_)) | None => updated,
        };
        if rejected {
            session.pending.clear();
        }
        session.geometry = input.geometry.cloned().filter(|_| !input.secure);
        session.caret = input.caret;
        let reload = session.traits != input.editor.input_traits && session.expected.marked.is_none() && session.displayed.marked.is_none();
        if reload {
            session.traits = input.editor.input_traits;
        }
        reload
    }

    fn reconcile(&self) {
        let change = self.editor().session.borrow().as_ref().and_then(|session| {
            (session.pending.is_empty() && session.displayed != session.expected).then(|| {
                let text_changed = session.displayed.text != session.expected.text || session.displayed.marked != session.expected.marked;
                let selection_changed = session.displayed.selection != session.expected.selection;
                (session.expected.clone(), text_changed, selection_changed)
            })
        });
        let Some((next, text_changed, selection_changed)) = change else { return };
        let delegate = self.editor().delegate.borrow().load();
        let input = Some(ProtocolObject::from_ref(self));
        if let Some(delegate) = &delegate {
            if text_changed {
                delegate.textWillChange(input);
            }
            if selection_changed {
                delegate.selectionWillChange(input);
            }
        }
        if let Some(session) = self.editor().session.borrow_mut().as_mut() {
            session.displayed = next;
        }
        if let Some(delegate) = &delegate {
            if selection_changed {
                delegate.selectionDidChange(input);
            }
            if text_changed {
                delegate.textDidChange(input);
            }
        }
    }
}

pub(super) struct TextInput {
    content: Retained<ContentView>,
    observer: Retained<KeyboardObserver>,
    active: bool,
}

impl TextInput {
    pub(super) fn new(content: Retained<ContentView>) -> Self {
        let view = Weak::new(&*content);
        let refresh: Box<dyn Fn()> = Box::new(move || {
            if let Some(view) = view.load() {
                view.refresh_viewport();
            }
        });
        // SAFETY: Register a main-thread observer with a selector accepting NSNotification.
        let observer: Retained<KeyboardObserver> = unsafe {
            let observer: Retained<KeyboardObserver> = msg_send![super(content.mtm().alloc::<KeyboardObserver>().set_ivars(refresh)), init];
            let center = NSNotificationCenter::defaultCenter();
            for name in [UIKeyboardWillChangeFrameNotification, UIKeyboardDidChangeFrameNotification, UIKeyboardWillHideNotification] {
                center.addObserver_selector_name_object(&observer, sel!(keyboardChanged:), Some(name), None);
            }
            observer
        };
        Self { content, observer, active: true }
    }

    pub(super) fn sync(&mut self, input: Option<&ActiveEditorView<'_>>) {
        let Some(input) = input.filter(|_| self.active) else {
            self.retire();
            return;
        };
        let same_session = self
            .content
            .editor()
            .session
            .borrow()
            .as_ref()
            .is_some_and(|session| session.id == input.session && session.secure == input.secure);
        if same_session {
            let reload = self.content.sync_session(input);
            self.content.reconcile();
            if reload {
                self.content.reloadInputViews();
            }
        } else {
            self.retire();
            self.content.install(input);
        }
        self.focus();
    }

    fn retire(&mut self) {
        if self.content.editor().session.borrow().is_some() {
            self.content.resignFirstResponder();
            self.content.editor().session.borrow_mut().take();
            if self.active
                && let Some(window) = self.content.window().filter(|window| window.isKeyWindow())
            {
                window.becomeFirstResponder();
            }
        }
    }

    pub(super) fn focus(&self) {
        if self.active && self.content.canBecomeFirstResponder() && !self.content.isFirstResponder() {
            self.content.becomeFirstResponder();
        }
    }

    pub(super) fn set_active(&mut self, active: bool) {
        self.active = active;
        if !active {
            self.retire();
        }
    }

    pub(super) fn take(&mut self) -> Option<Input> {
        let result = (|| {
            let mut current = self.content.editor().session.borrow_mut();
            let session = current.as_mut()?;
            if session.inflight.is_some() {
                return None;
            }
            loop {
                let change = session.pending.take()?;
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
                        session.pending.clear();
                        return None;
                    }
                }
            }
        })();
        if result.is_none() {
            self.content.reconcile();
        }
        result
    }

    pub(super) fn is_composing(&self) -> bool {
        self.content.editor().session.borrow().as_ref().is_some_and(|session| session.displayed.marked.is_some())
    }

    pub(super) fn is_focused(&self) -> bool {
        self.content.isFirstResponder()
    }

    pub(super) fn inspect(&self) -> Value {
        let session = self.content.editor().session.borrow();
        json!({
            "allowed": session.is_some(),
            "native_focus": self.content.isFirstResponder(),
            "key_window": self.content.window().is_some_and(|window| window.isKeyWindow()),
            "has_marked": session.as_ref().is_some_and(|session| session.displayed.marked.is_some()),
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

fn to_native_rect(rect: Rect) -> NSRect {
    NSRect::new(NSPoint::new(f64::from(rect.x), f64::from(rect.y)), NSSize::new(f64::from(rect.w), f64::from(rect.h)))
}
