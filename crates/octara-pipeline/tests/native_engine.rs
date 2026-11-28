use octara_model::geometry::{Point, Rect, Size, Vec2, WindowViewport};
use octara_model::identity::{NativeId, TreeId};
use octara_model::input::{Button, HostReply, InteractionEvent, Modifiers, PointerId, PointerKind};
use octara_model::motion::{Curve, Motion};
use octara_model::native::{NativeChange, NativeSlot};
use octara_model::paint::Item;
use octara_model::semantics::Role;
use octara_model::style::{FlexDirection, Length, Overflow, Position, Radius, State, StyleProp, Transform3D, When};
use octara_model::theme::{Color, ColorValue, Fill};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication, a11y::node_identity};
use octara_runtime::engine::AccessibilityConsumer;
use octara_runtime::{AppRuntime, HandlerEvent, HostEngine, Input, WindowOptions};
use octara_tree::tables::{AuthoredStyle, HandlerKind, SemanticsSlot};
use octara_tree::{NodeChange, NodeContent, NodeFlags, NodeSpec, RetiredNode, Retirement, RuntimeOp, Tree, TreeDelta};
use std::cell::RefCell;
use std::rc::Rc;

fn tree() -> Tree {
    Tree::new(TreeId::new([17, 33], 1), Default::default())
}

fn viewport() -> WindowViewport {
    WindowViewport { size: Size::new(320.0, 240.0), scale: 2.0, safe_area: None, keyboard_inset: 0.0 }
}

fn engine() -> NativeEngine {
    NativeEngine::new(NativeEngineConfig { check_paint_cache: true, check_a11y_cache: true, ..Default::default() })
}

#[test]
fn projected_buttons_deliver_press_positions_in_their_local_plane() {
    fn bounds(x: i32, y: i32, w: i32, h: i32) -> [StyleProp; 5] {
        [
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(x.into()),
            StyleProp::Top(y.into()),
            StyleProp::Width(w.into()),
            StyleProp::Height(h.into()),
        ]
    }

    fn style(props: impl IntoIterator<Item = StyleProp>) -> NodeChange {
        NodeChange::Style(Box::new(AuthoredStyle { base: props.into_iter().collect(), ..Default::default() }))
    }

    fn project(transform: Transform3D, rect: Rect, point: Point) -> Point {
        let matrix = transform.device_matrix(rect, 1.0);
        let [x, y, _, w] = std::array::from_fn(|row| matrix[0][row] * point.x + matrix[1][row] * point.y + matrix[3][row]);
        Point::new(x / w, y / w)
    }

    let shift = Transform3D::default().translate(100.0, 0.0, 0.0);
    let outer_transform = Transform3D::default().perspective(180.0).translate(100.0, 0.0, 0.0).rotate_y(38.0);
    let inner_transform = Transform3D::default().perspective(150.0).translate(22.0, 3.0, 30.0).rotate_x(35.0);
    let pressed = Rc::new(RefCell::new(Vec::new()));
    let mut app = AppRuntime::with_namespace([6, 2], |_, _| Box::new(engine()));
    let events = pressed.clone();
    let window = app
        .open_window(WindowOptions { viewport: viewport(), ..Default::default() }, move |cx| {
            let button = |cx: &mut octara_runtime::MountCx<'_>, name| {
                let node = cx.create(NodeSpec::new(NodeContent::Box));
                cx.change(node, NodeChange::TestId(Some(name)));
                let events = events.clone();
                cx.on(node, HandlerKind::Press, move |_, event| {
                    if let HandlerEvent::Interaction { local_pos, .. } = event {
                        events.borrow_mut().push(*local_pos);
                    }
                });
                node
            };
            let clip = [StyleProp::OverflowX(Overflow::Hidden), StyleProp::OverflowY(Overflow::Hidden)];
            let shifted = button(cx, "shifted".into());
            cx.change(shifted, style(bounds(0, 0, 40, 40).into_iter().chain([StyleProp::Transform3D(Some(shift))])));
            cx.place(cx.root(), 0, shifted);
            let window_clip = cx.create(NodeSpec::new(NodeContent::Box));
            cx.change(window_clip, style(bounds(0, 80, 183, 55).into_iter().chain(clip)));
            cx.place(cx.root(), 1, window_clip);
            let outer = cx.create(NodeSpec::new(NodeContent::Box));
            cx.change(outer, style(bounds(0, 0, 100, 80).into_iter().chain(clip).chain([StyleProp::Transform3D(Some(outer_transform))])));
            cx.place(window_clip, 0, outer);
            let inner = button(cx, "nested".into());
            cx.change(inner, style(bounds(35, 20, 40, 40).into_iter().chain([StyleProp::Transform3D(Some(inner_transform))])));
            cx.place(outer, 0, inner);
        })
        .unwrap();

    let runtime = app.window(window).unwrap();
    let shifted = runtime.tree().test_ids("shifted")[0];
    let nested = runtime.tree().test_ids("nested")[0];
    let hit = |app: &AppRuntime, point| app.window(window).unwrap().hit_test(point).and_then(|target| target.hit.node());
    assert_eq!(runtime.geometry(shifted).unwrap().rect, Rect::new(0.0, 0.0, 40.0, 40.0));
    assert_eq!(runtime.geometry(nested).unwrap().rect, Rect::new(35.0, 20.0, 40.0, 40.0));
    assert_ne!(hit(&app, Point::new(8.0, 12.0)), Some(shifted));
    assert_eq!(hit(&app, Point::new(108.0, 12.0)), Some(shifted));

    let nested_position = |local: Point| {
        let inner = project(inner_transform, Rect::new(35.0, 100.0, 40.0, 40.0), Point::new(35.0 + local.x, 100.0 + local.y));
        let screen = project(outer_transform, Rect::new(0.0, 80.0, 100.0, 80.0), inner);
        (inner, screen)
    };
    let (_, nested_hit) = nested_position(Point::new(4.0, 8.0));
    let (inner, outside_window_clip) = nested_position(Point::new(18.0, 33.0));
    assert!(inner.x < 100.0 && outside_window_clip.y >= 135.0);
    let (inner, outside_layer_clip) = nested_position(Point::new(39.0, 8.0));
    assert!(inner.x >= 100.0 && Rect::new(0.0, 80.0, 183.0, 55.0).contains(outside_layer_clip));
    assert_eq!(hit(&app, nested_hit), Some(nested));
    assert_ne!(hit(&app, outside_window_clip), Some(nested));
    assert_ne!(hit(&app, outside_layer_clip), Some(nested));
    assert_ne!(hit(&app, Point::new(39.0, 108.0)), Some(nested));

    for pos in [Point::new(8.0, 12.0), Point::new(108.0, 12.0), nested_hit, outside_window_clip, outside_layer_clip] {
        let target = app.window(window).unwrap().hit_test(pos);
        if let Some(target) = target {
            app.dispatch(
                window,
                Input::Interaction(InteractionEvent::PointerDown {
                    id: PointerId(1),
                    kind: PointerKind::Mouse,
                    target,
                    pos,
                    button: Button::Primary,
                    mods: Modifiers::empty(),
                }),
            )
            .unwrap();
            app.dispatch(
                window,
                Input::Interaction(InteractionEvent::PointerUp { id: PointerId(1), target: Some(target), pos, mods: Modifiers::empty() }),
            )
            .unwrap();
        }
    }
    let pressed = pressed.borrow();
    assert_eq!(pressed.len(), 2);
    assert_eq!(pressed[0], Some(Point::new(8.0, 12.0)));
    let local = pressed[1].unwrap();
    assert!((local.x - 4.0).abs() < 0.001 && (local.y - 8.0).abs() < 0.001, "nested local point: {local:?}");
}

