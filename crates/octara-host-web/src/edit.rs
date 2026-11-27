//! Delegates editing to native browser controls and admits proposals against the active session.

use crate::{Driver, dom, event_element, modifiers, rich, services::clipboard_image, undo::DomUndoLog};
use octara::App;
use octara::runtime::{Input, WindowId, turn::TurnError};
use octara::tree::{
    Tree,
    editor::{EditorSession, EditorSlot},
    selection::Address,
};
use octara_model::ask::ClipboardItem;
use octara_model::identity::{EditorSessionId, NodeId};
use octara_model::input::{EditorEvent, HostReply, InteractionEvent, Key, KeyEvent, Modifiers, PasteEvent};
use octara_model::semantics::TextPosition;
use octara_model::text::{CommitKind, TextEdit};
use std::cell::Cell;
use std::rc::Rc;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    ClipboardEvent, CompositionEvent, Element, Event, HtmlElement, HtmlInputElement, HtmlTextAreaElement, InputEvent as BrowserInput,
    KeyboardEvent, Node,
};

#[derive(Default)]
pub(crate) struct Editor {
    before: Cell<Option<Stamp>>,
    composition: Cell<Option<Stamp>>,
    ending: Cell<Option<Stamp>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    session: EditorSessionId,
    rev: u64,
}

impl Stamp {
    fn read(app: &App, window: WindowId, node: NodeId) -> Option<Self> {
        let window = app.runtime().window(window)?;
        window.settled()?;
        let session = EditorSession::active(window.tree())?;
        (session.id().node == node).then(|| Self { session: session.id(), rev: session.editor().document_rev() })
    }

    fn current(self, app: &App, window: WindowId) -> bool {
        Self::read(app, window, self.session.node) == Some(self)
    }
}

#[derive(PartialEq, Eq)]
struct PasteContext {
    window: WindowId,
    focus: Option<NodeId>,
    modal: Option<NodeId>,
    editor: Option<Stamp>,
}

impl PasteContext {
    fn read(app: &App, window: WindowId) -> Option<Self> {
        let tree = app.runtime().window(window)?.tree();
        let focus = tree.focus();
        Some(Self { window, focus, modal: tree.front_modal(), editor: focus.and_then(|node| Stamp::read(app, window, node)) })
    }
}

pub(crate) struct Field {
    node: NodeId,
    control: Control,
}

enum Control {
    Input(HtmlInputElement),
    Area(HtmlTextAreaElement),
    Rich(HtmlElement),
}

struct Observation {
    text: String,
    selection: (usize, usize),
}

impl Field {
    fn new(node: NodeId, element: Element) -> Option<Self> {
        let control = if let Some(input) = element.dyn_ref::<HtmlInputElement>() {
            if input.disabled() || input.read_only() || input.type_() == "checkbox" {
                return None;
            }
            Control::Input(input.clone())
        } else if let Some(area) = element.dyn_ref::<HtmlTextAreaElement>() {
            if area.disabled() || area.read_only() {
                return None;
            }
            Control::Area(area.clone())
        } else if element.has_attribute("data-octara-editor") && element.get_attribute("contenteditable").as_deref() == Some("true") {
            Control::Rich(element.dyn_into().ok()?)
        } else {
            return None;
        };
        Some(Self { node, control })
    }

    fn element(&self) -> &Element {
        match &self.control {
            Control::Input(input) => input,
            Control::Area(area) => area,
            Control::Rich(element) => element,
        }
    }

    fn active(&self) -> bool {
        self.element()
            .owner_document()
            .and_then(|document| document.active_element())
            .is_some_and(|element| self.element().is_same_node(Some(&element)))
    }

    fn read(&self) -> Option<Observation> {
        let (text, start, end, direction) = match &self.control {
            Control::Input(input) => (input.value(), input.selection_start(), input.selection_end(), input.selection_direction()),
            Control::Area(area) => (area.value(), area.selection_start(), area.selection_end(), area.selection_direction()),
            Control::Rich(element) => {
                return Some(Observation { text: rich::logical_text(element), selection: rich::selection(element)? });
            }
        };
        let start = byte_offset(&text, start.ok()??);
        let end = byte_offset(&text, end.ok()??);
        let selection = if direction.ok().flatten().as_deref() == Some("backward") { (end, start) } else { (start, end) };
        Some(Observation { text, selection })
    }

