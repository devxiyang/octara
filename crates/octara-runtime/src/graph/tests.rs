use std::cell::{Cell, RefCell};
use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use octara_model::diagnostics::codes;

use super::*;
use crate::graph::value::{Lens, ReadSignal, Signal, Watch};

#[test]
fn handle_size_copy_and_nonclone_reads() {
    struct Value(i32);
    fn copy<T: Copy>(value: T) -> (T, T) {
        (value, value)
    }

    assert_eq!(size_of::<Signal<Value>>(), 16);
    assert_eq!(size_of::<ReadSignal<Value>>(), 16);
    assert_eq!(size_of::<Watch>(), 16);
    assert_eq!(size_of::<ScopeId>(), 16);
    let mut world = ReactiveWorld::new();
    let (a, b) = copy(world.build(world.root_scope(), None).signal(Value(9)));
    assert_eq!(world.read().with2(a, b.read_only(), |a, b| a.0 + b.0), 18);
}

#[test]
fn diamond_is_fresh_and_only_recomputes_when_outputs_change() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let input = cx.signal(1);
    let runs = Rc::new(Cell::new(0));
    let left = cx.memo(move |r| r.get(input) % 2);
    let right = cx.memo(move |r| r.get(input) % 2 + 10);
    let total = cx.memo({
        let runs = runs.clone();
        move |r| {
            runs.set(runs.get() + 1);
            r.with2(left, right, |left, right| left + right)
        }
    });
    assert_eq!(world.read().get(total), 12);
    let mut w = world.write(scope, None);
    w.set(input, 3);
    w.set(input, 5);
    assert_eq!(w.get(total), 12);
    assert_eq!(runs.get(), 1);
    w.set(input, 2);
    assert_eq!(w.get(total), 10);
    assert_eq!(runs.get(), 2);
    assert!(!world.has_pending_work());
}

#[test]
fn dynamic_dependencies_are_deduplicated_replaced_and_depth_propagates() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let select = cx.signal(false);
    let shallow = cx.signal(1);
    let deeper = cx.signal(5);
    let intermediate = cx.memo(move |r| r.get(deeper) + 1);
    let deep = cx.memo(move |r| r.get(intermediate) + 1);
    let choose = cx.memo(move |r| {
        if r.get(select) {
            let before = r.get(deeper);
            before + r.get(deep) + r.get(deeper)
        } else {
            r.get(shallow)
        }
    });
    let downstream = cx.memo(move |r| 2 * r.get(choose));
    assert_eq!(world.read().get(downstream), 2);
    assert_eq!(world.hot(downstream.id()).depth, 2);
    world.write(scope, None).set(select, true);
    assert_eq!(world.read().get(downstream), 34);
    assert_eq!(world.hot(downstream.id()).depth, 4);
    assert_eq!(world.cold(choose.id()).sources.len(), 3);
    world.write(scope, None).set(shallow, -99);
    assert!(!world.has_pending_work());
    world.write(scope, None).set(select, false);
    assert_eq!(world.read().get(downstream), -198);
    assert_eq!(world.hot(downstream.id()).depth, 2);
}

#[test]
fn observer_tiers_and_post_commit_watches_follow_revisions() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let source = cx.signal(0);
    let result = cx.signal(0);
    let parity = cx.memo(move |r| r.get(source) % 2);
    let effects = Rc::new(RefCell::new(Vec::new()));
    cx.watch(move |r| r.get(parity), {
        let effects = effects.clone();
        move |w, value, before| {
            effects.borrow_mut().push((*value, before.copied()));
            w.set(result, *value + 10);
        }
    });
    let binding = cx.create_observer(NodeKind::Binding);
    let structure = cx.create_observer(NodeKind::Structure);
    world.flush_derivations();
    assert_eq!(world.next_observer(FlushTier::Structure), Some(structure));
    assert_eq!(world.evaluate_observer(structure, |r| r.get(source)), Some(0));
    assert_eq!(world.next_observer(FlushTier::Binding), Some(binding));
    assert_eq!(world.evaluate_observer(binding, |r| r.get(parity)), Some(0));
    world.flush_watches();
    assert!(effects.borrow().is_empty());
    assert_eq!(world.read().get(result), 0);
    world.run_watch_effects();
    assert_eq!(effects.borrow().as_slice(), &[(0, None)]);
    assert_eq!(world.read().get(result), 10);

    world.write(scope, None).set(source, 2);
    assert_eq!(world.next_observer(FlushTier::Structure), Some(structure));
    assert_eq!(world.evaluate_observer(structure, |r| r.get(source)), Some(2));
    assert_eq!(world.next_observer(FlushTier::Binding), Some(binding));
    assert_eq!(world.evaluate_observer(binding, |r| r.get(parity)), None);
    world.flush_watches();
    world.run_watch_effects();
    assert_eq!(effects.borrow().as_slice(), &[(0, None)]);

    world.write(scope, None).set(source, 3);
    world.flush_watches();
    assert_eq!(world.read().get(result), 10);
    world.run_watch_effects();
    assert_eq!(effects.borrow().as_slice(), &[(0, None), (1, Some(0))]);
    assert_eq!(world.read().get(result), 11);
}

