use super::{InteractionArbiter, InteractionDecision, InteractionPayload, guarded_node};
use crate::{HandlerKind, Tree};
use octara_model::geometry::{Point, Rect};
use octara_model::identity::{NodeId, TargetGuard};
use octara_model::input::{DragPayload, DropAxis, DropEvent, DropHover, DropPosition, Key, KeyEvent, Modifiers, PointerId};
use octara_model::style::State;
use std::sync::Arc;

/// Current hit and geometry supplied by a host during pointer arbitration.
#[derive(Clone, Copy)]
pub struct PointerGeometry<'a> {
    pub hit: Option<TargetGuard>,
    pub bounds: &'a dyn Fn(NodeId) -> Option<Rect>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Receiver {
    target: TargetGuard,
    position: DropPosition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Grab {
    Pointer(PointerId),
    Keyboard,
}

pub(super) struct Transfer {
    source: TargetGuard,
    payload: DragPayload,
    grab: Grab,
    receiver: Option<Receiver>,
}

fn accepts(tree: &Tree, node: NodeId, kind: &str) -> bool {
    tree.handler(node, HandlerKind::Drop).is_some()
        && tree
            .drag_drop(node)
            .and_then(|slot| slot.target.as_ref())
            .is_some_and(|policy| policy.kinds.iter().any(|accepted| accepted.as_ref() == kind))
}

pub(super) fn transfer_is_current(tree: &Tree, target: NodeId, source: TargetGuard, kind: &str, data: &Arc<serde_json::Value>) -> bool {
    guarded_node(tree, source, tree.front_modal()).is_some_and(|source| {
        tree.drag_drop(source)
            .and_then(|slot| slot.source.as_ref())
            .is_some_and(|payload| payload.kind.as_ref() == kind && Arc::ptr_eq(&payload.data, data))
    }) && accepts(tree, target, kind)
}

pub(super) fn keyboard_position(tree: &Tree, source: TargetGuard, target: NodeId) -> Option<DropPosition> {
    let source = source.node()?;
    let parent = tree.physical_parent(source)?;
    if tree.physical_parent(target) != Some(parent) || target == source {
        return None;
    }
    let first = tree.children(parent).iter().find(|&&node| node == source || node == target)?;
    Some(if *first == source { DropPosition::After } else { DropPosition::Before })
}

fn position(bounds: Rect, axis: DropAxis, point: Point) -> DropPosition {
    let (distance, extent) = match axis {
        DropAxis::Horizontal => (point.x - bounds.x, bounds.w),
        DropAxis::Vertical => (point.y - bounds.y, bounds.h),
    };
    if extent <= 0.0 {
        DropPosition::Inside
    } else if distance < extent / 3.0 {
        DropPosition::Before
    } else if distance > 2.0 * extent / 3.0 {
        DropPosition::After
    } else {
        DropPosition::Inside
    }
}

impl Transfer {
    fn highlight(&mut self, tree: &Tree, decision: &mut InteractionDecision, receiver: Option<Receiver>) {
        if self.receiver == receiver {
            return;
        }
        let from = self.receiver.map(|entry| entry.target);
        let to = receiver.map(|entry| entry.target);
        if from != to
            && let Some(target) = from
        {
            decision.state(target.node().expect("node receiver"), State::DROP_HOVER, false);
            self.hover(tree, decision, target, None);
        }
        if let Some(receiver) = receiver {
            if from != to {
                decision.state(receiver.target.node().expect("node receiver"), State::DROP_HOVER, true);
            }
            self.hover(tree, decision, receiver.target, Some(receiver.position));
        }
        self.receiver = receiver;
    }

    fn hover(&self, tree: &Tree, decision: &mut InteractionDecision, target: TargetGuard, position: Option<DropPosition>) {
        decision.invoke(
            tree,
            target,
            HandlerKind::DropHover,
            InteractionPayload::DropHover {
                event: DropHover { source: self.source, payload: self.payload.clone(), position },
                keyboard: self.grab == Grab::Keyboard,
            },
        );
    }
}

impl InteractionArbiter {
    pub(super) fn grab(&mut self, tree: &Tree, decision: &mut InteractionDecision, source: NodeId, grab: Grab) -> bool {
        let Some(payload) = tree.drag_drop(source).and_then(|slot| slot.source.clone()) else { return false };
        if self.transfer.is_some() {
            return false;
        }
        self.transfer = Some(Transfer { source: tree.guard(source), payload, grab, receiver: None });
        decision.state(source, State::DRAGGING, true);
        true
    }

