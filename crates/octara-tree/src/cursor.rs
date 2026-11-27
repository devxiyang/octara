use crate::tables::HandlerKind;
use crate::{NodeId, NodeKind, Tree};
use octara_model::input::CursorStyle;
use octara_model::semantics::Role;
use octara_model::style::State;

/// Resolve the cursor at a hit node using logical ancestry, shared by desktop and Web.
pub fn resolve_cursor(tree: &Tree, hit: Option<NodeId>) -> CursorStyle {
    let hit = hit.filter(|&node| tree.is_live(node));
    let mut fallback = None;
    for node in std::iter::successors(hit, |&node| tree.logical_parent(node)) {
        let core = tree.node(node).expect("live hit ancestry");
        let semantics = tree.semantics(node);
        let clickable = core.kind == NodeKind::Link
            || matches!(
                semantics.and_then(|slot| slot.role),
                Some(
                    Role::Button
                        | Role::Link
                        | Role::Tab
                        | Role::CheckBox
                        | Role::Switch
                        | Role::RadioButton
                        | Role::MenuItem
                        | Role::MenuItemCheckbox
                        | Role::MenuItemRadio
                        | Role::ListBoxOption
                )
            )
            || tree.has_activation_handler(node);
        if core.state.contains(State::DISABLED) {
            return if clickable || core.kind == NodeKind::TextInput { CursorStyle::OperationNotAllowed } else { CursorStyle::Arrow };
        }
        if let Some(cursor) = tree.style(node).and_then(|slot| slot.visual.cursor).or_else(|| semantics.and_then(|slot| slot.cursor)) {
            return cursor;
        }
        if fallback.is_none() {
            fallback = match core.kind {
                NodeKind::TextInput => Some(CursorStyle::IBeam),
                NodeKind::Text if tree.region_of(node).is_some() => Some(CursorStyle::IBeam),
                _ if clickable => Some(CursorStyle::PointingHand),
                _ => None,
            };
        }
    }
    fallback.unwrap_or_default()
}

impl Tree {
    /// Whether the node has a handler for pointer activation.
    pub fn has_activation_handler(&self, node: NodeId) -> bool {
        [HandlerKind::Press, HandlerKind::SecondaryPress, HandlerKind::Toggle, HandlerKind::DoublePress, HandlerKind::Drag]
            .into_iter()
            .any(|handler| self.handler(node, handler).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::EditorSlot;
    use crate::tables::{AuthoredStyle, HandlerId, LinkSlot, SemanticsSlot};
    use crate::transaction::RichText;
    use crate::{NodeChange, NodeContent, NodeFlags, NodeSpec, RuntimeOp, TreeId};
    use octara_model::style::{StyleProp, When};

    #[test]
    fn semantic_cursors_follow_the_hit_and_its_ancestors() {
        use CursorStyle::{Arrow, IBeam, OperationNotAllowed, PointingHand};

        let mut tree = Tree::new(TreeId::new([1, 2], 0), Default::default());
        let root = tree.node_ref(tree.root());
        let mut tx = tree.transaction();
        let region = tx.create(NodeSpec::new(NodeContent::Box));
        let selectable = tx.create(NodeSpec::new(NodeContent::Text("Link and more".into())));
        let excluded = tx.create(NodeSpec::new(NodeContent::Text("Private".into())));
        let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("")))));
        let button = tx.create(NodeSpec::new(NodeContent::Box));
        let label = tx.create(NodeSpec::new(NodeContent::Text("Button".into())));
        let action = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(region, NodeChange::Flags(NodeFlags::SELECT_REGION));
        tx.change(
            selectable,
            NodeChange::RichText(Box::new(RichText {
                text: "Link and more".into(),
                links: vec![LinkSlot { range: 0..4, href: "#link".into() }],
                ..Default::default()
            })),
        );
        tx.change(excluded, NodeChange::Flags(NodeFlags::SELECT_NONE));
        tx.change(button, NodeChange::Semantics(Box::new(SemanticsSlot { role: Some(Role::Button), ..Default::default() })));
        tx.change(action, NodeChange::Handler { kind: HandlerKind::Press, id: Some(HandlerId(1)) });
        for (index, child) in [region, editor, button, action].into_iter().enumerate() {
            tx.place(root, index, child);
        }
        tx.place(region, 0, selectable);
        tx.place(region, 1, excluded);
        tx.place(button, 0, label);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let [region, selectable, excluded, editor, button, label, action] =
            [region, selectable, excluded, editor, button, label, action].map(|node| receipt.node(node).unwrap());
        let link = tree.children(selectable)[0];

        for (hit, cursor) in [
            (None, Arrow),
            (Some(region), Arrow),
            (Some(selectable), IBeam),
            (Some(excluded), Arrow),
            (Some(link), PointingHand),
            (Some(editor), IBeam),
            (Some(label), PointingHand),
            (Some(action), PointingHand),
        ] {
            assert_eq!(resolve_cursor(&tree, hit), cursor);
        }

        let mut tx = tree.transaction();
        tx.change(tree.node_ref(button), NodeChange::Disabled(true));
        tx.change(tree.node_ref(editor), NodeChange::Disabled(true));
        tree.apply(tx.finish()).unwrap();
        assert_eq!(resolve_cursor(&tree, Some(label)), OperationNotAllowed);
        assert_eq!(resolve_cursor(&tree, Some(editor)), OperationNotAllowed);
    }

    #[test]
    fn resolved_authoring_overrides_semantic_defaults() {
        use CursorStyle::{Arrow, Crosshair, OpenHand, OperationNotAllowed, ResizeColumn, Wait};

        let mut tree = Tree::new(TreeId::new([1, 2], 1), Default::default());
        let mut tx = tree.transaction();
        let parent = tx.create(NodeSpec::new(NodeContent::Box));
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(
            parent,
            NodeChange::Style(Box::new(AuthoredStyle {
                base: [StyleProp::Cursor(ResizeColumn)].into_iter().collect(),
                overrides: [(When::State(State::HOVER), vec![StyleProp::Cursor(OpenHand)])].into_iter().collect(),
                ..Default::default()
            })),
        );
        tx.change(parent, NodeChange::Semantics(Box::new(SemanticsSlot { cursor: Some(Crosshair), ..Default::default() })));
        tx.change(child, NodeChange::Semantics(Box::new(SemanticsSlot { role: Some(Role::MenuItem), ..Default::default() })));
        tx.place(tree.node_ref(tree.root()), 0, parent);
        tx.place(parent, 0, child);
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let [parent, child] = [parent, child].map(|node| receipt.node(node).unwrap());
        assert_eq!(resolve_cursor(&tree, Some(child)), ResizeColumn);

        tree.apply_runtime(RuntimeOp::SetState { node: parent, state: State::HOVER, on: true });
        assert_eq!(resolve_cursor(&tree, Some(child)), OpenHand);

        let mut tx = tree.transaction();
        tx.change(
            tree.node_ref(child),
            NodeChange::Semantics(Box::new(SemanticsSlot { role: Some(Role::MenuItem), cursor: Some(Wait), ..Default::default() })),
        );
        tree.apply(tx.finish()).unwrap();
        assert_eq!(resolve_cursor(&tree, Some(child)), Wait);

        let mut tx = tree.transaction();
        tx.change(tree.node_ref(child), NodeChange::Disabled(true));
        tx.change(tree.node_ref(parent), NodeChange::Disabled(true));
        tree.apply(tx.finish()).unwrap();
        assert_eq!(resolve_cursor(&tree, Some(child)), OperationNotAllowed);
        assert_eq!(resolve_cursor(&tree, Some(parent)), Arrow);
    }
}