#[test]
fn equal_depth_observers_stay_ordered_across_pulls_and_slot_reuse() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let source = world.build(scope, None).signal(0);
    let [first, second, third, fourth, fifth, sixth] = std::array::from_fn(|_| world.create_observer(NodeKind::Binding, scope, None));

    for id in [fourth, second, fifth, first, sixth, third] {
        assert_eq!(world.evaluate_observer(id, |r| r.get(source)), Some(0));
    }
    assert!(!world.has_pending_work());
    world.write(scope, None).set(source, 1);
    world.invalidate_observer(first);
    assert_eq!(world.next_observer(FlushTier::Binding), Some(first));
    assert_eq!(world.evaluate_observer(first, |r| r.get(source)), Some(1));
    assert_eq!(world.evaluate_observer(fifth, |r| r.get(source)), Some(1));
    world.retire_node(third);

    let replacement = world.create_observer(NodeKind::Binding, scope, None);
    assert_eq!(replacement.node, third.node);
    assert_ne!(replacement, third);
    assert_eq!(world.next_observer(FlushTier::Binding), Some(replacement));
    assert_eq!(world.evaluate_observer(replacement, |r| r.get(source)), Some(1));
    for id in [second, fourth, sixth] {
        assert_eq!(world.next_observer(FlushTier::Binding), Some(id));
        assert_eq!(world.evaluate_observer(id, |r| r.get(source)), Some(1));
    }
    assert_eq!(world.next_observer(FlushTier::Binding), None);
    assert!(!world.has_pending_work());
    assert!(world.validate_integrity());
}

#[test]
fn dirty_observer_moves_between_mask_and_overflow_depths() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let source = cx.signal(0);
    let deep = cx.signal(false);
    let mut chain = vec![source.read_only()];
    for _ in 0..64 {
        let upstream = *chain.last().unwrap();
        chain.push(cx.memo(move |r| r.get(upstream) + 1));
    }
    let at_63 = chain[63];
    let switch = cx.memo(move |r| if r.get(deep) { r.get(at_63) } else { r.get(source) });
    let sources = [switch, chain[64], chain[63], chain[62]];
    let bindings = sources.map(|source| {
        let id = cx.create_observer(NodeKind::Binding);
        (id, source)
    });
    for (id, source) in bindings {
        world.evaluate_observer(id, |r| r.get(source));
    }
    let (moving, _) = bindings[0];
    assert_eq!(world.hot(moving).depth, 2);

    for (enabled, position, order) in [(true, 65, [3, 2, 0, 1]), (false, 2, [0, 3, 2, 1])] {
        let mut write = world.write(scope, None);
        write.set(deep, enabled);
        write.set(source, if enabled { 1 } else { 2 });
        world.flush_derivations();
        assert_eq!(world.hot(moving).depth, position);
        for index in order {
            let (id, source) = bindings[index];
            assert_eq!(world.next_observer(FlushTier::Binding), Some(id));
            assert!(world.evaluate_observer(id, |r| r.get(source)).is_some());
        }
        assert_eq!(world.next_observer(FlushTier::Binding), None);
        assert!(!world.has_pending_work());
        assert!(world.validate_integrity());
    }
}

#[test]
fn watch_source_and_effect_keep_mutable_state_between_runs() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let source = cx.signal(2);
    let result = cx.signal(Vec::new());
    let mut samples = 0;
    let mut history = Vec::new();
    cx.watch(
        move |read| {
            samples += 1;
            samples * read.get(source)
        },
        move |write, current, previous| {
            history.push((*current, previous.copied()));
            write.set(result, history.clone());
        },
    );

    world.flush_watches();
    world.run_watch_effects();
    world.write(scope, None).set(source, 3);
    world.flush_watches();
    world.run_watch_effects();
    assert_eq!(world.read().get(result), [(2, None), (6, Some(2))]);
}

