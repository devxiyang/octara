use super::*;

#[test]
fn released_cleanup_preserves_order_and_cannot_remove_a_reused_scopes_work() {
    let rt = Runtime::new();
    let calls = Rc::new(RefCell::new(Vec::new()));
    rt.enter(|| {
        let scope = rt.create_scope(None, "old");
        let stale = rt.with_scope(Some(scope), || {
            let out = calls.clone();
            rt.on_cleanup(Box::new(move || out.borrow_mut().push(1)));
            let removed = rt.register_cleanup(Box::new(|| panic!("released registration ran")));
            drop(removed);
            let out = calls.clone();
            rt.register_cleanup(Box::new(move || out.borrow_mut().push(2)))
        });
        rt.dispose_scope(scope);
        assert_eq!(*calls.borrow(), [2, 1]);
        let reused = rt.create_scope(None, "new");
        assert_eq!(reused.i, scope.i);
        let live = rt.with_scope(Some(reused), || {
            let out = calls.clone();
            rt.register_cleanup(Box::new(move || out.borrow_mut().push(3)))
        });
        drop(stale);
        assert_eq!(rt.cleanup_count(reused), 1);
        rt.dispose_scope(reused);
        drop(live);
        assert_eq!(*calls.borrow(), [2, 1, 3]);
    });
}

#[test]
fn collection_delivery_unwind_releases_pending_work() {
    let rt = Runtime::new();
    let retained = Rc::new(());
    let captured = retained.clone();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.deliver_collections([
            (1, Box::new(|| -> () { panic!("projection failed") }) as CollectionDelivery),
            (2, Box::new(move || drop(captured)) as CollectionDelivery),
        ]);
    }));
    assert!(failed.is_err());
    assert_eq!(Rc::strong_count(&retained), 1);
    assert!(rt.collections.work.borrow().is_empty());
    assert!(!rt.collections.active.get());
    let delivered = Rc::new(Cell::new(false));
    let observed = delivered.clone();
    rt.deliver_collections([(1, Box::new(move || observed.set(true)) as CollectionDelivery)]);
    assert!(delivered.get());
}

#[test]
fn interrupted_scope_teardown_preserves_order_and_never_replays_started_work() {
    struct ContextDrop(Rc<RefCell<Vec<&'static str>>>);
    impl Drop for ContextDrop {
        fn drop(&mut self) {
            self.0.borrow_mut().push("context");
            panic!("context drop failed");
        }
    }
    let rt = Runtime::new();
    let calls = Rc::new(RefCell::new(Vec::new()));
    rt.enter(|| {
        let parent = rt.create_scope(None, "parent");
        let child = rt.create_scope(Some(parent), "child");
        rt.with_scope(Some(parent), || {
            let out = calls.clone();
            rt.on_cleanup(Box::new(move || out.borrow_mut().push("parent")));
        });
        rt.with_scope(Some(child), || {
            let out = calls.clone();
            rt.on_cleanup(Box::new(move || out.borrow_mut().push("child")));
            let out = calls.clone();
            rt.on_cleanup(Box::new(move || {
                out.borrow_mut().push("failure");
                panic!("child cleanup failed");
            }));
            rt.provide(TypeId::of::<ContextDrop>(), Rc::new(ContextDrop(calls.clone())));
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.dispose_scope(parent))).is_err());
        assert_eq!(*calls.borrow(), ["failure"]);
        assert!(!rt.scope_alive(parent) && !rt.scope_alive(child));
        assert!(!rt.free_scopes.borrow().contains(&parent.i));
        assert!(!rt.free_scopes.borrow().contains(&child.i));
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.dispose_scope(parent))).is_err());
        assert_eq!(*calls.borrow(), ["failure", "child", "context"]);
        rt.dispose_scope(parent);
        assert_eq!(*calls.borrow(), ["failure", "child", "context", "parent"]);
        rt.dispose_scope(parent);
        assert_eq!(rt.free_scopes.borrow().len(), 2, "finished scopes return to storage exactly once");
        let replacement = rt.create_scope(None, "replacement");
        rt.dispose_scope(parent);
        assert!(rt.scope_alive(replacement), "a retired identity cannot dispose a reused slot");
        rt.dispose_scope(replacement);
    });
}