    pub(super) fn pointer_transfer(
        &mut self,
        tree: &Tree,
        decision: &mut InteractionDecision,
        pointer: PointerId,
        at: Point,
        geometry: Option<PointerGeometry<'_>>,
    ) {
        let Some(transfer) = self.transfer.as_mut().filter(|transfer| transfer.grab == Grab::Pointer(pointer)) else { return };
        let modal = tree.front_modal();
        let receiver = geometry.and_then(|geometry| {
            let hit = guarded_node(tree, geometry.hit?, modal)?;
            let node = std::iter::successors(Some(hit), |&node| tree.logical_parent(node)).find(|&node| {
                transfer.source.node() != Some(node) && tree.interactive_in(node, modal) && accepts(tree, node, &transfer.payload.kind)
            })?;
            let policy = tree.drag_drop(node)?.target.as_ref()?;
            let position = (geometry.bounds)(node).map_or(DropPosition::Inside, |bounds| position(bounds, policy.axis, at));
            Some(Receiver { target: tree.guard(node), position })
        });
        transfer.highlight(tree, decision, receiver);
    }

    pub(super) fn clear_receiver(&mut self, tree: &Tree, decision: &mut InteractionDecision) {
        if let Some(transfer) = self.transfer.as_mut().filter(|transfer| matches!(transfer.grab, Grab::Pointer(_))) {
            transfer.highlight(tree, decision, None);
        }
    }

    pub(super) fn end_transfer(&mut self, tree: &Tree, decision: &mut InteractionDecision, drop: Option<Modifiers>) {
        let Some(mut transfer) = self.transfer.take() else { return };
        decision.state(transfer.source.node().expect("node source"), State::DRAGGING, false);
        if let Some((receiver, modifiers)) = transfer.receiver.zip(drop) {
            let event = DropEvent {
                source: transfer.source,
                kind: transfer.payload.kind.clone(),
                data: transfer.payload.data.clone(),
                position: receiver.position,
                modifiers,
            };
            decision.invoke(
                tree,
                receiver.target,
                HandlerKind::Drop,
                InteractionPayload::Drop { event, keyboard: transfer.grab == Grab::Keyboard },
            );
        }
        transfer.highlight(tree, decision, None);
    }

    pub(super) fn validate_transfer(&mut self, tree: &Tree, decision: &mut InteractionDecision) {
        let Some(transfer) = self.transfer.as_mut() else { return };
        let modal = tree.front_modal();
        let source = guarded_node(tree, transfer.source, modal).and_then(|node| Some((node, tree.drag_drop(node)?)));
        if source.is_none_or(|(node, slot)| {
            slot.source.as_ref() != Some(&transfer.payload)
                || (transfer.grab == Grab::Keyboard && (!slot.keyboard || tree.focus() != Some(node)))
        }) {
            if self.contact.as_ref().is_some_and(|contact| matches!(contact.mode, super::ContactMode::Transferring(_))) {
                self.contact = None;
            }
            self.end_transfer(tree, decision, None);
        } else if let Some(receiver) = transfer.receiver {
            let next =
                guarded_node(tree, receiver.target, modal).filter(|&node| accepts(tree, node, &transfer.payload.kind)).and_then(|node| {
                    let position =
                        if transfer.grab == Grab::Keyboard { keyboard_position(tree, transfer.source, node)? } else { receiver.position };
                    Some(Receiver { position, ..receiver })
                });
            transfer.highlight(tree, decision, next);
        }
    }

