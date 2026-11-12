//! The chat's state and what people do with it: send (or queue while an answer streams), stop,
//! regenerate, edit and resend, attach files, start, rename and delete conversations. One list
//! holds every conversation's messages, so an answer keeps streaming into its conversation while
//! another one is open; the transcript shows the open one's.

use crate::engine::{Cancel, Model, Purpose, Request};
use crate::model::{Attachment, Author, Event, Message, MessageId, Status, Thread, ThreadId};
use crate::store::Store;
use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// A message waiting to be sent while its conversation's answer streams.
#[derive(Clone, PartialEq, Debug)]
pub struct Queued {
    pub id: u64,
    pub thread: ThreadId,
    pub text: String,
    pub files: Vec<Attachment>,
}

/// What the app starts with.
pub struct Options {
    /// The models to pick from (the first is picked).
    pub models: Vec<Rc<dyn Model>>,
    /// Where conversations are kept (`None`: nowhere).
    pub store: Option<Store>,
    /// The wall clock in seconds since the Unix epoch (conversations' dates).
    pub clock: Rc<dyn Fn() -> u64>,
}

impl Default for Options {
    fn default() -> Self {
        let seconds = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Options { models: vec![Rc::new(crate::engine::Scripted::default())], store: None, clock: Rc::new(seconds) }
    }
}

struct Shared {
    models: Vec<Rc<dyn Model>>,
    store: Option<Store>,
    clock: Rc<dyn Fn() -> u64>,
    next_id: Cell<u64>,
    /// What stops each answer streaming, by its message.
    cancels: RefCell<HashMap<MessageId, Cancel>>,
}

/// The chat: its lists and signals, and the flows over them. Cheap to clone; views get it with
/// `use_context`.
#[derive(Clone)]
pub struct Chat {
    pub threads: StateList<ThreadId, Thread>,
    pub messages: StateList<MessageId, Message>,
    pub queue: StateList<u64, Queued>,
    /// Files attached to the message being written.
    pub files: StateList<u64, (u64, Attachment)>,
    /// The open conversation (`None`: a new one, made when the first message is sent).
    pub current: Signal<Option<ThreadId>>,
    pub draft: Signal<String>,
    /// The person's message being edited: sending replaces it and what followed.
    pub editing: Signal<Option<MessageId>>,
    /// The conversation being renamed in the sidebar.
    pub renaming: Signal<Option<ThreadId>>,
    /// The picked model's index.
    pub model: Signal<usize>,
    /// Conversations with an answer streaming.
    pub answering: Signal<Vec<ThreadId>>,
    shared: Rc<Shared>,
}

impl Chat {
    /// The chat's state, as named state of the current component (it survives hot reload),
    /// loaded from the store.
    pub fn new(options: Options) -> Chat {
        let saved = options.store.as_ref().map(Store::load).unwrap_or_default();
        let mut max = 0;
        for (t, ms) in &saved {
            max = max.max(t.id);
            for m in ms {
                max = max.max(m.id);
            }
        }
        let threads_init: Vec<Thread> = saved.iter().map(|(t, _)| t.clone()).collect();
        let messages_init: Vec<Message> =
            saved.into_iter().flat_map(|(t, ms)| ms.into_iter().map(move |m| Message::load(m, t.id))).collect();
        Chat {
            threads: state_list("threads", move || threads_init, |t: &Thread| t.id),
            messages: state_list("messages", move || messages_init, |m: &Message| m.id),
            queue: state_list("queue", Vec::new, |q: &Queued| q.id),
            files: state_list("files", Vec::new, |f: &(u64, Attachment)| f.0),
            current: state("current", || None),
            draft: state("draft", String::new),
            editing: state("editing", || None),
            renaming: state("renaming", || None),
            model: state("model", || 0),
            answering: state("answering", Vec::new),
            shared: Rc::new(Shared {
                models: options.models,
                store: options.store,
                clock: options.clock,
                next_id: Cell::new(max + 1),
                cancels: RefCell::default(),
            }),
        }
    }

    pub fn models(&self) -> Vec<String> {
        self.shared.models.iter().map(|m| m.name().to_owned()).collect()
    }
    /// The wall clock (seconds since the Unix epoch).
    pub fn now(&self) -> u64 {
        (self.shared.clock)()
    }
    fn id(&self) -> u64 {
        let id = self.shared.next_id.get();
        self.shared.next_id.set(id + 1);
        id
    }
    fn model_now(&self) -> Rc<dyn Model> {
        let i = self.model.get_untracked().min(self.shared.models.len() - 1);
        self.shared.models[i].clone()
    }

