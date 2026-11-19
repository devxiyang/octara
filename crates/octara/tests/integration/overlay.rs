//! Overlays: shown above everything, placed next to their anchor, flipped and clamped.

use octara::app::{DEFAULT_VIEWPORT, Event};
use octara::prelude::*;
use octara_primitives::layout::Placed;

/// A trigger at `(left, top)` opening a 100×40 panel.
fn trigger_at(left: f32, top: f32, placement: Placement) -> impl Fn() -> View {
    move || {
        let open = state("open", || false);
        let picks = state("picks", || 0);
        el().child(el().child(text("Open")).id("trigger").w_16().h_8().on_press(move || open.update(|o| *o = !*o)).overlay(
            move || open.get(),
            placement,
            move || {
                let hovered = state("hovered", || false);
                el().child(text(move || format!("picked {}", picks.get())).id("pick").on_press(move || {
                    picks.update(|n| *n += 1);
                    hovered.set(true);
                }))
                .id("panel")
                .w(px(100.0))
                .h(px(40.0))
            },
        ))
        .pl(px(left))
        .pt(px(top))
        .into()
    }
}

fn rect(app: &Window, id: &str) -> (f32, f32, f32, f32) {
    let k = app.kernel();
    k.rect(k.find(id).unwrap())
}

fn side(app: &Window) -> Option<Side> {
    let k = app.kernel();
    k.placed_side(k.find("panel").unwrap())
}

#[test]
fn opens_below_its_anchor_and_closes() {
    let mut app = Window::headless(trigger_at(10.0, 20.0, Placement::BELOW));
    assert!(!app.kernel().exists("panel"));
    app.dispatch(Event::Press("trigger".into())).unwrap();
    let (tx, ty, _, th) = rect(&app, "trigger");
    assert_eq!(rect(&app, "panel"), (tx, ty + th + 4.0, 100.0, 40.0));
    assert_eq!(side(&app), Some(Side::Bottom));
    // Content inside the overlay is live, and handlers work.
    app.dispatch(Event::Press("pick".into())).unwrap();
    assert_eq!(app.kernel().text_of("pick").unwrap(), "picked 1");
    // The overlay paints over the app: it lives in the root's last child.
    let k = app.kernel();
    assert_eq!(k.children(k.root()).last(), Some(&k.overlay_layer()));
    drop(k);
    app.dispatch(Event::Press("trigger".into())).unwrap();
    assert!(!app.kernel().exists("panel"));
}

#[test]
fn anchored_overlay_mounts_a_fragment_as_siblings() {
    let app = Window::headless(|| {
        el().size_8().overlay(
            || true,
            Placement::BELOW,
            || View::fragment([text("First").id("first").into(), text("Second").id("second").into()]),
        )
    });
    let k = app.kernel();
    let [surface] = k.children(k.overlay_layer()) else { panic!("expected one placement surface") };
    assert_eq!(k.children(*surface), &[k.find("first").unwrap(), k.find("second").unwrap()]);
    assert!(app.app().diagnostics().is_empty());
}

#[test]
fn flips_when_there_is_no_room_and_clamps_into_the_window() {
    let (vw, vh) = DEFAULT_VIEWPORT;
    let mut app = Window::headless(trigger_at(vw - 20.0, vh - 40.0, Placement::BELOW));
    app.dispatch(Event::Press("trigger".into())).unwrap();
    let (_, ty, _, _) = rect(&app, "trigger");
    let (px, py, pw, ph) = rect(&app, "panel");
    assert_eq!(side(&app), Some(Side::Top), "no room below: opens above");
    assert_eq!(py + ph + 4.0, ty);
    assert_eq!(px + pw, vw - 8.0, "kept 8 px inside the right edge");
}

#[test]
fn overlay_state_survives_hot_reload_and_leaves_no_residue() {
    let mut app = Window::headless(trigger_at(0.0, 0.0, Placement::BELOW));
    app.dispatch(Event::Press("trigger".into())).unwrap();
    app.dispatch(Event::Press("pick".into())).unwrap();
    let before = app.kernel().tree_dump();
    app.app().hot_reload();
    assert_eq!(app.kernel().tree_dump(), before);
    assert_eq!(rect(&app, "panel").1, rect(&app, "trigger").3 + 4.0);
    app.dispatch(Event::Press("trigger".into())).unwrap();
    let k = app.kernel();
    assert!(k.children(k.overlay_layer()).is_empty());
}