    fn restore(&self, tree: &Tree) {
        let Some(editor) = tree.editor(self.node) else { return };
        if let Control::Rich(element) = &self.control {
            let _ = dom::update_rich_editor(&mut DomUndoLog::default(), element, tree, self.node, false);
            return;
        }
        let (text, selection) = displayed(editor);
        let previous = self.read();
        let changed = previous.as_ref().is_none_or(|previous| previous.text != text);
        if changed {
            match &self.control {
                Control::Input(input) => input.set_value(text),
                Control::Area(area) => area.set_value(text),
                Control::Rich(_) => unreachable!(),
            }
        }
        let Some((anchor, caret)) = selection else { return };
        if !changed && previous.is_some_and(|previous| previous.selection == (anchor, caret)) {
            return;
        }
        let rope = editor.displayed_rope();
        let start = rope.byte_to_utf16(anchor.min(caret)).unwrap_or_default() as u32;
        let end = rope.byte_to_utf16(anchor.max(caret)).unwrap_or_default() as u32;
        let direction = if caret < anchor { "backward" } else { "forward" };
        let _ = match &self.control {
            Control::Input(input) => input.set_selection_range_with_direction(start, end, direction),
            Control::Area(area) => area.set_selection_range_with_direction(start, end, direction),
            Control::Rich(_) => unreachable!(),
        };
    }

    fn rich(&self) -> bool {
        matches!(self.control, Control::Rich(_))
    }
}

