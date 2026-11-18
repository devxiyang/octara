//! Deterministic history and a real Model channel for long-session UI qualification.

use octara::prelude::*;
use octara::reactive::untrack;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::{Author, Event, SavedMessage, SavedPart, Status, Thread, ToolState};
use octara_chat::store::Store;
use octara_chat::{Chat, Options};
use serde::Serialize;
use std::cell::{Cell, RefCell};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const THREAD: u64 = 1;
pub const NOW: u64 = 1_800_000_000;
pub const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_REPLY_CHUNKS: usize = 8192;

#[derive(Clone, Copy, Default, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Payload {
    #[default]
    Mixed,
    Paragraph,
    Code,
}

#[derive(Clone, Debug, Serialize)]
pub struct Readback {
    pub answer: Option<u64>,
    pub status: Option<Status>,
    pub consumed_text_bytes: usize,
    pub draft: String,
    pub answering: bool,
    pub sent_chunks: usize,
    pub sent_bytes: usize,
    pub late_attempts: usize,
    pub late_accepted: usize,
}

#[derive(Serialize)]
struct Saved {
    thread: Thread,
    messages: Vec<SavedMessage>,
}

impl Payload {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mixed => "mixed",
            Self::Paragraph => "paragraph",
            Self::Code => "code",
        }
    }

    fn text(self, index: usize, size: usize) -> String {
        match self {
            Self::Paragraph => {
                let sentence = format!("word {index:06} stays in one paragraph while the reader types and scrolls. ");
                sentence.repeat(size.div_ceil(sentence.len()))[..size].to_owned()
            }
            Self::Code => {
                let line = format!("let item_{index:06} = {index}; // an incrementally streamed code line\n");
                let prefix = if index == 0 { "```rust\n" } else { "" };
                let mut text = format!("{prefix}{}", line.repeat(size.div_ceil(line.len())));
                text.truncate(size);
                text
            }
            Self::Mixed => match index % 12 {
                0 => format!("## Live checkpoint {}\n\n", index / 12),
                9 => "```rust\n".into(),
                10 => format!("let streamed_checkpoint = {index};\n"),
                11 => "```\n\n".into(),
                _ => format!(
                    "Chunk {index:04}: new text changes only the live answer while the reader explores earlier messages. 中文与 emoji 😀 wrap with the window.\n\n"
                ),
            },
        }
    }
}

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
            let next_part = parts.len() as u64;
            let parts = parts.into_iter().enumerate().map(|(i, part)| (i as u64, part)).collect();
            SavedMessage { id, author, parts, status: Status::Done, next_part }
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
    let (thread, messages) = history(count);
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(directory.join("1.v2.json"))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, &Saved { thread, messages })?;
    writer.flush()?;
    Ok(Store::new(directory))
}

#[derive(Clone)]
struct Reply {
    send: Sender<Event>,
    cancelled: Arc<AtomicBool>,
}

struct WallProducer {
    cancelled: Arc<AtomicBool>,
    thread: std::thread::Thread,
}

impl Drop for WallProducer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

#[derive(Default)]
struct Progress {
    chunks: AtomicUsize,
    bytes: AtomicUsize,
    sending: Mutex<()>,
}

impl Progress {
    fn send(&self, reply: &Reply, payload: Payload, chunk_bytes: usize, producer: Option<&AtomicBool>) -> bool {
        let _sending = self.sending.lock().expect("fixture send ordering");
        if reply.cancelled.load(Ordering::Acquire) || producer.is_some_and(|cancelled| cancelled.load(Ordering::Acquire)) {
            return false;
        }
        let index = self.chunks.load(Ordering::Acquire);
        let before = self.bytes.load(Ordering::Acquire);
        if index >= MAX_REPLY_CHUNKS {
            return false;
        }
        let text = payload.text(index, chunk_bytes.max(16));
        let bytes = text.len();
        if before + bytes > MAX_REPLY_BYTES || reply.send.send(Event::Text(text)).is_err() {
            return false;
        }
        self.bytes.store(before + bytes, Ordering::Release);
        self.chunks.store(index + 1, Ordering::Release);
        true
    }
}

/// Keeps only the current transport, not another copy of the entire request history.
#[derive(Clone, Default)]
pub struct LongSession {
    reply: Rc<RefCell<Option<Reply>>>,
    progress: Arc<Progress>,
    requests: Rc<Cell<usize>>,
    running: Rc<RefCell<Option<Task>>>,
    worker: Rc<RefCell<Option<WallProducer>>>,
    active_workers: Arc<AtomicUsize>,
    payload: Rc<Cell<Payload>>,
    chunk_bytes: Rc<Cell<usize>>,
    rate_hz: Rc<Cell<u32>>,
    chat: Rc<RefCell<Weak<Chat>>>,
    answer: Rc<Cell<Option<u64>>>,
    late_attempts: Rc<Cell<usize>>,
    late_accepted: Rc<Cell<usize>>,
}