#[test]
fn repeated_app_state_lookup_keeps_one_ownership_record() {
    let rt = Runtime::new();
    let root = rt.create_app_scope();
    rt.enter(|| {
        rt.with_scope(Some(root), || {
            let value = state("value", || 1u32);
            for _ in 0..100 {
                assert_eq!(state("value", || panic!("lookup reinitialized state")), value);
            }
        })
    });
    assert_eq!(rt.scopes.borrow()[root.i as usize].nodes.len(), 1);
    assert!(rt.diagnostics().is_empty());
    rt.enter(|| rt.dispose_scope(root));
}

fn with_scope<R>(f: impl FnOnce(&Rc<Runtime>) -> R) -> R {
    let rt = Runtime::new();
    let s = rt.create_scope(None, "t");
    rt.enter(|| rt.with_frame(Ctx::Idle, None, Some(Some(s)), || f(&rt)))
}

fn handler(rt: &Runtime, f: impl FnOnce()) {
    rt.with_frame(Ctx::Handler, None, None, f);
    rt.flush();
}

#[test]
fn diamond_glitch_free_and_cutoff() {
    with_scope(|rt| {
        let s = Signal::scoped(1);
        let a = memo(move || s.get() * 2);
        let parity = memo(move || s.get() % 2);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s2 = seen.clone();
        binding(Location::caller(), move || s2.borrow_mut().push((a.get(), parity.get())));
        handler(rt, || s.set(3));
        handler(rt, || s.set(4));
        assert_eq!(*seen.borrow(), vec![(2, 1), (6, 1), (8, 0)]);
    });
}

#[test]
fn equal_branch_switch_retracks_before_cutoff_with_or_without_early_reads() {
    for early_read in [false, true] {
        for start_left in [false, true] {
            with_scope(|rt| {
                let left = Signal::scoped(1);
                let right = Signal::scoped(3);
                let choose_left = Signal::scoped(start_left);
                let evaluations = Rc::new(Cell::new(0));
                let count = evaluations.clone();
                let parity = memo(move || {
                    count.set(count.get() + 1);
                    (if choose_left.get() { left.get() } else { right.get() }) % 2
                });
                let seen = Rc::new(RefCell::new(Vec::new()));
                let out = seen.clone();
                watch(move || parity.get(), move |new, old| out.borrow_mut().push((*new, old.copied())));
                let (old_branch, new_branch) = if start_left { (left, right) } else { (right, left) };
                choose_left.set(!start_left);
                old_branch.set(4);
                if early_read {
                    assert_eq!(untrack(|| parity.get()), 1);
                    assert_eq!(*seen.borrow(), [(1, None)], "pulling a memo must not run its watch");
                }
                rt.flush();
                assert_eq!(evaluations.get(), 2);
                assert_eq!(*seen.borrow(), [(1, None)], "equal result cuts off the callback, not retracking");
                handler(rt, || old_branch.set(6));
                assert_eq!(evaluations.get(), 2, "old branch must be detached even after equality cutoff");
                new_branch.set(4);
                if early_read {
                    assert_eq!(untrack(|| parity.get()), 0);
                    assert_eq!(seen.borrow().len(), 1);
                }
                rt.flush();
                assert_eq!(evaluations.get(), 3);
                assert_eq!(*seen.borrow(), [(1, None), (0, Some(1))]);
                handler(rt, || choose_left.set(start_left));
                assert_eq!(evaluations.get(), 4);
                handler(rt, || new_branch.set(9));
                assert_eq!(evaluations.get(), 4);
                assert_eq!(seen.borrow().len(), 2);
                assert!(!rt.has_pending());
                assert!(rt.diagnostics().is_empty());
            });
        }
    }
}

