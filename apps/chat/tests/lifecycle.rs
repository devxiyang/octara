//! Adversarial model delivery at the real Chat boundary. Cancellation is deliberately ignored
//! by the transport; queued and late events must be retired by their current request owner.

#[path = "../examples/support/controlled.rs"]
mod controlled;

#[path = "support/session.rs"]
mod session;

use controlled::Controlled;
use octara::prelude::*;
use octara_chat::engine::Purpose;
use octara_chat::model::{Author, Event, Message, MessageId, SavedMessage, Status, ThreadId};
use octara_chat::store::Store;
use octara_chat::{Chat, Options};
use session::Session;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Harness {
    session: Session,
    model: Controlled,
}

impl Harness {
    fn new(store: Option<Store>) -> Self {
        let model = Controlled::default();
        let options =
            Options { models: vec![octara_chat::ModelEntry::new("controlled", Rc::new(model.clone()))], store, ..Options::default() };
        Self { session: Session::new(options, |_, _| col().size(rel(1.0)).into()), model }
    }

    fn run(&self, action: impl FnOnce(&Chat, &mut WriteCx<'_>)) {
        self.session.update(action);
    }

    fn read<T>(&self, read: impl FnOnce(&Chat, &mut ReadCx<'_>) -> T) -> T {
        self.session.update(|chat, w| w.read(|r| read(chat, r)))
    }

    fn send(&self, text: &str) -> ThreadId {
        self.run(|chat, w| {
            w.set(chat.draft, text.to_owned());
            chat.send(w);
        });
        self.read(|chat, r| r.get(chat.current).unwrap())
    }

    fn reply(&self, ordinal: usize) -> usize {
        self.model.index(Purpose::Reply, ordinal).expect("the model received this answer request")
    }

    fn emit(&self, request: usize, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.model.emit(request, event);
        }
        self.session.settle();
    }

    fn saved(&self, thread: ThreadId) -> Vec<SavedMessage> {
        self.read(|chat, r| chat.thread_messages(r, thread).iter().map(Message::save).collect())
    }

    fn answer(&self, thread: ThreadId) -> MessageId {
        self.read(|chat, r| chat.thread_messages(r, thread).last().unwrap().id)
    }

    fn answering(&self, thread: ThreadId) -> bool {
        self.read(|chat, r| chat.is_answering(r, thread))
    }

    fn queued(&self) -> usize {
        self.read(|chat, r| r.len(chat.queue))
    }
}

#[test]
fn stop_retires_events_already_queued_before_transport_cancellation() {
    let h = Harness::new(None);
    let thread = h.send("first");
    let old = h.reply(0);
    h.emit(old, [Event::Text("original".into())]);
    let model = h.model.clone();
    h.run(move |chat, w| {
        assert!(model.emit(old, Event::Text(" queued before stop".into())));
        assert!(model.emit(old, Event::Done));
        chat.stop(w, thread);
    });
    let stopped = h.saved(thread);
    assert_eq!(stopped.last().unwrap().status, Status::Stopped);
    assert_eq!(h.read(|chat, r| chat.thread_messages(r, thread).last().unwrap().text()), "original");
    assert!(h.model.cancelled(old));
    h.emit(old, [Event::Text(" late".into()), Event::Done, Event::Failed("late failure".into())]);
    assert_eq!(h.saved(thread), stopped, "a stopped message stays frozen despite old deltas and terminal events");
    assert!(!h.answering(thread));
    assert_eq!(h.model.count(Purpose::Reply), 1);
}

#[test]
fn old_terminal_events_cannot_clear_a_new_answer_or_release_its_queue() {
    let h = Harness::new(None);
    let thread = h.send("first");
    let old = h.reply(0);
    h.emit(old, [Event::Text("old".into())]);
    h.run(move |chat, w| chat.stop(w, thread));
    h.send("second");
    let current = h.reply(1);
    h.send("third, queued");
    assert_eq!(h.queued(), 1);
    h.emit(old, [Event::Done, Event::Failed("late".into()), Event::Text(" polluted".into()), Event::Done]);
    assert!(h.answering(thread), "an old terminal cannot end the current request");
    assert_eq!(h.queued(), 1);
    assert_eq!(h.model.count(Purpose::Reply), 2, "old terminals cannot send queued messages early");

    h.emit(current, [Event::Text("new".into()), Event::Done, Event::Done, Event::Failed("duplicate terminal".into())]);
    assert_eq!(h.queued(), 0);
    assert_eq!(h.model.count(Purpose::Reply), 3, "one current terminal sends exactly one queued message");
    assert!(h.answering(thread), "duplicates from the previous request cannot stop the dequeued answer");
    let queued = h.reply(2);
    h.emit(queued, [Event::Done, Event::Done]);
    assert!(!h.answering(thread));
    assert_eq!(h.model.count(Purpose::Reply), 3);
    assert_eq!(h.saved(thread).len(), 6);
    let questions: Vec<_> = h
        .model
        .requests()
        .into_iter()
        .filter(|request| request.purpose == Purpose::Reply)
        .map(|request| request.history.last().unwrap().1.clone())
        .collect();
    assert_eq!(questions, ["first", "second", "third, queued"], "each queued question is requested exactly once and in order");
}

#[test]
fn regenerating_retires_queued_events_before_the_replacement_starts() {
    let h = Harness::new(None);
    let thread = h.send("question");
    let old = h.reply(0);
    h.emit(old, [Event::Text("old partial".into())]);
    let answer = h.answer(thread);
    let model = h.model.clone();
    h.run(move |chat, w| {
        model.emit(old, Event::Text(" queued stale delta".into()));
        model.emit(old, Event::Done);
        chat.regenerate(w, answer);
    });
    assert_eq!(h.model.count(Purpose::Reply), 2);
    assert!(h.model.cancelled(old));
    let current = h.reply(1);
    h.emit(current, [Event::Text("replacement".into())]);
    h.emit(old, [Event::Text("late old".into()), Event::Failed("late old failure".into())]);
    assert!(h.answering(thread));
    assert_eq!(h.saved(thread).len(), 2, "regeneration replaces the original pair");
    assert_eq!(h.read(|chat, r| chat.thread_messages(r, thread).last().unwrap().text()), "replacement");
    h.emit(current, [Event::Done]);
    assert!(!h.answering(thread));
}

#[test]
fn editing_a_question_retires_the_answer_and_its_pending_deltas() {
    let h = Harness::new(None);
    let thread = h.send("original question");
    let old = h.reply(0);
    h.emit(old, [Event::Text("obsolete answer".into())]);
    let question = h.read(|chat, r| chat.thread_messages(r, thread)[0].id);
    let model = h.model.clone();
    h.run(move |chat, w| {
        chat.edit(w, question);
        model.emit(old, Event::Text(" queued after edit".into()));
        model.emit(old, Event::Done);
        w.set(chat.draft, "edited question".into());
        chat.send(w);
    });
    let current = h.reply(1);
    h.emit(old, [Event::Done, Event::Failed("obsolete failure".into())]);
    h.emit(current, [Event::Text("edited answer".into()), Event::Done]);
    assert_eq!(h.model.count(Purpose::Reply), 2);
    assert!(h.model.cancelled(old));
    let all = h.read(|chat, r| chat.thread_messages(r, thread).iter().map(Message::text).collect::<Vec<_>>());
    assert_eq!(all, ["edited question", "edited answer"]);
    assert!(!h.answering(thread));
}

#[test]
fn deleting_a_thread_retires_queued_answers_and_does_not_recreate_its_file() {
    let directory = Directory::new();
    let store = Store::new(&directory.0);
    let h = Harness::new(Some(store.clone()));
    let thread = h.send("delete this conversation");
    let old = h.reply(0);
    let title = h.model.index(Purpose::Title, 0).unwrap();
    h.emit(old, [Event::Text("existing partial".into())]);
    h.send("queued but never sent");
    let model = h.model.clone();
    h.run(move |chat, w| {
        model.emit(old, Event::Text(" in flight during deletion".into()));
        model.emit(old, Event::Done);
        model.emit(title, Event::Text("obsolete title".into()));
        model.emit(title, Event::Done);
        chat.delete(w, thread);
    });
    h.emit(old, [Event::Text("late text".into()), Event::Done, Event::Failed("late failure".into())]);
    h.emit(title, [Event::Text("late title".into()), Event::Done, Event::Done]);
    assert!(h.model.cancelled(title), "deleting retires the title's transport too");
    assert_eq!(h.model.count(Purpose::Reply), 1, "deleting cannot release the queue");
    assert_eq!(h.queued(), 0);
    assert!(!h.answering(thread));
    assert!(h.saved(thread).is_empty());
    assert!(h.read(|chat, r| r.get_row(chat.threads, &thread).is_none()));
    assert_eq!(h.read(|chat, r| r.get(chat.current)), None);
    h.session.clean();
    store.flush();
    assert!(store.load().is_empty(), "a retired result cannot save or resurrect the deleted thread");
}

#[test]
fn indexed_history_keeps_interleaved_conversations_independent() {
    let h = Harness::new(None);
    let first = h.send("first question");
    h.emit(h.reply(0), [Event::Text("first answer".into()), Event::Done]);
    h.run(|chat, w| chat.new_chat(w));
    let second = h.send("second question");
    h.emit(h.reply(1), [Event::Text("second answer".into()), Event::Done]);

    h.run(|chat, w| chat.open(w, first));
    h.send("first follow-up");
    let requests = h.model.requests();
    assert_eq!(
        requests[h.reply(2)].history,
        [(Author::User, "first question".into()), (Author::Assistant, "first answer".into()), (Author::User, "first follow-up".into())],
    );
    let second_history = h.saved(second);
    assert_eq!(second_history.len(), 2);
    h.run(|chat, w| chat.delete(w, first));
    assert!(h.saved(first).is_empty());
    assert_eq!(h.saved(second), second_history);
    h.session.clean();
}

#[test]
fn saved_and_reloaded_conversations_contain_only_accepted_events() {
    let directory = Directory::new();
    let store = Store::new(&directory.0);
    let (thread, expected) = {
        let h = Harness::new(Some(store.clone()));
        let thread = h.send("remember the stopped answer");
        let stopped = h.reply(0);
        h.emit(stopped, [Event::Text("kept prefix".into())]);
        let title = h.model.index(Purpose::Title, 0).unwrap();
        h.emit(title, [Event::Text("Saved conversation".into()), Event::Done]);
        h.run(move |chat, w| chat.stop(w, thread));
        h.send("current answer");
        let current = h.reply(1);
        h.emit(stopped, [Event::Text("discarded old delta".into()), Event::Done]);
        h.emit(current, [Event::Text("kept current answer".into()), Event::Done]);
        h.emit(stopped, [Event::Failed("discarded old failure".into()), Event::Done]);
        h.session.clean();
        store.flush();
        let expected = h.saved(thread);
        assert_eq!(expected[1].status, Status::Stopped);
        assert_eq!(expected[3].status, Status::Done);
        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].1, expected);
        (thread, expected)
    };
    let h = Harness::new(Some(store));
    assert_eq!(h.saved(thread), expected);
    assert!(!h.answering(thread));
    assert_eq!(h.model.count(Purpose::Reply), 0, "loading history cannot start a model request");
}

