//! The chat app end to end, headless, with scripted and controlled models.

#[path = "../examples/support/controlled.rs"]
mod controlled;

use controlled::Controlled;

use octara::accesskit::{Action, NodeId};
use octara::app::{Event, InputEvent, Window};
use octara::ask::{Answer, Ask};
use octara::inspect::{self, Delivery};
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use octara::serde_json::json;
use octara_chat::engine::Purpose;
use octara_chat::model::Event as ModelEvent;
use octara_chat::store::Store;
use octara_chat::{ModelEntry, Options, root};
use std::time::Duration;

const LONG: Duration = Duration::from_secs(30);

fn open(options: impl Fn() -> Options + 'static) -> Window {
    let app = App::new(HeadlessPlatform::default());
    let options = std::rc::Rc::new(options);
    app.open_window(WindowOptions::new("Chat").size(1000.0, 720.0), move || {
        let o = options.clone();
        root(move || o())
    })
    .unwrap()
}

/// Two o'clock in the afternoon, some day.
const NOW: u64 = 1_800_000_000 - 1_800_000_000 % 86_400 + 14 * 3600;

fn options() -> Options {
    Options { clock: std::rc::Rc::new(|| NOW), ..Options::default() }
}

fn say(w: &mut Window, text: &str) {
    w.dispatch(Event::Focus("composer".into())).unwrap();
    w.dispatch(Event::Input("composer".into(), text.into())).unwrap();
    w.press_key("enter").unwrap();
}

fn text_of(w: &Window, id: &str) -> String {
    w.kernel().text_of(id).unwrap_or_default()
}

/// The ids of the open conversation's messages.
fn messages(w: &Window) -> Vec<String> {
    w.kernel().child_ids("transcript").into_iter().filter(|id| id.starts_with("msg-")).collect()
}

fn queued_rows(w: &Window) -> Vec<String> {
    w.kernel().child_ids("composer-area").into_iter().filter(|id| id.starts_with("queued-")).collect()
}

/// Every text under the element with test id `id`.
fn texts(w: &Window, id: &str) -> String {
    let k = w.kernel();
    let Some(n) = k.find(id) else { return String::new() };
    let mut out = Vec::new();
    let mut stack = vec![n];
    while let Some(n) = stack.pop() {
        if !k.text(n).is_empty() {
            out.push(k.text(n).to_owned());
        }
        stack.extend(k.children(n).iter().rev());
    }
    out.join(" ")
}

fn accessible_button(w: &Window, id: &str, name: &str) -> NodeId {
    let (target, node) = w.accessibility_tree().nodes.into_iter().find(|(_, node)| node.author_id() == Some(id)).unwrap();
    assert_eq!((node.role(), node.label()), (Role::Button, Some(name)));
    assert!(node.supports_action(Action::Click) && node.supports_action(Action::Focus));
    target
}

#[test]
fn the_welcome_card_has_a_visible_keyboard_focus_ring_and_activates() {
    let mut w = open(options);
    let id = "starter-build";
    let target = accessible_button(&w, id, "Build something");
    let theme = w.app().theme();
    assert_eq!(w.kernel().visual_of(id).unwrap().border, theme.get(Token::Border));

    w.invoke("chat.focus_composer").unwrap();
    w.press_key("shift+tab").unwrap();
    w.app().settle_motion();
    assert_eq!(w.accessibility_tree().focus, target);
    assert_eq!(w.kernel().visual_of(id).unwrap().border, theme.get(Token::Focus));
    w.press_key("enter").unwrap();
    assert_eq!(w.inspect().focus.as_deref(), Some("composer"));
    assert_eq!(
        w.kernel().value_of("composer").as_deref(),
        Some("Help me turn an idea into a small, practical project. Ask me about my idea first.")
    );
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
}

