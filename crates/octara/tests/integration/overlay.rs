//! Overlays: shown above everything, placed next to their anchor, flipped and clamped.

use octara::app::{DEFAULT_VIEWPORT, Event, InputEvent, Phase, PointerEvent};
use octara::inspect::{self, Delivery};
use octara::platform::HeadlessWindow;
use octara::prelude::*;
use octara::serde_json::json;
use octara_primitives::layout::Placed;
use octara_primitives::paint::Rect;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

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

fn interactive_popover(motion: Curve) -> Window {
    Window::headless(move || {
        let open = state("open", || false);
        let presses = state("presses", || 0);
        let outside_presses = state("outside-presses", || 0);
        let closes = state("closes", || 0);
        let left = state("left", || 120.0);
        on_command("popup.close", move || open.set(false));
        on_command("popup.open", move || open.set(true));
        on_command("popup.move", move || left.set(160.0));
        col().size_full().overlay_window(
            || true,
            move || {
                col()
                    .id("dialog")
                    .role(Role::Dialog)
                    .label("Settings")
                    .absolute()
                    .style_with(move |p| p.left(left.get()))
                    .top(100)
                    .w(400)
                    .h(300)
                    .p_4()
                    .gap_4()
                    .items_start()
                    .bg(Token::Surface)
                    .modal(false, || {})
                    .child(
                        el().id("tools")
                            .role(Role::Button)
                            .label("Tools")
                            .w(84)
                            .h(32)
                            .focusable()
                            .autofocus()
                            .expanded(open)
                            .on_press(move || open.update(|value| *value = !*value))
                            .overlay(
                                move || open.get(),
                                Placement::RIGHT,
                                move || {
                                    col()
                                        .id("tools-panel")
                                        .w(180)
                                        .h(90)
                                        .gap_2()
                                        .bg(Token::Surface)
                                        .opacity_motion(motion)
                                        .dismissible(true, move || {
                                            closes.update(|n| *n += 1);
                                            open.set(false);
                                        })
                                        .child(input().id("notes").label("Notes").value("Draft").h(34).w_full())
                                        .child(
                                            el().id("apply")
                                                .role(Role::Button)
                                                .label("Apply")
                                                .h(32)
                                                .w_full()
                                                .focusable()
                                                .on_press(move || presses.update(|n| *n += 1)),
                                        )
                                },
                            ),
                    )
                    .child(
                        el().id("after-tools")
                            .role(Role::Button)
                            .label("After tools")
                            .w(84)
                            .h(32)
                            .focusable()
                            .on_press(move || outside_presses.update(|n| *n += 1)),
                    )
                    .child(text(move || presses.get().to_string()).id("presses"))
                    .child(text(move || outside_presses.get().to_string()).id("outside-presses"))
                    .child(text(move || closes.get().to_string()).id("closes"))
            },
        )
    })
}

#[test]
fn an_interactive_popover_tabs_after_its_trigger_and_dismisses_when_focus_leaves() {
    let mut app = interactive_popover(Curve::Instant);
    app.tap("tools").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("tools"));
    for (key, expected) in [("tab", "notes"), ("shift+tab", "tools"), ("tab", "notes"), ("tab", "apply"), ("tab", "after-tools")] {
        app.press_key(key).unwrap();
        assert_eq!(app.inspect().focus.as_deref(), Some(expected), "{key}");
    }
    assert!(!app.kernel().exists("tools-panel"));
    assert!(app.kernel().exists("dialog"));
    assert_eq!(app.kernel().text_of("closes").as_deref(), Some("1"));
}