#[test]
fn paint_only_transform_changes_stage_and_rollback_projected_hits() {
    let mut tree = tree();
    let style = |transform| {
        NodeChange::Style(Box::new(AuthoredStyle {
            base: [
                StyleProp::Position(Position::Absolute),
                StyleProp::Width(40.into()),
                StyleProp::Height(40.into()),
                StyleProp::Transform3D(transform),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        }))
    };
    let mut tx = tree.transaction();
    let node = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(node, style(Some(Transform3D::default().translate(100.0, 0.0, 0.0))));
    tx.place(tree.node_ref(tree.root()), 0, node);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let node = receipt.node(node).unwrap();
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = NativeEngine::new(NativeEngineConfig { check_layout_cache: true, ..Default::default() });
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();
    let hits_node =
        |engine: &NativeEngine, tree: &Tree, x| engine.hit_test(tree, Point::new(x, 10.0)).and_then(|hit| hit.hit.node()) == Some(node);
    assert!(hits_node(&engine, &tree, 110.0));

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(node), style(None));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    assert!(delta.layout_dirty.is_empty());
    engine.project(&tree, &delta, viewport(), 1.0).unwrap();
    assert!(hits_node(&engine, &tree, 10.0));
    assert!(!hits_node(&engine, &tree, 110.0));
    engine.rollback();
    assert!(!hits_node(&engine, &tree, 10.0));
    assert!(hits_node(&engine, &tree, 110.0));

    engine.project(&tree, &delta, viewport(), 2.0).unwrap();
    engine.commit();
    assert!(hits_node(&engine, &tree, 10.0));
    assert!(!hits_node(&engine, &tree, 110.0));
}

#[test]
fn projected_controls_use_the_rendered_scroll_placement_for_local_points() {
    let mut tree = tree();
    let style =
        |base: Vec<StyleProp>| NodeChange::Style(Box::new(AuthoredStyle { base: base.into_iter().collect(), ..Default::default() }));
    let mut tx = tree.transaction();
    let port = tx.create(NodeSpec::new(NodeContent::Box));
    let button = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        port,
        style(vec![
            StyleProp::Position(Position::Relative),
            StyleProp::Width(60.into()),
            StyleProp::Height(40.into()),
            StyleProp::OverflowY(Overflow::Scroll),
        ]),
    );
    tx.change(
        button,
        style(vec![
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(4.into()),
            StyleProp::Top(20.into()),
            StyleProp::Width(40.into()),
            StyleProp::Height(40.into()),
            StyleProp::Transform3D(Some(Transform3D::default().translate(10.0, 0.0, 0.0))),
        ]),
    );
    tx.place(tree.node_ref(tree.root()), 0, port);
    tx.place(port, 0, button);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [port, button] = [port, button].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = engine();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();

    let delta = tree.apply_runtime(RuntimeOp::RequestScroll { node: port, target: Vec2::new(0.0, 0.3) });
    engine.project(&tree, &delta, viewport(), 1.0).unwrap();
    engine.commit();
    let pos = Point::new(20.0, 19.6);
    assert_eq!(engine.hit_test(&tree, pos).and_then(|hit| hit.hit.node()), Some(button));
    let local = engine.interaction_local_point(button, pos).unwrap();
    assert_eq!(local.x, 6.0);
    assert!((local.y - 0.1).abs() < 0.001, "local point: {local:?}");
}

