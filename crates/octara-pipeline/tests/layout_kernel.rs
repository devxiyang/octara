use accesskit::NodeId as AccessNodeId;
use octara_model::geometry::{Placement, Point, Rect, Size, WindowViewport};
use octara_model::identity::TreeId;
use octara_model::overlay::AnchorTarget;
use octara_model::paint::Item;
use octara_model::semantics::Role;
use octara_model::style::{
    AlignItems as Alignment, Contain as Containment, Display as DisplayMode, FlexDirection, LineHeight as Leading, Overflow,
    Position as PositionMode, State, StyleProp, When,
};
use octara_model::text::Wrap;
use octara_model::theme::Color;
use octara_pipeline::{NativeEngine, NativeEngineConfig, a11y::node_identity};
use octara_runtime::HostEngine;
use octara_runtime::engine::AccessibilityConsumer;
use octara_tree::tables::{AuthoredStyle, SemanticsSlot};
use octara_tree::transaction::{OverlaySpec, PendingNode};
use octara_tree::{NodeChange, NodeContent, NodeId, NodeRef, NodeSpec, RuntimeOp, Tree, TreeDelta, TxBuilder};
use std::collections::{HashMap, HashSet};

const ROW_COUNT: usize = 160;
const ROW_HEIGHT: f32 = 44.0;
const ROW_WIDTH: f32 = 296.0;
const LINE_HEIGHT: f32 = 16.0;

fn tree() -> Tree {
    Tree::new(TreeId::new([18, 3], 1), Default::default())
}

fn viewport(height: f32) -> WindowViewport {
    WindowViewport { size: Size::new(320.0, height), scale: 1.0, safe_area: None, keyboard_inset: 0.0 }
}

fn engine(tree: &Tree) -> NativeEngine {
    let engine = NativeEngine::new(NativeEngineConfig {
        check_layout_cache: true,
        check_paint_cache: true,
        check_a11y_cache: true,
        ..Default::default()
    });
    engine.set_accessibility_active(tree, AccessibilityConsumer::Inspector, true);
    engine
}

fn style(base: impl IntoIterator<Item = StyleProp>) -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle { base: base.into_iter().collect(), ..Default::default() }))
}

fn child(
    tx: &mut TxBuilder,
    parent: impl Into<NodeRef>,
    index: usize,
    content: NodeContent,
    base: impl IntoIterator<Item = StyleProp>,
) -> PendingNode {
    let node = tx.create(NodeSpec::new(content));
    tx.change(node, style(base));
    tx.place(parent, index, node);
    node
}

fn hit(engine: &NativeEngine, tree: &Tree, point: Point) -> Option<NodeId> {
    engine.hit_test(tree, point).and_then(|target| target.hit.node())
}

struct FixedRow {
    node: NodeId,
    title: NodeId,
    action: NodeId,
}

fn fixed_sidebar(tree: &mut Tree) -> (Vec<FixedRow>, TreeDelta) {
    use StyleProp::*;

    let mut tx = tree.transaction();
    let scrollport = child(
        &mut tx,
        tree.node_ref(tree.root()),
        0,
        NodeContent::Box,
        [Width(ROW_WIDTH.into()), Height(440.into()), Direction(FlexDirection::Column), OverflowY(Overflow::Scroll)],
    );
    let mut rows = Vec::with_capacity(ROW_COUNT);
    for index in 0..ROW_COUNT {
        let row = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(
            row,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [
                    Width(ROW_WIDTH.into()),
                    Height(ROW_HEIGHT.into()),
                    Direction(FlexDirection::Row),
                    AlignItems(Alignment::Center),
                    Shrink(0.0),
                    PaddingLeft(8.into()),
                    PaddingRight(8.into()),
                    OverflowX(Overflow::Clip),
                    OverflowY(Overflow::Clip),
                ]
                .into_iter()
                .collect(),
                group: Some("sidebar-row"),
                ..Default::default()
            })),
        );
        tx.place(scrollport, index, row);
        // The avatar keeps the row's intrinsic height at 28 pt in both hover states.
        let avatar = child(&mut tx, row, 0, NodeContent::Box, [Width(24.into()), Height(28.into()), Shrink(0.0)]);
        child(&mut tx, avatar, 0, NodeContent::Text("P".into()), []);
        let title = tx.create(NodeSpec::new(NodeContent::Text("Project notes and recent activity".into())));
        tx.change(
            title,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [Width(240.into()), TextWrap(Wrap::Truncate), Height(16.into()), Shrink(0.0)].into_iter().collect(),
                overrides: [(When::GroupHover("sidebar-row"), vec![Width(212.into())])].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.place(row, 1, title);
        let badge = child(&mut tx, row, 2, NodeContent::Box, [Width(16.into()), Height(16.into()), Shrink(0.0)]);
        child(&mut tx, badge, 0, NodeContent::Box, [Width(4.into()), Height(4.into())]);
        child(&mut tx, badge, 1, NodeContent::Text("2".into()), [TextWrap(Wrap::None)]);
        let action = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(
            action,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [Display(DisplayMode::None), Width(28.into()), Height(28.into()), Shrink(0.0), AlignItems(Alignment::Center)]
                    .into_iter()
                    .collect(),
                overrides: [(When::GroupHover("sidebar-row"), vec![Display(DisplayMode::Flex)])].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.change(
            action,
            NodeChange::Semantics(Box::new(SemanticsSlot {
                role: Some(Role::Button),
                label: Some("More actions".into()),
                ..Default::default()
            })),
        );
        tx.place(row, 3, action);
        for dot in 0..3 {
            child(&mut tx, action, dot, NodeContent::Box, [Width(4.into()), Height(4.into()), Background(Some(Color::BLACK.into()))]);
        }
        rows.push((row, title, action));
    }
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let rows = rows
        .into_iter()
        .map(|(row, title, action)| FixedRow {
            node: receipt.node(row).unwrap(),
            title: receipt.node(title).unwrap(),
            action: receipt.node(action).unwrap(),
        })
        .collect();
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    (rows, initial)
}