#[test]
fn attachments_with_the_same_name_have_independent_remove_buttons() {
    let mut w = open(options);
    w.tap("attach").unwrap();
    let asks = w.asks();
    assert!(matches!(asks.as_slice(), [(_, Ask::Paths(options))] if options.multiple), "{asks:?}");
    w.answer(asks[0].0, Answer::Paths(Some(vec!["/tmp/first/report.txt".into(), "/tmp/second/report.txt".into()]))).unwrap();

    let first = accessible_button(&w, "remove-file-1", "Remove report.txt");
    let second = accessible_button(&w, "remove-file-2", "Remove report.txt");
    assert_ne!(first, second);
    assert!(w.event(InputEvent::Accessibility { target: first, action: Action::Click, data: None }));
    assert!(!w.kernel().exists("file-1"));
    assert!(!w.kernel().exists("remove-file-1"));
    assert_eq!(accessible_button(&w, "remove-file-2", "Remove report.txt"), second);

    w.tap("remove-file-2").unwrap();
    assert!(!w.kernel().exists("file-2"));
    assert!(!w.kernel().exists("remove-file-2"));
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
}

#[test]
fn a_question_is_answered_as_it_streams() {
    let mut w = open(options);
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
    say(&mut w, "How do lists work?");
    assert_eq!(messages(&w).len(), 2, "the question and the answer to come");
    assert!(w.kernel().find("stop").is_some(), "while it streams, Stop replaces Send");
    w.app().advance(Duration::from_millis(40));
    assert!(texts(&w, "transcript").contains("Thinking"), "{}", texts(&w, "transcript"));
    assert!(w.app().run_until_idle(LONG).is_ok(), "{:?}", w.app().pending());
    let all = texts(&w, "transcript");
    assert!(all.contains("About \"How do lists work?\""), "{all}");
    assert!(all.contains("Thought for"), "the reasoning closed: {all}");
    assert!(w.kernel().find("send").is_some());
    assert_eq!(text_of(&w, "title"), "How do lists work", "named from the question");
    assert!(w.kernel().find("age-Today").is_some(), "listed under Today");
    let k = w.kernel();
    assert!(k.is_following_tail(k.find("transcript").unwrap()), "it followed the answer down");
    drop(k);
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
}

#[test]
fn what_is_sent_while_answering_waits_in_the_queue() {
    let mut w = open(options);
    say(&mut w, "first");
    w.app().advance(Duration::from_millis(100));
    say(&mut w, "second");
    let queued = queued_rows(&w);
    assert_eq!(queued.len(), 1, "queued while the first answer streams");
    assert_eq!(messages(&w).len(), 2);
    assert!(w.app().run_until_idle(LONG).is_ok());
    assert_eq!(messages(&w).len(), 4, "sent once the first answer came");
    assert!(w.kernel().find(&queued[0]).is_none());
}