#[test]
fn closing_an_interactive_popover_preserves_the_right_focus_and_press() {
    let mut app = interactive_popover(Curve::Instant);
    app.tap("tools").unwrap();
    app.tap("tools").unwrap();
    assert!(!app.kernel().exists("tools-panel"));
    assert_eq!(app.kernel().text_of("closes").as_deref(), Some("0"), "the trigger handles its own toggle");

    app.tap("tools").unwrap();
    app.tap("notes").unwrap();
    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("tools-panel"));
    assert_eq!(app.inspect().focus.as_deref(), Some("tools"));
    assert!(app.kernel().exists("dialog"));

    app.tap("tools").unwrap();
    app.tap("notes").unwrap();
    app.tap("after-tools").unwrap();
    assert!(!app.kernel().exists("tools-panel"));
    assert_eq!(app.inspect().focus.as_deref(), Some("after-tools"));
    assert_eq!(app.kernel().text_of("outside-presses").as_deref(), Some("1"));
    assert_eq!(app.kernel().text_of("closes").as_deref(), Some("2"));
}

#[test]
fn a_touch_leaves_the_popovers_editor_in_place_until_the_outside_press_qualifies() {
    let mut app = interactive_popover(Curve::Instant);
    app.tap("tools").unwrap();
    app.tap("notes").unwrap();
    let (x, y, w, h) = rect(&app, "after-tools");
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    app.event(InputEvent::Pointer(PointerEvent::touch(1, Phase::Down, x, y)));
    assert!(app.kernel().exists("tools-panel"), "dismissing now could move the target as the keyboard hides");
    assert_eq!(app.inspect().focus.as_deref(), Some("notes"));
    app.event(InputEvent::Pointer(PointerEvent::touch(1, Phase::Up, 2.0, 2.0)));
    assert!(app.kernel().exists("tools-panel"), "an outside contact that missed its target cannot close the panel");
    assert_eq!(app.kernel().text_of("closes").as_deref(), Some("0"));
    app.event(InputEvent::Pointer(PointerEvent::touch(2, Phase::Down, x, y)));
    app.event(InputEvent::Pointer(PointerEvent::touch(2, Phase::Up, x, y)));
    assert!(!app.kernel().exists("tools-panel"));
    assert_eq!(app.inspect().focus.as_deref(), Some("after-tools"));
    assert_eq!(app.kernel().text_of("outside-presses").as_deref(), Some("1"));
    assert_eq!(app.kernel().text_of("closes").as_deref(), Some("1"));
}

#[test]
fn an_exiting_popover_keeps_its_last_position_but_leaves_the_interactive_tree() {
    let mut app = interactive_popover(Curve::Fade(0.24));
    app.tap("tools").unwrap();
    app.tap("notes").unwrap();
    let panel = app.kernel().find("tools-panel").unwrap();
    let dialog = app.kernel().find("dialog").unwrap();
    let surface = app.kernel().parent(panel).unwrap();
    let field = app.kernel().find("notes").unwrap();
    let anchor_x = rect(&app, "tools").0;
    let painted = |app: &Window| app.display_list().quads().find(|q| q.source == panel).map(|q| q.rect);
    let before = painted(&app).unwrap();

    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("tools"));
    assert!(app.inspect().find("tools-panel").is_none());
    let field_a11y = octara::kernel::a11y::a11y_id(field);
    assert!(!app.accessibility_tree().nodes.iter().any(|(id, _)| *id == field_a11y));
    assert_eq!(painted(&app), Some(before), "the first closed frame still draws the old panel");
    let drawn: Vec<_> = app.display_list().quads().filter(|q| q.source == dialog || q.source == panel).map(|q| q.source).collect();
    assert_eq!(drawn, [dialog, panel], "the exit keeps its place above the Dialog");
    assert_eq!(app.kernel().hit_test(before.x + 4.0, before.y + 4.0), Some(dialog));

    app.invoke("popup.move").unwrap();
    assert_ne!(rect(&app, "tools").0, anchor_x);
    app.app().advance(Duration::from_millis(60));
    assert_eq!(painted(&app), Some(before), "the retired panel stays where it was shown");
    let opacity = app.display_list().items().into_iter().find_map(|item| match item {
        octara_primitives::paint::Item::PushLayer { source, opacity } if source == surface => Some(opacity),
        _ => None,
    });
    assert!(opacity.is_some_and(|opacity| opacity > 0.0 && opacity < 1.0), "the old surface is fading: {opacity:?}");
    app.app().advance(Duration::from_millis(300));
    assert!(painted(&app).is_none());
    assert!(!app.kernel().is_alive(panel));
}

