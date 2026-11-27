//! Target validation and translation into ordinary application input.

use crate::protocol::{
    AgentAction, AgentCommand, AgentReceipt, AgentRequest, Capture, EditorSnapshot, NodeSnapshot, PROTOCOL, Progress, TargetRef,
    TreeSnapshot, WindowInfo, WindowRef,
};
use crate::{AuditRule, InspectTransport, JsonLines, TraceRecorder, audit, replay};
use octara::{
    App,
    runtime::{
        Input, WindowRuntime,
        cycle::{AppError, CycleReceipt},
        engine::AccessibilityConsumer,
    },
    tree::{NodeKind, editor::EditorSession, tables::HandlerKind},
};
use octara_model::{
    capture::CaptureSource,
    diagnostics::{Code, codes},
    geometry::{Point, Rect},
    identity::WindowId,
    input::{Button, ContactTarget, HostReply, InteractionEvent, Modifiers, PointerId, PointerKind, WheelPhase},
    semantics::{Relations, Role},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc, time::Duration};

/// The presentation observations and capture sources supplied by an inspection host.
pub trait InspectHost {
    fn presented_frame(&self, _window: WindowId) -> Option<u64> {
        None
    }

    fn set_picking(&mut self, _window: WindowId, _enabled: bool) {}

    fn supports_restart_state(&self) -> bool {
        false
    }

    fn request_shutdown(&mut self) -> bool {
        false
    }

    fn advance_clock(&mut self, app: &mut App, elapsed: Duration) -> Result<(), AppError> {
        app.advance_to(app.runtime().now().saturating_add(elapsed))?;
        self.synchronize(app)
    }

    fn synchronize(&mut self, _app: &mut App) -> Result<(), AppError> {
        Ok(())
    }

    fn capture(&mut self, _window: &WindowRuntime, _source: CaptureSource, _now: Duration) -> Result<Capture, String> {
        Err("the host does not offer this capture source".into())
    }
}

impl InspectHost for () {}

pub struct Inspector {
    transport: Box<dyn InspectTransport>,
    state: InspectionState,
}

impl Default for Inspector {
    fn default() -> Self {
        Self { transport: Box::new(JsonLines), state: InspectionState::default() }
    }
}

#[derive(Default)]
struct InspectionState {
    picking: BTreeMap<WindowId, bool>,
    contacts: BTreeMap<WindowId, (PointerId, TargetRef)>,
    tap_sequence: u64,
    rules: Vec<Arc<dyn AuditRule>>,
    recorder: Option<Rc<RefCell<dyn TraceRecorder>>>,
}

impl Inspector {
    /// Register a rule to run on every inspected window after the built-in rules.
    pub fn with_audit_rule(mut self, rule: Arc<dyn AuditRule>) -> Self {
        self.state.rules.push(rule);
        self
    }

    /// Use a custom payload codec when exchanging bytes. The default is [`JsonLines`].
    pub fn with_transport(mut self, transport: impl InspectTransport + 'static) -> Self {
        self.transport = Box::new(transport);
        self
    }

    /// Record agent commands and raw inputs through a privacy-filtered trace sink.
    pub fn with_trace_recorder(mut self, recorder: impl TraceRecorder + 'static) -> Self {
        self.state.recorder = Some(Rc::new(RefCell::new(recorder)));
        self
    }

    /// Replace the runtime's module trace sink with this inspector's recorder, if configured.
    pub fn attach_module_trace(&self, app: &mut App) {
        let observer = self.state.recorder.clone().map(|recorder| {
            let record = move |observation: octara::module::ModuleObservation| {
                recorder.borrow_mut().record(replay::Entry { at: observation.at, event: replay::Event::Module(observation) });
            };
            Box::new(record) as Box<dyn FnMut(_)>
        });
        app.runtime_mut().set_module_observer(observer);
    }

