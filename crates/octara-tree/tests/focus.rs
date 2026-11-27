use octara_model::{
    style::{LayoutLength, State, StyleProp, When},
    theme::Theme,
};
use octara_tree::{
    NodeChange, NodeContent, NodeFlags, NodeId, NodeSpec, Retirement, RuntimeOp, Tree, TreeId,
    tables::{AuthoredStyle, LinkSlot},
    transaction::RichText,
};

fn tree() -> Tree {
    Tree::new(TreeId::new([2, 7], 1), Theme::default())
}

fn state(tree: &Tree, node: NodeId) -> State {
    tree.node(node).unwrap().state
}

#[test]
fn states_iterate_in_override_order() {
    let expected = [
        State::HOVER,
        State::FOCUS_WITHIN,
        State::FOCUSED,
        State::FOCUS_VISIBLE,
        State::PRESSED,
        State::SELECTED,
        State::CHECKED,
        State::EXPANDED,
        State::DISABLED,
        State::DRAG_OVER,
        State::DRAGGING,
        State::DROP_HOVER,
    ];
    assert_eq!(State::all().iter().collect::<Vec<_>>(), expected);
}

#[test]
fn focus_states_track_modality_and_resist_direct_writes() {
    let mut tree = tree();
    let root = tree.root();
    let mut tx = tree.transaction();
    let parent = tx.create(NodeSpec::new(NodeContent::Box));
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    for node in [parent, child] {
        tx.change(node, NodeChange::Flags(NodeFlags::FOCUSABLE));
    }
    tx.place(tree.node_ref(root), 0, parent);
    tx.place(parent, 0, child);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [parent, child] = [parent, child].map(|id| receipt.node(id).unwrap());

    tree.apply_runtime(RuntimeOp::SetState { node: parent, state: State::TREE_DERIVED | State::HOVER, on: true });
    assert_eq!(state(&tree, parent), State::HOVER);

    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(child), visible: false });
    assert_eq!(state(&tree, root), State::FOCUS_WITHIN);
    assert_eq!(state(&tree, parent), State::FOCUS_WITHIN | State::HOVER);
    assert_eq!(state(&tree, child), State::FOCUS_WITHIN | State::FOCUSED);
    let delta = tree.apply_runtime(RuntimeOp::SetFocus { node: Some(child), visible: true });
    assert_eq!(delta.semantics_dirty.into_iter().collect::<Vec<_>>(), [child]);
    assert_eq!(state(&tree, child), State::TREE_DERIVED);
    assert!(tree.apply_runtime(RuntimeOp::SetState { node: child, state: State::TREE_DERIVED, on: false }).is_empty());
    assert_eq!(state(&tree, child), State::TREE_DERIVED);

    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(parent), visible: true });
    assert_eq!(state(&tree, child), State::empty());
    assert_eq!(state(&tree, parent), State::TREE_DERIVED | State::HOVER);
    tree.apply_runtime(RuntimeOp::SetFocus { node: None, visible: true });
    assert_eq!(state(&tree, parent), State::HOVER);
    assert_eq!(state(&tree, root), State::empty());
}

#[test]
fn focus_within_follows_reparenting_and_clears_on_exit() {
    let mut tree = tree();
    let root = tree.root();
    let mut tx = tree.transaction();
    let old_parent = tx.create(NodeSpec::new(NodeContent::Box));
    let new_parent = tx.create(NodeSpec::new(NodeContent::Box));
    let branch = tx.create(NodeSpec::new(NodeContent::Box));
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    for (index, node) in [old_parent, new_parent].into_iter().enumerate() {
        tx.change(
            node,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [StyleProp::Width(20.into())].into_iter().collect(),
                overrides: [(When::State(State::FOCUS_WITHIN), [StyleProp::Width(40.into())].into_iter().collect())].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.place(tree.node_ref(root), index, node);
    }
    tx.change(child, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.place(old_parent, 0, branch);
    tx.place(branch, 0, child);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [old_parent, new_parent, branch, child] = [old_parent, new_parent, branch, child].map(|id| receipt.node(id).unwrap());
    let widths = |tree: &Tree| [old_parent, new_parent].map(|node| tree.layout_style(node).size.width);
    let normal = Some(LayoutLength::Px(20.0));
    let focused = Some(LayoutLength::Px(40.0));

    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(child), visible: true });
    assert_eq!(widths(&tree), [focused, normal]);

    let mut tx = tree.transaction();
    tx.place(tree.node_ref(new_parent), 0, tree.node_ref(branch));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.focus(), Some(child));
    assert_eq!(state(&tree, child), State::TREE_DERIVED);
    assert_eq!(state(&tree, branch), State::FOCUS_WITHIN);
    assert_eq!(state(&tree, old_parent), State::empty());
    assert_eq!(state(&tree, new_parent), State::FOCUS_WITHIN);
    assert_eq!(state(&tree, root), State::FOCUS_WITHIN);
    assert_eq!(widths(&tree), [normal, focused]);
    assert!([old_parent, new_parent].iter().all(|node| delta.layout_dirty.contains(node)));

    let mut tx = tree.transaction();
    tx.retire(tree.node_ref(branch), Retirement::Exit);
    tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.focus(), None);
    for node in [child, branch, new_parent, root] {
        assert_eq!(state(&tree, node), State::empty());
    }
    assert_eq!(widths(&tree), [normal, normal]);
}

#[test]
fn detaching_a_focused_branch_clears_its_ancestor_states() {
    let mut tree = tree();
    let root = tree.root();
    let mut tx = tree.transaction();
    let parent = tx.create(NodeSpec::new(NodeContent::Box));
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(child, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.place(tree.node_ref(root), 0, child);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let [parent, child] = [parent, child].map(|id| receipt.node(id).unwrap());

    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(child), visible: true });
    let mut tx = tree.transaction();
    tx.place(tree.node_ref(parent), 0, tree.node_ref(child));
    tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.focus(), None);
    for node in [child, parent, root] {
        assert_eq!(state(&tree, node), State::empty());
    }
}

#[test]
fn replacing_a_focused_inline_link_clears_focus_within() {
    let mut tree = tree();
    let root = tree.root();
    let mut tx = tree.transaction();
    let text = tx.create(NodeSpec::new(NodeContent::Text("link".into())));
    tx.change(
        text,
        NodeChange::RichText(Box::new(RichText {
            text: "link".into(),
            links: vec![LinkSlot { range: 0..4, href: "/link".into() }],
            ..Default::default()
        })),
    );
    tx.place(tree.node_ref(root), 0, text);
    let text = tree.apply(tx.finish()).unwrap().0.node(text).unwrap();
    let link = tree.children(text)[0];

    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(link), visible: false });
    assert_eq!(state(&tree, text), State::FOCUS_WITHIN);
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(text), NodeChange::Text("plain".into()));
    tree.apply(tx.finish()).unwrap();
    assert_eq!(tree.focus(), None);
    assert!(tree.node(link).is_none());
    assert_eq!(state(&tree, text), State::empty());
    assert_eq!(state(&tree, root), State::empty());
}