#[test]
fn watch_feedback_observes_creation_order_and_converges() {
    for writer_first in [false, true] {
        with_scope(|rt| {
            let input = Signal::scoped(0);
            let output = Signal::scoped(0);
            let doubled = memo(move || input.get() * 2);
            let seen = Rc::new(RefCell::new(Vec::new()));
            let writer = || watch(move || input.get(), move |new, _| output.set(new * 10));
            let reader = || {
                let out = seen.clone();
                watch(move || (input.get(), doubled.get(), output.get()), move |new, _| out.borrow_mut().push(*new))
            };
            if writer_first {
                writer();
                reader();
            } else {
                reader();
                writer();
            }
            rt.flush();
            seen.borrow_mut().clear();
            input.set(1);
            assert_eq!(untrack(|| doubled.get()), 2, "reads are current before effect delivery");
            assert!(seen.borrow().is_empty());
            rt.flush();
            let expected = if writer_first { vec![(1, 2, 10)] } else { vec![(1, 2, 0), (1, 2, 10)] };
            assert_eq!(*seen.borrow(), expected);
            assert_eq!(untrack(|| output.get()), 10);
            assert!(!rt.has_pending());
            assert!(rt.diagnostics().is_empty());
        });
    }
}

#[test]
fn watch_removing_a_branch_preempts_its_queued_descendant_watch() {
    with_scope(|rt| {
        let input = Signal::scoped(0);
        let visible = Signal::scoped(true);
        watch(
            move || input.get(),
            move |new, _| {
                if *new > 0 {
                    visible.set(false);
                }
            },
        );
        let child = rt.create_scope(rt.current_scope(), "branch");
        let calls = Rc::new(Cell::new(0));
        let capture = Rc::new(());
        let released = Rc::downgrade(&capture);
        let out = calls.clone();
        let derived = rt.with_scope(Some(child), || {
            let derived = memo(move || input.get() * 2);
            watch(
                move || derived.get(),
                move |_, _| {
                    let _keep = &capture;
                    out.set(out.get() + 1);
                },
            );
            derived
        });
        let runtime = Rc::downgrade(rt);
        structural_binding(Location::caller(), move || {
            if !visible.get() {
                runtime.upgrade().unwrap().dispose_scope(child);
            }
        });
        input.set(1);
        assert_eq!(untrack(|| derived.get()), 2);
        assert_eq!(calls.get(), 1);
        rt.flush();
        assert_eq!(calls.get(), 1, "structural removal must preempt the already queued descendant");
        assert!(released.upgrade().is_none(), "the retired callback releases its captures");
        handler(rt, || input.set(2));
        assert_eq!(calls.get(), 1);
        assert!(!rt.has_pending());
        assert!(rt.diagnostics().is_empty());
    });
}

#[test]
fn watch_clamp_settles_and_callback_is_untracked() {
    with_scope(|rt| {
        let count = Signal::scoped(0);
        let other = Signal::scoped(0);
        let calls = Rc::new(Cell::new(0));
        let c = calls.clone();
        watch(
            move || count.get(),
            move |v, _| {
                other.get(); // untracked: must not subscribe
                c.set(c.get() + 1);
                if *v > 3 {
                    count.set(3);
                }
            },
        );
        for _ in 0..5 {
            handler(rt, || count.update(|n| *n += 1));
        }
        assert_eq!(untrack(|| count.get()), 3);
        let before = calls.get();
        handler(rt, || other.set(1));
        assert_eq!(calls.get(), before, "callback reads are untracked");
        assert!(rt.diagnostics().is_empty());
    });
}

#[test]
fn layer_rules_and_dedupe() {
    with_scope(|rt| {
        let s = Signal::scoped(0);
        for _ in 0..3 {
            rt.with_frame(Ctx::Build, None, None, || s.set(1));
        }
        let m = memo(move || {
            s.set(2);
            0
        });
        m.get();
        let codes = rt.diagnostic_counts();
        assert_eq!(codes, vec![("WRITE_IN_BUILD", 3), ("WRITE_IN_DERIVE", 1)]);
        assert_eq!(untrack(|| s.get()), 0);
    });
}

#[test]
fn switching_dependencies_drops_the_old_ones() {
    with_scope(|rt| {
        let flag = Signal::scoped(true);
        let a = Signal::scoped(1);
        let b = Signal::scoped(10);
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        binding(Location::caller(), move || {
            r.set(r.get() + 1);
            let _ = if flag.get() { a.get() } else { b.get() };
        });
        handler(rt, || flag.set(false));
        handler(rt, || a.set(2));
        assert_eq!(runs.get(), 2, "a is no longer a dependency");
        handler(rt, || b.set(11));
        assert_eq!(runs.get(), 3);
    });
}