    /// Process a complete payload on the application's owning thread.
    pub fn exchange(&mut self, app: &mut App, host: &mut impl InspectHost, payload: &[u8]) -> Vec<u8> {
        if self.state.recorder.is_some() {
            self.attach_module_trace(app);
        }
        self.transport.exchange(payload, &mut |request| self.state.execute(app, host, request))
    }

    /// Record host input before dispatch. Agent commands are recorded by `execute` and `exchange`.
    pub fn record_input(&mut self, app: &App, window: WindowId, input: &Input) {
        if let Some(recorder) = &self.state.recorder {
            recorder.borrow_mut().record(replay::input_entry(app, window, input));
        }
    }

    pub fn execute(&mut self, app: &mut App, host: &mut impl InspectHost, request: AgentRequest) -> AgentReceipt {
        if self.state.recorder.is_some() {
            self.attach_module_trace(app);
        }
        self.state.execute(app, host, request)
    }

    pub(crate) fn execute_replayed(&mut self, app: &mut App, host: &mut impl InspectHost, request: AgentRequest) -> AgentReceipt {
        self.state.dispatch_request(app, host, request)
    }
}

impl InspectionState {
    fn execute(&mut self, app: &mut App, host: &mut impl InspectHost, request: AgentRequest) -> AgentReceipt {
        if let Some(recorder) = &self.recorder {
            recorder.borrow_mut().record(replay::request_entry(app, &request));
        }
        self.dispatch_request(app, host, request)
    }

