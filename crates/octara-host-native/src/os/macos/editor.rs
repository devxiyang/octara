//! AppKit text services for the retained tree's focused editor.

use super::super::{ActiveEditorView, HostWake, PlatformKey, WindowHostEvent, WindowTextInput};
use super::view::OctaraView;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, Message};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSTextInputContext};
use objc2_foundation::{NSArray, NSAttributedString, NSNotFound, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString};
use octara::EditorRope;
use octara::runtime::Input;
use octara::tree::editor::EditorMirror;
use octara_model::ask::ClipboardKinds;
use octara_model::geometry::{Point, Rect};
use octara_model::identity::EditorSessionId;
use octara_model::input::{EditorEvent, HostReply, PasteEvent};
use octara_model::text::{TextEdit, TextGeometry, TextLine};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::Range;

struct Session {
    id: EditorSessionId,
    revision: u64,
    document: EditorMirror,
    secure: bool,
    multiline: bool,
    caret: Option<Rect>,
    geometry: Option<TextGeometry>,
}

impl Session {
    fn new(active: &ActiveEditorView<'_>) -> Self {
        Self {
            id: active.session,
            revision: active.document_rev,
            document: EditorMirror::new(active.editor),
            secure: active.secure,
            multiline: active.multiline,
            caret: active.caret,
            geometry: active.geometry.cloned(),
        }
    }

    fn rope(&self) -> &EditorRope {
        self.document.editor().displayed_rope()
    }

    fn selection(&self) -> Option<Range<usize>> {
        self.document.editor().displayed_selection().map(|(a, b)| a.min(b)..a.max(b))
    }

    fn marked(&self) -> Option<Range<usize>> {
        self.document.editor().composition().map(|composition| composition.marked())
    }

    fn apply(&mut self, event: EditorEvent) -> Option<EditorEvent> {
        self.document.apply(event.clone()).then_some(event)
    }
}

#[derive(Default)]
struct State {
    session: Option<Session>,
    pending: Vec<Input>,
    predicted: VecDeque<(u64, EditorMirror)>,
}

impl State {
    fn propose(&mut self, change: impl FnOnce(&mut Session) -> Option<EditorEvent>) -> bool {
        let Some(session) = &mut self.session else { return false };
        let Some(event) = change(session) else { return false };
        session.revision = session.revision.checked_add(1).expect("editor document revisions exhausted");
        session.geometry = None;
        self.predicted.push_back((session.revision, session.document.clone()));
        self.pending.push(Input::Editor(event));
        true
    }
}

pub(super) struct TextInput {
    state: RefCell<State>,
    context: RefCell<Option<Retained<NSTextInputContext>>>,
    wake: HostWake,
    handled: Cell<bool>,
    forward: Cell<bool>,
    discarding: Cell<bool>,
    anchor: Cell<Option<NSRect>>,
}

impl TextInput {
    pub(super) fn new(wake: HostWake) -> Self {
        Self {
            state: RefCell::default(),
            context: RefCell::default(),
            wake,
            handled: Cell::new(false),
            forward: Cell::new(false),
            discarding: Cell::new(false),
            anchor: Cell::default(),
        }
    }
}

impl OctaraView {
    pub(super) fn interpret_editor_key(&self, event: &NSEvent) -> bool {
        let ivars = &self.ivars().text_input;
        let Some(context) = self.editor_context() else { return false };
        ivars.handled.set(false);
        ivars.forward.set(false);
        if !self.composing()
            && let Some(command) = edit_shortcut(event)
        {
            return self.command(command);
        }
        let consumed = context.handleEvent(event);
        if !consumed && !ivars.handled.get() && !ivars.forward.get() {
            self.interpretKeyEvents(&NSArray::from_slice(&[event]));
        }
        ivars.handled.get() || (consumed && !ivars.forward.get())
    }

    pub(super) fn editor_context(&self) -> Option<Retained<NSTextInputContext>> {
        self.ivars().text_input.context.borrow().clone()
    }

    pub(super) fn editor_insert_text(&self, string: &AnyObject, replacement: NSRange) {
        let Some(text) = native_string(string) else { return };
        let replaced = self
            .ivars()
            .text_input
            .state
            .borrow()
            .session
            .as_ref()
            .is_some_and(|session| !session.multiline && matches!(text.as_str(), "\r" | "\n"));
        if replaced {
            self.ivars().text_input.forward.set(true);
            return;
        }
        let replacement = self.replacement(replacement);
        self.enqueue(TextEdit::Insert { text, replacement });
    }

