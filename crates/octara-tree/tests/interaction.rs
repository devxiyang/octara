use std::time::Duration;

use octara_model::{
    geometry::{Placement, Point, Vec2},
    input::{Button, ContactTarget, InteractionEvent, Key, KeyEvent, Modifiers, PointerId, PointerKind},
    style::{Overflow, State, StyleProp},
    theme::Theme,
};
use octara_tree::{
    NodeChange, NodeContent, NodeFlags, NodeId, NodeSpec, Retirement, RuntimeOp, TargetGuard, Tree, TreeId,
    arbiter::{
        InteractionArbiter, InteractionContext, InteractionDecision, InteractionIntent, TouchScrollIntent, TouchScrollPhase,
        TouchScrollStage,
    },
    editor::EditorSlot,
    focus::{Arrow, FocusGroup},
    overlay::LayerPolicy,
    tables::{AuthoredStyle, HandlerId, HandlerKind, SemanticsSlot},
    transaction::OverlaySpec,
};

fn tree(window: u64) -> Tree {
    Tree::new(TreeId::new([1, 2], window), Theme::default())
}

fn target(tree: &Tree, node: NodeId) -> ContactTarget {
    ContactTarget { hit: tree.guard(node), action: Some(tree.guard(node)) }
}

fn down(contact: u64, kind: PointerKind, target: ContactTarget, pos: Point) -> InteractionEvent {
    InteractionEvent::PointerDown { id: PointerId(contact), kind, target, pos, button: Button::Primary, mods: Modifiers::empty() }
}

fn up(contact: u64, target: ContactTarget, pos: Point) -> InteractionEvent {
    InteractionEvent::PointerUp { id: PointerId(contact), target: Some(target), pos, mods: Modifiers::empty() }
}

fn effects(tree: &mut Tree, decision: InteractionDecision) -> Vec<InteractionIntent> {
    for operation in decision.operations {
        tree.apply_runtime(operation);
    }
    decision.intents
}

