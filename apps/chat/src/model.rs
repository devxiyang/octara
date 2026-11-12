//! The chat's data: threads, and messages made of parts (text, reasoning, tool calls, files),
//! and how the events a model sends while it answers change them. Streamed text lives in
//! `TextBuffer`s, so a delta appends to its part without touching the message; anything else
//! (a new part, a status) changes the message.

use octara::prelude::TextBuffer;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

pub type ThreadId = u64;
pub type MessageId = u64;

/// Who wrote a message.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Author {
    User,
    Assistant,
}

/// A conversation, as the sidebar lists it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub title: String,
    /// When it last changed (seconds since the Unix epoch).
    pub updated: u64,
}

/// A file sent with a message.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub path: PathBuf,
}

/// Where a tool call is.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum ToolState {
    /// Its input is still arriving.
    Streaming,
    Running,
    Done,
    Failed(String),
}

/// A tool the model called: its input as it streams, its state and output.
#[derive(Clone, PartialEq, Debug)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub input: TextBuffer,
    pub state: ToolState,
    pub output: Option<String>,
}

/// One part of a message.
#[derive(Clone, PartialEq, Debug)]
pub enum Part {
    Text(TextBuffer),
    /// What the model thought, and when it started and stopped (the app's clock).
    Reasoning {
        text: TextBuffer,
        started: Duration,
        ended: Option<Duration>,
    },
    Tool(Tool),
    File(Attachment),
}

/// Where a message is.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Status {
    Streaming,
    Done,
    /// The person stopped it.
    Stopped,
    Failed(String),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Message {
    pub id: MessageId,
    pub thread: ThreadId,
    pub author: Author,
    pub parts: Vec<Part>,
    pub status: Status,
}

/// What a model sends while it answers.
#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    Reasoning(String),
    ReasoningEnd,
    Text(String),
    ToolStart { id: String, name: String },
    ToolInput { id: String, delta: String },
    ToolRun { id: String },
    ToolDone { id: String, output: String },
    ToolFailed { id: String, error: String },
    Done,
    Failed(String),
}

impl Message {
    pub fn user(id: MessageId, thread: ThreadId, text: &str, files: Vec<Attachment>) -> Message {
        let mut parts: Vec<Part> = files.into_iter().map(Part::File).collect();
        parts.push(Part::Text(TextBuffer::new(text)));
        Message { id, thread, author: Author::User, parts, status: Status::Done }
    }
    /// An answer about to stream.
    pub fn answer(id: MessageId, thread: ThreadId) -> Message {
        Message { id, thread, author: Author::Assistant, parts: Vec::new(), status: Status::Streaming }
    }