    fn dispatch_request(&mut self, app: &mut App, host: &mut impl InspectHost, request: AgentRequest) -> AgentReceipt {
        let mut receipt = AgentReceipt::success(request.window);
        if request.protocol != PROTOCOL {
            receipt.fail(&codes::INSPECT_PROTOCOL, "expected the octara.inspect/2 protocol", false);
            return receipt;
        }
        if let Err(error) = host.synchronize(app) {
            receipt.fail(app_error_code(&error), error.to_string(), false);
            return receipt;
        }
        if matches!(request.command, AgentCommand::ExportRestartState | AgentCommand::Shutdown) {
            if request.window.is_some() {
                receipt.fail(&codes::INSPECT_REQUEST, "restart commands are scoped to the application", false);
            } else {
                match request.command {
                    AgentCommand::ExportRestartState if host.supports_restart_state() => {
                        if app.runtime().is_failed() {
                            receipt.fail(&codes::APP_FAILED, "application stopped after a failed cycle", false);
                        } else {
                            receipt.restart_state = Some(app.export_persisted());
                        }
                    }
                    AgentCommand::Shutdown if host.request_shutdown() => {}
                    _ => receipt.fail(&codes::INSPECT_RESTART_UNAVAILABLE, "restart commands require a dev host", false),
                }
            }
            return receipt;
        }
        self.picking.retain(|id, _| app.runtime().window(*id).is_some());
        self.contacts.retain(|id, (_, target)| app.runtime().window(*id).is_some_and(|window| window.tree().contains_target(*target)));
        let target = match &request.command {
            AgentCommand::Act { target, .. } => Some(*target),
            _ => None,
        };
        if matches!(request.command, AgentCommand::ListWindows) {
            receipt.windows = Some(
                app.runtime()
                    .windows()
                    .map(|(&id, window)| WindowInfo {
                        id: id.into(),
                        target: TargetRef::for_window(window.tree().id()),
                        title: window.presentation().title.to_string(),
                        viewport: window.options().viewport,
                        progress: progress(window, host),
                    })
                    .collect(),
            );
            return receipt;
        }
        let window_ref = request.window.or_else(|| target.map(|target| WindowRef(target.tree.window)));
        receipt.window = window_ref;
        // The application clock is global. Its command also works without a primary window.
        if window_ref.is_none()
            && let AgentCommand::AdvanceClock { ms } = request.command
        {
            if let Err(error) = host.advance_clock(app, Duration::from_millis(ms)) {
                receipt.fail(app_error_code(&error), error.to_string(), false);
            }
            receipt.diagnostics = app.runtime_mut().take_diagnostics();
            return receipt;
        }
        let window = {
            let mut windows = app.runtime().windows().map(|(&id, _)| id);
            match window_ref {
                Some(reference) => windows.find(|&id| WindowRef::from(id) == reference),
                None => windows.next().filter(|_| windows.next().is_none()),
            }
        };
        let Some(id) = window else {
            let code = if window_ref.is_some() { &codes::STALE_TARGET } else { &codes::INSPECT_REQUEST };
            receipt.fail(code, "request does not identify an open window", false);
            return receipt;
        };
        receipt.window = Some(id.into());
        let state = app.runtime().window(id).expect("selected open window");
        if let Some(target) = target
            && !state.tree().contains_target(target)
        {
            receipt.fail(&codes::STALE_TARGET, "target no longer identifies a live allocation in this window", false);
        } else if app.runtime().is_failed() {
            receipt.fail(&codes::APP_FAILED, "application stopped after a failed cycle", false);
        } else if state.settled().is_none() && !matches!(request.command, AgentCommand::AdvanceClock { .. }) {
            receipt.fail(&codes::NOT_SETTLED, "window has no settled observation", false);
        } else {
            match request.command {
                AgentCommand::Observe => {
                    let enabled = self.picking.get(&id).copied().unwrap_or(false);
                    receipt.snapshot = Some(snapshot(state, enabled));
                }
                AgentCommand::Audit => {
                    state.set_accessibility_active(AccessibilityConsumer::Inspector, true);
                    receipt.findings = Some(audit::run(state, app.runtime().world(), &self.rules));
                }
                AgentCommand::Act { target, action } => match self.act(app, id, target, action) {
                    Ok(cycle) => {
                        receipt.handled = Some(cycle.handled);
                        if let Err(error) = host.synchronize(app) {
                            receipt.fail(app_error_code(&error), error.to_string(), true);
                        }
                    }
                    Err(error) => receipt.fail(error.code, error.message, error.delivered),
                },
                AgentCommand::Capture { source, settle_ms } => {
                    if settle_ms > 0
                        && let Err(error) = host.advance_clock(app, Duration::from_millis(settle_ms))
                    {
                        receipt.fail(app_error_code(&error), error.to_string(), false);
                    } else if let Some(window) = app.runtime().window(id).filter(|window| window.settled().is_some()) {
                        match host.capture(window, source, app.runtime().now()) {
                            Ok(capture) => receipt.capture = Some(capture),
                            Err(error) => receipt.fail(&codes::CAPTURE_UNAVAILABLE, error, false),
                        }
                    } else {
                        receipt.fail(&codes::NOT_SETTLED, "window has no settled capture", false);
                    }
                }
                AgentCommand::AdvanceClock { ms } => {
                    if let Err(error) = host.advance_clock(app, Duration::from_millis(ms)) {
                        receipt.fail(app_error_code(&error), error.to_string(), false);
                    }
                }
                AgentCommand::SetPicking { enabled } => {
                    self.picking.insert(id, enabled);
                    host.set_picking(id, enabled);
                }
                AgentCommand::ListWindows | AgentCommand::ExportRestartState | AgentCommand::Shutdown => unreachable!(),
            }
        }
        receipt.progress = app.runtime().window(id).map_or_else(Progress::default, |window| progress(window, host));
        receipt.diagnostics = app.runtime_mut().take_diagnostics();
        receipt
    }

