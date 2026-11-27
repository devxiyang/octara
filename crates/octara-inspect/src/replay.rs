//! Time-ordered recording over interaction, editor, host, and agent channels.

use crate::{
    InspectHost, Inspector,
    protocol::{AgentAction, AgentCommand, AgentReceipt, AgentRequest, WindowRef},
};
use octara::{
    App,
    runtime::{
        Input,
        turn::{TurnError, TurnReceipt},
    },
    tree::editor::EditorSession,
};
use octara_model::{
    ask::{Answer, ServiceOutcome},
    identity::{EditorSessionId, WindowId},
    input::{EditorEvent, HostReply, InteractionEvent, Key, KeyEvent},
    semantics::A11yAction,
};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, fmt, rc::Rc, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Interaction,
    Editor,
    Host,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Interaction { window: WindowId, event: InteractionEvent },
    Editor { window: WindowId, event: EditorEvent },
    Host { window: WindowId, reply: HostReply },
    Agent(AgentRequest),
    Redacted(Channel),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: Duration,
    pub event: Event,
}

/// A sequential sink for timestamped, privacy-filtered inspection events.
pub trait TraceRecorder {
    fn record(&mut self, entry: Entry);
}

impl<F: FnMut(Entry)> TraceRecorder for F {
    fn record(&mut self, entry: Entry) {
        self(entry);
    }
}

/// A recorder stores elapsed app time and filters private payloads before appending an entry.
/// Recreate the application with the recorded namespace and the same initial window state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Replay {
    namespace: [u64; 2],
    entries: Vec<Entry>,
}

impl TraceRecorder for Replay {
    fn record(&mut self, entry: Entry) {
        self.entries.push(entry);
    }
}

/// Share the in-memory recording with its caller while an inspector appends events.
impl TraceRecorder for Rc<RefCell<Replay>> {
    fn record(&mut self, entry: Entry) {
        self.borrow_mut().record(entry);
    }
}

impl Replay {
    pub fn new(namespace: [u64; 2]) -> Self {
        Self { namespace, entries: Vec::new() }
    }

    pub fn namespace(&self) -> [u64; 2] {
        self.namespace
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Observe the input immediately before its ordinary dispatch.
    pub fn record_input(&mut self, app: &App, window: WindowId, input: &Input) {
        self.record(input_entry(app, window, input));
    }

    /// Record the outer command only. Its translated input is regenerated during playback.
    pub fn record_request(&mut self, app: &App, request: &AgentRequest) {
        self.record(request_entry(app, request));
    }

    /// Do not substitute fabricated data for private input. Subsequent events may depend on
    /// the omitted state; callers can inspect the returned steps and the redaction markers.
    pub fn play(&self, app: &mut App, inspector: &mut Inspector, host: &mut impl InspectHost) -> Result<Vec<Step>, ReplayError> {
        if app.runtime().windows().any(|(_, window)| window.tree().id().namespace != self.namespace) {
            return Err(ReplayError::WrongNamespace);
        }
        let mut steps = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            let now = app.runtime().now();
            if entry.at < now {
                return Err(ReplayError::TimeWentBackwards { index });
            }
            if entry.at > now {
                host.advance_clock(app, entry.at - now).map_err(|error| ReplayError::Turn { index, error })?;
            }
            let delivery = match &entry.event {
                Event::Interaction { window, event } => raw(app, index, *window, Input::Interaction(*event))?,
                Event::Editor { window, event } => raw(app, index, *window, Input::Editor(event.clone()))?,
                Event::Host { window, reply } => raw(app, index, *window, Input::Host(reply.clone()))?,
                Event::Agent(request) => Step::Agent(Box::new(inspector.execute_replayed(app, host, request.clone()))),
                Event::Redacted(channel) => Step::Redacted(*channel),
            };
            steps.push(delivery);
        }
        Ok(steps)
    }
}

