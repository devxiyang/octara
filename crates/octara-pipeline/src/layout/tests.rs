use super::*;
use octara_model::geometry::Placement;
use octara_model::input::EditorEvent;
use octara_model::style::{Length, StyleProp};
use octara_tree::editor::{EditorAction, EditorSession};
use octara_tree::tables::{AuthoredStyle, EditorSlot, LinkSlot};
use octara_tree::transaction::{OverlaySpec, RichText};
use octara_tree::{NodeChange, NodeContent, NodeSpec, Retirement, TreeId, TxBuilder};

fn style(props: impl IntoIterator<Item = StyleProp>) -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle { base: props.into_iter().collect(), ..Default::default() }))
}

#[test]
fn scroll_anchoring_keeps_window_bounds_stable_in_the_layout_that_moves_the_child() {
    let mut tree = Tree::new(TreeId::new([3, 5], 10), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    let port = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(port, NodeChange::Flags(NodeFlags::MAINTAIN_SCROLL_ANCHOR));
    tx.change(
        port,
        style([
            StyleProp::Width(Length::Px(100.0)),
            StyleProp::Height(Length::Px(50.0)),
            StyleProp::Shrink(0.0),
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::OverflowY(Overflow::Scroll),
        ]),
    );
    tx.place(tree.node_ref(tree.root()), 0, port);
    let leading = tx.create(NodeSpec::new(NodeContent::Box));
    let hidden = tx.create(NodeSpec::new(NodeContent::Box));
    let anchor = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(hidden, NodeChange::Flags(NodeFlags::HIDDEN));
    let height = |px| style([StyleProp::Height(Length::Px(px)), StyleProp::Shrink(0.0)]);
    for (index, (row, length)) in [(leading, 80.0), (hidden, 20.0), (anchor, 160.0)].into_iter().enumerate() {
        tx.change(row, height(length));
        tx.place(port, index, row);
    }
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    delta.merge(created);
    let port = receipt.node(port).unwrap();
    let leading = receipt.node(leading).unwrap();
    let hidden = receipt.node(hidden).unwrap();
    let anchor = receipt.node(anchor).unwrap();
    let viewport = WindowViewport { size: Size::new(200.0, 200.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    delta.merge(tree.apply_runtime(RuntimeOp::RequestScroll { node: port, target: Vec2::new(0.0, 90.0) }));
    for update in layout.project(&tree, &delta, viewport, &mut text) {
        tree.apply_runtime(update);
    }
    layout.commit_stage();
    text.commit_stage();
    assert_eq!(layout.scroll_anchor(port), Some(anchor));
    let before = layout.bounds(anchor).unwrap();
    assert_eq!(before.y, 10.0);

    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    tx.change(tree.node_ref(leading), height(95.0));
    tx.change(tree.node_ref(hidden), height(35.0));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    text.begin_stage();
    let updates = layout.project(&tree, &delta, viewport, &mut text);
    assert_eq!(layout.bounds(anchor), Some(before));
    assert_eq!(tree.scroll(port).unwrap().offset.y, 90.0);
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, RuntimeOp::SetScroll { node, offset, clamped: false } if *node == port && offset.y == 120.0))
    );
}