#[test]
fn clipped_sidebar_hover_projects_only_the_changed_row() {
    let mut tree = tree();
    let (rows, initial) = fixed_sidebar(&mut tree);
    let row = &rows[1];
    let viewport = viewport(440.0);
    let mut engine = engine(&tree);
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    engine.commit();

    let row_bounds = Rect::new(0.0, ROW_HEIGHT, ROW_WIDTH, ROW_HEIGHT);
    let pointer = Point::new(267.0, 60.0);
    assert_eq!(engine.interaction_bounds(row.node), Some(row_bounds));
    assert_eq!(engine.interaction_bounds(row.title), Some(Rect::new(32.0, 58.0, 240.0, 16.0)));
    assert_eq!(engine.interaction_bounds(row.action), None);
    assert_eq!(engine.flat_tree_metrics().grid_rebuilds, 0);
    assert_eq!(hit(&engine, &tree, pointer), Some(row.title));
    let flat_before = engine.flat_tree_metrics();
    assert_eq!(flat_before.grid_rebuilds, 1, "the initial real hit warms the index");
    let layout_before = engine.layout_work();
    let text_before = engine.text_work();
    let delta = tree.apply_runtime(RuntimeOp::SetState { node: row.node, state: State::HOVER, on: true });
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    engine.commit();

    let layout = engine.layout_work();
    let text = engine.text_work();
    let deferred = engine.flat_tree_metrics();
    let target = hit(&engine, &tree, pointer);
    let materialized = engine.flat_tree_metrics();
    let frames = layout.frames - layout_before.frames;
    let boundaries_hit = layout.boundaries_hit - layout_before.boundaries_hit;
    let escalations = layout.escalations - layout_before.escalations;
    let lazy_skips = deferred.lazy_skips - flat_before.lazy_skips;
    let grid_rebuilds = materialized.grid_rebuilds - flat_before.grid_rebuilds;
    let nodes_patched = materialized.nodes_patched - flat_before.nodes_patched;
    let measure_probe_hits = text.measure_probe_hits - text_before.measure_probe_hits;
    let line_breaks = text.line_breaks - text_before.line_breaks;
    let nodes_projected = text.nodes_projected - text_before.nodes_projected;
    println!(
        "Sidebar hover: {frames} layout frames, {boundaries_hit} boundary hits, {escalations} escalations, {lazy_skips} lazy skips, \
         {grid_rebuilds} grid rebuilds, {nodes_patched} patched indices, {nodes_projected} text projections, \
         {measure_probe_hits} measure probe hits, {line_breaks} line breaks"
    );

    assert_eq!(engine.interaction_bounds(row.node), Some(row_bounds));
    assert_eq!(engine.interaction_bounds(row.title), Some(Rect::new(32.0, 58.0, 212.0, 16.0)));
    assert_eq!(engine.interaction_bounds(row.action), Some(Rect::new(260.0, 52.0, 28.0, 28.0)));
    assert_eq!(target, Some(row.action));
    assert!(frames <= 16, "hover projected {frames} frames");
    assert!(boundaries_hit >= 1);
    assert_eq!(escalations, 0);
    assert_eq!(deferred.grid_rebuilds, flat_before.grid_rebuilds);
    assert_eq!(deferred.nodes_patched, flat_before.nodes_patched);
    assert_eq!(lazy_skips, 1);
    assert_eq!(grid_rebuilds, 0);
    assert!((1..=16).contains(&nodes_patched), "hover patched {nodes_patched} indices");
    assert_eq!(nodes_projected, 1, "only the resizing title needs text projection");
    assert!(measure_probe_hits > 0);
    assert_eq!(line_breaks, 0);
}

