//! AppKit text services for the retained tree's focused editor.

use super::super::{ActiveEditorView, HostWake, PlatformKey, WindowHostEvent, WindowTextInput};
use super::view::OctaraView;
use crate::text_bridge::{BridgeSession, HostTextBridge, native_range, utf16_range};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSMarkedClauseSegmentAttributeName, NSTextInputContext, NSUnderlineStyle, NSUnderlineStyleAttributeName,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound, NSNumber, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString,
};
use octara::EditorRope;
use octara::tree::selection::paragraph_at;
use octara_model::geometry::Point;
use octara_model::identity::EditorSessionId;
use octara_model::input::EditorCommand;
use octara_model::text::{ClauseStyle, MarkedClause, TextEdit};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::mem::DropGuard;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyOutcome {
    Waiting,
    Handled,
    Declined,
}

pub(super) struct TextInput {
    bridge: RefCell<HostTextBridge>,
    context: RefCell<Option<Retained<NSTextInputContext>>>,
    wake: HostWake,
    key: Cell<Option<KeyOutcome>>,
    discarding: Cell<bool>,
    anchor: Cell<Option<NSRect>>,
}

impl TextInput {
    pub(super) fn new(wake: HostWake) -> Self {
        Self {
            bridge: RefCell::default(),
            context: RefCell::default(),
            wake,
            key: Cell::default(),
            discarding: Cell::default(),
            anchor: Cell::default(),
        }
    }

    fn record_key_outcome(&self, outcome: KeyOutcome) {
        if matches!(self.key.get(), Some(KeyOutcome::Waiting | KeyOutcome::Declined)) {
            self.key.set(Some(outcome));
        }
    }
}

impl OctaraView {
    pub(super) fn interpret_editor_key(&self, event: &NSEvent) -> bool {
        let input = &self.ivars().text_input;
        let Some(context) = self.editor_context() else { return false };
        let previous = input.key.replace(Some(KeyOutcome::Waiting));
        let _restore = DropGuard::new(previous, |previous| input.key.set(previous));
        let composing = self.composing();
        let consumed = context.handleEvent(event);
        if !consumed && input.key.get() == Some(KeyOutcome::Waiting) {
            self.interpretKeyEvents(&NSArray::from_slice(&[event]));
        }
        if input.key.get() == Some(KeyOutcome::Handled) || (consumed && input.key.get() == Some(KeyOutcome::Waiting)) {
            return true;
        }
        // AppKit normally gets these via Edit menu equivalents. Supply them for
        // applications that do not install those menu items.
        !composing && edit_shortcut(event).is_some_and(|command| self.editor_action(command))
    }

    pub(super) fn editor_context(&self) -> Option<Retained<NSTextInputContext>> {
        self.ivars().text_input.context.borrow().clone()
    }

    pub(super) fn editor_insert_text(&self, string: &AnyObject, replacement: NSRange) {
        let Some(text) = native_string(string) else { return };
        let input = &self.ivars().text_input;
        if matches!(text.as_str(), "\r" | "\n") && input.bridge.borrow().session().is_some_and(|session| !session.multiline) {
            input.record_key_outcome(KeyOutcome::Declined);
            return;
        }
        self.enqueue(TextEdit::Insert { text, replacement: self.replacement(replacement) });
    }

    pub(super) fn editor_do_command(&self, selector: Sel) {
        let input = &self.ivars().text_input;
        if input.key.get() == Some(KeyOutcome::Handled) {
            return;
        }
        let name = selector.name().to_bytes();
        if self.composing() {
            if name == b"cancelOperation:" {
                self.enqueue(TextEdit::Cancel);
                self.discard_composer();
            }
            return;
        }
        let handled =
            if let Some(command) = selector_command(name) { self.editor_action(command) } else { self.editor_paragraph_action(name) };
        if !handled {
            input.record_key_outcome(KeyOutcome::Declined);
        }
    }

    pub(super) fn editor_set_marked_text(&self, string: &AnyObject, selection: NSRange, replacement: NSRange) {
        let Some(text) = native_string(string) else { return };
        let rope = EditorRope::new(text.clone());
        let selection = replacement_range(&rope, selection);
        let clauses = string.downcast_ref::<NSAttributedString>().into_iter().flat_map(|attrs| marked_clauses(attrs, &rope)).collect();
        self.enqueue(TextEdit::Mark { text, selection, replacement: self.replacement(replacement), clauses });
    }

