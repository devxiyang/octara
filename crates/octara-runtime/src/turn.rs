//! The atomic ingress, react, commit, project and publish turn.

mod text;

pub(crate) use text::{PasteTarget, TextInteraction};

use crate::app::{AppCommand, AppRuntime, HostCommand, Input, PendingAsk, PendingService};
use crate::arena::{NodeKind as ReactiveKind, ScopeId};
use crate::engine::ProjectError;
use crate::graph::FlushTier;
use crate::window::{FeedbackSignal, HandlerEvent, HandlerOutcome, MountCx, WindowRuntime};
use octara_model::ask::{AskId, ServiceId, ServiceRequest};
use octara_model::diagnostics::{Diagnostic, codes};
use octara_model::geometry::{Point, Vec2};
use octara_model::identity::WindowId;
use octara_model::input::{
    BackPhase, ContextMenuEvent, ContextMenuItem, CopyFormat, EditorEvent, HostReply, InteractionEvent, Key, KeyEvent, LinkEvent,
    LinkEventKind, Modifiers, PointerKind,
};
use octara_model::semantics::A11yAction;
use octara_model::style::State;
use octara_tree::arbiter::{
    FocusHandoff, HandlerIntent, InteractionContext, InteractionDecision, InteractionIntent, InteractionPayload, PointerGeometry,
    TouchScrollIntent, TouchScrollStage, focus_leave,
};
use octara_tree::editor::{EditorAction, EditorProposal, EditorSession};
use octara_tree::tables::{HandlerId, HandlerKind};
use octara_tree::transaction::TreeError;
use octara_tree::{NodeId, NodeRef, RuntimeOp, TreeDelta};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

pub const WINDOW_ROUNDS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnError {
    AppFailed,
    UnknownWindow(WindowId),
    Transaction { window: WindowId, reason: TreeError },
    Project { window: WindowId, error: ProjectError },
    LayoutLoop,
}

impl std::fmt::Display for TurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppFailed => f.write_str("application stopped after a failed turn"),
            Self::UnknownWindow(window) => write!(f, "unknown window {window:?}"),
            Self::Transaction { window, reason } => write!(f, "transaction for {window:?} rejected: {reason:?}"),
            Self::Project { window, error } => write!(f, "projection for {window:?} failed: {error}"),
            Self::LayoutLoop => f.write_str("window projection exhausted its turn budget"),
        }
    }
}

impl std::error::Error for TurnError {}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnReceipt {
    pub delivered: bool,
    pub handled: bool,
    pub rounds: usize,
}

enum Routed {
    Input(WindowId, Input),
    Intent(WindowId, InteractionIntent),
    ScrollViewport(WindowId, TouchScrollIntent, Vec2),
    ScrollPost(WindowId, TouchScrollIntent, Vec2),
    FocusEvent(WindowId, NodeId, bool),
    HoverEvent(WindowId, NodeId, bool),
    FocusLeave(WindowId, Option<NodeId>, HandlerIntent),
    Command(WindowId, String),
}

impl AppRuntime {
    pub(crate) fn require_active(&self) -> Result<(), TurnError> {
        if self.failed { Err(TurnError::AppFailed) } else { Ok(()) }
    }

    pub(crate) fn guarded<R>(&mut self, run: impl FnOnce(&mut Self) -> R) -> R {
        match catch_unwind(AssertUnwindSafe(|| run(self))) {
            Ok(value) => value,
            Err(error) => {
                self.failed = true;
                self.rollback();
                self.diagnostics.push(Diagnostic::new(&codes::APP_FAILED, "A panic escaped the application turn"));
                resume_unwind(error);
            }
        }
    }

    pub(crate) fn run_turn(&mut self, incoming: Option<(WindowId, Input)>) -> Result<TurnReceipt, TurnError> {
        self.require_active()?;
        if let Some((window, _)) = incoming.as_ref()
            && !self.windows.contains_key(window)
        {
            return Err(TurnError::UnknownWindow(*window));
        }
        self.guarded(|app| {
            let result = app.drive_turn(incoming);
            if result.is_err() {
                app.rollback();
            }
            result
        })
    }

    fn drive_turn(&mut self, incoming: Option<(WindowId, Input)>) -> Result<TurnReceipt, TurnError> {
        self.tasks.begin_turn();
        let now = self.now();
        self.coalesce_text_streams(now);
        for window in self.windows.values_mut() {
            if window.next_deadline.is_some_and(|deadline| deadline <= now) {
                window.force_project = true;
            }
        }

        let mut receipt = TurnReceipt::default();
        let mut routed = VecDeque::new();
        if let Some((window, input)) = incoming {
            receipt.delivered = self.ingress(window, input, &mut routed);
        }

        for round in 1..=WINDOW_ROUNDS {
            receipt.rounds = round;
            if let Some(job) = routed.pop_front() {
                receipt.handled |= self.route(job, &mut routed);
            }
            self.tasks.poll_turn_ready(&mut self.world, &mut self.commands, |id| self.windows.get(&id).map(|window| &window.tree));
            self.drain_commands(&mut routed);
            self.react()?;
            self.prepare_presentation(&mut routed);
            self.commit(&mut routed)?;
            self.project(&mut routed)?;
            self.sync_media();
            if routed.is_empty()
                && self.commands.queued.is_empty()
                && !self.world.has_pending_work()
                && !self.tasks.has_turn_ready()
                && self.windows.values().all(|window| window.pending.is_empty() && !window.force_project)
            {
                self.update_tooltips(now);
                if self.world.has_pending_work() {
                    continue;
                }
                self.publish();
                return Ok(receipt);
            }
        }
        self.diagnostics.push(Diagnostic::new(&codes::LAYOUT_LOOP, format!("Turn exhausted {WINDOW_ROUNDS} causal rounds")));
        Err(TurnError::LayoutLoop)
    }

    fn ingress(&mut self, id: WindowId, input: Input, routed: &mut VecDeque<Routed>) -> bool {
        let window = &self.windows[&id];
        let valid = match &input {
            Input::Interaction(InteractionEvent::PointerDown { target, .. }) => {
                window.settled.is_some()
                    && window.tree.contains_target(target.hit)
                    && target.action.is_none_or(|target| window.tree.contains_target(target))
            }
            Input::Interaction(_) => window.settled.is_some(),
            Input::Editor(event) => {
                window.settled.is_some()
                    && EditorSession::active(&window.tree)
                        .is_some_and(|session| session.id() == event.session() && session.editor().document_rev() == event.expected_rev())
            }
            Input::Host(HostReply::FilesDropped { target, files }) => !files.is_empty() && window.file_drop_handler(*target).is_some(),
            Input::Host(HostReply::Paste(event)) => window.settled.is_some() && !event.items.is_empty(),
            Input::Host(
                HostReply::NativeEvent { target, .. }
                | HostReply::NativeLive { target, .. }
                | HostReply::ScrollChanged { target, .. }
                | HostReply::AccessibilityAction { target, .. },
            ) => window.settled.is_some() && window.tree.contains_target(*target),
            Input::Host(HostReply::FocusChanged { target, .. }) => {
                window.settled.is_some() && target.is_none_or(|target| target.node().is_some() && window.tree.contains_target(target))
            }
            Input::Host(HostReply::ExitFinished(ticket)) => window.tree.accepts_exit(*ticket),
            Input::Host(_) => true,
        };
        if valid {
            routed.push_back(Routed::Input(id, input));
        }
        valid
    }

