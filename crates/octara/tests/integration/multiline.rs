//! Multi-line text inputs: wrapping and growing with the text, Enter breaking lines and
//! Cmd+Enter submitting, moving by visual lines with a goal x, line edges, selections across
//! lines, pointer placement on a line, paste keeping line breaks, and the caret's real line.
//! The monospace engine makes every position exact: 8 px a character, 24 px a line.

use octara::app::{MemoryClipboard, Phase, Platform, PointerEvent};
use octara::kernel::text::MonoEngine;
use octara::prelude::*;

fn app(initial: &'static str, width: f32) -> App {
    let mut app = App::new(move || {
        let v = state("v", move || initial.to_owned());
        let sent = state("sent", || 0u32);
        col()
            .size_full()
            .items_start()
            .child(
                text_input()
                    .id("area")
                    .label("Notes")
                    .multiline()
                    .text_size(px(16.0))
                    .line_height(px(24.0))
                    .w(px(width))
                    .value(move || v.get())
                    .on_input(move |s| v.set(s))
                    .on_submit(move || sent.update(|n| *n += 1)),
            )
            .child(text(move || format!("sent {}", sent.get())).id("sent"))
            .into()
    });
    app.kernel_mut().set_text_engine(Box::new(MonoEngine));
    app.set_platform(Platform::MacOs);
    app.tap("area").unwrap();
    app
}

fn value(app: &App) -> String {
    app.kernel().value_of("area").unwrap()
}
fn caret(app: &App) -> usize {
    let f = app.text_field().unwrap();
    if f.reversed { f.selection.start } else { f.selection.end }
}
fn place(app: &mut App, at: usize) {
    app.set_selection(at, at);
}
fn height(app: &App) -> f32 {
    let k = app.kernel();
    k.rect(k.find("area").unwrap()).3
}

#[test]
fn wraps_grows_and_breaks_lines() {
    // "aaaa bbbb " fits 80 px (10 characters); "cccc dd" goes on the next line.
    let mut app = app("aaaa bbbb cccc dd", 80.0);
    assert_eq!(height(&app), 48.0, "two lines");
    place(&mut app, 17);
    app.press_key("enter").unwrap();
    assert_eq!(value(&app), "aaaa bbbb cccc dd\n");
    assert_eq!(height(&app), 72.0, "a third, empty line");
    app.type_text("x");
    assert_eq!(app.text_field().unwrap().caret.1 - app.kernel().rect(app.kernel().find("area").unwrap()).1, 48.0, "the caret is on line 3");
    // Shift+Enter breaks lines too; Cmd+Enter submits.
    app.press_key("shift+enter").unwrap();
    assert!(value(&app).ends_with("x\n"));
    assert_eq!(app.kernel().text_of("sent").unwrap(), "sent 0");
    app.press_key("mod+enter").unwrap();
    assert_eq!(app.kernel().text_of("sent").unwrap(), "sent 1");
    let snap = app.inspect();
    assert_eq!(snap.find("area").unwrap().role, "multiline_text_input");
}

#[test]
fn moves_by_visual_lines_keeping_its_x() {
    let mut app = app("abcdefgh\nab\nabcdefgh", 200.0);
    place(&mut app, 6);
    app.press_key("down").unwrap();
    assert_eq!(caret(&app), 11, "the short line's end");
    app.press_key("down").unwrap();
    assert_eq!(caret(&app), 18, "back at x 48 on the long line");
    app.press_key("up").unwrap();
    app.press_key("up").unwrap();
    assert_eq!(caret(&app), 6, "and back");
    app.press_key("up").unwrap();
    assert_eq!(caret(&app), 0, "above the first line: the start");
    // Any other move forgets the goal x.
    place(&mut app, 6);
    app.press_key("down").unwrap();
    app.press_key("left").unwrap();
    app.press_key("down").unwrap();
    assert_eq!(caret(&app), 13, "x 8 now, not 48");
    // Shift+Down selects across lines.
    place(&mut app, 2);
    app.press_key("shift+down").unwrap();
    assert_eq!(app.text_field().unwrap().selection, 2..11);
}

#[test]
fn line_edges_on_a_wrapped_line() {
    let mut app = app("aaaa bbbb cccc dd", 80.0);
    place(&mut app, 12);
    app.press_key("cmd+left").unwrap();
    assert_eq!(caret(&app), 10, "start of the second visual line");
    app.press_key("cmd+right").unwrap();
    assert_eq!(caret(&app), 17, "its end");
    place(&mut app, 2);
    app.press_key("cmd+right").unwrap();
    assert_eq!(caret(&app), 9, "the end of the first visual line, before the space that wraps it");
    app.press_key("cmd+down").unwrap();
    assert_eq!(caret(&app), 17, "the text's end");
}