impl Driver {
    pub(crate) fn connect_editor(self: &Rc<Self>) -> Result<(), JsValue> {
        self.listen(self.mount.as_ref(), "focusin", false, |driver, _| driver.report_focus())?;
        self.listen(self.mount.as_ref(), "focusout", false, |driver, _| driver.defer(Self::report_focus))?;
        for target in [self.mount.as_ref(), self.document.as_ref()] {
            self.listen(target, "selectionchange", false, |driver, _| {
                if let Some(field) = driver.active_field()
                    && let Err(error) = driver.turn(|app, window| driver.sync_selection(app, window, &field))
                {
                    driver.report_error(error);
                }
            })?;
        }
        self.listen(self.document.as_ref(), "selectionchange", false, |driver, _| {
            if let Err(error) = driver.turn(|app, window| driver.sync_document_selection(app, window)) {
                driver.report_error(error);
            }
        })?;
        self.listen(self.mount.as_ref(), "select", false, |driver, event| {
            if let Some(field) = driver.event_field(&event)
                && let Err(error) = driver.turn(|app, window| driver.sync_selection(app, window, &field))
            {
                driver.report_error(error);
            }
        })?;
        self.listen(self.mount.as_ref(), "beforeinput", false, |driver, event| {
            let Some(field) = driver.event_field(&event) else { return };
            let Some(input) = event.dyn_ref::<BrowserInput>() else { return };
            if input.is_composing() || is_composition_input(&input.input_type()) || driver.editor.ending.get().is_some() {
                return;
            }
            driver.editor.before.set(None);
            if input.input_type() == "insertFromPaste"
                && input.cancelable()
                && let Some(text) = input.data_transfer().and_then(|data| data.get_data("text/plain").ok()).or_else(|| input.data())
            {
                event.prevent_default();
                if let Err(error) = driver.turn(|app, window| {
                    driver.sync_selection(app, window, &field)?;
                    let paste = PasteEvent { items: vec![ClipboardItem::Text(text)] };
                    app.dispatch(window, Input::Host(HostReply::Paste(paste)))?;
                    Ok(())
                }) {
                    driver.report_error(error);
                }
                return;
            }
            if field.rich() && input.cancelable() && driver.rich_edit(&field, input) {
                event.prevent_default();
                return;
            }
            if let Err(error) = driver.turn(|app, window| {
                driver.sync_selection(app, window, &field)?;
                let stamp = Stamp::read(app, window, field.node).filter(|_| {
                    field.read().zip(editor_slot(app, window, field.node)).is_some_and(|(observed, editor)| observed.text == editor.value())
                });
                driver.editor.before.set(stamp);
                Ok(())
            }) {
                driver.report_error(error);
            }
        })?;
        self.listen(self.mount.as_ref(), "input", false, |driver, event| {
            let Some(field) = driver.event_field(&event) else { return };
            let Some(input) = event.dyn_ref::<BrowserInput>() else { return };
            if input.is_composing() || is_composition_input(&input.input_type()) || driver.editor.composing() {
                return;
            }
            let before = driver.editor.before.take();
            let Some(observed) = field.read() else { return };
            if let Err(error) = driver.turn(|app, window| {
                if let Some(stamp) = before.filter(|stamp| stamp.session.node == field.node && stamp.current(app, window)) {
                    let kind = match input.input_type().as_str() {
                        "insertText" => CommitKind::Typing,
                        "deleteContentBackward" | "deleteContentForward" => CommitKind::Deleting,
                        _ => CommitKind::Other,
                    };
                    let edit = TextEdit::Commit { text: observed.text, selection: observed.selection, kind };
                    app.dispatch(
                        window,
                        Input::Editor(EditorEvent::ProposeEdit { session: stamp.session, expected_rev: stamp.rev, edit }),
                    )?;
                }
                if let Some(tree) = editor_tree(app, window) {
                    field.restore(tree);
                }
                Ok(())
            }) {
                driver.report_error(error);
            }
        })?;
        self.listen(self.mount.as_ref(), "compositionstart", false, |driver, event| {
            let Some(field) = driver.event_field(&event) else { return };
            if let Err(error) = driver.turn(|app, window| {
                driver.sync_selection(app, window, &field)?;
                let stamp = Stamp::read(app, window, field.node);
                driver.editor.before.set(None);
                driver.editor.ending.set(None);
                driver.editor.composition.set(stamp);
                driver.dom.set_composing(stamp.map(|stamp| stamp.session.node));
                Ok(())
            }) {
                driver.report_error(error);
            }
        })?;
        self.listen(self.mount.as_ref(), "compositionupdate", false, |driver, event| {
            let Some(field) = driver.event_field(&event) else { return };
            let Some(text) = event.dyn_ref::<CompositionEvent>().and_then(CompositionEvent::data) else { return };
            let Some(stamp) = driver.editor.composition.get().filter(|stamp| stamp.session.node == field.node) else { return };
            if let Err(error) = driver.turn(|app, window| {
                if stamp.current(app, window) {
                    let end = text.len();
                    app.dispatch(
                        window,
                        Input::Editor(EditorEvent::Composition {
                            session: stamp.session,
                            expected_rev: stamp.rev,
                            text,
                            selection: Some((end, end)),
                            replacement: None,
                        }),
                    )?;
                    driver.update_composition(app, window, stamp);
                }
                Ok(())
            }) {
                driver.report_error(error);
            }
        })?;
        self.listen(self.mount.as_ref(), "compositionend", false, |driver, event| {
            let Some(field) = driver.event_field(&event) else { return };
            let Some(text) = event.dyn_ref::<CompositionEvent>().and_then(CompositionEvent::data) else { return };
            let Some(stamp) = driver.editor.composition.get().filter(|stamp| stamp.session.node == field.node) else { return };
            driver.editor.composition.set(None);
            driver.editor.ending.set(Some(stamp));
            driver.defer(move |driver| driver.finish_composition(field, stamp, text));
        })?;
        self.listen(self.mount.as_ref(), "keydown", false, |driver, event| driver.keydown(&event))?;
        self.listen(self.document.as_ref(), "keydown", false, |driver, event| {
            // An accepted editor command can replace its event target before this listener runs.
            if !event.default_prevented()
                && event_element(&event).is_none_or(|element| !driver.mount.contains(Some(&element)))
                && driver
                    .document
                    .get_selection()
                    .ok()
                    .flatten()
                    .and_then(|selection| selection.anchor_node())
                    .and_then(|node| driver.dom_selection_node(node))
                    .is_some()
            {
                driver.keydown(&event);
            }
        })?;
        self.listen(self.document.as_ref(), "copy", false, |driver, event| {
            if !event.default_prevented()
                && let Some(clipboard) = event.dyn_ref::<ClipboardEvent>().and_then(ClipboardEvent::clipboard_data)
                && let Some(text) = driver.format_document_copy()
                && clipboard.set_data("text/plain", &text).is_ok()
            {
                event.prevent_default();
            }
        })?;
        for name in ["copy", "cut"] {
            self.listen(self.mount.as_ref(), name, false, move |driver, event| {
                if let Some(field) = driver.event_field(&event)
                    && field.rich()
                    && let Some(clipboard) = event.dyn_ref::<ClipboardEvent>().and_then(ClipboardEvent::clipboard_data)
                    && let Some(observed) = field.read()
                {
                    let (a, b) = observed.selection;
                    if a != b && clipboard.set_data("text/plain", &observed.text[a.min(b)..a.max(b)]).is_ok() {
                        event.prevent_default();
                        if name == "cut" {
                            driver.rich_key(&field, KeyEvent::new(Key::Delete, Modifiers::empty()));
                        }
                    }
                    return;
                }
                let Some(node) = event.target().and_then(|target| driver.dom.node(target.as_ref())) else { return };
                let Some(window) = driver.window.get() else { return };
                if driver
                    .app
                    .try_borrow()
                    .ok()
                    .and_then(|slot| Some(slot.as_ref()?.runtime().window(window)?.tree().editor(node)?.is_secure()))
                    == Some(true)
                {
                    event.prevent_default();
                }
            })?;
        }
        self.listen(self.mount.as_ref(), "paste", false, |driver, event| driver.paste(&event))?;
        Ok(())
    }

