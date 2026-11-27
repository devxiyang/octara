use super::{HeadlessDriver, Input, InteractionEvent, PointerId, PointerKind, driver, options, target};
use octara::NodeId;
use octara::prelude::*;
use octara::tree::NodeFlags;
use octara_model::{
    input::{HostReply, WheelPhase},
    motion::{Curve, Spring},
    paint::{Item, Quad},
};
use std::time::Duration;

fn node(driver: &HeadlessDriver, id: WindowId, name: &str) -> NodeId {
    target(driver, id, name).node().unwrap()
}

fn offset(driver: &HeadlessDriver, id: WindowId, port: NodeId) -> Vec2 {
    driver.app().runtime().window(id).unwrap().tree().scroll(port).unwrap().offset
}

fn top_row(driver: &HeadlessDriver, id: WindowId) -> NodeId {
    driver.app().runtime().window(id).unwrap().hit_test(Point::new(10.0, 1.0)).unwrap().hit.node().unwrap()
}

#[test]
fn keyboard_focus_reveals_nested_scrollports_and_respects_a_request() {
    let mut driver = driver();
    let id = driver
        .app_mut()
        .open_window(options(), |_| {
            col()
                .test_id("outer")
                .w(120)
                .h(80)
                .scroll_y()
                .child(el().test_id("start").h(130).shrink(0.0).focusable())
                .child(
                    col()
                        .test_id("inner")
                        .h(60)
                        .shrink(0.0)
                        .scroll_y()
                        .child(el().h(120).shrink(0.0))
                        .child(el().test_id("target").h(20).shrink(0.0).focusable()),
                )
                .child(el().h(140).shrink(0.0))
        })
        .unwrap();
    let outer = node(&driver, id, "outer");
    let inner = node(&driver, id, "inner");
    let end = target(&driver, id, "target");
    driver.dispatch(id, Input::Host(HostReply::FocusChanged { target: Some(end), visible: false })).unwrap();
    assert_eq!(offset(&driver, id, outer).y, 0.0);
    assert_eq!(offset(&driver, id, inner).y, 0.0);

    driver
        .app_mut()
        .update_window(id, |w| {
            w.scroll_to(outer, Vec2::new(0.0, 20.0));
            w.focus("target");
        })
        .unwrap();
    assert_eq!(offset(&driver, id, inner).y, 80.0);
    assert_eq!(offset(&driver, id, outer).y, 20.0);

    driver.app_mut().update_window(id, |w| w.focus("start")).unwrap();
    driver.dispatch(id, Input::Interaction(InteractionEvent::Key(KeyEvent::new(Key::Tab, Modifiers::empty())))).unwrap();
    assert_eq!(driver.app().runtime().window(id).unwrap().tree().focus(), end.node());
    assert_eq!(offset(&driver, id, inner).y, 80.0);
    assert_eq!(offset(&driver, id, outer).y, 110.0);
}

#[test]
fn keyboard_focus_reveals_a_new_inline_link_using_current_text_layout() {
    let mut driver = driver();
    let id = driver
        .app_mut()
        .open_window(options(), |_| {
            col()
                .test_id("port")
                .w(220)
                .h(45)
                .scroll_y()
                .child(text(rich().text("one\ntwo\nthree\nfour\nfive\n").link("end", "#end")).test_id("rich-text").shrink(0.0))
        })
        .unwrap();
    let port = node(&driver, id, "port");
    let rich_text = node(&driver, id, "rich-text");
    let link = driver.app().runtime().window(id).unwrap().tree().children(rich_text)[0];
    pointer(&mut driver, id, InteractionEvent::Key(KeyEvent::new(Key::Tab, Modifiers::empty())));
    assert_eq!(driver.app().runtime().window(id).unwrap().tree().focus(), Some(link));
    assert!(offset(&driver, id, port).y > 35.0);
}

#[test]
fn programmatic_scroll_precedes_follow_tail_when_the_range_changes_in_the_same_turn() {
    let mut driver = driver();
    let height = driver.app_mut().build(|b| b.signal(180)).unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col()
                .test_id("port")
                .w(120)
                .h(80)
                .scroll_y()
                .follow_tail()
                .child(el().test_id("old-content").h(180).shrink(0.0))
                .child(el().shrink(0.0).style_with(move |r, style| style.h(r.get(height) - 180)))
        })
        .unwrap();
    let port = node(&driver, id, "port");
    assert_eq!(offset(&driver, id, port).y, 100.0);
    driver
        .app_mut()
        .update_window(id, |w| {
            w.set(height, 280);
            w.scroll_to(port, Vec2::new(0.0, 100.0));
        })
        .unwrap();
    assert_eq!(offset(&driver, id, port).y, 100.0);
    assert!(!driver.app().runtime().window(id).unwrap().tree().node(port).unwrap().flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE));
    driver.app_mut().update_window(id, |w| w.scroll_to_end("port")).unwrap();
    assert_eq!(offset(&driver, id, port).y, 200.0);
}

