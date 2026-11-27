//! Provider selection uses stable configuration IDs and snapshots each request.

#[path = "../examples/support/controlled.rs"]
mod controlled;
#[path = "support/session.rs"]
mod session;

use controlled::Controlled;
use octara::prelude::*;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::{Event, Status};
use octara_chat::store::Store;
use octara_chat::{ModelEntry, Options, Queued};
use session::Session;
use std::cell::Cell;
use std::rc::Rc;

fn open(models: Vec<ModelEntry>, store: Option<Store>) -> Session {
    Session::new(Options { models, store, ..Options::default() }, |_, _| el().size(rel(1.0)).into())
}

fn send(session: &Session, text: &str) -> Option<u64> {
    session.update(|chat, w| {
        w.set(chat.draft, text.into());
        chat.send(w);
        w.get(chat.current)
    })
}

#[test]
fn persisted_selection_survives_reordering_and_missing_ids_never_fall_back() {
    let a = Controlled::default();
    let b = Controlled::default();
    let a_entry = ModelEntry::new("a", Rc::new(a.clone()));
    let b_entry = ModelEntry::new("b", Rc::new(b.clone()));
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path());
    let ui = open(vec![a_entry.clone(), b_entry.clone()], Some(store.clone()));
    ui.update(|chat, w| chat.configure(w, Some("b".into()), Default::default()).unwrap());
    drop(ui);

    let ui = open(vec![b_entry, a_entry.clone()], Some(store.clone()));
    assert_eq!(ui.update(|chat, w| w.get(chat.model)).as_deref(), Some("b"));
    send(&ui, "Remember the second model");
    assert_eq!(b.count(Purpose::Reply), 1);
    assert_eq!(b.requests()[0].history.last().unwrap().1, "Remember the second model");
    assert_eq!(a.count(Purpose::Reply), 0);
    drop(ui);

    let ui = open(vec![a_entry], Some(store));
    assert!(!ui.update(|chat, w| w.read(|r| chat.has_model(r))));
    assert_eq!(send(&ui, "keep this draft"), None);
    assert_eq!(ui.update(|chat, w| w.get(chat.draft)), "keep this draft");
    assert_eq!(a.count(Purpose::Reply), 0);
    ui.update(|chat, w| {
        w.set(chat.model, Some("a".into()));
        chat.send(w);
    });
    assert_eq!(a.count(Purpose::Reply), 1);
    ui.clean();
}

struct SwitchOnReply {
    inner: Controlled,
    selection: Rc<Cell<Option<Signal<Option<String>>>>>,
}

impl Model for SwitchOnReply {
    fn name(&self) -> &str {
        "Switch selection during respond"
    }

    fn respond(&self, w: &mut WriteCx<'_>, scope: ScopeId, request: Request, send: Sender<Event>) -> Cancel {
        if request.purpose == Purpose::Reply {
            w.set(self.selection.get().unwrap(), Some("b".into()));
        }
        self.inner.respond(w, scope, request, send)
    }
}

#[test]
fn reply_and_title_capture_the_same_model_even_when_respond_changes_selection() {
    let (a, b) = (Controlled::default(), Controlled::default());
    let selection = Rc::new(Cell::new(None));
    let ui = open(
        vec![
            ModelEntry::new("a", Rc::new(SwitchOnReply { inner: a.clone(), selection: selection.clone() })),
            ModelEntry::new("b", Rc::new(b.clone())),
        ],
        None,
    );
    selection.set(Some(ui.chat.model));
    let thread = send(&ui, "first question").unwrap();
    assert_eq!(ui.update(|chat, w| w.get(chat.model)).as_deref(), Some("b"));
    assert_eq!(a.count(Purpose::Reply), 1);
    assert_eq!(a.count(Purpose::Title), 1);
    assert_eq!(b.count(Purpose::Title), 0);
    ui.update(|chat, w| {
        chat.stop(w, thread);
        let answer = w.read(|r| chat.thread_messages(r, thread)).last().unwrap().id;
        chat.regenerate(w, answer);
    });
    assert!(a.cancelled(a.index(Purpose::Reply, 0).unwrap()));
    assert_eq!(b.count(Purpose::Reply), 1);
    assert_eq!(b.count(Purpose::Title), 1);
    ui.clean();
}

#[test]
fn unavailable_models_preserve_drafts_files_and_existing_requests() {
    let a = Controlled::default();
    let ui = open(vec![ModelEntry::new("a", Rc::new(a.clone()))], None);
    ui.update(|chat, w| {
        w.set(chat.model, Some("missing".into()));
        chat.attach(w, &["note.txt".into()]);
    });
    assert_eq!(send(&ui, "keep this draft and attachment"), None);
    assert_eq!(ui.update(|chat, w| w.len(chat.files)), 1);
    ui.update(|chat, w| {
        w.set(chat.model, Some("a".into()));
        chat.send(w);
        let thread = w.get(chat.current).unwrap();
        let id = w.read(|r| chat.thread_messages(r, thread)).last().unwrap().id;
        w.set(chat.model, Some("missing".into()));
        chat.regenerate(w, id);
        let last = w.read(|r| chat.thread_messages(r, thread)).pop().unwrap();
        assert_eq!(last.id, id);
        assert_eq!(last.status, Status::Streaming);
        assert!(!a.cancelled(a.index(Purpose::Reply, 0).unwrap()));
        chat.stop(w, thread);
    });
    assert!(a.cancelled(a.index(Purpose::Reply, 0).unwrap()));
    ui.clean();
}

#[test]
fn unavailable_queued_model_requires_an_explicit_reselection() {
    let a = Controlled::default();
    let ui = open(vec![ModelEntry::new("a", Rc::new(a.clone()))], None);
    let thread = send(&ui, "first on A").unwrap();
    let active = a.index(Purpose::Reply, 0).unwrap();
    ui.update(|chat, w| {
        let queued = Queued {
            id: 99,
            thread,
            text: "Send this later".into(),
            files: Vec::new(),
            model_id: "removed".into(),
            generation: Default::default(),
        };
        w.push_row(chat.queue, 99, queued.clone());
        chat.send_now(w, 99);
        assert_eq!(w.get_row(chat.queue, &99), Some(queued));
        assert!(!a.cancelled(active));
        chat.edit_queued(w, 99);
        assert_eq!(w.get(chat.model).as_deref(), Some("removed"));
        chat.send(w);
        assert_eq!(w.get(chat.draft), "Send this later");
        w.set(chat.model, Some("a".into()));
        chat.send(w);
    });
    a.emit(active, Event::Done);
    ui.settle();
    assert_eq!(a.count(Purpose::Reply), 2);
    assert_eq!(a.requests().last().unwrap().history.last().unwrap().1, "Send this later");
    assert_eq!(ui.update(|chat, w| w.len(chat.queue)), 0);
    ui.clean();
}

#[test]
#[should_panic(expected = "model IDs must be nonempty and unique")]
fn duplicate_configuration_ids_fail_at_construction() {
    let model = Rc::new(Controlled::default());
    open(vec![ModelEntry::new("same", model.clone()), ModelEntry::new("same", model)], None);
}
