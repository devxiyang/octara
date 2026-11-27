//! The chat's state and what people do with it: send (or queue while an answer streams), stop,
//! regenerate, edit and resend, attach files, start, rename and delete conversations. One list
//! holds every conversation's messages, so an answer keeps streaming into its conversation while
//! another one is open; the transcript shows the open one's.

use crate::engine::{Cancel, Model, Purpose, Request};
use crate::model::{Attachment, Author, Event, Message, MessageId, Status, Thread, ThreadId};
use crate::settings::{Generation, Preferences};
use crate::store::Store;
use octara::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

/// One configured model. Its application-owned ID stays stable across label changes and
/// reordering; it is independent of the provider's HTTP model name. IDs must be unique,
/// nonempty and contain no credentials. Display names may be shared by multiple entries.
#[derive(Clone)]
pub struct ModelEntry {
    pub id: String,
    pub model: Rc<dyn Model>,
}

impl ModelEntry {
    pub fn new(id: impl Into<String>, model: Rc<dyn Model>) -> Self {
        Self { id: id.into(), model }
    }
}

/// A message waiting to be sent while its conversation's answer streams.
#[derive(Clone, PartialEq, Debug)]
pub struct Queued {
    pub id: u64,
    pub thread: ThreadId,
    pub text: String,
    pub files: Vec<Attachment>,
    /// The model selected when Send was pressed, including when this message waits in a queue.
    pub model_id: String,
    pub generation: Generation,
}

/// What the app starts with.
#[derive(Clone)]
pub struct Options {
    /// Models with unique stable IDs (the first is initially picked). An empty list is valid;
    /// sending then preserves the draft until a model becomes available.
    pub models: Vec<ModelEntry>,
    /// Where conversations are kept (`None`: nowhere).
    pub store: Option<Store>,
    /// Clock for conversation dates, in seconds since the Unix epoch.
    pub clock: Rc<dyn Fn() -> u64>,
}

impl Default for Options {
    fn default() -> Self {
        let seconds = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Options {
            models: vec![ModelEntry::new("scripted", Rc::new(crate::engine::Scripted::default()))],
            store: None,
            clock: Rc::new(seconds),
        }
    }
}

/// One current request per conversation and purpose. The answer ID identifies its owner.
#[derive(Clone)]
struct Running {
    id: MessageId,
    receiver: TaskId,
    cancel: Option<Rc<dyn Fn()>>,
}

#[derive(Clone, Default)]
struct Requests {
    answers: HashMap<ThreadId, Running>,
    titles: HashMap<ThreadId, Running>,
}

/// Copyable domain handles. Construction, reads and actions use the caller's capability.
#[derive(Clone, Copy)]
pub struct Chat {
    pub threads: StateList<ThreadId, Thread>,
    pub messages: StateList<MessageId, Message>,
    pub queue: StateList<u64, Queued>,
    pub files: StateList<u64, Attachment>,
    pub current: Signal<Option<ThreadId>>,
    pub draft: Signal<String>,
    pub editing: Signal<Option<MessageId>>,
    pub renaming: Signal<Option<ThreadId>>,
    pub model: Signal<Option<String>>,
    pub generation: Signal<Generation>,
    pub answering: Signal<Vec<ThreadId>>,
    history: Groups<ThreadId, MessageId, Message>,
    scope: ScopeId,
    options: Signal<Options>,
    next_id: Signal<u64>,
    requests: Signal<Requests>,
}

impl Chat {
    /// Loads saved messages and settings and owns requests for this build scope.
    pub fn new(b: &mut BuildCx<'_>, options: Options) -> Self {
        let mut ids = HashSet::new();
        assert!(
            options.models.iter().all(|entry| !entry.id.trim().is_empty() && ids.insert(&entry.id)),
            "model IDs must be nonempty and unique"
        );
        let preferences = options.store.as_ref().and_then(Store::load_preferences).unwrap_or_default();
        let initial_model = preferences.model_id.or_else(|| options.models.first().map(|entry| entry.id.clone()));
        let saved = options.store.as_ref().map(Store::load).unwrap_or_default();
        let mut threads = Vec::with_capacity(saved.len());
        let mut messages = Vec::new();
        let mut max = 0;
        for (thread, history) in saved {
            let id = thread.id;
            max = max.max(id);
            threads.push((id, thread));
            for message in history {
                max = max.max(message.id);
                messages.push((message.id, Message::load(message, id)));
            }
        }
        let messages = b.list_from(messages);
        let chat = Self {
            threads: b.list_from(threads),
            messages,
            queue: b.list(),
            files: b.list(),
            current: b.signal(None),
            draft: b.signal(String::new()),
            editing: b.signal(None),
            renaming: b.signal(None),
            model: b.signal(initial_model),
            generation: b.signal(preferences.generation),
            answering: b.signal(Vec::new()),
            history: messages.read_only().group_by(b, |message| message.thread),
            scope: b.scope(),
            options: b.signal(options),
            next_id: b.signal(max.checked_add(1).expect("chat IDs exhausted")),
            requests: b.signal(Requests::default()),
        };
        b.on_cleanup(move |c| {
            let requests = c.sample(chat.requests);
            for request in requests.answers.into_values().chain(requests.titles.into_values()) {
                if let Some(cancel) = request.cancel {
                    cancel();
                }
            }
        });
        chat
    }

