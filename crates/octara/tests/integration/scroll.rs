//! Scroll containers (Tailwind `overflow-*`) and virtual lists inside them.

use octara::app::{Event, InputEvent, Phase, WheelPhase};
use octara::inspect::{self, Delivery, ScrollResult};
use octara::prelude::*;
use serde_json::json;

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

#[test]
fn inspector_scroll_reports_clamped_movement_for_the_bound_viewport() {
    let mut app = Window::headless(|| col().id("viewport").w(100).h(80).overflow_y_scroll().child(el().h(220).flex_shrink_0()));
    let target = app.inspect().find("viewport").unwrap().target.clone();
    let scroll = |app: &mut Window, dx, dy| {
        let request = json!({"op": "scroll", "target": target, "dx": dx, "dy": dy});
        inspect::handle_line(app, &request.to_string())
    };
    let wrong_axis = scroll(&mut app, 10, 0);
    assert_eq!(wrong_axis.code.as_deref(), Some("STEP_FAILED"));
    assert_eq!(wrong_axis.delivery, Some(Delivery::Refused));
    assert!(wrong_axis.scroll.is_none());
    for (dy, y, moved) in [(70, 70.0, 70.0), (100, 140.0, 70.0), (120, 140.0, 0.0), (-40, 100.0, -40.0)] {
        let reply = scroll(&mut app, 0, dy);
        assert!(reply.ok, "{reply:?}");
        assert_eq!(reply.delivery, Some(Delivery::Delivered));
        assert_eq!(reply.scroll, Some(ScrollResult { offset: [0.0, y], delta: [0.0, moved] }));
    }
    let nonfinite = inspect::handle(&mut app, inspect::Request::Scroll { id: Some("viewport".into()), dx: 0.0, dy: f32::INFINITY });
    assert_eq!(nonfinite.code.as_deref(), Some("BAD_REQUEST"));
    assert_eq!(nonfinite.delivery, Some(Delivery::Refused));

    app.app().hot_reload();
    let stale = scroll(&mut app, 0, -120);
    assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(stale.delivery, Some(Delivery::Refused));
    assert!(stale.scroll.is_none());
    let kernel = app.kernel();
    assert_eq!(kernel.scroll_offset(kernel.find("viewport").unwrap()), (0.0, 100.0));
}

#[test]
fn addressed_scrolls_share_target_and_axis_qualification() {
    const REJECTED: [(&str, f32, f32); 5] =
        [("plain", 0.0, 0.0), ("absent", 0.0, 10.0), ("repeated", 0.0, 10.0), ("vertical", 10.0, 25.0), ("horizontal", 0.0, 10.0)];
    let mut app = Window::headless(|| {
        on_command("scroll.invalid", || {
            for (id, x, y) in REJECTED {
                assert!(!scroll_to(id, x, y), "{id} cannot accept this request");
            }
            for id in ["plain", "absent", "repeated", "horizontal"] {
                assert!(!scroll_to_end(id), "{id} cannot scroll to the vertical end");
            }
        });
        on_command("scroll.valid", || {
            assert!(scroll_to("horizontal", 15.0, 0.0));
            assert!(scroll_to("both", 12.0, 20.0));
            assert!(scroll_to_end("both"));
            assert!(scroll_to_end("vertical"));
        });
        on_command("scroll.at-boundary", || {
            assert!(scroll_to("vertical", 0.0, f32::MAX));
            assert!(scroll_to_end("both"));
        });

        let vertical = |id| col().id(id).w(100).h(80).overflow_y_scroll().child(el().h(220).flex_shrink_0());
        col()
            .child(text("Plain text").id("plain"))
            .child(vertical("vertical"))
            .child(vertical("repeated"))
            .child(vertical("repeated"))
            .child(row().id("horizontal").w(100).h(40).overflow_x_scroll().child(el().w(240).flex_shrink_0()))
            .child(col().id("both").w(100).h(80).overflow_x_scroll().overflow_y_scroll().child(el().w(240).h(220).flex_shrink_0()))
    });

    app.invoke("scroll.invalid").unwrap();
    for (id, x, y) in REJECTED {
        assert!(app.dispatch(Event::ScrollTo(id.into(), x, y)).is_err(), "{id}");
        assert!(app.dispatch(Event::Scroll(id.into(), x, y)).is_err(), "{id}");
    }
    {
        let k = app.kernel();
        for node in k.find_all("repeated").into_iter().chain(k.find("vertical")).chain(k.find("horizontal")) {
            assert_eq!(k.scroll_offset(node), (0.0, 0.0));
        }
    }

    app.invoke("scroll.valid").unwrap();
    app.invoke("scroll.at-boundary").unwrap();
    app.dispatch(Event::Scroll("vertical".into(), 0.0, 25.0)).unwrap();
    app.dispatch(Event::ScrollTo("vertical".into(), 0.0, f32::MAX)).unwrap();
    let k = app.kernel();
    assert_eq!(k.scroll_offset(k.find("horizontal").unwrap()), (15.0, 0.0));
    assert_eq!(k.scroll_offset(k.find("vertical").unwrap()), (0.0, 140.0));
    assert_eq!(k.scroll_offset(k.find("both").unwrap()), (12.0, 140.0), "scrolling to the end keeps the horizontal position");
}