#[test]
fn ancestor_clipping_leaves_the_original_rounded_child_clip_in_place() {
    let mut tree = tree();
    let mut tx = tree.transaction();
    let outer = tx.create(NodeSpec::new(NodeContent::Box));
    let rounded = tx.create(NodeSpec::new(NodeContent::Box));
    let leaf = tx.create(NodeSpec::new(NodeContent::Box));
    let style =
        |props: &[StyleProp]| NodeChange::Style(Box::new(AuthoredStyle { base: props.iter().copied().collect(), ..Default::default() }));
    let clip = [StyleProp::OverflowX(Overflow::Hidden), StyleProp::OverflowY(Overflow::Hidden)];
    tx.change(outer, style(&[StyleProp::Width(40.into()), StyleProp::Height(40.into()), clip[0], clip[1]]));
    tx.change(
        rounded,
        style(&[
            StyleProp::Width(80.into()),
            StyleProp::Height(80.into()),
            StyleProp::Position(Position::Absolute),
            StyleProp::Left((-20).into()),
            StyleProp::Top((-20).into()),
            StyleProp::RadiusTopLeft(Radius::Px(20.0)),
            clip[0],
            clip[1],
        ]),
    );
    tx.change(
        leaf,
        style(&[StyleProp::Width(80.into()), StyleProp::Height(80.into()), StyleProp::Background(Some(Fill::Solid(Color::BLACK.into())))]),
    );
    tx.place(tree.node_ref(tree.root()), 0, outer);
    tx.place(outer, 0, rounded);
    tx.place(rounded, 0, leaf);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [rounded, leaf] = [rounded, leaf].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = engine();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    let draw = engine.commit().draw.unwrap();
    assert!(draw.items().contains(&Item::PushClip {
        source: rounded,
        rect: Rect::new(-20.0, -20.0, 80.0, 80.0),
        radius: [20.0, 0.0, 0.0, 0.0]
    }));
    assert_eq!(engine.hit_test(&tree, Point::new(2.0, 2.0)).and_then(|target| target.hit.node()), Some(leaf));
}

#[test]
fn changing_a_single_z_index_reorders_the_retained_scene_and_hit_test() {
    let mut tree = tree();
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.root());
    let box_style = |z_index| AuthoredStyle {
        base: [
            StyleProp::Position(Position::Absolute),
            StyleProp::Width(40.into()),
            StyleProp::Height(40.into()),
            StyleProp::Background(Some(Fill::Solid(Color::BLACK.into()))),
            StyleProp::ZIndex(z_index),
        ]
        .into_iter()
        .collect(),
        overrides: [(When::State(State::SELECTED), vec![StyleProp::ZIndex(20)])].into_iter().collect(),
        ..Default::default()
    };
    let nodes: Vec<_> = (0..32)
        .map(|index| {
            let node = tx.create(NodeSpec::new(NodeContent::Box));
            tx.change(node, NodeChange::Style(Box::new(box_style(if index == 0 { 10 } else { 0 }))));
            tx.place(root, index, node);
            node
        })
        .collect();
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let first = receipt.node(nodes[0]).unwrap();
    let second = receipt.node(nodes[1]).unwrap();
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = engine();
    let assert_order = |engine: &mut NativeEngine, tree: &Tree, delta: &TreeDelta, now, front| {
        engine.project(tree, delta, viewport(), now).unwrap();
        let draw = engine.commit().draw.unwrap();
        assert_eq!(draw.quads().last().unwrap().source, front);
        assert_eq!(engine.hit_test(tree, Point::new(5.0, 5.0)).and_then(|target| target.hit.node()), Some(front));
    };
    assert_order(&mut engine, &tree, &initial, 0.0, first);
    let delta = tree.apply_runtime(RuntimeOp::SetState { node: second, state: State::SELECTED, on: true });
    assert!(!delta.topology_dirty.contains(&tree.root()));
    assert_order(&mut engine, &tree, &delta, 1.0, second);
    let delta = tree.apply_runtime(RuntimeOp::SetState { node: second, state: State::SELECTED, on: false });
    assert_order(&mut engine, &tree, &delta, 2.0, first);
}