    fn route(&mut self, job: Routed, routed: &mut VecDeque<Routed>) -> bool {
        match job {
            Routed::Input(id, input) => self.route_input(id, input, routed),
            Routed::Intent(id, intent) => self.route_intent(id, intent, routed),
            Routed::ScrollViewport(id, scroll, available) => {
                let used = self
                    .windows
                    .get(&id)
                    .and_then(|window| scroll.viewport_delta(&window.tree, available))
                    .and_then(|delta| scroll.target.node().map(|node| self.scroll_by(id, node, delta)))
                    .unwrap_or_default();
                routed.push_front(Routed::ScrollPost(id, scroll, TouchScrollIntent::remaining(available, used)));
                used != Vec2::default()
            }
            Routed::ScrollPost(id, scroll, available) => {
                let handler = self.windows.get(&id).and_then(|window| scroll.handler(&window.tree, TouchScrollStage::Post, available));
                handler.is_some_and(|handler| self.invoke_intent(id, handler).consumed)
            }
            Routed::FocusEvent(id, node, focused) => {
                let kind = if focused { HandlerKind::Focus } else { HandlerKind::Blur };
                self.invoke_node(id, node, kind, HandlerEvent::Focus { node, focused }).consumed
            }
            Routed::HoverEvent(id, node, hovered) => {
                let current = self.windows.get(&id).is_some_and(|window| {
                    window.tree.is_live(node) && window.tree.node(node).is_some_and(|core| core.state.contains(State::HOVER) == hovered)
                });
                current
                    && (self.deliver_link(id, node, LinkEventKind::Hover(hovered), Modifiers::empty())
                        | self.invoke_node(id, node, HandlerKind::Hover, HandlerEvent::Hover(hovered)).consumed)
            }
            Routed::FocusLeave(id, next, intent) => {
                self.windows.get(&id).is_some_and(|window| window.tree.focus() == next) && self.invoke_intent(id, intent).consumed
            }
            Routed::Command(id, name) => self.invoke_command(id, &name),
        }
    }

    fn route_input(&mut self, id: WindowId, input: Input, routed: &mut VecDeque<Routed>) -> bool {
        let Some(window) = self.windows.get_mut(&id) else { return false };
        match &input {
            Input::Interaction(
                InteractionEvent::PointerDown { .. }
                | InteractionEvent::PointerCancel { .. }
                | InteractionEvent::PointerLeave
                | InteractionEvent::Wheel { .. }
                | InteractionEvent::Key(_)
                | InteractionEvent::BackGesture { .. }
                | InteractionEvent::WindowActive(false),
            )
            | Input::Editor(_)
            | Input::Host(HostReply::ScrollChanged { .. } | HostReply::Paste(_)) => window.tooltip.dismiss(),
            _ => {}
        }
        match input {
            Input::Interaction(InteractionEvent::Wheel { pos, delta, .. }) => {
                let window = &self.windows[&id];
                let Some(hit) = window.hit_test(pos).and_then(|target| target.hit.node()) else { return false };
                let scrolls: Vec<_> = std::iter::successors(Some(hit), |&node| window.tree.physical_parent(node))
                    .filter(|&node| window.engine.scroll_range(node).is_some() && window.tree.is_interactive(node))
                    .collect();
                let mut remaining = delta;
                let mut handled = false;
                for node in scrolls {
                    let used = self.scroll_by(id, node, remaining);
                    handled |= used != Vec2::default();
                    remaining = TouchScrollIntent::remaining(remaining, used);
                    if remaining == Vec2::default() {
                        break;
                    }
                }
                handled
            }
            Input::Interaction(InteractionEvent::BackGesture { id: token, phase }) => self.back_gesture(id, token, phase),
            Input::Interaction(event) => {
                if let InteractionEvent::WindowActive(active) = event {
                    let window = self.windows.get_mut(&id).expect("known window");
                    if window.arbiter.is_active() != active {
                        window.engine.set_window_active(active);
                        window.force_project = true;
                    }
                    if active {
                        self.menu_window = Some(id);
                    } else {
                        window.back = None;
                    }
                }
                if let InteractionEvent::Key(key) = event
                    && key.requests_context_menu()
                    && let Some(node) = self.windows[&id].tree.focus()
                    && self.windows[&id].tree.editor(node).is_none()
                    && self.context_menu(id, node, None)
                {
                    return true;
                }
                if let InteractionEvent::Key(key) = event
                    && self.route_transfer_key(id, key, routed)
                {
                    return true;
                }
                if let InteractionEvent::Key(key) = event
                    && let Some(session) = EditorSession::active(&self.windows[&id].tree)
                {
                    let event = EditorEvent::KeyIntercept { session: session.id(), expected_rev: session.editor().document_rev(), key };
                    return self.route_editor(id, event, routed) || self.editor_key_default(id, key, false, routed);
                }
                if let InteractionEvent::Key(key) = event
                    && (self.windows[&id]
                        .tree
                        .focus()
                        .is_some_and(|focus| self.invoke_node(id, focus, HandlerKind::Key, HandlerEvent::Key(key)).consumed)
                        || self.invoke_shortcut(id, key))
                {
                    return true;
                }
                if let InteractionEvent::Key(key) = event
                    && self.document_key_default(id, key, routed)
                {
                    return true;
                }
                let now = self.now();
                let window = self.windows.get_mut(&id).expect("known window");
                let hit = match event {
                    InteractionEvent::PointerMove { pos, .. } => window.engine.hit_test(&window.tree, pos).map(|target| target.hit),
                    InteractionEvent::PointerUp { target, .. } => target.map(|target| target.hit),
                    _ => None,
                };
                let scrollbar = match event {
                    InteractionEvent::PointerDown { kind, pos, .. } if kind != PointerKind::Touch => {
                        window.engine.scrollbar_hit(&window.tree, pos)
                    }
                    InteractionEvent::PointerMove { pos, .. } | InteractionEvent::PointerUp { pos, .. } => {
                        window.engine.scrollbar_hit(&window.tree, pos)
                    }
                    _ => None,
                };
                match event {
                    InteractionEvent::PointerDown { .. } | InteractionEvent::PointerMove { .. } | InteractionEvent::PointerUp { .. } => {
                        window.scrollbar_hovered = scrollbar.map(|hit| hit.key);
                    }
                    InteractionEvent::PointerLeave | InteractionEvent::PointerCancel { .. } | InteractionEvent::WindowActive(false) => {
                        window.scrollbar_hovered = None;
                    }
                    _ => {}
                }
                let bounds = |node| window.engine.interaction_bounds(node);
                let context = InteractionContext {
                    now,
                    keyboard_open: window.keyboard_open,
                    pointer: Some(PointerGeometry { hit, bounds: &bounds }),
                    text_selection: !window.engine.platform_text_selection(),
                    scrollbar,
                };
                let mut decision = window.arbiter.handle(&window.tree, event, context);
                if let InteractionEvent::PointerMove { pos, .. } = event {
                    decision.operations.extend(window.arbiter.update_hover(&window.tree, hit).operations);
                    if scrollbar.is_none() && window.arbiter.pointer_hover_active() {
                        window.tooltip.pointer_moved(pos, window.tree.front_modal());
                    } else {
                        window.tooltip.dismiss();
                    }
                } else if matches!(event, InteractionEvent::PointerLeave) {
                    decision.operations.extend(window.arbiter.update_hover(&window.tree, None).operations);
                }
                let dragging = window.arbiter.dragged_scrollbar().is_some();
                self.offer_decision(id, decision, routed) || dragging
            }
            Input::Editor(event) => self.route_editor(id, event, routed),
            Input::Host(reply) => self.route_host(id, reply, routed),
        }
    }