fn add_press(tree: &mut Tree) -> NodeId {
    let mut tx = tree.transaction();
    let button = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(button, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(button, NodeChange::Handler { kind: HandlerKind::Press, id: Some(HandlerId(1)) });
    tx.place(tree.node_ref(tree.root()), 0, button);
    tree.apply(tx.finish()).unwrap().0.node(button).unwrap()
}

#[test]
fn foreign_guards_window_deactivation_and_retirement_cancel_contact() {
    let mut tree = tree(1);
    let node = add_press(&mut tree);
    let target = target(&tree, node);
    let foreign = TargetGuard::for_node(TreeId::new([1, 2], 2), node);
    let mut arbiter = InteractionArbiter::default();
    let context = InteractionContext::default();
    let position = Point::new(10.0, 10.0);
    for forged in [ContactTarget { hit: foreign, ..target }, ContactTarget { action: Some(foreign), ..target }] {
        let decision = arbiter.handle(&tree, down(1, PointerKind::Mouse, forged, position), context);
        assert!(decision.operations.is_empty() && decision.intents.is_empty());
    }

    let decision = arbiter.handle(&tree, down(1, PointerKind::Mouse, target, position), context);
    effects(&mut tree, decision);
    assert!(tree.node(node).unwrap().state.contains(State::PRESSED));
    let wrong_contact = arbiter.handle(&tree, up(2, target, position), context);
    assert!(wrong_contact.operations.is_empty() && wrong_contact.intents.is_empty());

    let inactive = arbiter.handle(&tree, InteractionEvent::WindowActive(false), context);
    effects(&mut tree, inactive);
    assert!(!tree.node(node).unwrap().state.contains(State::PRESSED));
    assert!(arbiter.handle(&tree, up(1, target, position), context).intents.is_empty());

    arbiter.handle(&tree, InteractionEvent::WindowActive(true), context);
    let second = arbiter.handle(&tree, down(3, PointerKind::Mouse, target, position), context);
    effects(&mut tree, second);
    let mut retire = tree.transaction();
    retire.retire(tree.node_ref(node), Retirement::Exit);
    tree.apply(retire.finish()).unwrap();

    let cancelled = arbiter.reconcile(&tree, Duration::ZERO);
    assert!(
        matches!(cancelled.operations.as_slice(), [RuntimeOp::SetState { node: released, state: State::PRESSED, on: false }] if *released == node)
    );
    assert!(arbiter.handle(&tree, up(3, target, position), context).intents.is_empty());
}

#[test]
fn touch_dismissal_and_editor_handoff_settle_before_activation() {
    let mut tree = tree(1);
    let button = add_press(&mut tree);
    let mut tx = tree.transaction();
    let popover = tx.create(NodeSpec::new(NodeContent::Box));
    let editor = tx.create(NodeSpec::new(NodeContent::TextInput(Box::default())));
    tx.change(editor, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(popover, NodeChange::Overlay(OverlaySpec { layer: Some(LayerPolicy::non_modal()), ..OverlaySpec::default() }));
    tx.change(popover, NodeChange::Handler { kind: HandlerKind::Dismiss, id: Some(HandlerId(2)) });
    tx.place(tree.node_ref(tree.overlay_root()), 0, popover);
    tx.place(popover, 0, editor);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let popover = receipt.node(popover).unwrap();
    let editor = receipt.node(editor).unwrap();
    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(editor), visible: true });

    let mut arbiter = InteractionArbiter::default();
    let context = InteractionContext { keyboard_open: true, ..InteractionContext::default() };
    let contact = target(&tree, button);
    let pos = Point::new(20.0, 20.0);
    let decision = arbiter.handle(&tree, down(1, PointerKind::Touch, contact, pos), context);
    assert!(effects(&mut tree, decision).is_empty());
    assert_eq!(tree.focus(), Some(editor));

    let decision = arbiter.handle(&tree, up(1, contact, pos), context);
    let mut intents = effects(&mut tree, decision).into_iter();
    let Some(InteractionIntent::Focus(focus)) = intents.next() else { panic!("expected focus handoff") };
    let Some(InteractionIntent::Invoke(dismiss)) = intents.next() else { panic!("expected dismissal") };
    assert_eq!(dismiss.target, tree.guard(popover));
    assert_eq!(dismiss.resolve(&tree), Some(HandlerId(2)));
    let Some(InteractionIntent::Activate(activation)) = intents.next() else { panic!("expected deferred activation") };
    assert!(intents.next().is_none());

    let mut tx = tree.transaction();
    tx.retire(tree.node_ref(popover), Retirement::Destroy);
    tx.retire(tree.node_ref(button), Retirement::Destroy);
    tree.apply(tx.finish()).unwrap();
    let replacement = add_press(&mut tree);
    assert_ne!(replacement, button);
    assert!(focus.resolve(&tree).is_none());
    assert!(arbiter.resolve_activation(&tree, activation).is_empty());
}

#[test]
fn escape_and_outside_press_choose_the_top_eligible_layer() {
    let mut tree = tree(1);
    let outside = add_press(&mut tree);
    let mut tx = tree.transaction();
    let eligible = tx.create(NodeSpec::new(NodeContent::Box));
    let ineligible = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(eligible, NodeChange::Handler { kind: HandlerKind::Dismiss, id: Some(HandlerId(2)) });
    tx.change(ineligible, NodeChange::Handler { kind: HandlerKind::Dismiss, id: Some(HandlerId(3)) });
    for (index, node, layer) in [
        (0, eligible, LayerPolicy::non_modal()),
        (1, ineligible, LayerPolicy { dismiss_on_escape: false, dismiss_on_outside_press: false, ..LayerPolicy::non_modal() }),
    ] {
        tx.change(node, NodeChange::Overlay(OverlaySpec { layer: Some(layer), ..Default::default() }));
        tx.place(tree.node_ref(tree.overlay_root()), index, node);
    }
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let eligible = receipt.node(eligible).unwrap();
    let mut arbiter = InteractionArbiter::default();
    for event in [
        InteractionEvent::Key(KeyEvent::new(Key::Escape, Modifiers::empty())),
        down(1, PointerKind::Mouse, target(&tree, outside), Point::default()),
    ] {
        let decision = arbiter.handle(&tree, event, InteractionContext::default());
        let dismissals: Vec<_> = decision
            .intents
            .into_iter()
            .filter_map(|intent| match intent {
                InteractionIntent::Invoke(intent) if intent.kind == HandlerKind::Dismiss => Some(intent.target),
                _ => None,
            })
            .collect();
        assert_eq!(dismissals, [tree.guard(eligible)]);
    }
}