#[test]
fn disposed_scopes_stop_their_observers() {
    let rt = Runtime::new();
    rt.enter(|| {
        let root = rt.create_scope(None, "root");
        let child = rt.create_scope(Some(root), "child");
        let s = rt.with_frame(Ctx::Idle, None, Some(Some(root)), || Signal::scoped(0));
        let runs = Rc::new(Cell::new(0));
        let r = runs.clone();
        rt.with_frame(Ctx::Idle, None, Some(Some(child)), || {
            binding(Location::caller(), move || {
                s.get();
                r.set(r.get() + 1);
            })
        });
        rt.dispose_scope(child);
        handler(&rt, || s.set(1));
        assert_eq!(runs.get(), 1);
    });
}

#[test]
fn runaway_watch_is_cut() {
    with_scope(|rt| {
        let n = Signal::scoped(0);
        watch(move || n.get(), move |v, _| n.set(v + 1));
        rt.flush();
        assert_eq!(rt.diagnostics()[0].code, "FLUSH_RUNAWAY");
    });
}

#[test]
fn rejected_update_does_not_invoke_user_code() {
    with_scope(|rt| {
        let value = Signal::scoped(0);
        let called = Cell::new(false);
        rt.with_frame(Ctx::Build, None, None, || {
            value.update(|n| {
                called.set(true);
                *n = 7;
            });
        });
        assert!(!called.get());
        assert_eq!(value.get(), 0);
    });
}

#[test]
fn untrack_cannot_grant_write_permission() {
    with_scope(|rt| {
        let value = Signal::scoped(0);
        let derived = memo(move || {
            untrack(|| {
                value.set(8);
                1
            })
        });
        assert_eq!(derived.get(), 1);
        assert_eq!(value.get(), 0);
        assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"));
    });
}

#[test]
fn watch_registration_cannot_escape_a_pure_computation() {
    with_scope(|rt| {
        let value = Signal::scoped(0);
        let derived = memo(move || {
            watch(|| 1, move |_, _| value.set(9));
            1
        });
        derived.get();
        assert_eq!(value.get(), 0);
        assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"));
    });
}

#[test]
fn foreign_runtime_handle_cannot_mutate_matching_slot() {
    let a = Runtime::new();
    let b = Runtime::new();
    let value = a.enter(|| Signal::scoped(1));
    b.enter(|| {
        let other = Signal::scoped(2);
        assert_ne!(value, other);
        value.set(99);
        assert_eq!(other.get(), 2);
    });
    assert_eq!(a.enter(|| value.get()), 1);
}

#[test]
fn execution_context_restores_after_unwind() {
    with_scope(|rt| {
        let scope = rt.current_scope();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.with_frame(Ctx::Derive, None, Some(None), || panic!("test unwind"));
        }));
        assert!(result.is_err());
        assert_eq!(rt.ctx(), Ctx::Idle);
        assert_eq!(rt.current_scope(), scope);
    });
}

#[test]
fn non_clone_state_updates_in_place_and_memo_can_borrow_non_clone_results() {
    #[derive(PartialEq)]
    struct Form {
        names: Vec<String>,
    }
    with_scope(|rt| {
        let form = state("form", || Form { names: vec!["Ada".into()] });
        let names = memo(move || form.with(|f| Form { names: f.names.clone() }));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let out = seen.clone();
        binding(Location::caller(), move || out.borrow_mut().push(names.with(|f| f.names.len())));
        let address = form.with(|f| f as *const Form);
        handler(rt, || form.update(|f| f.names.push("Lin".into())));
        assert_eq!(form.read_only().with(|f| f as *const Form), address);
        assert_eq!(*seen.borrow(), [1, 2]);
        form.set(Form { names: Vec::new() });
        rt.flush();
        assert_eq!(*seen.borrow(), [1, 2, 0]);
    });
}

#[test]
fn borrowed_value_rejects_conflicts_without_borrowing_the_graph() {
    with_scope(|rt| {
        let value = Signal::scoped(vec![1]);
        let other = Signal::scoped(0);
        let called = Cell::new(false);
        value.with(|before| {
            other.update(|n| *n += 1);
            value.set(vec![9]);
            value.update(|_| called.set(true));
            assert_eq!(before, &[1]);
            assert_eq!(value.with(Vec::len), 1);
        });
        assert!(!called.get());
        assert_eq!(other.get(), 1);
        assert_eq!(value.get(), [1]);
        assert!(rt.diagnostics().iter().any(|d| d.code == "SIGNAL_BORROW"));
    });
}

