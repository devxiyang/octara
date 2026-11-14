//! Deterministic history and a real Model channel for long-session UI qualification.

use octara::prelude::*;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::{Author, Event, SavedMessage, SavedPart, Status, Thread, ToolState};
use octara_chat::store::Store;
use octara_chat::{Chat, Options};
use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

pub const THREAD: u64 = 1;
pub const NOW: u64 = 1_800_000_000;

/// Plain saved data: the same path through Store::load and Message::load as the product.
pub fn history(count: usize) -> (Thread, Vec<SavedMessage>) {
    let messages = (0..count)
        .map(|index| {
            let id = index as u64 + 2;
            let (author, parts) = if index.is_multiple_of(2) {
                (
                    Author::User,
                    vec![SavedPart::Text(format!("Question {index:05}: explain checkpoint {id} with a concrete example. 中文 😀"))],
                )
            } else {
                let paragraphs = (0..index % 5 + 1)
                    .map(|line| format!("Checkpoint {id}, paragraph {line}: retained rows keep their own identity while new text arrives. 中文段落与 emoji 😀 verify wrapping.\n\n"))
                    .collect::<String>();
                (
                    Author::Assistant,
                    vec![
                        SavedPart::Reasoning {
                            text: format!("Reasoning for checkpoint {id}.\n\nCompare the previous result, check the assumptions, and preserve the reader's place.\n\nThis detail can expand independently of the answer."),
                            seconds: 2.0,
                        },
                        SavedPart::Tool {
                            id: format!("lookup-{id}"),
                            name: "lookup_checkpoint".into(),
                            input: format!("{{\n  \"checkpoint\": {id},\n  \"include_details\": true\n}}"),
                            state: ToolState::Done,
                            output: Some(format!("checkpoint: {id}\nstatus: found\nrows: 3\nfirst: retained\nsecond: measured\nthird: complete")),
                        },
                        SavedPart::Text(format!("## Checkpoint {id}\n\n{paragraphs}- Stable message identity\n- Dynamic measured height\n\n```rust\nlet checkpoint = {id};\nassert!(checkpoint > 0);\n```\n\n**Checkpoint {id} complete.**")),
                    ],
                )
            };
            SavedMessage { id, author, parts, status: Status::Done }
        })
        .collect();
    (Thread { id: THREAD, title: format!("Long session {count}"), updated: NOW }, messages)
}

/// Seeds only an explicit empty directory, never the normal application data directory.
pub fn seed(directory: &Path, count: usize) -> Result<Store, Box<dyn std::error::Error>> {
    if !matches!(count, 1_000 | 10_000) {
        return Err("history count must be 1000 or 10000".into());
    }
    std::fs::create_dir_all(directory)?;
    if std::fs::read_dir(directory)?.next().is_some() {
        return Err(format!("long-session store must be empty: {}", directory.display()).into());
    }
    #[derive(Serialize)]
    struct Saved {
        thread: Thread,
        messages: Vec<SavedMessage>,
    }
    let (thread, messages) = history(count);
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(directory.join("1.json"))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &Saved { thread, messages })?;
    writer.flush()?;
    Ok(Store::new(directory))
}

#[derive(Clone)]
struct Reply {
    send: Sender<Event>,
    cancelled: Rc<Cell<bool>>,
}

/// Keeps only the current transport, not another copy of the entire request history.
#[derive(Clone, Default)]
pub struct LongSession {
    reply: Rc<RefCell<Option<Reply>>>,
    chunks: Rc<Cell<usize>>,
    bytes: Rc<Cell<usize>>,
    requests: Rc<Cell<usize>>,
    running: Rc<RefCell<Option<Task>>>,
}

impl LongSession {
    pub fn options(&self, store: Option<Store>) -> Options {
        Options { models: vec![Rc::new(self.clone())], store, clock: Rc::new(|| NOW) }
    }

    /// The unchanged product root, with fixture commands registered in its Chat scope.
    #[component]
    pub fn view(&self, store: Option<Store>) -> View {
        let model = self.clone();
        octara_chat::root(move || {
            let controls = model.clone();
            // Tasks restore their construction scope; commands restore only their window.
            // Capture Chat once here instead of looking context up from an input handler.
            spawn(async move {
                let chat = use_context::<Chat>().expect("initialization runs in the product Chat scope");
                chat.open(THREAD);
                controls.controls(chat);
            });
            model.options(store.clone())
        })
    }

