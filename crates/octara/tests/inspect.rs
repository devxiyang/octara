//! The Inspector protocol: snapshots of the semantic tree, requests, and scripts of them.

use octara::inspect::{self, Request};
use octara::prelude::*;

fn view() -> View {
    let count = state("count", || 0);
    let name = state("name", String::new);
    col()
        .size_full()
        .p_4()
        .gap_2()
        .child(text(move || format!("Count: {}", count.get())).id("count"))
        .child(el().id("inc").label("Increment").p_2().child("+1").on_press(move || count.update(|n| *n += 1)))
        .child(text_input().id("name").label("Name").on_input(move |v| name.set(v)))
        .child(text(move || format!("Hello {}", name.get())).id("greeting"))
        .into()
}

#[test]
fn a_snapshot_is_the_semantic_tree() {
    let app = App::new(view);
    let snap = app.inspect();
    assert_eq!(snap.schema, "octara.inspect/1");
    assert_eq!(snap.theme, "light");
    let inc = snap.find("inc").unwrap();
    assert_eq!(inc.name.as_deref(), Some("Increment"));
    assert!(inc.actions.contains(&"press".to_string()), "{:?}", inc.actions);
    assert!(inc.source.as_deref().is_some_and(|s| s.contains("tests/inspect.rs")), "{:?}", inc.source);
    assert!(inc.bounds[2] > 0.0 && inc.bounds[3] > 0.0);
    assert_eq!(snap.find("count").unwrap().name.as_deref(), Some("Count: 0"));
    assert_eq!(snap.find("name").unwrap().value.as_deref(), Some(""));
    // It round-trips as JSON, absent fields left out.
    let json = snap.to_json();
    assert!(!json.contains("\"checked\":null"));
    assert_eq!(serde_json::from_str::<inspect::Snapshot>(&json).unwrap(), snap);
}

#[test]
fn requests_drive_the_app() {
    let mut app = App::new(view);
    assert!(inspect::handle(&mut app, Request::Tap { id: "inc".into() }).ok);
    assert_eq!(app.kernel().text_of("count").as_deref(), Some("Count: 1"));
    let r = inspect::handle_line(&mut app, r#"{"op":"type","id":"name","text":"Ada"}"#);
    assert!(r.ok, "{r:?}");
    assert_eq!(app.kernel().text_of("greeting").as_deref(), Some("Hello Ada"));
    let r = inspect::handle_line(&mut app, r#"{"op":"inspect"}"#);
    assert_eq!(r.snapshot.unwrap().focus.as_deref(), Some("name"));
}

#[test]
fn failures_name_their_code() {
    let mut app = App::new(view);
    let r = inspect::handle_line(&mut app, r#"{"op":"tap","id":"nope"}"#);
    assert_eq!(r.code.as_deref(), Some("NO_SUCH_ELEMENT"));
    let r = inspect::handle_line(&mut app, r#"{"op":"tap"}"#);
    assert_eq!(r.code.as_deref(), Some("BAD_REQUEST"));
    let r = inspect::handle_line(&mut app, r#"{"op":"fly","id":"inc"}"#);
    assert_eq!(r.code.as_deref(), Some("BAD_REQUEST"));
    let r = inspect::handle_line(&mut app, r#"{"op":"expect","id":"count","text":"Count: 9"}"#);
    assert_eq!(r.code.as_deref(), Some("EXPECTATION_FAILED"));
    assert_eq!(r.actual, Some("Count: 0".into()));
    let r = inspect::handle_line(&mut app, r#"{"op":"type","text":"x"}"#);
    assert_eq!(r.code.as_deref(), Some("STEP_FAILED"), "nothing focused to type into");
}

#[test]
fn a_script_runs_line_by_line_and_stops_at_the_first_failure() {
    let mut app = App::new(view);
    let script = r#"
        // Count up twice, then check.
        {"op":"tap","id":"inc"}
        {"op":"tap","id":"inc"}
        {"op":"expect","id":"count","text":"Count: 2"}
        {"op":"expect","clean":true}
        {"op":"expect","id":"count","text":"Count: 3"}
        {"op":"tap","id":"inc"}
    "#;
    let replies = inspect::run_script(&mut app, script);
    assert_eq!(replies.len(), 5, "stops after the failing expect");
    assert!(replies[..4].iter().all(|(_, r)| r.ok));
    let (line, last) = &replies[4];
    assert_eq!((*line, last.code.as_deref()), (7, Some("EXPECTATION_FAILED")));
    assert_eq!(app.kernel().text_of("count").as_deref(), Some("Count: 2"), "the step after the failure did not run");
}

fn clean_view() -> View {
    col()
        .size_full()
        .p_4()
        .child(text("Settings").id("title"))
        .child(el().id("save").min_w(44.0).min_h(44.0).p_2().child("Save").on_press(|| {}))
        .into()
}

#[test]
fn check_runs_every_theme_and_size() {
    let report = inspect::check(clean_view, &inspect::CheckOptions::default());
    assert_eq!(report.cells, ["light/phone", "light/desktop", "dark/phone", "dark/desktop"]);
    assert!(report.clean, "{}", report.to_json());
}

fn findings(view: fn() -> View) -> inspect::CheckReport {
    inspect::check(view, &inspect::CheckOptions::default())
}

#[test]
fn check_finds_accessibility_problems() {
    // An icon-only button: no name, and small on a phone.
    let r = findings(|| col().size_full().child(el().id("close").size_4().bg(Token::Surface).on_press(|| {})).into());
    let small = r.findings.iter().find(|f| f.code == "SMALL_TARGET").expect("small on a phone");
    assert_eq!(small.cells, ["light/phone", "dark/phone"], "touch sizes only");
    let unnamed = r.findings.iter().find(|f| f.code == "UNNAMED_CONTROL").unwrap();
    assert_eq!(unnamed.id.as_deref(), Some("close"));
    assert_eq!(unnamed.cells.len(), 4, "merged across all cells");
    assert!(!unnamed.fix.is_empty() && unnamed.doc.contains('#'), "the code's fix and doc come along");
    assert!(!r.clean);

    // Muted gray on gray.
    let r = findings(|| col().size_full().bg(rgb(0x777777)).child(text("faint").id("faint").text_color(rgb(0x888888))).into());
    let low = r.findings.iter().find(|f| f.code == "LOW_CONTRAST").unwrap();
    assert_eq!(low.id.as_deref(), Some("faint"));

    // Over a gradient the background is not one color: not judged.
    let r = findings(|| {
        col()
            .size_full()
            .bg(linear_gradient(90.0, linear_color_stop(rgb(0xffffff), 0.0), linear_color_stop(rgb(0x000000), 1.0)))
            .child(text("on a gradient").id("hero").text_color(rgb(0xeeeeee)))
            .into()
    });
    assert!(!r.findings.iter().any(|f| f.code == "LOW_CONTRAST"), "{}", r.to_json());
}

#[test]
fn accepted_findings_need_a_reason_and_leave_the_report_clean() {
    let view = || col().size_full().child(el().id("dot-1").size_4().label("Page 1").on_press(|| {})).into();
    let opts = inspect::CheckOptions {
        accept: vec![inspect::Accept {
            code: "SMALL_TARGET".into(),
            id: Some("dot-*".into()),
            why: "page dots; swiping pages works too".into(),
        }],
        ..Default::default()
    };
    let r = inspect::check(view, &opts);
    assert!(r.clean, "{}", r.to_json());
    assert_eq!(r.accepted[0].why.as_deref(), Some("page dots; swiping pages works too"));
}
