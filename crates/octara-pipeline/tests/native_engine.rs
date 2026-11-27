use octara_model::geometry::{Point, Rect, Size, WindowViewport};
use octara_model::identity::{NativeId, TreeId};
use octara_model::input::HostReply;
use octara_model::motion::{Curve, Motion};
use octara_model::native::{NativeChange, NativeSlot};
use octara_model::paint::Item;
use octara_model::semantics::Role;
use octara_model::style::{Length, Overflow, Position, Radius, State, StyleProp, When};
use octara_model::theme::{Color, ColorValue, Fill};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication};
use octara_runtime::HostEngine;
use octara_tree::tables::{AuthoredStyle, SemanticsSlot};
use octara_tree::{NodeChange, NodeContent, NodeFlags, NodeSpec, RetiredNode, Retirement, RuntimeOp, Tree, TreeDelta};

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
    let hit = |engine: &NativeEngine, tree: &Tree, point| engine.hit_test(tree, point).and_then(|target| target.hit.node());
    assert_eq!(hit(&engine, &tree, Point::new(40.0, 40.0)), Some(back));
    assert_eq!(hit(&engine, &tree, Point::new(5.0, 5.0)), Some(child));
    assert_eq!(frames.accessibility_node(octara_pipeline::a11y::node_identity(front)), Some(front));

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
    assert_eq!(frames.accessibility_node(octara_pipeline::a11y::node_identity(front)), Some(front));
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
    let first_access = frames.latest().unwrap();
    assert!(first_access.accessibility.tree.is_some());
    assert!(first_access.accessibility.nodes.len() >= 5);

    engine.project(&tree, &TreeDelta::default(), viewport(), 0.05).unwrap();
    let after = engine.commit().draw.unwrap();
    assert_eq!(draw, after);
    assert_eq!(engine.drawings_compiled(), start);
    assert!(frames.latest().unwrap().accessibility.nodes.is_empty());

    let mut change = tree.transaction();
    change.change(tree.node_ref(second), NodeChange::Text("SECOND".into()));
    let (_, delta) = tree.apply(change.finish()).unwrap();
    engine.project(&tree, &delta, viewport(), 0.1).unwrap();
    engine.commit();
    let frame = frames.latest().unwrap();
    assert!(!frame.accessibility.nodes.is_empty());
    assert!(frame.accessibility.nodes.len() < frames.snapshot_full().unwrap().nodes.len());
    assert_eq!(frames.accessibility_node(octara_pipeline::a11y::node_identity(first)), Some(first));
    assert!(engine.text_hit(first, Point::new(1.0, 2.0)).is_some());
    assert!(engine.drawings_compiled() <= start + 2);
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
    let event = EditorEvent::ProposeSelection { session: session.id(), expected_rev: session.editor().document_rev(), selection: (0, 3) };
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