    pub(super) fn editor_do_command(&self, selector: Sel) {
        let name = selector.name().to_bytes();
        if name == b"cancelOperation:" && self.composing() {
            self.enqueue(TextEdit::Cancel);
            self.discard_composer();
        }
        if !self.ivars().text_input.handled.get() && !self.command(name) {
            self.ivars().text_input.forward.set(true);
        }
    }

    pub(super) fn editor_set_marked_text(&self, string: &AnyObject, selection: NSRange, replacement: NSRange) {
        let Some(text) = native_string(string) else { return };
        let selection = byte_range(&EditorRope::new(text.clone()), selection);
        let replacement = self.replacement(replacement);
        self.enqueue(TextEdit::Mark { text, selection, replacement });
    }

    pub(super) fn editor_unmark_text(&self) {
        self.enqueue(TextEdit::Unmark);
    }

    pub(super) fn editor_selected_range(&self) -> NSRange {
        let state = self.ivars().text_input.state.borrow();
        state.session.as_ref().and_then(|s| s.selection().map(|r| native_range(s.rope(), r))).unwrap_or_else(not_found)
    }

    pub(super) fn editor_marked_range(&self) -> NSRange {
        let state = self.ivars().text_input.state.borrow();
        state.session.as_ref().and_then(|s| s.marked().map(|r| native_range(s.rope(), r))).unwrap_or_else(not_found)
    }

    pub(super) fn editor_has_marked_text(&self) -> bool {
        self.composing()
    }

    pub(super) unsafe fn editor_substring(&self, proposed: NSRange, actual: NSRangePointer) -> Option<Retained<NSAttributedString>> {
        // SAFETY: AppKit provides writable storage or null.
        unsafe { write_range(actual, not_found()) };
        self.substring_value(proposed).map(|(text, range)| {
            unsafe { write_range(actual, range) };
            NSAttributedString::initWithString(NSAttributedString::alloc(), &NSString::from_str(&text))
        })
    }

    pub(super) unsafe fn editor_first_rect(&self, proposed: NSRange, actual: NSRangePointer) -> NSRect {
        unsafe { write_range(actual, not_found()) };
        let (position, covered) = {
            let state = self.ivars().text_input.state.borrow();
            let Some(session) = &state.session else { return NSRect::ZERO };
            let text = session.rope();
            let Some(requested) = query_range(text, proposed) else { return NSRect::ZERO };
            let current_caret = session.document.editor().displayed_selection().map(|(_, caret)| caret);
            let found = session.geometry.as_ref().and_then(|geometry| first_rect(geometry, requested.clone()));
            let fallback = || {
                let byte = current_caret.unwrap_or(requested.start);
                session.caret.map(|caret| (byte..byte, caret))
            };
            let geometry =
                if requested.is_empty() && current_caret == Some(requested.start) { fallback().or(found) } else { found.or_else(fallback) };
            let Some((range, rect)) = geometry else { return NSRect::ZERO };
            (rect, native_range(text, range))
        };
        let Some(window) = self.window() else { return NSRect::ZERO };
        let local =
            NSRect::new(NSPoint::new(position.x.into(), position.y.into()), NSSize::new(position.w.max(1.0).into(), position.h.into()));
        let screen = window.convertRectToScreen(self.convertRect_toView(local, None));
        self.ivars().text_input.anchor.set(Some(screen));
        unsafe { write_range(actual, covered) };
        screen
    }

    pub(super) fn editor_character_index(&self, point: NSPoint) -> usize {
        let Some(window) = self.window() else { return NSNotFound as usize };
        let local = self.convertPoint_fromView(window.convertPointFromScreen(point), None);
        let state = self.ivars().text_input.state.borrow();
        let Some(session) = &state.session else { return NSNotFound as usize };
        let point = Point::new(local.x as f32, local.y as f32);
        session
            .geometry
            .as_ref()
            .and_then(|geometry| index_at(geometry, point))
            .map_or(NSNotFound as usize, |index| session.rope().byte_to_utf16(index).unwrap_or(NSNotFound as usize))
    }

    fn substring_value(&self, proposed: NSRange) -> Option<(String, NSRange)> {
        let state = self.ivars().text_input.state.borrow();
        let session = state.session.as_ref().filter(|session| !session.secure)?;
        let text = session.rope();
        let range = query_range(text, proposed)?;
        let covered = native_range(text, range.clone());
        Some((text.slice(range)?, covered))
    }