impl LongSession {
    pub fn options(&self, store: Option<Store>) -> Options {
        Options { models: vec![octara_chat::ModelEntry::new("long-session", Rc::new(self.clone()))], store, clock: Rc::new(|| NOW) }
    }

    /// The product view, with its model and fixture commands owned by one Chat scope.
    #[ui]
    pub fn view(&self, store: Option<Store>) -> View {
        let model = self.clone();
        scope("Chat", move || {
            let chat = Rc::new(Chat::new(model.options(store)));
            let controls = model.clone();
            let initialized = chat.clone();
            // Opening the seeded thread is a handler-time change after the product is built.
            spawn(async move {
                *controls.chat.borrow_mut() = Rc::downgrade(&initialized);
                initialized.open(THREAD);
                controls.controls(initialized);
            });
            octara_chat::view(&chat)
        })
    }

    pub fn sent(&self) -> usize {
        self.progress.chunks.load(Ordering::Acquire)
    }

    pub fn requests(&self) -> usize {
        self.requests.get()
    }

    pub fn sent_bytes(&self) -> usize {
        self.progress.bytes.load(Ordering::Acquire)
    }

    pub fn set_payload(&self, payload: Payload, chunk_bytes: usize) -> Result<(), String> {
        if !(16..=16 * 1024).contains(&chunk_bytes) {
            return Err("payload chunks must contain 16..=16384 bytes".into());
        }
        self.payload.set(payload);
        self.chunk_bytes.set(chunk_bytes);
        Ok(())
    }

    pub fn emit_burst(&self, count: usize) -> usize {
        (0..count.min(256)).take_while(|_| self.emit_chunk()).count()
    }

    pub fn set_rate(&self, hz: u32) -> Result<(), String> {
        if !(1..=1000).contains(&hz) {
            return Err("producer rate must be 1..=1000 chunks per second".into());
        }
        self.rate_hz.set(hz);
        Ok(())
    }

    pub fn start_producer(&self) {
        self.pause();
        if self.rate_hz.get() != 0 {
            self.start_wall_producer().expect("start the bounded fixture producer");
            return;
        }
        let stream = self.clone();
        let task = spawn(async move {
            for _ in 0..1500 {
                if !stream.emit_chunk() {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        });
        *self.running.borrow_mut() = Some(task);
    }

    /// Explicit rate modes use a wall-clock worker, independent of the UI virtual clock.
    /// The worker never owns a UI object and pause never joins it on the UI thread.
    pub fn start_wall_producer(&self) -> Result<(), String> {
        self.start_wall_producer_for(Duration::from_secs(30))
    }

    fn start_wall_producer_for(&self, duration: Duration) -> Result<(), String> {
        self.pause();
        let reply = self.reply.borrow().clone().ok_or("start a reply before its producer")?;
        let payload = self.payload.get();
        let chunk_bytes = self.chunk_bytes.get();
        let hz = self.rate_hz.get().max(1);
        let progress = self.progress.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let active = self.active_workers.clone();
        active.fetch_add(1, Ordering::AcqRel);
        let spawned = std::thread::Builder::new().name("chat-fixture-producer".into()).spawn(move || {
            struct Active(Arc<AtomicUsize>);
            impl Drop for Active {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::AcqRel);
                }
            }
            let _active = Active(active);
            let period = Duration::from_secs_f64(1.0 / f64::from(hz));
            let began = Instant::now();
            let mut next = began;
            while began.elapsed() < duration {
                if !progress.send(&reply, payload, chunk_bytes, Some(&stop)) {
                    return;
                }
                next += period;
                let now = Instant::now();
                if next < now {
                    next = now + period;
                }
                while !stop.load(Ordering::Acquire) && !reply.cancelled.load(Ordering::Acquire) && Instant::now() < next {
                    std::thread::park_timeout(next.saturating_duration_since(Instant::now()));
                }
            }
        });
        match spawned {
            Ok(worker) => {
                *self.worker.borrow_mut() = Some(WallProducer { cancelled, thread: worker.thread().clone() });
                // The bounded worker owns its lifetime; cancellation wakes it without a UI join.
                drop(worker);
                Ok(())
            }
            Err(error) => {
                self.active_workers.fetch_sub(1, Ordering::AcqRel);
                Err(error.to_string())
            }
        }
    }

    pub fn workers(&self) -> usize {
        self.active_workers.load(Ordering::Acquire)
    }

    /// A transport deliberately attempts both a stale payload and a terminal event.
    /// These attempts are separate from the successful normal producer counters.
    pub fn attempt_late(&self) -> (bool, bool) {
        let Some(reply) = self.reply.borrow().clone() else { return (false, false) };
        let text = reply.send.send(Event::Text(" LATE_STREAM_PAYLOAD".repeat(64))).is_ok();
        let done = reply.send.send(Event::Done).is_ok();
        self.late_attempts.set(self.late_attempts.get() + 2);
        self.late_accepted.set(self.late_accepted.get() + usize::from(text) + usize::from(done));
        (text, done)
    }