#[test]
fn a_scroll_to_end_glide_keeps_follow_tail_alive_until_it_arrives() {
    let mut driver = driver();
    let id = driver
        .app_mut()
        .open_window(options(), |_| {
            col()
                .test_id("port")
                .w(120)
                .h(80)
                .scroll_y()
                .follow_tail()
                .style(StyleProp::LayoutMotion(Curve::Spring(Spring::GLIDE)))
                .child(el().test_id("long-row").h(320).shrink(0.0).bg(rgb(0x224466)))
        })
        .unwrap();
    let port = node(&driver, id, "port");
    let row = node(&driver, id, "long-row");
    let mods = Modifiers::empty();
    pointer(
        &mut driver,
        id,
        InteractionEvent::Wheel { pos: Point::new(30.0, 20.0), delta: Vec2::new(0.0, -80.0), phase: WheelPhase::Discrete, mods },
    );
    assert_eq!(offset(&driver, id, port).y, 160.0);
    driver.app_mut().update_window(id, |w| w.scroll_to_end("port")).unwrap();
    assert_eq!(offset(&driver, id, port).y, 160.0);
    driver.advance_to(Duration::from_millis(70)).unwrap();
    let current = offset(&driver, id, port).y;
    assert!((160.0..240.0).contains(&current));
    let state = driver.app().runtime().window(id).unwrap();
    let painted_y = state.last_commit().unwrap().draw.as_ref().unwrap().items().into_iter().find_map(|item| match item {
        Item::Quad(quad) if quad.source == row => Some(quad.rect.y),
        _ => None,
    });
    assert_eq!(painted_y, Some(-current));
    driver.advance_to(Duration::from_secs(1)).unwrap();
    assert_eq!(offset(&driver, id, port).y, 240.0);
    assert!(driver.app().runtime().window(id).unwrap().tree().node(port).unwrap().flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE));
}

#[test]
fn virtual_anchor_preserves_identity_and_uses_measured_row_changes_once() {
    let mut driver = driver();
    let rows = driver.app_mut().build(|b| b.list::<usize, u16>()).unwrap();
    driver
        .app_mut()
        .update(|w| {
            for i in 0..80 {
                w.push_row(rows, i, 20);
            }
        })
        .unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col().test_id("virtual-port").w(120).h(70).scroll_y().child(virtual_each(
                rows,
                VirtualOptions { estimated_row_height: 20.0, overscan: 2, maintain_scroll_anchor: true },
                |_, &key, row| {
                    el().test_id(format!("row-{key}")).style_with(move |r, style| style.h(i32::from(r.try_get(row).unwrap_or(20))))
                },
            ))
        })
        .unwrap();
    let port = node(&driver, id, "virtual-port");
    driver.app_mut().update_window(id, |w| w.scroll_to(port, Vec2::new(0.0, 405.0))).unwrap();
    let anchor = node(&driver, id, "row-20");
    assert_eq!(top_row(&driver, id), anchor);
    let assert_anchor = |driver: &HeadlessDriver, expected| {
        assert_eq!(node(driver, id, "row-20"), anchor);
        assert_eq!(top_row(driver, id), anchor);
        assert_eq!(offset(driver, id, port).y, expected);
        assert_eq!(driver.app().runtime().window(id).unwrap().geometry(anchor).unwrap().rect.y - expected, -5.0);
    };
    driver.app_mut().update(|w| w.insert_row(rows, 0, 90, 20)).unwrap();
    assert_anchor(&driver, 425.0);
    driver.app_mut().update(|w| w.update_row(rows, &19, |height| *height = 35)).unwrap();
    assert_anchor(&driver, 440.0);
    driver
        .app_mut()
        .update(|w| {
            w.remove_row(rows, &0);
            w.move_row(rows, &70, 0);
            w.move_row(rows, &20, 30);
        })
        .unwrap();
    assert_anchor(&driver, 620.0);
}