    pub(super) fn editor_unmark_text(&self) {
        self.enqueue(TextEdit::Unmark);
    }

    pub(super) fn editor_selected_range(&self) -> NSRange {
        self.editor_range(BridgeSession::selected_range)
    }

    pub(super) fn editor_marked_range(&self) -> NSRange {
        self.editor_range(BridgeSession::marked_range)
    }

    fn editor_range(&self, get: impl FnOnce(&BridgeSession) -> Option<Range<usize>>) -> NSRange {
        let bridge = self.ivars().text_input.bridge.borrow();
        bridge.session().and_then(|session| native_range(session.rope(), get(session)?)).map_or_else(not_found, ns_range)
    }

    pub(super) unsafe fn editor_substring(&self, proposed: NSRange, actual: NSRangePointer) -> Option<Retained<NSAttributedString>> {
        // SAFETY: AppKit provides writable storage or null.
        unsafe { write_range(actual, not_found()) };
        let (text, covered) = {
            let bridge = self.ivars().text_input.bridge.borrow();
            let session = bridge.session().filter(|session| !session.secure)?;
            let rope = session.rope();
            let requested = query_range(rope, proposed)?;
            (rope.slice(requested.clone())?, native_range(rope, requested)?)
        };
        unsafe { write_range(actual, ns_range(covered)) };
        Some(NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(&text)))
    }

    pub(super) unsafe fn editor_first_rect(&self, proposed: NSRange, actual: NSRangePointer) -> NSRect {
        unsafe { write_range(actual, not_found()) };
        let rect = {
            let bridge = self.ivars().text_input.bridge.borrow();
            bridge.session().and_then(|session| {
                let rope = session.rope();
                let (covered, rect) = session.first_rect(query_range(rope, proposed)?)?;
                Some((rect, native_range(rope, covered)?))
            })
        };
        let Some((position, covered)) = rect else { return NSRect::ZERO };
        let Some(window) = self.window() else { return NSRect::ZERO };
        let local =
            NSRect::new(NSPoint::new(position.x.into(), position.y.into()), NSSize::new(position.w.max(1.0).into(), position.h.into()));
        let screen = window.convertRectToScreen(self.convertRect_toView(local, None));
        self.ivars().text_input.anchor.set(Some(screen));
        unsafe { write_range(actual, ns_range(covered)) };
        screen
    }

    pub(super) fn editor_character_index(&self, point: NSPoint) -> usize {
        let Some(window) = self.window() else { return NSNotFound as usize };
        let local = self.convertPoint_fromView(window.convertPointFromScreen(point), None);
        let bridge = self.ivars().text_input.bridge.borrow();
        bridge
            .session()
            .and_then(|session| {
                let index = session.character_index_at(Point::new(local.x as f32, local.y as f32))?;
                native_range(session.rope(), index..index)
            })
            .map_or(NSNotFound as usize, |range| range.start)
    }

    fn replacement(&self, range: NSRange) -> Option<Range<usize>> {
        let bridge = self.ivars().text_input.bridge.borrow();
        replacement_range(bridge.session()?.rope(), range)
    }

    pub(super) fn editor_session(&self) -> Option<EditorSessionId> {
        self.ivars().text_input.bridge.borrow().session().map(|session| session.id)
    }

    pub(super) fn composing(&self) -> bool {
        self.ivars().text_input.bridge.borrow().is_composing()
    }

    fn enqueue(&self, edit: TextEdit) {
        let input = &self.ivars().text_input;
        input.record_key_outcome(KeyOutcome::Handled);
        if !input.discarding.get() && input.bridge.borrow_mut().edit(edit) {
            (input.wake)();
        }
    }

    pub(super) fn editor_action(&self, command: EditorCommand) -> bool {
        let input = &self.ivars().text_input;
        let mut bridge = input.bridge.borrow_mut();
        let Some(session) = bridge.session() else { return false };
        if command == EditorCommand::InsertNewline && !session.multiline {
            return false;
        }
        let queued = !bridge.is_composing() && bridge.command(command);
        drop(bridge);
        input.record_key_outcome(KeyOutcome::Handled);
        if queued {
            (input.wake)();
        }
        true
    }

    fn editor_paragraph_action(&self, selector: &[u8]) -> bool {
        let input = &self.ivars().text_input;
        let mut bridge = input.bridge.borrow_mut();
        let Some(session) = bridge.session() else { return false };
        let Some(selection) = session.selection() else { return false };
        let Some(change) = paragraph_change(selector, session.rope().as_str(), selection) else { return false };
        let queued = match change {
            ParagraphChange::Select(selection) => bridge.select(selection),
            ParagraphChange::Delete(range) => bridge.edit(TextEdit::Insert { text: String::new(), replacement: Some(range) }),
        };
        drop(bridge);
        input.record_key_outcome(KeyOutcome::Handled);
        if queued {
            (input.wake)();
        }
        true
    }

    fn discard_composer(&self) {
        let previous = self.ivars().text_input.discarding.replace(true);
        if let Some(context) = self.editor_context() {
            context.discardMarkedText();
        }
        self.ivars().text_input.discarding.set(previous);
    }
}