#[test]
fn row_growth_moves_later_rows_without_reprojecting_their_descendants() {
    use StyleProp::*;

    const GROWING_ROW: usize = 100;
    let mut tree = tree();
    let root = tree.node_ref(tree.root());
    let mut tx = tree.transaction();
    let mut nodes = Vec::with_capacity(ROW_COUNT);
    for index in 0..ROW_COUNT {
        let row = child(
            &mut tx,
            root,
            index,
            NodeContent::Box,
            [Direction(FlexDirection::Row), AlignItems(Alignment::Start), Width(240.into()), Shrink(0.0)],
        );
        let content = child(&mut tx, row, 0, NodeContent::Box, [Width(96.into())]);
        let text = child(
            &mut tx,
            content,
            0,
            NodeContent::Text("Streaming".into()),
            [FontSize(16.into()), LineHeight(Leading::Len(LINE_HEIGHT.into())), TextWrap(Wrap::Words)],
        );
        child(&mut tx, row, 1, NodeContent::Text("Updated".into()), [Width(80.into()), Height(LINE_HEIGHT.into())]);
        nodes.push((row, text));
    }
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let rows: Vec<_> = nodes.iter().map(|(row, _)| receipt.node(*row).unwrap()).collect();
    let growing_text = receipt.node(nodes[GROWING_ROW].1).unwrap();
    let shifted = &rows[GROWING_ROW + 1..];
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let viewport = viewport((ROW_COUNT + 2) as f32 * LINE_HEIGHT);
    let mut engine = engine(&tree);
    let publication = engine.publication();
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    engine.commit();

    let growing_bounds = Rect::new(0.0, GROWING_ROW as f32 * LINE_HEIGHT, 240.0, LINE_HEIGHT);
    assert_eq!(engine.interaction_bounds(rows[GROWING_ROW]), Some(growing_bounds));
    assert_eq!(engine.text_geometry(&tree, growing_text).unwrap().lines.len(), 1);
    let shifted_bounds: Vec<_> = shifted.iter().map(|&row| engine.interaction_bounds(row).unwrap()).collect();
    let descendants: Vec<_> = shifted
        .iter()
        .flat_map(|&row| tree.subtree(row).skip(1))
        .map(|node| (node, engine.geometry(node).unwrap(), engine.interaction_bounds(node).unwrap()))
        .collect();

    // Include synthetic AccessKit text runs among the descendants that must stay untouched.
    let snapshot = publication.snapshot_full().expect("active initial accessibility tree");
    let access_nodes: HashMap<_, _> = snapshot.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let mut pending: Vec<_> = shifted.iter().flat_map(|&row| access_nodes[&node_identity(row)].children().iter().copied()).collect();
    let mut access_descendants = HashSet::new();
    while let Some(id) = pending.pop() {
        assert!(access_descendants.insert(id));
        pending.extend(access_nodes[&id].children());
    }
    for (node, _, _) in &descendants {
        assert!(access_descendants.contains(&node_identity(*node)), "descendant {node:?} must be exposed before growth");
    }
    assert!(access_descendants.len() > descendants.len(), "the fixture must expose synthetic text runs");

    let layout_before = engine.layout_work();
    let text_before = engine.text_work();
    let access_before = publication.accessibility_work();
    let last_row_center = (ROW_COUNT as f32 - 0.5) * LINE_HEIGHT;
    assert_eq!(hit(&engine, &tree, Point::new(200.0, last_row_center)), rows.last().copied());
    let flat_before = engine.flat_tree_metrics();
    assert_eq!(flat_before.grid_rebuilds, 1);
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(growing_text), NodeChange::Text("Streaming response".into()));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    engine.commit();

    let frame = publication.latest().unwrap();
    let access = publication.accessibility_work();
    let flat = engine.flat_tree_metrics();
    let update = frame.accessibility.as_ref().expect("the shifted row transforms must be published");
    let updated: HashSet<AccessNodeId> = update.nodes.iter().map(|(id, _)| *id).collect();
    let projected = frame.text_work.nodes_projected - text_before.nodes_projected;
    let nodes_built = access.nodes_built - access_before.nodes_built;
    let geometry_updates = access.geometry_updates - access_before.geometry_updates;
    let candidates = access.candidates - access_before.candidates;
    let descendant_updates = updated.intersection(&access_descendants).count();
    let frames = engine.layout_work().frames - layout_before.frames;
    let lazy_skips = flat.lazy_skips - flat_before.lazy_skips;
    println!(
        "Row 100 growth: {frames} layout frames, {projected} text projections, {candidates} accessibility candidates, \
         {nodes_built} accessibility builds, {geometry_updates} accessibility geometry updates, \
         {descendant_updates} published accessibility descendant updates, {lazy_skips} lazy skips"
    );

    assert_eq!(engine.interaction_bounds(rows[GROWING_ROW]), Some(Rect { h: 2.0 * LINE_HEIGHT, ..growing_bounds }));
    assert_eq!(engine.text_geometry(&tree, growing_text).unwrap().lines.len(), 2);
    for (&row, before) in shifted.iter().zip(shifted_bounds) {
        assert_eq!(engine.interaction_bounds(row), Some(Rect { y: before.y + LINE_HEIGHT, ..before }));
        assert!(updated.contains(&node_identity(row)), "shifted row {row:?} must publish its transform");
    }
    for (node, local, world) in descendants {
        assert_eq!(engine.geometry(node), Some(local), "local box for shifted descendant {node:?}");
        assert_eq!(engine.interaction_bounds(node), Some(Rect { y: world.y + LINE_HEIGHT, ..world }));
    }
    assert_eq!(projected, 1, "only the growing text node needs projection");
    assert_eq!(nodes_built, 1, "only the growing text needs a semantic rebuild");
    assert_eq!(geometry_updates, shifted.len() as u64 + 2, "the changed row and its content need patches along with the shifted rows");
    assert_eq!(descendant_updates, 0, "row transforms carry all accessibility descendants");
    assert_eq!(flat.grid_rebuilds, flat_before.grid_rebuilds);
    assert_eq!(flat.nodes_patched, flat_before.nodes_patched);
    assert_eq!(lazy_skips, 1);
}