fn overlapping_modal_overlays() -> Window {
    Window::headless(|| {
        let a = state("dialog-a-open", || false);
        let b = state("dialog-b-open", || false);
        on_command("open-a", move || a.set(true));
        on_command("open-b", move || b.set(true));
        on_command("close-a", move || a.set(false));
        col().size_full().child(el().id("page-opener").size_8().focusable()).children([("a", a), ("b", b)].map(|(id, open)| {
            el().absolute().size(0.0).overlay_window(
                move || open.get(),
                move || {
                    col()
                        .id(format!("dialog-{id}"))
                        .role(Role::Dialog)
                        .absolute()
                        .left(160.0)
                        .top(120.0)
                        .w(160.0)
                        .h(100.0)
                        .modal(false, move || open.set(false))
                        .children(
                            ["first", "second"]
                                .map(|name| el().id(format!("{id}-{name}")).role(Role::Button).size_8().focusable().on_press(|| {})),
                        )
                },
            )
        }))
    })
}

#[test]
fn overlapping_modals_follow_paint_order_when_opened_in_reverse() {
    let mut app = overlapping_modal_overlays();
    app.invoke("open-b").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("b-first"));
    app.invoke("open-a").unwrap();
    assert_eq!(rect(&app, "dialog-a"), rect(&app, "dialog-b"));
    assert_eq!(app.inspect().focus.as_deref(), Some("b-first"));

    app.tap("b-second").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("b-second"));
    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("dialog-b"));
    assert!(app.kernel().exists("dialog-a"));
    assert_eq!(app.inspect().focus.as_deref(), Some("a-first"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn closing_the_lower_modal_keeps_focus_in_the_front_modal() {
    let mut app = overlapping_modal_overlays();
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("page-opener"));
    app.invoke("open-a").unwrap();
    app.invoke("open-b").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("b-second"));

    app.invoke("close-a").unwrap();
    assert!(!app.kernel().exists("dialog-a"));
    assert!(app.kernel().exists("dialog-b"));
    assert_eq!(app.inspect().focus.as_deref(), Some("b-second"));
    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("dialog-b"));
    assert_eq!(app.inspect().focus.as_deref(), Some("page-opener"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn outside_dismissal_does_not_retarget_after_reload() {
    let mut app = Window::headless(|| {
        let open = state("open", || true);
        let presses = state("presses", || 0);
        row()
            .child(el().size_12().overlay(
                move || open.get(),
                Placement::BELOW,
                move || {
                    el().id("panel").size_12().dismissible(true, move || {
                        open.set(false);
                        App::current().hot_reload();
                    })
                },
            ))
            .child(el().id("outside").size_12().on_press(move || presses.update(|n| *n += 1)))
            .child(text(move || presses.get().to_string()).id("presses"))
    });
    let before = app.inspect().find("outside").unwrap().target.clone();
    let (x, y, w, h) = rect(&app, "outside");
    app.click_at(x + w / 2.0, y + h / 2.0);
    assert!(!app.kernel().exists("panel"));
    assert_ne!(app.inspect().find("outside").unwrap().target, before);
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"));
    app.tap("outside").unwrap();
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("1"));
}

#[test]
fn wheel_inside_an_inline_modal_leaves_its_inert_scroll_ancestor_alone() {
    let mut app = Window::headless(|| {
        col()
            .id("scroll")
            .w(240)
            .h(100)
            .overflow_y_scroll()
            .child(el().id("modal").h(40).w_full().flex_shrink_0().modal(false, || {}))
            .child(el().h(240).flex_shrink_0())
    });
    let (x, y, w, h) = rect(&app, "modal");
    app.wheel(x + w / 2.0, y + h / 2.0, 0.0, 50.0);
    let k = app.kernel();
    assert_eq!(k.scroll_offset(k.find("scroll").unwrap()), (0.0, 0.0));
}

#[test]
fn an_opening_modal_cancels_a_background_drag() {
    let phases = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorded = phases.clone();
    let mut app = Window::headless(move || {
        let open = state("open", || false);
        let recorded = recorded.clone();
        on_command("open", move || open.set(true));
        col()
            .child(el().id("drag").w(100).h(60).on_drag(move |drag| recorded.borrow_mut().push(drag.phase)))
            .child(show(move || open.get(), || el().size_8().modal(false, || {})))
    });
    app.pointer_down(10.0, 10.0);
    app.pointer_move(30.0, 10.0);
    app.invoke("open").unwrap();
    app.pointer_move(50.0, 10.0);
    app.pointer_up(50.0, 10.0);
    assert_eq!(*phases.borrow(), [DragPhase::Start, DragPhase::Cancel]);
}

fn nested_overlays() -> Window {
    Window::headless(|| {
        let outer = state("outer", || false);
        let middle = state("middle", || false);
        let inner = state("inner", || false);
        on_command("nested.open-all", move || {
            outer.set(true);
            middle.set(true);
            inner.set(true);
        });
        col().size_full().child(
            text("Outer")
                .id("outer-trigger")
                .absolute()
                .left(relative(0.125))
                .top(relative(0.2))
                .w(80.0)
                .h(24.0)
                .on_press(move || outer.set(true))
                .overlay(
                    move || outer.get(),
                    Placement::BELOW,
                    move || {
                        col().id("outer-panel").w(240.0).h(120.0).p_4().child(
                            text("Middle").id("middle-trigger").w(80.0).h(24.0).on_press(move || middle.set(true)).overlay(
                                move || middle.get(),
                                Placement::BELOW,
                                move || {
                                    col().id("middle-panel").w(140.0).h(80.0).p_2().child(
                                        text("Inner").id("inner-trigger").w(80.0).h(24.0).on_press(move || inner.set(true)).overlay(
                                            move || inner.get(),
                                            Placement::BELOW,
                                            || el().id("inner-panel").w(100.0).h(32.0),
                                        ),
                                    )
                                },
                            ),
                        )
                    },
                ),
        )
    })
}

const NESTED_PAIRS: [(&str, &str); 3] =
    [("outer-trigger", "outer-panel"), ("middle-trigger", "middle-panel"), ("inner-trigger", "inner-panel")];

fn assert_nested_anchors(app: &Window) {
    for (trigger, panel) in NESTED_PAIRS {
        let (x, y, _, height) = rect(app, trigger);
        let (panel_x, panel_y, _, _) = rect(app, panel);
        assert_eq!((panel_x, panel_y), (x, y + height + 4.0), "{panel} must follow its anchor");
    }
    assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn nested_overlays_can_mount_all_at_once() {
    let mut app = nested_overlays();
    app.invoke("nested.open-all").unwrap();
    assert_nested_anchors(&app);
}

#[test]
fn nested_overlays_remain_next_to_their_anchors_when_rebuilt_open() {
    let mut app = nested_overlays();
    for (trigger, _) in NESTED_PAIRS {
        app.dispatch(Event::Press(trigger.into())).unwrap();
    }
    let before: Vec<_> = NESTED_PAIRS.iter().map(|(_, panel)| rect(&app, panel)).collect();
    app.app().hot_reload();
    assert_nested_anchors(&app);
    for ((_, panel), before) in NESTED_PAIRS.iter().zip(before) {
        assert_eq!(rect(&app, panel), before, "rebuilding an open nested overlay preserves its placement");
    }
}

#[test]
fn nested_overlays_follow_their_anchors_after_a_viewport_resize() {
    let mut app = nested_overlays();
    for (trigger, _) in NESTED_PAIRS {
        app.dispatch(Event::Press(trigger.into())).unwrap();
    }
    let before = rect(&app, "outer-trigger");
    app.resize(1000.0, 720.0);
    assert_ne!(rect(&app, "outer-trigger"), before, "the anchor actually moves with the viewport");
    assert_nested_anchors(&app);
}

#[test]
fn placement_math() {
    let vp = (200.0, 100.0);
    let a = (50.0, 40.0, 20.0, 20.0);
    let at = |x, y, side| Placed { x, y, side, limit: None, clamped: false };
    let p = |pl: Placement, size| pl.place(a, size, vp);
    assert_eq!(p(Placement::BELOW, (30.0, 10.0)), at(50.0, 64.0, Side::Bottom));
    assert_eq!(p(Placement::ABOVE, (30.0, 10.0)), at(45.0, 26.0, Side::Top));
    assert_eq!(p(Placement::RIGHT, (30.0, 10.0)), at(70.0, 40.0, Side::Right));
    assert_eq!(p(Placement::BELOW.align(Align::End), (30.0, 10.0)), at(40.0, 64.0, Side::Bottom));
    // Fits neither side: with `fit` it takes the roomier side (ties keep the requested one) and
    // is limited to the room there, 8 px from the window edge.
    assert_eq!(p(Placement::BELOW, (30.0, 90.0)), Placed { limit: Some(28.0), ..at(50.0, 64.0, Side::Bottom) });
    let low = (50.0, 50.0, 20.0, 20.0);
    assert_eq!(Placement::BELOW.place(low, (30.0, 90.0), vp), Placed { limit: Some(38.0), ..at(50.0, 8.0, Side::Top) });
    // Without `fit` it is pushed into the window and reported as clamped.
    assert_eq!(p(Placement::BELOW.fit(false), (30.0, 90.0)), Placed { clamped: true, ..at(50.0, 8.0, Side::Bottom) });
    // Wider than the window: pinned 8 px from the left edge.
    assert_eq!(p(Placement::BELOW, (300.0, 10.0)).x, 8.0);
}
