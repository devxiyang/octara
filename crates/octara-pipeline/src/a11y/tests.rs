use std::collections::BTreeMap;

use octara_model::geometry::{Size, Vec2, WindowViewport};
use octara_model::input::HostReply;
use octara_model::semantics::{A11yAction, TextPosition};
use octara_model::style::{FlexDirection, Length, Overflow, State, StyleProp, When};
use octara_runtime::engine::AccessibilityConsumer;
use octara_tree::overlay::LayerPolicy;
use octara_tree::tables::{AuthoredStyle, HandlerId, SemanticsSlot};
use octara_tree::transaction::OverlaySpec;
use octara_tree::{NodeChange, NodeContent, NodeSpec, Retirement, RuntimeOp};

use super::*;

fn tree() -> Tree {
    Tree::new(TreeId::new([21, 34], 1), Default::default())
}

fn style<const N: usize>(base: [StyleProp; N]) -> NodeChange {
    NodeChange::Style(Box::new(AuthoredStyle { base: base.into_iter().collect(), ..Default::default() }))
}

#[test]
fn a_z_only_update_changes_the_accessible_modal() {
    let mut tree = tree();
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.overlay_root());
    let first = tx.create(NodeSpec::new(NodeContent::Box));
    let second = tx.create(NodeSpec::new(NodeContent::Box));
    for (index, node) in [first, second].into_iter().enumerate() {
        tx.change(node, NodeChange::Overlay(OverlaySpec { layer: Some(LayerPolicy::modal()), ..Default::default() }));
        tx.change(node, NodeChange::Handler { kind: HandlerKind::Press, id: Some(HandlerId(index as u64)) });
        tx.change(
            node,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [StyleProp::Width(80.into()), StyleProp::Height(80.into())].into_iter().collect(),
                overrides: [(When::State(State::SELECTED), vec![StyleProp::ZIndex(10)])].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.place(root, index, node);
    }
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [first, second] = [first, second].map(|node| receipt.node(node).unwrap());
    let mut delta = tree.initial_delta();
    delta.merge(mounted);
    let mut projector = Projection::new();
    let publication = AccessPublication::for_window_host();
    publication.set_active(AccessibilityConsumer::Inspector, true);
    projector.access = AccessKitProjector::new(publication.clone());
    projector.publish(&tree, &delta);
    assert_eq!(publication.context().unwrap().modal, Some(node_identity(second)));
    assert_eq!(publication.target(node_identity(second)), Some(tree.guard(second)));
    assert_eq!(publication.target(node_identity(first)), None);
    assert_eq!(publication.target(node_identity(tree.root())), None);
    assert!(publication.supports_action(tree.guard(second), ak::Action::Click));
    assert!(!publication.supports_action(tree.guard(first), ak::Action::Click));
    assert!(publication.take_platform_commits().is_empty());

    publication.set_active(AccessibilityConsumer::Platform, true);
    let delta = tree.apply_runtime(RuntimeOp::SetState { node: first, state: State::SELECTED, on: true });
    assert!(delta.topology_dirty.is_empty());
    assert_eq!(tree.front_modal(), Some(first));
    projector.publish(&tree, &delta);
    let context = publication.context().unwrap();
    assert_eq!(context.modal, Some(node_identity(first)));
    assert_eq!(context.scale, projector.viewport.scale);
    assert_eq!(context.tree, tree.id());
    assert_eq!(publication.target(node_identity(first)), Some(tree.guard(first)));
    assert_eq!(publication.target(node_identity(second)), None);
    let commits = publication.take_platform_commits();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].context, context);
    assert!(publication.supports_action(tree.guard(first), ak::Action::Click));
    assert!(!publication.supports_action(tree.guard(second), ak::Action::Click));
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(first), NodeChange::Disabled(true));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    projector.publish(&tree, &delta);
    assert!(!publication.supports_action(tree.guard(first), ak::Action::Click));

    publication.set_active(AccessibilityConsumer::Platform, false);
    assert!(publication.snapshot_full().is_some());
    publication.set_active(AccessibilityConsumer::Inspector, false);
    assert!(publication.snapshot_full().is_none());
}

struct Projection {
    layout: LayoutEngine,
    text: TextEngine,
    access: AccessKitProjector,
    viewport: WindowViewport,
}