pub(crate) fn input_entry(app: &App, window: WindowId, input: &Input) -> Entry {
    let event = match input {
        Input::Interaction(event) if private_interaction(app, window, event) => Event::Redacted(Channel::Interaction),
        Input::Interaction(event) => Event::Interaction { window, event: *event },
        Input::Editor(event) if private_editor(app, window, event.session()) => Event::Redacted(Channel::Editor),
        Input::Editor(event) => Event::Editor { window, event: event.clone() },
        Input::Host(reply) if private_reply(app, window, reply) => Event::Redacted(Channel::Host),
        Input::Host(reply) => Event::Host { window, reply: reply.clone() },
    };
    Entry { at: app.runtime().now(), event }
}

pub(crate) fn request_entry(app: &App, request: &AgentRequest) -> Entry {
    let event = if private_request(app, request) { Event::Redacted(Channel::Agent) } else { Event::Agent(request.clone()) };
    Entry { at: app.runtime().now(), event }
}

fn raw(app: &mut App, index: usize, window: WindowId, input: Input) -> Result<Step, ReplayError> {
    app.dispatch(window, input).map(Step::Input).map_err(|error| ReplayError::Turn { index, error })
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Input(TurnReceipt),
    Agent(Box<AgentReceipt>),
    Redacted(Channel),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    WrongNamespace,
    TimeWentBackwards { index: usize },
    Turn { index: usize, error: TurnError },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongNamespace => f.write_str("replay requires the recorded application's namespace"),
            Self::TimeWentBackwards { index } => write!(f, "recorded time precedes the current clock at event {index}"),
            Self::Turn { index, error } => write!(f, "replay event {index} failed: {error}"),
        }
    }
}

impl std::error::Error for ReplayError {}

fn private_editor(app: &App, window: WindowId, session: EditorSessionId) -> bool {
    app.runtime().window(window).is_none_or(|window| {
        EditorSession::active(window.tree()).is_none_or(|editor| editor.id() != session || editor.editor().is_secure())
    })
}

fn printable_key(key: &KeyEvent) -> bool {
    matches!(key.key, Key::Char(_) | Key::Space) || key.code.is_some()
}

fn private_interaction(app: &App, window: WindowId, event: &InteractionEvent) -> bool {
    let InteractionEvent::Key(key) = event else { return false };
    printable_key(key)
        && app.runtime().window(window).is_none_or(|window| {
            let tree = window.tree();
            tree.focus().and_then(|node| tree.editor(node)).is_some_and(|editor| editor.is_secure())
        })
}

fn private_reply(app: &App, window: WindowId, reply: &HostReply) -> bool {
    match reply {
        HostReply::AskAnswer { answer: Answer::Paths(_) | Answer::NewPath(_), .. }
        | HostReply::FilesDropped { .. }
        | HostReply::ServiceReply { outcome: ServiceOutcome::FilePicked(_) | ServiceOutcome::ClipboardText(_), .. }
        | HostReply::NativeEvent { .. }
        | HostReply::NativeSnapshot { .. } => true,
        HostReply::AccessibilityAction { target, action: A11yAction::SetValue(_) } => app.runtime().window(window).is_none_or(|window| {
            !window.tree().contains_target(*target)
                || target.node().is_none_or(|node| window.tree().editor(node).is_some_and(|editor| editor.is_secure()))
        }),
        _ => false,
    }
}

fn private_request(app: &App, request: &AgentRequest) -> bool {
    let AgentCommand::Act { target, action } = &request.command else { return false };
    let window_ref = request.window.unwrap_or(WindowRef(target.tree.window));
    let window = app.runtime().windows().find_map(|(&id, _)| (WindowRef::from(id) == window_ref).then_some(id));
    match action {
        AgentAction::Editor { event } => window.is_none_or(|id| private_editor(app, id, event.session())),
        AgentAction::Key { event } => {
            printable_key(event) && window.is_none_or(|id| private_interaction(app, id, &InteractionEvent::Key(*event)))
        }
        AgentAction::Accessibility { action: A11yAction::SetValue(_) } => window.is_none_or(|id| {
            app.runtime().window(id).is_none_or(|window| {
                let tree = window.tree();
                !tree.contains_target(*target)
                    || target.node().is_none_or(|node| tree.editor(node).is_some_and(|editor| editor.is_secure()))
            })
        }),
        _ => false,
    }
}