    fn replacement(&self, range: NSRange) -> Option<Range<usize>> {
        let state = self.ivars().text_input.state.borrow();
        byte_range(state.session.as_ref()?.rope(), range)
    }

    pub(super) fn editor_session(&self) -> Option<EditorSessionId> {
        self.ivars().text_input.state.borrow().session.as_ref().map(|session| session.id)
    }

    pub(super) fn composing(&self) -> bool {
        self.ivars().text_input.state.borrow().session.as_ref().is_some_and(|s| s.marked().is_some())
    }

    fn enqueue(&self, edit: TextEdit) {
        let ivars = &self.ivars().text_input;
        ivars.handled.set(true);
        if ivars.discarding.get() {
            return;
        }
        self.propose(|session| session.apply(EditorEvent::ProposeEdit { session: session.id, expected_rev: session.revision, edit }));
    }

    fn propose(&self, change: impl FnOnce(&mut Session) -> Option<EditorEvent>) {
        let changed = self.ivars().text_input.state.borrow_mut().propose(change);
        if changed {
            (self.ivars().text_input.wake)();
        }
    }

    fn select(&self, selection: (usize, usize)) {
        self.propose(|session| {
            session.apply(EditorEvent::ProposeSelection { session: session.id, expected_rev: session.revision, selection })
        });
    }

    fn command(&self, selector: &[u8]) -> bool {
        if self.composing() {
            return false;
        }
        match selector {
            b"copy:" | b"cut:" => return self.copy_selection(selector == b"cut:"),
            b"paste:" | b"pasteAsPlainText:" => return self.paste(),
            _ => {}
        }
        let state = self.ivars().text_input.state.borrow();
        let Some(session) = state.session.as_ref() else { return false };
        let rope = session.rope();
        let (anchor, caret) = session.document.editor().displayed_selection().unwrap_or((rope.len(), rope.len()));
        let selected = session.selection().unwrap_or(caret..caret);
        let edit = match selector {
            b"insertNewline:" | b"insertLineBreak:" if session.multiline => TextEdit::Insert { text: "\n".into(), replacement: None },
            b"deleteBackward:" | b"deleteForward:" | b"deleteWordBackward:" | b"deleteWordForward:" => {
                let backward = matches!(selector, b"deleteBackward:" | b"deleteWordBackward:");
                let word = matches!(selector, b"deleteWordBackward:" | b"deleteWordForward:");
                let adjacent =
                    (!word && selected.is_empty()).then(|| session.document.editor().rich_text()?.adjacent_atom(caret, backward)).flatten();
                let replacement = if let Some(atom) = adjacent {
                    atom.range.clone()
                } else if selected.is_empty() {
                    let other =
                        if word { word_edge(rope.as_str(), caret, backward) } else { rope.grapheme_edge(caret, backward).unwrap_or(caret) };
                    caret.min(other)..caret.max(other)
                } else {
                    selected
                };
                TextEdit::Insert { text: String::new(), replacement: Some(replacement) }
            }
            b"selectAll:" => {
                let end = rope.len();
                drop(state);
                self.select((0, end));
                return true;
            }
            _ => {
                let Some(next) = movement(session, selector, anchor, caret) else { return false };
                drop(state);
                self.select(next);
                return true;
            }
        };
        drop(state);
        self.enqueue(edit);
        true
    }

    fn copy_selection(&self, cut: bool) -> bool {
        let (revision, selected, text) = {
            let state = self.ivars().text_input.state.borrow();
            let Some(session) = &state.session else { return false };
            let Some(selected) = session.selection().filter(|range| !session.secure && !range.is_empty()) else {
                return true;
            };
            let text = session.rope().slice(selected.clone()).expect("native selection is valid");
            ((session.id, session.revision), selected, text)
        };
        super::services::write_text(&text);
        if cut && self.has_revision(revision) {
            self.enqueue(TextEdit::Insert { text: String::new(), replacement: Some(selected) });
        }
        true
    }

    pub(super) fn paste(&self) -> bool {
        let ivars = &self.ivars().text_input;
        if ivars.state.borrow().session.is_none() {
            return false;
        }
        let items = super::services::read_clipboard(ClipboardKinds::all());
        if !items.is_empty() {
            ivars.state.borrow_mut().pending.push(Input::Host(HostReply::Paste(PasteEvent { items })));
            (ivars.wake)();
        }
        true
    }

