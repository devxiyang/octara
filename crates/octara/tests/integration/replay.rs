//! Input as data and frames out ([core](../../../../docs/design/core.md)): every input is
//! an `InputEvent`, recorded with its time and window and replayed to the same state; each
//! operation presents a `Frame` to the window's platform; the protocol names windows.

use octara::app::{Event, InputEvent, Recording};
use octara::inspect::handle_line;
use octara::kernel::TextDocument;
use octara::platform::{HeadlessWindow, TextInput, WindowOptions};
use octara::prelude::*;
use octara::view::web_view;
use octara_primitives::text::TextEdit;
use std::time::Duration;

/// A list with a field, an add button and a delayed counter.
fn todos() -> View {
    let items = state_list("items", Vec::<(u32, String)>::new, |t: &(u32, String)| t.0);
    let draft = state("draft", String::new);
    let next = state("next", || 1u32);
    let ticks = state("ticks", || 0u32);
    let add = move || {
        let text = draft.get();
        if !text.is_empty() {
            items.push((next.get(), text));
            next.set(next.get() + 1);
            draft.set(String::new());
        }
    };
    col()
        .child(input().id("draft").label("New").value(move || draft.get()).on_input(move |v| draft.set(v)).on_submit(add))
        .child(el().id("add").role(Role::Button).label("Add").size_8().on_press(add))
        .child(el().id("later").role(Role::Button).label("Later").size_8().on_press(move || {
            spawn(async move {
                sleep(Duration::from_millis(300)).await;
                ticks.update(|t| *t += 1);
            });
        }))
        .child(text(move || format!("{} later", ticks.get())).id("ticks"))
        .child(col().id("list").child(each(move || items.read_only(), |_, t| text(move || t.with(|t| t.1.clone())))))
        .into()
}

/// What a window shows, for comparing two runs: its tree and its display list (font data
/// handles are numbered per text system, so they are left out).
fn shown(w: &Window) -> (String, String) {
    let list = format!("{:?}", w.display_list().items());
    let mut out = String::new();
    let mut rest = list.as_str();
    while let Some(i) = rest.find("Blob { id: ") {
        out.push_str(&rest[..i + "Blob { id: ".len()]);
        rest = rest[i + "Blob { id: ".len()..].trim_start_matches(|c: char| c.is_ascii_digit());
        out.push('_');
    }
    out.push_str(rest);
    (w.kernel().tree_dump(), out)
}

#[test]
fn a_recorded_session_replays_to_the_same_state() {
    let mut a = Window::headless(todos);
    a.app().start_recording();
    a.tap("draft").unwrap();
    a.type_text("milk");
    a.press_key("enter").unwrap();
    a.tap("draft").unwrap();
    a.type_text("eggs");
    a.tap("add").unwrap();
    a.tap("later").unwrap();
    a.app().advance(Duration::from_millis(400));
    a.wheel(10.0, 10.0, 0.0, 30.0);
    let recording = a.app().stop_recording();
    assert!(recording.events.len() > 8, "the helpers send events: {}", recording.events.len());
    assert_eq!(a.kernel().text_of("ticks").as_deref(), Some("1 later"));

    // Through JSON, as an agent or a bug report would keep it.
    let json = serde_json::to_string(&recording).unwrap();
    let back: Recording = serde_json::from_str(&json).unwrap();
    assert_eq!(back, recording);

    let b = Window::headless(todos);
    b.app().replay(&back).unwrap();
    assert_eq!(shown(&b), shown(&a), "the replay ends where the session did");
}

fn native_input(w: &Window) -> TextInput {
    w.platform_window().as_any().downcast_ref::<HeadlessWindow>().unwrap().text_input.clone().unwrap()
}