    fn paste(self: &Rc<Self>, event: &Event) {
        if event.default_prevented() {
            return;
        }
        let Some(clipboard) = event.dyn_ref::<ClipboardEvent>().and_then(ClipboardEvent::clipboard_data) else { return };
        let mut items = clipboard
            .get_data("text/plain")
            .ok()
            .filter(|text| !text.is_empty())
            .map_or_else(Vec::new, |text| vec![ClipboardItem::Text(text)]);
        let files = clipboard.files();
        let images: Vec<_> = files
            .into_iter()
            .flat_map(|files| (0..files.length()).filter_map(move |index| files.get(index)))
            .filter(|file| file.type_().starts_with("image/"))
            .collect();
        let field = self.event_field(event);
        if items.is_empty() && images.is_empty() && field.is_none() {
            return;
        }
        // Browser clipboard files expose a Blob and a basename, with no local path.
        event.prevent_default();
        let context = self.turn(|app, window| {
            if let Some(field) = &field {
                self.sync_selection(app, window, field)?;
            }
            if images.is_empty() {
                let paste = PasteEvent { items: std::mem::take(&mut items) };
                app.dispatch(window, Input::Host(HostReply::Paste(paste)))?;
                return Ok(None);
            }
            Ok(PasteContext::read(app, window))
        });
        let context = match context {
            Ok(Some(context)) => context,
            Ok(None) => return,
            Err(error) => {
                self.report_error(error);
                return;
            }
        };
        let driver = Rc::downgrade(self);
        spawn_local(async move {
            for file in images {
                if let Some(image) = clipboard_image(file.as_ref(), file.type_()).await {
                    items.push(image);
                }
            }
            let Some(driver) = driver.upgrade().filter(|driver| !driver.stopped.get()) else { return };
            if let Err(error) = driver.turn(|app, window| {
                if PasteContext::read(app, window) == Some(context) {
                    app.dispatch(window, Input::Host(HostReply::Paste(PasteEvent { items })))?;
                }
                Ok(())
            }) {
                driver.report_error(error);
            }
        });
    }

