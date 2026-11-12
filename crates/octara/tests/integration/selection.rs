//! Document selection ([text core](../../../../docs/design/text.md) step 2): the texts of a
//! selectable region selected as one document by drag, word, block, Shift, `mod+a`; copied as
//! text; rows of a virtual list keep their part while they scroll out and back, and copy reads
//! the rows that are not mounted.

use octara::app::{Modifiers, Phase, PointerEvent};
use octara::inspect::{Request, handle};
use octara::paint::Item;
use octara::prelude::*;
use std::time::Duration;

fn note() -> View {
    col()
        .id("region")
        .selectable()
        .w(400.0)
        .child(text("alpha one").id("a"))
        .child(text("beta two").id("b"))
        .child(row().select_none().child(text("•").id("marker")))
        .child(text("gamma three").id("c"))
        .into()
}

fn center(w: &Window, id: &str) -> (f32, f32) {
    let k = w.kernel();
    let (x, y, _, h) = k.shown_rect(k.find(id).unwrap());
    (x + 2.0, y + h / 2.0)
}

fn highlighted(w: &Window, id: &str) -> bool {
    let n = w.kernel().find(id).unwrap();
    let accent = Theme::light().get(Token::Accent).multiply_alpha(0.3);
    w.display_list().items().iter().any(|i| matches!(i, Item::Quad(q) if q.source == n && q.fill == Some(accent)))
}

#[test]
fn texts_select_as_one_document() {
    let mut w = Window::headless(note);
    w.select(("a", 6), ("c", 5)).unwrap();
    assert_eq!(w.selected_text().as_deref(), Some("one\nbeta two\ngamma"), "the marker is not selected");
    assert!(highlighted(&w, "a") && highlighted(&w, "b") && highlighted(&w, "c"));
    assert!(!highlighted(&w, "marker"));
    // Backwards is the same selection.
    w.select(("c", 5), ("a", 6)).unwrap();
    assert_eq!(w.selected_text().as_deref(), Some("one\nbeta two\ngamma"));
    w.press_key("escape").unwrap();
    assert_eq!(w.selected_text(), None, "Escape clears it");
    assert!(!highlighted(&w, "a"));
}

#[test]
fn the_pointer_selects_words_blocks_and_ranges() {
    let mut w = Window::headless(note);
    let (ax, ay) = center(&w, "a");
    let (cx, cy) = center(&w, "c");
    // Press and drag from the start of `a` into `c`.
    w.pointer_down(ax, ay);
    w.pointer_move(cx + 20.0, cy);
    w.pointer_up(cx + 20.0, cy);
    let text = w.selected_text().unwrap();
    assert!(text.starts_with("alpha one\nbeta two\ng"), "{text:?}");
    // A double press selects a word, a triple press the whole text.
    w.click_at(ax + 2.0, ay);
    w.click_at(ax + 2.0, ay);
    assert_eq!(w.selected_text().as_deref(), Some("alpha"));
    w.click_at(ax + 2.0, ay);
    assert_eq!(w.selected_text().as_deref(), Some("alpha one"));
    // Shift extends it.
    w.app().advance(Duration::from_secs(1));
    let shift = |phase| InputEvent::Pointer(PointerEvent { modifiers: Modifiers::SHIFT, ..PointerEvent::mouse(phase, cx + 200.0, cy) });
    w.event(shift(Phase::Down));
    w.event(shift(Phase::Up));
    assert_eq!(w.selected_text().as_deref(), Some("alpha one\nbeta two\ngamma three"));
    // `mod+c` copies; `mod+a` selects the region.
    w.press_key("mod+c").unwrap();
    assert_eq!(w.app().platform().read_text().as_deref(), Some("alpha one\nbeta two\ngamma three"));
    w.clear_selection();
    w.pointer_move(ax, ay);
    w.press_key("mod+a").unwrap();
    assert_eq!(w.selected_text().as_deref(), Some("alpha one\nbeta two\ngamma three"));
    // A press outside the region ends it.
    w.click_at(700.0, 500.0);
    assert_eq!(w.selected_text(), None);
}

#[test]
fn rows_join_with_blank_lines_and_appends_keep_the_selection() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let live = app.run(|| TextBuffer::new("streaming"));
    let stream = live.clone();
    let mut w = app
        .open_window(octara::platform::WindowOptions::new("Chat"), move || {
            let items =
                state_list("msgs", || vec![(1u32, "first message".to_owned()), (2, "second message".to_owned())], |m: &(u32, String)| m.0);
            col()
                .id("chat")
                .selectable()
                .child(each(move || items.source(), |k, m| text(move || m.with(|m| m.1.clone())).id(format!("m{k}")).into()))
                .child(text(&stream).id("live"))
                .into()
        })
        .unwrap();
    w.select(("m1", 6), ("live", 6)).unwrap();
    assert_eq!(w.selected_text().as_deref(), Some("message\n\nsecond message\n\nstream"));
    app.run(|| live.append(" more words"));
    app.settle();
    assert_eq!(w.selected_text().as_deref(), Some("message\n\nsecond message\n\nstream"), "an append after the end keeps it");
}

#[test]
fn a_selection_through_a_virtual_list_survives_scrolling_and_copies_every_row() {
    let mut w = Window::headless(|| {
        let items = state_list("rows", || (0..1000u32).map(|i| (i, format!("Message {i}"))).collect::<Vec<_>>(), |m: &(u32, String)| m.0);
        col()
            .size_full()
            .child(col().id("list").selectable().flex_1().overflow_y_scroll().child(virtual_each(
                move || items.source(),
                |_| 24.0,
                |k, m| text(move || m.with(|m| m.1.clone())).id(format!("m{k}")).h(24.0).into(),
            )))
            .into()
    });
    // From row 3 …
    w.select(("m3", 0), ("m3", 0)).unwrap();
    // … scroll to row 900 and Shift+press its end.
    w.dispatch(Event::ScrollTo("list".into(), 0.0, 900.0 * 24.0 - 100.0)).unwrap();
    assert!(w.kernel().find("m3").is_none(), "row 3 is not mounted now");
    // Rows measured shorter than their estimate: bring row 900 into view where it really is.
    let (_, y) = center(&w, "m900");
    w.dispatch(Event::Scroll("list".into(), 0.0, y - 200.0)).unwrap();
    let (x, y) = center(&w, "m900");
    let shift = |phase| InputEvent::Pointer(PointerEvent { modifiers: Modifiers::SHIFT, ..PointerEvent::mouse(phase, x + 300.0, y) });
    w.event(shift(Phase::Down));
    w.event(shift(Phase::Up));
    assert!(highlighted(&w, "m899") && highlighted(&w, "m900"));
    assert!(!highlighted(&w, "m901"));
    // Back to the top: row 3 draws its part again, row 2 none.
    w.dispatch(Event::ScrollTo("list".into(), 0.0, 0.0)).unwrap();
    assert!(highlighted(&w, "m3") && !highlighted(&w, "m2"));
    // Copy reads every row, mounted or not.
    let text = w.selected_text().unwrap();
    let rows: Vec<&str> = text.split("\n\n").collect();
    assert_eq!(rows.len(), 898, "rows 3 to 900");
    assert_eq!((rows[0], rows[897]), ("Message 3", "Message 900"));
    // Select all, through the protocol: every row.
    let reply = handle(&mut w, Request::Select { from: None, to: None, region: Some("list".into()) });
    assert!(reply.ok, "{reply:?}");
    let reply = handle(&mut w, Request::Copy);
    assert_eq!(reply.text.as_deref().map(|t| t.split("\n\n").count()), Some(1000));
}
