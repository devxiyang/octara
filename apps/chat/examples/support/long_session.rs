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
    autoplay: bool,
    reply: Rc<RefCell<Option<Reply>>>,
    progress: Arc<Progress>,
    requests: Rc<Cell<usize>>,
    running: Rc<Cell<Option<TaskId>>>,
    worker: Rc<RefCell<Option<WallProducer>>>,
    active_workers: Arc<AtomicUsize>,
    payload: Rc<Cell<Payload>>,
    chunk_bytes: Rc<Cell<usize>>,
    rate_hz: Rc<Cell<u32>>,
    chat: Rc<Cell<Option<Chat>>>,
    answer: Rc<Cell<Option<u64>>>,
    late_attempts: Rc<Cell<usize>>,
    late_accepted: Rc<Cell<usize>>,
}

impl LongSession {
    pub fn new(autoplay: bool) -> Self {
        Self { autoplay, ..Self::default() }
    }

    pub fn options(&self, store: Option<Store>) -> Options {
        Options { models: vec![octara_chat::ModelEntry::new("long-session", Rc::new(self.clone()))], store, clock: Rc::new(|| NOW) }
    }

    /// The product view with its model and fixture commands owned by one Chat scope.
    pub fn view(&self, b: &mut BuildCx<'_>, store: Option<Store>) -> View {
        let chat = Chat::new(b, self.options(store));
        self.chat.set(Some(chat));
        b.watch(|_| (), move |w, _, _| chat.open(w, THREAD));
        let model = self.clone();
        b.on_cleanup(move |_| {
            model.worker.borrow_mut().take();
            model.chat.set(None);
        });
        self.controls(col().size(rel(1.0)).child(octara_chat::view(b, chat)), chat).into_view()
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

    pub fn start_producer(&self, w: &mut WriteCx<'_>) {
        self.pause(w);
        if self.rate_hz.get() != 0 {
            self.start_wall_producer().expect("start the bounded fixture producer");
            return;
        }
        self.start_clocked(w, w.scope(), 1500, false);
    }

    fn start_clocked(&self, w: &mut WriteCx<'_>, owner: ScopeId, chunks: usize, auto_finish: bool) {
        let stream = self.clone();
        let task = w.spawn_in(owner, "long-session stream", move |task| async move {
            for _ in 0..chunks {
                if !stream.emit_chunk() {
                    return;
                }
                task.sleep(Duration::from_millis(20)).await;
            }
            if auto_finish && let Some(reply) = stream.reply.borrow().as_ref() {
                let _ = reply.send.send(Event::Done);
            }
        });
        self.running.set(Some(task));
    }

    /// Explicit rate modes use a wall-clock worker, independent of the UI virtual clock.
    /// The worker never owns a UI object and pause never joins it on the UI thread.
    pub fn start_wall_producer(&self) -> Result<(), String> {
        self.start_wall_producer_for(Duration::from_secs(30))
    }

    fn start_wall_producer_for(&self, duration: Duration) -> Result<(), String> {
        self.worker.borrow_mut().take();
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
    pub fn readback(&self, w: &mut WriteCx<'_>) -> Option<Readback> {
        let chat = self.chat.get()?;
        let message = self.answer.get().and_then(|id| w.get_row(chat.messages, &id));
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
            draft: w.get(chat.draft),
            answering: w.read(|r| chat.is_answering(r, THREAD)),
            sent_chunks: self.sent(),
            sent_bytes: self.sent_bytes(),
            late_attempts: self.late_attempts.get(),
            late_accepted: self.late_accepted.get(),
        })
    }

    pub fn pause(&self, w: &mut WriteCx<'_>) {
        drop(self.worker.borrow_mut().take());
        if let Some(task) = self.running.take() {
            w.cancel_task(task);
        }
    }

    pub fn emit_chunk(&self) -> bool {
        let reply = self.reply.borrow().clone();
        reply.is_some_and(|reply| self.progress.send(&reply, self.payload.get(), self.chunk_bytes.get(), None))
    }

    pub fn finish(&self, w: &mut WriteCx<'_>) -> bool {
        self.pause(w);
        self.reply
            .borrow()
            .as_ref()
            .filter(|reply| !reply.cancelled.load(Ordering::Acquire))
            .is_some_and(|reply| reply.send.send(Event::Done).is_ok())
    }