#[test]
fn touch_scroll_preserves_signed_delta_and_suppresses_press() {
    let mut tree = tree(1);
    let mut tx = tree.transaction();
    let scroller = tx.create(NodeSpec::new(NodeContent::Box));
    let button = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        scroller,
        NodeChange::Style(Box::new(AuthoredStyle {
            base: [StyleProp::OverflowY(Overflow::Scroll)].into_iter().collect(),
            ..AuthoredStyle::default()
        })),
    );
    tx.change(button, NodeChange::Handler { kind: HandlerKind::Press, id: Some(HandlerId(1)) });
    tx.change(scroller, NodeChange::Handler { kind: HandlerKind::TouchScrollPre, id: Some(HandlerId(2)) });
    tx.change(scroller, NodeChange::Handler { kind: HandlerKind::TouchScrollPost, id: Some(HandlerId(3)) });
    tx.place(tree.node_ref(tree.root()), 0, scroller);
    tx.place(scroller, 0, button);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let (scroller, button) = (receipt.node(scroller).unwrap(), receipt.node(button).unwrap());
    let contact = target(&tree, button);
    let mut arbiter = InteractionArbiter::default();
    let context = InteractionContext::default();

    let decision = arbiter.handle(&tree, down(1, PointerKind::Touch, contact, Point::new(0.0, 40.0)), context);
    effects(&mut tree, decision);
    let decision = arbiter.handle(
        &tree,
        InteractionEvent::PointerMove { id: PointerId(1), pos: Point::new(2.0, 14.0), mods: Modifiers::empty() },
        context,
    );
    let intents = effects(&mut tree, decision);
    let [InteractionIntent::TouchScroll(scroll)] = intents.as_slice() else { panic!("expected captured scroll") };
    assert!(!tree.node(button).unwrap().state.contains(State::PRESSED));
    assert_eq!(scroll.target, tree.guard(scroller));
    assert_eq!(scroll.phase, TouchScrollPhase::Start);
    assert_eq!(scroll.delta, Vec2::new(-2.0, 26.0));

    assert_eq!(scroll.handler(&tree, TouchScrollStage::Pre, scroll.delta).unwrap().resolve(&tree), Some(HandlerId(2)));
    let remaining = TouchScrollIntent::remaining(scroll.delta, Vec2::new(3.0, 12.0));
    assert_eq!(scroll.viewport_delta(&tree, remaining), Some(Vec2::new(0.0, 14.0)));
    let remaining = TouchScrollIntent::remaining(remaining, Vec2::new(0.0, 11.0));
    assert_eq!(remaining, Vec2::new(-2.0, 3.0));
    assert_eq!(scroll.handler(&tree, TouchScrollStage::Post, remaining).unwrap().resolve(&tree), Some(HandlerId(3)));

    let decision = arbiter.handle(&tree, up(1, contact, Point::new(2.0, 10.0)), context);
    let [InteractionIntent::TouchScroll(last), InteractionIntent::TouchScroll(end)] = decision.intents.as_slice() else {
        panic!("expected final displacement and scroll completion")
    };
    assert_eq!(last.phase, TouchScrollPhase::Move);
    assert_eq!(last.delta, Vec2::new(0.0, 4.0));
    assert_eq!(end.phase, TouchScrollPhase::End);
}

fn open_modal(tree: &mut Tree, order: u64) -> (NodeId, NodeId) {
    let mut tx = tree.transaction();
    let modal = tx.create(NodeSpec::new(NodeContent::Box));
    let focus = tx.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("draft")))));
    tx.change(focus, NodeChange::Flags(NodeFlags::FOCUSABLE | NodeFlags::AUTOFOCUS));
    tx.change(
        modal,
        NodeChange::Overlay(OverlaySpec {
            layer: Some(LayerPolicy::modal()),
            mount: Some((tree.node_ref(tree.root()), order)),
            ..OverlaySpec::default()
        }),
    );
    tx.change(modal, NodeChange::Handler { kind: HandlerKind::Dismiss, id: Some(HandlerId(order)) });
    tx.place(tree.node_ref(tree.overlay_root()), tree.children(tree.overlay_root()).len(), modal);
    tx.place(modal, 0, focus);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    (receipt.node(modal).unwrap(), receipt.node(focus).unwrap())
}

