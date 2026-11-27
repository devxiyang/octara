//! The real model picker keeps search selection and retained composer state independent.

#[allow(dead_code)]
#[path = "../examples/support/controlled.rs"]
mod controlled;
#[path = "support/inspect.rs"]
mod inspect;
#[path = "support/session.rs"]
mod session;

use controlled::Controlled;
use octara::prelude::*;
use octara_chat::engine::{Cancel, Model, Purpose, Request};
use octara_chat::model::Event;
use octara_chat::{ModelEntry, Options, view};
use session::Session;
use std::rc::Rc;

struct Named {
    name: &'static str,
    engine: Controlled,
}

impl Model for Named {
    fn name(&self) -> &str {
        self.name
    }

    fn respond(&self, w: &mut WriteCx<'_>, owner: ScopeId, request: Request, send: Sender<Event>) -> Cancel {
        self.engine.respond(w, owner, request, send)
    }
}

fn model(id: &str, name: &'static str) -> (ModelEntry, Controlled) {
    let engine = Controlled::default();
    (ModelEntry::new(id, Rc::new(Named { name, engine: engine.clone() })), engine)
}

fn open(models: Vec<ModelEntry>) -> Session {
    Session::new(Options { models, ..Options::default() }, view)
}

fn choose(ui: &Session, id: &str) {
    assert!(ui.press("model"));
    assert!(ui.press(&format!("model-option-{id}")));
}

#[test]
fn chinese_search_selects_same_named_models_by_id_without_consuming_the_draft() {
    let (first, a) = model("first", "聊天模型");
    let (second, b) = model("second", "聊天模型");
    let (writer, _) = model("writer", "Writer");
    let ui = open(vec![first, writer, second]);
    ui.command("chat.focus_composer");
    ui.text("Keep this draft 😀");
    let composer = ui.node("composer").target;
    assert!(ui.press("model"));
    assert_eq!(ui.node("model").role, Some(Role::ComboBox));
    assert_eq!(ui.snapshot().focus, Some(ui.node("model-search").target));
    ui.text("聊天");
    assert!(!ui.exists("model-option-writer"));
    assert!(ui.exists("model-option-first"));
    assert!(ui.press("model-option-second"));
    assert!(!ui.exists("model-popover"));
    assert_eq!(ui.node("composer").target, composer);
    assert_eq!(ui.update(|chat, w| w.get(chat.draft)), "Keep this draft 😀");
    ui.command("chat.focus_composer");
    ui.key(Key::Enter, Modifiers::empty());
    assert_eq!(a.count(Purpose::Reply), 0);
    assert_eq!(b.count(Purpose::Reply), 1);
    choose(&ui, "first");
    assert!(ui.press("stop"));
    assert!(b.cancelled(b.index(Purpose::Reply, 0).unwrap()));
    ui.clean();
}

#[test]
fn queue_shows_the_original_model_and_send_now_uses_it() {
    let (alpha, a) = model("alpha", "Alpha");
    let (beta, b) = model("beta", "Beta");
    let ui = open(vec![alpha, beta]);
    ui.command("chat.focus_composer");
    ui.text("First message");
    ui.key(Key::Enter, Modifiers::empty());
    choose(&ui, "beta");
    ui.command("chat.focus_composer");
    ui.text("Queued message");
    ui.key(Key::Enter, Modifiers::empty());
    let id = ui.update(|chat, w| w.keys(chat.queue)[0]);
    let label = format!("queued-model-{id}");
    assert_eq!(ui.node(&label).text.as_deref(), Some("Queued · Beta"));
    choose(&ui, "alpha");
    assert_eq!(ui.node(&label).text.as_deref(), Some("Queued · Beta"));
    assert!(ui.press(&format!("send-now-{id}")));
    assert_eq!(a.count(Purpose::Reply), 1);
    assert_eq!(b.count(Purpose::Reply), 1);
    assert!(!ui.exists(&label));
    ui.clean();
}

#[test]
fn no_models_keeps_the_draft_and_composer_usable() {
    let ui = open(Vec::new());
    ui.resize(320.0, 640.0);
    assert!(ui.exists("model-status"));
    assert!(!ui.press("model"));
    ui.command("chat.focus_composer");
    ui.text("Draft with no provider");
    ui.key(Key::Enter, Modifiers::empty());
    assert!(!ui.press("send"));
    assert_eq!(ui.update(|chat, w| w.get(chat.draft)), "Draft with no provider");
    ui.clean();
}