#[test]
fn the_queue_follows_the_open_conversation_and_keeps_the_composer_mounted() {
    let model = Controlled::default();
    let engine = model.clone();
    let mut w = open(move || Options { models: vec![ModelEntry::new("controlled", std::rc::Rc::new(engine.clone()))], ..options() });
    let selected_thread = |w: &Window| {
        let kernel = w.kernel();
        kernel
            .child_ids("threads")
            .into_iter()
            .find(|id| kernel.state_of(id).is_some_and(|state| state.contains(State::SELECTED)))
            .expect("the open conversation is selected in the sidebar")
    };

    say(&mut w, "Question in B");
    let b = selected_thread(&w);
    w.invoke("chat.new").unwrap();
    say(&mut w, "Question in A");
    let a = selected_thread(&w);
    assert_ne!(a, b);
    let a_reply = model.index(Purpose::Reply, 1).unwrap();
    let b_reply = model.index(Purpose::Reply, 0).unwrap();

    say(&mut w, "A follow-up");
    let first = queued_rows(&w).into_iter().next().expect("A is answering, so its follow-up waits");
    let first_target = w.inspect().find(&first).unwrap().target.clone();
    say(&mut w, "A second follow-up");
    let a_queue = queued_rows(&w);
    assert_eq!(a_queue.len(), 2);
    assert_eq!(model.count(Purpose::Reply), 2);

    w.type_text("Keep this draft");
    w.set_selection(4, 0);
    let before = w.inspect();
    assert_eq!(before.find(&first).unwrap().target, first_target, "inserting a sibling keeps the existing row");
    let composer = &before.find("composer").unwrap().target;
    let id = first.strip_prefix("queued-").unwrap();
    let actions = ["send-now", "edit-queued", "unqueue"].map(|action| format!("{action}-{id}"));
    let remove = &actions[2];
    let stale_remove = json!({"op": "tap", "id": remove, "target": before.find(remove).unwrap().target}).to_string();

    for (thread, rows) in [(b.as_str(), &[][..]), (a.as_str(), a_queue.as_slice())] {
        w.dispatch(Event::Press(thread.into())).unwrap();
        assert_eq!(selected_thread(&w), thread);
        assert_eq!(queued_rows(&w).as_slice(), rows);
        let current = w.inspect();
        assert_eq!(&current.find("composer").unwrap().target, composer);
        assert_eq!(current.focus.as_deref(), Some("composer"));
        let field = w.text_field().unwrap();
        assert_eq!((field.text.as_str(), field.selection, field.reversed), ("Keep this draft", 0..4, true));
        for action in &actions {
            if rows.is_empty() {
                assert!(current.find(action).is_none(), "{action} belongs to the closed queue");
            } else {
                assert_ne!(current.find(action).unwrap().target, before.find(action).unwrap().target);
            }
        }
        let reply = inspect::handle_line(&mut w, &stale_remove);
        assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"), "{reply:?}");
        assert_eq!(reply.delivery, Some(Delivery::Refused), "{reply:?}");
        if thread == b {
            assert!(model.emit(a_reply, ModelEvent::Text("Still answering in A".into())));
            w.app().poll();
        }
    }
    assert!(texts(&w, "transcript").contains("Still answering in A"));
    assert_ne!(w.inspect().find(&first).unwrap().target, first_target, "returning mounts a new row target");
    w.tap(&actions[0]).unwrap();
    assert_eq!(queued_rows(&w).as_slice(), &a_queue[1..]);
    assert!(model.cancelled(a_reply));
    assert!(!model.cancelled(b_reply));
    assert_eq!(model.count(Purpose::Reply), 3);
    assert_eq!(model.requests().last().unwrap().history.last().unwrap().1, "A follow-up");
    assert!(w.layout_issues().is_empty(), "{:?}", w.layout_issues());
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
    w.app().shutdown();
}

#[test]
fn stopping_keeps_what_came_and_regenerating_answers_again() {
    let mut w = open(options);
    say(&mut w, "tell me something long");
    w.app().advance(Duration::from_millis(300));
    w.tap("stop").unwrap();
    assert!(texts(&w, "transcript").contains("Stopped"));
    let ids = messages(&w);
    let answer = ids[1].trim_start_matches("msg-").to_owned();
    assert!(w.app().run_until_idle(LONG).is_ok());
    w.tap(&format!("regenerate-{answer}")).unwrap();
    assert!(w.app().run_until_idle(LONG).is_ok());
    let ids = messages(&w);
    assert_eq!(ids.len(), 2, "the question once, a new answer: {ids:?}");
    assert!(!texts(&w, "transcript").contains("Stopped"));
    assert!(texts(&w, "transcript").contains("About \"tell me something long\""), "the replacement must finish after its old row unmounts");
    assert!(w.kernel().find("send").is_some(), "regeneration must leave the streaming state");
    assert!(w.app().diagnostics().is_empty(), "{:?}", w.app().diagnostics());
}

#[test]
fn a_failure_offers_a_retry() {
    let mut w = open(options);
    say(&mut w, "please fail");
    assert!(w.app().run_until_idle(LONG).is_ok());
    let answer = messages(&w)[1].trim_start_matches("msg-").to_owned();
    assert_eq!(w.kernel().role_of(&format!("failed-{answer}")), Some(Role::Alert));
    w.tap(&format!("retry-{answer}")).unwrap();
    assert!(w.app().run_until_idle(LONG).is_ok());
    assert_eq!(messages(&w).len(), 2);
    let answer = messages(&w)[1].trim_start_matches("msg-").to_owned();
    assert!(w.kernel().find(&format!("failed-{answer}")).is_some(), "retry must receive the replacement model's failure");
    assert!(w.kernel().find("send").is_some());
}