#[test]
fn modal_focus_traps_in_paint_order_and_restores_live_opener() {
    let mut tree = tree(1);
    let opener = add_press(&mut tree);
    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(opener), visible: true });
    let (lower, lower_field) = open_modal(&mut tree, 10);
    let (upper, upper_field) = open_modal(&mut tree, 20);
    assert_eq!(tree.modal_layers().collect::<Vec<_>>(), [lower, upper]);
    assert_eq!(tree.focus(), Some(upper_field));
    assert_eq!(tree.tab_target(false), Some(upper_field));

    let mut tx = tree.transaction();
    let popup = tx.create(NodeSpec::new(NodeContent::Box));
    let owned = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(owned, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(
        popup,
        NodeChange::Overlay(OverlaySpec {
            target: Some((octara_model::overlay::AnchorTarget::Node(tree.node_ref(upper_field)), Placement::BELOW)),
            mount: Some((tree.node_ref(upper_field), 30)),
            layer: Some(LayerPolicy::non_modal()),
            ..OverlaySpec::default()
        }),
    );
    tx.place(tree.node_ref(tree.overlay_root()), 2, popup);
    tx.place(popup, 0, owned);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let (popup, owned) = (receipt.node(popup).unwrap(), receipt.node(owned).unwrap());
    assert_eq!(tree.focus_order(Some(upper), tree.focus()), [upper_field, owned]);
    assert_eq!(tree.tab_target(true), Some(owned));

    let scope = tree.semantic_scope();
    assert_eq!(scope.modal, Some(upper));
    assert!(scope.exposes(upper_field) && scope.exposes(owned));
    assert!(scope.includes(tree.overlay_root()) && !scope.exposes(tree.overlay_root()));
    assert!(!scope.includes(lower_field) && !scope.includes(opener));

    let mut retire = tree.transaction();
    retire.retire(tree.node_ref(popup), Retirement::Destroy);
    retire.retire(tree.node_ref(upper), Retirement::Exit);
    tree.apply(retire.finish()).unwrap();
    assert_eq!(tree.front_modal(), Some(lower));
    assert_eq!(tree.focus(), Some(lower_field));
    let mut retire = tree.transaction();
    retire.retire(tree.node_ref(lower), Retirement::Destroy);
    tree.apply(retire.finish()).unwrap();
    assert_eq!(tree.focus(), Some(opener));
}

#[test]
fn focus_group_roves_without_reordering_positive_tab_stops() {
    let mut tree = tree(1);
    let outside = add_press(&mut tree);
    let mut tx = tree.transaction();
    let group = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(
        group,
        NodeChange::Semantics(Box::new(SemanticsSlot { focus_group: Some(FocusGroup::horizontal().wrap()), ..SemanticsSlot::default() })),
    );
    let first = tx.create(NodeSpec::new(NodeContent::Box));
    let preferred = tx.create(NodeSpec::new(NodeContent::Box));
    tx.change(first, NodeChange::Flags(NodeFlags::FOCUSABLE));
    tx.change(preferred, NodeChange::Flags(NodeFlags::FOCUSABLE | NodeFlags::FOCUS_PREFERRED));
    tx.change(preferred, NodeChange::Semantics(Box::new(SemanticsSlot { tab_index: i32::MAX, ..SemanticsSlot::default() })));
    tx.place(tree.node_ref(tree.root()), 1, group);
    tx.place(group, 0, first);
    tx.place(group, 1, preferred);
    let (receipt, _) = tree.apply(tx.finish()).unwrap();
    let (first, preferred) = (receipt.node(first).unwrap(), receipt.node(preferred).unwrap());
    assert_eq!(tree.focus_order(None, None), [preferred, outside]);
    assert_eq!(tree.group_step(preferred, Arrow::Right), Some(first));
    assert_eq!(tree.group_step(first, Arrow::Down), None);
    tree.apply_runtime(RuntimeOp::SetFocus { node: Some(first), visible: true });
    assert_eq!(tree.focus_order(None, tree.focus()), [outside, first]);
}
