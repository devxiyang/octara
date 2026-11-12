//! Input as data and frames out ([core](../../../../docs/design/core.md)): every input is
//! an `InputEvent`, recorded with its time and window and replayed to the same state; each
//! operation presents a `Frame` to the window's platform; the protocol names windows.

use octara::app::Recording;
use octara::inspect::handle_line;
use octara::platform::{HeadlessWindow, WindowOptions};
use octara::prelude::*;
use std::time::Duration;

/// A list with a field, an add button and a delayed counter.
fn todos() -> View {
    let items = list("items", Vec::<(u32, String)>::new, |t: &(u32, String)| t.0);
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
        .child(text_input().id("draft").label("New").value(move || draft.get()).on_input(move |v| draft.set(v)).on_submit(add))
        .child(el().id("add").role(Role::Button).label("Add").size_8().on_press(add))
        .child(el().id("later").role(Role::Button).label("Later").size_8().on_press(move || {
            spawn(async move {
                sleep(Duration::from_millis(300)).await;
                ticks.update(|t| *t += 1);
            });
        }))
        .child(text(move || format!("{} later", ticks.get())).id("ticks"))
        .child(col().id("list").child(each(move || items.source(), |_, t| text(move || t.with(|t| t.1.clone())).into())))
        .into()
}

/// What a window shows, for comparing two runs: its tree and its display list (font data
/// handles are numbered per text system, so they are left out).
fn shown(w: &Window) -> (String, String) {
    let list = format!("{:?}", w.display_list().items);
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

#[test]
fn each_operation_presents_a_frame() {
    let mut w = Window::headless(|| {
        col()
            .child(text_input().id("name").label("Name").secure().value("").on_input(|_| {}))
            .child(web_view("https://example.com").id("web").label("Page"))
            .into()
    });
    let window = |w: &Window| {
        let p = w.platform_window();
        let h = p.as_any().downcast_ref::<HeadlessWindow>().unwrap();
        (h.frames, h.native.len(), h.text_input)
    };
    let (frames, native, input) = window(&w);
    assert!(frames >= 1 && native == 2, "the web view was created and placed: {frames} frames, {native} changes");
    assert_eq!(input, None, "no field has focus");

    w.tap("name").unwrap();
    let (after_tap, _, input) = window(&w);
    assert!(after_tap > frames);
    assert!(input.is_some_and(|t| t.secure), "the focused secure field: {input:?}");

    // Nothing changed: nothing is presented.
    w.app().advance(Duration::from_secs(10));
    let (idle, _, _) = window(&w);
    w.app().advance(Duration::from_secs(10));
    assert_eq!(window(&w).0, idle, "an idle app presents nothing");
}

#[test]
fn requests_name_their_window() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let mut a = app.open_window(WindowOptions::new("A"), || text("in A").id("label").into()).unwrap();
    let b = app.open_window(WindowOptions::new("B"), || text("in B").id("label").into()).unwrap();
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
