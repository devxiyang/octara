use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, Ordering};

use octara_model::diagnostics::{Code, codes};

use super::{MAX_NODE_INDEX, MAX_RUNTIME, NodeKind, ReactiveId, ReactiveWorld, Revision, Source, next_runtime};

#[test]
fn packed_id_preserves_namespace_index_and_generation() {
    let id = ReactiveId::new(MAX_RUNTIME, MAX_NODE_INDEX, u16::MAX);
    assert_eq!(id.runtime(), MAX_RUNTIME);
    assert_eq!(id.node(), MAX_NODE_INDEX);
    assert_eq!(id.generation(), u16::MAX);
    assert!(ReactiveId::new(MAX_RUNTIME - 1, MAX_NODE_INDEX, u16::MAX) < ReactiveId::new(MAX_RUNTIME, 0, 0));
    assert!(ReactiveId::new(MAX_RUNTIME, MAX_NODE_INDEX - 1, u16::MAX) < ReactiveId::new(MAX_RUNTIME, MAX_NODE_INDEX, 0));
}

#[test]
fn runtime_ids_exhaust_without_wrapping() {
    let counter = AtomicU32::new(MAX_RUNTIME);
    assert_eq!(next_runtime(&counter), MAX_RUNTIME);
    assert!(catch_unwind(|| next_runtime(&counter)).is_err());
    assert_eq!(counter.load(Ordering::Relaxed), MAX_RUNTIME + 1);
}

#[test]
fn reused_slots_and_matching_slots_in_other_worlds_are_isolated() {
    fn rejects(code: &Code, operation: impl FnOnce()) {
        let panic = catch_unwind(AssertUnwindSafe(operation)).unwrap_err();
        let message = panic.downcast_ref::<String>().unwrap();
        assert!(message.contains(&code.to_string()), "expected {code} in {message}");
    }

    let mut first = ReactiveWorld::new();
    let first_scope = first.root_scope();
    let stale = first.alloc_value(NodeKind::ValueSource, first_scope, None, 1);
    first.retire_node(stale);
    let live = first.alloc_value(NodeKind::ValueSource, first_scope, None, 2);

    let mut second = ReactiveWorld::new();
    let second_scope = second.root_scope();
    let foreign_stale = second.alloc_value(NodeKind::ValueSource, second_scope, None, 3);
    second.retire_node(foreign_stale);
    let foreign = second.alloc_value(NodeKind::ValueSource, second_scope, None, 4);

    assert_eq!(stale.node(), live.node());
    assert_eq!(foreign.node(), live.node());
    assert_eq!(foreign.generation(), live.generation());
    assert_ne!(foreign, live);
    assert!(!first.is_alive(stale) && !first.is_alive(foreign) && !second.is_alive(live));
    rejects(&codes::READ_DISPOSED, || {
        first.value::<i32>(stale);
    });
    rejects(&codes::WRITE_DISPOSED, || *first.value_mut::<i32>(stale) = 5);
    rejects(&codes::WRONG_RUNTIME, || {
        first.value::<i32>(foreign);
    });
    rejects(&codes::WRONG_RUNTIME, || *first.value_mut::<i32>(foreign_stale) = 5);
    first.retire_node(foreign);
    assert_eq!(*first.value::<i32>(live), 2);
    assert_eq!(*second.value::<i32>(foreign), 4);
    assert!(first.validate_integrity() && second.validate_integrity());
}

#[test]
fn exhausted_node_slots_never_alias_old_handles() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let first = world.alloc_node(NodeKind::ValueSource, scope, None);
    world.retire_node(first);
    for generation in 1..=u16::MAX {
        let id = world.alloc_node(NodeKind::ValueSource, scope, None);
        assert_eq!(id.node(), first.node());
        assert_eq!(id.generation(), generation);
        world.retire_node(id);
    }
    let next = world.alloc_node(NodeKind::ValueSource, scope, None);
    assert_ne!(next.node(), first.node());
    assert_eq!(next.generation(), 0);
    assert!(!world.is_alive(first));
    assert!(world.is_alive(next) && world.validate_integrity());
}

#[test]
fn source_preserves_the_fixed_flag_at_maximum_revision() {
    let id = ReactiveId::new(1, 0, 0);
    let revision = Revision(Revision::MAX - 1).next();
    let mut source = Source::new(id, revision, false);
    assert_eq!(source.revision(), revision);
    assert!(!source.fixed());
    source.set_fixed();
    assert!(source.fixed());
    assert_eq!(source.revision(), revision);
    assert!(catch_unwind(|| revision.next()).is_err());
}

#[test]
fn trigger_owns_only_a_graph_node() {
    let mut world = ReactiveWorld::new();
    let scope = world.new_scope(world.root_scope());
    let trigger = world.build(scope, None).trigger();
    assert_eq!(world.hot(trigger.id()).value_bucket(), None);
    assert_eq!(world.live_nodes(), 1);
    world.dispose(scope);
    assert_eq!(world.live_nodes(), 0);
}
