use super::{AppCommand, AppRuntime, HandlerEvent, HandlerKind, Routed, WindowRuntime};
use octara_model::{
    ask::{ClipboardItem, ClipboardKinds, ServiceOutcome, ServiceRequest},
    geometry::Point,
    identity::{EditorSessionId, TargetGuard, WindowId},
    input::{EditorCommand, EditorEvent, KeyEvent, PasteEvent, TextEdit},
};
use octara_tree::{
    NodeId, RuntimeOp, Tree,
    arbiter::{InteractionIntent, TextPointerIntent, TextPointerTarget},
    editor::{EditorSession, command_for_key},
    selection::{Address, DocSelection, paragraph_at, word_at},
};
use std::{collections::VecDeque, ops::Range};

#[derive(Default)]
pub(crate) struct TextInteraction {
    region: Option<TargetGuard>,
    drag: Option<TextDrag>,
}

#[derive(Clone, Copy)]
struct TextDrag {
    target: TextPointerTarget,
    span: DocSelection,
    count: u8,
    value_rev: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct PasteTarget {
    node: TargetGuard,
    editor: Option<(EditorSessionId, u64)>,
}

impl PasteTarget {
    fn current(window: &WindowRuntime) -> Option<Self> {
        if !window.arbiter.is_active() {
            return None;
        }
        let tree = &window.tree;
        let root = tree.root();
        let node = tree.focus().or_else(|| tree.front_modal()).unwrap_or_else(|| {
            tree.children(root).iter().copied().find(|&node| node != tree.overlay_root() && tree.is_interactive(node)).unwrap_or(root)
        });
        let editor = match EditorSession::active(tree) {
            Some(session) if session.editor().composition().is_some() => return None,
            Some(session) => Some((session.id(), session.editor().document_rev())),
            None => None,
        };
        tree.is_interactive(node).then(|| Self { node: tree.guard(node), editor })
    }

    fn handlers(self, tree: &Tree) -> impl Iterator<Item = octara_tree::tables::HandlerId> {
        std::iter::successors(self.node.node(), |&node| tree.logical_parent(node)).filter_map(|node| tree.handler(node, HandlerKind::Paste))
    }
}

impl AppRuntime {
    fn request_paste(&mut self, id: WindowId) -> bool {
        let window = &self.windows[&id];
        let Some(target) = PasteTarget::current(window) else { return false };
        let accept = if target.handlers(&window.tree).next().is_some() {
            ClipboardKinds::all()
        } else if target.editor.is_some() {
            ClipboardKinds::TEXT
        } else {
            return false;
        };
        self.write(window.scope, id).request_service(ServiceRequest::ReadClipboard { accept }, move |cx, outcome| {
            if let ServiceOutcome::Clipboard(items) = outcome {
                cx.commands().queued.push_back(AppCommand::Paste { window: id, target, event: PasteEvent { items } });
            }
        });
        true
    }

    pub(super) fn deliver_paste(
        &mut self,
        id: WindowId,
        expected: Option<PasteTarget>,
        event: PasteEvent,
        routed: &mut VecDeque<Routed>,
    ) -> bool {
        let Some(window) = self.windows.get(&id) else { return false };
        let Some(target) = PasteTarget::current(window) else { return false };
        if event.items.is_empty() || expected.is_some_and(|expected| expected != target) {
            return false;
        }
        let handlers = target.handlers(&window.tree).collect::<smallvec::SmallVec<[_; 2]>>();
        if handlers.into_iter().any(|handler| self.invoke(id, handler, HandlerEvent::Paste(&event)).consumed) {
            return true;
        }
        let Some((session, expected_rev)) = target.editor else { return false };
        let Some(mut text) = event.items.into_iter().find_map(|item| match item {
            ClipboardItem::Text(text) => Some(text),
            _ => None,
        }) else {
            return false;
        };
        if self.windows[&id].tree.editor(session.node).is_some_and(|editor| !editor.multiline) {
            text.retain(|ch| !matches!(ch, '\r' | '\n'));
        }
        !text.is_empty()
            && self.route_editor(
                id,
                EditorEvent::Edit { session, expected_rev, edit: TextEdit::Insert { text, replacement: None } },
                routed,
            )
    }

    pub(super) fn route_text_pointer(&mut self, id: WindowId, intent: TextPointerIntent, routed: &mut VecDeque<Routed>) -> bool {
        let window = self.windows.get_mut(&id).expect("known window");
        let target = match intent {
            TextPointerIntent::Down { target, .. } | TextPointerIntent::Move { target, .. } => target,
            TextPointerIntent::Clear => {
                window.text.drag = None;
                return self.clear_document_selection(id, routed);
            }
        };
        let Some(node) = target.resolve(&window.tree) else { return false };
        match target {
            TextPointerTarget::Editor(_) => {
                self.clear_document_selection(id, routed);
                self.editor_pointer(id, node, intent, routed)
            }
            TextPointerTarget::Region(_) => self.document_pointer(id, node, intent, routed),
        }
    }