    fn route_transfer_key(&mut self, id: WindowId, key: KeyEvent, routed: &mut VecDeque<Routed>) -> bool {
        let window = self.windows.get_mut(&id).expect("known window");
        let Some(decision) = window.arbiter.transfer_key(&window.tree, key) else { return false };
        self.offer_decision(id, decision, routed);
        true
    }

    fn offer_decision(&mut self, id: WindowId, decision: InteractionDecision, routed: &mut VecDeque<Routed>) -> bool {
        let pending = !decision.operations.is_empty() || !decision.intents.is_empty();
        for op in decision.operations {
            self.apply_operation(id, op, routed);
        }
        routed.extend(decision.intents.into_iter().map(|intent| Routed::Intent(id, intent)));
        pending
    }

    fn route_intent(&mut self, id: WindowId, intent: InteractionIntent, routed: &mut VecDeque<Routed>) -> bool {
        let Some(window) = self.windows.get_mut(&id) else { return false };
        match intent {
            InteractionIntent::Invoke(intent) => self.invoke_intent(id, intent).consumed,
            InteractionIntent::Focus(intent) => {
                if let Some(op) = intent.resolve(&window.tree) {
                    self.apply_operation_with_pointer(id, op, Some(intent), routed);
                    true
                } else {
                    false
                }
            }
            InteractionIntent::Activate(intent) => {
                let handlers = window.arbiter.resolve_activation(&window.tree, intent);
                let delivered = !handlers.is_empty();
                for handler in handlers {
                    self.invoke_intent(id, handler);
                }
                delivered
            }
            InteractionIntent::TextPointer(intent) => self.route_text_pointer(id, intent, routed),
            InteractionIntent::Scrollbar(scroll) => {
                let Some(delta) = scroll.delta(&window.tree) else { return false };
                let Some(node) = scroll.target.node() else { return false };
                self.scroll_by(id, node, delta) != Vec2::default()
            }
            InteractionIntent::TouchScroll(scroll) => {
                let handler = scroll.handler(&window.tree, TouchScrollStage::Pre, scroll.delta);
                let used = handler.map_or_default(|handler| self.invoke_intent(id, handler).scroll);
                let available = TouchScrollIntent::remaining(scroll.delta, used);
                if available != Vec2::default() {
                    routed.push_front(Routed::ScrollViewport(id, scroll, available));
                }
                used != Vec2::default()
            }
            InteractionIntent::WindowDrag { target, maximize } => {
                if !window.tree.contains_target(target) || !target.node().is_some_and(|node| window.tree.is_interactive(node)) {
                    return false;
                }
                self.staged_host.push(if maximize {
                    HostCommand::ToggleMaximizeWindow { window: id }
                } else {
                    HostCommand::StartWindowDrag { window: id }
                });
                true
            }
        }
    }

    fn route_editor(&mut self, id: WindowId, event: EditorEvent, routed: &mut VecDeque<Routed>) -> bool {
        let node = event.session().node;
        let Some(action) = EditorSession::active(&self.windows[&id].tree).and_then(|session| session.route(event).ok()) else {
            return false;
        };
        match action {
            EditorAction::Apply(update) => {
                let window = self.windows.get_mut(&id).expect("known window");
                window.text.reset_column();
                window.apply(RuntimeOp::EditorApply { node, update });
                true
            }
            EditorAction::Propose(proposal) => {
                self.invoke_editor_input(id, node, &proposal);
                if let Some(window) = self.windows.get_mut(&id) {
                    window.text.reset_column();
                    window.propose_edit(node, proposal);
                }
                true
            }
            EditorAction::KeyIntercept(key) => {
                if self.route_transfer_key(id, key, routed) {
                    return true;
                }
                if self.windows[&id].tree.editor(node).is_some_and(|editor| editor.composition().is_some()) {
                    return false;
                }
                if key.requests_context_menu() && self.context_menu(id, node, None) {
                    return true;
                }
                if self.invoke_node(id, node, HandlerKind::Key, HandlerEvent::Key(key)).consumed || self.invoke_shortcut(id, key) {
                    return true;
                }
                let tree = &self.windows[&id].tree;
                if matches!(key.key, Key::Char('z' | 'Z'))
                    && key.modifiers.intersects(Modifiers::META | Modifiers::CTRL)
                    && !key.modifiers.contains(Modifiers::ALT)
                    && let Some(session) = EditorSession::active(tree)
                {
                    let proposal = if key.modifiers.contains(Modifiers::SHIFT) { session.redo() } else { session.undo() };
                    if let Some(proposal) = proposal {
                        self.invoke_editor_input(id, node, &proposal);
                        self.windows.get_mut(&id).expect("known window").propose_edit(node, proposal);
                    }
                    return true;
                }
                if key.key == Key::Enter
                    && key.modifiers.is_empty()
                    && tree.editor(node).is_some_and(|editor| editor.submit_on_enter && editor.composition().is_none())
                    && tree.handler(node, HandlerKind::Submit).is_some()
                {
                    self.invoke_node(id, node, HandlerKind::Submit, HandlerEvent::Key(key));
                    return true;
                }
                if matches!(key.key, Key::Tab | Key::Escape) {
                    let now = self.now();
                    let window = self.windows.get_mut(&id).expect("known window");
                    let context = InteractionContext { now, keyboard_open: window.keyboard_open, ..InteractionContext::default() };
                    let decision = window.arbiter.handle(&window.tree, InteractionEvent::Key(key), context);
                    return self.offer_decision(id, decision, routed);
                }
                false
            }
            EditorAction::KeyFallback(key) => self.editor_key_default(id, key, true, routed),
        }
    }