impl Projection {
    fn new() -> Self {
        let publication = AccessPublication::default();
        publication.set_active(AccessibilityConsumer::Inspector, true);
        Self {
            layout: LayoutEngine::default(),
            text: TextEngine::new(false),
            access: AccessKitProjector::new(publication),
            viewport: WindowViewport { size: Size::new(320.0, 240.0), scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
        }
    }

    fn project(&mut self, tree: &Tree, delta: &TreeDelta, verify: bool) {
        self.text.begin_stage();
        self.layout.project(tree, delta, self.viewport, &mut self.text);
        self.access.project(tree, delta, &self.layout, &self.text, self.viewport.scale, verify);
    }

    fn commit(&mut self) -> ak::TreeUpdate {
        self.layout.commit_stage();
        self.text.commit_stage();
        self.access.commit(0).map_or_else(
            || ak::TreeUpdate {
                nodes: Vec::new(),
                tree: None,
                tree_id: ak::TreeId::ROOT,
                focus: node_identity(self.access.publication.0.borrow().focus.unwrap()),
            },
            Arc::unwrap_or_clone,
        )
    }

    fn publish(&mut self, tree: &Tree, delta: &TreeDelta) -> ak::TreeUpdate {
        self.project(tree, delta, true);
        self.commit()
    }

    fn rollback(&mut self) {
        self.layout.rollback_stage();
        self.text.rollback_stage();
        self.access.reset_staging();
    }
}

#[derive(Default)]
struct AppliedUpdates {
    tree: Option<ak::TreeInfo>,
    focus: Option<ak::NodeId>,
    nodes: BTreeMap<ak::NodeId, ak::Node>,
}

#[test]
fn mixed_check_state_and_description_follow_incremental_identity_changes() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let control = tx.create(NodeSpec::new(NodeContent::Box));
    let description = tx.create(NodeSpec::new(NodeContent::Text("Shared permissions".into())));
    let root = tree.node_ref(tree.root());
    tx.place(root, 0, control);
    tx.place(root, 1, description);
    tx.change(description, NodeChange::TestId(Some("description".into())));
    let semantics = SemanticsSlot {
        role: Some(Role::MenuItemCheckbox),
        checked: Some(Checked::Mixed),
        relations: Relations { described_by: Some("description".into()), ..Default::default() },
        ..Default::default()
    };
    tx.change(control, NodeChange::Semantics(Box::new(semantics.clone())));
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    initial.merge(delta);
    let control = receipt.node(control).unwrap();
    let description = receipt.node(description).unwrap();
    let mut engine = Projection::new();
    let mut applied = AppliedUpdates::default();
    applied.apply(engine.publish(&tree, &initial));
    let state = &applied.nodes[&node_identity(control)];
    assert_eq!(state.role(), ak::Role::MenuItemCheckBox);
    assert_eq!(state.toggled(), Some(ak::Toggled::Mixed));
    assert_eq!(state.described_by(), &[node_identity(description)]);

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(description), NodeChange::TestId(Some("renamed".into())));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    applied.apply(engine.publish(&tree, &delta));
    assert!(applied.nodes[&node_identity(control)].described_by().is_empty());

    for (checked, toggled) in [(Checked::Checked, ak::Toggled::True), (Checked::Unchecked, ak::Toggled::False)] {
        let mut tx = tree.transaction();
        tx.change(tree.node_ref(control), NodeChange::Semantics(Box::new(SemanticsSlot { checked: Some(checked), ..semantics.clone() })));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        applied.apply(engine.publish(&tree, &delta));
        assert_eq!(applied.nodes[&node_identity(control)].toggled(), Some(toggled));
    }
}

impl AppliedUpdates {
    fn apply(&mut self, update: ak::TreeUpdate) {
        if update.tree.is_some() {
            self.tree = update.tree;
        }
        self.focus = Some(update.focus);
        let updated: Vec<_> = update.nodes.iter().map(|(node, _)| *node).collect();
        self.nodes.extend(update.nodes);
        let mut reachable = FxHashSet::default();
        let mut pending = vec![self.tree.as_ref().unwrap().root];
        while let Some(node) = pending.pop() {
            assert!(reachable.insert(node), "duplicate child {node:?}");
            pending.extend_from_slice(self.nodes[&node].children());
        }
        assert!(reachable.contains(&update.focus));
        assert!(updated.into_iter().all(|node| reachable.contains(&node)));
        self.nodes.retain(|node, _| reachable.contains(node));
    }

