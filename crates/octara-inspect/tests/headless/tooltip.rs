use super::*;
use octara::NodeId;
use std::time::Duration;

fn move_pointer(driver: &mut HeadlessDriver, id: WindowId, x: f32) {
    let event = InteractionEvent::PointerMove { id: PointerId(1), pos: Point::new(x, 12.0), mods: Modifiers::empty() };
    driver.dispatch(id, Input::Interaction(event)).unwrap();
}

fn tooltip_portal(driver: &HeadlessDriver, id: WindowId) -> Option<NodeId> {
    let tree = driver.app().runtime().window(id).unwrap().tree();
    let mut tooltips = tree.overlay_nodes().filter(|&node| tree.semantics(node).is_some_and(|slot| slot.role == Some(Role::Tooltip)));
    let tooltip = tooltips.next();
    assert!(tooltips.next().is_none());
    tooltip
}

#[test]
fn window_tooltip_uses_virtual_dwell_and_the_hovered_nodes_theme() {
    let mut driver = driver();
    let dark = driver.app_mut().build(|b| b.signal(false)).unwrap();
    let modal = driver.app_mut().build(|b| b.signal(false)).unwrap();
    let light_color = rgb(0xaabbaau32);
    let dark_color = rgb(0x223344u32);
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            row()
                .selectable(true)
                .test_id("document")
                .theme_patch(Prop::Dyn(Box::new(move |read| {
                    ThemePatch::default().color(ColorToken::SurfaceElevated, if read.get(dark) { dark_color } else { light_color })
                })))
                .child(el().test_id("first").size(36).tooltip("First").child(text("A")))
                .child(el().test_id("second").size(36).tooltip("Second").child(text("B")))
                .portal_part(move |r| r.get(modal), |_| el().size(100).layer(LayerPolicy::modal(), |_| {}))
        })
        .unwrap();
    let first = target(&driver, id, "first").node().unwrap();
    let second = target(&driver, id, "second").node().unwrap();
    let at = Duration::from_millis;
    move_pointer(&mut driver, id, 6.0);
    assert_eq!(driver.app().runtime().next_deadline(), Some(at(450)));
    driver.advance_to(at(449)).unwrap();
    assert_eq!(tooltip_portal(&driver, id), None);
    driver.advance_to(at(450)).unwrap();

    let portal = tooltip_portal(&driver, id).unwrap();
    let window = driver.app().runtime().window(id).unwrap();
    let tree = window.tree();
    assert_eq!(tree.logical_parent(portal), Some(first));
    assert_eq!(tree.physical_parent(portal), Some(tree.overlay_root()));
    assert_eq!(tree.overlay(portal).unwrap().target.unwrap().0, AnchorTarget::Node(first));
    assert_eq!(tree.visual(portal).background, Some(light_color));
    let bounds = window.geometry(portal).unwrap().rect;
    assert!(bounds.x >= 0.0 && bounds.y >= 0.0);
    let region = tree.test_ids("document")[0];
    assert_eq!(tree.selectable_texts(region).len(), 2);
    let center = Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0);
    assert!(window.hit_test(center).and_then(|hit| hit.hit.node()).is_none_or(|node| !tree.is_inert(node)));

    driver.app_mut().update(|w| w.set(dark, true)).unwrap();
    let tree = driver.app().runtime().window(id).unwrap().tree();
    assert_eq!(tree.visual(portal).background, Some(dark_color));

    move_pointer(&mut driver, id, 47.0);
    assert_eq!(tooltip_portal(&driver, id), None);
    driver.advance_to(at(699)).unwrap();
    assert_eq!(tooltip_portal(&driver, id), None);
    driver.advance_to(at(700)).unwrap();
    let portal = tooltip_portal(&driver, id).unwrap();
    assert_eq!(driver.app().runtime().window(id).unwrap().tree().logical_parent(portal), Some(second));

    driver.dispatch(id, Input::Interaction(InteractionEvent::Key(KeyEvent::new(Key::F(3), Modifiers::empty())))).unwrap();
    assert_eq!(tooltip_portal(&driver, id), None);
    driver.advance_to(at(1200)).unwrap();
    assert_eq!(tooltip_portal(&driver, id), None);
    move_pointer(&mut driver, id, 47.0);
    driver.advance_to(at(1650)).unwrap();
    assert!(tooltip_portal(&driver, id).is_some());
    driver.app_mut().update(|w| w.set(modal, true)).unwrap();
    assert_eq!(tooltip_portal(&driver, id), None);
}

