use octara_model::geometry::{Rect, Size, Vec2, WindowViewport};
use octara_model::identity::{NativeId, TreeId};
use octara_model::image::{ExternalTextureHandle, ExternalTextureKind, ImageSource, ImageState, MediaAsset};
use octara_model::input::HostReply;
use octara_model::motion::{Curve, Motion};
use octara_model::native::{NativeChange, NativeSlot};
use octara_model::paint::{Image, Item, ObjectFit, Part};
use octara_model::style::{FlexDirection, Length, Overflow, Position, ScrollbarVisibility, StyleProp, Transform3D};
use octara_model::theme::{ColorValue, Fill, rgb};
use octara_pipeline::{NativeEngine, NativeEngineConfig};
use octara_runtime::HostEngine;
use octara_tree::tables::{AuthoredStyle, EditorSlot};
use octara_tree::{NodeChange, NodeContent, NodeSpec, Retirement, RuntimeOp, Tree, TreeDelta};
use std::sync::Arc;

fn tree() -> Tree {
    Tree::new(TreeId::new([17, 37], 1), Default::default())
}

fn viewport() -> WindowViewport {
    WindowViewport { size: Size::new(320.0, 800.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 }
}

fn style(props: impl IntoIterator<Item = StyleProp>) -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle { base: props.into_iter().collect(), ..Default::default() }))
}

fn fill() -> StyleProp {
    StyleProp::Background(Some(Fill::Solid(ColorValue::Color(rgb(0x4090b0)))))
}

