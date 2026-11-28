//! The rendering view translates UIKit's text protocol into shared editor inputs.

use super::view::{NativeWindow, ViewState};
use crate::os::{ActiveEditorView, PlatformEvent, PlatformKey, PointerPhase, WindowHostEvent};
use crate::text_bridge::{HostTextBridge, native_range, utf16_offset, utf16_range};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSComparisonResult, NSDictionary, NSNotification, NSNotificationCenter, NSNumber,
    NSPoint, NSRange, NSRect, NSSet, NSSize, NSString,
};
use objc2_quartz_core::CAMetalLayer;
use objc2_ui_kit::{
    NSUnderlineStyle, NSUnderlineStyleAttributeName, NSWritingDirection, UIEvent, UIKeyCommand, UIKeyInput, UIKeyModifierFlags,
    UIKeyboardDidChangeFrameNotification, UIKeyboardType, UIKeyboardWillChangeFrameNotification, UIKeyboardWillHideNotification, UIPress,
    UIPressPhase, UIPressesEvent, UIReturnKeyType, UITextAutocapitalizationType, UITextAutocorrectionType, UITextContentTypeEmailAddress,
    UITextContentTypeNewPassword, UITextContentTypeOneTimeCode, UITextContentTypePassword, UITextContentTypeTelephoneNumber,
    UITextContentTypeURL, UITextContentTypeUsername, UITextInput, UITextInputDelegate, UITextInputStringTokenizer, UITextInputTokenizer,
    UITextInputTraits, UITextLayoutDirection, UITextPosition, UITextRange, UITextSelectionRect, UITextStorageDirection, UITouch, UIView,
};
use octara::EditorRope;
use octara_model::geometry::{Point, Rect};
use octara_model::input::{EditorCommand, Key, KeyEvent, Modifiers};
use octara_model::text::{
    AutoCapitalize, Autocorrect, ClauseStyle, InputTraits, KeyboardType, MarkedClause, ReturnKeyType, TextContentType, TextEdit,
};
use serde_json::{Value, json};
use smallvec::SmallVec;
use std::cell::{Cell, RefCell};
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
    bridge: RefCell<HostTextBridge>,
    delegate: RefCell<Weak<ProtocolObject<dyn UITextInputDelegate>>>,
    marked_style: RefCell<Option<Retained<NSDictionary<NSAttributedStringKey, AnyObject>>>>,
    // UIKit can hide its selection independently of the editor's anchor and caret.
    selection_hidden: Cell<bool>,
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
            self.editor().bridge.borrow().session().is_some()
        }

        #[unsafe(method_id(keyCommands))]
        fn key_commands(&self) -> Option<Retained<NSArray<UIKeyCommand>>> {
            self.offers_editor_keys().then(|| {
                let input = NSString::from_str("z");
                // SAFETY: The actions are implemented below with a single sender argument.
                let (undo, redo) = unsafe {
                    (
                        UIKeyCommand::keyCommandWithInput_modifierFlags_action(&input, UIKeyModifierFlags::Command, sel!(undo:), self.mtm()),
                        UIKeyCommand::keyCommandWithInput_modifierFlags_action(&input, UIKeyModifierFlags::Command | UIKeyModifierFlags::Shift, sel!(redo:), self.mtm()),
                    )
                };
                undo.setWantsPriorityOverSystemBehavior(true);
                redo.setWantsPriorityOverSystemBehavior(true);
                NSArray::from_retained_slice(&[undo, redo])
            })
        }

        #[unsafe(method(undo:))]
        fn undo_action(&self, _: Option<&AnyObject>) {
            self.route_editor_key(Key::Char('z'), Modifiers::META);
        }

        #[unsafe(method(redo:))]
        fn redo_action(&self, _: Option<&AnyObject>) {
            self.route_editor_key(Key::Char('z'), Modifiers::META | Modifiers::SHIFT);
        }

        #[unsafe(method(paste:))]
        fn paste_action(&self, _: Option<&AnyObject>) {
            self.command(EditorCommand::Paste);
        }

        #[unsafe(method(copy:))]
        fn copy_action(&self, _: Option<&AnyObject>) {
            self.command(EditorCommand::Copy);
        }

        #[unsafe(method(cut:))]
        fn cut_action(&self, _: Option<&AnyObject>) {
            self.command(EditorCommand::Cut);
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _: Option<&AnyObject>) {
            self.command(EditorCommand::SelectAll);
        }

        #[unsafe(method(canPerformAction:withSender:))]
        fn can_perform_action(&self, action: Sel, sender: Option<&AnyObject>) -> bool {
            if action == sel!(paste:) {
                self.canBecomeFirstResponder() && super::has_paste_items()
            } else if action == sel!(copy:) || action == sel!(cut:) {
                self.has_copyable_selection()
            } else if action == sel!(selectAll:) {
                self.document_length() > 0
            } else if action == sel!(undo:) || action == sel!(redo:) {
                self.offers_editor_keys()
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
            self.editor().bridge.borrow().session().is_some_and(|session| session.secure)
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
            self.document_length() != 0
        }

        #[unsafe(method(insertText:))]
        fn insert_text(&self, text: &NSString) {
            let text = text.to_string();
            if matches!(text.as_str(), "\n" | "\r") && !self.editor().bridge.borrow().is_composing() {
                self.route_editor_key(Key::Enter, Modifiers::empty());
            } else {
                self.edit(TextEdit::Insert { text, replacement: None });
            }
        }

        #[unsafe(method(deleteBackward))]
        fn backspace(&self) {
            let bridge = self.editor().bridge.borrow();
            let Some(session) = bridge.session() else { return };
            let Some(composition) = session.editor().composition() else {
                drop(bridge);
                self.command(EditorCommand::DeleteBackward);
                return;
            };
            let edit = delete_marked_backward(session.rope(), composition.marked(), session.selected_range(), composition.clauses());
            drop(bridge);
            if let Some(edit) = edit {
                self.edit(edit);
            }
        }
    }

    unsafe impl UITextInput for ContentView {
        #[unsafe(method_id(textInRange:))]
        fn text_in_range(&self, range: &UITextRange) -> Option<Retained<NSString>> {
            let bridge = self.editor().bridge.borrow();
            bridge.session().and_then(|session| {
                let rope = session.rope();
                rope.slice(utf16_range(rope, offsets(range)?)?).map(|text| NSString::from_str(&text))
            })
        }

        #[unsafe(method(replaceRange:withText:))]
        fn replace_range(&self, range: &UITextRange, text: &NSString) {
            if let Some(replacement) = offsets(range).and_then(|range| self.byte_range(range)) {
                self.edit(TextEdit::Insert { text: text.to_string(), replacement: Some(replacement) });
            }
        }

        #[unsafe(method_id(selectedTextRange))]
        fn selected_range(&self) -> Option<Retained<UITextRange>> {
            let editor = self.editor();
            editor.bridge.borrow().session().filter(|_| !editor.selection_hidden.get()).and_then(|session| {
                self.range(session.rope(), session.selected_range()?)
            })
        }

        #[unsafe(method(setSelectedTextRange:))]
        fn set_selection(&self, range: Option<&UITextRange>) {
            let Some(range) = range else {
                self.editor().selection_hidden.set(true);
                return;
            };
            let Some(range) = offsets(range).and_then(|range| self.byte_range(range)) else { return };
            self.editor().selection_hidden.set(false);
            let mut bridge = self.editor().bridge.borrow_mut();
            if bridge.session().and_then(|session| session.selected_range()).as_ref() != Some(&range)
                && bridge.select((range.start, range.end))
            {
                self.wake_editor();
            }
        }

        #[unsafe(method_id(markedTextRange))]
        fn marked_range(&self) -> Option<Retained<UITextRange>> {
            let bridge = self.editor().bridge.borrow();
            bridge.session().and_then(|session| self.range(session.rope(), session.marked_range()?))
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
            self.mark(text.map(ToString::to_string).unwrap_or_default(), selected, None);
        }

        #[unsafe(method(setAttributedMarkedText:selectedRange:))]
        fn set_attributed_marked(&self, text: Option<&NSAttributedString>, selected: NSRange) {
            self.mark(text.map(|text| text.string().to_string()).unwrap_or_default(), selected, text);
        }

        #[unsafe(method(unmarkText))]
        fn unmark(&self) {
            self.edit(TextEdit::Unmark);
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
        fn writing_direction(&self, native: &UITextPosition, direction: UITextStorageDirection) -> NSWritingDirection {
            let bridge = self.editor().bridge.borrow();
            let rtl = (|| {
                let session = bridge.session()?;
                if session.secure {
                    return None;
                }
                let rope = session.rope();
                let byte = utf16_offset(rope, position(native)?);
                let backward = direction == UITextStorageDirection::Backward;
                let edge = rope.grapheme_edge(byte, backward).or_else(|| rope.grapheme_edge(byte, !backward))?;
                session.selection_rects(byte.min(edge)..byte.max(edge)).first().map(|part| part.rtl)
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
                .and_then(|(range, offset)| {
                    let end = range.end.min(self.document_length());
                    (range.start <= end).then(|| Position::make(self.mtm(), offset.clamp(range.start, end)))
                })
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
        self.editor().bridge.borrow().session().map_or_else(InputTraits::default, |session| session.traits)
    }

    fn position_offset(&self, native: &UITextPosition, offset: isize) -> Option<Retained<UITextPosition>> {
        let offset = position(native)?.checked_add_signed(offset)?;
        (offset <= self.document_length()).then(|| Position::make(self.mtm(), offset))
    }

    fn grapheme_range(&self, offset: usize, backward: bool) -> Option<Retained<UITextRange>> {
        let bridge = self.editor().bridge.borrow();
        let rope = bridge.session()?.rope();
        let start = utf16_offset(rope, offset);
        let end = rope.grapheme_edge(start, backward)?;
        self.range(rope, start.min(end)..start.max(end))
    }

    fn document_length(&self) -> usize {
        self.editor().bridge.borrow().session().map_or(0, |session| session.rope().len_utf16())
    }

    fn range(&self, rope: &EditorRope, range: Range<usize>) -> Option<Retained<UITextRange>> {
        Some(TextRange::make(self.mtm(), native_range(rope, range)?))
    }

    fn byte_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        let bridge = self.editor().bridge.borrow();
        utf16_range(bridge.session()?.rope(), range)
    }

    fn has_copyable_selection(&self) -> bool {
        let editor = self.editor();
        !editor.selection_hidden.get()
            && editor
                .bridge
                .borrow()
                .session()
                .is_some_and(|session| !session.secure && session.selected_range().is_some_and(|range| !range.is_empty()))
    }

    fn wake_editor(&self) {
        self.ivars().sender.send_event(PlatformEvent::UserWake);
    }

    fn edit(&self, edit: TextEdit) {
        if self.editor().bridge.borrow_mut().edit(edit) {
            self.editor().selection_hidden.set(false);
            self.wake_editor();
        }
    }

    fn command(&self, command: EditorCommand) {
        if self.editor().bridge.borrow_mut().command(command) {
            if matches!(command, EditorCommand::SelectAll | EditorCommand::DeleteBackward) {
                self.editor().selection_hidden.set(false);
            }
            self.wake_editor();
        }
    }

    fn route_editor_key(&self, key: Key, modifiers: Modifiers) {
        if !self.offers_editor_keys() {
            return;
        }
        let event = PlatformKey { event: Some(KeyEvent::new(key, modifiers)), text: None };
        super::app::send_key(PlatformEvent::Key { window: self.ivars().id, event });
    }

    fn offers_editor_keys(&self) -> bool {
        let bridge = self.editor().bridge.borrow();
        bridge.session().is_some() && !bridge.is_composing()
    }

    fn mark(&self, text: String, selected: NSRange, attributed: Option<&NSAttributedString>) {
        if text.is_empty() {
            self.edit(TextEdit::Cancel);
            return;
        }
        let rope = EditorRope::new(&text);
        let length = rope.len_utf16();
        let start = selected.location.min(length);
        let end = start.saturating_add(selected.length).min(length);
        let selection = utf16_range(&rope, start..end);
        let style = self.editor().marked_style.borrow();
        // SAFETY: This key is an immutable UIKit constant.
        let underline = unsafe { NSUnderlineStyleAttributeName };
        let default_style = style.as_ref().and_then(|style| style.objectForKey(underline)).as_deref().and_then(clause_style);
        let mut clauses = SmallVec::new();
        if let Some(attributed) = attributed {
            let mut index = 0;
            while index < attributed.length() {
                let mut run = NSRange::new(0, 0);
                // SAFETY: The index is within the native string and the range is writable.
                let value = unsafe { attributed.attribute_atIndex_effectiveRange(underline, index, &mut run) };
                let end = run.location + run.length;
                let range = utf16_offset(&rope, index)..utf16_offset(&rope, end);
                let style = value.as_deref().and_then(clause_style).or(default_style);
                append_clauses(&mut clauses, range, selection.as_ref(), style);
                index = end;
            }
        } else {
            append_clauses(&mut clauses, 0..rope.len(), selection.as_ref(), default_style);
        }
        self.edit(TextEdit::Mark { text, selection, replacement: None, clauses });
    }

    fn project(&self, range: Range<usize>) -> NSRect {
        let bridge = self.editor().bridge.borrow();
        let Some(session) = bridge.session() else { return NSRect::ZERO };
        let Some(range) = utf16_range(session.rope(), range) else { return NSRect::ZERO };
        let rect = if session.secure { session.caret } else { session.first_rect(range).map(|(_, rect)| rect) };
        rect.map_or(NSRect::ZERO, to_native_rect)
    }

    fn project_selection(&self, offsets: Range<usize>) -> Retained<NSArray<UITextSelectionRect>> {
        let bridge = self.editor().bridge.borrow();
        let Some(session) = bridge.session() else { return NSArray::new() };
        let Some(range) = utf16_range(session.rope(), offsets) else { return NSArray::new() };
        if session.secure || range.is_empty() {
            return NSArray::new();
        }
        let rects: Vec<_> = session
            .selection_rects(range.clone())
            .into_iter()
            .map(|part| {
                SelectionRect::make(
                    self.mtm(),
                    SelectionState {
                        rect: to_native_rect(part.rect),
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
        let bridge = self.editor().bridge.borrow();
        let session = bridge.session()?;
        let rope = session.rope();
        let closest = (!session.secure)
            .then(|| session.closest_character_index_at(Point::new(point.x as f32, point.y as f32)))
            .flatten()
            .or_else(|| session.selection().map(|(_, focus)| focus))
            .unwrap_or_else(|| rope.len());
        rope.byte_to_utf16(closest)
    }

    fn sync_editor(&self, input: &ActiveEditorView<'_>) {
        let changes = self.editor().bridge.borrow().sync_changes(Some(input));
        let text_changed = changes.text_changed || changes.composition_changed;
        let delegate = self.editor().delegate.borrow().load();
        let native = Some(ProtocolObject::from_ref(self));
        if let Some(delegate) = &delegate {
            if text_changed {
                delegate.textWillChange(native);
            }
            if changes.selection_changed {
                delegate.selectionWillChange(native);
            }
        }
        self.editor().bridge.borrow_mut().sync(Some(input));
        if changes.selection_changed {
            self.editor().selection_hidden.set(false);
        }
        if let Some(delegate) = &delegate {
            if changes.selection_changed {
                delegate.selectionDidChange(native);
            }
            if text_changed {
                delegate.textDidChange(native);
            }
        }
    }
}

fn delete_marked_backward(
    rope: &EditorRope,
    marked: Range<usize>,
    selection: Option<Range<usize>>,
    clauses: &[MarkedClause],
) -> Option<TextEdit> {
    let selected = selection.clone().unwrap_or(marked.end..marked.end);
    let start =
        if selected.is_empty() && selected.start <= marked.end { rope.grapheme_edge(selected.start, true)? } else { selected.start };
    let deleted = start.max(marked.start)..selected.end.min(marked.end);
    if deleted.is_empty() {
        return None;
    }
    if deleted == marked {
        return Some(TextEdit::Cancel);
    }

    let deleted = deleted.start - marked.start..deleted.end - marked.start;
    let mut text = rope.slice(marked)?;
    text.replace_range(deleted.clone(), "");
    let shift = |offset: usize| offset - offset.saturating_sub(deleted.start).min(deleted.len());
    let clauses = clauses
        .iter()
        .filter_map(|clause| {
            let range = shift(clause.range.start)..shift(clause.range.end);
            (!range.is_empty()).then_some(MarkedClause { range, style: clause.style })
        })
        .collect();
    Some(TextEdit::Mark { text, selection: selection.map(|_| deleted.start..deleted.start), replacement: None, clauses })
}

fn clause_style(value: &AnyObject) -> Option<ClauseStyle> {
    let value = value.downcast_ref::<NSNumber>()?.integerValue();
    let style = NSUnderlineStyle::from_bits_retain(value);
    if style.contains(NSUnderlineStyle::Thick) || style.contains(NSUnderlineStyle::Double) {
        Some(ClauseStyle::ActiveClause)
    } else if style.contains(NSUnderlineStyle::Single) {
        Some(ClauseStyle::Clause)
    } else {
        None
    }
}

// UIKit supplies one marked-text style for a plain string. The selected part
// identifies the active clause when the input method does not supply styled runs.
fn append_clauses(
    clauses: &mut SmallVec<[MarkedClause; 4]>,
    range: Range<usize>,
    selected: Option<&Range<usize>>,
    style: Option<ClauseStyle>,
) {
    if range.is_empty() {
        return;
    }
    let selected = selected.filter(|selected| !selected.is_empty());
    if let Some(selected) = selected.filter(|_| style != Some(ClauseStyle::ActiveClause)) {
        let active = range.start.max(selected.start)..range.end.min(selected.end);
        if !active.is_empty() {
            for (range, style) in [
                (range.start..active.start, ClauseStyle::Clause),
                (active.clone(), ClauseStyle::ActiveClause),
                (active.end..range.end, ClauseStyle::Clause),
            ] {
                if !range.is_empty() {
                    clauses.push(MarkedClause { range, style });
                }
            }
            return;
        }
    }
    if let Some(style) = style.or(selected.map(|_| ClauseStyle::Clause)) {
        clauses.push(MarkedClause { range, style });
    }
}

pub(super) struct TextInput {
    content: Retained<ContentView>,
    observer: Retained<KeyboardObserver>,
    active: bool,
    loaded_traits: InputTraits,
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
        Self { content, observer, active: true, loaded_traits: InputTraits::default() }
    }

    pub(super) fn sync(&mut self, input: Option<&ActiveEditorView<'_>>) {
        let Some(input) = input.filter(|_| self.active) else {
            self.retire();
            return;
        };
        let same_session = {
            let bridge = self.content.editor().bridge.borrow();
            bridge.session().is_some_and(|session| session.id == input.session && session.secure == input.secure)
        };
        if same_session {
            self.content.sync_editor(input);
            let traits = self.content.traits();
            if traits != self.loaded_traits && !self.is_composing() {
                self.loaded_traits = traits;
                self.content.reloadInputViews();
            }
        } else {
            self.retire();
            self.content.editor().selection_hidden.set(false);
            self.content.editor().bridge.borrow_mut().sync(Some(input));
            self.loaded_traits = self.content.traits();
        }
        self.focus();
    }

    fn retire(&mut self) {
        if self.content.canBecomeFirstResponder() {
            self.content.resignFirstResponder();
            let editor = self.content.editor();
            editor.bridge.borrow_mut().clear();
            editor.marked_style.borrow_mut().take();
            editor.selection_hidden.set(false);
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

    pub(super) fn drain(&self, out: &mut Vec<WindowHostEvent>) {
        self.content.editor().bridge.borrow_mut().drain(out);
    }

    pub(super) fn is_composing(&self) -> bool {
        self.content.editor().bridge.borrow().is_composing()
    }

    pub(super) fn is_focused(&self) -> bool {
        self.content.isFirstResponder()
    }

    pub(super) fn inspect(&self) -> Value {
        let bridge = self.content.editor().bridge.borrow();
        json!({
            "allowed": bridge.session().is_some(),
            "native_focus": self.content.isFirstResponder(),
            "key_window": self.content.window().is_some_and(|window| window.isKeyWindow()),
            "has_marked": bridge.is_composing(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marked_backspace_deletes_one_grapheme_and_preserves_clauses() {
        let rope = EditorRope::new("Aか👩‍🚀なZ");
        let clauses =
            [MarkedClause { range: 0..14, style: ClauseStyle::ActiveClause }, MarkedClause { range: 14..17, style: ClauseStyle::Clause }];
        assert_eq!(
            delete_marked_backward(&rope, 1..18, Some(15..15), &clauses),
            Some(TextEdit::Mark {
                text: "かな".into(),
                selection: Some(3..3),
                replacement: None,
                clauses: [
                    MarkedClause { range: 0..3, style: ClauseStyle::ActiveClause },
                    MarkedClause { range: 3..6, style: ClauseStyle::Clause },
                ]
                .into_iter()
                .collect(),
            })
        );
    }

    #[test]
    fn marked_backspace_clips_selection_and_cancels_an_empty_mark() {
        let rope = EditorRope::new("AかなZ");
        assert_eq!(
            delete_marked_backward(&rope, 1..7, Some(0..4), &[]),
            Some(TextEdit::Mark { text: "な".into(), selection: Some(0..0), replacement: None, clauses: SmallVec::new() })
        );
        assert_eq!(delete_marked_backward(&rope, 1..7, Some(1..1), &[]), None);
        assert_eq!(delete_marked_backward(&rope, 1..7, Some(8..8), &[]), None);
        assert_eq!(delete_marked_backward(&rope, 1..7, Some(0..8), &[]), Some(TextEdit::Cancel));
    }
}