    /// The text of its text parts, joined by blank lines (what is sent back to the model, copied).
    pub fn text(&self) -> String {
        let texts: Vec<String> =
            self.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.get_untracked()) } else { None }).collect();
        texts.join("\n\n")
    }
    /// Its files.
    pub fn files(&self) -> Vec<Attachment> {
        self.parts.iter().filter_map(|p| if let Part::File(f) = p { Some(f.clone()) } else { None }).collect()
    }

    /// Applies `event` if it only adds to the part it belongs to (text to the last text or
    /// reasoning, input to a tool's): the part's buffer grows and the message stays as it is.
    /// Returns whether it did; otherwise `apply` it.
    pub fn stream(&self, event: &Event) -> bool {
        match (event, self.parts.last()) {
            (Event::Text(s), Some(Part::Text(t))) => t.append(s),
            (Event::Reasoning(s), Some(Part::Reasoning { text, ended: None, .. })) => text.append(s),
            (Event::ToolInput { id, delta }, _) => match self.tool(id) {
                Some(tool) if tool.state == ToolState::Streaming => tool.input.append(delta),
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// Applies `event` at `now` (the app's clock): new parts, tools moving on, the message
    /// ending. Deltas that `stream` takes are applied here too (starting their part).
    pub fn apply(&mut self, event: Event, now: Duration) {
        if self.status != Status::Streaming {
            return;
        }
        if self.stream(&event) {
            return;
        }
        match event {
            Event::Text(s) => {
                self.end_reasoning(now);
                self.parts.push(Part::Text(TextBuffer::new(s)));
            }
            Event::Reasoning(s) => self.parts.push(Part::Reasoning { text: TextBuffer::new(s), started: now, ended: None }),
            Event::ReasoningEnd => self.end_reasoning(now),
            Event::ToolStart { id, name } => {
                self.end_reasoning(now);
                self.parts.push(Part::Tool(Tool { id, name, input: TextBuffer::new(""), state: ToolState::Streaming, output: None }));
            }
            Event::ToolInput { .. } => {}
            Event::ToolRun { id } => self.set_tool(&id, |t| t.state = ToolState::Running),
            Event::ToolDone { id, output } => self.set_tool(&id, |t| {
                t.state = ToolState::Done;
                t.output = Some(output);
            }),
            Event::ToolFailed { id, error } => self.set_tool(&id, |t| t.state = ToolState::Failed(error)),
            Event::Done => self.finish(Status::Done, now),
            Event::Failed(why) => self.finish(Status::Failed(why), now),
        }
    }

    /// Ends it as `status` (stopped by the person, failed, done): what is still open closes.
    pub fn finish(&mut self, status: Status, now: Duration) {
        self.end_reasoning(now);
        for p in &mut self.parts {
            if let Part::Tool(t) = p
                && matches!(t.state, ToolState::Streaming | ToolState::Running)
            {
                t.state = ToolState::Failed("Stopped".into());
            }
        }
        self.status = status;
    }

    fn tool(&self, id: &str) -> Option<&Tool> {
        self.parts.iter().find_map(|p| match p {
            Part::Tool(t) if t.id == id => Some(t),
            _ => None,
        })
    }
    fn set_tool(&mut self, id: &str, f: impl FnOnce(&mut Tool)) {
        if let Some(Part::Tool(t)) = self.parts.iter_mut().find(|p| matches!(p, Part::Tool(t) if t.id == id)) {
            f(t);
        }
    }
    fn end_reasoning(&mut self, now: Duration) {
        for p in &mut self.parts {
            if let Part::Reasoning { ended: e @ None, .. } = p {
                *e = Some(now);
            }
        }
    }
}

// ---- saved form ----

/// A part as saved: plain text.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum SavedPart {
    Text(String),
    Reasoning { text: String, seconds: f64 },
    Tool { id: String, name: String, input: String, state: ToolState, output: Option<String> },
    File(Attachment),
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SavedMessage {
    pub id: MessageId,
    pub author: Author,
    pub parts: Vec<SavedPart>,
    pub status: Status,
}

impl Message {
    pub fn save(&self) -> SavedMessage {
        let parts = self
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => SavedPart::Text(t.get_untracked()),
                Part::Reasoning { text, started, ended } => SavedPart::Reasoning {
                    text: text.get_untracked(),
                    seconds: ended.map_or(0.0, |e| (e.saturating_sub(*started)).as_secs_f64()),
                },
                Part::Tool(t) => SavedPart::Tool {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    input: t.input.get_untracked(),
                    state: t.state.clone(),
                    output: t.output.clone(),
                },
                Part::File(f) => SavedPart::File(f.clone()),
            })
            .collect();
        // A message still streaming when saved was cut off.
        let status = if self.status == Status::Streaming { Status::Stopped } else { self.status.clone() };
        SavedMessage { id: self.id, author: self.author, parts, status }
    }

    pub fn load(saved: SavedMessage, thread: ThreadId) -> Message {
        let parts = saved
            .parts
            .into_iter()
            .map(|p| match p {
                SavedPart::Text(t) => Part::Text(TextBuffer::new(t)),
                SavedPart::Reasoning { text, seconds } => {
                    Part::Reasoning { text: TextBuffer::new(text), started: Duration::ZERO, ended: Some(Duration::from_secs_f64(seconds)) }
                }
                SavedPart::Tool { id, name, input, state, output } => {
                    Part::Tool(Tool { id, name, input: TextBuffer::new(input), state, output })
                }
                SavedPart::File(f) => Part::File(f),
            })
            .collect();
        Message { id: saved.id, thread, author: saved.author, parts, status: saved.status }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// Runs `f` in an app (text buffers are reactive).
    fn run(f: impl FnOnce()) {
        let app = octara::app::App::new(octara::platform::HeadlessPlatform::default());
        app.run(f);
    }

    #[test]
    fn events_build_an_answer() {
        run(|| {
            let mut m = Message::answer(1, 1);
            for (e, t) in [
                (Event::Reasoning("Think".into()), 0),
                (Event::Reasoning("ing.".into()), 10),
                (Event::ToolStart { id: "t1".into(), name: "weather".into() }, 500),
                (Event::ToolInput { id: "t1".into(), delta: "{\"city\":".into() }, 510),
                (Event::ToolInput { id: "t1".into(), delta: "\"Oslo\"}".into() }, 520),
                (Event::ToolRun { id: "t1".into() }, 530),
                (Event::ToolDone { id: "t1".into(), output: "4 °C".into() }, 700),
                (Event::Text("It is ".into()), 710),
                (Event::Text("4 °C.".into()), 720),
                (Event::Done, 730),
            ] {
                m.apply(e, s(t));
            }
            assert_eq!(m.status, Status::Done);
            let Part::Reasoning { text, started, ended } = &m.parts[0] else { panic!() };
            assert_eq!(
                (text.get_untracked().as_str(), *started, *ended),
                ("Thinking.", s(0), Some(s(500))),
                "a tool call ends the thinking"
            );
            let Part::Tool(t) = &m.parts[1] else { panic!() };
            assert_eq!(
                (t.input.get_untracked().as_str(), &t.state, t.output.as_deref()),
                ("{\"city\":\"Oslo\"}", &ToolState::Done, Some("4 °C"))
            );
            assert_eq!(m.text(), "It is 4 °C.");
            assert_eq!(m.parts.len(), 3);
        });
    }

    #[test]
    fn deltas_stream_into_their_part_and_the_rest_needs_the_message() {
        run(|| {
            let mut m = Message::answer(1, 1);
            assert!(!m.stream(&Event::Text("a".into())), "the first text starts a part");
            m.apply(Event::Text("a".into()), s(0));
            assert!(m.stream(&Event::Text("b".into())));
            assert_eq!(m.text(), "ab");
            assert!(!m.stream(&Event::Done));
        });
    }

    #[test]
    fn stopping_closes_what_is_open_and_saving_keeps_the_rest() {
        run(|| {
            let mut m = Message::answer(3, 9);
            m.apply(Event::Reasoning("Hm".into()), s(0));
            m.apply(Event::ToolStart { id: "x".into(), name: "search".into() }, s(1000));
            m.finish(Status::Stopped, s(2000));
            let Part::Tool(t) = &m.parts[1] else { panic!() };
            assert_eq!(t.state, ToolState::Failed("Stopped".into()));
            m.apply(Event::Text("late".into()), s(3000));
            assert_eq!(m.text(), "", "nothing lands after it ended");
            let back = Message::load(serde_json::from_str(&serde_json::to_string(&m.save()).unwrap()).unwrap(), 9);
            assert_eq!(back.save(), m.save());
            let Part::Reasoning { ended, .. } = &back.parts[0] else { panic!() };
            assert_eq!(*ended, Some(s(1000)), "thought for a second");
        });
    }
}
