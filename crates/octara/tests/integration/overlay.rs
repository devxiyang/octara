//! Overlays: shown above everything, placed next to their anchor, flipped and clamped.

use octara::app::DEFAULT_VIEWPORT;
use octara::kernel::Placed;
use octara::prelude::*;

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
                .into()
            },
        ))
        .pl(px(left))
        .pt(px(top))
        .into()
    }
}

fn rect(app: &App, id: &str) -> (f32, f32, f32, f32) {
    let k = app.kernel();
    k.rect(k.find(id).unwrap())
}

fn side(app: &App) -> Option<Side> {
    let k = app.kernel();
    k.placed_side(k.find("panel").unwrap())
}

#[test]
fn opens_below_its_anchor_and_closes() {
    let mut app = App::new(trigger_at(10.0, 20.0, Placement::BELOW));
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
fn flips_when_there_is_no_room_and_clamps_into_the_window() {
    let (vw, vh) = DEFAULT_VIEWPORT;
    let mut app = App::new(trigger_at(vw - 20.0, vh - 40.0, Placement::BELOW));
    app.dispatch(Event::Press("trigger".into())).unwrap();
    let (_, ty, _, _) = rect(&app, "trigger");
    let (px, py, pw, ph) = rect(&app, "panel");
    assert_eq!(side(&app), Some(Side::Top), "no room below: opens above");
    assert_eq!(py + ph + 4.0, ty);
    assert_eq!(px + pw, vw - 8.0, "kept 8 px inside the right edge");
}

#[test]
fn overlay_state_survives_hot_reload_and_leaves_no_residue() {
    let mut app = App::new(trigger_at(0.0, 0.0, Placement::BELOW));
    app.dispatch(Event::Press("trigger".into())).unwrap();
    app.dispatch(Event::Press("pick".into())).unwrap();
    let before = app.kernel().tree_dump();
    app.hot_reload();
    assert_eq!(app.kernel().tree_dump(), before);
    assert_eq!(rect(&app, "panel").1, rect(&app, "trigger").3 + 4.0);
    app.dispatch(Event::Press("trigger".into())).unwrap();
    let k = app.kernel();
    assert!(k.children(k.overlay_layer()).is_empty());
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