    /// Publish settings after saving them. A failed write keeps the active configuration.
    pub fn configure(&self, w: &mut WriteCx<'_>, model_id: Option<String>, generation: Generation) -> Result<(), String> {
        let generation = generation.validate()?;
        let options = w.get(self.options);
        if !model_id.as_deref().is_some_and(|id| model_by_id(&options.models, id).is_some()) {
            return Err("Choose an available model.".into());
        }
        let preferences = Preferences { model_id: model_id.clone(), generation };
        if let Some(store) = &options.store {
            store.save_preferences(&preferences)?;
        }
        w.set(self.model, model_id);
        w.set(self.generation, generation);
        Ok(())
    }

    pub(crate) fn model_choices(&self, b: &mut BuildCx<'_>) -> Vec<(String, String)> {
        b.sample(self.options).models.into_iter().map(|m| (m.id, m.model.name().to_owned())).collect()
    }

    pub(crate) fn has_store(&self, b: &mut BuildCx<'_>) -> bool {
        b.sample(self.options).store.is_some()
    }

    pub(crate) fn now(&self, b: &mut BuildCx<'_>) -> u64 {
        (b.sample(self.options).clock)()
    }

    pub fn active_thread(&self, r: &mut ReadCx<'_>) -> Option<ThreadId> {
        r.get(self.current)
    }

    pub fn has_model(&self, r: &mut ReadCx<'_>) -> bool {
        r.with2(self.model, self.options, |id, options| id.as_deref().is_some_and(|id| model_by_id(&options.models, id).is_some()))
    }

    pub fn is_answering(&self, r: &mut ReadCx<'_>, thread: ThreadId) -> bool {
        r.with(self.answering, |a| a.contains(&thread))
    }

    pub fn can_send(&self, r: &mut ReadCx<'_>) -> bool {
        (!r.with(self.draft, |draft| draft.trim().is_empty()) || r.len(self.files) != 0)
            && self.has_model(r)
            && r.with(self.generation, |g| g.validate().is_ok())
    }

    pub fn thread_messages(&self, r: &mut ReadCx<'_>, thread: ThreadId) -> Vec<Message> {
        self.project_messages(r, thread, Message::clone)
    }

    fn thread_message_ids(&self, r: &mut ReadCx<'_>, thread: ThreadId) -> Vec<MessageId> {
        let Some(group) = r.get_row(self.history.read_only(), &thread) else { return Vec::new() };
        // Members are contiguous after their group header, even when requests interleave.
        let rows = self.history.flatten();
        let first = r.rank(rows, &FlatKey::Header(thread)).expect("indexed thread header") + 1;
        (first..first + group.size)
            .map(|rank| match r.row(rows, rank) {
                Some(FlatKey::Item(id)) => id,
                _ => unreachable!("indexed thread member"),
            })
            .collect()
    }

    fn project_messages<R>(&self, r: &mut ReadCx<'_>, thread: ThreadId, project: impl Fn(&Message) -> R) -> Vec<R> {
        self.thread_message_ids(r, thread).iter().filter_map(|id| r.with_row(self.messages, id, |message| message.map(&project))).collect()
    }

    pub fn new_chat(&self, w: &mut WriteCx<'_>) {
        w.set(self.current, None);
        w.set(self.editing, None);
        w.focus("composer");
    }

    pub fn open(&self, w: &mut WriteCx<'_>, thread: ThreadId) {
        if w.set_if_changed(self.current, Some(thread)) {
            w.set(self.editing, None);
        }
    }