pub(super) struct Editor {
    view: Retained<OctaraView>,
    secure: bool,
}

impl Editor {
    pub(super) fn new(view: &OctaraView) -> Self {
        Self { view: view.retain(), secure: false }
    }

    fn sync(&mut self, active: Option<&ActiveEditorView<'_>>) {
        let input = &self.view.ivars().text_input;
        let composing = self.view.composing();
        let outcome = input.bridge.borrow_mut().sync(active);
        let secure = active.is_some_and(|active| active.secure) && self.view.window().is_some_and(|window| window.isKeyWindow());
        self.set_secure(secure);
        let input = &self.view.ivars().text_input;
        if outcome.session_changed
            || outcome.rejected
            || (composing && (outcome.text_changed || outcome.selection_changed || outcome.composition_changed))
        {
            self.view.discard_composer();
            input.anchor.set(None);
        }
        if outcome.session_changed {
            let context = active.map(|_| {
                NSTextInputContext::initWithClient(NSTextInputContext::alloc(self.view.mtm()), ProtocolObject::from_ref(&*self.view))
            });
            let old = input.context.replace(context);
            if self.is_focused() {
                if let Some(old) = old {
                    old.deactivate();
                }
                if let Some(context) = self.view.editor_context() {
                    context.activate();
                }
            }
        }
        if let Some(context) = self.view.editor_context() {
            context.invalidateCharacterCoordinates();
            if outcome.selection_changed {
                context.textInputClientDidUpdateSelection();
            }
        }
    }

    fn set_focused(&mut self, active: bool) {
        let secure = self.view.ivars().text_input.bridge.borrow().session().is_some_and(|session| session.secure);
        self.set_secure(active && secure);
    }

    fn set_secure(&mut self, requested: bool) {
        if self.secure == requested {
            return;
        }
        // SAFETY: These APIs take no parameters and the enable is balanced on deactivate/drop.
        self.secure = unsafe {
            if requested {
                EnableSecureEventInput() == 0
            } else {
                DisableSecureEventInput();
                false
            }
        };
    }

    fn is_focused(&self) -> bool {
        self.view
            .window()
            .and_then(|window| window.firstResponder())
            .is_some_and(|responder| std::ptr::eq(Retained::as_ptr(&responder).cast::<OctaraView>(), &*self.view))
    }

    fn inspect(&self) -> Value {
        let native_focus = self.is_focused();
        let bridge = self.view.ivars().text_input.bridge.borrow();
        json!({
            "active": bridge.session().is_some(),
            "secure": self.secure,
            "native_focus": native_focus,
            "composing": bridge.is_composing(),
            "pending": bridge.pending_len(),
            "screen_anchor": self.view.ivars().text_input.anchor.get().map(|r| [r.origin.x, r.origin.y, r.size.width, r.size.height]),
        })
    }
}

impl WindowTextInput for Editor {
    fn sync_text_input(&mut self, active: Option<&ActiveEditorView<'_>>) {
        self.sync(active);
    }

    fn focused(&mut self, active: bool) {
        self.set_focused(active);
    }

    fn is_composing(&self) -> bool {
        self.view.composing()
    }

    fn on_unhandled_key(&mut self, key: &PlatformKey) -> bool {
        self.view.interpret_pending_key(key)
    }

    fn drain_text_input(&mut self, out: &mut Vec<WindowHostEvent>) {
        self.view.ivars().text_input.bridge.borrow_mut().drain(out);
    }