#[test]
fn ended_messages_reject_the_public_stream_fast_path_for_every_part_kind() {
    let h = Harness::new(None);
    h.run(|_, _| {
        let now = Duration::from_secs(1);
        for status in [Status::Stopped, Status::Done, Status::Failed("failure".into())] {
            for event in [
                Event::Text("initial".into()),
                Event::Reasoning("thinking".into()),
                Event::ToolStart { id: "tool".into(), name: "search".into() },
            ] {
                let mut message = Message::answer(1, 1);
                message.apply(event, Duration::ZERO);
                message.finish(status.clone(), now);
                let frozen = message.save();
                for delta in [
                    Event::Text("late text".into()),
                    Event::Reasoning("late reasoning".into()),
                    Event::ToolInput { id: "tool".into(), delta: "late input".into() },
                ] {
                    assert!(!message.stream(&delta), "a terminal message cannot accept {delta:?}");
                    message.apply(delta, now + now);
                    assert_eq!(message.save(), frozen);
                }
            }
        }
    });
}

#[test]
fn closing_the_owning_window_cancels_transport_interest_and_rejects_delivery() {
    let h = Harness::new(None);
    h.send("close while answering");
    let current = h.reply(0);
    let title = h.model.index(Purpose::Title, 0).unwrap();
    let survivor = {
        let mut driver = h.session.driver.borrow_mut();
        let app = driver.app_mut();
        let survivor = app.open_window(WindowOptions::default(), |_| col().size(rel(1.0))).unwrap();
        app.close_window(h.session.window).unwrap();
        assert!(app.window(h.session.window).is_none());
        survivor
    };
    assert!(h.session.driver.borrow().app().window(survivor).is_some(), "the runtime stays alive after the chat owner closes");
    assert!(h.model.cancelled(current), "disposing the owner also asks its transport to stop");
    assert!(h.model.cancelled(title));
    assert!(!h.model.emit(current, Event::Text("after owner disposal".into())), "the receiver belongs to the disposed window scope");
    assert!(!h.model.emit(title, Event::Done));
    h.session.settle();
    assert_eq!(h.model.count(Purpose::Reply), 1);
    h.session.clean();
}