fn thumb(driver: &HeadlessDriver, id: WindowId, port: NodeId) -> Option<Quad> {
    driver.app().runtime().window(id)?.last_commit()?.draw.as_ref()?.items().into_iter().find_map(|item| match item {
        Item::Quad(quad) if quad.source == port && quad.radius == [2.5; 4] => Some(quad),
        _ => None,
    })
}

fn pointer(driver: &mut HeadlessDriver, id: WindowId, event: InteractionEvent) {
    driver.dispatch(id, Input::Interaction(event)).unwrap();
}

fn down(driver: &mut HeadlessDriver, window: WindowId, id: PointerId, pos: Point) {
    let target = driver.app().runtime().window(window).unwrap().hit_test(pos).unwrap();
    pointer(
        driver,
        window,
        InteractionEvent::PointerDown { id, pos, target, kind: PointerKind::Mouse, button: Button::Primary, mods: Modifiers::empty() },
    );
}

#[test]
fn scrollbar_pages_drags_detaches_and_cancels_when_hidden() {
    let mut driver = driver();
    let visible = driver.app_mut().build(|b| b.signal(true)).unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            col()
                .test_id("port")
                .w(100)
                .h(80)
                .scroll_y()
                .follow_tail()
                .style_with(move |r, style| {
                    style.style(StyleProp::Scrollbar(if r.get(visible) {
                        ScrollbarVisibility::Always
                    } else {
                        ScrollbarVisibility::Hidden
                    }))
                })
                .child(el().h(400).shrink(0.0))
        })
        .unwrap();
    let port = node(&driver, id, "port");
    assert_eq!(offset(&driver, id, port).y, 320.0);
    let contact = PointerId(1);
    let mods = Modifiers::empty();
    let track = Point::new(95.0, 5.0);
    down(&mut driver, id, contact, track);
    pointer(&mut driver, id, InteractionEvent::PointerUp { id: contact, target: None, pos: track, mods });
    assert_eq!(offset(&driver, id, port).y, 240.0);
    assert!(!driver.app().runtime().window(id).unwrap().tree().node(port).unwrap().flags.contains(NodeFlags::FOLLOW_TAIL_ACTIVE));
    let rect = thumb(&driver, id, port).unwrap().rect;
    let grab = Point::new(rect.x + 1.0, rect.y + 1.0);
    down(&mut driver, id, contact, grab);
    let at = Point::new(170.0, grab.y - 16.0);
    pointer(&mut driver, id, InteractionEvent::PointerMove { id: contact, pos: at, mods });
    let dragged = offset(&driver, id, port).y;
    assert!(dragged < 160.0, "{dragged}");
    driver.app_mut().update(|w| w.set(visible, false)).unwrap();
    assert!(thumb(&driver, id, port).is_none());
    pointer(&mut driver, id, InteractionEvent::PointerUp { id: contact, target: None, pos: Point::new(170.0, 0.0), mods });
    assert_eq!(offset(&driver, id, port).y, dragged);
}

#[test]
fn auto_scrollbar_shows_for_wheel_and_edge_hover_then_fades() {
    let mut driver = driver();
    let id =
        driver.app_mut().open_window(options(), |_| col().test_id("port").w(100).h(80).scroll_y().child(el().h(400).shrink(0.0))).unwrap();
    let port = node(&driver, id, "port");
    assert!(thumb(&driver, id, port).is_none());
    let edge = Point::new(95.0, 30.0);
    let mods = Modifiers::empty();
    pointer(&mut driver, id, InteractionEvent::PointerMove { id: PointerId(1), pos: edge, mods });
    let hovered = thumb(&driver, id, port).unwrap().fill.unwrap().components[3];
    driver.advance_to(Duration::from_secs(2)).unwrap();
    assert_eq!(thumb(&driver, id, port).unwrap().fill.unwrap().components[3], hovered);
    pointer(&mut driver, id, InteractionEvent::PointerLeave);
    pointer(
        &mut driver,
        id,
        InteractionEvent::Wheel { pos: Point::new(50.0, 25.0), delta: Vec2::new(0.0, 40.0), phase: WheelPhase::Discrete, mods },
    );
    let initial = thumb(&driver, id, port).unwrap().fill.unwrap().components[3];
    driver.advance_to(Duration::from_millis(2800)).unwrap();
    let fading = thumb(&driver, id, port).unwrap().fill.unwrap().components[3];
    assert!(fading > 0.0 && fading < initial && initial < hovered);
    driver.advance_to(Duration::from_secs(3)).unwrap();
    assert!(thumb(&driver, id, port).is_none());
}
