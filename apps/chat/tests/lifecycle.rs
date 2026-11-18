//! Adversarial model delivery at the real Chat boundary. Cancellation is deliberately ignored
//! by the transport; queued and late events must be retired by their current request owner.

#[path = "../examples/support/controlled.rs"]
mod controlled;

use controlled::Controlled;
use octara::accesskit::Action as AccessibilityAction;
use octara::app::Window;
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;
use octara_chat::engine::Purpose;
use octara_chat::model::{Event, Message, MessageId, SavedMessage, Status, ThreadId};
use octara_chat::settings::Generation;
use octara_chat::store::Store;
use octara_chat::{Chat, Options};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Action = Box<dyn FnOnce(&Chat)>;

struct Harness {
    window: Window,
    chat: Chat,
    model: Controlled,
    action: Rc<RefCell<Option<Action>>>,
}

impl Harness {
    fn new(store: Option<Store>) -> Self {
        Self::with_view(store, |chat| col().size_full().child(octara_kit::text_field(chat.draft).id("composer").label("Message")).into())
    }

    fn with_view(store: Option<Store>, view: impl Fn(&Chat) -> View + 'static) -> Self {
        let model = Controlled::default();
        let engine = model.clone();
        let exposed = Rc::new(RefCell::new(None));
        let slot = exposed.clone();
        let action: Rc<RefCell<Option<Action>>> = Rc::default();
        let invoke = action.clone();
        let window = Window::headless(move || {
            let chat = Chat::new(Options {
                models: vec![octara_chat::ModelEntry::new("controlled", Rc::new(engine.clone()))],
                store: store.clone(),
                ..Options::default()
            });
            *slot.borrow_mut() = Some(chat.clone());
            let invoke = invoke.clone();
            on_command("test.chat", {
                let chat = chat.clone();
                move || {
                    let action = invoke.borrow_mut().take();
                    if let Some(action) = action {
                        action(&chat);
                    }
                }
            });
            view(&chat)
        });
        let chat = exposed.borrow_mut().take().expect("the root exposes its Chat");
        Self { window, chat, model, action }
    }

    fn run(&mut self, action: impl FnOnce(&Chat) + 'static) {
        *self.action.borrow_mut() = Some(Box::new(action));
        self.window.invoke("test.chat").unwrap();
        self.window.app().poll();
    }

    fn read<T>(&self, read: impl FnOnce(&Chat) -> T) -> T {
        self.window.app().run(|| read(&self.chat))
    }

    fn send(&mut self, text: &str) -> ThreadId {
        let text = text.to_owned();
        self.run(move |chat| {
            chat.draft.set(text);
            chat.send();
        });
        self.read(|chat| untrack(|| chat.current.get()).unwrap())
    }

    fn reply(&self, ordinal: usize) -> usize {
        self.model.index(Purpose::Reply, ordinal).expect("the model received this answer request")
    }

    fn emit(&mut self, request: usize, events: impl IntoIterator<Item = Event>) {
        for event in events {
            // A retired receiver may reject delivery. Either way the transport attempts it.
            self.model.emit(request, event);
        }
        self.window.app().poll();
    }

    fn saved(&self, thread: ThreadId) -> Vec<SavedMessage> {
        self.read(|chat| chat.thread_messages(thread).iter().map(|message| message.save()).collect())
    }

    fn answer(&self, thread: ThreadId) -> MessageId {
        self.read(|chat| chat.thread_messages(thread).last().unwrap().id)
    }

    fn answering(&self, thread: ThreadId) -> bool {
        self.read(|chat| chat.is_answering(thread))
    }

    fn queued(&self) -> usize {
        self.read(|chat| chat.queue.keys().len())
    }
}

