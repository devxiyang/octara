use super::{NativeEvent, Protocol, Shared, window::WindowState};
use crate::os::{OsHost, PlatformEvent, ShellDriver};
use crate::text_bridge::{BridgeSession, HostTextBridge, SurroundingText};
use octara::runtime::Input;
use octara_model::identity::EditorSessionId;
use octara_model::text::{AutoCapitalize, Autocorrect, ClauseStyle, KeyboardType, MarkedClause, TextContentType, TextEdit};
use smithay_client_toolkit::shell::WaylandSurface;
use std::collections::VecDeque;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{self as protocol, ContentHint, ContentPurpose, ZwpTextInputV3};

const SURROUNDING_LIMIT: usize = 4000;
const SURROUNDING_HISTORY: usize = 16;

#[derive(PartialEq, Eq)]
struct Settings {
    surrounding: Option<Surrounding>,
    caret: Option<[i32; 4]>,
    hint: ContentHint,
    purpose: ContentPurpose,
}

#[derive(PartialEq, Eq)]
struct Surrounding {
    text: String,
    cursor: i32,
    anchor: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SurroundingBasis {
    value_rev: u64,
    selection: (usize, usize),
}

impl From<&BridgeSession> for SurroundingBasis {
    fn from(session: &BridgeSession) -> Self {
        let editor = session.editor();
        // Relative deletes target the committed text and selection after removing preedit.
        Self { value_rev: editor.value_rev(), selection: editor.selection() }
    }
}

impl From<&BridgeSession> for Settings {
    fn from(session: &BridgeSession) -> Self {
        let surrounding = surrounding(session).map(|window| {
            let (mut cursor, mut anchor) = (window.cursor, window.anchor);
            let mut text = window.text.to_owned();
            if session.marked_range().is_some() {
                let start = cursor.min(anchor);
                let end = cursor.max(anchor);
                text.replace_range(start..end, "");
                cursor = start;
                anchor = start;
            }
            Surrounding { text, cursor: cursor as i32, anchor: anchor as i32 }
        });
        let caret = session
            .caret
            .map(|rect| [rect.x.floor() as i32, rect.y.floor() as i32, rect.w.ceil().max(1.0) as i32, rect.h.ceil().max(1.0) as i32]);
        Self { surrounding, caret, hint: content_hint(session), purpose: content_purpose(session) }
    }
}

fn surrounding(session: &BridgeSession) -> Option<SurroundingText<'_>> {
    let (anchor, cursor) = session.editor().selection();
    if sensitive(session) || anchor.abs_diff(cursor) > SURROUNDING_LIMIT {
        return None;
    }
    session.surrounding_text(SURROUNDING_LIMIT).filter(|window| !window.text.contains('\0'))
}

fn sensitive(session: &BridgeSession) -> bool {
    session.secure || matches!(session.traits.content_type, TextContentType::Password | TextContentType::NewPassword)
}

fn content_hint(view: &BridgeSession) -> ContentHint {
    let traits = view.traits;
    let mut flags = ContentHint::empty();
    flags.set(ContentHint::Multiline, view.multiline);
    flags.set(ContentHint::HiddenText | ContentHint::SensitiveData, sensitive(view));
    if !sensitive(view) {
        if traits.autocorrect == Autocorrect::On {
            flags |= ContentHint::Spellcheck | ContentHint::Completion;
        }
        flags |= match traits.autocapitalize {
            Some(AutoCapitalize::Sentences) => ContentHint::AutoCapitalization,
            Some(AutoCapitalize::Words) => ContentHint::Titlecase,
            Some(AutoCapitalize::Characters) => ContentHint::Uppercase,
            _ => ContentHint::empty(),
        };
    }
    flags
}

fn content_purpose(view: &BridgeSession) -> ContentPurpose {
    use ContentPurpose as Purpose;

    let traits = view.traits;
    if sensitive(view) {
        return if traits.keyboard_type == KeyboardType::Number { Purpose::Pin } else { Purpose::Password };
    }
    match traits.keyboard_type {
        KeyboardType::Number => Purpose::Digits,
        KeyboardType::Decimal => Purpose::Number,
        KeyboardType::Phone => Purpose::Phone,
        KeyboardType::Url => Purpose::Url,
        KeyboardType::Email => Purpose::Email,
        _ => match traits.content_type {
            TextContentType::Email => Purpose::Email,
            TextContentType::Telephone => Purpose::Phone,
            TextContentType::Url => Purpose::Url,
            TextContentType::OneTimeCode => Purpose::Digits,
            _ => Purpose::Normal,
        },
    }
}

#[derive(Default)]
struct Pending {
    preedit: Option<(String, i32, i32)>,
    commit: Option<String>,
    delete: (u32, u32),
}

