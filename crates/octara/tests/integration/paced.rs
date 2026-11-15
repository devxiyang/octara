//! Paced text: a `TextBuffer` revealing what is appended to another over a few frames.

use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use octara::task::WorkKind;
use std::time::Duration;

const FRAME: Duration = Duration::from_micros(16_667);

/// An app showing `paced(source)` as the text `shown`.
fn setup() -> (App, Window, TextBuffer) {
    let app = App::new(HeadlessPlatform::default());
    let source = app.run(|| TextBuffer::new("Hi. "));
    let s = source.clone();
    let w = app.open_window(WindowOptions::new("Paced"), move || text(paced(&s)).id("shown").into()).unwrap();
    (app, w, source)
}

fn shown(w: &Window) -> String {
    let k = w.kernel();
    k.text(k.find("shown").unwrap()).to_owned()
}

#[test]
fn a_burst_shows_over_a_few_frames() {
    let (app, w, source) = setup();
    assert_eq!(shown(&w), "Hi. ", "what is there at first shows at once");
    let burst = "word ".repeat(120);
    app.run(|| source.append(&burst));
    app.settle();
    let first = shown(&w).len();
    assert!(first > 4 && first < 4 + burst.len(), "a share shows at once, not all: {first}");
    assert!(app.pending().iter().any(|p| p.kind == WorkKind::Motion && p.label == "paced text"), "{:?}", app.pending());
    app.advance(FRAME);
    let second = shown(&w).len();
    assert!(second > first, "the next frame shows more");
    app.advance(Duration::from_millis(250));
    assert_eq!(shown(&w), format!("Hi. {burst}"), "a burst has shown within about a quarter of a second");
    assert!(app.run_until_idle(Duration::from_secs(1)).is_ok());
    assert!(!app.pending().iter().any(|p| p.kind == WorkKind::Motion && p.label == "paced text"), "caught up: no longer motion");
}

#[test]
fn a_trickle_shows_as_it_comes() {
    let (app, w, source) = setup();
    for t in ["a", "b", "c"] {
        app.run(|| source.append(t));
        app.settle();
    }
    assert_eq!(shown(&w), "Hi. abc");
}

#[test]
fn edits_other_than_appends_and_reduce_motion_show_at_once() {
    let (app, w, source) = setup();
    app.run(|| source.append(&"x".repeat(300)));
    app.settle();
    app.run(|| source.set("Replaced."));
    app.settle();
    assert_eq!(shown(&w), "Replaced.");

    app.set_reduced_motion(true);
    app.run(|| source.append(&" y".repeat(300)));
    app.settle();
    assert_eq!(shown(&w), app.run(|| untrack(|| source.get())));
}

#[test]
fn settling_motion_lands_it_and_characters_stay_whole() {
    let (app, w, source) = setup();
    let burst = "文字和 emoji 🙂 ".repeat(40);
    app.run(|| source.append(&burst));
    app.settle();
    for _ in 0..3 {
        app.advance(FRAME);
        assert!(app.run(|| untrack(|| source.get())).starts_with(&shown(&w)));
    }
    app.settle_motion();
    assert_eq!(shown(&w), app.run(|| untrack(|| source.get())));
}

fn selectable_setup(initial: &'static str) -> (App, Window, TextBuffer) {
    let app = App::new(HeadlessPlatform::default());
    let source = app.run(|| TextBuffer::new(initial));
    let stream = source.clone();
    let window = app
        .open_window(WindowOptions::new("Paced selection"), move || {
            col().size_full().overflow_y_scroll().selectable().child(text(paced(&stream)).id("shown")).into()
        })
        .unwrap();
    (app, window, source)
}

#[test]
fn a_unicode_burst_keeps_the_selection() {
    const PREFIX: &str = "已选😀 ";
    let (app, mut window, source) = selectable_setup(PREFIX);
    window.select(("shown", 0), ("shown", "已选😀".len())).unwrap();
    let node = window.kernel().find("shown").unwrap();
    let burst = "中文🙂abc ".repeat(40);
    app.run(|| source.append(&burst));
    app.settle();
    let complete = format!("{PREFIX}{burst}");
    assert!(shown(&window).len() < complete.len(), "the burst still has a paced tail");
    let mut previous = PREFIX.len();
    for _ in 0..64 {
        let current = shown(&window);
        assert!(current.len() > previous, "each step reveals more text");
        assert!(complete.starts_with(&current), "the prefix ends at a whole UTF8 character");
        assert_eq!(window.kernel().find("shown"), Some(node));
        assert_eq!(window.selected_text().as_deref(), Some("已选😀"));
        if current == complete {
            break;
        }
        previous = current.len();
        app.advance(FRAME);
    }
    assert_eq!(shown(&window), complete, "paced steps eventually show every byte in order");
    window.press_key("mod+c").unwrap();
    assert_eq!(app.platform().read_text().as_deref(), Some("已选😀"));
    assert!(!app.pending().iter().any(|pending| pending.kind == WorkKind::Motion && pending.label == "paced text"));
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn enabling_reduce_motion_lands_an_existing_backlog_on_the_next_tick() {
    let (app, window, source) = setup();
    let burst = "中🙂\n".repeat(40);
    app.run(|| source.append(&burst));
    app.settle();
    assert!(shown(&window).len() < app.run(|| untrack(|| source.get())).len());
    app.set_reduced_motion(true);
    app.advance(FRAME);
    assert_eq!(shown(&window), app.run(|| untrack(|| source.get())), "enabling Reduce Motion also lands an existing backlog");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}