#[test]
fn pointer_events_skip_only_the_node_and_preserve_focus_and_accessibility() {
    let mut tree = tree();
    let mut tx = tree.transaction();
    let back = tx.create(NodeSpec::new(NodeContent::Box));
    let front = tx.create(NodeSpec::new(NodeContent::Box));
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    let bounds = [
        StyleProp::Position(Position::Absolute),
        StyleProp::Top(Length::Px(0.0)),
        StyleProp::Left(Length::Px(0.0)),
        StyleProp::Width(Length::Px(80.0)),
        StyleProp::Height(Length::Px(80.0)),
    ];
    tx.change(back, NodeChange::Style(Box::new(AuthoredStyle { base: bounds.into_iter().collect(), ..Default::default() })));
    tx.change(
        front,
        NodeChange::Style(Box::new(AuthoredStyle {
            base: bounds.into_iter().collect(),
            overrides: [(When::State(State::SELECTED), vec![StyleProp::PointerEvents(true)])].into_iter().collect(),
            ..Default::default()
        })),
    );
    tx.change(front, NodeChange::Flags(NodeFlags::POINTER_EVENTS_NONE | NodeFlags::FOCUSABLE));
    tx.change(front, NodeChange::Semantics(Box::new(SemanticsSlot { role: Some(Role::Button), ..Default::default() })));
    tx.change(
        child,
        NodeChange::Style(Box::new(AuthoredStyle {
            base: [StyleProp::Width(Length::Px(20.0)), StyleProp::Height(Length::Px(20.0))].into_iter().collect(),
            ..Default::default()
        })),
    );
    tx.place(tree.node_ref(tree.root()), 0, back);
    tx.place(tree.node_ref(tree.overlay_root()), 0, front);
    tx.place(front, 0, child);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [back, front, child] = [back, front, child].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    initial.merge(tree.apply_runtime(RuntimeOp::SetFocus { node: Some(front), visible: true }));
    assert_eq!(tree.focus(), Some(front));

    let mut engine = engine();
    let frames = engine.publication();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();
    engine.set_accessibility_active(&tree, AccessibilityConsumer::Inspector, true);
    let hit = |engine: &NativeEngine, tree: &Tree, point| engine.hit_test(tree, point).and_then(|target| target.hit.node());
    assert_eq!(hit(&engine, &tree, Point::new(40.0, 40.0)), Some(back));
    assert_eq!(hit(&engine, &tree, Point::new(5.0, 5.0)), Some(child));
    assert_eq!(frames.accessibility_node(node_identity(front)), Some(front));
    assert_eq!(frames.snapshot_full().unwrap().focus, node_identity(front));

    let delta = tree.apply_runtime(RuntimeOp::SetState { node: front, state: State::SELECTED, on: true });
    assert!(!tree.node(front).unwrap().flags.contains(NodeFlags::POINTER_EVENTS_NONE));
    assert!(delta.paint_dirty.contains(&child));
    assert!(delta.layout_dirty.is_empty());
    engine.project(&tree, &delta, viewport(), 1.0).unwrap();
    engine.commit();
    assert_eq!(hit(&engine, &tree, Point::new(40.0, 40.0)), Some(front));

    let delta = tree.apply_runtime(RuntimeOp::SetState { node: front, state: State::SELECTED, on: false });
    assert!(tree.node(front).unwrap().flags.contains(NodeFlags::POINTER_EVENTS_NONE));
    engine.project(&tree, &delta, viewport(), 2.0).unwrap();
    engine.commit();
    assert_eq!(hit(&engine, &tree, Point::new(40.0, 40.0)), Some(back));
    assert_eq!(tree.focus(), Some(front));
    assert_eq!(frames.accessibility_node(node_identity(front)), Some(front));
    assert_eq!(frames.snapshot_full().unwrap().focus, node_identity(front));
}

#[test]
fn native_host_preserves_island_changes_when_a_frame_is_skipped() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let web = tx.create(NodeSpec::new(NodeContent::Native { kind: "test.view", slot: NativeSlot::Underlay }));
    tx.place(tree.node_ref(tree.root()), 0, web);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    initial.merge(mounted);
    let web = receipt.node(web).unwrap();
    let id = NativeId::from(web);

    let publication = NativePublication::for_window_host();
    let mut engine = NativeEngine::with_publication(NativeEngineConfig::default(), publication.clone());
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();
    let props = serde_json::json!({"html": "new page"});
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(web), NodeChange::NativeProps(props.clone()));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    let next_viewport = WindowViewport { scale: 3.0, ..viewport() };
    engine.project(&tree, &delta, next_viewport, 1.0).unwrap();
    engine.commit();

    let latest = publication.latest().unwrap();
    assert!(!latest.native_changes.iter().any(|change| matches!(change, NativeChange::Create { .. })));
    let received = publication.take_native_changes();
    assert!(
        matches!(received.first(), Some(NativeChange::Create { id: created, kind, slot: NativeSlot::Underlay, .. }) if *created == id && kind == "test.view")
    );
    assert!(received.contains(&NativeChange::Update { id, props }));
    assert!(publication.take_native_changes().is_empty());
}

fn tinted(opacity: f32) -> NodeChange {
    tinted_with_fade(opacity, 0.2)
}

fn tinted_with_fade(opacity: f32, duration: f32) -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle {
        base: [
            StyleProp::Width(Length::Px(96.0)),
            StyleProp::Height(Length::Px(32.0)),
            StyleProp::Background(Some(Fill::Solid(ColorValue::Color(Color::new([0.1, 0.4, 0.8, 1.0]))))),
            StyleProp::Opacity(opacity),
            StyleProp::Motion(Motion { opacity: Curve::Fade(duration), ..Motion::NONE }),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    }))
}