#[test]
fn anchor_fit_projects_only_the_overlay_subtree() {
    use StyleProp::*;

    let mut tree = tree();
    let (rows, mut initial) = fixed_sidebar(&mut tree);
    let anchor = tree.node_ref(rows[0].node);
    let placement = |fit| {
        NodeChange::Overlay(OverlaySpec { target: Some((AnchorTarget::Node(anchor), Placement::BELOW.fit(fit))), ..Default::default() })
    };
    let mut tx = tree.transaction();
    let overlay =
        child(&mut tx, tree.node_ref(tree.overlay_root()), 0, NodeContent::Box, [Width(100.into()), Direction(FlexDirection::Column)]);
    tx.change(overlay, placement(false));
    let body = child(&mut tx, overlay, 0, NodeContent::Box, [Height(600.into()), Shrink(0.0), Background(Some(Color::BLACK.into()))]);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [overlay, body] = [overlay, body].map(|node| receipt.node(node).unwrap());
    initial.merge(mounted);
    let viewport = viewport(440.0);
    let mut engine = engine(&tree);
    let feedback = engine.project(&tree, &initial, viewport, 0.0).unwrap();
    let mut settled = TreeDelta::default();
    for update in feedback.updates {
        settled.merge(tree.apply_runtime(update));
    }
    engine.project(&tree, &settled, viewport, 0.0).unwrap();
    engine.commit();
    assert_eq!(engine.interaction_bounds(overlay), Some(Rect::new(8.0, 8.0, 100.0, 600.0)));
    let row_bounds: Vec<_> = rows.iter().map(|row| engine.interaction_bounds(row.node).unwrap()).collect();
    let before = engine.layout_work();
    let text_before = engine.text_work();

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(overlay), placement(true));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    let feedback = engine.project(&tree, &delta, viewport, 0.1).unwrap();
    engine.commit();

    let after = engine.layout_work();
    let overlay_nodes = tree.subtree(overlay).count() as u64;
    let frames = after.frames - before.frames;
    let computes = after.computes - before.computes;
    let anchors = after.anchors - before.anchors;
    println!("Anchor fit: {frames} layout frames for {overlay_nodes} overlay nodes, {computes} computations, {anchors} anchors");
    assert_eq!(engine.interaction_bounds(overlay), Some(Rect::new(8.0, 48.0, 100.0, 384.0)));
    assert_eq!(engine.interaction_bounds(body), Some(Rect::new(8.0, 48.0, 100.0, 600.0)));
    assert!(feedback.updates.iter().any(|update| matches!(
        update,
        RuntimeOp::SetPlacement { node, placed: Some(placed) }
            if *node == overlay && placed.limit == Some(384.0)
    )));
    assert!(frames <= overlay_nodes + 2, "fit projected {frames} frames");
    assert!(computes >= 1);
    assert_eq!(anchors, 1);
    assert_eq!(engine.text_work().nodes_projected, text_before.nodes_projected);
    for (row, before) in rows.iter().zip(row_bounds) {
        assert_eq!(engine.interaction_bounds(row.node), Some(before));
    }
}

