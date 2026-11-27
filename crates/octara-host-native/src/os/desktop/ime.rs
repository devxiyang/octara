//! winit owns the system input context. Each context belongs to one focused session.

use crate::os::ActiveEditorView;
use octara_model::identity::EditorSessionId;
use octara_model::input::EditorEvent;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::Ime;
use winit::window::Window;

#[derive(Default)]
pub(super) struct WinitIme {
    input: Option<(EditorSessionId, u64)>,
    owner: Option<EditorSessionId>,
    composing: bool,
}

impl WinitIme {
    pub(super) fn sync(&mut self, window: &Window, editor: Option<&ActiveEditorView<'_>>) {
        let next = editor.filter(|editor| !editor.secure);
        let next_owner = next.map(|editor| editor.session);
        let previous_owner = self.input.map(|(session, _)| session);
        if previous_owner != next_owner {
            self.owner = None;
            if previous_owner.is_some() {
                window.set_ime_allowed(false);
            }
            if next.is_some() {
                window.set_ime_allowed(true);
            }
        }
        self.input = next.map(|editor| (editor.session, editor.document_rev));
        self.composing = next.is_some_and(|editor| editor.composition.is_some());
        if let Some(caret) = next.and_then(|editor| editor.caret) {
            window.set_ime_cursor_area(LogicalPosition::new(caret.x, caret.y), LogicalSize::new(caret.w.max(1.0), caret.h.max(1.0)));
        }
    }

    pub(super) fn event(&mut self, event: &Ime) -> Option<EditorEvent> {
        let (session, expected_rev) = self.input?;
        match event {
            Ime::Enabled => {
                self.owner = Some(session);
                None
            }
            Ime::Disabled => {
                let current = self.owner.take() == Some(session);
                (current && self.composing).then_some(EditorEvent::CancelComposition { session, expected_rev })
            }
            Ime::Preedit(text, selection) if self.owner == Some(session) => {
                if text.is_empty() {
                    return self.composing.then_some(EditorEvent::CancelComposition { session, expected_rev });
                }
                let selection = selection.map(|(start, end)| (text.floor_char_boundary(start), text.floor_char_boundary(end)));
                Some(EditorEvent::Composition { session, expected_rev, text: text.clone(), selection, replacement: None })
            }
            Ime::Commit(text) if self.owner == Some(session) && !text.is_empty() => {
                Some(EditorEvent::Commit { session, expected_rev, text: text.clone() })
            }
            _ => None,
        }
    }

    pub(super) fn inspect(&self, window: &Window) -> serde_json::Value {
        serde_json::json!({
            "allowed": self.input.is_some(),
            "native_focus": window.has_focus(),
            "has_marked": self.composing,
        })
    }
}