#[test]
fn stationary_hover_rechecks_layout_and_disabled_targets() {
    let mut driver = driver();
    let (offset, disabled) = driver.app_mut().build(|b| (b.signal(0), b.signal(false))).unwrap();
    let id = driver
        .app_mut()
        .open_window(options(), move |_| {
            row()
                .child(el().style_with(move |read, style| style.w(read.get(offset))))
                .child(el().size(36).disabled(disabled).tooltip("Help"))
        })
        .unwrap();
    let at = Duration::from_millis;
    move_pointer(&mut driver, id, 6.0);
    driver.advance_to(at(200)).unwrap();
    driver.app_mut().update(|w| w.set(offset, 80)).unwrap();
    driver.advance_to(at(450)).unwrap();
    assert_eq!(tooltip_portal(&driver, id), None);
    driver.app_mut().update(|w| w.set(offset, 0)).unwrap();
    driver.advance_to(at(900)).unwrap();
    assert!(tooltip_portal(&driver, id).is_some());
    driver.app_mut().update(|w| w.set(disabled, true)).unwrap();
    assert_eq!(tooltip_portal(&driver, id), None);
}

#[test]
fn window_definition_can_replace_or_disable_the_tooltip_layer() {
    let mut driver = driver();
    let content = |_: &mut BuildCx<'_>| el().size(36).tooltip("Hint");
    let custom = WindowTooltipLayer::new(|_, tip| el().test_id("custom-tooltip").child(text(tip.text.clone())));
    let [custom, disabled] = AppDefinition::new()
        .window(WindowDefinition::new(options(), content).tooltip_layer(Some(custom)))
        .window(WindowDefinition::new(options(), content).tooltip_layer(None))
        .launch(driver.app_mut(), None)
        .unwrap()
        .try_into()
        .unwrap();
    move_pointer(&mut driver, custom, 10.0);
    move_pointer(&mut driver, disabled, 10.0);
    driver.advance_to(Duration::from_millis(450)).unwrap();
    assert_eq!(tooltip_portal(&driver, custom), target(&driver, custom, "custom-tooltip").node());
    assert_eq!(tooltip_portal(&driver, disabled), None);

    driver.dispatch(custom, Input::Interaction(InteractionEvent::PointerLeave)).unwrap();
    assert_eq!(tooltip_portal(&driver, custom), None);

    move_pointer(&mut driver, custom, 10.0);
    driver.advance_to(Duration::from_millis(900)).unwrap();
    assert!(tooltip_portal(&driver, custom).is_some());
    let pos = Point::new(10.0, 12.0);
    let hit = driver.app().runtime().window(custom).unwrap().hit_test(pos).unwrap();
    driver
        .dispatch(
            custom,
            Input::Interaction(InteractionEvent::PointerDown {
                id: PointerId(1),
                kind: PointerKind::Mouse,
                target: hit,
                pos,
                button: Button::Primary,
                mods: Modifiers::empty(),
            }),
        )
        .unwrap();
    assert_eq!(tooltip_portal(&driver, custom), None);
    driver
        .dispatch(
            custom,
            Input::Interaction(InteractionEvent::PointerUp { id: PointerId(1), target: Some(hit), pos, mods: Modifiers::empty() }),
        )
        .unwrap();

    move_pointer(&mut driver, custom, 10.0);
    driver.advance_to(Duration::from_millis(1350)).unwrap();
    assert!(tooltip_portal(&driver, custom).is_some());
    driver
        .dispatch(
            custom,
            Input::Interaction(InteractionEvent::Wheel {
                pos,
                delta: Vec2::new(0.0, 10.0),
                phase: octara::WheelPhase::Discrete,
                mods: Modifiers::empty(),
            }),
        )
        .unwrap();
    assert_eq!(tooltip_portal(&driver, custom), None);
}