    fn has_revision(&self, revision: (EditorSessionId, u64)) -> bool {
        self.ivars().text_input.state.borrow().session.as_ref().is_some_and(|session| (session.id, session.revision) == revision)
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
        let next = active.map(Session::new);
        let (replace_context, selection_changed) = {
            let mut state = self.view.ivars().text_input.state.borrow_mut();
            let old = state.session.as_ref();
            let replace = old.map(|s| s.id) != next.as_ref().map(|s| s.id);
            let changed = old.and_then(|s| s.selection()) != next.as_ref().and_then(|s| s.selection());
            let mut discard = replace;
            if let (Some(old), Some(next)) = (old, &next)
                && !replace
            {
                // While several callbacks are outstanding, an accepted prefix may publish
                // an intermediate mirror. Keep the still-pending suffix for AppKit queries.
                let accepted = state.predicted.iter().position(|(rev, document)| *rev == next.revision && document == &next.document);
                if old.revision > next.revision
                    && let Some(index) = accepted
                {
                    state.predicted.drain(..=index);
                    if let Some(session) = &mut state.session {
                        session.caret = next.caret;
                    }
                    return;
                }
                discard = old.marked().is_some() && (old.marked() != next.marked() || changed || old.rope() != next.rope());
            }
            state.predicted.clear();
            if discard {
                state.pending.clear();
            }
            state.session = next;
            (discard, changed)
        };
        let secure = active.is_some_and(|active| active.secure) && self.view.window().is_some_and(|window| window.isKeyWindow());
        self.set_secure(secure);
        if replace_context {
            self.view.discard_composer();
            self.view.ivars().text_input.anchor.set(None);
            let context = active.map(|_| {
                NSTextInputContext::initWithClient(NSTextInputContext::alloc(self.view.mtm()), ProtocolObject::from_ref(&*self.view))
            });
            let old = self.view.ivars().text_input.context.replace(context);
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
            if selection_changed {
                context.textInputClientDidUpdateSelection();
            }
        }
    }

    fn drain(&self) -> Vec<Input> {
        std::mem::take(&mut self.view.ivars().text_input.state.borrow_mut().pending)
    }