    /// Whether `thread` has an answer streaming (tracked).
    pub fn is_answering(&self, thread: ThreadId) -> bool {
        self.answering.with(|a| a.contains(&thread))
    }
    /// The open conversation's messages, in order (untracked).
    pub fn thread_messages(&self, thread: ThreadId) -> Vec<Message> {
        self.messages
            .keys()
            .into_iter()
            .filter_map(|k| self.messages.item(&k))
            .map(|m| m.get_untracked())
            .filter(|m| m.thread == thread)
            .collect()
    }

    // ---- conversations ----

    /// Opens a new conversation (made when its first message is sent) and focuses the composer.
    pub fn new_chat(&self) {
        self.current.set(None);
        self.editing.set(None);
        focus("composer");
    }
    pub fn open(&self, thread: ThreadId) {
        if self.current.get_untracked() != Some(thread) {
            self.current.set(Some(thread));
            self.editing.set(None);
        }
    }
    pub fn rename(&self, thread: ThreadId, title: &str) {
        let title = title.trim();
        if !title.is_empty() {
            self.threads.update(&thread, |t| t.title = title.to_owned());
            self.save(thread);
        }
        self.renaming.set(None);
    }
    /// Deletes a conversation, its messages and what it queued (an answer streaming stops).
    pub fn delete(&self, thread: ThreadId) {
        self.stop(thread);
        self.messages.retain(|m| m.thread != thread);
        self.queue.retain(|q| q.thread != thread);
        self.threads.remove(&thread);
        if self.current.get_untracked() == Some(thread) {
            self.current.set(None);
        }
        if let Some(s) = &self.shared.store {
            s.delete(thread);
        }
    }

    // ---- composing ----

    pub fn attach(&self, paths: &[std::path::PathBuf]) {
        for p in paths {
            let name = p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
            let id = self.id();
            self.files.push((id, Attachment { name, path: p.clone() }));
        }
    }
    pub fn detach(&self, id: u64) {
        self.files.remove(&id);
    }

    /// Sends the draft and its files: into a new conversation if none is open; queued while the
    /// conversation's answer streams; replacing the edited message and what followed when editing.
    pub fn send(&self) {
        let text = self.draft.get_untracked().trim().to_owned();
        let files: Vec<Attachment> =
            self.files.keys().into_iter().filter_map(|k| self.files.item(&k)).map(|f| f.get_untracked().1).collect();
        if text.is_empty() && files.is_empty() {
            return;
        }
        self.draft.set(String::new());
        self.files.retain(|_| false);
        let thread = match self.current.get_untracked() {
            Some(t) => t,
            None => {
                let id = self.id();
                self.threads.push(Thread { id, title: "New chat".into(), updated: self.now() });
                self.current.set(Some(id));
                id
            }
        };
        if let Some(edited) = self.editing.get_untracked() {
            self.editing.set(None);
            self.truncate(thread, edited);
        } else if self.is_answering_now(thread) {
            let id = self.id();
            self.queue.push(Queued { id, thread, text, files });
            return;
        }
        self.start(thread, &text, files);
    }

    /// Puts the person's message `id` back in the composer: sending replaces it and what followed.
    pub fn edit(&self, id: MessageId) {
        let Some(m) = self.messages.item(&id).map(|m| m.get_untracked()) else { return };
        self.draft.set(m.text());
        self.editing.set(Some(id));
        focus("composer");
    }
    pub fn cancel_edit(&self) {
        if self.editing.get_untracked().is_some() {
            self.editing.set(None);
            self.draft.set(String::new());
        }
    }

    /// Answers again: the answer `id` and what followed go, and its question is sent again.
    pub fn regenerate(&self, id: MessageId) {
        let Some(answer) = self.messages.item(&id).map(|m| m.get_untracked()) else { return };
        let all = self.thread_messages(answer.thread);
        let Some(question) = all.iter().rev().find(|m| m.id < id && m.author == Author::User).cloned() else { return };
        self.truncate(answer.thread, question.id);
        self.start(answer.thread, &question.text(), question.files());
    }

    // ---- the queue ----

    /// Sends a queued message now: the answer streaming stops.
    pub fn send_now(&self, id: u64) {
        let Some(q) = self.queue.item(&id).map(|q| q.get_untracked()) else { return };
        self.queue.remove(&id);
        self.stop(q.thread);
        self.start(q.thread, &q.text, q.files);
    }
    /// Takes a queued message back into the composer.
    pub fn edit_queued(&self, id: u64) {
        let Some(q) = self.queue.item(&id).map(|q| q.get_untracked()) else { return };
        self.queue.remove(&id);
        self.draft.set(q.text);
        for f in q.files {
            let id = self.id();
            self.files.push((id, f));
        }
        focus("composer");
    }
    pub fn unqueue(&self, id: u64) {
        self.queue.remove(&id);
    }

