//! Scroll containers (Tailwind `overflow-*`) and virtual lists inside them.

use octara::prelude::*;

fn rows(n: usize) -> Vec<View> {
    (0..n).map(|i| text(format!("row {i}")).id(format!("r{i}")).into()).collect()
}

/// A header, a scroll area filling the rest of the window, a footer.
fn page() -> View {
    col()
        .child(text("header").id("header"))
        .child(col().children(rows(100)).id("list").flex_1().overflow_y_scroll())
        .child(text("footer").id("footer"))
        .size_full()
        .into()
}

fn y(app: &Window, id: &str) -> f32 {
    let k = app.kernel();
    k.rect(k.find(id).unwrap()).1
}

#[test]
fn a_scroll_container_bounds_its_content_and_scrolls() {
    let mut app = Window::headless(page);
    assert!(app.layout_issues().is_empty(), "{:#?}", app.layout_issues());
    let k = app.kernel();
    let list = k.find("list").unwrap();
    let (_, ch) = k.client_size(list);
    assert_eq!(ch, 600.0 - 20.0 * 2.0, "fills the window between header and footer");
    assert_eq!(k.scroll_size(list).1, 100.0 * 20.0);
    drop(k);

    let before = y(&app, "r10");
    app.dispatch(Event::ScrollTo("list".into(), 0.0, 100.0)).unwrap();
    assert_eq!(y(&app, "r10"), before - 100.0);
    app.dispatch(Event::Scroll("list".into(), 0.0, 1e6)).unwrap();
    let k = app.kernel();
    assert_eq!(k.scroll_offset(list).1, 2000.0 - ch, "clamped to the end");
    drop(k);
    let err = app.dispatch(Event::Scroll("header".into(), 0.0, 10.0)).unwrap_err();
    assert!(err.contains("overflow_y_scroll"), "{err}");
}

#[derive(Clone, PartialEq)]
struct Item {
    id: u32,
    lines: usize,
}

fn feed() -> View {
    // Rows are one 20 px line or three; the estimate says 24 for all.
    let items = list("items", || (0..10_000).map(|i| Item { id: i, lines: if i % 3 == 0 { 3 } else { 1 } }).collect(), |i: &Item| i.id);
    col()
        .child(text("feed").id("title"))
        .child(
            col()
                .child(virtual_each(
                    move || items.all(),
                    |_| 24.0,
                    |id, it| text(move || vec!["line"; it.with(|i| i.lines)].join("\n")).id(format!("item-{id}")).into(),
                ))
                .id("feed")
                .flex_1()
                .overflow_y_scroll(),
        )
        .size_full()
        .into()
}

#[test]
fn virtual_list_follows_its_scroll_container_and_measures_rows() {
    let mut app = Window::headless(feed);
    let mounted = app.kernel().child_ids("feed").len();
    assert!(mounted < 40, "only rows near the window exist: {mounted}");
    assert!(app.layout_issues().is_empty(), "{:#?}", app.layout_issues());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());

    // Jump far down: the window follows (rows near the estimated position exist).
    app.dispatch(Event::ScrollTo("feed".into(), 0.0, 24.0 * 5_000.0)).unwrap();
    let (first, first_y) = top_row(&app);
    let n: u32 = first.trim_start_matches("item-").parse().unwrap();
    assert!((4_900..=5_000).contains(&n), "rows near 5000 (earlier rows were measured, on average taller): {first}");
    // Mounted rows have their measured heights: three-line rows are 60 px.
    let k = app.kernel();
    let three = n - n % 3;
    assert_eq!(k.rect(k.find(&format!("item-{three}")).unwrap()).3, 60.0);
    drop(k);

    // Scrolling up mounts rows above, which are measured (most are taller than the estimate).
    // Anchoring keeps the content in place: the old top row moves down by exactly the scroll.
    app.dispatch(Event::Scroll("feed".into(), 0.0, -200.0)).unwrap();
    let k = app.kernel();
    let y = k.rect(k.find(&first).unwrap()).1;
    assert_eq!(y, first_y + 200.0, "{first} moved by the scroll only");
}

/// The row covering the top edge of the feed, and its y.
fn top_row(app: &Window) -> (String, f32) {
    let k = app.kernel();
    let feed = k.find("feed").unwrap();
    let top = k.content_origin_y(feed);
    k.child_ids("feed")
        .into_iter()
        .filter(|id| id.starts_with("item-"))
        .map(|id| {
            let r = k.rect(k.find(&id).unwrap());
            (id, r.1, r.3)
        })
        .find(|(_, y, h)| *y <= top && y + h > top)
        .map(|(id, y, _)| (id, y))
        .expect("a row covers the top of the window")
}

#[test]
fn a_virtual_list_outside_a_scroll_container_is_reported() {
    let app = Window::headless(|| {
        let items = list("items", || (0..10u32).collect(), |i: &u32| *i);
        col().child(virtual_each(move || items.all(), |_| 20.0, |i, _| text(format!("{i}")).into())).into()
    });
    assert!(app.app().diagnostics().iter().any(|d| d.code == "VIRTUAL_LIST_NOT_SCROLLABLE"), "{:?}", app.app().diagnostics());
}