    fn route_host(&mut self, id: WindowId, reply: HostReply, routed: &mut VecDeque<Routed>) -> bool {
        match reply {
            HostReply::ViewportChanged(viewport) => {
                let window = self.windows.get_mut(&id).expect("known window");
                if window.options.viewport != viewport {
                    window.options.viewport = viewport;
                    window.viewport_rev.0 = window.viewport_rev.0.checked_add(1).expect("viewport revision exhausted");
                    window.force_project = true;
                    let breakpoint = window.tree.theme().breakpoint_of(viewport.size.width);
                    let mut write = self.world.write(window.scope, Some(id));
                    write.set_if_changed(window.viewport_source, viewport);
                    write.set_if_changed(window.breakpoint_source, breakpoint);
                    let touch = window.options.touch;
                    self.apply_operation(id, RuntimeOp::SetViewportContext { breakpoint, width: viewport.size.width, touch }, routed);
                }
                true
            }
            HostReply::ChromeChanged(chrome) => {
                let window = &self.windows[&id];
                self.world.write(window.scope, Some(id)).set_if_changed(window.chrome_source, chrome);
                true
            }
            HostReply::GeometryChanged(geometry) => {
                let window = self.windows.get_mut(&id).expect("known window");
                if window.geometry.position != geometry.position {
                    window.arbiter.window_moved();
                }
                window.geometry = geometry;
                self.world.write(window.scope, Some(id)).set_if_changed(window.geometry_source, geometry);
                true
            }
            HostReply::SystemPreferencesChanged(preferences) => {
                self.update_system_preferences(preferences);
                true
            }
            HostReply::CloseRequested => {
                let callback = self.windows[&id].authoring.close_request(&self.world);
                if callback.is_none_or(|(scope, callback)| callback(&mut self.write(scope, id))) {
                    self.commands.queued.push_back(AppCommand::Close(id));
                }
                true
            }
            HostReply::FocusChanged { target, visible } => {
                let node = target.and_then(|target| target.node());
                self.apply_operation(id, RuntimeOp::SetFocus { node, visible }, routed);
                if let Some(window) = self.windows.get_mut(&id)
                    && window.tree.focus() != node
                {
                    window.force_project = true;
                }
                true
            }
            HostReply::ScrollChanged { target, offset, at_end } => {
                let window = self.windows.get_mut(&id).expect("known window");
                let Some(node) = target.node() else { return false };
                let Some(slot) = window.tree.scroll(node).filter(|slot| slot.requested.is_none() && slot.offset != offset) else {
                    return false;
                };
                if !offset.x.is_finite() || !offset.y.is_finite() {
                    return false;
                }
                if offset.y < slot.offset.y && !at_end {
                    window.apply(RuntimeOp::ReleaseFollowTail { node });
                } else if offset.y > slot.offset.y && at_end {
                    window.apply(RuntimeOp::ResumeFollowTail { node });
                }
                window.apply(RuntimeOp::SetScroll { node, offset, clamped: false });
                true
            }
            HostReply::FilesDropped { target, files } => {
                let Some(handler) = self.windows[&id].file_drop_handler(target) else { return false };
                if files.is_empty() {
                    return false;
                }
                self.invoke(id, handler, HandlerEvent::FileDrop(&files));
                true
            }
            HostReply::Paste(event) => self.deliver_paste(id, None, event, routed),
            HostReply::MenuCommand { name, .. } => self.invoke_command(id, &name),
            HostReply::AskAnswer { id: ask, answer } => {
                let Some(pending) = self.asks.get(&ask.0) else { return false };
                if id != pending.window || !self.world.is_scope_alive(pending.scope) || !answer.fits(&pending.ask) {
                    return false;
                }
                let pending = self.asks.remove(&ask.0).expect("validated ask");
                (pending.done)(&mut self.write(pending.scope, id), answer);
                true
            }
            HostReply::ServiceReply { id: service, outcome } => {
                let Some(pending) = self.services.get(&service.0) else { return false };
                if id != pending.window || !self.world.is_scope_alive(pending.scope) {
                    return false;
                }
                let pending = self.services.remove(&service.0).expect("validated service");
                (pending.done)(&mut self.write(pending.scope, id), outcome);
                true
            }
            HostReply::ExitFinished(ticket) => {
                if !self.windows[&id].tree.accepts_exit(ticket) {
                    return false;
                }
                self.apply_operation(id, RuntimeOp::FinishExit(ticket), routed);
                true
            }
            HostReply::NativeLive { target, live } => {
                let Some(node) = target.node().filter(|_| self.windows[&id].tree.contains_target(target)) else { return false };
                self.apply_operation(id, RuntimeOp::SetNativeLive { node, live }, routed);
                true
            }
            HostReply::NativeEvent { target, event } => target.node().is_some_and(|node| {
                self.windows[&id].tree.contains_target(target)
                    && self.invoke_node(id, node, HandlerKind::Native, HandlerEvent::Native(&event)).consumed
            }),
            HostReply::DocumentSelection { selection } => {
                let Some((target, [anchor, focus])) = selection else { return self.clear_document_selection(id, routed) };
                let tree = &self.windows[&id].tree;
                let Some(region) = target.node().filter(|_| tree.contains_target(target)) else { return false };
                let selection = [(anchor.node, anchor.offset), (focus.node, focus.offset)];
                if selection[0] == selection[1] || !tree.valid_doc_selection(region, selection) {
                    return false;
                }
                if tree.doc_selection(region) != Some(selection) {
                    self.set_document_selection(id, region, selection, routed);
                }
                true
            }
            HostReply::AccessibilityAction { target, action } => {
                let window = &self.windows[&id];
                let Some(node) = target.node().filter(|&node| window.tree.contains_target(target) && window.tree.is_interactive(node))
                else {
                    return false;
                };
                match action {
                    A11yAction::Focus => {
                        self.apply_operation(id, RuntimeOp::SetFocus { node: Some(node), visible: true }, routed);
                        true
                    }
                    A11yAction::Press => {
                        if window.tree.link(node).is_some() {
                            return self.deliver_link(id, node, LinkEventKind::Press, Modifiers::empty());
                        }
                        let handler =
                            [HandlerKind::Press, HandlerKind::Toggle].into_iter().find_map(|kind| window.tree.handler(node, kind));
                        let Some(handler) = handler else { return false };
                        let scope = window.handlers.get(&handler).map(|entry| entry.scope);
                        if scope.is_none_or(|scope| !self.world.is_scope_alive(scope)) {
                            return false;
                        }
                        self.invoke(id, handler, HandlerEvent::Accessibility(&action));
                        true
                    }
                    A11yAction::ShowContextMenu if self.context_menu(id, node, None) => true,
                    A11yAction::SetTextSelection { anchor, focus } => {
                        if self.invoke_node(id, node, HandlerKind::Accessibility, HandlerEvent::Accessibility(&action)).consumed {
                            return true;
                        }
                        let tree = &self.windows[&id].tree;
                        if let Some(session) = EditorSession::active(tree)
                            && [node, anchor.node, focus.node].into_iter().all(|target| target == session.id().node)
                        {
                            let event = EditorEvent::ProposeSelection {
                                session: session.id(),
                                expected_rev: session.editor().document_rev(),
                                selection: (anchor.offset, focus.offset),
                            };
                            return self.route_editor(id, event, routed);
                        }
                        let selection = [(anchor.node, anchor.offset), (focus.node, focus.offset)];
                        if !tree.valid_doc_selection(node, selection) {
                            return false;
                        }
                        self.set_document_selection(id, node, selection, routed);
                        true
                    }
                    A11yAction::CopyTextSelection => {
                        let window = &self.windows[&id];
                        let text = match window.tree.editor(node) {
                            Some(editor) => editor.clipboard_text().map(str::to_owned),
                            None => window.copy_selection(&mut self.world, node),
                        };
                        let Some(text) = text else { return false };
                        let scope = window.scope;
                        self.write(scope, id).request_service(ServiceRequest::WriteClipboardText { text }, |_, _| {});
                        true
                    }
                    _ => self.invoke_node(id, node, HandlerKind::Accessibility, HandlerEvent::Accessibility(&action)).consumed,
                }
            }
            reply @ (HostReply::NativeMeasurement { .. } | HostReply::NativeSnapshot { .. }) => {
                let window = self.windows.get_mut(&id).expect("known window");
                window.engine.native_reply(&reply);
                window.force_project = true;
                true
            }
        }
    }