fn native_editor() -> View {
    let value = state("value", || "A😀Z".to_owned());
    let proposals = state("proposals", Vec::<String>::new);
    let submitted = state("submitted", String::new);
    col()
        .child(
            input()
                .id("field")
                .w(240)
                .value(value)
                .on_input(move |proposal| {
                    value.set(proposal.replace('!', ""));
                    proposals.update(|values| values.push(proposal));
                })
                .on_submit(move || submitted.set(value.get())),
        )
        .child(text(move || proposals.with(|values| values.join(" | "))).id("proposals"))
        .child(text(submitted).id("submitted"))
        .into()
}

#[test]
fn native_text_recording_replays_selection_composition_and_submit() {
    let mut w = Window::headless(native_editor);
    w.app().start_recording();
    w.dispatch(Event::Focus("field".into())).unwrap();
    let input = native_input(&w);
    let owner = input.id;
    assert!(w.apply_text_selection(owner, &input.document.unwrap(), 1..5));
    let selected = native_input(&w).document.unwrap();
    assert_eq!(selected.selection(), Some(1..5));

    let mark = TextEdit::Mark { text: "ni😀".into(), selection: Some(6..6), replacement: None };
    assert!(w.apply_text_edit(owner, &selected, &mark));
    let composing = native_input(&w).document.unwrap();
    assert_eq!(composing.text(), "Ani😀Z");
    assert_eq!(composing.marked(), Some(1..7));
    assert!(w.apply_text_selection(owner, &composing, 3..7));
    let composing = native_input(&w).document.unwrap();
    assert_eq!(composing.selection(), Some(3..7));
    assert_eq!(w.text_field().unwrap().text, "A😀Z");

    let insert = TextEdit::Insert { text: "你!".into(), replacement: None };
    assert!(w.apply_text_edit(owner, &composing, &insert));
    let confirmed = native_input(&w).document.unwrap();
    let mut proposed = composing;
    proposed.apply(&insert);
    assert_eq!(proposed.text(), "A你!Z");
    assert_eq!(confirmed.text(), "A你Z");
    let stale = TextEdit::Insert { text: "late".into(), replacement: None };
    assert!(!w.apply_text_edit(owner, &proposed, &stale));
    assert!(!w.apply_text_selection(owner, &proposed, 0..0));

    let mark = TextEdit::Mark { text: "好".into(), selection: None, replacement: Some(5..5) };
    assert!(w.apply_text_edit(owner, &confirmed, &mark));
    let composing = native_input(&w).document.unwrap();
    assert!(w.apply_text_edit(owner, &composing, &TextEdit::Unmark));
    w.press_key("enter").unwrap();
    assert_eq!(w.kernel().text_of("proposals").as_deref(), Some("A你!Z | A你Z好"));
    assert_eq!(w.kernel().text_of("submitted").as_deref(), Some("A你Z好"));
    let final_document = native_input(&w).document.unwrap();
    assert_eq!(final_document.selection(), Some(8..8));
    assert_eq!(final_document.marked(), None);

    let recording = w.app().stop_recording();
    let native_events =
        recording.events.iter().filter(|r| matches!(r.event, InputEvent::NativeTextEdit { .. } | InputEvent::NativeTextSelection { .. }));
    assert_eq!(native_events.count(), 6, "each admitted native operation is recorded once");
    let json = serde_json::to_string(&recording).unwrap();
    let round_trip: Recording = serde_json::from_str(&json).unwrap();
    assert_eq!(round_trip, recording);

    let replayed = Window::headless(native_editor);
    replayed.app().replay(&round_trip).unwrap();
    assert_eq!(native_input(&replayed).document, Some(final_document));
    assert_eq!(shown(&replayed), shown(&w));
}