    fn act(&mut self, app: &mut App, id: WindowId, target: TargetRef, action: AgentAction) -> Result<CycleReceipt, ActionError> {
        let state = app.runtime().window(id).expect("validated window");
        let tree = state.tree();
        let input = match action {
            AgentAction::Tap { pos, modifiers } => return self.tap(app, id, target, pos, modifiers),
            AgentAction::PointerDown { id: pointer_id, pointer, pos, button, modifiers } => {
                if self.contacts.contains_key(&id) {
                    return Err(ActionError::rejected("another agent pointer is already down in this window"));
                }
                let hit = guarded_hit(state, target, pos, button)?;
                let event = InteractionEvent::PointerDown { id: pointer_id, kind: pointer, pos, target: hit, button, mods: modifiers };
                let receipt = dispatch(app, id, Input::Interaction(event))?;
                self.contacts.insert(id, (pointer_id, target));
                return Ok(receipt);
            }
            AgentAction::PointerMove { id: pointer, pos, modifiers } => {
                self.check_contact(id, pointer, target)?;
                check_point(pos)?;
                Input::Interaction(InteractionEvent::PointerMove { id: pointer, pos, mods: modifiers })
            }
            AgentAction::PointerUp { id: pointer, pos, modifiers } => {
                self.check_contact(id, pointer, target)?;
                check_point(pos)?;
                let hit = state.hit_test(pos);
                self.contacts.remove(&id);
                Input::Interaction(InteractionEvent::PointerUp { id: pointer, target: hit, pos, mods: modifiers })
            }
            AgentAction::PointerCancel { id: pointer } => {
                self.check_contact(id, pointer, target)?;
                self.contacts.remove(&id);
                Input::Interaction(InteractionEvent::PointerCancel { id: pointer })
            }
            AgentAction::Wheel { pos, delta, modifiers } => {
                check_point(pos)?;
                if !delta.x.is_finite() || !delta.y.is_finite() {
                    return Err(ActionError::rejected("wheel delta must be finite"));
                }
                if let Some(node) = target.node() {
                    let hit = state.hit_test(pos).and_then(|hit| hit.hit.node());
                    let receiver = std::iter::successors(hit, |&node| tree.physical_parent(node))
                        .find(|&node| tree.scroll(node).is_some() && tree.is_interactive(node));
                    if receiver != Some(node) {
                        return Err(ActionError::rejected("the wheel position routes to another scroll target"));
                    }
                }
                Input::Interaction(InteractionEvent::Wheel { pos, delta, phase: WheelPhase::Discrete, mods: modifiers })
            }
            AgentAction::Key { event } => {
                if target.node().is_some_and(|node| tree.focus() != Some(node) || !tree.is_interactive(node)) {
                    return Err(ActionError::rejected("the keyboard target is not focused"));
                }
                Input::Interaction(InteractionEvent::Key(event))
            }
            AgentAction::Editor { event } => {
                let current = EditorSession::active(tree).is_some_and(|session| {
                    target.node() == Some(session.id().node)
                        && event.session() == session.id()
                        && event.expected_rev() == session.editor().document_rev()
                });
                if !current {
                    return Err(ActionError::stale("the editor session or document revision changed", false));
                }
                Input::Editor(event)
            }
            AgentAction::Accessibility { action } => {
                if target.node().is_none_or(|node| !tree.is_interactive(node)) {
                    return Err(ActionError::rejected("the accessibility target is blocked"));
                }
                Input::Host(HostReply::AccessibilityAction { target, action })
            }
            AgentAction::MenuCommand { name } => {
                if target.node().is_some() {
                    return Err(ActionError::rejected("menu commands are scoped to a window"));
                }
                Input::Host(HostReply::MenuCommand { name, chord: None })
            }
        };
        dispatch(app, id, input)
    }

    fn check_contact(&self, id: WindowId, pointer: PointerId, target: TargetRef) -> Result<(), ActionError> {
        if self.contacts.get(&id) != Some(&(pointer, target)) {
            return Err(ActionError::rejected("pointer contact belongs to a different target or pointer"));
        }
        Ok(())
    }