    fn set_focused(&mut self, active: bool) {
        let secure = self.view.ivars().text_input.state.borrow().session.as_ref().is_some_and(|session| session.secure);
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
        let state = self.view.ivars().text_input.state.borrow();
        json!({
            "active": state.session.is_some(),
            "secure": self.secure,
            "native_focus": native_focus,
            "composing": state.session.as_ref().is_some_and(|session| session.marked().is_some()),
            "pending": state.pending.len(),
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

    fn on_unhandled_key(&mut self, key: &PlatformKey) -> bool {
        self.view.interpret_pending_key(key)
    }

    fn drain_text_input(&mut self, out: &mut Vec<WindowHostEvent>) {
        out.extend(self.drain());
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

fn edit_shortcut(event: &NSEvent) -> Option<&'static [u8]> {
    let modifiers = event.modifierFlags().intersection(NSEventModifierFlags::DeviceIndependentFlagsMask);
    if modifiers != NSEventModifierFlags::Command {
        return None;
    }
    match event.charactersIgnoringModifiers()?.to_string().as_str() {
        "a" | "A" => Some(b"selectAll:"),
        "c" | "C" => Some(b"copy:"),
        "v" | "V" => Some(b"paste:"),
        "x" | "X" => Some(b"cut:"),
        _ => None,
    }
}

fn movement(session: &Session, selector: &[u8], anchor: usize, caret: usize) -> Option<(usize, usize)> {
    let (selector, modify) =
        selector.strip_suffix(b"AndModifySelection:").map_or_else(|| (selector.strip_suffix(b":"), false), |normal| (Some(normal), true));
    let selector = selector?;
    let rope = session.rope();
    let move_end = |backward: bool| {
        if anchor != caret && !modify {
            if backward { anchor.min(caret) } else { anchor.max(caret) }
        } else {
            let next = rope.grapheme_edge(caret, backward).unwrap_or(caret);
            session.document.editor().rich_text().map_or(next, |rich| rich.snap_movement(next, backward))
        }
    };
    let move_visual = |left: bool| {
        if modify || anchor == caret {
            visual_move(session, caret, left).unwrap_or_else(|| move_end(left))
        } else {
            let positions = line_at(session, anchor).zip(line_at(session, caret));
            positions.map_or_else(
                || move_end(left),
                |((a, anchor_line), (c, caret_line))| {
                    let order = a.cmp(&c).then_with(|| caret_x(anchor_line, anchor).total_cmp(&caret_x(caret_line, caret)));
                    if order.is_lt() == left { anchor } else { caret }
                },
            )
        }
    };
    let next = match selector {
        b"moveLeft" => move_visual(true),
        b"moveRight" => move_visual(false),
        b"moveBackward" => move_end(true),
        b"moveForward" => move_end(false),
        b"moveWordLeft" | b"moveWordBackward" => word_edge(rope.as_str(), caret, true),
        b"moveWordRight" | b"moveWordForward" => word_edge(rope.as_str(), caret, false),
        b"moveToBeginningOfDocument" => 0,
        b"moveToEndOfDocument" => rope.len(),
        b"moveToBeginningOfParagraph" => rope.byte_to_line_col(caret).and_then(|(line, _)| rope.line_start(line)).unwrap_or(0),
        b"moveToEndOfParagraph" => {
            rope.byte_to_line_col(caret).and_then(|(line, _)| rope.line_start(line + 1)).map_or(rope.len(), |start| start - 1)
        }
        b"moveToBeginningOfLine" => line_at(session, caret).map_or(0, |(_, line)| line.range.start),
        b"moveToEndOfLine" => line_at(session, caret).map_or(rope.len(), |(_, line)| {
            let end = line.range.end.min(rope.len());
            end - usize::from(rope.as_str()[..end].ends_with('\n'))
        }),
        b"moveUp" | b"moveDown" => {
            let delta = if selector == b"moveUp" { -1 } else { 1 };
            line_at(session, caret)
                .and_then(|(index, line)| {
                    let geometry = session.geometry.as_ref()?;
                    let target = geometry.lines.get(index.checked_add_signed(delta)?)?;
                    Some(line_index_at(target, caret_x(line, caret)))
                })
                .unwrap_or(if delta < 0 { 0 } else { rope.len() })
        }
        _ => return None,
    };
    let next = match selector {
        b"moveLeft" | b"moveRight" | b"moveBackward" | b"moveForward" => {
            // Logical grapheme movement already jumped the atom. Visual movement can
            // still land on an interior glyph stop, especially in a bidi run.
            session.document.editor().rich_text().map_or(next, |rich| rich.snap_movement(next, next < caret))
        }
        _ => next,
    };
    Some((if modify { anchor } else { next }, next))
}

fn line_at(session: &Session, caret: usize) -> Option<(usize, &TextLine)> {
    session.geometry.as_ref()?.lines.iter().enumerate().rfind(|(_, line)| line.range.start <= caret)
}

fn visual_move(session: &Session, caret: usize, left: bool) -> Option<usize> {
    let (index, line) = line_at(session, caret)?;
    let x = caret_x(line, caret);
    let nearby = visual_stops(line).filter(|(position, _)| if left { *position < x } else { *position > x });
    if let Some((_, stop)) = nearby.min_by(|(a, _), (b, _)| (x - a).abs().total_cmp(&(x - b).abs())) {
        return Some(stop);
    }
    let geometry = session.geometry.as_ref()?;
    let neighbor = index.checked_add_signed(if left { -1 } else { 1 }).and_then(|index| geometry.lines.get(index));
    Some(neighbor.map_or(caret, |line| line_index_at(line, if left { line.rect.right() } else { line.rect.x })))
}

fn word_edge(text: &str, caret: usize, backward: bool) -> usize {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if backward {
        let prefix = &text[..caret];
        let skipped = prefix.trim_end_matches(|c: char| !word(c));
        skipped.trim_end_matches(word).len()
    } else {
        let suffix = &text[caret..];
        let skipped = suffix.trim_start_matches(|c: char| !word(c));
        text.len() - skipped.trim_start_matches(word).len()
    }
}

fn first_rect(geometry: &TextGeometry, range: Range<usize>) -> Option<(Range<usize>, Rect)> {
    let line = geometry.lines.iter().rfind(|line| line.range.start <= range.start)?;
    let mut covered = range.start..range.start;
    let mut rect = Rect::new(caret_x(line, range.start), line.rect.y, 0.0, line.rect.h);
    if !range.is_empty() {
        let mut fragments = line.fragments.iter().filter(|fragment| range.start < fragment.range.end && fragment.range.start < range.end);
        if let Some(first) = fragments.next() {
            covered = first.range.clone();
            rect = first.rect;
            for fragment in fragments {
                covered = covered.start.min(fragment.range.start)..covered.end.max(fragment.range.end);
                let x = rect.x.min(fragment.rect.x);
                rect = Rect::new(x, line.rect.y, rect.right().max(fragment.rect.right()) - x, line.rect.h);
            }
        }
    }
    rect.x += geometry.origin.x;
    rect.y += geometry.origin.y;
    Some((covered, rect))
}

fn caret_x(line: &TextLine, index: usize) -> f32 {
    let fragment = line
        .fragments
        .iter()
        .find(|fragment| fragment.range.contains(&index))
        .or_else(|| line.fragments.iter().filter(|fragment| fragment.range.end <= index).max_by_key(|fragment| fragment.range.end));
    fragment.map_or(
        line.rect.x,
        |fragment| {
            if (index >= fragment.range.end) != fragment.rtl { fragment.rect.right() } else { fragment.rect.x }
        },
    )
}

fn index_at(geometry: &TextGeometry, point: Point) -> Option<usize> {
    let y = point.y - geometry.origin.y;
    let line = geometry.lines.iter().find(|line| line.rect.y <= y && y < line.rect.bottom())?;
    Some(line_index_at(line, point.x - geometry.origin.x))
}

fn line_index_at(line: &TextLine, x: f32) -> usize {
    let nearest = visual_stops(line).min_by(|(a, _), (b, _)| (x - a).abs().total_cmp(&(x - b).abs()));
    nearest.map_or(line.range.start, |(_, index)| index)
}

fn visual_stops(line: &TextLine) -> impl Iterator<Item = (f32, usize)> {
    line.fragments.iter().flat_map(|fragment| {
        let (start, end) = (fragment.range.start, fragment.range.end);
        let (left, right) = if fragment.rtl { (end, start) } else { (start, end) };
        [(fragment.rect.x, left), (fragment.rect.right(), right)]
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

fn query_range(text: &EditorRope, range: NSRange) -> Option<Range<usize>> {
    let length = text.len_utf16();
    if range.location > length || (range.location == length && range.length != 0) {
        return None;
    }
    byte_range(text, range)
}

/// Expands a split surrogate pair for nonempty ranges. Offsets past the
/// displayed text are clamped to its end.
fn byte_range(text: &EditorRope, range: NSRange) -> Option<Range<usize>> {
    if range.location == NSNotFound as usize {
        return None;
    }
    let len = text.len_utf16();
    let (start, _) = text.utf16_byte_bounds(range.location.min(len))?;
    if range.length == 0 {
        return Some(start..start);
    }
    let (_, end) = text.utf16_byte_bounds(range.location.saturating_add(range.length).min(len))?;
    Some(start..end)
}

fn native_range(text: &EditorRope, range: Range<usize>) -> NSRange {
    let start = text.byte_to_utf16(range.start).expect("native range starts at a character boundary");
    let end = text.byte_to_utf16(range.end).expect("native range ends at a character boundary");
    NSRange::new(start, end - start)
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
    use octara_model::text::TextFragment;
    use std::sync::Arc;

    #[test]
    fn appkit_ranges_expand_surrogate_pairs_and_keep_insertion_points() {
        let text = &EditorRope::new("A😀Z");
        assert_eq!(byte_range(text, NSRange::new(1, 1)), Some(1..5));
        assert_eq!(byte_range(text, NSRange::new(2, 0)), Some(1..1));
        assert_eq!(byte_range(text, NSRange::new(2, 1)), Some(1..5));
        assert_eq!(native_range(text, 1..5), NSRange::new(1, 2));
        assert_eq!(query_range(text, NSRange::new(5, 0)), None);
    }

    #[test]
    fn appkit_navigation_respects_composed_characters_and_rtl_fragments() {
        let text = &EditorRope::new("a👩🏽‍💻Z");
        let end = text.len() - 1;
        assert_eq!(text.grapheme_edge(end, true), Some(1));
        assert_eq!(text.grapheme_edge(1, false), Some(end));

        let geometry = TextGeometry {
            origin: Point::new(30.0, 40.0),
            lines: Arc::from([TextLine {
                range: 0..2,
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                glyph_top: 0.0,
                glyph_height: 10.0,
                fragments: Arc::from([TextFragment { range: 0..2, rect: Rect::new(0.0, 0.0, 10.0, 10.0), rtl: true }]),
            }]),
        };
        assert_eq!(index_at(&geometry, Point::new(31.0, 45.0)), Some(2));
        assert_eq!(index_at(&geometry, Point::new(39.0, 45.0)), Some(0));
        assert_eq!(first_rect(&geometry, 0..2), Some((0..2, Rect::new(30.0, 40.0, 10.0, 10.0))));
    }
}