#[test]
fn nested_exit_paints_the_longer_child_after_the_parent_finishes() {
    for reduced_motion in [false, true] {
        let mut tree = tree();
        let mut initial = tree.initial_delta();
        let mut tx = tree.transaction();
        let parent = tx.create(NodeSpec::new(NodeContent::Box));
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(parent, tinted_with_fade(1.0, 0.1));
        tx.change(child, tinted_with_fade(1.0, 0.4));
        tx.place(tree.node_ref(tree.root()), 0, parent);
        tx.place(parent, 0, child);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let [parent, child] = [parent, child].map(|node| receipt.node(node).unwrap());
        initial.merge(mounted);

        let mut engine = NativeEngine::new(NativeEngineConfig { reduced_motion, check_paint_cache: true, ..Default::default() });
        engine.project(&tree, &initial, viewport(), 0.0).unwrap();
        engine.commit();

        let mut tx = tree.transaction();
        tx.retire(tree.node_ref(parent), Retirement::VisibleExit);
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        let [parent_exit, child_exit] = [parent, child].map(|node| tree.pending_exit(node).unwrap());
        let closing = engine.project(&tree, &delta, viewport(), 0.0).unwrap();
        assert!(!tree.contains_target(tree.guard(child)));
        let closing_draw = engine.commit().draw.unwrap();
        if reduced_motion {
            assert_eq!(closing.replies, [HostReply::ExitFinished(parent_exit), HostReply::ExitFinished(child_exit)]);
            assert!(closing.next_deadline.is_none());
            assert!(!closing_draw.quads().any(|quad| [parent, child].contains(&quad.source)));
        } else {
            assert!(closing.replies.is_empty());
            assert_eq!(closing_draw.quads().filter(|quad| quad.source == child).count(), 1);
            let feedback = engine.project(&tree, &TreeDelta::default(), viewport(), 0.2).unwrap();
            assert_eq!(feedback.replies, [HostReply::ExitFinished(parent_exit)]);
            assert!(feedback.next_deadline.is_some());
            let remaining = engine.commit().draw.unwrap();
            assert!(remaining.quads().any(|quad| quad.source == child));
            assert!(!remaining.quads().any(|quad| quad.source == parent));
            assert!(tree.apply_runtime(RuntimeOp::FinishExit(parent_exit)).is_empty());
            assert!(tree.node(parent).is_some());
            let done = engine.project(&tree, &TreeDelta::default(), viewport(), 0.5).unwrap();
            assert_eq!(done.replies, [HostReply::ExitFinished(child_exit)]);
        }
        tree.apply_runtime(RuntimeOp::FinishExit(parent_exit));
        tree.apply_runtime(RuntimeOp::FinishExit(child_exit));
        assert!(tree.node(parent).is_none());
        assert!(tree.node(child).is_none());
    }
}

#[test]
fn reuses_local_drawings_and_emits_only_changed_accessibility_entries() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let first = tx.create(NodeSpec::new(NodeContent::Text("first".into())));
    let second = tx.create(NodeSpec::new(NodeContent::Text("second".into())));
    tx.place(tree.node_ref(tree.root()), 0, first);
    tx.place(tree.node_ref(tree.root()), 1, second);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    let first = receipt.node(first).unwrap();
    let second = receipt.node(second).unwrap();
    initial.merge(delta);

    let mut engine = engine();
    let frames = engine.publication();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    let draw = engine.commit().draw.unwrap();
    let start = engine.drawings_compiled();
    assert_eq!(draw.glyph_runs().count(), 2);
    assert!(frames.latest().unwrap().accessibility.is_none());
    engine.set_accessibility_active(&tree, AccessibilityConsumer::Inspector, true);
    let first_access = frames.snapshot_full().unwrap();
    assert!(first_access.tree.is_some());
    assert!(first_access.nodes.len() >= 5);

    engine.project(&tree, &TreeDelta::default(), viewport(), 0.05).unwrap();
    let after = engine.commit().draw.unwrap();
    assert_eq!(draw, after);
    assert_eq!(engine.drawings_compiled(), start);
    assert!(frames.latest().unwrap().accessibility.is_none());

    let mut change = tree.transaction();
    change.change(tree.node_ref(second), NodeChange::Text("SECOND".into()));
    let (_, delta) = tree.apply(change.finish()).unwrap();
    engine.project(&tree, &delta, viewport(), 0.1).unwrap();
    engine.commit();
    let frame = frames.latest().unwrap();
    let update = frame.accessibility.as_ref().unwrap();
    assert!(!update.nodes.is_empty());
    assert!(update.nodes.len() < frames.snapshot_full().unwrap().nodes.len());
    assert_eq!(frames.accessibility_node(node_identity(first)), Some(first));
    assert!(engine.text_hit(first, Point::new(1.0, 2.0)).is_some());
    assert!(engine.drawings_compiled() <= start + 2);
}

