//! Model identity belongs to configuration; requests capture their model before calling it.

#[path = "../examples/support/controlled.rs"]
mod controlled;

use controlled::Controlled;
use octara::prelude::*;
use octara::reactive::untrack;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::{Event as ModelEvent, Status};
use octara_chat::{Chat, ModelEntry, Options};
use std::cell::RefCell;
use std::rc::Rc;

type Action = Box<dyn FnOnce(&Chat)>;

struct Harness {
    window: Window,
    current: Rc<RefCell<Option<Chat>>>,
    entries: Rc<RefCell<Vec<ModelEntry>>>,
    action: Rc<RefCell<Option<Action>>>,
}

impl Harness {
    fn new(entries: Vec<ModelEntry>) -> Self {
        let entries = Rc::new(RefCell::new(entries));
        let current = Rc::new(RefCell::new(None));
        let action: Rc<RefCell<Option<Action>>> = Rc::default();
        let (models, exposed, invoke) = (entries.clone(), current.clone(), action.clone());
        let window = Window::headless(move || {
            let chat = Chat::new(Options { models: models.borrow().clone(), ..Options::default() });
            *exposed.borrow_mut() = Some(chat.clone());
            let action = invoke.clone();
            on_command("test.chat", move || {
                let run = action.borrow_mut().take().unwrap();
                run(&chat);
            });
            el().size_full()
        });
        Self { window, current, entries, action }
    }

    fn run(&mut self, f: impl FnOnce(&Chat) + 'static) {
        *self.action.borrow_mut() = Some(Box::new(f));
        self.window.invoke("test.chat").unwrap();
        self.window.app().poll();
    }

    fn read<T>(&self, f: impl FnOnce(&Chat) -> T) -> T {
        self.window.app().run(|| f(self.current.borrow().as_ref().unwrap()))
    }

    fn send(&mut self, text: &str) {
        let text = text.to_owned();
        self.run(move |chat| {
            chat.draft.set(text);
            chat.send();
        });
    }
}

#[test]
fn selection_survives_reordering_and_missing_ids_never_pick_another_model() {
    let a = Controlled::default();
    let b = Controlled::default();
    // Both deliberately have the same display name.
    let entries = vec![ModelEntry::new("a", Rc::new(a.clone())), ModelEntry::new("b", Rc::new(b.clone()))];
    let mut h = Harness::new(entries);
    h.run(|chat| chat.model.set(Some("b".into())));
    h.entries.borrow_mut().reverse();
    h.window.app().hot_reload();
    assert_eq!(h.read(|chat| untrack(|| chat.model.get())), Some("b".into()));
    h.send("reordered");
    assert_eq!(b.count(Purpose::Reply), 1);
    assert_eq!(b.requests()[0].history.last().unwrap().1, "reordered");
    assert_eq!(a.count(Purpose::Reply), 0);
    b.emit(b.index(Purpose::Reply, 0).unwrap(), ModelEvent::Text("B remains selected after reordering".into()));
    h.window.app().poll();
    h.run(|chat| chat.stop(untrack(|| chat.current.get()).unwrap()));

    h.entries.borrow_mut().retain(|entry| entry.id != "b");
    h.window.app().hot_reload();
    assert!(!h.read(Chat::has_model));
    h.send("keep this draft");
    assert_eq!(h.read(|chat| untrack(|| chat.draft.get())), "keep this draft");
    assert_eq!(a.count(Purpose::Reply), 0, "removing B must not silently select A at its old index");
    h.run(|chat| {
        chat.model.set(Some("a".into()));
        chat.send();
    });
    assert_eq!(a.count(Purpose::Reply), 1);
    assert!(h.window.app().diagnostics().is_empty(), "{:?}", h.window.app().diagnostics());
}

type SelectionChange = Rc<RefCell<Option<Box<dyn Fn()>>>>;

struct SwitchOnReply {
    inner: Controlled,
    change: SelectionChange,
}

impl Model for SwitchOnReply {
    fn name(&self) -> &str {
        "Switches selection inside respond"
    }

    fn respond(&self, request: Request, send: Sender<ModelEvent>) -> Cancel {
        if request.purpose == Purpose::Reply {
            self.change.borrow().as_ref().unwrap()();
        }
        self.inner.respond(request, send)
    }
}

#[test]
fn reply_and_title_capture_the_same_model_even_if_respond_changes_selection() {
    let (a, b) = (Controlled::default(), Controlled::default());
    let change = Rc::new(RefCell::new(None));
    let mut h = Harness::new(vec![
        ModelEntry::new("a", Rc::new(SwitchOnReply { inner: a.clone(), change: change.clone() })),
        ModelEntry::new("b", Rc::new(b.clone())),
    ]);
    let selection = h.read(|chat| chat.model);
    *change.borrow_mut() = Some(Box::new(move || selection.set(Some("b".into()))));
    h.send("first question");
    assert_eq!(h.read(|chat| untrack(|| chat.model.get())), Some("b".into()));
    assert_eq!(a.count(Purpose::Reply), 1);
    assert_eq!(a.count(Purpose::Title), 1);
    assert_eq!(b.count(Purpose::Title), 0);
    let thread = h.read(|chat| untrack(|| chat.current.get()).unwrap());
    h.run(move |chat| chat.stop(thread));
    assert!(a.cancelled(a.index(Purpose::Reply, 0).unwrap()));
    let answer = h.read(|chat| chat.thread_messages(thread).last().unwrap().id);
    h.run(move |chat| chat.regenerate(answer));
    assert_eq!(b.count(Purpose::Reply), 1);
    assert_eq!(b.count(Purpose::Title), 1);
    assert!(h.window.app().diagnostics().is_empty(), "{:?}", h.window.app().diagnostics());
}