#[test]
fn reopening_an_interactive_popover_never_adopts_an_old_press_or_target() {
    let mut app = interactive_popover(Curve::Fade(0.24));
    app.tap("tools").unwrap();
    let old = app.inspect().find("apply").unwrap().target.clone();
    let old_panel = app.kernel().find("tools-panel").unwrap();
    let dialog = app.kernel().find("dialog").unwrap();
    let (x, y, w, h) = rect(&app, "apply");
    let point = (x + w * 0.5, y + h * 0.5);
    app.pointer_down(point.0, point.1);
    app.invoke("popup.close").unwrap();
    app.app().advance(Duration::from_millis(60));
    app.invoke("popup.open").unwrap();
    app.pointer_up(point.0, point.1);
    let fresh = app.inspect().find("apply").unwrap().target.clone();
    assert_ne!(fresh, old);
    let request = octara::serde_json::json!({ "op": "tap", "id": "apply", "target": old });
    let reply = octara::inspect::handle_line(&mut app, &request.to_string());
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"));
    let new_panel = app.kernel().find("tools-panel").unwrap();
    let drawn: Vec<_> = app
        .display_list()
        .quads()
        .filter(|q| q.source == dialog || q.source == old_panel || q.source == new_panel)
        .map(|q| q.source)
        .collect();
    assert_eq!(drawn, [dialog, old_panel, new_panel], "the fresh panel draws over the old exit");
    app.tap("apply").unwrap();
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("1"));
    app.app().advance(Duration::from_millis(240));
    assert!(!app.kernel().is_alive(old_panel));
    assert_eq!(app.kernel().find("tools-panel"), Some(new_panel));
    assert_eq!(app.inspect().find("apply").unwrap().target, fresh);
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

#[derive(Default)]
struct PopupPresses {
    count: Cell<u32>,
    point: Cell<Option<(f32, f32)>>,
}

fn reordering_popup() -> (Window, Rc<PopupPresses>) {
    let applied = Rc::new(PopupPresses::default());
    let presses = applied.clone();
    let mut window = Window::headless(move || {
        let rows = state("rows", || (0..8u32).map(|key| (key, ())).collect::<Vec<_>>());
        let open = state("open", || false);
        on_command("reorder", move || rows.update(|rows| rows.swap(2, 5)));
        let presses = presses.clone();
        col().size_full().p_4().child(col().id("scroller").w(240.0).h(320.0).overflow_y_scroll().layout_motion(Spring::REFLOW).child(each(
            move || rows.get(),
            move |&key, _| {
                let row = col().h(64.0).flex_shrink_0();
                if key != 2 {
                    return row;
                }
                let presses = presses.clone();
                row.child(el().id("reordering-trigger").label("Open notes").w(96.0).h(32.0).on_press(move || open.set(true)).overlay(
                    move || open.get(),
                    Placement::BELOW,
                    move || {
                        let presses = presses.clone();
                        col()
                            .id("reordering-popup")
                            .w(180.0)
                            .h(96.0)
                            .p_2()
                            .gap_2()
                            .bg(Token::Surface)
                            .child(input().id("popup-editor").label("Notes").value("Draft").h(28.0).w_full())
                            .child(
                                el().id("popup-apply")
                                    .role(Role::Button)
                                    .label("Apply")
                                    .h(28.0)
                                    .w_full()
                                    .bg(Token::Accent)
                                    .preserve_focus_on_press(true)
                                    .on_press(move || {
                                        presses.count.set(presses.count.get() + 1);
                                        presses.point.set(pointer_position_in("popup-apply"));
                                    }),
                            )
                    },
                ))
            },
        )))
    });
    window.app().set_target_platform(octara::app::TargetPlatform::MacOs);
    window.dispatch(Event::Scroll("scroller".into(), 0.0, 48.0)).unwrap();
    window.dispatch(Event::Press("reordering-trigger".into())).unwrap();
    window.dispatch(Event::Focus("popup-editor".into())).unwrap();
    window.set_selection(1, 3);
    (window, applied)
}