#[test]
fn native_text_replay_requires_the_recorded_owner_and_document() {
    let fields = || col().child(input().id("first").w(240).value("A😀Z")).child(input().id("second").w(240).value("A😀Z"));
    let mut w = Window::headless(fields);
    w.dispatch(Event::Focus("first".into())).unwrap();
    w.set_selection(0, 0);
    let input = native_input(&w);
    let original = input.document.unwrap();
    w.app().start_recording();
    assert!(w.apply_text_selection(input.id, &original, 1..5));
    let recording = w.app().stop_recording();

    let mut replayed = Window::headless(fields);
    replayed.dispatch(Event::Focus("second".into())).unwrap();
    replayed.set_selection(0, 0);
    let other = native_input(&replayed);
    assert_ne!(other.id, input.id);
    assert_eq!(other.document.as_ref(), Some(&original));
    assert!(replayed.app().replay(&recording).is_err());
    assert_eq!(native_input(&replayed).document, other.document);

    replayed.dispatch(Event::Focus("first".into())).unwrap();
    replayed.set_selection(6, 6);
    let diverged = native_input(&replayed);
    assert_eq!(diverged.id, input.id);
    assert!(replayed.app().replay(&recording).is_err());
    assert_eq!(native_input(&replayed).document, diverged.document);
}

#[test]
fn rejected_native_operations_on_secure_fields_are_omitted() {
    let mut w = Window::headless(|| input().id("password").secure().w(240).value("test secret"));
    w.dispatch(Event::Focus("password".into())).unwrap();
    let input = native_input(&w);
    assert!(input.secure);
    assert!(input.document.is_none());
    let selection = w.text_field().unwrap().selection;
    let expected = TextDocument::new("test secret".into(), selection.start, selection.end);
    let insert = TextEdit::Insert { text: "more secret".into(), replacement: None };
    w.app().start_recording();
    assert!(!w.apply_text_edit(input.id, &expected, &insert));
    assert!(!w.apply_text_selection(input.id, &expected, 0..0));
    assert!(w.app().stop_recording().events.is_empty());
}

#[test]
fn each_operation_presents_a_frame() {
    let mut w = Window::headless(|| {
        col()
            .child(input().id("name").label("Name").secure().value("").on_input(|_| {}))
            .child(web_view("https://example.com").id("web").label("Page"))
    });
    let window = |w: &Window| {
        let p = w.platform_window();
        let h = p.as_any().downcast_ref::<HeadlessWindow>().unwrap();
        (h.frames, h.native.len(), h.text_input.clone())
    };
    let (frames, native, input) = window(&w);
    assert!(frames >= 1 && native == 2, "the web view was created and placed: {frames} frames, {native} changes");
    assert_eq!(input, None, "no field has focus");

    w.tap("name").unwrap();
    let (after_tap, _, input) = window(&w);
    assert!(after_tap > frames);
    assert!(input.as_ref().is_some_and(|t| t.secure), "the focused secure field: {input:?}");

    // Nothing changed (no field focused: no caret blinks): nothing is presented.
    w.press_key("escape").unwrap();
    w.app().advance(Duration::from_secs(10));
    let (idle, _, _) = window(&w);
    w.app().advance(Duration::from_secs(10));
    assert_eq!(window(&w).0, idle, "an idle app presents nothing");
}

#[test]
fn requests_name_their_window() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let mut a = app.open_window(WindowOptions::new("A"), || text("in A").id("label")).unwrap();
    let b = app.open_window(WindowOptions::new("B"), || text("in B").id("label")).unwrap();
    let snapshot = a.inspect();
    assert_eq!(snapshot.windows.iter().map(|w| w.title.as_str()).collect::<Vec<_>>(), ["A", "B"]);
    assert_eq!(snapshot.window, a.id().number());

    let line = format!(r#"{{"op":"expect","id":"label","text":"in B","window":{}}}"#, b.id().number());
    assert!(handle_line(&mut a, &line).ok, "the request ran in window B");
    let line = r#"{"op":"expect","id":"label","text":"in A"}"#;
    assert!(handle_line(&mut a, line).ok, "without a window, the one it was given");
    let line = r#"{"op":"inspect","window":12345}"#;
    assert_eq!(handle_line(&mut a, line).code.as_deref(), Some("BAD_REQUEST"));
}
