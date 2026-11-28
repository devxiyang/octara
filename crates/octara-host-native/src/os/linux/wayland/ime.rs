use super::{NativeEvent, Protocol, Shared, window::WindowState};
use crate::os::{ActiveEditorView, OsHost, PlatformEvent, ShellDriver};
use octara::tree::editor::EditorRope;
use octara_model::identity::EditorSessionId;
use octara_model::input::EditorEvent;
use octara_model::text::{AutoCapitalize, Autocorrect, KeyboardType, TextContentType, TextEdit};
use smithay_client_toolkit::shell::WaylandSurface;
use std::ops::Range;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::zwp_text_input_v3::{self as protocol, ContentHint, ContentPurpose, ZwpTextInputV3};

pub(super) struct EditorState {
    session: EditorSessionId,
    rev: u64,
    pub marked: Option<Range<usize>>,
    selection: Range<usize>,
    displayed: EditorRope,
    settings: Settings,
}

#[derive(Clone, PartialEq, Eq)]
struct Settings {
    surrounding: Option<Surrounding>,
    caret: Option<[i32; 4]>,
    hint: ContentHint,
    purpose: ContentPurpose,
}

#[derive(Clone, PartialEq, Eq)]
struct Surrounding {
    text: String,
    cursor: i32,
    anchor: i32,
}

impl From<&ActiveEditorView<'_>> for EditorState {
    fn from(view: &ActiveEditorView<'_>) -> Self {
        let (anchor, cursor) = view.selection;
        let selection = anchor.min(cursor)..anchor.max(cursor);
        let marked = view.composition.map(|composition| composition.marked());
        let displayed = view.editor.displayed_rope().clone();
        let settings = Settings {
            surrounding: surrounding(view, &selection, marked.as_ref()),
            caret: view
                .caret
                .map(|rect| [rect.x.floor() as i32, rect.y.floor() as i32, rect.w.ceil().max(1.0) as i32, rect.h.ceil().max(1.0) as i32]),
            hint: content_hint(view),
            purpose: content_purpose(view),
        };
        Self { session: view.session, rev: view.document_rev, marked, selection, displayed, settings }
    }
}

fn surrounding(view: &ActiveEditorView<'_>, selection: &Range<usize>, marked: Option<&Range<usize>>) -> Option<Surrounding> {
    const LIMIT: usize = 4000;

    if view.secure || selection.len() > LIMIT {
        return None;
    }
    let (anchor, cursor) = view.selection;
    let prefix = (LIMIT - selection.len()) / 2;
    let start = view.value.ceil_char_boundary(selection.start.saturating_sub(prefix));
    let end = view.value.floor_char_boundary(view.value.len().min(start + LIMIT));
    if end < selection.end {
        return None;
    }
    let text = &view.value[start..end];
    if text.contains('\0') {
        return None;
    }
    if marked.is_some() {
        let relative = selection.start - start;
        let after = selection.end - start;
        let mut text = text.to_owned();
        text.replace_range(relative..after, "");
        return Some(Surrounding { text, cursor: relative as i32, anchor: relative as i32 });
    }
    Some(Surrounding { text: text.to_owned(), cursor: (cursor - start) as i32, anchor: (anchor - start) as i32 })
}

fn content_hint(view: &ActiveEditorView<'_>) -> ContentHint {
    let traits = view.editor.input_traits;
    let mut flags = ContentHint::empty();
    flags.set(ContentHint::Multiline, view.multiline);
    flags.set(ContentHint::HiddenText | ContentHint::SensitiveData, view.secure);
    if !view.secure {
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

fn content_purpose(view: &ActiveEditorView<'_>) -> ContentPurpose {
    use ContentPurpose as Purpose;

    let traits = view.editor.input_traits;
    if view.secure || matches!(traits.content_type, TextContentType::Password | TextContentType::NewPassword) {
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
    pending: Pending,
    handling_update: bool,
}

pub(super) struct Update {
    surface: wl_surface::WlSurface,
    owner: EditorSessionId,
    seat: ObjectId,
    pending: Pending,
    cancelled: bool,
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
            pending: Pending::default(),
            handling_update: false,
        }
    }

    fn sync(&mut self, editor: Option<&EditorState>) {
        let Some(native) = self.native.as_ref() else { return };
        let owner = editor.map(|editor| editor.session);
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
            self.confirmed = true;
            if owner.is_some() {
                native.enable();
                self.enabled_at = self.serial.wrapping_add(1);
            }
        }
        let Some(settings) = editor.map(|editor| &editor.settings) else { return };
        if !self.confirmed || self.settings.as_ref() == Some(settings) {
            return;
        }

        let previous = self.settings.as_ref();
        if previous.is_none_or(|previous| previous.surrounding != settings.surrounding) {
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
        self.settings = Some(settings.clone());
    }

    pub(super) fn leave(&mut self, shared: &Shared, seat: ObjectId) {
        if let (Some(surface), Some(owner)) = (self.surface.take(), self.owner.take()) {
            shared.events.borrow_mut().push_back(NativeEvent::Ime(Update {
                surface,
                owner,
                seat,
                pending: Pending::default(),
                cancelled: true,
            }));
        }
        self.settings = None;
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
                seat.ime.sync(if window.closed.get() { None } else { editor.as_ref() });
            }
        }
    }
}