    fn tap(
        &mut self,
        app: &mut App,
        id: WindowId,
        target: TargetRef,
        pos: Point,
        modifiers: Modifiers,
    ) -> Result<CycleReceipt, ActionError> {
        if self.contacts.contains_key(&id) {
            return Err(ActionError::rejected("another agent pointer is already down in this window"));
        }
        let state = app.runtime().window(id).expect("validated window");
        let hit = guarded_hit(state, target, pos, Button::Primary)?;
        let pointer = PointerId(u64::MAX - self.tap_sequence);
        self.tap_sequence = self.tap_sequence.wrapping_add(1);
        let down = dispatch(
            app,
            id,
            Input::Interaction(InteractionEvent::PointerDown {
                id: pointer,
                kind: PointerKind::Mouse,
                target: hit,
                pos,
                button: Button::Primary,
                mods: modifiers,
            }),
        )?;
        let Some(state) = app.runtime().window(id) else { return Ok(down) };
        let release = guarded_hit(state, target, pos, Button::Primary);
        if release.is_err() {
            // The first half may already have delivered focus or dismissal. Unwind the contact
            // without releasing it into a different allocation.
            dispatch(app, id, Input::Interaction(InteractionEvent::PointerCancel { id: pointer })).map_err(ActionError::after_dispatch)?;
            return Err(ActionError::stale("the hit changed between pointer down and pointer up", true));
        }
        let up =
            dispatch(app, id, Input::Interaction(InteractionEvent::PointerUp { id: pointer, target: release.ok(), pos, mods: modifiers }))
                .map_err(ActionError::after_dispatch)?;
        Ok(CycleReceipt { delivered: true, handled: down.handled || up.handled, rounds: down.rounds + up.rounds })
    }
}

fn check_point(pos: Point) -> Result<(), ActionError> {
    if pos.x.is_finite() && pos.y.is_finite() { Ok(()) } else { Err(ActionError::rejected("pointer position must be finite")) }
}

fn guarded_hit(window: &WindowRuntime, target: TargetRef, pos: Point, button: Button) -> Result<ContactTarget, ActionError> {
    check_point(pos)?;
    let tree = window.tree();
    if !tree.contains_target(target) {
        return Err(ActionError::stale("target no longer identifies a live allocation", false));
    }
    let size = window.options().viewport.size;
    if !Rect::new(0.0, 0.0, size.width, size.height).contains(pos) {
        return Err(ActionError::rejected("pointer position is outside the window"));
    }
    let hit = window.hit_test(pos).unwrap_or(ContactTarget { hit: TargetRef::for_window(tree.id()), action: None });
    let receiver = if button == Button::Secondary {
        std::iter::successors(hit.hit.node(), |&node| tree.logical_parent(node))
            .find(|&node| tree.handler(node, HandlerKind::SecondaryPress).is_some())
            .map_or(hit.hit, |node| tree.guard(node))
    } else {
        hit.action.unwrap_or(hit.hit)
    };
    if receiver != target {
        return Err(ActionError::rejected("the point currently routes to a different receiver"));
    }
    Ok(hit)
}

fn dispatch(app: &mut App, id: WindowId, input: Input) -> Result<CycleReceipt, ActionError> {
    let receipt = app.dispatch(id, input).map_err(|error| ActionError {
        code: app_error_code(&error),
        message: error.to_string(),
        delivered: !matches!(error, AppError::UnknownWindow(_) | AppError::AppFailed),
    })?;
    if receipt.delivered { Ok(receipt) } else { Err(ActionError::stale("runtime rejected the input's target or revision", false)) }
}

fn app_error_code(error: &AppError) -> &'static Code {
    match error {
        AppError::UnknownWindow(_) => &codes::STALE_TARGET,
        AppError::AppFailed => &codes::APP_FAILED,
        AppError::DuplicateModule(_) => &codes::DUPLICATE_MODULE,
        AppError::ModuleInstall { .. } => &codes::MODULE_INSTALL_FAILED,
        AppError::Transaction { .. } | AppError::Project { .. } | AppError::LayoutLoop => &codes::NOT_SETTLED,
    }
}

struct ActionError {
    code: &'static Code,
    message: String,
    delivered: bool,
}

impl ActionError {
    fn rejected(message: &str) -> Self {
        Self { code: &codes::INPUT_REJECTED, message: message.into(), delivered: false }
    }