#[test]
fn mutation_unwind_releases_borrow_and_invalidates_changed_value() {
    with_scope(|rt| {
        let value = Signal::scoped(1);
        let doubled = memo(move || value.get() * 2);
        assert_eq!(doubled.get(), 2);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            value.update(|v| {
                *v = 5;
                panic!("partially mutated");
            })
        }));
        assert!(result.is_err());
        assert_eq!(doubled.get(), 10, "never leave the old memo clean after mutation");
        assert_eq!(rt.ctx(), Ctx::Idle);
    });
}

#[test]
fn untracked_memo_read_still_tracks_its_own_inputs() {
    with_scope(|rt| {
        let source = Signal::scoped(1);
        let trigger = Signal::scoped(0);
        let inner = memo(move || source.get() * 2);
        let outer = memo(move || {
            trigger.get();
            untrack(|| inner.get())
        });
        assert_eq!(outer.get(), 2);
        handler(rt, || source.set(3));
        assert_eq!(outer.get(), 2, "untracked source did not subscribe outer");
        handler(rt, || trigger.set(1));
        assert_eq!(outer.get(), 6, "inner still follows source");
    });
}

#[test]
fn initializers_sample_without_subscribing_or_writing_existing_state() {
    with_scope(|rt| {
        let source = Signal::scoped(1);
        let initialized = rt.with_frame(Ctx::Build, None, None, || {
            state("initial", || {
                untrack(|| source.set(7));
                source.get()
            })
        });
        assert_eq!(source.get(), 1);
        assert_eq!(initialized.get(), 1);
        assert!(!rt.diagnostics().iter().any(|d| d.code == "STRICT_READ_UNTRACKED"));
        assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_BUILD"));
    });
}

#[test]
fn watch_and_cleanup_restore_registration_scope() {
    with_scope(|rt| {
        let parent = rt.current_scope().unwrap();
        let child = rt.create_scope(Some(parent), "child");
        let value = Signal::scoped(0);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let cleanups = Rc::new(RefCell::new(Vec::new()));
        rt.with_frame(Ctx::Build, None, Some(Some(child)), || {
            provide("child");
            let out = seen.clone();
            watch(move || value.get(), move |_, _| out.borrow_mut().push(use_context::<&str>()));
            let out = cleanups.clone();
            rt.on_cleanup(Box::new(move || out.borrow_mut().push(use_context::<&str>())));
        });
        handler(rt, || value.set(1));
        assert_eq!(*seen.borrow(), [Some("child"), Some("child")]);
        rt.dispose_scope(child);
        handler(rt, || value.set(2));
        assert_eq!(seen.borrow().len(), 2);
        assert_eq!(*cleanups.borrow(), [Some("child")]);
    });
}

#[test]
fn observer_unwind_keeps_the_rest_of_the_ready_batch() {
    with_scope(|rt| {
        let trigger = Signal::scoped(false);
        let observed = Rc::new(Cell::new(false));
        binding(Location::caller(), move || {
            assert!(!trigger.get(), "failing binding");
        });
        let out = observed.clone();
        binding(Location::caller(), move || out.set(trigger.get()));
        trigger.set(true);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.flush()));
        assert!(result.is_err());
        assert!(!rt.flushing.get());
        assert!(rt.has_pending());
        rt.flush();
        assert!(observed.get());
        assert!(!rt.has_pending());
    });
}

#[test]
fn cleanup_cannot_register_work_after_its_owner_retires() {
    with_scope(|rt| {
        let child = rt.create_scope(rt.current_scope(), "child");
        let called = Rc::new(Cell::new(false));
        let out = called.clone();
        rt.with_scope(Some(child), || {
            rt.on_cleanup(Box::new(move || {
                watch(|| 1, move |_, _| out.set(true));
            }))
        });
        rt.dispose_scope(child);
        assert!(!called.get());
        assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_DISPOSED"));
    });
}