    /// Give a drag its reserved keys before application shortcuts or layer dismissal.
    pub fn transfer_key(&mut self, tree: &Tree, key: KeyEvent) -> Option<InteractionDecision> {
        if !self.active || !key.modifiers.is_empty() {
            return None;
        }
        let mut decision = InteractionDecision::default();
        self.validate_transfer(tree, &mut decision);
        if key.key == Key::Escape && self.transfer.is_some() {
            self.contact = None;
            self.end_transfer(tree, &mut decision, None);
            return Some(decision);
        }
        let Some(transfer) = &mut self.transfer else {
            if matches!(key.key, Key::Space | Key::Enter)
                && let Some(source) = tree.focus().filter(|&node| tree.is_interactive(node) && tree.editor(node).is_none())
                && tree.drag_drop(source).is_some_and(|slot| slot.keyboard && slot.source.is_some())
                && self.contact.is_none()
            {
                if !key.repeat {
                    self.grab(tree, &mut decision, source, Grab::Keyboard);
                }
                return Some(decision);
            }
            return (!decision.operations.is_empty() || !decision.intents.is_empty()).then_some(decision);
        };
        if transfer.grab != Grab::Keyboard {
            return None;
        }
        match key.key {
            Key::Space | Key::Enter => {
                if !key.repeat {
                    self.end_transfer(tree, &mut decision, Some(key.modifiers));
                }
            }
            Key::Left | Key::Up | Key::Right | Key::Down => {
                let forward = matches!(key.key, Key::Right | Key::Down);
                if let Some(source) = transfer.source.node()
                    && let Some(parent) = tree.physical_parent(source)
                {
                    let rows = tree.children(parent);
                    let current = transfer.receiver.and_then(|receiver| receiver.target.node()).unwrap_or(source);
                    if let Some(source_index) = rows.iter().position(|&node| node == source)
                        && let Some(index) = rows.iter().position(|&node| node == current)
                    {
                        let modal = tree.front_modal();
                        let eligible = |&(_, &node): &(usize, &NodeId)| {
                            node == source || (tree.interactive_in(node, modal) && accepts(tree, node, &transfer.payload.kind))
                        };
                        let rows = rows.iter().enumerate();
                        let next = if forward { rows.skip(index + 1).find(eligible) } else { rows.take(index).rev().find(eligible) };
                        if let Some((next_index, &next)) = next {
                            let receiver = (next != source).then(|| Receiver {
                                target: tree.guard(next),
                                position: if next_index > source_index { DropPosition::After } else { DropPosition::Before },
                            });
                            transfer.highlight(tree, &mut decision, receiver);
                        }
                    }
                }
            }
            _ => return None,
        }
        Some(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        HandlerId, NodeChange, NodeContent, NodeSpec, TreeId,
        arbiter::handler_intent,
        tables::{DragDrop, DropTarget},
    };

    #[test]
    fn queued_hover_entry_rechecks_source_and_current_keyboard_group() {
        let mut tree = Tree::new(TreeId::new([3, 8], 1), Default::default());
        let mut tx = tree.transaction();
        let first = tx.create(NodeSpec::new(NodeContent::Box));
        let second = tx.create(NodeSpec::new(NodeContent::Box));
        let source = tx.create(NodeSpec::new(NodeContent::Box));
        let target = tx.create(NodeSpec::new(NodeContent::Box));
        tx.place(tree.node_ref(tree.root()), 0, first);
        tx.place(tree.node_ref(tree.root()), 1, second);
        tx.place(first, 0, source);
        tx.place(first, 1, target);
        let data = DragPayload::new("row", 6);
        tx.change(source, NodeChange::DragDrop(Some(Box::new(DragDrop { source: Some(data.clone()), ..Default::default() }))));
        tx.change(
            target,
            NodeChange::DragDrop(Some(Box::new(DragDrop {
                target: Some(DropTarget { kinds: Arc::from([Arc::from("row")]), axis: DropAxis::Vertical }),
                ..Default::default()
            }))),
        );
        tx.change(target, NodeChange::Handler { kind: HandlerKind::Drop, id: Some(HandlerId(1)) });
        tx.change(target, NodeChange::Handler { kind: HandlerKind::DropHover, id: Some(HandlerId(2)) });
        let (receipt, _) = tree.apply(tx.finish()).unwrap();
        let source = receipt.node(source).unwrap();
        let target = receipt.node(target).unwrap();
        let first = receipt.node(first).unwrap();
        let second = receipt.node(second).unwrap();
        let payload = |position| InteractionPayload::DropHover {
            event: DropHover { source: tree.guard(source), payload: data.clone(), position },
            keyboard: true,
        };
        let entry = handler_intent(&tree, tree.guard(target), HandlerKind::DropHover, payload(Some(DropPosition::After))).unwrap();
        let leave = handler_intent(&tree, tree.guard(target), HandlerKind::DropHover, payload(None)).unwrap();
        assert!(entry.resolve(&tree).is_some());

        let mut tx = tree.transaction();
        tx.place(tree.node_ref(second), 0, tree.node_ref(target));
        tree.apply(tx.finish()).unwrap();
        assert!(entry.resolve(&tree).is_none());
        assert!(leave.resolve(&tree).is_some());

        let mut tx = tree.transaction();
        tx.place(tree.node_ref(first), 1, tree.node_ref(target));
        tx.change(tree.node_ref(source), NodeChange::Disabled(true));
        tree.apply(tx.finish()).unwrap();
        assert!(entry.resolve(&tree).is_none());
        assert!(leave.resolve(&tree).is_some());
    }
}