    fn scroll_by(&mut self, id: WindowId, node: NodeId, delta: Vec2) -> Vec2 {
        let Some(window) = self.windows.get_mut(&id) else { return Vec2::default() };
        let (Some(slot), Some(range)) = (window.tree.scroll(node), window.engine.scroll_range(node)) else {
            return Vec2::default();
        };
        let previous = slot.offset;
        let offset = range.clamp(Vec2::new(previous.x + delta.x, previous.y + delta.y));
        if offset.y < previous.y {
            window.apply(RuntimeOp::ReleaseFollowTail { node });
        } else if offset.y > previous.y && offset.y >= range.max.y - 0.5 {
            window.apply(RuntimeOp::ResumeFollowTail { node });
        }
        window.apply(RuntimeOp::SetScroll { node, offset, clamped: false });
        Vec2::new(offset.x - previous.x, offset.y - previous.y)
    }

    fn invoke_intent(&mut self, id: WindowId, intent: HandlerIntent) -> HandlerOutcome {
        let Some(window) = self.windows.get(&id) else { return HandlerOutcome::default() };
        let position = match &intent.payload {
            InteractionPayload::Press { pos, .. } => Some(*pos),
            InteractionPayload::Drag(drag) => Some(drag.pos),
            _ => None,
        };
        if let Some(link) = intent.resolve_link(&window.tree) {
            let kind = if intent.kind == HandlerKind::SecondaryPress { LinkEventKind::SecondaryPress } else { LinkEventKind::Press };
            let modifiers = match &intent.payload {
                InteractionPayload::Press { modifiers, .. } => *modifiers,
                _ => Modifiers::empty(),
            };
            if self.deliver_link(id, link, kind, modifiers) || kind == LinkEventKind::Press {
                return HandlerOutcome { consumed: true, ..Default::default() };
            }
        }
        let window = &self.windows[&id];
        if let Some(handler) = intent.resolve(&window.tree) {
            let local_pos = position.and_then(|pos| {
                let bounds = window.engine.interaction_bounds(intent.target.node()?)?;
                Some(Point::new(pos.x - bounds.x, pos.y - bounds.y))
            });
            return self.invoke(
                id,
                handler,
                HandlerEvent::Interaction { target: intent.target, kind: intent.kind, payload: intent.payload, local_pos },
            );
        }
        let mut consumed = false;
        if intent.kind == HandlerKind::SecondaryPress
            && window.tree.contains_target(intent.target)
            && let Some(node) = intent.target.node().filter(|&node| window.tree.is_interactive(node))
        {
            consumed = self.context_menu(id, node, position);
            if !consumed {
                let window = &self.windows[&id];
                let parent = window.tree.logical_parent(node);
                let receiver = std::iter::successors(parent, |&node| window.tree.logical_parent(node))
                    .find(|&node| window.tree.handler(node, HandlerKind::SecondaryPress).is_some() && window.tree.is_interactive(node));
                if let Some(receiver) = receiver {
                    let target = window.tree.guard(receiver);
                    let local_pos = position.and_then(|pos| {
                        let bounds = window.engine.interaction_bounds(receiver)?;
                        Some(Point::new(pos.x - bounds.x, pos.y - bounds.y))
                    });
                    return self.invoke_node(
                        id,
                        receiver,
                        HandlerKind::SecondaryPress,
                        HandlerEvent::Interaction { target, kind: HandlerKind::SecondaryPress, payload: intent.payload, local_pos },
                    );
                }
            }
        }
        HandlerOutcome { consumed, ..Default::default() }
    }

    fn deliver_link(&mut self, id: WindowId, node: NodeId, kind: LinkEventKind, modifiers: Modifiers) -> bool {
        let window = &self.windows[&id];
        let tree = &window.tree;
        let Some(link) = tree.link(node) else { return false };
        let handlers = std::iter::successors(Some(node), |&node| tree.logical_parent(node))
            .filter_map(|node| tree.handler(node, HandlerKind::Link))
            .collect::<smallvec::SmallVec<[HandlerId; 2]>>();
        let navigation = (kind == LinkEventKind::Press).then(|| link.navigation_href().map(str::to_owned)).flatten();
        let handled = if handlers.is_empty() {
            false
        } else {
            let text = tree
                .physical_parent(node)
                .and_then(|parent| tree.text(parent))
                .and_then(|text| text.text.get(link.range.clone()))
                .unwrap_or_default();
            let event = LinkEvent {
                href: link.href.clone(),
                text: text.into(),
                kind,
                modifiers,
                rect: window.engine.interaction_bounds(node).unwrap_or_default(),
            };
            handlers.into_iter().any(|handler| self.invoke(id, handler, HandlerEvent::Link(&event)).consumed)
        };
        if !handled && let Some(url) = navigation {
            self.staged_host.push(HostCommand::OpenUrl { url });
        }
        handled || kind == LinkEventKind::Press
    }

    fn context_menu(&mut self, id: WindowId, target: NodeId, position: Option<Point>) -> bool {
        let window = &self.windows[&id];
        let tree = &window.tree;
        let Some(handler) = std::iter::successors(Some(target), |&node| tree.logical_parent(node))
            .filter(|&node| tree.is_interactive(node))
            .find_map(|node| tree.handler(node, HandlerKind::ContextMenu))
        else {
            return false;
        };
        let mut items = Vec::new();
        let region = tree.region_of(target);
        if let Some(node) = region {
            for format in window.copy_formats(&self.world, node) {
                if let Some(text) = window.copy_selection_as(&mut self.world, node, Some(&format)).filter(|text| !text.is_empty()) {
                    items.push(ContextMenuItem::Copy { format, text });
                }
            }
        }
        if let Some(text) = tree.editor(target).and_then(|editor| editor.clipboard_text()) {
            items.push(ContextMenuItem::Copy { format: CopyFormat::Plain, text: text.to_owned() });
        }
        if let Some(link) = tree.link(target) {
            items.push(ContextMenuItem::CopyLink(link.href.clone()));
            if let Some(href) = link.navigation_href() {
                items.push(ContextMenuItem::OpenLink(href.into()));
            }
        }
        let rect = window
            .engine
            .selection_rect(tree, region.unwrap_or(target))
            .or_else(|| window.engine.interaction_bounds(target))
            .unwrap_or_default();
        let event = ContextMenuEvent { target, position, rect, items };
        self.invoke(id, handler, HandlerEvent::ContextMenu(&event)).consumed
    }

    fn invoke_editor_input(&mut self, id: WindowId, node: NodeId, proposal: &EditorProposal) {
        let handler = self.windows.get(&id).and_then(|window| window.tree.handler(node, HandlerKind::Input));
        if let Some(handler) = handler {
            self.invoke(id, handler, HandlerEvent::EditorInput { node, value: proposal.value() });
        }
    }

    fn invoke_node(&mut self, id: WindowId, node: NodeId, kind: HandlerKind, event: HandlerEvent<'_>) -> HandlerOutcome {
        let handler = self.windows.get(&id).and_then(|window| window.tree.handler(node, kind));
        handler.map_or_default(|handler| self.invoke(id, handler, event))
    }

    fn invoke(&mut self, id: WindowId, handler: HandlerId, event: HandlerEvent<'_>) -> HandlerOutcome {
        let Some(window) = self.windows.get(&id) else { return HandlerOutcome::default() };
        let Some(callback) = window.handlers.get(&handler) else {
            return HandlerOutcome::default();
        };
        let scope = callback.scope;
        if !self.world.is_scope_alive(scope) {
            return HandlerOutcome::default();
        }
        (callback.call)(&mut window.write(&mut self.world, scope, self.tasks.spawner(), &mut self.commands), &event)
    }