pub(super) struct Ime {
    native: Option<ZwpTextInputV3>,
    surface: Option<wl_surface::WlSurface>,
    owner: Option<EditorSessionId>,
    enabled_at: u32,
    serial: u32,
    confirmed: bool,
    settings: Option<Settings>,
    sent_surrounding: VecDeque<(u32, Option<SurroundingBasis>)>,
    pending: Pending,
    handling_update: bool,
}

pub(super) struct Update {
    surface: wl_surface::WlSurface,
    owner: EditorSessionId,
    seat: ObjectId,
    pending: Pending,
    serial: Option<u32>,
}

impl Ime {
    pub(super) fn new(shared: &Shared, seat: &wl_seat::WlSeat) -> Self {
        let native = shared.text_input.as_ref().map(|manager| manager.get_text_input(seat, &shared.queue, seat.id()));
        Self {
            native,
            surface: None,
            owner: None,
            enabled_at: 0,
            serial: 0,
            confirmed: true,
            settings: None,
            sent_surrounding: VecDeque::new(),
            pending: Pending::default(),
            handling_update: false,
        }
    }

    fn sync(&mut self, editor: Option<&BridgeSession>) {
        let Some(native) = self.native.as_ref() else { return };
        let owner = editor.map(|editor| editor.id);
        let new_owner = self.owner != owner;
        if new_owner {
            if self.owner.is_some() {
                native.disable();
                native.commit();
                self.serial = self.serial.wrapping_add(1);
            }
            self.owner = owner;
            self.pending = Pending::default();
            self.settings = None;
            self.sent_surrounding.clear();
            self.confirmed = true;
            if owner.is_some() {
                native.enable();
                self.enabled_at = self.serial.wrapping_add(1);
            }
        }
        if !self.confirmed {
            return;
        }
        let Some(editor) = editor else { return };
        let settings = Settings::from(editor);
        let basis = settings.surrounding.as_ref().map(|_| SurroundingBasis::from(editor));
        let previous_basis = self.sent_surrounding.back().and_then(|(_, basis)| *basis);
        if self.settings.as_ref() == Some(&settings) && previous_basis == basis {
            return;
        }

        let previous = self.settings.as_ref();
        if previous.is_none_or(|previous| previous.surrounding != settings.surrounding) || previous_basis != basis {
            if let Some(surrounding) = &settings.surrounding {
                native.set_surrounding_text(surrounding.text.clone(), surrounding.cursor, surrounding.anchor);
                if !self.handling_update && !new_owner {
                    native.set_text_change_cause(protocol::ChangeCause::Other);
                }
            } else if previous.is_some_and(|previous| previous.surrounding.is_some()) {
                native.set_surrounding_text(String::new(), 0, 0);
            }
        }
        if previous.is_none_or(|previous| (previous.hint, previous.purpose) != (settings.hint, settings.purpose)) {
            native.set_content_type(settings.hint, settings.purpose);
        }
        if previous.is_none_or(|previous| previous.caret != settings.caret)
            && let Some([x, y, width, height]) = settings.caret
        {
            native.set_cursor_rectangle(x, y, width, height);
        }
        native.commit();
        self.serial = self.serial.wrapping_add(1);
        if self.sent_surrounding.len() == SURROUNDING_HISTORY {
            self.sent_surrounding.pop_front();
        }
        self.sent_surrounding.push_back((self.serial, basis));
        self.settings = Some(settings);
    }

    pub(super) fn leave(&mut self, shared: &Shared, seat: ObjectId) {
        if let (Some(surface), Some(owner)) = (self.surface.take(), self.owner.take()) {
            shared.events.borrow_mut().push_back(NativeEvent::Ime(Update {
                surface,
                owner,
                seat,
                pending: Pending::default(),
                serial: None,
            }));
        }
        self.settings = None;
        self.sent_surrounding.clear();
        self.pending = Pending::default();
        self.confirmed = true;
    }
}

impl Drop for Ime {
    fn drop(&mut self) {
        if let Some(native) = &self.native {
            native.destroy();
        }
    }
}

impl Shared {
    pub(super) fn sync_ime(&self, window: &WindowState) {
        for seat in self.seats.borrow().values() {
            let mut seat = seat.borrow_mut();
            if seat.ime.surface.as_ref() == Some(window.native.wl_surface()) && !seat.ime.handling_update {
                let editor = window.editor.borrow();
                seat.ime.sync(if window.closed.get() { None } else { editor.session() });
            }
        }
    }
}