    fn rich_edit(&self, field: &Field, input: &BrowserInput) -> bool {
        match input.input_type().as_str() {
            "deleteContentBackward" => self.rich_key(field, KeyEvent::new(Key::Backspace, Modifiers::empty())),
            "deleteContentForward" => self.rich_key(field, KeyEvent::new(Key::Delete, Modifiers::empty())),
            "deleteWordBackward" => self.rich_key(field, KeyEvent::new(Key::Backspace, Modifiers::ALT)),
            "deleteWordForward" => self.rich_key(field, KeyEvent::new(Key::Delete, Modifiers::ALT)),
            "insertParagraph" | "insertLineBreak" => self.rich_insert(field, "\n".to_owned()),
            "insertFromPaste" => {}
            "insertFromDrop" => {
                if let Some(text) = input.data_transfer().and_then(|data| data.get_data("text/plain").ok()).or_else(|| input.data()) {
                    self.rich_insert(field, text);
                }
            }
            kind if kind.starts_with("format") => {}
            _ => return false,
        }
        true
    }

    fn rich_key(&self, field: &Field, key: KeyEvent) {
        if let Err(error) = self.turn(|app, window| {
            self.sync_selection(app, window, field)?;
            if let Some(stamp) = Stamp::read(app, window, field.node) {
                app.dispatch(window, Input::Editor(EditorEvent::KeyFallback { session: stamp.session, expected_rev: stamp.rev, key }))?;
            }
            Ok(())
        }) {
            self.report_error(error);
        }
    }

    fn rich_insert(&self, field: &Field, text: String) {
        if let Err(error) = self.turn(|app, window| {
            self.sync_selection(app, window, field)?;
            if let Some(stamp) = Stamp::read(app, window, field.node)
                && let Some(editor) = editor_slot(app, window, field.node)
            {
                let mut text = text.replace("\r\n", "\n").replace('\r', "\n");
                if !editor.multiline {
                    text.retain(|ch| ch != '\n');
                }
                if text.is_empty() {
                    return Ok(());
                }
                let edit = TextEdit::Insert { text, replacement: None };
                app.dispatch(window, Input::Editor(EditorEvent::ProposeEdit { session: stamp.session, expected_rev: stamp.rev, edit }))?;
            }
            Ok(())
        }) {
            self.report_error(error);
        }
    }

    fn report_focus(&self) {
        if let Err(error) = self.turn(|app, window| {
            let element = self.document.active_element();
            let visible = element.as_ref().is_some_and(|element| element.matches(":focus-visible").unwrap_or(false));
            let node = element.and_then(|element| self.dom.node(element.as_ref()));
            let Some(tree) = app.runtime().window(window).map(|window| window.tree()) else { return Ok(()) };
            let target = node.filter(|&node| tree.is_live(node)).map(|node| tree.guard(node));
            if tree.focus() == target.and_then(|target| target.node())
                && tree
                    .focus()
                    .and_then(|node| tree.node(node))
                    .is_none_or(|node| node.state.contains(octara_model::style::State::FOCUS_VISIBLE) == visible)
            {
                return Ok(());
            }
            app.dispatch(window, Input::Host(HostReply::FocusChanged { target, visible }))?;
            Ok(())
        }) {
            self.report_error(error);
        }
    }