    fn inspect_text_input(&self) -> Value {
        self.inspect()
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.sync(None);
    }
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn EnableSecureEventInput() -> i32;
    fn DisableSecureEventInput() -> i32;
}

fn edit_shortcut(event: &NSEvent) -> Option<EditorCommand> {
    use EditorCommand::*;
    use NSEventModifierFlags as Flags;
    let modifiers = event.modifierFlags().intersection(Flags::Command | Flags::Control | Flags::Option | Flags::Shift);
    let key = event.charactersIgnoringModifiers()?;
    let key = key.to_string().to_ascii_lowercase();
    match (key.as_str(), modifiers) {
        ("a", Flags::Command) => Some(SelectAll),
        ("c", Flags::Command) => Some(Copy),
        ("v", Flags::Command) => Some(Paste),
        ("x", Flags::Command) => Some(Cut),
        ("z", Flags::Command) => Some(Undo),
        ("z", flags) if flags == Flags::Command | Flags::Shift => Some(Redo),
        _ => None,
    }
}

fn selector_command(selector: &[u8]) -> Option<EditorCommand> {
    use EditorCommand::*;
    Some(match selector {
        b"moveLeft:" | b"moveBackward:" => MoveLeft,
        b"moveRight:" | b"moveForward:" => MoveRight,
        b"moveUp:" => MoveUp,
        b"moveDown:" => MoveDown,
        b"moveWordLeft:" | b"moveWordBackward:" => MoveWordLeft,
        b"moveWordRight:" | b"moveWordForward:" => MoveWordRight,
        b"moveToBeginningOfLine:" | b"moveToLeftEndOfLine:" => MoveLineStart,
        b"moveToEndOfLine:" | b"moveToRightEndOfLine:" => MoveLineEnd,
        b"moveToBeginningOfDocument:" => MoveDocumentStart,
        b"moveToEndOfDocument:" => MoveDocumentEnd,
        b"moveLeftAndModifySelection:" | b"moveBackwardAndModifySelection:" => SelectLeft,
        b"moveRightAndModifySelection:" | b"moveForwardAndModifySelection:" => SelectRight,
        b"moveUpAndModifySelection:" => SelectUp,
        b"moveDownAndModifySelection:" => SelectDown,
        b"moveWordLeftAndModifySelection:" | b"moveWordBackwardAndModifySelection:" => SelectWordLeft,
        b"moveWordRightAndModifySelection:" | b"moveWordForwardAndModifySelection:" => SelectWordRight,
        b"moveToBeginningOfLineAndModifySelection:" | b"moveToLeftEndOfLineAndModifySelection:" => SelectLineStart,
        b"moveToEndOfLineAndModifySelection:" | b"moveToRightEndOfLineAndModifySelection:" => SelectLineEnd,
        b"moveToBeginningOfDocumentAndModifySelection:" => SelectDocumentStart,
        b"moveToEndOfDocumentAndModifySelection:" => SelectDocumentEnd,
        b"selectAll:" => SelectAll,
        b"deleteBackward:" | b"deleteBackwardByDecomposingPreviousCharacter:" => DeleteBackward,
        b"deleteForward:" => DeleteForward,
        b"deleteWordBackward:" => DeleteWordBackward,
        b"deleteWordForward:" => DeleteWordForward,
        b"deleteToBeginningOfLine:" => DeleteToLineStart,
        b"deleteToEndOfLine:" => DeleteToLineEnd,
        b"insertNewline:" | b"insertLineBreak:" | b"insertNewlineIgnoringFieldEditor:" => InsertNewline,
        b"undo:" => Undo,
        b"redo:" => Redo,
        b"copy:" => Copy,
        b"cut:" => Cut,
        b"paste:" | b"pasteAsPlainText:" => Paste,
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum ParagraphChange {
    Select((usize, usize)),
    Delete(Range<usize>),
}

fn paragraph_change(selector: &[u8], text: &str, (anchor, caret): (usize, usize)) -> Option<ParagraphChange> {
    use ParagraphChange::*;
    // Paragraph selectors address logical text, including when its lines wrap visually.
    let paragraph = || paragraph_at(text, caret);
    let delete = |edge| {
        let (start, end) = if anchor == caret { (edge, caret) } else { (anchor, caret) };
        if start == end { Select((anchor, caret)) } else { Delete(start.min(end)..start.max(end)) }
    };
    Some(match selector {
        b"moveToBeginningOfParagraph:" => {
            let start = paragraph().start;
            Select((start, start))
        }
        b"moveToEndOfParagraph:" => {
            let end = paragraph().end;
            Select((end, end))
        }
        b"moveToBeginningOfParagraphAndModifySelection:" => Select((anchor, paragraph().start)),
        b"moveToEndOfParagraphAndModifySelection:" => Select((anchor, paragraph().end)),
        b"deleteToBeginningOfParagraph:" => delete(paragraph().start),
        b"deleteToEndOfParagraph:" => delete(paragraph().end),
        _ => return None,
    })
}

pub(super) fn marked_attributes() -> [&'static NSAttributedStringKey; 2] {
    // SAFETY: These AppKit constants are immutable and live for the process lifetime.
    unsafe { [NSMarkedClauseSegmentAttributeName, NSUnderlineStyleAttributeName] }
}

fn marked_clauses<'a>(attrs: &'a NSAttributedString, rope: &'a EditorRope) -> impl Iterator<Item = MarkedClause> + 'a {
    let [segment_key, underline_key] = marked_attributes();
    let length = attrs.length();
    let limit = NSRange::new(0, length);
    let mut index = 0;
    let mut covered = 0;
    std::iter::from_fn(move || {
        while index < length {
            let mut segment_range = NSRange::new(index, 1);
            let mut underline_range = segment_range;
            // SAFETY: The UTF-16 index is within the string and the ranges are writable.
            let (segment, underline) = unsafe {
                (
                    attrs.attribute_atIndex_longestEffectiveRange_inRange(segment_key, index, &mut segment_range, limit),
                    attrs.attribute_atIndex_longestEffectiveRange_inRange(underline_key, index, &mut underline_range, limit),
                )
            };
            let end = (segment_range.location + segment_range.length).min(underline_range.location + underline_range.length);
            let start = index;
            index = end;
            let underline = underline.as_deref().and_then(AnyObject::downcast_ref::<NSNumber>).map_or(0, NSNumber::integerValue) & 0xf;
            if segment.is_none() && underline == NSUnderlineStyle::None.bits() {
                continue;
            }
            let Some(mut range) = utf16_range(rope, start..end) else { continue };
            // AppKit attributes can split a surrogate pair. Keep the converted clauses disjoint.
            range.start = range.start.max(covered);
            if range.is_empty() {
                continue;
            }
            covered = range.end;
            let style = if underline == NSUnderlineStyle::Thick.bits() || underline == NSUnderlineStyle::Double.bits() {
                ClauseStyle::ActiveClause
            } else {
                ClauseStyle::Clause
            };
            return Some(MarkedClause { range, style });
        }
        None
    })
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

fn query_range(rope: &EditorRope, range: NSRange) -> Option<Range<usize>> {
    if range.length != 0 && range.location == rope.len_utf16() {
        return None;
    }
    utf16_range(rope, range.location..range.location.saturating_add(range.length))
}

fn replacement_range(rope: &EditorRope, range: NSRange) -> Option<Range<usize>> {
    if range.location == NSNotFound as usize {
        return None;
    }
    utf16_range(rope, range.location.min(rope.len_utf16())..range.location.saturating_add(range.length))
}

fn ns_range(range: Range<usize>) -> NSRange {
    NSRange::new(range.start, range.end - range.start)
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
    use objc2_foundation::NSMutableAttributedString;

    #[test]
    fn appkit_ranges_clamp_replacements_but_reject_out_of_bounds_queries() {
        let rope = EditorRope::new("A😀Z");
        assert_eq!(replacement_range(&rope, NSRange::new(1, 1)), Some(1..5));
        assert_eq!(replacement_range(&rope, NSRange::new(2, 0)), Some(1..1));
        assert_eq!(replacement_range(&rope, NSRange::new(5, 0)), Some(6..6));
        assert_eq!(replacement_range(&rope, not_found()), None);
        assert_eq!(query_range(&rope, NSRange::new(5, 0)), None);
        assert_eq!(query_range(&rope, NSRange::new(4, 1)), None);
    }

    #[test]
    fn appkit_selector_aliases_route_to_shared_commands() {
        use EditorCommand::*;

        for (selector, expected) in [
            (b"moveWordBackwardAndModifySelection:".as_slice(), SelectWordLeft),
            (b"moveToRightEndOfLine:", MoveLineEnd),
            (b"moveToEndOfDocumentAndModifySelection:", SelectDocumentEnd),
            (b"deleteToBeginningOfLine:", DeleteToLineStart),
            (b"deleteBackwardByDecomposingPreviousCharacter:", DeleteBackward),
            (b"insertLineBreak:", InsertNewline),
            (b"pasteAsPlainText:", Paste),
            (b"undo:", Undo),
            (b"redo:", Redo),
        ] {
            assert_eq!(selector_command(selector), Some(expected));
        }
        assert_eq!(selector_command(b"cancelOperation:"), None);
        assert_eq!(selector_command(b"insertTab:"), None);
    }

    #[test]
    fn appkit_paragraph_selectors_use_document_breaks_and_directional_selection() {
        use ParagraphChange::*;

        let text = "prev\n你好 paragraph with soft wraps\r\nnext";
        let caret = text.find("soft").unwrap();
        let start = text.find('你').unwrap();
        let end = text.find('\r').unwrap();
        assert_eq!(selector_command(b"moveToBeginningOfParagraph:"), None);
        assert_eq!(selector_command(b"moveToEndOfParagraphAndModifySelection:"), None);
        assert_eq!(selector_command(b"deleteToBeginningOfParagraph:"), None);
        assert_eq!(paragraph_change(b"moveToBeginningOfParagraph:", text, (caret, caret)), Some(Select((start, start))));
        assert_eq!(paragraph_change(b"moveToEndOfParagraph:", text, (caret, caret)), Some(Select((end, end))));
        assert_eq!(paragraph_change(b"moveToBeginningOfParagraphAndModifySelection:", text, (end, caret)), Some(Select((end, start))));
        assert_eq!(paragraph_change(b"moveToEndOfParagraphAndModifySelection:", text, (start, caret)), Some(Select((start, end))));
        assert_eq!(paragraph_change(b"deleteToBeginningOfParagraph:", text, (caret, caret)), Some(Delete(start..caret)));
        assert_eq!(paragraph_change(b"deleteToEndOfParagraph:", text, (caret, caret)), Some(Delete(caret..end)));
        assert_eq!(paragraph_change(b"deleteToBeginningOfParagraph:", text, (end, caret)), Some(Delete(caret..end)));
        assert_eq!(paragraph_change(b"deleteToBeginningOfParagraph:", text, (start, start)), Some(Select((start, start))));
    }

    #[test]
    fn appkit_marked_attributes_keep_clause_boundaries_and_active_style() {
        objc2::rc::autoreleasepool(|_| {
            let text = "你😀好!";
            let attrs = NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &NSString::from_str(text));
            let [segment, underline] = marked_attributes();
            let inactive = NSNumber::numberWithInteger(NSUnderlineStyle::Single.bits());
            let active = NSNumber::numberWithInteger((NSUnderlineStyle::Thick | NSUnderlineStyle::PatternDot).bits());
            let double_underlined = NSNumber::numberWithInteger(NSUnderlineStyle::Double.bits());
            // SAFETY: All ranges are within the string and these attributes accept NSNumber.
            unsafe {
                attrs.addAttribute_value_range(segment, &NSNumber::numberWithInteger(0), NSRange::new(0, 1));
                attrs.addAttribute_value_range(segment, &NSNumber::numberWithInteger(1), NSRange::new(1, 2));
                attrs.addAttribute_value_range(underline, &inactive, NSRange::new(0, 3));
                attrs.addAttribute_value_range(underline, &active, NSRange::new(3, 1));
                attrs.addAttribute_value_range(underline, &double_underlined, NSRange::new(4, 1));
            }
            let rope = EditorRope::new(text);
            assert_eq!(
                marked_clauses(&attrs, &rope).collect::<Vec<_>>(),
                [
                    MarkedClause { range: 0..3, style: ClauseStyle::Clause },
                    MarkedClause { range: 3..7, style: ClauseStyle::Clause },
                    MarkedClause { range: 7..10, style: ClauseStyle::ActiveClause },
                    MarkedClause { range: 10..11, style: ClauseStyle::ActiveClause },
                ]
            );
        });
    }
}