#[test]
fn accessibility_stays_dormant_then_scroll_updates_only_the_container() {
    let mut tree = tree();
    let mut tx = tree.transaction();
    let panel = tx.create(NodeSpec::new(NodeContent::Box));
    let style =
        |props: &[StyleProp]| NodeChange::Style(Box::new(AuthoredStyle { base: props.iter().copied().collect(), ..Default::default() }));
    tx.change(
        panel,
        style(&[
            StyleProp::Width(200.into()),
            StyleProp::Height(100.into()),
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::OverflowY(Overflow::Scroll),
        ]),
    );
    tx.place(tree.node_ref(tree.root()), 0, panel);
    let rows: Vec<_> = (0..144)
        .map(|index| {
            let row = tx.create(NodeSpec::new(NodeContent::Text(format!("Row {index}").into())));
            tx.change(row, style(&[StyleProp::Height(22.into()), StyleProp::Shrink(0.0), StyleProp::OverflowY(Overflow::Hidden)]));
            tx.place(panel, index, row);
            row
        })
        .collect();
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let panel = receipt.node(panel).unwrap();
    let row = receipt.node(rows[3]).unwrap();
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let frames = NativePublication::for_window_host();
    let mut engine = NativeEngine::with_publication(NativeEngineConfig::default(), frames.clone());
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();

    let scroll = |tree: &mut Tree, engine: &mut NativeEngine, offset, time| {
        let delta = tree.apply_runtime(RuntimeOp::SetScroll { node: panel, offset: Vec2::new(0.0, offset), clamped: false });
        engine.project(tree, &delta, viewport(), time).unwrap();
        engine.commit();
    };
    scroll(&mut tree, &mut engine, 5.3, 0.01);
    assert_eq!(frames.accessibility_work(), Default::default());
    assert!(frames.snapshot_full().is_none());
    assert!(frames.latest().unwrap().accessibility.is_none());

    engine.set_accessibility_active(&tree, AccessibilityConsumer::Platform, true);
    let initial_work = frames.accessibility_work();
    let snapshot = frames.snapshot_full().unwrap();
    assert!(snapshot.nodes.len() > 288);
    assert!(initial_work.nodes_built > 144);
    let initial_number = frames.latest().unwrap().number;
    let lookup = |id| snapshot.nodes.iter().find(|(key, _)| *key == id).unwrap().1.clone();
    let panel_node = lookup(node_identity(panel));
    assert_eq!(panel_node.scroll_y(), Some(f64::from(5.3_f32)));
    let row_node = lookup(node_identity(row));
    let run_node = lookup(row_node.children()[0]);
    let root_transform = *lookup(node_identity(tree.root())).transform().unwrap();
    let initial_panel_rect = (root_transform * *panel_node.transform().unwrap()).transform_rect_bbox(panel_node.bounds().unwrap());
    let row_transform = *row_node.transform().unwrap();
    let run_transform = *run_node.transform().unwrap();

    for (index, offset) in [12.37, 37.3, 9.85].into_iter().enumerate() {
        let work = frames.accessibility_work();
        scroll(&mut tree, &mut engine, offset, 0.02 + index as f64 * 0.01);
        let updated_work = frames.accessibility_work();
        assert_eq!(updated_work.nodes_built, initial_work.nodes_built);
        assert_eq!(updated_work.geometry_updates - work.geometry_updates, 1);
        assert_eq!(updated_work.candidates - work.candidates, 1);
        let frame = frames.latest().unwrap();
        assert_eq!(frame.number, initial_number + index as u64 + 1);
        let update = frame.accessibility.as_ref().unwrap();
        let [(key, new_panel)] = update.nodes.as_slice() else { panic!("expected only the scroller") };
        assert_eq!(*key, node_identity(panel));
        assert_eq!(new_panel.scroll_y(), Some(f64::from(offset)));
        let panel_transform = root_transform * *new_panel.transform().unwrap();
        assert_eq!(panel_transform.transform_rect_bbox(new_panel.bounds().unwrap()), initial_panel_rect);
        let row_bounds = (panel_transform * row_transform).transform_rect_bbox(row_node.bounds().unwrap());
        let layout_bounds = engine.interaction_bounds(row).unwrap();
        assert!((row_bounds.y0 - f64::from(layout_bounds.y * viewport().scale)).abs() < 0.001);
        let run_bounds = (panel_transform * row_transform * run_transform).transform_rect_bbox(run_node.bounds().unwrap());
        let text = engine.text_geometry(&tree, row).unwrap();
        let line = text.lines[0].rect;
        assert!((run_bounds.y0 - f64::from((text.origin.y + line.y) * viewport().scale)).abs() < 0.001);
        assert!((run_bounds.height() - f64::from(line.h * viewport().scale)).abs() < 0.001);
    }

    let pending = frames.take_accessibility_updates();
    assert_eq!(pending.len(), 3);
    for (index, (version, update)) in pending.into_iter().enumerate() {
        assert_eq!(version, initial_number + index as u64 + 1);
        assert_eq!(update.nodes[0].0, node_identity(panel));
    }
    engine.set_accessibility_active(&tree, AccessibilityConsumer::Platform, false);
    let work = frames.accessibility_work();
    scroll(&mut tree, &mut engine, 21.25, 0.1);
    assert_eq!(frames.accessibility_work(), work);
    assert!(frames.snapshot_full().is_none());
}

