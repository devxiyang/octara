//! An application catch cannot validate an interrupted incremental update.
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

#[test]
fn caught_row_mutation_blocks_reads_and_publication_but_keeps_cleanup() {
    let app = App::new(HeadlessPlatform::default());
    let cleaned = Rc::new(Cell::new(false));
    let (items, selected) = app.run(|| {
        let items = state_list("rows", || vec![(1u32, false)], |row| row.0);
        let selected = items.filter(|row| row.1);
        let item = items.item(&1).unwrap();
        let cleaned = cleaned.clone();
        let cleanup_view = selected.clone();
        on_cleanup(move || {
            assert!(catch_unwind(AssertUnwindSafe(|| items.item(&1))).is_err());
            assert!(catch_unwind(AssertUnwindSafe(|| cleanup_view.keys())).is_err());
            cleaned.set(item.with(|row| row.1));
        });
        (items, selected)
    });
    let displayed = selected.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            let displayed = displayed.clone();
            text(move || displayed.len().to_string())
        })
        .unwrap();
    let frames = window.frames_presented();
    let result = catch_unwind(AssertUnwindSafe(|| {
        app.run(|| {
            assert!(
                catch_unwind(AssertUnwindSafe(|| items.update(&1, |row| {
                    row.1 = true;
                    panic!("interrupted row edit");
                })))
                .is_err()
            );
            assert!(app.is_failed());
            assert!(catch_unwind(AssertUnwindSafe(|| selected.keys())).is_err());
            assert!(catch_unwind(AssertUnwindSafe(|| items.keys())).is_err());
            assert!(catch_unwind(AssertUnwindSafe(|| items.push((2, true)))).is_err());
        })
    }));
    assert!(result.is_err());
    assert_eq!(window.frames_presented(), frames);
    assert!(!window.unsettled().is_empty());
    assert!(app.diagnostics().iter().any(|d| d.code == "APP_FAILED"));
    app.shutdown();
    assert!(cleaned.get(), "cleanup can inspect existing source values; mutation was not rolled back");
    assert!(app.pending().is_empty());
}

#[test]
fn projection_panic_abandons_siblings_and_invalidates_composed_reads() {
    let app = App::new(HeadlessPlatform::default());
    let (items, mapped, sibling, composed) = app.run(|| {
        let items = state_list("rows", || vec![(1u32, 0u32)], |row| row.0);
        let mapped = items.map(|row| {
            assert_ne!(row.1, 1, "projection failed");
            row.1
        });
        let sibling = items.filter(|row| row.1 > 0);
        let composed = sibling.map(|row| row.1 * 2);
        (items, mapped, sibling, composed)
    });
    assert!(
        catch_unwind(AssertUnwindSafe(|| app.run(|| {
            assert!(catch_unwind(AssertUnwindSafe(|| items.update(&1, |row| row.1 = 1))).is_err());
            assert!(app.is_failed());
            assert!(catch_unwind(AssertUnwindSafe(|| mapped.item(&1).unwrap().get())).is_err());
            assert!(catch_unwind(AssertUnwindSafe(|| sibling.keys())).is_err());
            assert!(catch_unwind(AssertUnwindSafe(|| composed.keys())).is_err());
        })))
        .is_err()
    );
    app.shutdown();
    assert!(app.pending().is_empty());
}

#[test]
fn post_mutation_clone_failure_is_terminal() {
    struct Row {
        key: u32,
        value: u32,
    }
    impl Clone for Row {
        fn clone(&self) -> Self {
            assert_ne!(self.value, 1, "post-mutation snapshot failed");
            Self { key: self.key, value: self.value }
        }
    }
    let app = App::new(HeadlessPlatform::default());
    let items = app.run(|| state_list("rows", || vec![Row { key: 1, value: 0 }], |row| row.key));
    assert!(
        catch_unwind(AssertUnwindSafe(|| app.run(|| {
            assert!(catch_unwind(AssertUnwindSafe(|| items.update(&1, |row| row.value = 1))).is_err());
            assert!(app.is_failed());
        })))
        .is_err()
    );
    app.shutdown();
    assert!(app.pending().is_empty());
}

#[test]
fn pre_mutation_refusals_and_predicate_panics_do_not_poison_the_graph() {
    let app = App::new(HeadlessPlatform::default());
    app.run(|| {
        let items = state_list("rows", || vec![(1u32, false)], |row| row.0);
        items.push((1, true)); // Duplicate key: ordinary refusal, no partial change.
        items.update(&99, |_| panic!("missing key callback must not execute"));
        items.remove(&99);
        assert!(catch_unwind(AssertUnwindSafe(|| items.retain(|_| panic!("predicate before any removal")))).is_err());
        assert!(!app.is_failed());
        assert_eq!(items.keys(), vec![1]);
        items.update(&1, |row| row.1 = true);
        assert!(items.item(&1).unwrap().get().1);
    });
    assert!(!app.is_failed());
    app.shutdown();
}

#[test]
fn a_task_catching_collection_failure_cannot_run_the_next_ready_task() {
    let app = App::new(HeadlessPlatform::default());
    let items = app.run(|| state_list("rows", || vec![(1u32, 0u32)], |row| row.0));
    let sibling_ran = Rc::new(Cell::new(false));
    let ran = sibling_ran.clone();
    app.run(|| {
        spawn(async move {
            assert!(
                catch_unwind(AssertUnwindSafe(|| items.update(&1, |row| {
                    row.1 = 1;
                    panic!("task mutation failed");
                })))
                .is_err()
            );
        });
        spawn(async move {
            ran.set(true);
        });
    });
    assert!(catch_unwind(AssertUnwindSafe(|| app.poll())).is_err());
    assert!(app.is_failed());
    assert!(!sibling_ran.get());
    app.shutdown();
    assert!(app.pending().is_empty());
}