#[test]
fn a_tool_call_shows_its_state_and_detail() {
    let mut w = open(options);
    say(&mut w, "What is the weather?");
    assert!(w.app().run_until_idle(LONG).is_ok());
    let answer = messages(&w)[1].trim_start_matches("msg-").to_owned();
    let trigger = format!("tool-{answer}-1-trigger");
    assert!(texts(&w, &trigger).contains("weather"));
    assert!(texts(&w, &trigger).contains("Done"));
    w.tap(&trigger).unwrap();
    assert!(texts(&w, &format!("tool-{answer}-1-content")).contains("Oslo"));
    assert!(texts(&w, "transcript").contains("4 °C"));
}

#[test]
fn editing_a_question_replaces_it_and_what_followed() {
    let mut w = open(options);
    say(&mut w, "one");
    assert!(w.app().run_until_idle(LONG).is_ok());
    say(&mut w, "two");
    assert!(w.app().run_until_idle(LONG).is_ok());
    let first = messages(&w)[0].trim_start_matches("msg-").to_owned();
    w.dispatch(Event::Press(format!("edit-{first}"))).unwrap();
    assert_eq!(w.kernel().value_of("composer").as_deref(), Some("one"));
    say(&mut w, "uno");
    assert!(w.app().run_until_idle(LONG).is_ok());
    let all = texts(&w, "transcript");
    assert_eq!(messages(&w).len(), 2, "{all}");
    assert!(all.contains("uno") && !all.contains("\"two\""), "{all}");
}

#[test]
fn conversations_are_renamed_and_deleted_from_their_menu() {
    let mut w = open(options);
    say(&mut w, "hello there");
    assert!(w.app().run_until_idle(LONG).is_ok());
    let row = w.kernel().child_ids("threads").into_iter().find(|id| id.starts_with("thread-")).unwrap();
    let id = row.trim_start_matches("thread-").to_owned();
    w.secondary_tap(&row).unwrap();
    let (ask, _) = w.asks()[0].clone();
    w.answer(ask, Answer::Menu(Some("rename".into()))).unwrap();
    w.app().settle();
    let field = format!("rename-{id}");
    assert!(w.kernel().state_of(&field).is_some_and(|s| s.contains(State::FOCUSED)), "the field took focus");
    w.dispatch(Event::Input(field.clone(), "Greetings".into())).unwrap();
    w.set_selection(4, 4);
    for (key, selection) in [("home", 0..0), ("end", 9..9), ("shift+home", 0..9)] {
        w.press_key(key).unwrap();
        assert_eq!(w.inspect().focus.as_deref(), Some(field.as_str()));
        assert_eq!(w.text_field().unwrap().selection, selection);
    }
    w.press_key("enter").unwrap();
    assert_eq!(text_of(&w, "title"), "Greetings");

    w.secondary_tap(&row).unwrap();
    let (menu, _) = w.asks()[0].clone();
    w.answer(menu, Answer::Menu(Some("delete".into()))).unwrap();
    w.app().settle();
    let (confirm, ask) = w.asks()[0].clone();
    assert!(matches!(ask, Ask::Prompt { .. }));
    w.answer(confirm, Answer::Prompt(Some(1))).unwrap();
    w.app().settle();
    assert!(w.kernel().find(&row).is_none());
    assert_eq!(text_of(&w, "title"), "New chat");
    assert!(messages(&w).is_empty());
}

#[test]
fn an_answer_streams_on_while_another_conversation_is_open() {
    let mut w = open(options);
    say(&mut w, "first conversation");
    w.app().advance(Duration::from_millis(200));
    w.invoke("chat.new").unwrap();
    assert!(messages(&w).is_empty());
    assert!(w.kernel().state_of("composer").is_some_and(|s| s.contains(State::FOCUSED)));
    say(&mut w, "second conversation");
    assert!(w.app().run_until_idle(LONG).is_ok());
    let rows: Vec<String> = w.kernel().child_ids("threads").into_iter().filter(|id| id.starts_with("thread-")).collect();
    assert_eq!(rows.len(), 2);
    let other = rows.iter().find(|r| texts(&w, r).contains("first")).unwrap().clone();
    w.tap(&other).unwrap();
    let all = texts(&w, "transcript");
    assert!(all.contains("About \"first conversation\""), "it finished while away: {all}");
}