    pub fn rename(&self, w: &mut WriteCx<'_>, thread: ThreadId, title: &str) {
        let title = title.trim();
        if !title.is_empty() {
            self.stop_title(w, thread);
            w.update_row(self.threads, &thread, |t| t.title = title.to_owned());
            self.save(w, thread);
        }
        w.set(self.renaming, None);
    }

    pub fn delete(&self, w: &mut WriteCx<'_>, thread: ThreadId) {
        self.stop(w, thread);
        self.stop_title(w, thread);
        self.remove_messages(w, thread, 0);
        for id in w.keys(self.queue) {
            if w.with_row(self.queue, &id, |q| q.is_some_and(|q| q.thread == thread)) {
                w.remove_row(self.queue, &id);
            }
        }
        w.remove_row(self.threads, &thread);
        if w.get(self.current) == Some(thread) {
            w.set(self.current, None);
            w.set(self.editing, None);
        }
        if let Some(store) = w.with(self.options, |o| o.store.clone()) {
            store.delete(thread);
        }
    }

    pub fn attach(&self, w: &mut WriteCx<'_>, paths: &[PathBuf]) {
        for path in paths {
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            self.attach_file(w, name, Some(path.clone()));
        }
    }

    pub(crate) fn attach_dropped(&self, w: &mut WriteCx<'_>, files: &[DroppedFile]) {
        for file in files {
            match file {
                DroppedFile::Path(path) => self.attach(w, std::slice::from_ref(path)),
                DroppedFile::Name(name) => self.attach_file(w, name.clone(), None),
            }
        }
    }

    pub(crate) fn attach_file(&self, w: &mut WriteCx<'_>, name: String, path: Option<PathBuf>) {
        let id = self.id(w);
        w.push_row(self.files, id, Attachment { name, path });
    }

    pub fn detach(&self, w: &mut WriteCx<'_>, id: u64) {
        w.remove_row(self.files, &id);
    }

    pub fn send(&self, w: &mut WriteCx<'_>) {
        if !w.read(|r| self.can_send(r)) {
            return;
        }
        let Some(model) = self.selected_model(w) else { return };
        let Ok(generation) = w.get(self.generation).validate() else { return };
        let text = w.with(self.draft, |draft| draft.trim().to_owned());
        let files = w.keys(self.files).into_iter().filter_map(|id| w.remove_row(self.files, &id)).collect();
        w.set(self.draft, String::new());
        let thread = w.get(self.current).unwrap_or_else(|| {
            let id = self.id(w);
            let updated = w.with(self.options, |o| (o.clock)());
            w.push_row(self.threads, id, Thread { id, title: "New chat".into(), updated });
            w.set(self.current, Some(id));
            id
        });
        if let Some(edited) = w.get(self.editing) {
            w.set(self.editing, None);
            self.truncate(w, thread, edited);
        } else if w.with(self.answering, |a| a.contains(&thread)) {
            let id = self.id(w);
            w.push_row(self.queue, id, Queued { id, thread, text, files, model_id: model.id, generation });
            return;
        }
        self.start(w, thread, &text, files, model.model, generation);
    }

    pub fn edit(&self, w: &mut WriteCx<'_>, id: MessageId) {
        let Some(message) = w.get_row(self.messages, &id).filter(|m| m.author == Author::User) else { return };
        w.set(self.draft, message.text());
        w.set(self.editing, Some(id));
        for file_id in w.keys(self.files) {
            w.remove_row(self.files, &file_id);
        }
        for file in message.files() {
            let file_id = self.id(w);
            w.push_row(self.files, file_id, file);
        }
        w.focus("composer");
    }

    pub fn cancel_edit(&self, w: &mut WriteCx<'_>) {
        if w.set_if_changed(self.editing, None) {
            w.set(self.draft, String::new());
            for id in w.keys(self.files) {
                w.remove_row(self.files, &id);
            }
        }
    }

    pub fn regenerate(&self, w: &mut WriteCx<'_>, id: MessageId) {
        let Some(model) = self.selected_model(w) else { return };
        let Ok(generation) = w.get(self.generation).validate() else { return };
        let Some(thread) = w.with_row(self.messages, &id, |m| m.filter(|m| m.author == Author::Assistant).map(|m| m.thread)) else {
            return;
        };
        let question = w.read(|r| {
            self.thread_message_ids(r, thread)
                .iter()
                .rev()
                .filter(|key| **key < id)
                .find_map(|key| r.with_row(self.messages, key, |message| message.filter(|m| m.author == Author::User).cloned()))
        });
        let Some(question) = question else { return };
        self.truncate(w, thread, question.id);
        self.start(w, thread, &question.text(), question.files(), model.model, generation);
    }

