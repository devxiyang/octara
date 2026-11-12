//! The chat app end to end, headless, answered by the scripted model on the app's clock.

use octara::app::{Event, Window};
use octara::ask::{Answer, Ask};
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use octara_chat::store::Store;
use octara_chat::{Options, root};
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
    let queued: Vec<String> = w.kernel().child_ids("composer-area").into_iter().filter(|id| id.starts_with("queued-")).collect();
    assert_eq!(queued.len(), 1, "queued while the first answer streams");
    assert_eq!(messages(&w).len(), 2);
    assert!(w.app().run_until_idle(LONG).is_ok());
    assert_eq!(messages(&w).len(), 4, "sent once the first answer came");
    assert!(w.kernel().find(&queued[0]).is_none());
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
