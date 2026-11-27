use octara_model::{geometry::Placement, theme::Theme};
use octara_tree::{
    NodeChange, NodeContent, NodeFlags, NodeId, NodeSpec, Retirement, RuntimeOp, Tree, TreeId,
    overlay::{FocusTrapFeatures as Features, LayerPolicy},
    transaction::OverlaySpec,
};

fn window() -> (Tree, NodeId) {
    let mut tree = Tree::new(TreeId::new([1, 2], 1), Theme::default());
    let mut tx = tree.transaction();
    let trigger = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(trigger, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.place(tree.node_ref(tree.root()), 0, trigger);
    let trigger = tree.apply(tx.finish()).unwrap().0.node(trigger).unwrap();
    (tree, trigger)
}

fn layer(tree: &mut Tree, trigger: NodeId, policy: LayerPolicy) -> (NodeId, NodeId, NodeId) {
    let mut tx = tree.transaction();
    let layer = tx.create(NodeSpec::new(NodeContent::Box));
    let first = tx.create(NodeSpec::new(NodeContent::Box));
    let preferred = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(first, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(preferred, NodeChange::Flags(NodeFlags::FOCUSABLE | NodeFlags::AUTOFOCUS));
    tx.change(
        layer,
        NodeChange::Overlay(OverlaySpec {
            layer: Some(policy),
            mount: Some((tree.node_ref(trigger), 1)),
            target: Some((octara_model::overlay::AnchorTarget::Node(tree.node_ref(trigger)), Placement::BELOW)),
            ..Default::default()
        }),
    );
    tx.place(tree.node_ref(tree.overlay_root()), 0, layer);
    tx.place(layer, 0, first);
    tx.place(layer, 1, preferred);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    (receipt.node(layer).unwrap(), receipt.node(first).unwrap(), receipt.node(preferred).unwrap())
}

fn focus(tree: &mut Tree, node: Option<NodeId>) {
    tree.apply_runtime(RuntimeOp::SetFocus { node, visible: true });
}

fn retire(tree: &mut Tree, node: NodeId) {
    let mut tx = tree.transaction();
    tx.retire(tree.node_ref(node), Retirement::Destroy);
    tree.apply(tx.finish()).unwrap();
}

#[test]
fn initial_focus_and_restore_are_independent() {
    let (mut tree, trigger) = window();
    let (plain, first, _) = layer(&mut tree, trigger, LayerPolicy::non_modal());
    assert_eq!(tree.focus(), None, "a layer without initial focus ignores its mounted autofocus controls");
    focus(&mut tree, Some(first));
    retire(&mut tree, plain);
    assert_eq!(tree.focus(), None, "a layer without restore focus leaves focus unset after retirement");

    focus(&mut tree, Some(trigger));
    let only_initial = LayerPolicy { focus_trap: Features::INITIAL_FOCUS, ..LayerPolicy::non_modal() };
    let (panel, _, preferred) = layer(&mut tree, trigger, only_initial);
    assert_eq!(tree.focus(), Some(preferred));
    retire(&mut tree, panel);
    assert_eq!(tree.focus(), None);

    focus(&mut tree, Some(trigger));
    let only_restore = LayerPolicy { focus_trap: Features::RESTORE_FOCUS, ..LayerPolicy::non_modal() };
    let (panel, first, _) = layer(&mut tree, trigger, only_restore);
    assert_eq!(tree.focus(), Some(trigger));
    focus(&mut tree, Some(first));
    retire(&mut tree, panel);
    assert_eq!(tree.focus(), Some(trigger));
}

#[test]
fn tab_lock_does_not_block_direct_focus_and_focus_lock_does_not_wrap() {
    let (mut tree, trigger) = window();
    let mut tx = tree.transaction();
    let external = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(external, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.place(tree.node_ref(tree.root()), 1, external);
    let external = tree.apply(tx.finish()).unwrap().0.node(external).unwrap();

    let tab_lock = LayerPolicy { focus_trap: Features::TAB_LOCK, ..LayerPolicy::non_modal() };
    let (panel, first, preferred) = layer(&mut tree, trigger, tab_lock);
    focus(&mut tree, Some(preferred));
    assert_eq!(tree.tab_target(true), Some(trigger));
    focus(&mut tree, Some(trigger));
    assert_eq!(tree.tab_target(false), Some(preferred));
    focus(&mut tree, Some(external));
    assert_eq!(tree.focus(), Some(external));
    retire(&mut tree, panel);

    let focus_lock = LayerPolicy { focus_trap: Features::FOCUS_LOCK, ..LayerPolicy::non_modal() };
    let (_, first_locked, preferred) = layer(&mut tree, trigger, focus_lock);
    assert_ne!(first_locked, first);
    assert_eq!(tree.focus(), Some(external), "focus lock alone leaves initial focus alone");
    assert!(tree.is_interactive(external), "focus lock does not make outside controls inert");
    focus(&mut tree, Some(preferred));
    focus(&mut tree, Some(external));
    focus(&mut tree, None);
    assert_eq!(tree.focus(), Some(preferred));
    assert_eq!(tree.tab_target(true), None);
    assert_eq!(tree.tab_target(false), Some(first_locked));
}

#[test]
fn auto_focus_recovers_a_removed_descendant_to_the_container() {
    let (mut tree, trigger) = window();
    let auto_focus = LayerPolicy { focus_trap: Features::AUTO_FOCUS, ..LayerPolicy::non_modal() };
    let (panel, first, preferred) = layer(&mut tree, trigger, auto_focus);
    assert_eq!(tree.focus(), None);
    focus(&mut tree, Some(preferred));
    retire(&mut tree, preferred);
    assert_eq!(tree.focus(), Some(panel));
    assert_eq!(tree.tab_target(true), Some(trigger));
    assert!(tree.focus_order(Some(panel), tree.focus()).contains(&first));
}

#[test]
fn tab_lock_keeps_focus_in_an_empty_modal() {
    let (mut tree, trigger) = window();
    let (panel, first, preferred) = layer(&mut tree, trigger, LayerPolicy::modal());
    retire(&mut tree, first);
    retire(&mut tree, preferred);
    assert_eq!(tree.focus(), Some(panel));
    assert_eq!(tree.tab_target(true), Some(panel));
    assert_eq!(tree.tab_target(false), Some(panel));
}