#[test]
fn a_large_scene_reuses_resting_siblings_during_edit_scroll_and_blink() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.root());
    for row_index in 0..576 {
        let row = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(row, style([StyleProp::Height(Length::Px(1.0)), StyleProp::Shrink(0.0), fill()]));
        tx.place(root, row_index, row);
    }
    let scroll = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        scroll,
        style([
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::Height(Length::Px(40.0)),
            StyleProp::Shrink(0.0),
            StyleProp::OverflowY(Overflow::Scroll),
        ]),
    );
    tx.place(root, 576, scroll);
    let mut rows = Vec::new();
    for index in 0..224 {
        let row = tx.create(NodeSpec::new(NodeContent::Text(format!("Row {index}").into())));
        tx.change(row, style([StyleProp::Height(Length::Px(20.0)), StyleProp::Shrink(0.0), fill()]));
        tx.place(scroll, index, row);
        rows.push(row);
    }
    let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("first")))));
    tx.change(editor, style([StyleProp::Width(Length::Px(180.0)), StyleProp::Height(Length::Px(24.0)), StyleProp::Shrink(0.0)]));
    tx.place(root, 577, editor);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    initial.merge(delta);
    let (scroll, before_row, after_row, editor) =
        (receipt.node(scroll).unwrap(), receipt.node(rows[0]).unwrap(), receipt.node(rows[2]).unwrap(), receipt.node(editor).unwrap());
    initial.merge(tree.apply_runtime(RuntimeOp::SetFocus { node: Some(editor), visible: true }));

    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    let publication = engine.publication();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    let first = engine.commit().draw.unwrap();
    let mounted = engine.paint_metrics();
    assert!(mounted.subtrees_rebuilt > 580);
    assert!(first.quads().any(|quad| quad.source == before_row));
    assert!(!first.quads().any(|quad| quad.source == after_row));

    let mut edit = tree.transaction();
    edit.change(tree.node_ref(editor), NodeChange::EditorValue("another value".into()));
    let (_, delta) = tree.apply(edit.finish()).unwrap();
    engine.project(&tree, &delta, viewport(), 0.1).unwrap();
    let edited_draw = engine.commit().draw.unwrap();
    let edited = engine.paint_metrics();
    assert_eq!(edited.drawings_compiled - mounted.drawings_compiled, 1);
    assert_eq!(edited.draw_keys_evaluated - mounted.draw_keys_evaluated, 1);
    assert_eq!(edited.subtrees_rebuilt - mounted.subtrees_rebuilt, 2);
    assert!(edited.child_spans_rebuilt - mounted.child_spans_rebuilt <= 12);
    assert_ne!(first.items(), edited_draw.items());

    let layout_before = engine.layout_work();
    let text_before = engine.text_work();
    let delta = tree.apply_runtime(RuntimeOp::SetScroll { node: scroll, offset: Vec2::new(0.0, 40.0), clamped: false });
    engine.project(&tree, &delta, viewport(), 0.2).unwrap();
    let scrolled_draw = engine.commit().draw.unwrap();
    let scrolled = engine.paint_metrics();
    assert_eq!(scrolled.drawings_compiled, edited.drawings_compiled);
    assert_eq!(scrolled.draw_keys_evaluated, edited.draw_keys_evaluated);
    assert_eq!(scrolled.subtrees_rebuilt, edited.subtrees_rebuilt);
    assert_eq!(scrolled.child_spans_rebuilt, edited.child_spans_rebuilt);
    assert_eq!(scrolled.scroll_wrappers_updated, edited.scroll_wrappers_updated + 1);
    assert_eq!(engine.layout_work().frames - layout_before.frames, 1);
    assert_eq!(engine.layout_work().computes, layout_before.computes);
    assert_eq!(engine.text_work(), text_before);
    assert!(!scrolled_draw.quads().any(|quad| quad.source == before_row));
    assert!(scrolled_draw.quads().any(|quad| quad.source == after_row));
    assert!(scrolled_draw.items().iter().any(|item| matches!(item, Item::PushClip { source, .. } if *source == scroll)));

    let scene = publication.latest().unwrap().scene.clone().unwrap();
    let visible = scene.sample(0.2);
    engine.project(&tree, &TreeDelta::default(), viewport(), 0.61).unwrap();
    let hidden = engine.commit().draw.unwrap();
    let blink = engine.paint_metrics();
    assert_eq!(blink.drawings_compiled, scrolled.drawings_compiled);
    assert_eq!(blink.draw_keys_evaluated, scrolled.draw_keys_evaluated);
    assert_eq!(blink.subtrees_rebuilt, scrolled.subtrees_rebuilt);
    assert_eq!(blink.child_spans_rebuilt, scrolled.child_spans_rebuilt);
    assert_eq!(blink.timed_nodes_sampled - scrolled.timed_nodes_sampled, 2);
    assert!(blink.timed_child_spans_sampled - scrolled.timed_child_spans_sampled <= 23);
    assert!(hidden.parts.len() <= 13);
    assert_eq!(visible.quads().filter(|quad| quad.source == editor).count(), 1);
    assert_eq!(hidden.quads().filter(|quad| quad.source == editor).count(), 0);
    assert_eq!(scene.sample(0.61).items(), hidden.items());

    let published = publication.latest().unwrap();
    let row = tree.children(tree.root())[0];
    let mut edit = tree.transaction();
    edit.change(
        tree.node_ref(row),
        style([
            StyleProp::Height(Length::Px(1.0)),
            StyleProp::Shrink(0.0),
            StyleProp::Background(Some(Fill::Solid(ColorValue::Color(rgb(0xb09040))))),
        ]),
    );
    let (_, delta) = tree.apply(edit.finish()).unwrap();
    engine.project(&tree, &delta, viewport(), 0.7).unwrap();
    let staged = engine.paint_metrics();
    engine.rollback();
    assert!(Arc::ptr_eq(&publication.latest().unwrap(), &published));
    engine.project(&tree, &delta, viewport(), 0.8).unwrap();
    let retry = engine.commit().draw.unwrap();
    assert_ne!(retry.items(), hidden.items());
    assert_eq!(engine.paint_metrics().drawings_compiled, staged.drawings_compiled);
}