    fn editor_pointer(&mut self, id: WindowId, node: NodeId, intent: TextPointerIntent, routed: &mut VecDeque<Routed>) -> bool {
        let window = self.windows.get_mut(&id).expect("known window");
        let Some(session) = EditorSession::active(&window.tree).filter(|session| session.id().node == node) else { return false };
        let editor = session.editor();
        let session = session.id();
        let expected_rev = editor.document_rev();
        let value = editor.value();
        let hit = |pos| if value.is_empty() { Some((0, 0)) } else { window.engine.text_pointer_hit(node, pos) };
        let span = |count, pos| {
            let (caret, character) = hit(pos)?;
            Some(text_span(value, caret, character, count, editor.is_secure(), editor.multiline))
        };
        let selection =
            match intent {
                TextPointerIntent::Down { target, pos, count, extend } => {
                    window.text.drag = None;
                    if editor.composition().is_some() {
                        let edit = EditorEvent::Edit { session, expected_rev, edit: TextEdit::Unmark };
                        if !self.route_editor(id, edit, routed) {
                            return false;
                        }
                        routed.push_front(Routed::Intent(id, InteractionIntent::TextPointer(intent)));
                        return true;
                    }
                    let Some(range) = span(count, pos) else { return false };
                    let endpoints = if extend { (editor.selection().0, range.end) } else { (range.start, range.end) };
                    let (start, end) = if extend { (endpoints.0, endpoints.0) } else { endpoints };
                    window.text.drag =
                        Some(TextDrag { target, span: [(node, start), (node, end)], count, value_rev: Some(editor.value_rev()) });
                    endpoints
                }
                TextPointerIntent::Move { target, pos } => {
                    let Some(drag) = window.text.drag.filter(|drag| {
                        drag.target == target && drag.value_rev == Some(editor.value_rev()) && editor.composition().is_none()
                    }) else {
                        return false;
                    };
                    let Some(range) = span(drag.count, pos) else { return false };
                    let [(_, start), (_, end)] = drag.span;
                    extend_span([start, end], [range.start, range.end], range.start < start)
                }
                TextPointerIntent::Clear => unreachable!(),
            };
        self.route_editor(id, EditorEvent::Select { session, expected_rev, selection }, routed)
    }

    fn document_pointer(&mut self, id: WindowId, region: NodeId, intent: TextPointerIntent, routed: &mut VecDeque<Routed>) -> bool {
        let window = self.windows.get_mut(&id).expect("known window");
        let selection = match intent {
            TextPointerIntent::Down { target, pos, count, extend } => {
                window.text.drag = None;
                let Some(range) = document_span(window, region, pos, count) else {
                    return self.clear_document_selection(id, routed);
                };
                let anchor = extend.then(|| window.tree.doc_selection(region)).flatten().map(|[anchor, _]| anchor);
                window.text.drag = Some(TextDrag { target, span: anchor.map_or(range, |anchor| [anchor; 2]), count, value_rev: None });
                anchor.map_or(range, |anchor| [anchor, range[1]])
            }
            TextPointerIntent::Move { target, pos } => {
                let Some(drag) = window.text.drag.filter(|drag| drag.target == target) else { return false };
                let Some(range) = document_span(window, region, pos, drag.count) else { return false };
                let (first, current) = (drag.span[0], range[0]);
                let backward = if current.0 == first.0 {
                    current.1 < first.1
                } else {
                    window.tree.selectable_texts(region).into_iter().find(|&node| node == first.0 || node == current.0) == Some(current.0)
                };
                let (anchor, focus) = extend_span(drag.span, range, backward);
                [anchor, focus]
            }
            TextPointerIntent::Clear => unreachable!(),
        };
        if !window.tree.valid_doc_selection(region, selection) {
            return false;
        }
        self.set_document_selection(id, region, selection, routed);
        true
    }

    pub(super) fn set_document_selection(&mut self, id: WindowId, region: NodeId, selection: DocSelection, routed: &mut VecDeque<Routed>) {
        let window = self.windows.get_mut(&id).expect("known window");
        let next = window.tree.guard(region);
        if window.text.region != Some(next) {
            self.clear_document_selection(id, routed);
            self.windows.get_mut(&id).expect("known window").text.region = Some(next);
        }
        self.apply_operation(id, RuntimeOp::SetDocSelection { region, selection: Some(selection) }, routed);
    }

    pub(super) fn clear_document_selection(&mut self, id: WindowId, routed: &mut VecDeque<Routed>) -> bool {
        let window = self.windows.get_mut(&id).expect("known window");
        let Some(region) = window.text.region.take().filter(|&guard| window.tree.contains_target(guard)).and_then(TargetGuard::node) else {
            return false;
        };
        self.apply_operation(id, RuntimeOp::SetDocSelection { region, selection: None }, routed);
        true
    }