#[test]
fn selection_rectangles_pointer_paste() {
    let mut app = app("aaaa bbbb cccc dd", 80.0);
    app.set_selection(2, 12);
    let n = app.kernel().find("area").unwrap();
    let (x, y, ..) = app.kernel().rect(n);
    let dl = app.display_list();
    let sel: Vec<_> = dl.quads().filter(|q| q.source == n && q.rect.h == 24.0).collect();
    assert_eq!(sel.len(), 2, "one rectangle per line");
    assert_eq!((sel[0].rect.x - x, sel[0].rect.w), (16.0, 64.0), "2..10 on line 1");
    assert_eq!((sel[1].rect.x - x, sel[1].rect.y - y, sel[1].rect.w), (0.0, 24.0, 16.0), "10..12 on line 2");

    // A click on the second line places the caret there.
    let at = (x + 4.0 * 8.0 + 1.0, y + 24.0 + 12.0);
    app.pointer(PointerEvent::mouse(Phase::Down, at.0, at.1));
    app.pointer(PointerEvent::mouse(Phase::Up, at.0, at.1));
    assert_eq!(caret(&app), 14);

    // A paste keeps its line breaks.
    app.set_clipboard(Box::new(MemoryClipboard(Some("1\r\n2".into()))));
    place(&mut app, 0);
    app.press_key("mod+v").unwrap();
    assert!(value(&app).starts_with("1\n2"), "{:?}", value(&app));
}

fn rows_app(chat: bool) -> App {
    let mut app = App::new(move || {
        let v = state("v", String::new);
        let sent = state("sent", || 0u32);
        let field = text_input()
            .id("area")
            .label("Notes")
            .text_size(px(16.0))
            .line_height(px(24.0))
            .w(px(200.0))
            .value(move || v.get())
            .on_input(move |s| v.set(s))
            .on_submit(move || sent.update(|n| *n += 1));
        let field = if chat { field.submit_on_enter() } else { field.rows(2, 4) };
        col().size_full().items_start().child(field).child(text(move || format!("sent {}", sent.get())).id("sent")).into()
    });
    app.kernel_mut().set_text_engine(Box::new(MonoEngine));
    app.set_platform(Platform::MacOs);
    app.tap("area").unwrap();
    app
}

#[test]
fn rows_bound_the_height_and_the_text_scrolls_inside() {
    let mut app = rows_app(false);
    assert_eq!(height(&app), 48.0, "two lines at least");
    app.type_text("1");
    for c in ["2", "3", "4", "5", "6"] {
        app.press_key("enter").unwrap();
        app.type_text(c);
    }
    assert_eq!(height(&app), 96.0, "four lines at most");
    let n = app.kernel().find("area").unwrap();
    let (_, y, _, h) = app.kernel().rect(n);
    let caret = app.text_field().unwrap().caret;
    assert!(caret.1 >= y && caret.1 + caret.3 <= y + h, "the caret's line shows: {caret:?} in {y}..{}", y + h);
    assert_eq!(app.kernel().scroll_offset(n), (0.0, 48.0), "two lines scrolled away");
    // A wheel scrolls the text, away from the caret, and it stays there.
    let (x, y, ..) = app.kernel().rect(n);
    app.wheel(x + 10.0, y + 10.0, 0.0, -30.0);
    assert_eq!(app.kernel().scroll_offset(n), (0.0, 18.0));
    let _ = app.display_list();
    assert_eq!(app.kernel().scroll_offset(n), (0.0, 18.0), "drawing does not pull it back");
    // Typing brings the caret back.
    app.type_text("!");
    assert_eq!(app.kernel().scroll_offset(n), (0.0, 48.0));
}

#[test]
fn a_growing_field_keeps_its_caret_in_view_in_a_scroll_container() {
    let mut app = App::new(|| {
        let v = state("v", String::new);
        col()
            .id("page")
            .h(px(100.0))
            .w(px(300.0))
            .overflow_y_scroll()
            .child(el().h(px(60.0)))
            .child(text_input().id("area").label("Notes").multiline().line_height(px(24.0)).w(px(200.0)).value(move || v.get()).on_input(move |s| v.set(s)))
            .into()
    });
    app.kernel_mut().set_text_engine(Box::new(MonoEngine));
    app.tap("area").unwrap();
    for _ in 0..5 {
        app.press_key("enter").unwrap();
    }
    let page = app.kernel().find("page").unwrap();
    let (_, py, _, ph) = app.kernel().rect(page);
    let caret = app.text_field().unwrap().caret;
    assert!(app.kernel().scroll_offset(page).1 > 0.0, "the page scrolled");
    assert!(caret.1 >= py && caret.1 + caret.3 <= py + ph + 0.01, "the caret shows: {caret:?} in {py}..{}", py + ph);
}

#[test]
fn a_chat_composer_submits_on_enter() {
    let mut app = rows_app(true);
    app.type_text("hi");
    app.press_key("shift+enter").unwrap();
    app.type_text("there");
    assert_eq!(value(&app), "hi\nthere");
    assert_eq!(height(&app), 48.0, "it grows");
    app.press_key("enter").unwrap();
    assert_eq!(app.kernel().text_of("sent").unwrap(), "sent 1");
    assert_eq!(value(&app), "hi\nthere", "submitting does not break the line");
}