#[test]
fn conversations_are_kept_on_disk() {
    let dir = std::env::temp_dir().join(format!("octara-chat-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    {
        let d = dir.clone();
        let mut w = open(move || Options { store: Some(Store::new(&d)), ..options() });
        say(&mut w, "remember this");
        assert!(w.app().run_until_idle(LONG).is_ok());
        w.app().settle();
    }
    let d = dir.clone();
    let mut w = open(move || Options { store: Some(Store::new(&d)), ..options() });
    let row = w.kernel().child_ids("threads").into_iter().find(|id| id.starts_with("thread-")).expect("loaded");
    w.tap(&row).unwrap();
    assert!(texts(&w, "transcript").contains("remember this"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_validate_cancel_save_restore_and_survive_restart() {
    use octara_chat::settings::Generation;
    let path = std::env::temp_dir().join(format!(
        "octara-settings-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let store = Store::new(&path);
    let saved = store.clone();
    let mut w = open(move || Options { store: Some(saved.clone()), ..options() });
    w.tap("settings-open").unwrap();
    w.dispatch(Event::Input("settings-temperature".into(), "oops".into())).unwrap();
    assert!(text_of(&w, "settings-error").contains("number"));
    w.tap("settings-cancel").unwrap();
    assert!(store.load_preferences().is_none());
    w.tap("settings-open").unwrap();
    w.dispatch(Event::Input("settings-temperature".into(), "0.7".into())).unwrap();
    w.dispatch(Event::Input("settings-tokens".into(), "2048".into())).unwrap();
    w.tap("settings-save").unwrap();
    let expected = Generation { temperature: Some(0.7), max_completion_tokens: Some(2048) };
    assert_eq!(store.load_preferences().unwrap().generation, expected);
    w.app().shutdown();
    let saved = store.clone();
    let mut reopened = open(move || Options { store: Some(saved.clone()), ..options() });
    reopened.tap("settings-open").unwrap();
    reopened.tap("settings-save").unwrap();
    assert_eq!(store.load_preferences().unwrap().generation, expected);
    reopened.tap("settings-open").unwrap();
    reopened.tap("settings-reset").unwrap();
    reopened.tap("settings-save").unwrap();
    assert_eq!(store.load_preferences().unwrap().generation, Generation::default());
    assert!(reopened.app().diagnostics().is_empty(), "{:?}", reopened.app().diagnostics());
    assert!(reopened.layout_issues().is_empty(), "{:?}", reopened.layout_issues());
    reopened.app().shutdown();
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn failed_settings_save_keeps_the_previous_configuration_and_invalid_files_use_defaults() {
    use octara_chat::settings::Generation;
    let path = std::env::temp_dir().join(format!(
        "octara-settings-failure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("settings.json"), r#"{"generation":{"temperature":9}}"#).unwrap();
    let store = Store::new(&path);
    assert!(store.load_preferences().is_none());
    std::fs::remove_file(path.join("settings.json")).unwrap();
    // A directory in place of the destination makes atomic rename fail on every platform.
    std::fs::create_dir(path.join("settings.json")).unwrap();
    let saved = store.clone();
    let mut w = open(move || Options { store: Some(saved.clone()), ..options() });
    w.tap("settings-open").unwrap();
    w.dispatch(Event::Input("settings-temperature".into(), "0.8".into())).unwrap();
    w.tap("settings-save").unwrap();
    assert!(text_of(&w, "settings-error").contains("could not be saved"));
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1, "failed commits remove their temporary file");
    w.tap("settings-cancel").unwrap();
    std::fs::remove_dir(path.join("settings.json")).unwrap();
    w.tap("settings-open").unwrap();
    w.tap("settings-save").unwrap();
    assert_eq!(store.load_preferences().unwrap().generation, Generation::default());
    assert!(w.app().diagnostics().is_empty());
    w.app().shutdown();
    std::fs::remove_dir_all(path).unwrap();
}