impl Update {
    pub(super) fn apply(self, shared: &Shared, host: &mut dyn OsHost, driver: &mut dyn ShellDriver) -> Result<(), String> {
        let Some(window) = shared.window(&self.surface) else { return Ok(()) };
        let Some(seat) = shared.seat(&self.seat).filter(|_| self.serial.is_some()) else {
            {
                let mut editor = window.editor.borrow_mut();
                if editor.is_composing() && editor.session().is_some_and(|session| session.id == self.owner) {
                    editor.edit(TextEdit::Cancel);
                }
            }
            return flush_editor(&window, self.owner, host, driver);
        };

        let delete_basis = {
            let mut seat = seat.borrow_mut();
            let ime = &mut seat.ime;
            ime.handling_update = true;
            if ime.owner == Some(self.owner) && ime.surface.as_ref() == Some(&self.surface) {
                ime.confirmed = self.serial == Some(ime.serial);
                ime.sent_surrounding.iter().find(|(serial, _)| self.serial == Some(*serial)).and_then(|(_, basis)| *basis)
            } else {
                None
            }
        };
        {
            let mut editor = window.editor.borrow_mut();
            if editor
                .session()
                .filter(|session| session.id == self.owner)
                .is_some_and(|session| self.pending.valid_delete(session, delete_basis))
            {
                if (self.pending.preedit.is_some() || self.pending.commit.is_some() || self.pending.delete != (0, 0))
                    && let Some(compose) = &window.compose
                {
                    compose.reset();
                }
                self.pending.apply(&mut editor);
            }
        }
        let result = flush_editor(&window, self.owner, host, driver);
        {
            let mut seat = seat.borrow_mut();
            if seat.ime.surface.as_ref() == Some(&self.surface) {
                let editor = window.editor.borrow();
                seat.ime.sync(if window.closed.get() { None } else { editor.session() });
            }
            seat.ime.handling_update = false;
        }
        result
    }
}

impl Pending {
    fn valid_delete(&self, session: &BridgeSession, basis: Option<SurroundingBasis>) -> bool {
        let (before, after) = self.delete;
        if (before, after) == (0, 0) {
            return true;
        }
        if basis != Some(SurroundingBasis::from(session)) {
            return false;
        }
        surrounding(session).is_some_and(|surrounding| {
            let start = surrounding.cursor.min(surrounding.anchor).checked_sub(before as usize);
            let end = surrounding.cursor.max(surrounding.anchor).checked_add(after as usize);
            start.is_some_and(|start| surrounding.text.is_char_boundary(start))
                && end.is_some_and(|end| surrounding.text.is_char_boundary(end))
        })
    }

    fn apply(self, editor: &mut HostTextBridge) {
        let (before, after) = self.delete;
        let preedit = self.preedit.filter(|(text, ..)| !text.is_empty());
        // Deletion uses the committed surrounding text. A commit by itself can
        // replace the current marked range directly.
        if editor.is_composing()
            && ((before, after) != (0, 0) || (preedit.is_none() && self.commit.is_none()))
            && !editor.edit(TextEdit::Cancel)
        {
            return;
        }
        if (before, after) != (0, 0)
            && !editor.edit(TextEdit::DeleteSurrounding { before_bytes: before as usize, after_bytes: after as usize })
        {
            return;
        }
        if let Some(text) = self.commit.filter(|text| !text.is_empty() || editor.is_composing())
            && !editor.edit(TextEdit::Insert { text, replacement: None })
        {
            return;
        }
        if let Some((text, start, end)) = preedit {
            editor.edit(mark(text, start, end));
        }
    }
}

fn mark(text: String, start: i32, end: i32) -> TextEdit {
    let selection = (start >= 0 && end >= 0).then(|| text.floor_char_boundary(start as usize)..text.floor_char_boundary(end as usize));
    let mut clauses = smallvec::SmallVec::new();
    let active = selection.as_ref().filter(|range| range.start != range.end);
    if let Some(active) = active {
        let (begin, end) = (active.start.min(active.end), active.start.max(active.end));
        for (range, style) in
            [(0..begin, ClauseStyle::Clause), (begin..end, ClauseStyle::ActiveClause), (end..text.len(), ClauseStyle::Clause)]
        {
            if !range.is_empty() {
                clauses.push(MarkedClause { range, style });
            }
        }
    } else {
        clauses.push(MarkedClause { range: 0..text.len(), style: ClauseStyle::Clause });
    }
    TextEdit::Mark { text, selection, replacement: None, clauses }
}

fn flush_editor(window: &WindowState, owner: EditorSessionId, host: &mut dyn OsHost, driver: &mut dyn ShellDriver) -> Result<(), String> {
    loop {
        let event = {
            let mut bridge = window.editor.borrow_mut();
            if window.closed.get() || bridge.session().is_none_or(|session| session.id != owner) {
                return Ok(());
            }
            bridge.take()
        };
        let Some(Input::Editor(event)) = event else { return Ok(()) };
        driver.event(host, PlatformEvent::Editor { window: window.id, event })?;
    }
}