// ---- following the tail ----

/// A log that keeps its end in view, a "latest" button shown while it does not, and a button
/// adding a line.
fn log() -> View {
    let lines = list("lines", || (0..40u32).collect(), |i: &u32| *i);
    col()
        .child(
            col()
                .child(each(move || lines.all(), |i, _| text(format!("line {i}")).id(format!("l{i}")).into()))
                .id("log")
                .flex_1()
                .overflow_y_scroll()
                .follow_tail(),
        )
        .child(show(
            move || !following_tail("log").get(),
            || {
                button_like("latest", || {
                    scroll_to_end("log");
                })
            },
        ))
        .child(button_like("add", move || {
            let n = lines.len() as u32;
            lines.push(n);
        }))
        .size_full()
        .into()
}

fn button_like(id: &str, f: impl Fn() + 'static) -> View {
    el().id(id).h(px(20.0)).on_press(f).child(text(id)).into()
}

fn offset(app: &Window, id: &str) -> f32 {
    let k = app.kernel();
    k.scroll_offset(k.find(id).unwrap()).1
}

fn end(app: &Window, id: &str) -> f32 {
    let k = app.kernel();
    let n = k.find(id).unwrap();
    k.scroll_size(n).1 - k.client_size(n).1
}

fn following(app: &Window, id: &str) -> bool {
    let k = app.kernel();
    k.is_following_tail(k.find(id).unwrap())
}

#[test]
fn a_container_following_its_tail_keeps_its_end_in_view() {
    let mut app = Window::headless(log);
    assert!(end(&app, "log") > 0.0);
    assert_eq!(offset(&app, "log"), end(&app, "log"), "starts at the end");
    assert!(app.kernel().find("latest").is_none());

    app.tap("add").unwrap();
    app.tap("add").unwrap();
    assert_eq!(offset(&app, "log"), end(&app, "log"), "the end stays in view as lines come");

    // The person scrolls up: it lets go, and new lines do not move what they read.
    let (x, y, w, h) = app.kernel().shown_rect(app.kernel().find("log").unwrap());
    app.wheel(x + w / 2.0, y + h / 2.0, 0.0, -100.0);
    assert!(!following(&app, "log"));
    let at = offset(&app, "log");
    app.tap("add").unwrap();
    assert_eq!(offset(&app, "log"), at);
    assert!(app.kernel().find("latest").is_some(), "a way back shows");

    // Scrolling back to the end follows again.
    app.wheel(x + w / 2.0, y + h / 2.0, 0.0, 1e6);
    assert!(following(&app, "log"));
    assert!(app.kernel().find("latest").is_none());
    app.tap("add").unwrap();
    assert_eq!(offset(&app, "log"), end(&app, "log"));
}

#[test]
fn scrolling_to_the_end_follows_the_tail_again() {
    let mut app = Window::headless(log);
    app.dispatch(Event::ScrollTo("log".into(), 0.0, 0.0)).unwrap();
    assert!(!following(&app, "log"), "a program scrolling away lets go too");
    app.tap("add").unwrap();
    assert_eq!(offset(&app, "log"), 0.0);
    app.tap("latest").unwrap();
    app.tap("add").unwrap();
    assert!(following(&app, "log"));
    assert_eq!(offset(&app, "log"), end(&app, "log"));
}

#[test]
fn a_virtual_list_following_its_tail_lands_on_its_measured_end() {
    let mut app = Window::headless(|| {
        let items = list("items", || (0..10_000).map(|i| Item { id: i, lines: if i % 3 == 0 { 3 } else { 1 } }).collect(), |i: &Item| i.id);
        col()
            .child(
                col()
                    .child(virtual_each(
                        move || items.all(),
                        |_| 24.0,
                        |id, it| text(move || vec!["line"; it.with(|i| i.lines)].join("\n")).id(format!("item-{id}")).into(),
                    ))
                    .id("feed")
                    .flex_1()
                    .overflow_y_scroll()
                    .follow_tail(),
            )
            .child(button_like("grow", move || {
                let n = items.len() as u32;
                items.push(Item { id: n, lines: 5 });
            }))
            .size_full()
            .into()
    });
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
    let bottom = |app: &Window, id: &str| {
        let k = app.kernel();
        let r = k.rect(k.find(id).unwrap());
        r.1 + r.3
    };
    let feed_bottom = |app: &Window| {
        let k = app.kernel();
        let r = k.shown_rect(k.find("feed").unwrap());
        r.1 + r.3
    };
    assert_eq!(bottom(&app, "item-9999"), feed_bottom(&app), "the last row sits at the bottom");
    app.tap("grow").unwrap();
    assert_eq!(bottom(&app, "item-10000"), feed_bottom(&app), "a taller new row is measured and kept at the bottom");
    assert!(following(&app, "feed"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}