#[test]
fn initial_keyboard_focus_reveals_an_inline_link_before_link_projection() {
    let mut tree = Tree::new(TreeId::new([3, 5], 11), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    let port = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        port,
        style([
            StyleProp::Width(Length::Px(120.0)),
            StyleProp::Height(Length::Px(35.0)),
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::OverflowY(Overflow::Scroll),
        ]),
    );
    tx.place(tree.node_ref(tree.root()), 0, port);
    let label = tx.create(NodeSpec::new(NodeContent::Text("".into())));
    let value = "one\ntwo\nthree\nfour\nfive\nend";
    tx.change(
        label,
        NodeChange::RichText(Box::new(RichText {
            text: value.into(),
            links: vec![LinkSlot { range: value.len() - 3..value.len(), href: "#end".into() }],
            ..Default::default()
        })),
    );
    tx.change(label, style([StyleProp::Shrink(0.0)]));
    tx.place(port, 0, label);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    delta.merge(mounted);
    let port = receipt.node(port).unwrap();
    let link = tree.children(receipt.node(label).unwrap())[0];
    delta.merge(tree.apply_runtime(RuntimeOp::SetFocus { node: Some(link), visible: true }));
    let viewport = WindowViewport { size: Size::new(200.0, 200.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    assert!(layout.bounds(link).is_none());
    layout.project(&tree, &delta, viewport, &mut text);
    let link = layout.bounds(link).unwrap();
    assert!(layout.scroll_offset(port).y > 0.0);
    assert!(link.y >= 0.0 && link.bottom() <= 35.0, "{link:?}");
}

#[test]
fn scroll_projects_only_its_subtree_links_and_dependent_anchors() {
    let mut tree = Tree::new(TreeId::new([3, 5], 1), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    let port = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        port,
        style([
            StyleProp::Width(Length::Px(120.0)),
            StyleProp::Height(Length::Px(100.0)),
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::OverflowY(Overflow::Scroll),
            StyleProp::Shrink(0.0),
        ]),
    );
    tx.place(tree.node_ref(tree.root()), 0, port);
    let leading = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(leading, style([StyleProp::Height(Length::Px(40.0)), StyleProp::Shrink(0.0)]));
    tx.place(port, 0, leading);
    let linked_text = || {
        NodeChange::RichText(Box::new(RichText {
            text: "open details".into(),
            links: vec![LinkSlot { range: 0..4, href: "#open".into() }],
            ..Default::default()
        }))
    };
    let label = tx.create(NodeSpec::new(NodeContent::Text("".into())));
    tx.change(label, linked_text());
    tx.change(label, style([StyleProp::Shrink(0.0)]));
    tx.place(port, 1, label);
    let trailing = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(trailing, style([StyleProp::Height(Length::Px(180.0)), StyleProp::Shrink(0.0)]));
    tx.place(port, 2, trailing);
    let unrelated = tx.create(NodeSpec::new(NodeContent::Text("".into())));
    tx.change(unrelated, linked_text());
    tx.place(tree.node_ref(tree.root()), 1, unrelated);
    for index in 0..500 {
        let leaf = tx.create(NodeSpec::new(NodeContent::Box));
        tx.place(tree.node_ref(tree.root()), index + 2, leaf);
    }
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    delta.merge(created);
    let port = receipt.node(port).unwrap();
    let label = receipt.node(label).unwrap();
    let unrelated = receipt.node(unrelated).unwrap();
    let link = tree.children(label)[0];
    let unrelated_link = tree.children(unrelated)[0];

    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    let first = tx.create(NodeSpec::new(NodeContent::Box));
    let second = tx.create(NodeSpec::new(NodeContent::Box));
    let idle = tx.create(NodeSpec::new(NodeContent::Box));
    for (index, (node, target)) in
        [(first, tree.node_ref(link)), (second, first.into()), (idle, tree.node_ref(unrelated))].into_iter().enumerate()
    {
        tx.change(node, style([StyleProp::Width(Length::Px(40.0)), StyleProp::Height(Length::Px(20.0))]));
        tx.change(
            node,
            NodeChange::Overlay(OverlaySpec { target: Some((AnchorTarget::Node(target), Placement::BELOW)), ..Default::default() }),
        );
        tx.place(tree.node_ref(tree.overlay_root()), index, node);
    }
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    delta.merge(created);
    let first = receipt.node(first).unwrap();
    let second = receipt.node(second).unwrap();
    let idle = receipt.node(idle).unwrap();
    let viewport = WindowViewport { size: Size::new(320.0, 300.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    for update in layout.project(&tree, &delta, viewport, &mut text) {
        tree.apply_runtime(update);
    }
    layout.commit_stage();
    text.commit_stage();
    let before = layout.work();
    let initial_link = layout.bounds(link).unwrap();
    let initial_clip = layout.clip(link);
    let initial_first = layout.bounds(first).unwrap();
    let initial_second = layout.bounds(second).unwrap();
    let initial_unrelated = layout.bounds(unrelated_link);
    let initial_idle = layout.bounds(idle);

    let delta = tree.apply_runtime(RuntimeOp::RequestScroll { node: port, target: Vec2::new(0.0, 20.0) });
    text.begin_stage();
    let updates = layout.project(&tree, &delta, viewport, &mut text);
    let after = layout.work();
    assert_eq!(after.computes, before.computes);
    assert_eq!(after.frames - before.frames, 6);
    assert_eq!(after.anchors - before.anchors, 2);
    assert_eq!(after.links - before.links, 1);
    assert_eq!(layout.bounds(link).unwrap().y, initial_link.y - 20.0);
    assert_eq!(layout.bounds(first).unwrap().y, initial_first.y - 20.0);
    assert_eq!(layout.bounds(second).unwrap().y, initial_second.y - 20.0);
    assert_eq!(layout.clip(link), initial_clip);
    assert_eq!(layout.bounds(unrelated_link), initial_unrelated);
    assert_eq!(layout.bounds(idle), initial_idle);
    assert!(layout.changed().contains(&link));
    assert!(!layout.changed().contains(&unrelated_link));
    assert!(updates.iter().any(|update| matches!(
        update,
        RuntimeOp::SetScroll { node, offset, clamped: false } if *node == port && offset.y == 20.0
    )));

    layout.rollback_stage();
    text.rollback_stage();
    assert_eq!(layout.bounds(link), Some(initial_link));
    assert_eq!(layout.bounds(first), Some(initial_first));
    assert_eq!(layout.bounds(second), Some(initial_second));
    layout.project(&tree, &delta, viewport, &mut text);
    assert_eq!(layout.work().computes, after.computes);
    assert_eq!(layout.work().frames - after.frames, 6);
    assert_eq!(layout.bounds(link).unwrap().y, initial_link.y - 20.0);
    assert_eq!(layout.bounds(second).unwrap().y, initial_second.y - 20.0);
}

#[test]
fn aborted_topology_restores_projected_geometry_and_reuses_taffy() {
    let mut tree = Tree::new(TreeId::new([3, 5], 2), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    let existing = tx.create(NodeSpec::new(NodeContent::Text("original".into())));
    let retired = tx.create(NodeSpec::new(NodeContent::Box));
    tx.place(tree.node_ref(tree.root()), 0, existing);
    tx.place(tree.node_ref(tree.root()), 1, retired);
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    delta.merge(created);
    let existing = receipt.node(existing).unwrap();
    let retired = receipt.node(retired).unwrap();
    let viewport = WindowViewport { size: Size::new(320.0, 300.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    layout.project(&tree, &delta, viewport, &mut text);
    layout.commit_stage();
    text.commit_stage();
    let committed = layout.bounds(existing).unwrap();
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    tx.retire(tree.node_ref(retired), Retirement::Destroy);
    let (_, mut delta) = tree.apply(tx.finish()).unwrap();
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    let created = tx.create(NodeSpec::new(NodeContent::Text("new sibling".into())));
    tx.place(tree.node_ref(tree.root()), 0, created);
    tx.change(tree.node_ref(existing), NodeChange::Text("changed".into()));
    let (receipt, replacement) = tree.apply(tx.finish()).unwrap();
    delta.merge(replacement);
    let created = receipt.node(created).unwrap();
    text.begin_stage();
    layout.project(&tree, &delta, viewport, &mut text);
    let staged = layout.bounds(existing).unwrap();
    let computes = layout.work().computes;
    assert_eq!(layout.taffy.total_node_count(), tree.nodes().count());
    assert!(staged.y > committed.y);
    assert!(layout.bounds(created).is_some());
    layout.rollback_stage();
    text.rollback_stage();
    assert_eq!(layout.bounds(existing), Some(committed));
    assert_eq!(layout.bounds(created), None);
    assert!(layout.bounds(retired).is_some());
    layout.project(&tree, &delta, viewport, &mut text);
    assert_eq!(layout.work().computes, computes);
    assert_eq!(layout.bounds(existing), Some(staged));
    assert!(layout.bounds(created).is_some());
    assert_eq!(layout.bounds(retired), None);
    assert_eq!(layout.taffy.total_node_count(), tree.nodes().count());
}

#[test]
fn fixed_height_editor_reveals_the_caret_and_replaces_text_without_recomputing_taffy() {
    let mut tree = Tree::new(TreeId::new([3, 5], 3), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = tree.transaction();
    let value = "first\nsecond\nthird\nfourth\nfifth\nsixth\nseventh";
    let mut editor = EditorSlot::new(value);
    editor.multiline = true;
    editor.rows = (2, 2);
    let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(editor))));
    tx.change(editor, style([StyleProp::Width(Length::Px(160.0))]));
    tx.place(tree.node_ref(tree.root()), 0, editor);
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    delta.merge(created);
    let editor = receipt.node(editor).unwrap();
    delta.merge(tree.apply_runtime(RuntimeOp::SetFocus { node: Some(editor), visible: true }));
    let viewport = WindowViewport { size: Size::new(320.0, 300.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    layout.project(&tree, &delta, viewport, &mut text);
    layout.commit_stage();
    text.commit_stage();
    let before = layout.work();
    let bounds = layout.bounds(editor).unwrap();
    let old_caret = text.caret(editor).unwrap();
    let session = EditorSession::active(&tree).unwrap();
    let EditorAction::Apply(update) = session
        .route(EditorEvent::ProposeSelection {
            session: session.id(),
            expected_rev: session.editor().document_rev(),
            selection: (value.len(), value.len()),
        })
        .unwrap()
    else {
        panic!("expected a selection update");
    };
    let delta = tree.apply_runtime(RuntimeOp::EditorApply { node: editor, update });
    text.begin_stage();
    let updates = layout.project(&tree, &delta, viewport, &mut text);
    assert_eq!(layout.work().computes, before.computes);
    assert_eq!(layout.work().frames - before.frames, 1);
    assert_eq!(layout.bounds(editor), Some(bounds));
    assert!(layout.scroll_offset(editor).y > 0.0);
    assert!(updates.iter().any(|update| matches!(update, RuntimeOp::SetScroll { node, .. } if *node == editor)));
    let caret = text.caret(editor).unwrap();
    assert!(caret.y > old_caret.y);
    assert!(caret.bottom() <= bounds.bottom());
    layout.rollback_stage();
    text.rollback_stage();
    assert_eq!(layout.scroll_offset(editor), Vec2::default());
    assert_eq!(text.caret(editor), Some(old_caret));

    let before = layout.work();
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(editor), NodeChange::EditorValue("shorter".into()));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    layout.project(&tree, &delta, viewport, &mut text);
    assert_eq!(layout.work().computes, before.computes);
    assert_eq!(layout.work().frames - before.frames, 1);
    assert_eq!(layout.bounds(editor), Some(bounds));
    assert_eq!(text.geometry(editor).unwrap().lines.len(), 1);
}

#[test]
fn growing_editor_recomputes_layout_after_the_value_changes() {
    let mut tree = Tree::new(TreeId::new([3, 5], 4), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = tree.transaction();
    let mut editor = EditorSlot::new("one line");
    editor.multiline = true;
    editor.rows = (1, 3);
    let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(editor))));
    tx.place(tree.node_ref(tree.root()), 0, editor);
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    delta.merge(created);
    let editor = receipt.node(editor).unwrap();
    let viewport = WindowViewport { size: Size::new(320.0, 300.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    layout.project(&tree, &delta, viewport, &mut text);
    let before = layout.work();
    let original_height = layout.bounds(editor).unwrap().h;
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(editor), NodeChange::EditorValue("first\nsecond\nthird".into()));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    layout.project(&tree, &delta, viewport, &mut text);
    assert_eq!(layout.work().computes - before.computes, 1);
    assert_eq!(layout.bounds(editor).unwrap().h, original_height * 3.0);
}

#[test]
fn settles_safe_fit_scroll_feedback_and_destroyed_layout_nodes() {
    let mut tree = Tree::new(TreeId::new([3, 5], 1), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    let anchor = tx.create(NodeSpec::new(NodeContent::Box));
    let overlay = tx.create(NodeSpec::new(NodeContent::Box));
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(anchor, style([StyleProp::Height(Length::Px(36.0))]));
    tx.change(overlay, style([StyleProp::Width(Length::Px(100.0)), StyleProp::Direction(FlexDirection::Column)]));
    tx.change(child, style([StyleProp::Height(Length::Px(150.0)), StyleProp::Shrink(0.0)]));
    tx.change(
        overlay,
        NodeChange::Overlay(OverlaySpec { target: Some((AnchorTarget::Node(anchor.into()), Placement::BELOW)), ..Default::default() }),
    );
    tx.place(tree.node_ref(tree.root()), 0, anchor);
    tx.place(tree.node_ref(tree.overlay_root()), 0, overlay);
    tx.place(overlay, 0, child);
    let (receipt, transaction_delta) = tree.apply(tx.finish()).unwrap();
    delta.merge(transaction_delta);
    let overlay = receipt.node(overlay).unwrap();
    let child = receipt.node(child).unwrap();
    let viewport = WindowViewport {
        size: Size::new(180.0, 140.0),
        scale: 1.0,
        safe_area: Some(Rect::new(12.0, 16.0, 144.0, 112.0)),
        keyboard_inset: 0.0,
    };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    let updates = layout.project(&tree, &delta, viewport, &mut text);
    assert_eq!(layout.bounds(overlay), Some(Rect::new(20.0, 40.0, 100.0, 80.0)));
    assert_eq!(layout.scroll_range(overlay).expect("fitted overlay scrollport").max.y, 70.0);
    let mut delta = TreeDelta::default();
    for update in updates {
        delta.merge(tree.apply_runtime(update));
    }
    delta.merge(tree.apply_runtime(RuntimeOp::RequestScroll { node: overlay, target: Vec2::new(0.0, 500.0) }));
    let updates = layout.project(&tree, &delta, viewport, &mut text);
    assert!(updates.iter().any(|update| matches!(
        update,
        RuntimeOp::SetScroll { node, offset, clamped: true } if *node == overlay && offset.y == 70.0
    )));
    assert_eq!(layout.bounds(child).unwrap().y, -30.0);
    let mut tx = TxBuilder::new(tree.id(), tree.revision());
    tx.retire(tree.node_ref(child), Retirement::Destroy);
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    layout.project(&tree, &delta, viewport, &mut text);
    assert!(layout.bounds(child).is_none());
    assert_eq!(layout.scroll_range(overlay).expect("overlay scrollport").max.y, 0.0);
}

#[test]
fn unanchored_portal_geometry_follows_the_display_of_its_owner() {
    let mut tree = Tree::new(TreeId::new([3, 5], 6), Default::default());
    let mut delta = tree.initial_delta();
    let mut tx = tree.transaction();
    let page = tx.create(NodeSpec::new(NodeContent::Box));
    let portal = tx.create(NodeSpec::new(NodeContent::Box));
    tx.place(tree.node_ref(tree.root()), 0, page);
    tx.place(tree.node_ref(tree.overlay_root()), 0, portal);
    tx.change(portal, NodeChange::Overlay(OverlaySpec { mount: Some((page.into(), 1)), ..Default::default() }));
    tx.change(portal, style([StyleProp::Width(Length::Px(50.0)), StyleProp::Height(Length::Px(40.0))]));
    let (receipt, created) = tree.apply(tx.finish()).unwrap();
    delta.merge(created);
    let page = receipt.node(page).unwrap();
    let portal = receipt.node(portal).unwrap();
    let viewport = WindowViewport { size: Size::new(320.0, 300.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
    let mut layout = LayoutEngine::default();
    let mut text = TextEngine::new(false);
    layout.project(&tree, &delta, viewport, &mut text);
    let bounds = layout.bounds(portal).expect("visible portal bounds");
    for (display, expected) in [(Display::None, None), (Display::Flex, Some(bounds))] {
        let mut tx = tree.transaction();
        tx.change(tree.node_ref(page), style([StyleProp::Display(display)]));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        layout.project(&tree, &delta, viewport, &mut text);
        assert_eq!(layout.bounds(portal), expected);
    }
}