    fn keydown(&self, event: &Event) {
        let Some(key) = event.dyn_ref::<KeyboardEvent>().filter(|key| !key.is_composing()) else { return };
        let Some(key) = browser_key_event(key) else { return };
        let field = self.event_field(event);
        let shortcut_key = key.modifiers.intersects(Modifiers::META | Modifiers::CTRL | Modifiers::ALT);
        let shortcut = self.services.borrow().shortcut(key);
        let handled = self.turn(|app, window| {
            let authored_shortcut = app.runtime().window(window).is_some_and(|window| window.has_shortcut(key));
            let managed_tab = app.runtime().window(window).is_some_and(|window| window.tree().manages_tab());
            if let Some(field) = field.as_ref() {
                if self.editor.composing() {
                    return Ok(false);
                }
                if shortcut_key
                    || key.requests_context_menu()
                    || authored_shortcut
                    || matches!(
                        key.key,
                        Key::Tab
                            | Key::Enter
                            | Key::Escape
                            | Key::Up
                            | Key::Down
                            | Key::Left
                            | Key::Right
                            | Key::Home
                            | Key::End
                            | Key::PageUp
                            | Key::PageDown
                    )
                {
                    self.sync_selection(app, window, field)?;
                    if let Some(stamp) = Stamp::read(app, window, field.node)
                        && app
                            .dispatch(
                                window,
                                Input::Editor(EditorEvent::KeyIntercept { session: stamp.session, expected_rev: stamp.rev, key }),
                            )?
                            .handled
                    {
                        return Ok(true);
                    }
                }
            }
            if field.is_none()
                && (key.key != Key::Tab || authored_shortcut || managed_tab)
                && app.dispatch(window, Input::Interaction(InteractionEvent::Key(key)))?.handled
            {
                return Ok(true);
            }
            // The browser keeps native editing and sequential focus for unhandled keys.
            if let Some((name, chord)) = shortcut
                && app.dispatch(window, Input::Host(HostReply::MenuCommand { name, chord: Some(chord) }))?.handled
            {
                return Ok(true);
            }
            Ok(field.is_none()
                && matches!(key.key, Key::Char('a' | 'A'))
                && matches!(key.modifiers, Modifiers::CTRL | Modifiers::META)
                && self.select_document_region(app, window))
        });
        match handled {
            Ok(true) => event.prevent_default(),
            Err(error) => self.report_error(error),
            Ok(false) => {}
        }
    }

    fn dom_selection_node(&self, node: Node) -> Option<NodeId> {
        std::iter::successors(Some(node), Node::parent_node).find_map(|node| self.dom.node(node.as_ref()))
    }

    fn select_document_region(&self, app: &App, window: WindowId) -> bool {
        let Some(tree) = app.runtime().window(window).map(|window| window.tree()) else { return false };
        let Some(selection) = self.document.get_selection().ok().flatten() else { return false };
        let Some(region) = selection
            .anchor_node()
            .and_then(|node| self.dom_selection_node(node))
            .and_then(|node| tree.region_of(node))
            .or_else(|| tree.focus().and_then(|node| tree.region_of(node)))
        else {
            return false;
        };
        let texts = tree.selectable_texts(region);
        let Some((first, last)) =
            texts.first().and_then(|&first| self.dom.element(first)).zip(texts.last().and_then(|&last| self.dom.element(last)))
        else {
            return false;
        };
        selection.set_base_and_extent(&first, 0, &last, last.child_nodes().length()).is_ok()
    }

    fn format_document_copy(&self) -> Option<String> {
        // A stale document selection can coexist with a focused field.
        if self.active_field().is_some() {
            return None;
        }
        let id = self.window.get()?;
        let mut state = self.app.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        let tree = app.runtime().window(id)?.tree();
        if let Some((region, selection)) = self.document_selection(tree) {
            return app.runtime_mut().format_document_selection(id, region, selection);
        }
        let selection = self.document.get_selection().ok()??;
        if selection.is_collapsed() {
            return None;
        }
        let (node, start) = self.document_address(tree, selection.anchor_node()?, selection.anchor_offset())?;
        let (end_node, end) = self.document_address(tree, selection.focus_node()?, selection.focus_offset())?;
        let slot = tree.text(node)?;
        (node == end_node && start != end && !slot.attachments.is_empty())
            .then(|| slot.text.get(start.min(end)..start.max(end)).map(str::to_owned))
            .flatten()
    }

    fn document_selection(&self, tree: &Tree) -> Option<(NodeId, [Address; 2])> {
        let selection = self.document.get_selection().ok()??;
        if selection.is_collapsed() {
            return None;
        }
        let anchor = self.document_address(tree, selection.anchor_node()?, selection.anchor_offset())?;
        let focus = self.document_address(tree, selection.focus_node()?, selection.focus_offset())?;
        let region = tree.region_of(anchor.0)?;
        (anchor != focus && tree.valid_doc_selection(region, [anchor, focus])).then_some((region, [anchor, focus]))
    }