fn hosted_selection(app: &Window) -> Rect {
    let host = app.platform_window();
    let host = host.as_any().downcast_ref::<HeadlessWindow>().unwrap();
    let (range, rect) = host.text_input.as_ref().unwrap().geometry.as_ref().unwrap().first_rect(1..3).unwrap();
    assert_eq!(range, 1..3);
    rect
}

#[test]
fn an_open_popup_follows_a_reordering_anchor_across_paint_input_and_text_geometry() {
    let (mut window, applied) = reordering_popup();
    let app = window.app();
    let (trigger, panel, editor, button, placement) = {
        let k = window.kernel();
        let panel = k.find("reordering-popup").unwrap();
        let viewport = k.find("scroller").unwrap();
        assert_eq!(k.scroll_offset(viewport).1, 48.0);
        (
            k.find("reordering-trigger").unwrap(),
            panel,
            k.find("popup-editor").unwrap(),
            k.find("popup-apply").unwrap(),
            k.parent(panel).unwrap(),
        )
    };
    let initial = window.inspect();
    let editor_target = &initial.find("popup-editor").unwrap().target;
    let button_target = &initial.find("popup-apply").unwrap().target;
    let before_panel = rect(&window, "reordering-popup");
    let before_editor = hosted_selection(&window);
    let placement = octara::kernel::a11y::a11y_id(placement);
    let editor_ax = octara::kernel::a11y::a11y_id(editor);
    let ax = window.accessibility_update();
    assert_eq!(ax.focus, editor_ax);
    assert!(ax.nodes.iter().any(|(id, _)| *id == placement));

    window.invoke("reorder").unwrap();
    let target_panel = rect(&window, "reordering-popup");
    let target_button = rect(&window, "popup-apply");
    assert_eq!(target_panel, (before_panel.0, before_panel.1 + 3.0 * 64.0, before_panel.2, before_panel.3));
    window.reset_counters();
    let near = |actual: f32, expected: f32| assert!((actual - expected).abs() <= 0.06, "{actual} != {expected}");
    let start = app.now();
    for elapsed in [0, 80, 160, 2000] {
        app.advance_to(start + Duration::from_millis(elapsed));
        let (trigger_x, trigger_y, _, trigger_h) = window.kernel().shown_rect(trigger);
        let (x, y, w, h) = window.kernel().shown_rect(panel);
        near(x, trigger_x);
        near(y, trigger_y + trigger_h + 4.0);
        assert_eq!((w, h), (target_panel.2, target_panel.3));
        assert_eq!(window.kernel().placed_side(panel), Some(Side::Bottom));
        assert_eq!(rect(&window, "reordering-popup"), target_panel);
        match elapsed {
            0 => near(y, before_panel.1),
            2000 => near(y, target_panel.1),
            _ => assert!(before_panel.1 < y && y < target_panel.1),
        }

        let drawing = window.display_list();
        let panel_quad = drawing.quads().find(|q| q.source == panel).unwrap();
        assert_eq!(panel_quad.rect, Rect::new(x, y, w, h));
        let selection = hosted_selection(&window);
        near(selection.y - before_editor.y, y - before_panel.1);
        assert!(drawing.quads().filter(|q| q.source == editor).any(|q| {
            (q.rect.y - selection.y).abs() <= 0.01 && (q.rect.x - selection.x).abs() <= 0.01 && (q.rect.w - selection.w).abs() <= 0.01
        }));

        let snapshot = window.inspect();
        let bounds = snapshot.find("reordering-popup").unwrap().bounds;
        near(bounds[1], y);
        assert_eq!(snapshot.focus.as_deref(), Some("popup-editor"));
        assert_eq!(&snapshot.find("popup-editor").unwrap().target, editor_target);
        assert_eq!(snapshot.text_input.as_ref().unwrap().selection, [1, 3]);
        assert_eq!(&snapshot.find("popup-apply").unwrap().target, button_target);

        let ax = window.accessibility_update();
        assert!(ax.tree.is_none());
        assert_eq!(ax.focus, editor_ax);
        if elapsed > 0 {
            let surface = &ax.nodes.iter().find(|(id, _)| *id == placement).expect("the anchored surface reports its new transform").1;
            let bounds = surface.transform().unwrap().transform_rect_bbox(surface.bounds().unwrap());
            near(bounds.x0 as f32, x);
            near(bounds.y0 as f32, y);
        }

        if elapsed == 80 {
            let (tx, ty, tw, th) = target_button;
            let stale = json!({"op":"tap_at","target":button_target,"x":tx + tw / 2.0,"y":ty + th / 2.0});
            let reply = inspect::handle_line(&mut window, &stale.to_string());
            assert_eq!(reply.code.as_deref(), Some("STEP_FAILED"));
            assert_eq!(reply.delivery, Some(Delivery::Refused));
            assert!(reply.pointer.is_empty());
            assert_eq!(applied.count.get(), 0);

            let at = drawing.quads().find(|q| q.source == button).unwrap().rect;
            let (local_x, local_y) = (at.w / 4.0, at.h / 2.0);
            let (x, y) = (at.x + local_x, at.y + local_y);
            assert_eq!(window.kernel().hit_test(x, y), Some(button));
            let [bx, by, bw, bh] = snapshot.find("popup-apply").unwrap().bounds;
            near(bx + bw / 4.0, x);
            near(by + bh / 2.0, y);
            let revision = snapshot.observation.unwrap().drawing_revision.unwrap();
            let click = json!({"op":"tap_at","target":button_target,"x":x,"y":y,"drawing_revision":revision});
            let reply = inspect::handle_line(&mut window, &click.to_string());
            assert!(reply.ok, "{reply:?}");
            assert_eq!(reply.pointer.last().unwrap().captured.as_ref().unwrap().target, *button_target);
            assert_eq!(applied.count.get(), 1);
            let (px, py) = applied.point.get().expect("the press has an element-relative point");
            near(px, local_x);
            near(py, local_y);
        }
    }
    assert_eq!(window.kernel().counters().layout_runs, 0, "animation samples only move the presented geometry");

    app.set_reduced_motion(true);
    let safe_area = Rect::new(48.0, 24.0, 504.0, 452.0);
    window.event(InputEvent::Resize { width: 600.0, height: 500.0, scale: 1.0, safe_area: Some(safe_area) });
    window.invoke("reorder").unwrap();
    assert_eq!(window.size(), (600.0, 500.0));
    let (x, y, w, h) = window.kernel().shown_rect(panel);
    assert_eq!((x, y, w, h), rect(&window, "reordering-popup"));
    let (_, anchor_y, _, anchor_h) = window.kernel().shown_rect(trigger);
    assert_eq!(y, anchor_y + anchor_h + 4.0);
    assert_eq!(y, before_panel.1);
    assert!(x > before_panel.0, "the inset constrains the popup's left edge");
    let shown = Rect::new(x, y, w, h);
    assert_eq!(shown.intersect(&safe_area), shown);
    let selection = hosted_selection(&window);
    near(selection.x - before_editor.x, x - before_panel.0);
    near(selection.y - before_editor.y, y - before_panel.1);
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
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