#[test]
fn replacing_context_drops_user_values_outside_the_scope_table_borrow() {
    struct Probe;
    impl Drop for Probe {
        fn drop(&mut self) {
            rt(|rt| {
                let _ = rt.scope_path(rt.current_scope().unwrap());
            });
        }
    }
    with_scope(|rt| {
        provide(Rc::new(Probe));
        provide(Rc::new(Probe));
        rt.dispose_scope(rt.current_scope().unwrap());
    });
}

#[test]
fn read_handles_share_identity_without_value_trait_bounds() {
    struct Payload;
    #[derive(PartialEq)]
    struct Derived(usize);
    with_scope(|rt| {
        let source = Signal::scoped(Payload);
        let read = source.read_only();
        assert_eq!(read, source.read_only());
        assert_ne!(read, Signal::scoped(Payload).read_only());
        assert!(format!("{read:?}").starts_with("ReadSignal("));
        let runs = Rc::new(Cell::new(0));
        let out = runs.clone();
        let derived: ReadSignal<Derived> = memo(move || {
            source.with(|_| ());
            out.set(out.get() + 1);
            Derived(42)
        });
        assert_eq!(runs.get(), 0, "the unified read handle keeps memo evaluation lazy");
        assert_eq!(derived.with(|value| value.0), 42);
        assert_eq!(runs.get(), 1);
        handler(rt, || source.set(Payload));
        assert_eq!(derived.with(|value| value.0), 42);
        assert_eq!(runs.get(), 2);
    });
}

#[test]
fn non_clone_watch_borrows_previous_results_and_cuts_off_equal_values() {
    #[derive(PartialEq)]
    struct Snapshot(i32);
    with_scope(|rt| {
        let source = Signal::scoped(0);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let out = seen.clone();
        let _ = watch(move || Snapshot(source.get() / 2), move |new, old| out.borrow_mut().push((new.0, old.map(|old| old.0))));
        handler(rt, || source.set(1));
        handler(rt, || source.set(2));
        handler(rt, || source.set(3));
        handler(rt, || source.set(4));
        assert_eq!(*seen.borrow(), [(0, None), (1, Some(0)), (2, Some(1))]);
        assert!(rt.diagnostics().is_empty());
    });
}

#[test]
fn watch_stop_cancels_queued_work_and_cannot_stop_a_reused_slot() {
    with_scope(|rt| {
        let source = Signal::scoped(0);
        let calls = Rc::new(Cell::new(0));
        let out = calls.clone();
        let stopped = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
        source.set(1);
        assert!(rt.has_pending());
        stopped.stop();
        stopped.stop();
        assert!(!rt.has_pending(), "stopping cancels pending invocations");
        rt.flush();
        assert_eq!(calls.get(), 1);
        let replacement_calls = Rc::new(Cell::new(0));
        let out = replacement_calls.clone();
        let _ = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
        stopped.stop();
        handler(rt, || source.set(2));
        assert_eq!(calls.get(), 1);
        assert_eq!(replacement_calls.get(), 2);
    });
}