impl Dispatch<ZwpTextInputV3, ObjectId> for Protocol {
    fn event(state: &mut Self, _: &ZwpTextInputV3, event: protocol::Event, data: &ObjectId, _: &Connection, _: &QueueHandle<Self>) {
        use protocol::Event::*;

        let Some(seat) = state.shared.seat(data) else { return };
        let mut seat = seat.borrow_mut();
        let ime = &mut seat.ime;
        match event {
            Enter { surface } => {
                if ime.surface.as_ref() != Some(&surface) {
                    ime.leave(&state.shared, data.clone());
                    if let Some(window) = state.shared.window(&surface) {
                        ime.surface = Some(surface);
                        ime.sync(window.editor.borrow().session());
                    }
                }
            }
            Leave { .. } => ime.leave(&state.shared, data.clone()),
            PreeditString { text, cursor_begin, cursor_end } => ime.pending.preedit = text.map(|text| (text, cursor_begin, cursor_end)),
            CommitString { text } => ime.pending.commit = text,
            DeleteSurroundingText { before_length, after_length } => ime.pending.delete = (before_length, after_length),
            Done { serial } => {
                let pending = std::mem::take(&mut ime.pending);
                if let (Some(surface), Some(owner)) = (&ime.surface, ime.owner)
                    && serial.wrapping_sub(ime.enabled_at) < (1 << 31)
                {
                    state.shared.events.borrow_mut().push_back(NativeEvent::Ime(Update {
                        surface: surface.clone(),
                        owner,
                        seat: data.clone(),
                        pending,
                        serial: Some(serial),
                    }));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::ActiveEditorView;
    use octara::tree::editor::EditorSlot;
    use octara_model::identity::{NodeId, TreeId};

    #[test]
    fn relative_delete_requires_the_sent_committed_context() {
        let editor = EditorSlot::new("abc😀");
        let view = ActiveEditorView {
            session: EditorSessionId { tree: TreeId::new([19, 4], 1), node: NodeId::default(), session_gen: editor.session_gen() },
            document_rev: editor.document_rev(),
            value: editor.value(),
            selection: editor.selection(),
            composition: editor.composition(),
            editor: &editor,
            multiline: false,
            secure: false,
            caret: None,
            geometry: None,
        };
        let mut bridge = HostTextBridge::default();
        bridge.sync(Some(&view));
        assert!(bridge.select((1, 1)));
        let sent = SurroundingBasis::from(bridge.session().unwrap());
        let delete = Pending { delete: (1, 0), ..Pending::default() };
        assert!(delete.valid_delete(bridge.session().unwrap(), Some(sent)));

        assert!(bridge.select((3, 3)));
        let session = bridge.session().unwrap();
        assert!(!delete.valid_delete(session, Some(sent)));
        assert!(!delete.valid_delete(session, None));
        assert!(delete.valid_delete(session, Some(SurroundingBasis::from(session))));

        let sent = SurroundingBasis::from(session);
        let delete_emoji = Pending { delete: (0, 4), ..Pending::default() };
        assert!(bridge.edit(mark("漢".into(), 3, 3)));
        assert!(delete_emoji.valid_delete(bridge.session().unwrap(), Some(sent)));
        let split_emoji = Pending { delete: (0, 3), ..Pending::default() };
        assert!(!split_emoji.valid_delete(bridge.session().unwrap(), Some(sent)));
        delete_emoji.apply(&mut bridge);
        let session = bridge.session().unwrap();
        assert_eq!(session.editor().value(), "abc");
        assert!(!delete.valid_delete(session, Some(sent)));
    }

    #[test]
    fn preedit_clauses_respect_utf8_boundaries_and_direction() {
        let TextEdit::Mark { selection, clauses, .. } = mark("a\u{6f22}\u{5b57}z".into(), 7, 2) else { unreachable!() };
        assert_eq!(selection.map(|range| (range.start, range.end)), Some((7, 1)));
        assert_eq!(
            clauses.as_slice(),
            &[
                MarkedClause { range: 0..1, style: ClauseStyle::Clause },
                MarkedClause { range: 1..7, style: ClauseStyle::ActiveClause },
                MarkedClause { range: 7..8, style: ClauseStyle::Clause },
            ]
        );

        let TextEdit::Mark { selection, clauses, .. } = mark("a".into(), -1, -1) else { unreachable!() };
        assert!(selection.is_none());
        assert_eq!(clauses.as_slice(), &[MarkedClause { range: 0..1, style: ClauseStyle::Clause }]);
    }
}