    fn sync_document_selection(&self, app: &mut App, window: WindowId) -> Result<(), TurnError> {
        let Some(tree) = editor_tree(app, window) else { return Ok(()) };
        let observed = self.active_field().is_none().then(|| self.document_selection(tree)).flatten();
        let selection = if let Some((region, addresses)) = observed {
            if tree.doc_selection(region) == Some(addresses) {
                return Ok(());
            }
            Some((tree.guard(region), addresses.map(|(node, offset)| TextPosition { node, offset })))
        } else {
            if tree.selection_endpoints().next().is_none() {
                return Ok(());
            }
            None
        };
        app.dispatch(window, Input::Host(HostReply::DocumentSelection { selection }))?;
        Ok(())
    }

    fn document_address(&self, tree: &Tree, endpoint: Node, offset: u32) -> Option<Address> {
        let node = self.dom_selection_node(endpoint.clone())?;
        let node = if tree.link(node).is_some() { tree.physical_parent(node)? } else { node };
        let owner = self.dom.element(node)?;
        tree.text(node)?;
        Some((node, rich::logical_offset(&owner, &endpoint, offset)?))
    }

    fn event_field(&self, event: &Event) -> Option<Field> {
        let element = event_element(event)?;
        let element = element.closest("[data-octara-editor]").ok()??;
        let node = self.dom.node(element.as_ref())?;
        let field = Field::new(node, element)?;
        field.active().then_some(field)
    }

    pub(crate) fn active_field(&self) -> Option<Field> {
        let element = self.document.active_element()?;
        Field::new(self.dom.node(element.as_ref())?, element)
    }

    fn sync_selection(&self, app: &mut App, window: WindowId, field: &Field) -> Result<(), TurnError> {
        if self.editor.ending.get().is_some() {
            return Ok(());
        }
        let Some(stamp) = Stamp::read(app, window, field.node) else { return Ok(()) };
        let Some(editor) = editor_slot(app, window, field.node) else { return Ok(()) };
        let Some(observed) = field.read() else { return Ok(()) };
        let (text, selection) = displayed(editor);
        if observed.text == text && Some(observed.selection) != selection {
            app.dispatch(
                window,
                Input::Editor(EditorEvent::ProposeSelection {
                    session: stamp.session,
                    expected_rev: stamp.rev,
                    selection: observed.selection,
                }),
            )?;
            if self.editor.composition.get() == Some(stamp) {
                self.update_composition(app, window, stamp);
            }
        }
        Ok(())
    }

    fn update_composition(&self, app: &App, window: WindowId, stamp: Stamp) {
        let next = Stamp::read(app, window, stamp.session.node).filter(|next| next.session == stamp.session);
        self.editor.composition.set(next);
        self.dom.set_composing(next.map(|next| next.session.node));
    }

    pub(crate) fn reconcile_editor(&self, app: &App, window: WindowId) {
        let Some(settled) = app.runtime().window(window).map(|window| window.settled().is_some()) else { return };
        if !settled {
            return;
        }
        let stale = |entry: &Cell<Option<Stamp>>| {
            let stamp = entry.get().filter(|stamp| !stamp.current(app, window))?;
            entry.set(None);
            Some(stamp)
        };
        stale(&self.editor.before);
        let stale = stale(&self.editor.composition).or_else(|| stale(&self.editor.ending));
        if let Some(stamp) = stale {
            self.dom.set_composing(None);
            if let Some(field) = self.active_field().filter(|field| field.node == stamp.session.node)
                && let Some(tree) = editor_tree(app, window)
            {
                field.restore(tree);
            }
        }
    }