#[test]
fn paint_tracks_sample_at_presentation_time_and_exit_finishes_at_rest() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let handle = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(handle, tinted(1.0));
    tx.place(tree.node_ref(tree.root()), 0, handle);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    let node = receipt.node(handle).unwrap();
    initial.merge(delta);

    let mut engine = engine();
    let frames = engine.publication();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();

    let mut opacity = tree.transaction();
    opacity.change(tree.node_ref(node), tinted(0.4));
    let (_, delta) = tree.apply(opacity.finish()).unwrap();
    let feedback = engine.project(&tree, &delta, viewport(), 0.5).unwrap();
    assert!(feedback.next_deadline.is_some());
    engine.commit();
    let frame = frames.latest().unwrap();
    let scene = frame.scene.as_ref().expect("opacity is animated");
    let layer = scene
        .sample(0.6)
        .items()
        .into_iter()
        .find_map(|item| match item {
            Item::PushLayer { source, opacity } if source == node => Some(opacity),
            _ => None,
        })
        .unwrap();
    assert!(layer > 0.4 && layer < 1.0);

    let mut retire = tree.transaction();
    retire.retire(tree.node_ref(node), Retirement::VisibleExit);
    let (_, delta) = tree.apply(retire.finish()).unwrap();
    let ticket = tree.pending_exit(node).unwrap();
    assert!(matches!(delta.retired.as_slice(), [RetiredNode::Exiting { .. }]));
    let animating = engine.project(&tree, &delta, viewport(), 0.9).unwrap();
    assert!(animating.replies.is_empty());
    assert!(engine.commit().draw.unwrap().quads().any(|quad| quad.source == node));
    let complete = engine.project(&tree, &TreeDelta::default(), viewport(), 1.2).unwrap();
    assert_eq!(complete.replies, [HostReply::ExitFinished(ticket)]);
    assert!(tree.apply_runtime(RuntimeOp::FinishExit(ticket)).retired.contains(&RetiredNode::Destroyed(node)));
}

#[test]
fn reduced_motion_finishes_in_flight_paint_without_a_tree_change() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let handle = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(handle, tinted(1.0));
    tx.place(tree.node_ref(tree.root()), 0, handle);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    let node = receipt.node(handle).unwrap();
    initial.merge(delta);
    let mut engine = engine();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(node), tinted(0.4));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    assert!(engine.project(&tree, &delta, viewport(), 0.5).unwrap().next_deadline.is_some());
    engine.commit();

    engine.set_reduced_motion(true);
    assert!(engine.project(&tree, &TreeDelta::default(), viewport(), 0.51).unwrap().next_deadline.is_none());
    let draw = engine.commit().draw.unwrap();
    assert!(draw.items().into_iter().any(|item| matches!(item, Item::PushLayer { source, opacity } if source == node && opacity == 0.4)));
}

#[test]
fn text_reveal_samples_the_same_glyphs_without_reflow() {
    use octara_model::motion::{TextReveal, TextRevealKind};

    let mut tree = tree();
    let mut tx = tree.transaction();
    let text = tx.create(NodeSpec::new(NodeContent::Text("AéZ".into())));
    let reveal = TextReveal::new(TextRevealKind::Fade { duration_secs: 1.0 }, 3, false);
    tx.change(text, NodeChange::TextReveal(Some(reveal)));
    tx.place(tree.node_ref(tree.root()), 0, text);
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    let node = receipt.node(text).unwrap();
    let mut initial = tree.initial_delta();
    initial.merge(created);
    let mut engine = engine();
    let alpha = |draw: &octara_model::paint::DisplayList| {
        draw.glyph_runs()
            .flat_map(|run| run.placed_colored().map(|(glyph, color)| (glyph.source_end, color.components[3])).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };

    assert!(engine.project(&tree, &initial, viewport(), 0.0).unwrap().next_deadline.is_some());
    let first = engine.commit().draw.unwrap();
    assert_eq!(alpha(&first), [(1, 0.0), (3, 0.0), (4, 0.0)]);
    let scene = engine.publication().latest().unwrap().scene.clone().unwrap();
    let layout = engine.layout_work().computes;
    let shaped = engine.text_work();
    let drawings = engine.drawings_compiled();
    let sample = scene.sample(0.5);
    assert_eq!(alpha(&sample), [(1, 0.5), (3, 0.5), (4, 0.0)]);
    assert_eq!(scene.sample(0.5).items(), sample.items());

    engine.project(&tree, &TreeDelta::default(), viewport(), 0.5).unwrap();
    assert_eq!(engine.commit().draw.unwrap().items(), sample.items());
    assert_eq!(engine.layout_work().computes, layout);
    assert_eq!(engine.text_work(), shaped);
    assert_eq!(engine.drawings_compiled(), drawings);
    assert_eq!(alpha(&scene.sample(1.0)), [(1, 1.0), (3, 1.0), (4, 0.0)]);

    engine.set_reduced_motion(true);
    assert!(engine.project(&tree, &TreeDelta::default(), viewport(), 0.5).unwrap().next_deadline.is_none());
    assert_eq!(alpha(&engine.commit().draw.unwrap()), [(1, 1.0), (3, 1.0), (4, 1.0)]);

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(node), NodeChange::TextReveal(Some(TextReveal { visible_bytes: 1, allow_reduced_motion: true, ..reveal })));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    assert!(engine.project(&tree, &delta, viewport(), 0.5).unwrap().next_deadline.is_some());
    assert_eq!(alpha(&engine.commit().draw.unwrap()), [(1, 1.0), (3, 0.0), (4, 0.0)]);
}