    pub fn send_now(&self, w: &mut WriteCx<'_>, id: u64) {
        let Some(queued) = w.get_row(self.queue, &id) else { return };
        let Some(model) = self.model_by_id(w, &queued.model_id) else { return };
        w.remove_row(self.queue, &id);
        self.stop(w, queued.thread);
        self.start(w, queued.thread, &queued.text, queued.files, model.model, queued.generation);
    }

    pub fn edit_queued(&self, w: &mut WriteCx<'_>, id: u64) {
        let Some(queued) = w.remove_row(self.queue, &id) else { return };
        w.set(self.model, Some(queued.model_id));
        w.set(self.generation, queued.generation);
        w.set(self.draft, queued.text);
        for file in queued.files {
            let id = self.id(w);
            w.push_row(self.files, id, file);
        }
        w.focus("composer");
    }

    pub fn unqueue(&self, w: &mut WriteCx<'_>, id: u64) {
        w.remove_row(self.queue, &id);
    }

    fn id(&self, w: &mut WriteCx<'_>) -> u64 {
        w.update(self.next_id, |next| {
            let id = *next;
            *next = id.checked_add(1).expect("chat IDs exhausted");
            id
        })
    }

    fn selected_model(&self, w: &mut WriteCx<'_>) -> Option<ModelEntry> {
        let id = w.get(self.model)?;
        self.model_by_id(w, &id)
    }

    fn model_by_id(&self, w: &mut WriteCx<'_>, id: &str) -> Option<ModelEntry> {
        w.with(self.options, |o| model_by_id(&o.models, id).cloned())
    }

    fn remove_messages(&self, w: &mut WriteCx<'_>, thread: ThreadId, from: MessageId) {
        let ids = w.read(|r| self.thread_message_ids(r, thread));
        for id in ids.into_iter().filter(|id| *id >= from) {
            w.remove_row(self.messages, &id);
        }
    }

    fn truncate(&self, w: &mut WriteCx<'_>, thread: ThreadId, from: MessageId) {
        self.stop(w, thread);
        self.remove_messages(w, thread, from);
    }

    fn start(
        &self,
        w: &mut WriteCx<'_>,
        thread: ThreadId,
        text: &str,
        files: Vec<Attachment>,
        model: Rc<dyn Model>,
        generation: Generation,
    ) {
        let user = Message::user(self.id(w), thread, text, files);
        w.push_row(self.messages, user.id, user);
        let history = w.read(|r| self.project_messages(r, thread, |message| (message.author, message.text())));
        let title_history = (history.iter().filter(|(author, _)| *author == Author::User).count() == 1).then(|| history.clone());
        let answer = self.id(w);
        w.push_row(self.messages, answer, Message::answer(answer, thread));
        w.update(self.answering, |a| a.push(thread));
        self.touch(w, thread);
        let (tx, mut rx) = channel();
        let chat = *self;
        let receiver = w.spawn_in(self.scope, "chat reply events", move |task| async move {
            loop {
                let event =
                    task.recv(&mut rx).await.unwrap_or_else(|| Event::Failed("The model closed before completing its reply.".into()));
                let end = matches!(event, Event::Done | Event::Failed(_));
                if task.write(|w| chat.on_event(w, thread, answer, event)).is_none() || end {
                    break;
                }
            }
        });
        w.update(self.requests, |r| r.answers.insert(thread, Running { id: answer, receiver, cancel: None }));
        let cancel = model.respond(w, self.scope, Request { purpose: Purpose::Reply, generation, history }, tx);
        if self.install_cancel(w, thread, answer, false, cancel)
            && let Some(history) = title_history
        {
            self.name(w, thread, answer, history, model);
        }
    }

    fn name(&self, w: &mut WriteCx<'_>, thread: ThreadId, answer: MessageId, history: Vec<(Author, String)>, model: Rc<dyn Model>) {
        self.stop_title(w, thread);
        let (tx, mut rx) = channel();
        let chat = *self;
        let receiver = w.spawn_in(self.scope, "chat title events", move |task| async move {
            let mut title = String::new();
            while let Some(event) = task.recv(&mut rx).await {
                match event {
                    Event::Text(text) => title.push_str(&text),
                    Event::Done | Event::Failed(_) => {
                        let title = matches!(event, Event::Done).then(|| title.trim()).filter(|title| !title.is_empty());
                        task.write(|w| chat.finish_title(w, thread, answer, title));
                        return;
                    }
                    _ => {}
                }
            }
            task.write(|w| chat.finish_title(w, thread, answer, None));
        });
        w.update(self.requests, |r| r.titles.insert(thread, Running { id: answer, receiver, cancel: None }));
        let cancel = model.respond(w, self.scope, Request { purpose: Purpose::Title, generation: Generation::default(), history }, tx);
        self.install_cancel(w, thread, answer, true, cancel);
    }