    fn write(&mut self, scope: ScopeId, window: WindowId) -> crate::WriteCx<'_> {
        self.windows[&window].write(&mut self.world, scope, self.tasks.spawner(), &mut self.commands)
    }

    fn invoke_shortcut(&mut self, id: WindowId, key: KeyEvent) -> bool {
        let window = &self.windows[&id];
        let Some((scope, callback)) = window.authoring.shortcut(&self.world, key, window.shortcut_modifier) else {
            return false;
        };
        callback(&mut window.write(&mut self.world, scope, self.tasks.spawner(), &mut self.commands));
        true
    }

    fn invoke_command(&mut self, id: WindowId, name: &str) -> bool {
        let handler = self.windows.get(&id).and_then(|window| window.named_command(name));
        handler.is_some_and(|(_, handler)| self.invoke(id, handler, HandlerEvent::Command(name)).consumed)
    }

    fn back_gesture(&mut self, id: WindowId, token: u64, phase: BackPhase) -> bool {
        let state = self.windows.get_mut(&id).expect("known window");
        let candidate = state.named_command("navigation.back").map(|(_, handler)| (token, handler));
        match phase {
            BackPhase::Started => {
                state.back = candidate;
                candidate.is_some()
            }
            BackPhase::Progress(progress) => progress.is_finite() && candidate.is_some() && state.back == candidate,
            BackPhase::Cancelled => {
                if state.back.is_some_and(|(id, _)| id == token) {
                    state.back = None;
                    true
                } else {
                    false
                }
            }
            BackPhase::Completed => {
                if state.back.is_none_or(|(id, _)| id != token) {
                    return false;
                }
                let started = state.back.take();
                candidate
                    .filter(|candidate| started == Some(*candidate))
                    .is_some_and(|(_, handler)| self.invoke(id, handler, HandlerEvent::Command("navigation.back")).consumed)
            }
        }
    }

    fn drain_commands(&mut self, routed: &mut VecDeque<Routed>) {
        let queued = std::mem::take(&mut self.commands.queued);
        for command in queued {
            match command {
                AppCommand::Open { id, options, root } => self.insert_window(id, *options, root),
                AppCommand::Close(id) => self.remove_window(id),
                AppCommand::WindowControl { window, command } => {
                    if self.windows.contains_key(&window) {
                        self.staged_host.push(command);
                    }
                }
                AppCommand::WindowPresentation { window, update } => {
                    if let Some(window) = self.windows.get_mut(&window) {
                        update.apply(window.pending_presentation.get_or_insert_with(|| window.options.presentation.clone()));
                    }
                }
                AppCommand::SetAppBadge(badge) => self.app_badge = badge,
                AppCommand::RequestUserAttention { window, attention } => {
                    self.staged_host.push(HostCommand::RequestUserAttention { window, attention });
                }
                AppCommand::Focus { window, test_id } => {
                    let target = if let Some(test_id) = test_id {
                        let Some(state) = self.windows.get(&window) else { continue };
                        let [node] = state.tree.named_nodes(&test_id) else { continue };
                        Some(*node)
                    } else {
                        None
                    };
                    self.apply_operation(window, RuntimeOp::SetFocus { node: target, visible: true }, routed);
                }
                AppCommand::Command { window, name, after_commit } => {
                    if after_commit {
                        routed.push_back(Routed::Command(window, name));
                    } else {
                        self.invoke_command(window, &name);
                    }
                }
                AppCommand::Scroll { window, node, offset } => {
                    self.apply_operation(window, RuntimeOp::RequestScroll { node, target: offset }, routed);
                }
                AppCommand::ScrollEnd { window, container } => {
                    let Some(state) = self.windows.get(&window) else { continue };
                    let [node] = state.tree.named_nodes(&container) else { continue };
                    let Some(scroll) = state.tree.scroll(*node) else { continue };
                    let node = *node;
                    let target = Vec2::new(scroll.offset.x, f32::MAX);
                    self.apply_operation(window, RuntimeOp::ResumeFollowTail { node }, routed);
                    self.apply_operation(window, RuntimeOp::RequestScroll { node, target }, routed);
                }
                AppCommand::Reveal { window, container, key } => {
                    let Some(state) = self.windows.get(&window) else { continue };
                    let [container] = state.tree.named_nodes(&container) else { continue };
                    let callback = state.reveal.iter().find(|entry| {
                        matches!(
                            entry.node, NodeRef::Existing { id, .. } if id == *container
                        ) && self.world.is_scope_alive(entry.scope)
                    });
                    if let Some(callback) = callback {
                        let mut write = state.write(&mut self.world, callback.scope, self.tasks.spawner(), &mut self.commands);
                        (callback.call)(&mut write, &key);
                    }
                }
                AppCommand::Ask { window, scope, ask, done } => {
                    if self.windows.contains_key(&window) && self.world.is_scope_alive(scope) {
                        let id = self.next_request();
                        self.asks.insert(id, PendingAsk { window, scope, ask: ask.clone(), done });
                        self.staged_host.push(HostCommand::Ask { window, id: AskId(id), ask });
                    }
                }
                AppCommand::Service { window, scope, request, done } => {
                    if self.windows.contains_key(&window) && self.world.is_scope_alive(scope) {
                        let id = self.next_request();
                        self.services.insert(id, PendingService { window, scope, done });
                        self.staged_host.push(HostCommand::Service { window, id: ServiceId(id), request });
                    }
                }
                AppCommand::EditorEdit { window, event } => routed.push_back(Routed::Input(window, Input::Editor(event))),
                AppCommand::Paste { window, target, event } => {
                    self.deliver_paste(window, Some(target), event, routed);
                }
                AppCommand::NativeCommand { window, command } => self.staged_host.push(HostCommand::NativeCommand { window, command }),
            }
        }
    }

    fn react(&mut self) -> Result<(), TurnError> {
        let tasks = self.tasks.spawner();
        let system_preferences = self.system_preferences.read_only();
        for window in self.windows.values_mut() {
            if let Some(root) = window.root.take() {
                let scope = window.scope;
                root(&mut MountCx { world: &mut self.world, scope, window, tasks: &tasks, system_preferences, current_structure: None });
            }
        }
        for _ in 0..WINDOW_ROUNDS {
            for window in self.windows.values_mut() {
                Self::register_work(&mut self.world, window);
            }
            self.world.flush_derivations();

            let mut added_work = false;
            while let Some(observer) = self.world.next_observer(FlushTier::Structure) {
                let (scope, window) = self.world.owner(observer);
                let Some(window) = window.and_then(|id| self.windows.get_mut(&id)) else { continue };
                let Some(mut structure) = window.structures.remove(&observer) else { continue };
                if window.scope_is_retiring(&self.world, scope) {
                    self.world.retire_node(observer);
                    continue;
                }
                if self.world.evaluate_observer(observer, |read| structure.check(read)) == Some(true) {
                    structure.build(&mut MountCx {
                        world: &mut self.world,
                        scope,
                        window,
                        tasks: &tasks,
                        system_preferences,
                        current_structure: Some(observer),
                    });
                    added_work = true;
                }
                window.structures.insert(observer, structure);
            }
            if added_work {
                continue;
            }
            for window in self.windows.values_mut() {
                for (scope, call) in std::mem::take(&mut window.work.before) {
                    if self.world.is_scope_alive(scope) && !window.scope_is_retiring(&self.world, scope) {
                        call(&mut MountCx {
                            world: &mut self.world,
                            scope,
                            window,
                            tasks: &tasks,
                            system_preferences,
                            current_structure: None,
                        });
                    }
                }
            }
            if self.windows.values().all(|window| window.work.before.is_empty() && window.work.structures.is_empty()) {
                for window in self.windows.values_mut() {
                    Self::register_work(&mut self.world, window);
                }
                while let Some(observer) = self.world.next_observer(FlushTier::Binding) {
                    let (scope, window) = self.world.owner(observer);
                    let Some(window) = window.and_then(|id| self.windows.get_mut(&id)) else { continue };
                    let Some(mut binding) = window.bindings.remove(&observer) else { continue };
                    if window.is_retiring(binding.node) || window.scope_is_retiring(&self.world, scope) {
                        self.world.retire_node(observer);
                        continue;
                    }
                    if let Some(changes) = self.world.evaluate_observer(observer, |read| (binding.call)(read)) {
                        for change in changes {
                            window.stage_change(binding.node, change);
                        }
                    }
                    window.bindings.insert(observer, binding);
                }
                self.world.flush_watches();
                return Ok(());
            }
        }
        self.diagnostics.push(Diagnostic::new(&codes::FLUSH_RUNAWAY, "Structure repeatedly registered more structure in one turn"));
        Err(TurnError::LayoutLoop)
    }

    fn register_work(world: &mut crate::arena::ReactiveWorld, window: &mut WindowRuntime) {
        for (scope, observer) in std::mem::take(&mut window.work.structures) {
            let id = world.create_observer(ReactiveKind::Structure, scope, Some(window.id));
            window.structures.insert(id, observer);
        }
        for (scope, binding) in std::mem::take(&mut window.work.bindings) {
            let id = world.create_observer(ReactiveKind::Binding, scope, Some(window.id));
            window.bindings.insert(id, binding);
        }
    }

    fn commit(&mut self, routed: &mut VecDeque<Routed>) -> Result<(), TurnError> {
        let now = self.now();
        for window in self.windows.values_mut() {
            let previous = window.tree.focus();
            window.discard_retiring_allocations(&self.world);
            if let Err(rejected) = window.commit_tree() {
                self.failed = true;
                return Err(TurnError::Transaction { window: window.id, reason: rejected.reason });
            }
            Self::note_focus(window, previous, routed);
            for scope in std::mem::take(&mut window.work.retire_scopes) {
                if self.world.is_scope_alive(scope) {
                    self.world.dispose(scope);
                }
            }
            let dead_bindings: Vec<_> = window
                .bindings
                .iter()
                .filter_map(|(&observer, binding)| {
                    matches!(binding.node, NodeRef::Existing { id, .. } if !window.tree.is_live(id)).then_some(observer)
                })
                .collect();
            for id in dead_bindings {
                self.world.retire_node(id);
                window.bindings.remove(&id);
            }
            window.structures.retain(|&observer, _| self.world.is_alive(observer));
            window.bindings.retain(|&observer, _| self.world.is_alive(observer));
            window.handlers.retain(|_, handler| self.world.is_scope_alive(handler.scope));
            window.feedback.retain(|entry| {
                if let FeedbackSignal::Editor(signal) = entry.signal
                    && !matches!(entry.node, NodeRef::Existing { id, .. } if window.tree.is_live(id))
                {
                    if self.world.is_alive(signal.id()) {
                        self.world.write(window.scope, Some(window.id)).set_if_changed(signal, None);
                    }
                    return false;
                }
                self.world.is_scope_alive(entry.scope)
            });
            window.reveal.retain(|entry| self.world.is_scope_alive(entry.scope));
            window.copy_rows.retain(|entry| self.world.is_scope_alive(entry.scope));
            window.authoring.prune(&self.world);

            let decision = window.arbiter.reconcile(&window.tree, now);
            window.scrollbar_hovered =
                window.scrollbar_hovered.filter(|key| window.arbiter.pointer_hover_active() && window.tree.is_interactive(key.node));
            window.force_project |= window.engine.update_scrollbars(window.scrollbar_hovered, window.arbiter.dragged_scrollbar());
            for op in decision.operations {
                let previous = window.tree.focus();
                Self::apply_and_note_hover(window, op, routed);
                Self::note_focus(window, previous, routed);
            }
            routed.extend(decision.intents.into_iter().map(|intent| Routed::Intent(window.id, intent)));
        }
        self.tasks.cancel_retired_scopes(&self.world);
        self.cancel_platform_work();
        self.world
            .run_watch_effects_with_tree(&mut self.commands, &self.tasks.spawner(), |id| self.windows.get(&id).map(|window| &window.tree));
        Ok(())
    }

    fn project(&mut self, routed: &mut VecDeque<Routed>) -> Result<(), TurnError> {
        let now = self.now().as_secs_f64();
        for window in self.windows.values_mut() {
            self.diagnostics.append(&mut window.pending.diagnostics);
            if window.pending.is_empty() && !window.force_project {
                continue;
            }
            window.projection_open = true;
            let feedback = window
                .engine
                .project(&window.tree, &window.pending, window.options.viewport, now)
                .map_err(|error| TurnError::Project { window: window.id, error })?;
            window.staged.merge(std::mem::take(&mut window.pending));
            window.force_project = false;
            window.next_deadline = feedback.next_deadline;
            for update in feedback.updates {
                let previous = window.tree.focus();
                window.apply(update);
                Self::note_focus(window, previous, routed);
            }
            for reply in feedback.replies {
                if let HostReply::ExitFinished(ticket) = reply {
                    window.apply(RuntimeOp::FinishExit(ticket));
                } else {
                    routed.push_back(Routed::Input(window.id, Input::Host(reply)));
                }
            }
            for binding in &window.feedback {
                let NodeRef::Existing { id, .. } = binding.node else { continue };
                if !window.tree.is_live(id) || !self.world.is_scope_alive(binding.scope) {
                    continue;
                }
                let mut write = self.world.write(binding.scope, Some(window.id));
                match binding.signal {
                    FeedbackSignal::Media(signal) => {
                        if let Some(slot) = window.tree.media_source(id) {
                            write.set_if_changed(signal, slot.state.clone());
                        }
                    }
                    FeedbackSignal::Geometry(signal) => {
                        write.set_if_changed(signal, window.engine.geometry(id));
                    }
                    FeedbackSignal::Composition(signal) => {
                        let composing = window.tree.editor(id).is_some_and(|editor| editor.composition().is_some());
                        write.set_if_changed(signal, composing);
                    }
                    FeedbackSignal::Editor(signal) => {
                        let editor = EditorSession::active(&window.tree).filter(|editor| editor.id().node == id);
                        write.set_if_changed(signal, editor.and_then(|editor| editor.state()));
                    }
                    FeedbackSignal::Scroll(signal) => {
                        write.set_if_changed(signal, window.tree.scroll(id).map_or(Vec2::default(), |slot| slot.offset));
                    }
                    FeedbackSignal::ScrollAnchor(signal) => {
                        write.set_if_changed(signal, window.engine.scroll_anchor(&window.tree, id));
                    }
                    FeedbackSignal::Pinned(signal) => {
                        let pinned = window
                            .tree
                            .focus()
                            .into_iter()
                            .chain(window.arbiter.captured_node())
                            .chain(window.tree.selection_endpoints())
                            .any(|node| window.tree.is_within(node, id));
                        write.set_if_changed(signal, pinned);
                    }
                }
            }
        }
        Ok(())
    }

    fn apply_operation(&mut self, id: WindowId, op: RuntimeOp, routed: &mut VecDeque<Routed>) {
        self.apply_operation_with_pointer(id, op, None, routed);
    }

    fn apply_operation_with_pointer(&mut self, id: WindowId, op: RuntimeOp, pointer: Option<FocusHandoff>, routed: &mut VecDeque<Routed>) {
        if let Some(window) = self.windows.get_mut(&id) {
            let previous = window.tree.focus();
            let sets_focus = matches!(op, RuntimeOp::SetFocus { .. });
            Self::apply_and_note_hover(window, op, routed);
            let next = window.tree.focus();
            if sets_focus
                && previous != next
                && let Some(previous) = previous
            {
                let tree = &window.tree;
                let dismiss = pointer.map_or_else(|| focus_leave(tree, previous, next), |pointer| pointer.focus_leave(tree, previous));
                if let Some(dismiss) = dismiss {
                    routed.push_front(Routed::FocusLeave(id, next, dismiss));
                }
            }
            Self::note_focus(window, previous, routed);
        }
    }

    fn apply_and_note_hover(window: &mut WindowRuntime, op: RuntimeOp, routed: &mut VecDeque<Routed>) {
        let hover = if let RuntimeOp::SetState { node, state, on } = &op
            && state.contains(State::HOVER)
            && (window.tree.handler(*node, HandlerKind::Hover).is_some() || window.tree.link(*node).is_some())
            && window.tree.node(*node).is_some_and(|core| core.state.contains(State::HOVER) != *on)
        {
            Some((*node, *on))
        } else {
            None
        };
        window.apply(op);
        if let Some((node, hovered)) = hover {
            routed.push_back(Routed::HoverEvent(window.id, node, hovered));
        }
    }

    fn note_focus(window: &WindowRuntime, previous: Option<NodeId>, routed: &mut VecDeque<Routed>) {
        let next = window.tree.focus();
        if previous == next {
            return;
        }
        if let Some(node) = next {
            routed.push_front(Routed::FocusEvent(window.id, node, true));
        }
        if let Some(node) = previous {
            routed.push_front(Routed::FocusEvent(window.id, node, false));
        }
    }

    fn update_tooltips(&mut self, now: std::time::Duration) {
        for window in self.windows.values_mut() {
            let modal = window.tree.front_modal();
            let hovered = window.tooltip.position().and_then(|position| {
                if window.engine.scrollbar_hit(&window.tree, position).is_some() {
                    return None;
                }
                let hit = window.engine.hit_test(&window.tree, position).map(|target| target.hit);
                window.arbiter.hovered_tooltip(&window.tree, hit, modal)
            });
            let tooltip = window.tooltip.update(modal, hovered, now);
            self.world.write(window.scope, Some(window.id)).set_if_changed(window.hovered_tooltip, tooltip);
        }
    }

    // Resolve retained styles with the staged theme before mounting or updating nodes.
    fn prepare_presentation(&mut self, routed: &mut VecDeque<Routed>) {
        for window in self.windows.values_mut() {
            for update in window.authoring.presentation_updates(&mut self.world) {
                update.apply(window.pending_presentation.get_or_insert_with(|| window.options.presentation.clone()));
            }
            let Some(presentation) = &window.pending_presentation else { continue };
            let theme = &presentation.theme;
            let material = presentation.material;
            if window.tree.theme() != theme {
                let op = RuntimeOp::ThemeChanged(Box::new(theme.clone()));
                let previous = window.tree.focus();
                let width = window.options.viewport.size.width;
                let breakpoint = theme.breakpoint_of(width);
                window.apply(op);
                window.apply(RuntimeOp::SetViewportContext { breakpoint, width, touch: window.options.touch });
                self.world.write(window.scope, Some(window.id)).set_if_changed(window.breakpoint_source, breakpoint);
                Self::note_focus(window, previous, routed);
                window.force_project = true;
            }
            window.force_project |= window.engine.set_window_material(material);
        }
    }

    fn publish(&mut self) {
        let now = self.now();
        for window in self.windows.values_mut().filter(|window| window.projection_open) {
            if let Some((_, handler)) = window.back
                && window.named_command("navigation.back").is_none_or(|(_, current)| handler != current)
            {
                window.back = None;
            }
            window.last_commit = Some(window.engine.commit());
            window.committed_at = Some(now);
            window.settled = Some(octara_model::identity::WindowRev { tree: window.tree.revision(), viewport: window.viewport_rev });
            window.observation_rev = window.observation_rev.checked_add(1).expect("observation revision exhausted");
            window.staged = TreeDelta::default();
            window.projection_open = false;
        }
        for window in self.windows.values_mut() {
            let mut changed = !window.presentation_published;
            if let Some(presentation) = window.pending_presentation.take()
                && presentation != window.options.presentation
            {
                window.options.presentation = presentation;
                changed = true;
            }
            if changed {
                let presentation = window.presentation();
                self.staged_host.push(HostCommand::UpdateWindowPresentation {
                    window: window.id,
                    title: presentation.title.clone(),
                    color_scheme: presentation.theme.scheme,
                    material: presentation.material,
                });
                window.presentation_published = true;
            }
            let Some(key) = &window.options.persistence_key else { continue };
            let geometry = window.geometry;
            if self.window_geometry.get(key) != Some(&geometry)
                && [geometry.size.width, geometry.size.height, geometry.scale].into_iter().all(|value| value.is_finite() && value > 0.0)
            {
                self.window_geometry.insert(key.clone(), geometry);
                if !geometry.maximized && !geometry.fullscreen {
                    self.normal_window_geometry.insert(key.clone(), geometry);
                }
            }
        }
        if self.published_badge != self.app_badge {
            self.published_badge.clone_from(&self.app_badge);
            self.staged_host.push(HostCommand::SetAppBadge { badge: self.app_badge.clone() });
        }
        let menu_window = self.menu_window.filter(|id| self.windows.contains_key(id)).or_else(|| self.windows.keys().next().copied());
        self.menu_window = menu_window;
        let menus = || menu_window.into_iter().flat_map(|id| self.windows[&id].authoring.menus(&self.world));
        if !menus().eq(&self.published_menus) {
            self.published_menus = menus().cloned().collect();
            self.staged_host.push(HostCommand::Menus(self.published_menus.clone()));
        }
        self.staged_host.retain(|command| match command {
            HostCommand::NativeCommand { window, command } => self.windows.get(window).is_some_and(|window| {
                let tree = &window.tree;
                let node = command.id.node();
                tree.is_live(node) && tree.native(node).is_some_and(|native| native.live)
            }),
            _ => true,
        });
        self.published_host.append(&mut self.staged_host);
    }

    fn rollback(&mut self) {
        self.staged_host.retain(|command| !matches!(command, HostCommand::NativeCommand { .. }));
        self.commands.queued.retain(|command| !matches!(command, AppCommand::NativeCommand { .. }));
        for window in self.windows.values_mut().filter(|window| window.projection_open) {
            window.engine.rollback();
            window.staged.merge(std::mem::take(&mut window.pending));
            window.pending = std::mem::take(&mut window.staged);
            window.projection_open = false;
            window.force_project = true;
            window.settled = None;
        }
    }
}
