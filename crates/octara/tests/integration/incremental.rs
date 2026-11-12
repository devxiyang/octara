//! Incremental frames ([core](../../../../docs/design/core.md#incremental-frames)): each node's
//! own drawing is kept until what it is drawn from changes, and accessibility compares the
//! places of what a layout or a motion may have moved, not the whole tree.

use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use std::time::Duration;

/// Per frame while text streams into the last of `n` paragraphs: own drawings built, glyph runs
/// built, accessibility places compared and nodes sent.
fn stream(n: usize, motion: bool) -> (f64, f64, f64, f64) {
    let app = App::new(HeadlessPlatform::default());
    let live = app.run(|| LiveText::new("Streaming"));
    let tail = live.clone();
    let mut w = app
        .open_window(WindowOptions::new("Chat"), move || {
            let c = col()
                .id("list")
                .size_full()
                .overflow_y_scroll()
                .gap_2()
                .children(
                    (0..n).map(|i| text(format!("Message {i} with a few words that wrap over the window's width at times.")).w(300.0)),
                )
                .child(text(&tail).id("tail").w(300.0));
            if motion { c.motion(Spring::SNAP).into() } else { c.into() }
        })
        .unwrap();
    w.resize(400.0, 600.0);
    let _ = w.accessibility_update();
    // Settle what entered, at the end.
    w.dispatch(Event::ScrollTo("list".into(), 0.0, 1.0e6)).unwrap();
    app.advance(Duration::from_secs(2));
    let _ = w.display_list();
    let _ = w.accessibility_update();
    let (d0, c0) = (w.kernel().drawings(), w.kernel().a11y_compared());
    let (mut sent, frames) = (0, 40);
    for i in 0..frames {
        app.run(|| live.append(&format!(" word{i}")));
        // Following the stream, as a chat does.
        w.dispatch(Event::ScrollTo("list".into(), 0.0, 1.0e6)).unwrap();
        app.advance(Duration::from_millis(16));
        let _ = w.display_list();
        sent += w.accessibility_update().nodes.len();
    }
    let (d1, c1) = (w.kernel().drawings(), w.kernel().a11y_compared());
    let f = frames as f64;
    ((d1.0 - d0.0) as f64 / f, (d1.2 - d0.2) as f64 / f, (c1 - c0) as f64 / f, sent as f64 / f)
}

#[test]
fn a_stream_builds_its_tail_not_the_window() {
    let (small, large) = (stream(5, false), stream(80, false));
    // The growing text alone is drawn again: its runs, whatever the others.
    assert!(small.0 <= 2.0 && large.0 <= 2.0, "drawings built per frame: {small:?} {large:?}");
    assert_eq!(small.1, large.1, "glyph runs built per frame grow with the tail, not the window");
    // Accessibility sends the text that changed (and its runs).
    assert!(large.3 <= 4.0, "nodes sent per frame: {large:?}");
}

#[test]
fn moving_layout_keeps_drawings_and_culls() {
    let still = stream(80, false);
    let moving = stream(80, true);
    assert!(moving.0 <= 4.0, "a moving layout moves kept drawings: {moving:?}");
    assert!(moving.1 <= still.1 + 1.0, "{moving:?} {still:?}");
}

#[test]
fn a_moved_subtree_is_sent_at_its_root() {
    let mut w = Window::headless(|| {
        let shown = state("shown", || false);
        col()
            .child(button_like("toggle", move || shown.update(|s| *s = !*s)))
            .child(show(move || shown.get(), || text("Inserted above").id("new").into()))
            .child(col().id("card").child(text("A").id("a")).child(text("B").id("b")))
            .into()
    });
    let _ = w.accessibility_update();
    w.tap("toggle").unwrap();
    let u = w.accessibility_update();
    let k = w.kernel();
    let id = |t: &str| octara::kernel::a11y::a11y_id(k.find(t).unwrap());
    let sent: Vec<_> = u.nodes.iter().map(|(i, _)| *i).collect();
    assert!(sent.contains(&id("card")), "the card moved down");
    assert!(!sent.contains(&id("a")) && !sent.contains(&id("b")), "its texts keep their place in it");
}

fn button_like(id: &'static str, f: impl Fn() + 'static) -> El {
    el().id(id).size_8().role(Role::Button).label("Toggle").on_press(f)
}