    // ---- turns ----

    fn is_answering_now(&self, thread: ThreadId) -> bool {
        self.answering.with_untracked(|a| a.contains(&thread))
    }

    /// The messages of `thread` from `from` on go (an answer streaming among them stops).
    fn truncate(&self, thread: ThreadId, from: MessageId) {
        self.stop(thread);
        self.messages.retain(|m| m.thread != thread || m.id < from);
    }

    /// Sends `text` in `thread` and starts the answer.
    fn start(&self, thread: ThreadId, text: &str, files: Vec<Attachment>) {
        let first = !self.thread_messages(thread).iter().any(|m| m.author == Author::User);
        let user = Message::user(self.id(), thread, text, files);
        self.messages.push(user);
        let history: Vec<(Author, String)> = self.thread_messages(thread).iter().map(|m| (m.author, m.text())).collect();
        let answer = self.id();
        self.messages.push(Message::answer(answer, thread));
        self.answering.update(|a| a.push(thread));
        self.touch(thread);
        let (tx, rx) = channel::<Event>();
        {
            let chat = self.clone();
            rx.on(move |ev| chat.on_event(thread, answer, ev));
        }
        let cancel = self.model_now().respond(Request { purpose: Purpose::Reply, history: history.clone() }, tx);
        self.shared.cancels.borrow_mut().insert(answer, cancel);
        if first {
            self.name(thread, history);
        }
    }

    /// Asks the model for the conversation's title.
    fn name(&self, thread: ThreadId, history: Vec<(Author, String)>) {
        let (tx, rx) = channel::<Event>();
        let title = Rc::new(RefCell::new(String::new()));
        let chat = self.clone();
        rx.on(move |ev| match ev {
            Event::Text(s) => title.borrow_mut().push_str(&s),
            Event::Done => {
                let t = title.borrow().trim().to_owned();
                if !t.is_empty() && chat.threads.item(&thread).is_some() {
                    chat.threads.update(&thread, |th| th.title = t);
                    chat.save(thread);
                }
            }
            _ => {}
        });
        drop(self.model_now().respond(Request { purpose: Purpose::Title, history }, tx));
    }

    fn on_event(&self, thread: ThreadId, answer: MessageId, ev: Event) {
        let Some(item) = self.messages.item(&answer) else { return };
        let end = matches!(ev, Event::Done | Event::Failed(_));
        if !item.with_untracked(|m| m.stream(&ev)) {
            let now = octara::task::now();
            self.messages.update(&answer, |m| m.apply(ev, now));
        }
        if end {
            self.ended(thread, answer);
            // The next queued message goes once an answer has come (not after a stop).
            let next =
                self.queue.keys().into_iter().filter_map(|k| self.queue.item(&k)).map(|q| q.get_untracked()).find(|q| q.thread == thread);
            if let Some(q) = next {
                self.queue.remove(&q.id);
                self.start(thread, &q.text, q.files);
            }
        }
    }

    /// Stops the answer streaming in `thread`, if any; what came stays.
    pub fn stop(&self, thread: ThreadId) {
        if !self.is_answering_now(thread) {
            return;
        }
        let streaming = self.thread_messages(thread).into_iter().rev().find(|m| m.status == Status::Streaming).map(|m| m.id);
        if let Some(id) = streaming {
            if let Some(cancel) = self.shared.cancels.borrow_mut().remove(&id) {
                cancel();
            }
            let now = octara::task::now();
            self.messages.update(&id, |m| m.finish(Status::Stopped, now));
            self.ended(thread, id);
        } else {
            self.answering.update(|a| a.retain(|t| *t != thread));
        }
    }

    fn ended(&self, thread: ThreadId, answer: MessageId) {
        self.shared.cancels.borrow_mut().remove(&answer);
        self.answering.update(|a| a.retain(|t| *t != thread));
        self.touch(thread);
        self.save(thread);
    }

    fn touch(&self, thread: ThreadId) {
        let now = self.now();
        self.threads.update(&thread, |t| t.updated = now);
    }

    fn save(&self, thread: ThreadId) {
        let (Some(store), Some(t)) = (&self.shared.store, self.threads.item(&thread)) else { return };
        let messages = self.thread_messages(thread).iter().map(Message::save).collect();
        store.save(t.get_untracked(), messages);
    }
}