    fn assert_snapshot(&self, snapshot: &ak::TreeUpdate) {
        assert_eq!(self.tree, snapshot.tree);
        assert_eq!(self.focus, Some(snapshot.focus));
        assert!(self.nodes.iter().eq(snapshot.nodes.iter().map(|(node, value)| (node, value))));
    }
}

#[test]
fn edits_and_local_scroll_do_dirty_work_with_a_lazy_committed_snapshot() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let root = tree.node_ref(tree.root());
    let text = tx.create(NodeSpec::new(NodeContent::Text("first".into())));
    tx.change(text, style([StyleProp::Width(Length::Px(100.0)), StyleProp::Height(Length::Px(24.0))]));
    tx.place(root, 0, text);
    let panel = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        panel,
        style([
            StyleProp::Width(Length::Px(80.0)),
            StyleProp::Height(Length::Px(24.0)),
            StyleProp::Direction(FlexDirection::Column),
            StyleProp::OverflowY(Overflow::Scroll),
        ]),
    );
    tx.place(root, 1, panel);
    for index in 0..3 {
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(child, style([StyleProp::Height(Length::Px(24.0)), StyleProp::Shrink(0.0)]));
        tx.place(panel, index, child);
    }
    let background = tx.create(NodeSpec::new(NodeContent::Box));
    tx.place(root, 2, background);
    for index in 0..511 {
        let sibling = tx.create(NodeSpec::new(NodeContent::Box));
        tx.place(root, index + 3, sibling);
    }
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    initial.merge(mounted);
    let text = receipt.node(text).unwrap();
    let panel = receipt.node(panel).unwrap();
    let background = receipt.node(background).unwrap();

    let mut projector = Projection::new();
    let published = projector.access.publication.clone();
    assert!(published.snapshot_full().is_none());
    let mut applied = AppliedUpdates::default();
    let initial_update = projector.publish(&tree, &initial);
    assert!(initial_update.nodes.len() > 512);
    applied.apply(initial_update);
    assert_eq!(published.0.borrow().snapshotted.get(), 0);

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(text), NodeChange::Text("second".into()));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    projector.project(&tree, &delta, false);
    assert_eq!(projector.access.staging.built, 1);
    assert!(projector.access.staging.candidates <= 6);
    let text_update = projector.commit();
    assert_eq!(published.0.borrow().committed, 1);
    assert_eq!(text_update.nodes.len(), 2);
    applied.apply(text_update);
    assert_eq!(published.0.borrow().snapshotted.get(), 0);

    let delta = tree.apply_runtime(RuntimeOp::SetScroll { node: panel, offset: Vec2::new(0.0, 10.0), clamped: false });
    projector.project(&tree, &delta, false);
    assert_eq!(projector.access.staging.built, 0);
    assert_eq!(projector.access.staging.candidates, 1);
    let scroll_update = projector.commit();
    assert_eq!(published.0.borrow().committed, 1);
    assert_eq!(scroll_update.nodes.len(), 1);
    assert_eq!(scroll_update.nodes[0].0, node_identity(panel));
    applied.apply(scroll_update);
    assert_eq!(published.0.borrow().snapshotted.get(), 0);

    let snapshot = published.snapshot_full().unwrap();
    assert_eq!(snapshot.nodes.len(), 519);
    assert_eq!(published.0.borrow().snapshotted.get(), snapshot.nodes.len());
    applied.assert_snapshot(&snapshot);
    assert!(projector.publish(&tree, &TreeDelta::default()).nodes.is_empty());

    let mut tx = tree.transaction();
    let overlay = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(overlay, NodeChange::Overlay(OverlaySpec { layer: Some(LayerPolicy::modal()), ..Default::default() }));
    let modal_text = tx.create(NodeSpec::new(NodeContent::Text("modal".into())));
    tx.change(modal_text, style([StyleProp::Width(Length::Px(100.0)), StyleProp::Height(Length::Px(24.0))]));
    tx.place(tree.node_ref(tree.overlay_root()), 0, overlay);
    tx.place(overlay, 0, modal_text);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    let modal_text = receipt.node(modal_text).unwrap();
    applied.apply(projector.publish(&tree, &delta));
    let modal_scope = published.0.borrow().modal.clone().unwrap();

    let mut tx = tree.transaction();
    let child = tx.create(NodeSpec::new(NodeContent::Box));
    tx.place(tree.node_ref(background), 0, child);
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    projector.project(&tree, &delta, false);
    assert!(Arc::ptr_eq(projector.access.staging.modal.as_ref().unwrap(), &modal_scope));
    assert_eq!(projector.access.staging.built, 0);
    let background_update = projector.commit();
    assert!(background_update.nodes.is_empty());
    applied.apply(background_update);

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(modal_text), NodeChange::Text("updated".into()));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    projector.project(&tree, &delta, false);
    assert!(Arc::ptr_eq(projector.access.staging.modal.as_ref().unwrap(), &modal_scope));
    assert_eq!(projector.access.staging.built, 1);
    applied.apply(projector.commit());
    applied.assert_snapshot(&published.snapshot_full().unwrap());
}