#[test]
fn a_manual_rename_retires_title_events_queued_before_it() {
    let h = Harness::new(None);
    let thread = h.send("automatic title request");
    let title = h.model.index(Purpose::Title, 0).unwrap();
    let model = h.model.clone();
    h.run(move |chat, w| {
        assert!(model.emit(title, Event::Text("Queued automatic title".into())));
        assert!(model.emit(title, Event::Done));
        chat.rename(w, thread, "Chosen by the person");
    });
    h.emit(title, [Event::Text("late suffix".into()), Event::Done, Event::Failed("late".into())]);
    assert_eq!(h.read(|chat, r| r.get_row(chat.threads, &thread).unwrap().title), "Chosen by the person");
    assert!(h.model.cancelled(title));
}

#[test]
fn title_completion_is_terminal_and_cannot_repeat_after_a_later_rename() {
    let h = Harness::new(None);
    let thread = h.send("automatic title request");
    let title = h.model.index(Purpose::Title, 0).unwrap();
    h.emit(
        title,
        [
            Event::Text("Accepted title".into()),
            Event::Done,
            Event::Text(" duplicate suffix".into()),
            Event::Done,
            Event::Failed("duplicate failure".into()),
        ],
    );
    assert_eq!(h.read(|chat, r| r.get_row(chat.threads, &thread).unwrap().title), "Accepted title");
    h.run(move |chat, w| chat.rename(w, thread, "Later manual title"));
    h.emit(title, [Event::Done, Event::Text("late".into()), Event::Done]);
    assert_eq!(h.read(|chat, r| r.get_row(chat.threads, &thread).unwrap().title), "Later manual title");
    assert!(h.model.cancelled(title));
}

/// A unique directory created by this test; never remove an existing user's path.
struct Directory(std::path::PathBuf);

impl Directory {
    fn new() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("octara-chat-lifecycle-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