#[test]
fn the_composer_shares_send_eligibility_across_input_routes() {
    let mut h = Harness::with_view(None, octara_chat::view);
    let send_disabled = |window: &Window| window.kernel().state_of("send").unwrap().contains(State::DISABLED);
    let accessible_send = |window: &mut Window| {
        let target = a11y_id(window.kernel().find("send").unwrap());
        window.event(InputEvent::Accessibility { target, action: AccessibilityAction::Click, data: None })
    };
    h.window.invoke("chat.focus_composer").unwrap();
    h.window.type_text("   ");
    assert!(send_disabled(&h.window));
    assert!(!accessible_send(&mut h.window));
    h.window.press_key("enter").unwrap();
    assert_eq!(h.model.count(Purpose::Reply), 0);
    assert_eq!(h.window.kernel().value_of("composer").as_deref(), Some("   "));

    h.run(|chat| {
        chat.attach(&["pointer.txt".into()]);
        chat.generation.set(Generation { max_completion_tokens: Some(0), ..Generation::default() });
    });
    assert!(send_disabled(&h.window));
    h.window.press_key("enter").unwrap();
    assert_eq!(h.model.count(Purpose::Reply), 0);
    assert_eq!(h.read(|chat| chat.files.len()), 1);

    h.run(|chat| chat.generation.set(Generation::default()));
    assert!(!send_disabled(&h.window));
    h.window.tap("send").unwrap();
    let thread = h.read(|chat| untrack(|| chat.current.get()).unwrap());
    let first = h.reply(0);
    assert!(h.window.kernel().exists("stop"));

    h.run(|chat| chat.attach(&["keyboard.txt".into()]));
    h.window.invoke("chat.focus_composer").unwrap();
    h.window.press_key("enter").unwrap();
    assert_eq!(h.queued(), 1);
    assert_eq!(h.model.count(Purpose::Reply), 1);
    h.emit(first, [Event::Done]);
    assert_eq!(h.queued(), 0);
    assert_eq!(h.model.count(Purpose::Reply), 2);
    h.window.invoke("chat.stop").unwrap();

    h.run(|chat| chat.attach(&["accessibility.txt".into()]));
    assert!(!send_disabled(&h.window));
    assert!(accessible_send(&mut h.window));
    assert_eq!(h.model.count(Purpose::Reply), 3);
    h.read(|chat| {
        let messages = chat.thread_messages(thread);
        for (index, name) in ["pointer.txt", "keyboard.txt", "accessibility.txt"].into_iter().enumerate() {
            let message = &messages[2 * index];
            assert!(message.text().is_empty());
            assert_eq!(message.files()[0].name, name);
        }
        assert!(chat.files.is_empty());
    });
    assert!(h.window.app().diagnostics().is_empty(), "{:?}", h.window.app().diagnostics());
}

#[test]
fn stop_retires_events_already_queued_before_transport_cancellation() {
    let mut h = Harness::new(None);
    let thread = h.send("first");
    let old = h.reply(0);
    h.emit(old, [Event::Text("original".into())]);
    let model = h.model.clone();
    h.run(move |chat| {
        assert!(model.emit(old, Event::Text(" queued before stop".into())));
        assert!(model.emit(old, Event::Done));
        chat.stop(thread);
    });
    let stopped = h.saved(thread);
    assert_eq!(stopped.last().unwrap().status, Status::Stopped);
    assert_eq!(h.read(|chat| chat.thread_messages(thread).last().unwrap().text()), "original");
    assert!(h.model.cancelled(old));
    h.emit(old, [Event::Text(" late".into()), Event::Done, Event::Failed("late failure".into())]);
    assert_eq!(h.saved(thread), stopped, "a stopped message stays frozen despite old deltas and terminal events");
    assert!(!h.answering(thread));
    assert_eq!(h.model.count(Purpose::Reply), 1);
}

#[test]
fn old_terminal_events_cannot_clear_a_new_answer_or_release_its_queue() {
    let mut h = Harness::new(None);
    let thread = h.send("first");
    let old = h.reply(0);
    h.emit(old, [Event::Text("old".into())]);
    h.run(move |chat| chat.stop(thread));
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
    let mut h = Harness::new(None);
    let thread = h.send("question");
    let old = h.reply(0);
    h.emit(old, [Event::Text("old partial".into())]);
    let answer = h.answer(thread);
    let model = h.model.clone();
    h.run(move |chat| {
        model.emit(old, Event::Text(" queued stale delta".into()));
        model.emit(old, Event::Done);
        chat.regenerate(answer);
    });
    assert_eq!(h.model.count(Purpose::Reply), 2);
    assert!(h.model.cancelled(old));
    let current = h.reply(1);
    h.emit(current, [Event::Text("replacement".into())]);
    h.emit(old, [Event::Text("late old".into()), Event::Failed("late old failure".into())]);
    assert!(h.answering(thread));
    assert_eq!(h.saved(thread).len(), 2, "regeneration replaces the original pair");
    assert_eq!(h.read(|chat| chat.thread_messages(thread).last().unwrap().text()), "replacement");
    h.emit(current, [Event::Done]);
    assert!(!h.answering(thread));
}

#[test]
fn editing_a_question_retires_the_answer_and_its_pending_deltas() {
    let mut h = Harness::new(None);
    let thread = h.send("original question");
    let old = h.reply(0);
    h.emit(old, [Event::Text("obsolete answer".into())]);
    let question = h.read(|chat| chat.thread_messages(thread)[0].id);
    let model = h.model.clone();
    h.run(move |chat| {
        chat.edit(question);
        model.emit(old, Event::Text(" queued after edit".into()));
        model.emit(old, Event::Done);
        chat.draft.set("edited question".into());
        chat.send();
    });
    let current = h.reply(1);
    h.emit(old, [Event::Done, Event::Failed("obsolete failure".into())]);
    h.emit(current, [Event::Text("edited answer".into()), Event::Done]);
    assert_eq!(h.model.count(Purpose::Reply), 2);
    assert!(h.model.cancelled(old));
    let all = h.read(|chat| chat.thread_messages(thread).iter().map(Message::text).collect::<Vec<_>>());
    assert_eq!(all, ["edited question", "edited answer"]);
    assert!(!h.answering(thread));
}