#[test]
fn watch_can_stop_itself_without_dropping_its_running_callback() {
    struct Capture(Rc<Cell<bool>>);
    impl Drop for Capture {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    with_scope(|rt| {
        let source = Signal::scoped(0);
        let handle = Rc::new(Cell::new(None::<Watch>));
        let dropped = Rc::new(Cell::new(false));
        let calls = Rc::new(Cell::new(0));
        let (stop, out) = (handle.clone(), calls.clone());
        let capture = Capture(dropped.clone());
        let watch = watch(
            move || source.get(),
            move |new, old| {
                assert!(!capture.0.get(), "the callback's capture remains alive while it runs");
                out.set(out.get() + 1);
                if *new == 1 {
                    assert_eq!(old, Some(&0));
                    source.set(2); // Queue another round before stopping in this callback.
                    stop.get().unwrap().stop();
                    assert!(!capture.0.get());
                }
            },
        );
        handle.set(Some(watch));
        handler(rt, || source.set(1));
        assert_eq!(calls.get(), 2);
        assert!(dropped.get(), "stopped callback captures are released after it returns");
        assert!(!rt.has_pending());
        handler(rt, || source.set(3));
        assert_eq!(calls.get(), 2);
    });
}

#[test]
fn watch_scope_disposal_releases_non_clone_results_and_queued_callbacks() {
    struct Snapshot(Rc<Cell<usize>>);
    impl PartialEq for Snapshot {
        fn eq(&self, _: &Self) -> bool {
            true
        }
    }
    impl Drop for Snapshot {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    with_scope(|rt| {
        let source = Signal::scoped(0);
        let child = rt.create_scope(rt.current_scope(), "watch owner");
        let dropped = Rc::new(Cell::new(0));
        let calls = Rc::new(Cell::new(0));
        let (snapshot, out) = (dropped.clone(), calls.clone());
        let stop = rt.with_scope(Some(child), || {
            watch(
                move || {
                    source.get();
                    Snapshot(snapshot.clone())
                },
                move |_, _| out.set(out.get() + 1),
            )
        });
        source.set(1);
        rt.dispose_scope(child);
        assert_eq!(dropped.get(), 1, "the retained source result leaves with its scope");
        assert!(!rt.has_pending());
        stop.stop();
        handler(rt, || source.set(2));
        assert_eq!(calls.get(), 1);
    });
}

#[test]
fn foreign_runtime_watch_stop_does_not_stop_either_runtime() {
    let a = Runtime::new();
    let b = Runtime::new();
    let a_scope = a.create_scope(None, "a");
    let b_scope = b.create_scope(None, "b");
    let a_calls = Rc::new(Cell::new(0));
    let b_calls = Rc::new(Cell::new(0));
    let out = a_calls.clone();
    let (a_source, a_watch) = a.enter(|| {
        a.with_scope(Some(a_scope), || {
            let source = Signal::scoped(0);
            let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
            (source, watch)
        })
    });
    let out = b_calls.clone();
    b.enter(|| {
        b.with_scope(Some(b_scope), || {
            let source = Signal::scoped(0);
            let _ = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
            a_watch.stop();
            handler(&b, || source.set(1));
            assert_eq!(b_calls.get(), 2);
            assert!(b.diagnostics().iter().any(|d| d.code == "WRONG_RUNTIME"));
        })
    });
    a.enter(|| {
        handler(&a, || a_source.set(1));
        assert_eq!(a_calls.get(), 2, "a foreign stop leaves the original watch active");
        a_watch.stop();
        handler(&a, || a_source.set(2));
        assert_eq!(a_calls.get(), 2);
    });
}

#[test]
fn untrack_cannot_stop_a_watch_from_a_pure_computation() {
    with_scope(|rt| {
        let source = Signal::scoped(0);
        let calls = Rc::new(Cell::new(0));
        let out = calls.clone();
        let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
        let derived = memo(move || untrack(|| watch.stop()));
        derived.get();
        handler(rt, || source.set(1));
        assert_eq!(calls.get(), 2);
        assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"));
    });
}

#[test]
fn watch_stop_is_allowed_during_build_and_cleanup() {
    with_scope(|rt| {
        let source = Signal::scoped(0);
        let calls = Rc::new(Cell::new(0));
        let out = calls.clone();
        rt.with_frame(Ctx::Build, None, None, || {
            let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
            watch.stop();
        });
        handler(rt, || source.set(1));
        assert_eq!(calls.get(), 1);
        let out = calls.clone();
        let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
        let child = rt.create_scope(rt.current_scope(), "stop from cleanup");
        rt.with_scope(Some(child), || rt.on_cleanup(Box::new(move || watch.stop())));
        source.set(2);
        rt.dispose_scope(child);
        rt.flush();
        assert_eq!(calls.get(), 2, "a retiring child can stop another live watch");
        assert!(!rt.has_pending());
        assert!(rt.diagnostics().is_empty(), "{:?}", rt.diagnostics());
    });
}

#[test]
fn exhausted_node_and_scope_generations_never_wrap() {
    with_scope(|rt| {
        let signal = Signal::scoped(1);
        signal.dispose();
        rt.nodes.borrow_mut()[signal.idx.i as usize].g = u32::MAX;
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Signal::scoped(2))).is_err());
        assert!(!rt.alive(signal.idx));
        let scope = rt.create_scope(None, "old");
        rt.dispose_scope(scope);
        rt.scopes.borrow_mut()[scope.i as usize].g = u32::MAX;
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.create_scope(None, "new"))).is_err());
        assert!(!rt.scope_alive(scope));
    });
}