    fn controls(&self, mut content: El, chat: Chat) -> El {
        content = content.on_command("long.read", move |w| {
            let ids = w.keys(chat.messages);
            if let Some(id) = ids.get(ids.len().saturating_sub(20)) {
                w.reveal_row("transcript", *id);
            }
        });
        for payload in [Payload::Mixed, Payload::Paragraph, Payload::Code] {
            let model = self.clone();
            content = content.on_command(format!("long.scene.{}", payload.name()), move |_| {
                model.set_payload(payload, 1024).expect("fixture payload size is bounded");
            });
        }
        for bytes in [64, 256, 1024, 4096, 16384] {
            let model = self.clone();
            content = content.on_command(format!("long.bytes.{bytes}"), move |_| {
                model.set_payload(model.payload.get(), bytes).expect("fixture payload size is bounded");
            });
        }
        for count in [32, 256] {
            let model = self.clone();
            content = content.on_command(format!("long.burst.{count}"), move |_| {
                model.emit_burst(count);
            });
        }
        for rate in [50, 200, 1000] {
            let model = self.clone();
            content =
                content.on_command(format!("long.rate.{rate}"), move |_| model.set_rate(rate).expect("fixture producer rate is bounded"));
        }
        let model = self.clone();
        content = content.on_command("long.late", move |_| {
            model.attempt_late();
        });
        let opened = chat;
        content = content.on_command("long.open", move |w| opened.open(w, THREAD));
        let model = self.clone();
        let started = chat;
        content = content.on_command("long.start", move |w| {
            model.pause(w);
            started.open(w, THREAD);
            started.stop(w, THREAD);
            w.set(started.draft, "Continue the long-session validation.".into());
            started.send(w);
        });
        for count in [1, 10, 100] {
            let model = self.clone();
            content = content.on_command(format!("long.stream.{count}"), move |_| {
                for _ in 0..count {
                    if !model.emit_chunk() {
                        break;
                    }
                }
            });
        }
        let model = self.clone();
        content = content.on_command("long.chunk", move |_| {
            model.emit_chunk();
        });
        let model = self.clone();
        content = content.on_command("long.run", move |w| model.start_producer(w));
        let model = self.clone();
        content = content.on_command("long.run.short", move |_| {
            model.start_wall_producer_for(Duration::from_secs(5)).expect("start the short bounded fixture producer");
        });
        let model = self.clone();
        content = content.on_command("long.pause", move |w| model.pause(w));
        let model = self.clone();
        content = content.on_command("long.finish", move |w| {
            model.finish(w);
        });
        let model = self.clone();
        let stopped = chat;
        content = content.on_command("long.stop", move |w| {
            model.pause(w);
            stopped.stop(w, THREAD);
        });
        let model = self.clone();
        content = content.on_command("long.summary", move |w| {
            eprintln!(
                "long-session: {}",
                serde_json::json!({"messages": w.len(chat.messages),
                "chunks": model.sent(), "bytes": model.sent_bytes(), "reply_requests": model.requests(),
                "current": w.get(chat.current), "readback": model.readback(w), "workers": model.workers(),
                "producer_rate_hz":model.rate_hz.get(), "producer_clock":if model.rate_hz.get()==0 {"virtual_clock"} else {"wall_clock_thread"}})
            );
        });
        content
    }
}

impl Model for LongSession {
    fn name(&self) -> &str {
        "Controlled long session"
    }

    fn respond(&self, w: &mut WriteCx<'_>, owner: ScopeId, request: Request, send: Sender<Event>) -> Cancel {
        if request.purpose == Purpose::Title {
            let _ = send.send(Event::Text("Long session".into()));
            let _ = send.send(Event::Done);
            return Box::new(|| {});
        }
        self.pause(w);
        let _sending = self.progress.sending.lock().expect("retire the previous fixture producer");
        self.progress.chunks.store(0, Ordering::Release);
        self.progress.bytes.store(0, Ordering::Release);
        self.requests.set(self.requests.get() + 1);
        self.answer.set(self.chat.get().and_then(|chat| w.keys(chat.messages).last().copied()));
        self.late_attempts.set(0);
        self.late_accepted.set(0);
        let cancelled = Arc::new(AtomicBool::new(false));
        *self.reply.borrow_mut() = Some(Reply { send, cancelled: cancelled.clone() });
        if self.autoplay {
            self.start_clocked(w, owner, 300, true);
        }
        Box::new(move || cancelled.store(true, Ordering::Release))
    }
}
