use crate::{NativeEngine, NativeEngineConfig};
use octara_model::geometry::{Point, Rect, Size, Vec2, WindowViewport};
use octara_model::identity::TreeId;
use octara_model::motion::{Curve, Motion};
use octara_model::paint::{DisplayList, Item, Part};
use octara_model::style::{Contain, FlexDirection, Length::Px, Overflow, Position, StyleProp};
use octara_model::theme::{Fill, rgb};
use octara_runtime::HostEngine;
use octara_tree::tables::AuthoredStyle;
use octara_tree::{NodeChange, NodeContent, NodeId, NodeSpec, Retirement, RuntimeOp, Tree};
use std::sync::Arc;

fn style<const N: usize>(base: [StyleProp; N]) -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle { base: base.into_iter().collect(), ..Default::default() }))
}

#[test]
fn paint_containment_clips_to_the_layout_client_and_survives_retry() {
    let mut tree = Tree::new(TreeId::new([79, 12], 1), Default::default());
    let container_style = |contain| {
        style([
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(Px(11.5)),
            StyleProp::Top(Px(7.25)),
            StyleProp::Width(Px(40.0)),
            StyleProp::Height(Px(30.0)),
            StyleProp::BorderLeft(Px(2.0)),
            StyleProp::BorderTop(Px(3.0)),
            StyleProp::Contain(contain),
        ])
    };
    let mut tx = tree.transaction();
    let container = tx.create(NodeSpec::new(NodeContent::Box));
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(container, container_style(Contain::empty()));
    tx.change(
        child,
        style([
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(Px(45.0)),
            StyleProp::Top(Px(4.0)),
            StyleProp::Width(Px(10.0)),
            StyleProp::Height(Px(10.0)),
            StyleProp::Background(Some(Fill::Solid(rgb(0x4090b0).into()))),
        ]),
    );
    tx.place(tree.node_ref(tree.root()), 0, container);
    tx.place(container, 0, child);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [container, child] = [container, child].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let viewport = WindowViewport { size: Size::new(100.0, 100.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    let publication = engine.publication();
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    let first = engine.commit().draw.unwrap();
    assert!(first.quads().any(|quad| quad.source == child));
    let published = publication.latest().unwrap();

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(container), container_style(Contain::PAINT));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    engine.rollback();
    assert!(Arc::ptr_eq(&publication.latest().unwrap(), &published));
    engine.project(&tree, &delta, viewport, 0.2).unwrap();
    let draw = engine.commit().draw.unwrap();
    assert!(!draw.quads().any(|quad| quad.source == child));
    assert!(draw.items().iter().any(|item| matches!(item, Item::PushClip { source, rect, .. }
        if *source == container && *rect == Rect::new(13.5, 10.25, 38.0, 27.0))));
}

#[test]
fn an_absolute_child_escapes_the_intervening_scrollport_in_paint_and_hit_order() {
    let mut tree = Tree::new(TreeId::new([79, 12], 2), Default::default());
    let mut tx = tree.transaction();
    let outer = tx.create(NodeSpec::new(NodeContent::Box));
    let spacer = tx.create(NodeSpec::new(NodeContent::Box));
    let inner = tx.create(NodeSpec::new(NodeContent::Box));
    let before = tx.create(NodeSpec::new(NodeContent::Box));
    let escaped = tx.create(NodeSpec::new(NodeContent::Box));
    let after = tx.create(NodeSpec::new(NodeContent::Box));
    let filler = tx.create(NodeSpec::new(NodeContent::Box));
    let fill = |color| StyleProp::Background(Some(Fill::Solid(rgb(color).into())));
    tx.change(
        outer,
        style([
            StyleProp::Position(Position::Relative),
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::Width(Px(120.0)),
            StyleProp::Height(Px(110.0)),
            StyleProp::OverflowY(Overflow::Scroll),
        ]),
    );
    tx.change(spacer, style([StyleProp::Height(Px(30.0)), StyleProp::Shrink(0.0)]));
    tx.change(
        inner,
        style([
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::Width(Px(100.0)),
            StyleProp::Height(Px(40.0)),
            StyleProp::OverflowX(Overflow::Clip),
            StyleProp::OverflowY(Overflow::Scroll),
            StyleProp::Shrink(0.0),
            StyleProp::Opacity(0.8),
        ]),
    );
    tx.change(before, style([StyleProp::Height(Px(40.0)), StyleProp::Shrink(0.0), fill(0xd04020)]));
    tx.change(
        escaped,
        style([
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(Px(70.0)),
            StyleProp::Top(Px(60.0)),
            StyleProp::Width(Px(10.0)),
            StyleProp::Height(Px(60.0)),
            StyleProp::Motion(Motion { opacity: Curve::Fade(0.4), ..Motion::NONE }),
            fill(0x4090b0),
        ]),
    );
    tx.change(after, style([StyleProp::Height(Px(30.0)), StyleProp::Shrink(0.0), fill(0x40b040)]));
    tx.change(filler, style([StyleProp::Height(Px(120.0)), StyleProp::Shrink(0.0), StyleProp::PointerEvents(false)]));
    tx.place(tree.node_ref(tree.root()), 0, outer);
    tx.place(outer, 0, spacer);
    tx.place(outer, 1, inner);
    tx.place(outer, 2, filler);
    tx.place(inner, 0, before);
    tx.place(inner, 1, escaped);
    tx.place(inner, 2, after);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [outer, inner, before, escaped, after] = [outer, inner, before, escaped, after].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let viewport = WindowViewport { size: Size::new(160.0, 140.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    let first = engine.commit().draw.unwrap();
    let assert_at = |engine: &NativeEngine, tree: &Tree, draw: &DisplayList, x, y, node| {
        let point = Point::new(x, y);
        assert_eq!(painted_at(draw, point), Some(node), "paint at {point:?}");
        assert_eq!(engine.hit_test(tree, point).and_then(|target| target.hit.node()), Some(node), "hit at {point:?}");
    };
    assert_at(&engine, &tree, &first, 75.0, 65.0, escaped);
    assert_at(&engine, &tree, &first, 75.0, 95.0, escaped);
    let outside_outer_clip = Point::new(75.0, 115.0);
    assert_eq!(painted_at(&first, outside_outer_clip), None);
    assert_ne!(engine.hit_test(&tree, outside_outer_clip).and_then(|target| target.hit.node()), Some(escaped));

    let mut delta = tree.apply_runtime(RuntimeOp::SetScroll { node: inner, offset: Vec2::new(0.0, 20.0), clamped: false });
    delta.merge(tree.apply_runtime(RuntimeOp::SetScroll { node: outer, offset: Vec2::new(0.0, 10.0), clamped: false }));
    let metrics = engine.paint_metrics();
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    let scrolled = engine.commit().draw.unwrap();
    assert_eq!(engine.paint_metrics().subtrees_rebuilt, metrics.subtrees_rebuilt);
    let [Part::ScrollFrame { parts: original, .. }] = first.parts.as_slice() else { panic!("missing scroll frame") };
    let [Part::ScrollFrame { parts: current, .. }] = scrolled.parts.as_slice() else { panic!("missing scroll frame") };
    assert!(Arc::ptr_eq(original, current));
    assert_at(&engine, &tree, &scrolled, 75.0, 35.0, before);
    assert_at(&engine, &tree, &scrolled, 75.0, 55.0, after);
    assert_at(&engine, &tree, &scrolled, 75.0, 95.0, escaped);
    assert_eq!(scrolled.quads().find(|quad| quad.source == escaped).unwrap().rect, Rect::new(70.0, 50.0, 10.0, 60.0));
    let items = scrolled.items();
    let layer = items.iter().position(|item| matches!(item, Item::PushLayer { source, opacity } if *source == inner && *opacity == 0.8));
    let quad = items.iter().position(|item| matches!(item, Item::Quad(quad) if quad.source == escaped));
    let layer_end = items.iter().position(|item| matches!(item, Item::PopLayer));
    assert!(layer.unwrap() < quad.unwrap() && quad.unwrap() < layer_end.unwrap());

    let mut tx = tree.transaction();
    tx.retire(tree.node_ref(escaped), Retirement::VisibleExit);
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    engine.project(&tree, &delta, viewport, 0.2).unwrap();
    let exiting = engine.commit().draw.unwrap();
    let point = Point::new(75.0, 95.0);
    assert_eq!(painted_at(&exiting, point), Some(escaped));
    assert_ne!(engine.hit_test(&tree, point).and_then(|target| target.hit.node()), Some(escaped));
}

fn painted_at(draw: &DisplayList, point: Point) -> Option<NodeId> {
    let mut clips = Vec::new();
    let mut result = None;
    for item in draw.items() {
        match item {
            Item::PushClip { rect, radius, .. } => clips.push(rect.contains_rounded(point, radius)),
            Item::PushAxisClip { rect, horizontal, .. } => clips.push(if horizontal {
                rect.x <= point.x && point.x < rect.right()
            } else {
                rect.y <= point.y && point.y < rect.bottom()
            }),
            Item::PopClip => {
                clips.pop().unwrap();
            }
            Item::Quad(quad) if quad.rect.contains(point) && clips.iter().all(|&visible| visible) => result = Some(quad.source),
            _ => {}
        }
    }
    assert!(clips.is_empty());
    result
}