    pub(super) fn document_key_default(&mut self, id: WindowId, key: KeyEvent, routed: &mut VecDeque<Routed>) -> bool {
        let window = &self.windows[&id];
        if window.engine.platform_text_selection() || !window.arbiter.is_active() {
            return false;
        }
        let Some(command) = command_for_key(key) else { return false };
        if command == EditorCommand::Paste {
            return self.request_paste(id);
        }
        let region = window
            .text
            .region
            .and_then(|guard| TextPointerTarget::Region(guard).resolve(&window.tree))
            .or_else(|| window.tree.focus().and_then(|focus| window.tree.region_of(focus)));
        let Some(region) = region else { return false };
        match command {
            EditorCommand::SelectAll => {
                let texts = window.tree.selectable_texts(region);
                let Some((&first, &last)) = texts.first().zip(texts.last()) else { return false };
                let end = window.tree.text(last).expect("selectable text").text.len();
                self.set_document_selection(id, region, [(first, 0), (last, end)], routed);
                true
            }
            EditorCommand::Copy => {
                let Some([anchor, focus]) = window.tree.doc_selection(region) else { return false };
                if anchor == focus {
                    return false;
                }
                let Some(text) = window.copy_selection(&mut self.world, region) else { return false };
                self.write(window.scope, id)
                    .request_service(ServiceRequest::WriteClipboard { items: vec![ClipboardItem::Text(text)] }, |_, _| {});
                true
            }
            _ => false,
        }
    }

    pub(super) fn editor_key_default(&mut self, id: WindowId, key: KeyEvent, routed: &mut VecDeque<Routed>) -> bool {
        let window = &self.windows[&id];
        if window.engine.platform_text_selection() || !window.arbiter.is_active() {
            return false;
        }
        let Some(session) = EditorSession::active(&window.tree) else { return false };
        let Some(command) = command_for_key(key) else { return false };
        let event = EditorEvent::Command { session: session.id(), expected_rev: session.editor().document_rev(), command };
        self.route_editor(id, event, routed)
    }

    pub(super) fn editor_clipboard(&mut self, id: WindowId, command: EditorCommand) -> bool {
        if command == EditorCommand::Paste {
            return self.request_paste(id);
        }
        let window = &self.windows[&id];
        let Some(session) = EditorSession::active(&window.tree) else { return false };
        let editor = session.editor();
        let Some(text) = editor.clipboard_text().map(str::to_owned) else { return true };
        let event = (command == EditorCommand::Cut).then(|| EditorEvent::Edit {
            session: session.id(),
            expected_rev: editor.document_rev(),
            edit: TextEdit::Insert { text: String::new(), replacement: None },
        });
        self.write(window.scope, id).request_service(
            ServiceRequest::WriteClipboard { items: vec![ClipboardItem::Text(text)] },
            move |cx, outcome| {
                if outcome == ServiceOutcome::ClipboardWritten
                    && let Some(event) = event
                {
                    cx.commands().queued.push_back(AppCommand::EditorEdit { window: id, event });
                }
            },
        );
        true
    }
}

fn extend_span<T: Copy>([start, end]: [T; 2], [first, last]: [T; 2], backward: bool) -> (T, T) {
    if backward { (end, first) } else { (start, last) }
}

fn text_span(value: &str, caret: usize, character: usize, count: u8, secure: bool, multiline: bool) -> Range<usize> {
    match count {
        2 if !secure => word_at(value, character),
        3 if multiline => paragraph_at(value, character),
        2 | 3 => 0..value.len(),
        _ => caret..caret,
    }
}

fn document_span(window: &WindowRuntime, region: NodeId, pos: Point, count: u8) -> Option<DocSelection> {
    let tree = &window.tree;
    let hit = window.engine.hit_test(tree, pos).and_then(|hit| hit.hit.node());
    let direct = std::iter::successors(hit, |&node| tree.physical_parent(node)).find(|&node| tree.selectable_in(region, node));
    let node = direct.or_else(|| {
        tree.selectable_texts(region)
            .into_iter()
            .filter_map(|node| {
                let rect = window.engine.interaction_bounds(node)?;
                let dx = (rect.x - pos.x).max(pos.x - rect.right()).max(0.0);
                let dy = (rect.y - pos.y).max(pos.y - rect.bottom()).max(0.0);
                Some((node, dx.hypot(dy)))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(node, _)| node)
    })?;
    let text = &tree.text(node)?.text;
    let (caret, character) = window.engine.text_pointer_hit(node, pos).or_else(|| text.is_empty().then_some((0, 0)))?;
    let range = text_span(text, caret, character, count, false, false);
    let address = |byte| -> Address { (node, byte) };
    Some([address(range.start), address(range.end)])
}
