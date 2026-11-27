use octara_model::geometry::{Edges, Rect, Size, WindowViewport};
use octara_model::identity::TreeId;
use octara_model::motion::Curve;
use octara_model::paint::DisplayList;
use octara_model::style::{Length::Px, Position, Radius, State, StyleProp, When};
use octara_model::theme::{ColorToken, rgb};
use octara_pipeline::{NativeEngine, NativeEngineConfig};
use octara_runtime::HostEngine;
use octara_tree::tables::AuthoredStyle;
use octara_tree::{NodeChange, NodeContent, NodeFlags, NodeSpec, RuntimeOp, Tree, TreeDelta};

const VIEWPORT: WindowViewport = WindowViewport { size: Size::new(100.0, 100.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };

#[test]
fn focus_ring_adds_an_outer_stroke_without_changing_the_border_box() {
    let mut tree = Tree::new(TreeId::new([91, 72], 1), Default::default());
    let mut tx = tree.transaction();
    let node = tx.create(NodeSpec::new(NodeContent::Box));
    let ring_color = rgb(0x336699);
    tx.change(node, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(
        node,
        NodeChange::Style(Box::new(AuthoredStyle {
            base: [
                StyleProp::Width(Px(50.0)),
                StyleProp::Height(Px(30.0)),
                StyleProp::Position(Position::Absolute),
                StyleProp::Top(Px(10.0)),
                StyleProp::Left(Px(12.0)),
                StyleProp::RadiusTopLeft(Radius::Px(6.0)),
                StyleProp::RadiusTopRight(Radius::Px(6.0)),
                StyleProp::RadiusBottomRight(Radius::Px(6.0)),
                StyleProp::RadiusBottomLeft(Radius::Px(6.0)),
                StyleProp::BorderTop(Px(1.0)),
                StyleProp::BorderRight(Px(2.0)),
                StyleProp::BorderBottom(Px(3.0)),
                StyleProp::BorderLeft(Px(4.0)),
                StyleProp::TextColor(ring_color.into()),
                StyleProp::RingOffset(Px(3.0)),
                StyleProp::ColorMotion(Curve::Fade(0.2)),
            ]
            .into_iter()
            .collect(),
            overrides: [(When::State(State::FOCUS_VISIBLE), vec![StyleProp::RingWidth(Px(2.0))])].into_iter().collect(),
            ..Default::default()
        })),
    );
    tx.place(tree.node_ref(tree.root()), 0, node);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let node = receipt.node(node).unwrap();

    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    let mut delta = tree.initial_delta();
    delta.merge(mounted);
    engine.project(&tree, &delta, VIEWPORT, 0.0).unwrap();
    let initial = engine.commit().draw.unwrap();
    let geometry = engine.geometry(node).unwrap();
    assert_eq!(geometry.border, Edges::from([1.0, 2.0, 3.0, 4.0]));
    assert_eq!(initial.quads().filter(|quad| quad.source == node).count(), 1);

    let delta = tree.apply_runtime(RuntimeOp::SetFocus { node: Some(node), visible: true });
    assert!(!delta.layout_dirty.contains(&node));
    assert!(delta.paint_dirty.contains(&node));
    let focused = engine.project(&tree, &delta, VIEWPORT, 1.0).unwrap();
    assert!(focused.next_deadline.is_some());
    engine.commit();
    assert_eq!(engine.geometry(node), Some(geometry));
    let frame = engine.publication().latest().unwrap();
    let scene = frame.scene.as_ref().unwrap();
    let ring = scene.sample(1.1).quads().find(|quad| quad.source == node && quad.border == [2.0; 4]).unwrap();
    assert_eq!(ring.rect, Rect::new(7.0, 5.0, 60.0, 40.0));
    assert_eq!(ring.radius, [11.0; 4]);
    assert_eq!(ring.fill, None);
    assert!((ring.border_color.components[3] - 0.5).abs() < 0.001);
    let focused = scene.sample(1.3);
    let mut quads = focused.quads().filter(|quad| quad.source == node);
    assert_eq!(quads.next().unwrap().border_color, ring_color);
    let border = quads.next().unwrap();
    assert_eq!(border.rect, geometry.rect);
    assert_eq!(border.border, [1.0, 2.0, 3.0, 4.0]);

    let delta = tree.apply_runtime(RuntimeOp::SetFocus { node: None, visible: false });
    engine.project(&tree, &delta, VIEWPORT, 2.0).unwrap();
    engine.commit();
    let frame = engine.publication().latest().unwrap();
    let scene = frame.scene.as_ref().unwrap();
    assert_eq!(scene.sample(2.1).quads().filter(|quad| quad.source == node).count(), 2);
    assert_eq!(scene.sample(2.3).quads().filter(|quad| quad.source == node).count(), 1);
}

#[test]
fn a_ring_reaching_inside_the_viewport_survives_paint_culling() {
    let mut tree = Tree::new(TreeId::new([91, 72], 2), Default::default());
    let mut tx = tree.transaction();
    let near = tx.create(NodeSpec::new(NodeContent::Box));
    let far = tx.create(NodeSpec::new(NodeContent::Box));
    for (index, node, left) in [(0, near, -22.0), (1, far, -40.0)] {
        tx.change(
            node,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [
                    StyleProp::Position(Position::Absolute),
                    StyleProp::Top(Px(10.0)),
                    StyleProp::Left(Px(left)),
                    StyleProp::Width(Px(20.0)),
                    StyleProp::Height(Px(20.0)),
                    StyleProp::RingWidth(Px(2.0)),
                    StyleProp::RingOffset(Px(3.0)),
                    StyleProp::RingColor(ColorToken::Focus.into()),
                ]
                .into_iter()
                .collect(),
                ..Default::default()
            })),
        );
        tx.place(tree.node_ref(tree.root()), index, node);
    }
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let near = receipt.node(near).unwrap();
    let far = receipt.node(far).unwrap();
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let mut engine = NativeEngine::new(NativeEngineConfig { check_paint_cache: true, ..Default::default() });
    engine.project(&tree, &initial, VIEWPORT, 0.0).unwrap();
    let assert_visible = |draw: &DisplayList| {
        let ring = draw.quads().find(|quad| quad.source == near).unwrap();
        assert_eq!(ring.rect.right(), 3.0);
        assert_eq!(ring.border_color, tree.theme().get(ColorToken::Focus));
        assert!(!draw.quads().any(|quad| quad.source == far));
    };
    assert_visible(&engine.commit().draw.unwrap());
    engine.project(&tree, &TreeDelta::default(), VIEWPORT, 0.1).unwrap();
    assert_visible(&engine.commit().draw.unwrap());
}