    fn finish_composition(&self, field: Field, stamp: Stamp, text: String) {
        if self.editor.ending.get() != Some(stamp) {
            return;
        }
        self.editor.ending.set(None);
        self.dom.set_composing(None);
        if let Err(error) = self.turn(|app, window| {
            if stamp.current(app, window) && field.active() {
                let session = stamp.session;
                let expected_rev = stamp.rev;
                let event = if text.is_empty() {
                    EditorEvent::CancelComposition { session, expected_rev }
                } else if let Some(observed) = field.read()
                    && editor_slot(app, window, field.node).is_some_and(|editor| matches_commit(editor, &text, &observed.text))
                {
                    EditorEvent::ProposeEdit {
                        session,
                        expected_rev,
                        edit: TextEdit::Commit { text: observed.text, selection: observed.selection, kind: CommitKind::Other },
                    }
                } else {
                    EditorEvent::Commit { session, expected_rev, text }
                };
                app.dispatch(window, Input::Editor(event))?;
            }
            if let Some(tree) = editor_tree(app, window) {
                field.restore(tree);
            }
            Ok(())
        }) {
            self.report_error(error);
        }
    }
}

impl Editor {
    fn composing(&self) -> bool {
        self.composition.get().is_some() || self.ending.get().is_some()
    }
}

fn editor_slot(app: &App, window: WindowId, node: NodeId) -> Option<&EditorSlot> {
    editor_tree(app, window)?.editor(node)
}

fn editor_tree(app: &App, window: WindowId) -> Option<&Tree> {
    let window = app.runtime().window(window)?;
    window.settled()?;
    Some(window.tree())
}

fn displayed(editor: &EditorSlot) -> (&str, Option<(usize, usize)>) {
    editor
        .composition()
        .map_or_else(|| (editor.value(), Some(editor.selection())), |composition| (composition.text(), composition.selection()))
}

fn matches_commit(editor: &EditorSlot, text: &str, observed: &str) -> bool {
    let (displayed, _) = displayed(editor);
    let range = editor.composition().map_or_else(
        || {
            let (anchor, focus) = editor.selection();
            anchor.min(focus)..anchor.max(focus)
        },
        |composition| composition.marked(),
    );
    observed.strip_prefix(&displayed[..range.start]).and_then(|rest| rest.strip_prefix(text)) == Some(&displayed[range.end..])
}

fn byte_offset(text: &str, mut offset: u32) -> usize {
    for (byte, ch) in text.char_indices() {
        let width = ch.len_utf16() as u32;
        if offset < width {
            return byte;
        }
        offset -= width;
    }
    text.len()
}

fn is_composition_input(kind: &str) -> bool {
    matches!(kind, "insertCompositionText" | "deleteCompositionText" | "insertFromComposition" | "deleteByComposition")
}

fn browser_key_event(event: &KeyboardEvent) -> Option<KeyEvent> {
    let name = event.key();
    let key = match name.as_str() {
        "Tab" => Key::Tab,
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        " " => Key::Space,
        "ArrowUp" => Key::Up,
        "ArrowDown" => Key::Down,
        "ArrowLeft" => Key::Left,
        "ArrowRight" => Key::Right,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "ContextMenu" => Key::Menu,
        name => {
            if let Some(number) = name.strip_prefix('F').and_then(|number| number.parse().ok()).filter(|number| (1..=24).contains(number)) {
                Key::F(number)
            } else {
                let mut chars = name.chars();
                let first = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                Key::Char(first)
            }
        }
    };
    let mods = modifiers(event.shift_key(), event.ctrl_key(), event.alt_key(), event.meta_key());
    let mut key = KeyEvent::new(key, mods);
    key.repeat = event.repeat();
    let code = event.code();
    key.code = match code.as_str() {
        "Backquote" => Some('`'),
        "Backslash" => Some('\\'),
        "BracketLeft" => Some('['),
        "BracketRight" => Some(']'),
        "Comma" => Some(','),
        "Equal" => Some('='),
        "Minus" => Some('-'),
        "Period" => Some('.'),
        "Quote" => Some('\''),
        "Semicolon" => Some(';'),
        "Slash" => Some('/'),
        _ => ["Key", "Digit"].iter().find_map(|prefix| {
            let mut chars = code.strip_prefix(prefix)?.chars();
            let first = chars.next()?;
            chars.next().is_none().then(|| first.to_ascii_lowercase())
        }),
    };
    Some(key)
}