#[test]
fn overlays_follow_editor_caret_and_selection_in_the_same_projection() {
    use octara_model::geometry::Placement;
    use octara_model::input::EditorEvent;
    use octara_model::overlay::AnchorTarget;
    use octara_tree::editor::{EditorAction, EditorSession, EditorSlot};
    use octara_tree::transaction::OverlaySpec;

    let mut tree = tree();
    let mut tx = tree.transaction();
    let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("Aé word")))));
    tx.change(editor, NodeChange::Flags(NodeFlags::AUTOFOCUS | NodeFlags::FOCUSABLE));
    tx.change(
        editor,
        NodeChange::Style(Box::new(AuthoredStyle {
            base: [StyleProp::Width(160.into()), StyleProp::Height(32.into())].into_iter().collect(),
            ..Default::default()
        })),
    );
    tx.place(tree.node_ref(tree.root()), 0, editor);

    let placement = Placement { fit: false, margin: 0.0, ..Placement::BELOW };
    let caret_popup = tx.create(NodeSpec::new(NodeContent::Box));
    let selection_popup = tx.create(NodeSpec::new(NodeContent::Box));
    for (index, (popup, target)) in
        [(caret_popup, AnchorTarget::EditorCaret(editor.into())), (selection_popup, AnchorTarget::TextSelection(editor.into()))]
            .into_iter()
            .enumerate()
    {
        tx.change(
            popup,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [StyleProp::Width(48.into()), StyleProp::Height(20.into())].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.change(popup, NodeChange::Overlay(OverlaySpec { target: Some((target, placement)), ..Default::default() }));
        tx.place(tree.node_ref(tree.overlay_root()), index, popup);
    }
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    let [editor, caret_popup, selection_popup] = [editor, caret_popup, selection_popup].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(created);
    let mut engine = engine();
    let feedback = engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();

    let caret = engine.caret_rect(&tree, editor).unwrap();
    let expected = placement.place(caret, Size::new(48.0, 20.0), viewport().safe_bounds());
    assert!(feedback.updates.iter().any(|op| matches!(op,
        RuntimeOp::SetPlacement { node, placed: Some(placed) } if *node == caret_popup && *placed == expected
    )));
    assert!(engine.interaction_bounds(selection_popup).is_none());

    let mut delta = TreeDelta::default();
    for update in feedback.updates {
        delta.merge(tree.apply_runtime(update));
    }
    let session = EditorSession::active(&tree).unwrap();
    let event = EditorEvent::Select { session: session.id(), expected_rev: session.editor().document_rev(), selection: (0, 3) };
    let EditorAction::Apply(update) = session.route(event).unwrap() else { panic!("expected selection update") };
    delta.merge(tree.apply_runtime(RuntimeOp::EditorApply { node: editor, update }));
    engine.project(&tree, &delta, viewport(), 0.1).unwrap();
    engine.commit();
    let selection = engine.selection_rect(&tree, editor).unwrap();
    let moved = engine.caret_rect(&tree, editor).unwrap();
    assert!(moved.x > caret.x);
    assert!(selection.w > 0.0);
    let expected = placement.place(selection, Size::new(48.0, 20.0), viewport().safe_bounds());
    assert_eq!(engine.interaction_bounds(selection_popup), Some(Rect::new(expected.x, expected.y, 48.0, 20.0)));
}

#[test]
fn theme_token_changes_only_recompile_dependent_drawings() {
    use octara_model::theme::{ColorToken, Theme, rgb};

    let mut tree = tree();
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.root());
    let mut accent = None;
    for index in 0..16 {
        let node = tx.create(NodeSpec::new(NodeContent::Box));
        let fill = if index == 0 { ColorToken::Accent.into() } else { Color::BLACK.into() };
        let base = [StyleProp::Height(8.into()), StyleProp::Background(Some(fill))].into_iter().collect();
        tx.change(node, NodeChange::Style(Box::new(AuthoredStyle { base, ..Default::default() })));
        tx.place(root, index, node);
        if index == 0 {
            accent = Some(node);
        }
    }
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let accent = receipt.node(accent.unwrap()).unwrap();
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = engine();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();
    let before = engine.paint_metrics();

    let new_color = rgb(0x225566);
    let delta = tree.apply_runtime(RuntimeOp::ThemeChanged(Box::new(Theme::default().color(ColorToken::Accent, new_color))));
    engine.project(&tree, &delta, viewport(), 1.0).unwrap();
    let draw = engine.commit().draw.unwrap();
    assert!(draw.quads().any(|quad| quad.source == accent && quad.fill == Some(new_color)));
    let after = engine.paint_metrics();
    // The window root also refreshes its theme-facing projection.
    assert_eq!(after.draw_keys_evaluated - before.draw_keys_evaluated, 2);
}