#[test]
fn deleting_a_thread_retires_queued_answers_and_does_not_recreate_its_file() {
    let directory = Directory::new();
    let store = Store::new(&directory.0);
    let mut h = Harness::new(Some(store.clone()));
    let thread = h.send("delete this conversation");
    let old = h.reply(0);
    let title = h.model.index(Purpose::Title, 0).unwrap();
    h.emit(old, [Event::Text("existing partial".into())]);
    h.send("queued but never sent");
    let model = h.model.clone();
    h.run(move |chat| {
        model.emit(old, Event::Text(" in flight during deletion".into()));
        model.emit(old, Event::Done);
        model.emit(title, Event::Text("obsolete title".into()));
        model.emit(title, Event::Done);
        chat.delete(thread);
    });
    h.emit(old, [Event::Text("late text".into()), Event::Done, Event::Failed("late failure".into())]);
    h.emit(title, [Event::Text("late title".into()), Event::Done, Event::Done]);
    assert!(h.model.cancelled(title), "deleting retires the title's transport too");
    assert_eq!(h.model.count(Purpose::Reply), 1, "deleting cannot release the queue");
    assert_eq!(h.queued(), 0);
    assert!(!h.answering(thread));
    assert!(h.saved(thread).is_empty());
    assert!(h.read(|chat| chat.threads.item(&thread).is_none()));
    assert_eq!(h.read(|chat| untrack(|| chat.current.get())), None);
    assert!(h.window.app().settle().is_empty());
    assert!(store.load().is_empty(), "a retired result cannot save or resurrect the deleted thread");
}

#[test]
fn saved_and_reloaded_conversations_contain_only_accepted_events() {
    let directory = Directory::new();
    let store = Store::new(&directory.0);
    let (thread, expected) = {
        let mut h = Harness::new(Some(store.clone()));
        let thread = h.send("remember the stopped answer");
        let stopped = h.reply(0);
        h.emit(stopped, [Event::Text("kept prefix".into())]);
        let title = h.model.index(Purpose::Title, 0).unwrap();
        h.emit(title, [Event::Text("Saved conversation".into()), Event::Done]);
        h.run(move |chat| chat.stop(thread));
        h.send("current answer");
        let current = h.reply(1);
        h.emit(stopped, [Event::Text("discarded old delta".into()), Event::Done]);
        h.emit(current, [Event::Text("kept current answer".into()), Event::Done]);
        h.emit(stopped, [Event::Failed("discarded old failure".into()), Event::Done]);
        assert!(h.window.app().settle().is_empty());
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
    let mut h = Harness::new(None);
    h.run(|_| {
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
    let model = Controlled::default();
    let engine = model.clone();
    let mut window = Window::headless(move || {
        let engine = engine.clone();
        octara_chat::root(move || Options {
            models: vec![octara_chat::ModelEntry::new("controlled", Rc::new(engine.clone()))],
            ..Options::default()
        })
    });
    window.dispatch(octara::app::Event::Focus("composer".into())).unwrap();
    window.dispatch(octara::app::Event::Input("composer".into(), "close while answering".into())).unwrap();
    window.press_key("enter").unwrap();
    let current = model.index(Purpose::Reply, 0).unwrap();
    let title = model.index(Purpose::Title, 0).unwrap();
    let app = window.app();
    let survivor = app.open_window(octara::platform::WindowOptions::new("Still open"), || col().size_full()).unwrap();
    app.close_window(window.id());
    assert!(window.is_closed());
    assert!(!survivor.is_closed(), "the runtime stays alive after the chat owner closes");
    assert!(model.cancelled(current), "disposing the owner also asks its transport to stop");
    assert!(model.cancelled(title));
    assert!(!model.emit(current, Event::Text("after owner disposal".into())), "the receiver belongs to the disposed window scope");
    assert!(!model.emit(title, Event::Done));
    app.poll();
    assert_eq!(model.count(Purpose::Reply), 1);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn a_manual_rename_retires_title_events_queued_before_it() {
    let mut h = Harness::new(None);
    let thread = h.send("automatic title request");
    let title = h.model.index(Purpose::Title, 0).unwrap();
    let model = h.model.clone();
    h.run(move |chat| {
        assert!(model.emit(title, Event::Text("Queued automatic title".into())));
        assert!(model.emit(title, Event::Done));
        chat.rename(thread, "Chosen by the person");
    });
    h.emit(title, [Event::Text("late suffix".into()), Event::Done, Event::Failed("late".into())]);
    assert_eq!(h.read(|chat| untrack(|| chat.threads.item(&thread).unwrap().get()).title), "Chosen by the person");
    assert!(h.model.cancelled(title));
}

#[test]
fn title_completion_is_terminal_and_cannot_repeat_after_a_later_rename() {
    let mut h = Harness::new(None);
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
    assert_eq!(h.read(|chat| untrack(|| chat.threads.item(&thread).unwrap().get()).title), "Accepted title");
    h.run(move |chat| chat.rename(thread, "Later manual title"));
    h.emit(title, [Event::Done, Event::Text("late".into()), Event::Done]);
    assert_eq!(h.read(|chat| untrack(|| chat.threads.item(&thread).unwrap().get()).title), "Later manual title");
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