#[test]
fn strict_containment_isolates_intrinsic_size_fixed_positioning_and_paint() {
    use StyleProp::*;

    let mut tree = tree();
    let mut tx = tree.transaction();
    let port = child(
        &mut tx,
        tree.node_ref(tree.root()),
        0,
        NodeContent::Box,
        [Width(120.into()), Height(100.into()), Direction(FlexDirection::Column), OverflowX(Overflow::Scroll), OverflowY(Overflow::Scroll)],
    );
    child(&mut tx, port, 0, NodeContent::Box, [Height(30.into()), Shrink(0.0)]);
    let boundary = child(
        &mut tx,
        port,
        1,
        NodeContent::Box,
        [Contain(Containment::STRICT), Direction(FlexDirection::Column), Width(80.into()), MinHeight(40.into()), Shrink(0.0)],
    );
    let overflow = child(&mut tx, boundary, 0, NodeContent::Box, [Height(120.into()), Shrink(0.0)]);
    let fixed = child(
        &mut tx,
        overflow,
        0,
        NodeContent::Box,
        [
            Position(PositionMode::Fixed),
            Left(60.into()),
            Top(20.into()),
            Width(40.into()),
            Height(40.into()),
            Background(Some(Color::BLACK.into())),
        ],
    );
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [port, boundary, overflow, fixed] = [port, boundary, overflow, fixed].map(|node| receipt.node(node).unwrap());
    let mut initial = tree.initial_delta();
    initial.merge(mounted);
    let viewport = viewport(160.0);
    let mut engine = engine(&tree);
    let publication = engine.publication();
    engine.project(&tree, &initial, viewport, 0.0).unwrap();
    engine.commit();

    let boundary_rect = Rect::new(0.0, 30.0, 80.0, 40.0);
    let fixed_rect = Rect::new(60.0, 50.0, 40.0, 40.0);
    assert_eq!(engine.interaction_bounds(boundary), Some(boundary_rect));
    assert_eq!(engine.interaction_bounds(overflow), Some(Rect::new(0.0, 30.0, 80.0, 120.0)));
    assert_eq!(engine.interaction_bounds(fixed), Some(fixed_rect));
    let before = engine.layout_work();
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(overflow), style([Height(152.into()), Shrink(0.0)]));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    engine.project(&tree, &delta, viewport, 0.1).unwrap();
    let draw = engine.commit().draw.unwrap();

    let after = engine.layout_work();
    let frames = after.frames - before.frames;
    let boundaries_hit = after.boundaries_hit - before.boundaries_hit;
    let escalations = after.escalations - before.escalations;
    println!("Strict containment: {frames} layout frames, {boundaries_hit} boundary hits, {escalations} escalations");
    assert_eq!(boundaries_hit, 1);
    assert_eq!(escalations, 0);
    assert_eq!(engine.interaction_bounds(boundary), Some(boundary_rect));
    assert_eq!(engine.interaction_bounds(overflow), Some(Rect::new(0.0, 30.0, 80.0, 152.0)));
    assert_eq!(engine.interaction_bounds(fixed), Some(fixed_rect));
    let snapshot = publication.snapshot_full().unwrap();
    let (_, access_port) = snapshot.nodes.iter().find(|(id, _)| *id == node_identity(port)).unwrap();
    assert_eq!(access_port.scroll_x_max(), Some(0.0));
    assert_eq!(access_port.scroll_y_max(), Some(0.0));
    assert!(draw.quads().any(|quad| quad.source == fixed));
    assert!(draw.items().contains(&Item::PushClip { source: boundary, rect: boundary_rect, radius: [0.0; 4] }));
    assert_eq!(hit(&engine, &tree, Point::new(65.0, 55.0)), Some(fixed));
    assert_ne!(hit(&engine, &tree, Point::new(85.0, 55.0)), Some(fixed));
    assert_ne!(hit(&engine, &tree, Point::new(65.0, 75.0)), Some(fixed));
}