    pub fn sent(&self) -> usize {
        self.chunks.get()
    }

    pub fn requests(&self) -> usize {
        self.requests.get()
    }

    pub fn sent_bytes(&self) -> usize {
        self.bytes.get()
    }

    pub fn pause(&self) {
        let running = self.running.borrow_mut().take();
        if let Some(task) = running {
            task.cancel();
        }
    }

    pub fn emit_chunk(&self) -> bool {
        let reply = self.reply.borrow().clone();
        let Some(reply) = reply.filter(|reply| !reply.cancelled.get()) else { return false };
        let index = self.sent();
        let text = match index % 12 {
            0 => format!("## Live checkpoint {}\n\n", index / 12),
            9 => "```rust\n".into(),
            10 => format!("let streamed_checkpoint = {index};\n"),
            11 => "```\n\n".into(),
            _ => format!(
                "Chunk {index:04}: new text changes only the live answer while the reader explores earlier messages. 中文与 emoji 😀 wrap with the window.\n\n"
            ),
        };
        let bytes = text.len();
        if reply.send.send(Event::Text(text)).is_err() {
            return false;
        }
        self.chunks.set(index + 1);
        self.bytes.set(self.bytes.get() + bytes);
        true
    }

    pub fn finish(&self) -> bool {
        self.pause();
        self.reply.borrow().as_ref().filter(|reply| !reply.cancelled.get()).is_some_and(|reply| reply.send.send(Event::Done).is_ok())
    }

    fn controls(&self, chat: Chat) {
        let opened = chat.clone();
        on_command("long.open", move || opened.open(THREAD));
        let model = self.clone();
        let started = chat.clone();
        on_command("long.start", move || {
            model.pause();
            started.open(THREAD);
            started.stop(THREAD);
            started.draft.set("Continue the long-session validation.".into());
            started.send();
        });
        for count in [1, 10, 100] {
            let model = self.clone();
            on_command(&format!("long.stream.{count}"), move || {
                for _ in 0..count {
                    if !model.emit_chunk() {
                        break;
                    }
                }
            });
        }
        let model = self.clone();
        on_command("long.chunk", move || {
            model.emit_chunk();
        });
        let model = self.clone();
        on_command("long.run", move || {
            model.pause();
            let stream = model.clone();
            let task = spawn(async move {
                for _ in 0..1500 {
                    if !stream.emit_chunk() {
                        break;
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            });
            *model.running.borrow_mut() = Some(task);
        });
        let model = self.clone();
        on_command("long.pause", move || model.pause());
        let model = self.clone();
        on_command("long.finish", move || {
            model.finish();
        });
        let model = self.clone();
        let stopped = chat.clone();
        on_command("long.stop", move || {
            model.pause();
            stopped.stop(THREAD);
        });
        let model = self.clone();
        on_command("long.summary", move || {
            eprintln!(
                "long-session: {}",
                serde_json::json!({
                "messages": chat.messages.keys().len(), "chunks": model.sent(), "bytes": model.sent_bytes(), "reply_requests": model.requests(),
                    "answering": chat.is_answering(THREAD), "current": chat.current.get_untracked(),
                    "draft_lines": chat.draft.with_untracked(|draft| draft.lines().count()),
                })
            );
        });
        let model = self.clone();
        on_cleanup(move || model.pause());
    }
}

impl Model for LongSession {
    fn name(&self) -> &str {
        "Controlled long session"
    }

    fn respond(&self, request: Request, send: Sender<Event>) -> Cancel {
        if request.purpose == Purpose::Title {
            let _ = send.send(Event::Text("Long session".into()));
            let _ = send.send(Event::Done);
            return Box::new(|| {});
        }
        self.pause();
        self.chunks.set(0);
        self.bytes.set(0);
        self.requests.set(self.requests.get() + 1);
        let cancelled = Rc::new(Cell::new(false));
        *self.reply.borrow_mut() = Some(Reply { send, cancelled: cancelled.clone() });
        Box::new(move || cancelled.set(true))
    }
}