#[test]
fn nested_scroll_keeps_rounded_clips_stationary_and_old_frames_immutable() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let outer = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        outer,
        style([
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::Width(Length::Px(100.0)),
            StyleProp::Height(Length::Px(80.0)),
            StyleProp::OverflowY(Overflow::Scroll),
            StyleProp::Scrollbar(ScrollbarVisibility::Hidden),
        ]),
    );
    tx.place(tree.node_ref(tree.root()), 0, outer);
    let spacer = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(spacer, style([StyleProp::Height(Length::Px(32.0)), StyleProp::Shrink(0.0)]));
    tx.place(outer, 0, spacer);
    let inner = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        inner,
        style([
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::Width(Length::Px(80.0)),
            StyleProp::Height(Length::Px(40.0)),
            StyleProp::Shrink(0.0),
            StyleProp::OverflowX(Overflow::Hidden),
            StyleProp::OverflowY(Overflow::Scroll),
            StyleProp::RadiusTopLeft(octara_model::style::Radius::Px(8.0)),
            StyleProp::Scrollbar(ScrollbarVisibility::Hidden),
        ]),
    );
    tx.place(outer, 1, inner);
    let tail = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(tail, style([StyleProp::Height(Length::Px(80.0)), StyleProp::Shrink(0.0)]));
    tx.place(outer, 2, tail);
    let mut rows = Vec::new();
    for index in 0..10 {
        let row = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(row, style([StyleProp::Height(Length::Px(20.0)), StyleProp::Shrink(0.0), fill()]));
        tx.place(inner, index, row);
        rows.push(row);
    }
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    initial.merge(delta);
    let (outer, inner, row) = (receipt.node(outer).unwrap(), receipt.node(inner).unwrap(), receipt.node(rows[1]).unwrap());
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    let viewport = WindowViewport { scale: 2.0, ..viewport() };
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    let first = engine.commit().draw.unwrap();
    let before = engine.paint_metrics();
    let mut delta = tree.apply_runtime(RuntimeOp::SetScroll { node: outer, offset: Vec2::new(0.0, 10.22), clamped: false });
    delta.merge(tree.apply_runtime(RuntimeOp::SetScroll { node: inner, offset: Vec2::new(0.0, 9.22), clamped: false }));
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    engine.rollback();
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    let second = engine.commit().draw.unwrap();
    let after = engine.paint_metrics();
    assert_eq!(after.child_spans_rebuilt, before.child_spans_rebuilt);
    assert_eq!(after.subtrees_rebuilt, before.subtrees_rebuilt);
    let [Part::ScrollFrame { parts: first_parts, .. }] = first.parts.as_slice() else { panic!("missing scroll frame") };
    let [Part::ScrollFrame { parts: second_parts, .. }] = second.parts.as_slice() else { panic!("missing scroll frame") };
    assert!(Arc::ptr_eq(first_parts, second_parts));

    let quad_y = |draw: &octara_model::paint::DisplayList| {
        draw.placed_items_at(2.0)
            .find_map(|placed| match placed.resolve().as_ref() {
                Item::Quad(quad) if quad.source == row => Some(quad.rect.y),
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(quad_y(&first), 52.0);
    assert_eq!(quad_y(&second), 32.5);
    assert!(second.placed_items_at(2.0).any(|item| matches!(
        item.resolve().as_ref(),
        Item::PushClip { source, rect, radius }
            if *source == inner && *rect == Rect::new(0.0, 22.0, 80.0, 40.0) && *radius == [8.0, 0.0, 0.0, 0.0]
    )));
}

#[test]
fn transform_layers_keep_scroll_placement_clips_and_group_opacity() {
    let mut tree = tree();
    let mut tx = tree.transaction();
    let outer = tx.create(NodeSpec::new(NodeContent::Box));
    let spacer = tx.create(NodeSpec::new(NodeContent::Box));
    let inner = tx.create(NodeSpec::new(NodeContent::Box));
    let leaf = tx.create(NodeSpec::new(NodeContent::Box));
    let escaped = tx.create(NodeSpec::new(NodeContent::Box));
    let tail = tx.create(NodeSpec::new(NodeContent::Box));
    let transform = Transform3D::default().perspective(200.0).rotate_y(30.0);
    let leaf_transform = Transform3D::default().rotate_z(15.0);
    let inner_style = |transform| {
        style([
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::Width(80.into()),
            StyleProp::Height(40.into()),
            StyleProp::OverflowX(Overflow::Hidden),
            StyleProp::OverflowY(Overflow::Scroll),
            StyleProp::Scrollbar(ScrollbarVisibility::Hidden),
            StyleProp::Shrink(0.0),
            StyleProp::Opacity(0.75),
            StyleProp::Transform3D(transform),
        ])
    };
    tx.change(
        outer,
        style([
            StyleProp::Position(Position::Relative),
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::Width(100.into()),
            StyleProp::Height(80.into()),
            StyleProp::OverflowY(Overflow::Scroll),
            StyleProp::Scrollbar(ScrollbarVisibility::Hidden),
        ]),
    );
    tx.change(spacer, style([StyleProp::Height(20.into()), StyleProp::Shrink(0.0)]));
    tx.change(inner, inner_style(Some(transform)));
    tx.change(leaf, style([StyleProp::Height(80.into()), StyleProp::Shrink(0.0), StyleProp::Transform3D(Some(leaf_transform)), fill()]));
    // The outer box is the containing block for this child, so it escapes the inner scroll and clip.
    tx.change(
        escaped,
        style([
            StyleProp::Position(Position::Absolute),
            StyleProp::Top(60.into()),
            StyleProp::Width(10.into()),
            StyleProp::Height(20.into()),
            fill(),
        ]),
    );
    tx.change(tail, style([StyleProp::Height(80.into()), StyleProp::Shrink(0.0)]));
    tx.place(tree.node_ref(tree.root()), 0, outer);
    tx.place(outer, 0, spacer);
    tx.place(outer, 1, inner);
    tx.place(outer, 2, tail);
    tx.place(inner, 0, leaf);
    tx.place(inner, 1, escaped);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [outer, inner, leaf, escaped] = [outer, inner, leaf, escaped].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    let viewport = WindowViewport { scale: 2.0, ..viewport() };
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    engine.commit();
    let mut delta = tree.apply_runtime(RuntimeOp::SetScroll { node: outer, offset: Vec2::new(0.0, 10.3), clamped: false });
    delta.merge(tree.apply_runtime(RuntimeOp::SetScroll { node: inner, offset: Vec2::new(0.0, 9.2), clamped: false }));
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    let draw = engine.commit().draw.unwrap();

    let mut clips = Vec::new();
    let mut layers = Vec::new();
    let mut quads = Vec::new();
    for item in draw.placed_items_at(2.0) {
        match item.resolve().into_owned() {
            Item::PushClip { source, .. } | Item::PushAxisClip { source, .. } => clips.push(source),
            Item::PopClip => {
                clips.pop().unwrap();
            }
            Item::PushTransform { source, rect, transform: value } if source == inner => {
                assert_eq!(value, transform);
                assert_eq!(rect, Rect::new(0.0, 9.5, 80.0, 40.0));
                assert!(clips.contains(&outer));
                assert!(!clips.contains(&inner));
                layers.push(source);
            }
            Item::PushTransform { source, rect, transform: value } if source == leaf => {
                assert_eq!(value, leaf_transform);
                assert_eq!(rect.y, 0.5);
                assert!(clips.contains(&inner));
                layers.push(source);
            }
            Item::PushLayer { source, opacity } => {
                assert_eq!((source, opacity), (inner, 0.75));
                layers.push(source);
            }
            Item::PopLayer => {
                layers.pop().unwrap();
            }
            Item::Quad(quad) if quad.source == leaf => {
                assert_eq!(layers, [inner, inner, leaf]);
                assert!(clips.contains(&inner));
                quads.push(leaf);
            }
            Item::Quad(quad) if quad.source == escaped => {
                assert_eq!(layers, [inner, inner]);
                assert_eq!(quad.rect.y, 49.5);
                assert!(!clips.contains(&inner));
                quads.push(escaped);
            }
            _ => {}
        }
    }
    assert!(clips.is_empty());
    assert!(layers.is_empty());
    assert_eq!(quads, [leaf, escaped]);

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(inner), inner_style(None));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    assert!(delta.paint_dirty.contains(&inner));
    assert!(delta.layout_dirty.is_empty());
    engine.project(&tree, &delta, viewport, 0.2).unwrap();
    let updated = engine.commit().draw.unwrap();
    assert!(!updated.items().iter().any(|item| matches!(item, Item::PushTransform { source, .. } if *source == inner)));
}

#[test]
fn live_native_underlays_keep_their_window_space_cutouts() {
    for overflow in [Overflow::Visible, Overflow::Hidden] {
        let mut tree = tree();
        let mut tx = tree.transaction();
        let parent = tx.create(NodeSpec::new(NodeContent::Box));
        let underlay = tx.create(NodeSpec::new(NodeContent::Native { kind: "test.view", slot: NativeSlot::Underlay }));
        // The pipeline retains the texture handle without accessing its backend payload.
        let texture = ExternalTextureHandle::new(ExternalTextureKind::Metal, (10, 10), ());
        let media =
            tx.create(NodeSpec::new(NodeContent::ImageSource { source: ImageSource::External(texture.clone()), fit: ObjectFit::Fill }));
        tx.change(
            parent,
            style([
                StyleProp::Width(80.into()),
                StyleProp::Height(40.into()),
                StyleProp::OverflowY(overflow),
                StyleProp::Transform3D(Some(Transform3D::default())),
                fill(),
            ]),
        );
        tx.change(underlay, style([StyleProp::Width(30.into()), StyleProp::Height(20.into())]));
        // The absolute child escapes the parent's clip in the hidden-overflow case.
        tx.change(
            media,
            style([
                StyleProp::Position(Position::Absolute),
                StyleProp::Top(25.into()),
                StyleProp::Width(10.into()),
                StyleProp::Height(10.into()),
                StyleProp::Transform3D(Some(Transform3D::default().rotate_y(30.0))),
            ]),
        );
        tx.place(tree.node_ref(tree.root()), 0, parent);
        tx.place(parent, 0, underlay);
        tx.place(parent, 1, media);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let [parent, underlay, media] = [parent, underlay, media].map(|node| receipt.node(node).unwrap());
        let mut initial = tree.initial_delta();
        initial.merge(mounted);
        initial.merge(tree.apply_runtime(RuntimeOp::SetNativeLive { node: underlay, live: true }));
        initial.merge(tree.apply_runtime(RuntimeOp::SetMedia {
            node: media,
            state: ImageState::Ready { natural_size: texture.size() },
            asset: Some(MediaAsset::External(texture)),
        }));
        let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
        let publication = engine.publication();
        engine.project(&tree, &initial, viewport(), 0.0).unwrap();
        let items = engine.commit().draw.unwrap().items();
        let frame = publication.latest().unwrap();
        let placement = frame.native_changes.iter().find_map(|change| match change {
            NativeChange::Place { placements } => placements.iter().find(|place| place.id == NativeId::from(underlay)),
            _ => None,
        });
        let placement = placement.expect("native placement");
        assert!(placement.visible);
        assert!(items.iter().any(|item| matches!(item, Item::Native { source, rect, cut: true, .. }
            if *source == underlay && *rect == placement.rect)));
        let transforms = |items: &[Item]| {
            items
                .iter()
                .filter_map(|item| match item {
                    Item::PushTransform { source, .. } => Some(*source),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(transforms(&items), [media]);
        assert!(items.windows(3).any(|items| matches!(
            items, [Item::PushTransform { source, .. }, Item::ExternalTexture(paint), Item::PopLayer]
                if *source == media && paint.source == media
        )));

        let mut tx = tree.transaction();
        tx.retire(tree.node_ref(underlay), Retirement::Destroy);
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        engine.project(&tree, &delta, viewport(), 0.1).unwrap();
        assert_eq!(transforms(&engine.commit().draw.unwrap().items()), [parent, media]);
    }
}

#[test]
fn translation_only_leaf_animation_reuses_its_spatial_content() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let tile = tx.create(NodeSpec::new(NodeContent::Box));
    let placed = |y| {
        style([
            StyleProp::Position(Position::Absolute),
            StyleProp::Top(Length::Px(y)),
            StyleProp::Width(Length::Px(12.0)),
            StyleProp::Height(Length::Px(12.0)),
            StyleProp::Motion(Motion { layout: Curve::Fade(1.0), ..Motion::NONE }),
            fill(),
        ])
    };
    tx.change(tile, placed(5.0));
    tx.place(tree.node_ref(tree.root()), 0, tile);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    initial.merge(delta);
    let tile = receipt.node(tile).unwrap();
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    let publication = engine.publication();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(tile), placed(21.0));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    engine.project(&tree, &delta, viewport(), 0.25).unwrap();
    engine.commit();
    let scene = publication.latest().unwrap().scene.clone().unwrap();
    let first = scene.sample(0.25);
    let moving = scene.sample(0.75);
    let settled = scene.sample(1.5);
    let [Part::ScrollFrame { parts: first_parts, .. }] = first.parts.as_slice() else { panic!("missing spatial frame") };
    let [Part::ScrollFrame { parts: moving_parts, .. }] = moving.parts.as_slice() else { panic!("missing spatial frame") };
    let [Part::ScrollFrame { parts: settled_parts, .. }] = settled.parts.as_slice() else { panic!("missing spatial frame") };
    assert!(Arc::ptr_eq(first_parts, moving_parts));
    assert!(Arc::ptr_eq(first_parts, settled_parts));

    let position = |draw: &octara_model::paint::DisplayList| {
        draw.placed_items_at(2.0)
            .find_map(|item| match item.resolve().as_ref() {
                Item::Quad(quad) if quad.source == tile => Some((item.spatial_id, quad.rect.y)),
                _ => None,
            })
            .unwrap()
    };
    let (spatial_id, start_y) = position(&first);
    assert_ne!(spatial_id, 0);
    assert_eq!(start_y, 5.0);
    assert_eq!(position(&moving), (spatial_id, 13.0));
    assert_eq!(position(&settled), (spatial_id, 21.0));
}

#[test]
fn mutable_images_refresh_without_tree_deltas_and_frozen_draws_keep_their_pixels() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let image = Image::rgba(1, 1, [255, 0, 0, 255]);
    let mut tx = tree.transaction();
    let node = tx.create(NodeSpec::new(NodeContent::Image { image: image.clone(), fit: ObjectFit::default() }));
    tx.change(node, style([StyleProp::Width(Length::Px(16.0)), StyleProp::Height(Length::Px(16.0))]));
    tx.place(tree.node_ref(tree.root()), 0, node);
    initial.merge(tree.apply(tx.finish()).unwrap().1);
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    let first = engine.commit().draw.unwrap();
    let built = engine.drawings_compiled();
    image.set(1, 1, [0, 255, 0, 255]);
    engine.project(&tree, &TreeDelta::default(), viewport(), 0.1).unwrap();
    let second = engine.commit().draw.unwrap();
    let version = |items: Vec<Item>| {
        items.into_iter().find_map(|item| match item {
            Item::Image(image) => Some(image.image.frame().version),
            _ => None,
        })
    };
    assert_eq!(version(first.items()), Some(0));
    assert_eq!(version(second.items()), Some(1));
    assert_eq!(engine.drawings_compiled(), built + 1);
}

#[test]
fn rollback_restores_motion_targets_and_exit_completion() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let animated = |opacity| {
        style([
            StyleProp::Height(Length::Px(16.0)),
            StyleProp::Motion(Motion { opacity: Curve::Fade(0.2), ..Motion::NONE }),
            StyleProp::Opacity(opacity),
            fill(),
        ])
    };
    let mut tx = tree.transaction();
    let node = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(node, animated(1.0));
    tx.place(tree.node_ref(tree.root()), 0, node);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    initial.merge(delta);
    let node = receipt.node(node).unwrap();
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    let publication = engine.publication();
    engine.project(&tree, &initial, viewport(), 0.0).unwrap();
    engine.commit();

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(node), animated(0.2));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    engine.project(&tree, &delta, viewport(), 0.1).unwrap();
    engine.rollback();
    engine.project(&tree, &delta, viewport(), 0.4).unwrap();
    engine.commit();
    let scene = publication.latest().unwrap().scene.clone().expect("retry starts the published element's fade");
    let opacity = scene.sample(0.5).items().into_iter().find_map(|item| match item {
        Item::PushLayer { source, opacity } if source == node => Some(opacity),
        _ => None,
    });
    assert!((opacity.unwrap() - 0.6).abs() < 0.001);

    let mut tx = tree.transaction();
    tx.retire(tree.node_ref(node), Retirement::VisibleExit);
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    let ticket = tree.pending_exit(node).unwrap();
    engine.project(&tree, &delta, viewport(), 0.7).unwrap();
    engine.rollback();
    let retry = engine.project(&tree, &delta, viewport(), 1.0).unwrap();
    assert!(retry.replies.is_empty());
    assert!(engine.commit().draw.unwrap().quads().any(|quad| quad.source == node));

    let settled = publication.latest().unwrap();
    for _ in 0..2 {
        let complete = engine.project(&tree, &TreeDelta::default(), viewport(), 1.3).unwrap();
        assert_eq!(complete.replies, [HostReply::ExitFinished(ticket)]);
        engine.rollback();
        assert!(Arc::ptr_eq(&publication.latest().unwrap(), &settled));
    }
}