    /// Read in the app runtime. The weak reference never makes the Model own Chat.
    pub fn readback(&self) -> Option<Readback> {
        let chat = self.chat.borrow().upgrade()?;
        let message = self.answer.get().and_then(|id| chat.messages.item(&id)).map(|item| untrack(|| item.get()));
        let consumed_text_bytes = message.as_ref().map_or(0, |message| {
            message
                .parts
                .iter()
                .map(|(_, part)| match part {
                    octara_chat::model::Part::Text(text) => text.len(),
                    _ => 0,
                })
                .sum()
        });
        Some(Readback {
            answer: self.answer.get(),
            status: message.map(|message| message.status),
            consumed_text_bytes,
            draft: untrack(|| chat.draft.get()),
            answering: chat.is_answering(THREAD),
            sent_chunks: self.sent(),
            sent_bytes: self.sent_bytes(),
            late_attempts: self.late_attempts.get(),
            late_accepted: self.late_accepted.get(),
        })
    }

    pub fn pause(&self) {
        drop(self.worker.borrow_mut().take());
        let running = self.running.borrow_mut().take();
        if let Some(task) = running {
            task.cancel();
        }
    }

    pub fn emit_chunk(&self) -> bool {
        let reply = self.reply.borrow().clone();
        reply.is_some_and(|reply| self.progress.send(&reply, self.payload.get(), self.chunk_bytes.get(), None))
    }

    pub fn finish(&self) -> bool {
        self.pause();
        self.reply
            .borrow()
            .as_ref()
            .filter(|reply| !reply.cancelled.load(Ordering::Acquire))
            .is_some_and(|reply| reply.send.send(Event::Done).is_ok())
    }

    fn controls(&self, chat: Rc<Chat>) {
        let window_id = current_window();
        on_command("long.read", move || {
            if let Some(window) = App::current().window(window_id) {
                let offset = {
                    let kernel = window.kernel();
                    kernel.find("transcript").map(|node| (kernel.scroll_offset(node).1 - 1200.0).max(0.0))
                };
                if let Some(offset) = offset {
                    scroll_to("transcript", 0.0, offset);
                }
            }
        });
        for payload in [Payload::Mixed, Payload::Paragraph, Payload::Code] {
            let model = self.clone();
            on_command(&format!("long.scene.{}", payload.name()), move || {
                model.set_payload(payload, 1024).expect("fixture payload size is bounded");
            });
        }
        for bytes in [64, 256, 1024, 4096, 16384] {
            let model = self.clone();
            on_command(&format!("long.bytes.{bytes}"), move || {
                model.set_payload(model.payload.get(), bytes).expect("fixture payload size is bounded");
            });
        }
        for count in [32, 256] {
            let model = self.clone();
            on_command(&format!("long.burst.{count}"), move || {
                model.emit_burst(count);
            });
        }
        for rate in [50, 200, 1000] {
            let model = self.clone();
            on_command(&format!("long.rate.{rate}"), move || model.set_rate(rate).expect("fixture producer rate is bounded"));
        }
        let model = self.clone();
        on_command("long.late", move || {
            model.attempt_late();
        });
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
        on_command("long.run", move || model.start_producer());
        let model = self.clone();
        on_command("long.run.short", move || {
            model.start_wall_producer_for(Duration::from_secs(5)).expect("start the short bounded fixture producer");
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
                serde_json::json!({"messages": chat.messages.keys().len(),
                "chunks": model.sent(), "bytes": model.sent_bytes(), "reply_requests": model.requests(),
                "current": untrack(|| chat.current.get()), "readback": model.readback(), "workers": model.workers(),
                "producer_rate_hz":model.rate_hz.get(), "producer_clock":if model.rate_hz.get()==0 {"legacy_ui_clock"} else {"wall_clock_thread"}})
            );
        });
        let model = self.clone();
        on_cleanup(move || {
            model.pause();
            *model.chat.borrow_mut() = Weak::new();
        });
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
        let _sending = self.progress.sending.lock().expect("retire the previous fixture producer");
        self.progress.chunks.store(0, Ordering::Release);
        self.progress.bytes.store(0, Ordering::Release);
        self.requests.set(self.requests.get() + 1);
        self.answer.set(self.chat.borrow().upgrade().and_then(|chat| chat.messages.keys().last().copied()));
        self.late_attempts.set(0);
        self.late_accepted.set(0);
        let cancelled = Arc::new(AtomicBool::new(false));
        *self.reply.borrow_mut() = Some(Reply { send, cancelled: cancelled.clone() });
        Box::new(move || cancelled.store(true, Ordering::Release))
    }
}