#[test]
fn scrolling_an_option_into_view_waits_for_mount_and_preserves_input_focus() {
    let mut app = Window::headless(|| {
        let open = state("open", || false);
        col()
            .size_full()
            .child(text_input().id("search").on_key(move |key| {
                if key.key != Key::Down {
                    return false;
                }
                open.set(true);
                scroll_into_view("r19");
                true
            }))
            .child(col().id("options").h(80.0).overflow_y_scroll().child(show(move || open.get(), || col().children(rows(20)))))
    });
    app.tap("search").unwrap();
    assert!(!app.kernel().exists("r19"));
    app.press_key("down").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("search"));
    let k = app.kernel();
    let list = k.find("options").unwrap();
    let option = k.find("r19").unwrap();
    let (_, list_y, _, list_h) = k.shown_rect(list);
    let (_, option_y, _, option_h) = k.shown_rect(option);
    assert!(k.scroll_offset(list).1 > 0.0);
    assert!(option_y >= list_y && option_y + option_h <= list_y + list_h + 0.5);
}

#[derive(Clone, PartialEq)]
struct Item {
    id: u32,
    lines: usize,
}

fn feed() -> View {
    // Rows are one 20 px line or three; the estimate says 24 for all.
    let items =
        state_list("items", || (0..10_000).map(|i| Item { id: i, lines: if i % 3 == 0 { 3 } else { 1 } }).collect(), |i: &Item| i.id);
    col()
        .child(text("feed").id("title"))
        .child(
            col()
                .child(virtual_each(
                    move || items.read_only(),
                    |_| 24.0,
                    |id, it| text(move || vec!["line"; it.with(|i| i.lines)].join("\n")).id(format!("item-{id}")),
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

#[ui]
fn reading_feed(wrapping: bool) -> View {
    let items = state_list(
        "reading-items",
        || (0..1_000).map(|id| Item { id, lines: if id % 2 == 0 { 3 } else { 1 } }).collect(),
        |item: &Item| item.id,
    );
    col()
        .child(
            col()
                .child(virtual_each(
                    move || items.read_only(),
                    |item| item.lines as f32 * 20.0,
                    move |id, item| {
                        text(move || {
                            item.with(
                                |item| {
                                    if wrapping && item.id % 2 == 0 { "word ".repeat(100) } else { vec!["line"; item.lines].join("\n") }
                                },
                            )
                        })
                        .id(format!("item-{id}"))
                    },
                ))
                .id("feed")
                .flex_1()
                .overflow_y_scroll()
                .follow_tail(),
        )
        .child(button_like("grow-reading-row", move || items.update(&100, |item| item.lines += 3)))
        .child(button_like("grow-offscreen-tail", move || items.update(&999, |item| item.lines += 100)))
        .child(button_like("jump", || {
            scroll_to_end("feed");
        }))
        .size_full()
        .into()
}

#[test]
fn streaming_a_visible_row_preserves_its_reading_position() {
    let mut app = Window::headless(|| reading_feed(false));
    app.dispatch(Event::ScrollTo("feed".into(), 0.0, 4_007.0)).unwrap();
    let before = top_row(&app);
    assert_eq!(before.0, "item-100");
    assert!(!following(&app, "feed"));

    app.tap("grow-reading-row").unwrap();

    assert_eq!(top_row(&app), before, "growing the row extends below what the person is reading");
    assert!(!following(&app, "feed"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn wrapping_resize_preserves_the_first_visible_row_and_its_screen_position() {
    let mut app = Window::headless(|| reading_feed(true));
    app.dispatch(Event::ScrollTo("feed".into(), 0.0, 4_007.0)).unwrap();
    let before = top_row(&app);
    // Read within a wrapping paragraph, followed by a short row whose height does not change.
    if before.0.trim_start_matches("item-").parse::<u32>().unwrap() % 2 == 1 {
        app.dispatch(Event::Scroll("feed".into(), 0.0, 20.0)).unwrap();
    }
    let before = top_row(&app);
    let height_before = app.kernel().rect(app.kernel().find(&before.0).unwrap()).3;
    app.resize(320.0, 600.0);

    assert_eq!(top_row(&app), before, "reflow must anchor the paragraph being read, even when its height changes");
    assert!(app.kernel().rect(app.kernel().find(&before.0).unwrap()).3 > height_before);
    // Previously measured offscreen rows still carry other widths' estimates. Revisiting
    // widths and enlarging the viewport must neither accumulate drift nor leave a blank end.
    for (width, height) in [(640.0, 600.0), (280.0, 780.0), (800.0, 360.0), (320.0, 700.0)] {
        app.resize(width, height);
        assert_eq!(top_row(&app), before, "reading position after resize to {width}x{height}");
        let k = app.kernel();
        let feed = k.find("feed").unwrap();
        let bottom = k.content_origin_y(feed) + k.client_size(feed).1;
        assert!(
            k.child_ids("feed").iter().filter(|id| id.starts_with("item-")).any(|id| {
                let row = k.rect(k.find(id).unwrap());
                row.1 <= bottom && row.1 + row.3 >= bottom
            }),
            "mounted rows fill the enlarged viewport"
        );
        assert!(!k.is_following_tail(feed));
    }
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn jump_measures_an_offscreen_streamed_tail_and_follows_later_growth() {
    let mut app = Window::headless(|| reading_feed(false));
    app.dispatch(Event::ScrollTo("feed".into(), 0.0, 4_007.0)).unwrap();
    let before = top_row(&app);
    assert!(!app.kernel().exists("item-999"));
    app.reset_counters();
    app.tap("grow-offscreen-tail").unwrap();
    assert_eq!(top_row(&app), before);
    assert!(!following(&app, "feed"));
    assert_eq!(app.kernel().counters().nodes_created, 0, "offscreen stream must not mount rows");

    app.tap("jump").unwrap();
    assert!(following(&app, "feed"));
    assert_eq!(offset(&app, "feed"), end(&app, "feed"));
    let k = app.kernel();
    let last = k.rect(k.find("item-999").unwrap());
    let feed = k.find("feed").unwrap();
    assert_eq!(last.1 + last.3, k.content_origin_y(feed) + k.client_size(feed).1);
    drop(k);
    app.tap("grow-offscreen-tail").unwrap();
    assert_eq!(offset(&app, "feed"), end(&app, "feed"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn a_virtual_list_outside_a_scroll_container_is_reported() {
    let app = Window::headless(|| {
        let items = state_list("items", || (0..10u32).collect(), |i: &u32| *i);
        col().child(virtual_each(move || items.read_only(), |_| 20.0, |i, _| text(format!("{i}"))))
    });
    assert!(app.app().diagnostics().iter().any(|d| d.code == "VIRTUAL_LIST_NOT_SCROLLABLE"), "{:?}", app.app().diagnostics());
}

// ---- following the tail ----

/// A log that keeps its end in view, a "latest" button shown while it does not, and a button
/// adding a line.
fn log() -> View {
    let lines = state_list("lines", || (0..40u32).collect(), |i: &u32| *i);
    col()
        .child(
            col()
                .child(each(move || lines.read_only(), |i, _| text(format!("line {i}")).id(format!("l{i}"))))
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

fn nested_wheel_window() -> Window {
    Window::headless(|| {
        let inner = state("inner-visible", || true);
        on_command("wheel.toggle-inner", move || inner.update(|shown| shown.toggle()));
        col()
            .id("outer")
            .h(250)
            .overflow_y_scroll()
            .child(show(move || inner.get(), || col().id("inner").h(100).overflow_y_scroll().child(el().h(300).child("Inner content"))))
            .child(el().h(500).child("Outer content"))
    })
}

fn wheel(app: &mut Window, phase: WheelPhase, y: f32, dy: f32) {
    app.event(InputEvent::Wheel { x: 40.0, y, dx: 0.0, dy, phase });
}

#[test]
fn a_phased_wheel_keeps_the_first_moving_target_past_the_edge_and_during_momentum() {
    use Phase::{Down, Move, Up};
    use WheelPhase::{Direct, Momentum};
    let mut app = nested_wheel_window();
    app.dispatch(Event::ScrollTo("inner".into(), 0.0, 188.0)).unwrap();

    wheel(&mut app, Direct(Down), 150.0, 0.0);
    wheel(&mut app, Direct(Move), 50.0, 30.0);
    assert_eq!(offset(&app, "inner"), 200.0);
    wheel(&mut app, Direct(Move), 150.0, 25.0);
    wheel(&mut app, Direct(Move), 150.0, -8.0);
    assert_eq!(offset(&app, "inner"), 192.0, "moving the pointer kept the same scroller");
    wheel(&mut app, Direct(Up), 150.0, 0.0);
    wheel(&mut app, Momentum(Down), 150.0, 210.0);
    wheel(&mut app, Momentum(Move), 150.0, 40.0);
    wheel(&mut app, Momentum(Up), 150.0, 0.0);

    assert_eq!(offset(&app, "inner"), 200.0);
    assert_eq!(offset(&app, "outer"), 0.0);
    app.wheel(40.0, 150.0, 0.0, 20.0);
    assert_eq!(offset(&app, "outer"), 20.0, "the next discrete input hit-tests independently");
}

#[test]
fn momentum_of_a_retired_viewport_cannot_scroll_the_parent_or_a_same_name_replacement() {
    use Phase::{Down, Move, Up};
    use WheelPhase::{Direct, Momentum};
    let mut app = nested_wheel_window();
    let original = app.inspect().find("inner").unwrap().target.clone();
    wheel(&mut app, Direct(Down), 50.0, 30.0);
    wheel(&mut app, Direct(Up), 50.0, 0.0);
    wheel(&mut app, Momentum(Down), 50.0, 15.0);
    assert_eq!(offset(&app, "inner"), 45.0);

    app.invoke("wheel.toggle-inner").unwrap();
    assert!(app.inspect().find("inner").is_none());
    wheel(&mut app, Momentum(Move), 50.0, 132.0);
    assert_eq!(offset(&app, "outer"), 0.0);

    app.invoke("wheel.toggle-inner").unwrap();
    assert_ne!(app.inspect().find("inner").unwrap().target, original);
    wheel(&mut app, Momentum(Move), 50.0, 80.0);
    wheel(&mut app, Momentum(Up), 50.0, 0.0);
    assert_eq!(offset(&app, "inner"), 0.0);
    assert_eq!(offset(&app, "outer"), 0.0);
    wheel(&mut app, Direct(Down), 50.0, 11.0);
    assert_eq!(offset(&app, "inner"), 11.0);
}

#[test]
fn cancellation_closes_one_wheel_sequence_without_reviving_it_or_cancelling_a_new_direct_contact() {
    use Phase::{Cancel, Down, Move, Up};
    use WheelPhase::{Direct, Momentum};
    let mut app = nested_wheel_window();
    wheel(&mut app, Direct(Down), 50.0, 20.0);
    wheel(&mut app, Direct(Up), 50.0, 0.0);
    app.event(InputEvent::Active(false));
    app.event(InputEvent::Active(true));
    wheel(&mut app, Direct(Move), 50.0, 40.0);
    wheel(&mut app, Momentum(Down), 50.0, 50.0);
    assert_eq!(offset(&app, "inner"), 20.0);

    wheel(&mut app, Direct(Down), 50.0, 5.0);
    wheel(&mut app, Momentum(Cancel), 50.0, 0.0);
    wheel(&mut app, Direct(Move), 50.0, 5.0);
    wheel(&mut app, Direct(Cancel), 50.0, 0.0);
    wheel(&mut app, Direct(Move), 50.0, 40.0);
    assert_eq!(offset(&app, "inner"), 30.0);
}

#[test]
fn a_wheel_recording_without_a_phase_replays_as_a_discrete_delta() {
    let event: InputEvent = octara::serde_json::from_str(r#"{"wheel":{"x":40,"y":150,"dx":0,"dy":12}}"#).unwrap();
    let mut app = nested_wheel_window();
    assert!(app.event(event));
    assert_eq!(offset(&app, "outer"), 12.0);
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
        let items =
            state_list("items", || (0..10_000).map(|i| Item { id: i, lines: if i % 3 == 0 { 3 } else { 1 } }).collect(), |i: &Item| i.id);
        col()
            .child(
                col()
                    .child(virtual_each(
                        move || items.read_only(),
                        |_| 24.0,
                        |id, it| text(move || vec!["line"; it.with(|i| i.lines)].join("\n")).id(format!("item-{id}")),
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