impl Update {
    pub(super) fn apply(self, shared: &Shared, host: &mut dyn OsHost, driver: &mut dyn ShellDriver) -> Result<(), String> {
        let Some(window) = shared.window(&self.surface) else { return Ok(()) };
        let Some(seat) = shared.seat(&self.seat) else {
            return self.cancel(&window, host, driver);
        };
        if self.cancelled {
            return self.cancel(&window, host, driver);
        }
        let deliver = |driver: &mut dyn ShellDriver, host: &mut dyn OsHost, edit| {
            driver.event(host, PlatformEvent::Editor { window: window.id, event: edit }).map(|_| ())
        };
        let (editor, commit) = {
            let editor = window.editor.borrow();
            let Some(editor) = editor.as_ref().filter(|editor| editor.session == self.owner) else { return Ok(()) };
            let commit = self.pending.commit.is_some() || self.pending.delete != (0, 0);
            let edit = if commit {
                let replacement = if self.pending.delete == (0, 0) {
                    None
                } else {
                    let range = editor.marked.as_ref().unwrap_or(&editor.selection);
                    let (before, after) = self.pending.delete;
                    let Some(start) = range.start.checked_sub(before as usize) else { return Ok(()) };
                    let Some(end) = range.end.checked_add(after as usize) else { return Ok(()) };
                    if editor.settings.surrounding.is_none()
                        || !editor.displayed.is_char_boundary(start)
                        || !editor.displayed.is_char_boundary(end)
                    {
                        return Ok(());
                    }
                    Some(start..end)
                };
                Some(EditorEvent::ProposeEdit {
                    session: editor.session,
                    expected_rev: editor.rev,
                    edit: TextEdit::Insert { text: self.pending.commit.unwrap_or_default(), replacement },
                })
            } else {
                None
            };
            (edit, commit)
        };
        seat.borrow_mut().ime.handling_update = true;
        let result = (|| {
            if let Some(edit) = editor {
                deliver(driver, host, edit)?;
            }
            let edit = {
                let editor = window.editor.borrow();
                let Some(editor) = editor.as_ref().filter(|editor| editor.session == self.owner) else { return Ok(()) };
                if let Some((text, start, end)) = self.pending.preedit.filter(|(text, ..)| !text.is_empty()) {
                    let selection = (start >= 0 && end >= 0)
                        .then(|| (text.floor_char_boundary(start as usize), text.floor_char_boundary(end as usize)));
                    Some(EditorEvent::Composition { session: editor.session, expected_rev: editor.rev, text, selection, replacement: None })
                } else if !commit && editor.marked.is_some() {
                    Some(EditorEvent::CancelComposition { session: editor.session, expected_rev: editor.rev })
                } else {
                    None
                }
            };
            if let Some(edit) = edit {
                deliver(driver, host, edit)?;
            }
            Ok(())
        })();
        {
            let mut seat = seat.borrow_mut();
            if seat.ime.surface.as_ref() == Some(&self.surface) {
                let editor = window.editor.borrow();
                seat.ime.sync(if window.closed.get() { None } else { editor.as_ref() });
            }
            seat.ime.handling_update = false;
        }
        result
    }

    fn cancel(&self, window: &WindowState, host: &mut dyn OsHost, driver: &mut dyn ShellDriver) -> Result<(), String> {
        let event = window
            .editor
            .borrow()
            .as_ref()
            .filter(|editor| editor.session == self.owner && editor.marked.is_some())
            .map(|editor| EditorEvent::CancelComposition { session: editor.session, expected_rev: editor.rev });
        if let Some(event) = event {
            driver.event(host, PlatformEvent::Editor { window: window.id, event })?;
        }
        Ok(())
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
                        ime.sync(window.editor.borrow().as_ref());
                    }
                }
            }
            Leave { .. } => ime.leave(&state.shared, data.clone()),
            PreeditString { text, cursor_begin, cursor_end } => ime.pending.preedit = text.map(|text| (text, cursor_begin, cursor_end)),
            CommitString { text } => ime.pending.commit = text,
            DeleteSurroundingText { before_length, after_length } => ime.pending.delete = (before_length, after_length),
            Done { serial } => {
                let pending = std::mem::take(&mut ime.pending);
                ime.confirmed = ime.serial == serial;
                if let (Some(surface), Some(owner)) = (&ime.surface, ime.owner)
                    && serial.wrapping_sub(ime.enabled_at) < (1 << 31)
                {
                    state.shared.events.borrow_mut().push_back(NativeEvent::Ime(Update {
                        surface: surface.clone(),
                        owner,
                        seat: data.clone(),
                        pending,
                        cancelled: false,
                    }));
                }
            }
            _ => {}
        }
    }
}