#[test]
fn unavailable_models_preserve_drafts_files_and_existing_requests() {
    let mut h = Harness::new(vec![]);
    h.run(|chat| chat.attach(&["/tmp/model-choice-note.txt".into()]));
    h.send("keep the draft and attachment");
    assert_eq!(h.read(|chat| untrack(|| chat.current.get())), None);
    assert_eq!(h.read(|chat| untrack(|| chat.draft.get())), "keep the draft and attachment");
    assert_eq!(h.read(|chat| chat.files.keys().len()), 1);

    let a = Controlled::default();
    h.entries.borrow_mut().push(ModelEntry::new("a", Rc::new(a.clone())));
    h.window.app().hot_reload();
    h.run(|chat| {
        chat.model.set(Some("a".into()));
        chat.send();
    });
    assert_eq!(a.count(Purpose::Reply), 1);
    let thread = h.read(|chat| untrack(|| chat.current.get()).unwrap());
    let answer = h.read(|chat| chat.thread_messages(thread).last().unwrap().id);
    h.run(move |chat| {
        chat.model.set(Some("missing".into()));
        chat.regenerate(answer);
    });
    assert_eq!(h.read(|chat| chat.thread_messages(thread).last().unwrap().id), answer);
    assert_eq!(h.read(|chat| chat.thread_messages(thread).last().unwrap().status.clone()), Status::Streaming);
    assert!(!a.cancelled(a.index(Purpose::Reply, 0).unwrap()));
    h.run(move |chat| chat.stop(thread));
    assert!(a.cancelled(a.index(Purpose::Reply, 0).unwrap()), "Stop still targets the running model when the selection is unavailable");
    assert!(h.window.app().diagnostics().is_empty(), "{:?}", h.window.app().diagnostics());
}

#[test]
fn a_removed_queued_model_preserves_the_queue_and_active_request_until_explicitly_reselected() {
    let (a, b) = (Controlled::default(), Controlled::default());
    let mut h = Harness::new(vec![ModelEntry::new("a", Rc::new(a.clone())), ModelEntry::new("b", Rc::new(b.clone()))]);
    h.send("first on A");
    let thread = h.read(|chat| untrack(|| chat.current.get()).unwrap());
    h.run(|chat| chat.model.set(Some("b".into())));
    h.send("queued for B");
    let queued = h.read(|chat| {
        let id = chat.queue.keys()[0];
        untrack(|| chat.queue.item(&id).unwrap().get())
    });
    h.run(move |chat| chat.stop(thread));

    h.entries.borrow_mut().retain(|entry| entry.id != "b");
    h.window.app().hot_reload();
    h.run(|chat| chat.model.set(Some("a".into())));
    h.send("active after B was removed");
    let active = a.index(Purpose::Reply, 1).unwrap();
    let before = h.read(|chat| chat.thread_messages(thread).iter().map(|message| message.save()).collect::<Vec<_>>());
    let queued_id = queued.id;
    h.run(move |chat| chat.send_now(queued_id));
    assert_eq!(h.read(|chat| untrack(|| chat.queue.item(&queued_id).unwrap().get())), queued);
    assert_eq!(h.read(|chat| chat.thread_messages(thread).iter().map(|message| message.save()).collect::<Vec<_>>()), before);
    assert!(!a.cancelled(active), "Send now cannot cancel A when the queued model B is missing");
    assert_eq!(a.count(Purpose::Reply), 2, "the queued message must not fall back to selected model A");
    assert_eq!(b.count(Purpose::Reply), 0);

    h.run(move |chat| chat.edit_queued(queued_id));
    assert!(h.read(|chat| chat.queue.is_empty()));
    assert_eq!(h.read(|chat| untrack(|| chat.model.get())).as_deref(), Some("b"));
    assert_eq!(h.read(|chat| untrack(|| chat.draft.get())), "queued for B");
    h.run(Chat::send);
    assert_eq!(
        h.read(|chat| untrack(|| chat.draft.get())),
        "queued for B",
        "editing retains B's unavailable identity until explicitly changed"
    );
    assert!(!a.cancelled(active));

    h.run(|chat| {
        chat.model.set(Some("a".into()));
        chat.send();
    });
    assert_eq!(
        h.read(|chat| {
            let id = chat.queue.keys()[0];
            untrack(|| chat.queue.item(&id).unwrap().get()).model_id
        }),
        "a"
    );
    a.emit(active, ModelEvent::Done);
    h.window.app().poll();
    assert_eq!(a.count(Purpose::Reply), 3);
    assert_eq!(a.requests().last().unwrap().history.last().unwrap().1, "queued for B");
    assert_eq!(b.count(Purpose::Reply), 0);
    assert!(h.read(|chat| chat.queue.is_empty()));
    assert!(h.window.app().diagnostics().is_empty(), "{:?}", h.window.app().diagnostics());
}

#[test]
#[should_panic(expected = "model IDs must be nonempty and unique")]
fn duplicate_configuration_ids_are_rejected_before_building_the_picker() {
    let model = Rc::new(Controlled::default());
    Harness::new(vec![ModelEntry::new("same", model.clone()), ModelEntry::new("same", model)]);
}