#[test]
fn untracked_reads_and_field_slices_are_fine_grained() {
    struct Shape {
        width: i32,
        height: i32,
    }
    let width = Lens::new(|shape: &Shape| &shape.width, |shape| &mut shape.width);
    let height = Lens::new(|shape: &Shape| &shape.height, |shape| &mut shape.height);
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let shape = cx.signal(Shape { width: 2, height: 3 });
    let sampled = cx.signal(2);
    let narrow = cx.slice(shape, width);
    let scaled = cx.memo(move |r| r.get(narrow) * r.untrack(|r| r.get(sampled)));
    assert_eq!(world.read().get(scaled), 4);
    let mut w = world.write(scope, None);
    w.set(sampled, 4);
    w.set_slice(shape, height, 10);
    assert_eq!(w.get(scaled), 4);
    w.update_slice(shape, width, |value| *value += 1);
    assert_eq!(w.get(scaled), 12);
}

#[test]
fn cycle_reports_its_definition_and_unwinds_the_evaluation_stack() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let enter_cycle = cx.signal(true);
    let handle = Rc::new(Cell::new(None));
    let cyclic = cx.memo({
        let handle = handle.clone();
        move |r| if r.get(enter_cycle) { r.get(handle.get().unwrap()) } else { 42 }
    });
    handle.set(Some(cyclic));
    assert!(catch_unwind(AssertUnwindSafe(|| world.read().get(cyclic))).is_err());
    let diagnostics = world.take_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, &codes::CYCLE);
    assert!(diagnostics[0].location.is_some());
    let mut w = world.write(scope, None);
    w.set(enter_cycle, false);
    assert_eq!(w.get(cyclic), 42);
}

#[test]
fn stale_slots_and_foreign_worlds_are_rejected() {
    let mut world = ReactiveWorld::new();
    let parent = world.root_scope();
    let old_scope = world.new_scope(parent);
    let stale = world.build(old_scope, None).signal(1);
    world.dispose(old_scope);
    let fresh_scope = world.new_scope(parent);
    let fresh = world.build(fresh_scope, None).signal(2);
    assert_ne!(old_scope, fresh_scope);
    assert_eq!(stale.id().node, fresh.id().node);
    assert_ne!(stale, fresh);
    assert!(catch_unwind(AssertUnwindSafe(|| world.read().get(stale))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| world.write(fresh_scope, None).set(stale, 3))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| ReactiveWorld::new().read().get(fresh))).is_err());
    assert_eq!(world.read().get(fresh), 2);
}

#[test]
fn scopes_shadow_context_and_resume_cleanup_after_a_panic() {
    let history = Rc::new(RefCell::new(Vec::new()));
    let mut world = ReactiveWorld::new();
    let root = world.root_scope();
    let parent = world.new_scope(root);
    let child = world.new_scope(parent);
    let mut cx = world.build(parent, None);
    cx.provide(10u8);
    cx.on_cleanup({
        let history = history.clone();
        move |_| history.borrow_mut().push(1)
    });
    let mut cx = world.build(child, None);
    assert_eq!(cx.context::<u8>(), Some(10));
    cx.provide(20u8);
    assert_eq!(cx.context::<u8>(), Some(20));
    let local = cx.signal(2);
    cx.on_cleanup({
        let history = history.clone();
        move |cx| history.borrow_mut().push(cx.sample(local))
    });
    cx.on_cleanup(|_| panic!("cleanup failed"));

    assert!(catch_unwind(AssertUnwindSafe(|| world.dispose(parent))).is_err());
    assert!(!world.is_scope_alive(child));
    assert!(world.is_alive(local.id()));
    world.dispose(parent);
    assert_eq!(*history.borrow(), [2, 1]);
    assert!(!world.is_alive(local.id()));
}

#[test]
fn persisted_named_scope_survives_retirement_and_restart() {
    let mut world = ReactiveWorld::new();
    let scope = world.new_keyed_scope(world.root_scope(), "scope/name");
    let mut cx = world.build(scope, None);
    let value = cx.persisted("counter", || 2u32);
    cx.signal(17);
    world.write(scope, None).set(value, 5);
    world.dispose(scope);
    let scope = world.new_keyed_scope(world.root_scope(), "scope/name");
    let mut cx = world.build(scope, None);
    let value = cx.persisted("counter", || panic!("persisted initializer ran"));
    assert_eq!(cx.sample::<u32>(value), 5);
    let saved = world.export_persisted();
    assert_eq!(saved.len(), 1);

    let mut after_restart = ReactiveWorld::new();
    after_restart.import_persisted(saved);
    let scope = after_restart.new_keyed_scope(after_restart.root_scope(), "scope/name");
    let mut cx = after_restart.build(scope, None);
    let different_type = cx.persisted("counter", || 10u64);
    assert_eq!(cx.sample(different_type), 10);
    assert_eq!(after_restart.take_diagnostics()[0].code, &codes::PERSISTED_TYPE_CHANGED);
}