    fn install_cancel(&self, w: &mut WriteCx<'_>, thread: ThreadId, id: MessageId, title: bool, cancel: Cancel) -> bool {
        let mut pending = Some(cancel);
        w.mutate(self.requests, |requests| {
            let requests = if title { &mut requests.titles } else { &mut requests.answers };
            let Some(request) = requests.get_mut(&thread).filter(|request| request.id == id) else { return false };
            request.cancel = pending.take().map(Rc::from);
            true
        });
        if let Some(cancel) = pending {
            cancel();
            false
        } else {
            true
        }
    }

    fn finish_title(&self, w: &mut WriteCx<'_>, thread: ThreadId, answer: MessageId, title: Option<&str>) {
        if !w.with(self.requests, |r| r.titles.get(&thread).is_some_and(|r| r.id == answer)) {
            return;
        }
        self.stop_title(w, thread);
        if let Some(title) = title {
            w.update_row(self.threads, &thread, |t| t.title = title.to_owned());
            self.save(w, thread);
        }
    }

    fn stop_title(&self, w: &mut WriteCx<'_>, thread: ThreadId) {
        if let Some(request) = w.update(self.requests, |r| r.titles.remove(&thread)) {
            w.cancel_task(request.receiver);
            if let Some(cancel) = request.cancel {
                cancel();
            }
        }
    }

    fn on_event(&self, w: &mut WriteCx<'_>, thread: ThreadId, answer: MessageId, event: Event) {
        if !w.with(self.requests, |r| r.answers.get(&thread).is_some_and(|r| r.id == answer)) {
            return;
        }
        let end = matches!(event, Event::Done | Event::Failed(_));
        let now = w.now();
        w.update_row(self.messages, &answer, |m| m.apply(event, now));
        if end {
            let retired = w.update(self.requests, |r| r.answers.remove(&thread));
            self.ended(w, thread);
            let next = w.keys(self.queue).iter().find_map(|id| w.with_row(self.queue, id, |q| q.filter(|q| q.thread == thread).cloned()));
            if let Some(queued) = next
                && let Some(model) = self.model_by_id(w, &queued.model_id)
            {
                w.remove_row(self.queue, &queued.id);
                self.start(w, thread, &queued.text, queued.files, model.model, queued.generation);
            }
            if let Some(cancel) = retired.and_then(|request| request.cancel) {
                cancel();
            }
        }
    }

    pub fn stop(&self, w: &mut WriteCx<'_>, thread: ThreadId) {
        if let Some(request) = w.update(self.requests, |r| r.answers.remove(&thread)) {
            w.cancel_task(request.receiver);
            let now = w.now();
            w.update_row(self.messages, &request.id, |m| m.finish(Status::Stopped, now));
            self.ended(w, thread);
            if let Some(cancel) = request.cancel {
                cancel();
            }
        } else {
            w.update(self.answering, |a| a.retain(|t| *t != thread));
        }
    }

    fn ended(&self, w: &mut WriteCx<'_>, thread: ThreadId) {
        w.update(self.answering, |a| a.retain(|t| *t != thread));
        self.touch(w, thread);
        self.save(w, thread);
    }

    fn touch(&self, w: &mut WriteCx<'_>, thread: ThreadId) {
        let now = w.with(self.options, |o| (o.clock)());
        w.update_row(self.threads, &thread, |t| t.updated = now);
    }

    fn save(&self, w: &mut WriteCx<'_>, thread: ThreadId) {
        if let Some(store) = w.with(self.options, |o| o.store.clone())
            && let Some(thread) = w.get_row(self.threads, &thread)
        {
            let messages = w.read(|r| self.project_messages(r, thread.id, Message::save));
            store.save(thread, messages);
        }
    }
}

fn model_by_id<'a>(models: &'a [ModelEntry], id: &str) -> Option<&'a ModelEntry> {
    models.iter().find(|entry| entry.id == id)
}