#[test]
fn modal_topology_relations_and_selection_match_a_fresh_projection() {
    let mut tree = tree();
    let mut initial = tree.initial_delta();
    let mut tx = tree.transaction();
    let region = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(region, NodeChange::Flags(NodeFlags::SELECT_REGION));
    tx.place(tree.node_ref(tree.root()), 0, region);
    let label = tx.create(NodeSpec::new(NodeContent::Text("éclair".into())));
    tx.change(label, NodeChange::TestId(Some("label".into())));
    tx.place(region, 0, label);
    let details = tx.create(NodeSpec::new(NodeContent::Text("body".into())));
    tx.place(region, 1, details);
    let control = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(control, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(
        control,
        NodeChange::Semantics(Box::new(SemanticsSlot {
            role: Some(Role::Button),
            relations: Relations { labelled_by: Some("label".into()), ..Relations::default() },
            ..SemanticsSlot::default()
        })),
    );
    tx.place(tree.node_ref(tree.root()), 1, control);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    initial.merge(delta);
    let region = receipt.node(region).unwrap();
    let label = receipt.node(label).unwrap();
    let details = receipt.node(details).unwrap();
    let control = receipt.node(control).unwrap();
    let mut projector = Projection::new();
    let published = projector.access.publication.clone();
    let mut applied = AppliedUpdates::default();
    applied.apply(projector.publish(&tree, &initial));
    assert_eq!(applied.nodes[&node_identity(control)].labelled_by(), &[node_identity(label)]);

    let delta = tree.apply_runtime(RuntimeOp::SetDocSelection { region, selection: Some([(label, 2), (details, 2)]) });
    applied.apply(projector.publish(&tree, &delta));
    let selection = applied.nodes[&node_identity(region)].text_selection().unwrap();
    assert_eq!(
        published.host_action(
            &tree,
            ak::ActionRequest {
                action: ak::Action::SetTextSelection,
                target_tree: ak::TreeId::ROOT,
                target_node: node_identity(region),
                data: Some(ak::ActionData::SetTextSelection(*selection)),
            },
        ),
        Some(HostReply::AccessibilityAction {
            target: tree.guard(region),
            action: A11yAction::SetTextSelection {
                anchor: TextPosition { node: label, offset: 2 },
                focus: TextPosition { node: details, offset: 2 },
            },
        })
    );

    // A hidden duplicate test ID still makes a relation ambiguous.
    let mut tx = tree.transaction();
    let duplicate = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(duplicate, NodeChange::TestId(Some("label".into())));
    tx.change(duplicate, NodeChange::Flags(NodeFlags::HIDDEN));
    tx.place(tree.node_ref(tree.root()), 2, duplicate);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    let duplicate = receipt.node(duplicate).unwrap();
    applied.apply(projector.publish(&tree, &delta));
    assert!(applied.nodes[&node_identity(control)].labelled_by().is_empty());

    let mut tx = tree.transaction();
    tx.change(tree.node_ref(duplicate), NodeChange::TestId(Some("other".into())));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    applied.apply(projector.publish(&tree, &delta));
    assert_eq!(applied.nodes[&node_identity(control)].labelled_by(), &[node_identity(label)]);

    // Later feedback rounds keep dirty inputs but publish only the final result.
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(label), NodeChange::Flags(NodeFlags::HIDDEN));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    projector.project(&tree, &delta, true);
    assert_eq!(published.node(node_identity(label)), Some(label));
    let mut tx = tree.transaction();
    tx.change(tree.node_ref(label), NodeChange::Flags(NodeFlags::empty()));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    applied.apply(projector.publish(&tree, &delta));
    assert_eq!(applied.nodes[&node_identity(control)].labelled_by(), &[node_identity(label)]);
    assert!(applied.nodes[&node_identity(region)].text_selection().is_none());

    // An unpublished modal and its action targets disappear on rollback.
    let committed = published.snapshot_full().unwrap();
    let mut tx = tree.transaction();
    let modal = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(modal, NodeChange::Overlay(OverlaySpec { layer: Some(LayerPolicy::modal()), ..Default::default() }));
    tx.place(tree.node_ref(tree.overlay_root()), 0, modal);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    let modal = receipt.node(modal).unwrap();
    projector.project(&tree, &delta, true);
    assert_eq!(published.snapshot_full().unwrap(), committed);
    assert_eq!(published.node(node_identity(control)), Some(control));
    assert_eq!(published.node(node_identity(modal)), None);
    projector.rollback();
    assert_eq!(published.snapshot_full().unwrap(), committed);
    applied.apply(projector.publish(&tree, &delta));
    assert_eq!(published.node(node_identity(control)), None);
    assert_eq!(published.node(node_identity(modal)), Some(modal));
    assert_eq!(applied.focus, Some(node_identity(modal)));

    let mut tx = tree.transaction();
    let portal = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(portal, NodeChange::Overlay(OverlaySpec { mount: Some((tree.node_ref(modal), 1)), ..Default::default() }));
    tx.place(tree.node_ref(region), 2, portal);
    let (receipt, delta) = tree.apply(tx.finish()).unwrap();
    let portal = receipt.node(portal).unwrap();
    applied.apply(projector.publish(&tree, &delta));
    assert_eq!(applied.nodes[&node_identity(region)].children(), &[node_identity(portal)]);

    for flags in [NodeFlags::SELECT_REGION | NodeFlags::HIDDEN, NodeFlags::SELECT_REGION] {
        let mut tx = tree.transaction();
        tx.change(tree.node_ref(region), NodeChange::Flags(flags));
        let (_, delta) = tree.apply(tx.finish()).unwrap();
        applied.apply(projector.publish(&tree, &delta));
        assert_eq!(applied.nodes.contains_key(&node_identity(portal)), !flags.contains(NodeFlags::HIDDEN));
    }

    let mut tx = tree.transaction();
    tx.retire(tree.node_ref(portal), Retirement::Destroy);
    tx.retire(tree.node_ref(modal), Retirement::Exit);
    tx.place(tree.node_ref(region), 0, tree.node_ref(details));
    let (_, delta) = tree.apply(tx.finish()).unwrap();
    applied.apply(projector.publish(&tree, &delta));
    assert_eq!(published.node(node_identity(modal)), None);
    assert!(applied.nodes[&node_identity(region)].children().starts_with(&[node_identity(details)]));
    assert_eq!(applied.nodes[&node_identity(control)].labelled_by(), &[node_identity(label)]);

    projector.viewport.scale = 2.0;
    let rescaled = projector.publish(&tree, &TreeDelta::default());
    assert_eq!(rescaled.nodes.len(), 1);
    assert_eq!(rescaled.nodes[0].0, node_identity(tree.root()));
    applied.apply(rescaled);
    applied.assert_snapshot(&published.snapshot_full().unwrap());
}

#[test]
fn a_buttons_text_child_supplies_its_accessible_name() {
    let mut tree = tree();
    let mut tx = tree.transaction();
    let button = tx.create(NodeSpec::new(NodeContent::Box));
    let caption = tx.create(NodeSpec::new(NodeContent::Text("Continue".into())));
    tx.change(button, NodeChange::Semantics(Box::new(SemanticsSlot { role: Some(Role::Button), ..Default::default() })));
    tx.place(tree.node_ref(tree.root()), 0, button);
    tx.place(button, 0, caption);
    let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
    let [button, caption] = [button, caption].map(|node| node_identity(receipt.node(node).unwrap()));
    let mut delta = tree.initial_delta();
    delta.merge(mounted);
    let mut applied = AppliedUpdates::default();
    applied.apply(Projection::new().publish(&tree, &delta));
    assert_eq!(applied.nodes[&button].labelled_by(), &[caption]);
    assert_eq!(applied.nodes[&caption].value(), Some("Continue"));
}