    fn stale(message: &str, delivered: bool) -> Self {
        Self { code: &codes::STALE_TARGET, message: message.into(), delivered }
    }

    fn after_dispatch(mut self) -> Self {
        self.delivered = true;
        self
    }
}

fn progress(window: &WindowRuntime, host: &impl InspectHost) -> Progress {
    Progress {
        observation_rev: window.observation_revision(),
        tree_rev: window.tree().revision().0,
        settled: window.settled().is_some(),
        committed_frame: window.last_commit().and_then(|commit| commit.frame),
        presented_frame: host.presented_frame(window.id()),
    }
}

pub(crate) fn snapshot(window: &WindowRuntime, picking: bool) -> TreeSnapshot {
    let tree = window.tree();
    let scope = tree.semantic_scope();
    let relation = |name: &str| {
        let [node] = tree.named_nodes(name) else { return None };
        scope.exposes(*node).then(|| tree.guard(*node))
    };
    let nodes = tree
        .subtree(tree.root())
        .filter(|&node| tree.is_live(node))
        .map(|node| {
            let semantics = tree.semantics(node);
            let editor = tree.editor(node);
            let value = match editor {
                Some(editor) => editor.reportable_value().map(str::to_owned),
                None => semantics.and_then(|slot| slot.a11y_value.as_deref()).map(str::to_owned),
            };
            NodeSnapshot {
                target: tree.guard(node),
                parent: tree.physical_parent(node).map(|parent| tree.guard(parent)),
                test_id: tree.provenance(node).and_then(|slot| slot.test_id.as_deref()).map(str::to_owned),
                role: semantics.and_then(|slot| slot.role).or_else(|| match tree.node(node)?.kind {
                    NodeKind::Root => Some(Role::Window),
                    NodeKind::Text => Some(Role::Label),
                    NodeKind::TextInput => match editor {
                        Some(editor) if editor.is_secure() => Some(Role::PasswordInput),
                        Some(editor) if editor.multiline => Some(Role::MultilineTextInput),
                        _ => Some(Role::TextInput),
                    },
                    NodeKind::Link => Some(Role::Link),
                    NodeKind::Image | NodeKind::Media => Some(Role::Image),
                    _ => None,
                }),
                label: semantics.and_then(|slot| slot.label.as_deref()).map(str::to_owned),
                checked: semantics.and_then(|slot| slot.checked),
                relations: semantics.filter(|_| scope.exposes(node)).map_or_else(Relations::default, |slot| {
                    let links = &slot.relations;
                    Relations {
                        active_descendant: links.active_descendant.as_deref().and_then(relation),
                        labelled_by: links.labelled_by.as_deref().and_then(relation),
                        described_by: links.described_by.as_deref().and_then(relation),
                        controls: links.controls.as_deref().and_then(relation),
                    }
                }),
                value,
                text: tree.text(node).map(|slot| slot.text.to_string()),
                native_props: tree.native(node).and_then(|view| view.typed_props.as_ref()?.inspect.clone()),
                geometry: window.geometry(node),
                scroll_offset: tree.scroll(node).map(|slot| slot.offset),
                shown: tree.is_shown(node),
                interactive: tree.is_interactive(node),
            }
        })
        .collect();
    let editor = EditorSession::active(tree).map(|session| {
        let editor = session.editor();
        EditorSnapshot {
            session: session.id(),
            document_rev: editor.document_rev(),
            secure: editor.is_secure(),
            value: editor.reportable_value().map(str::to_owned),
            selection: (!editor.is_secure()).then(|| editor.selection()),
            marked: if editor.is_secure() { None } else { editor.composition().map(|composition| composition.marked()) },
        }
    });
    TreeSnapshot {
        viewport: window.options().viewport,
        target: TargetRef::for_window(tree.id()),
        focus: tree.focus().map(|node| tree.guard(node)),
        modal: tree.front_modal().map(|node| tree.guard(node)),
        picking,
        nodes,
        editor,
    }
}
