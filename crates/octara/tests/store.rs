use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use octara::prelude::*;
use octara::runtime::ReactiveWorld;
use octara::runtime::{arena::NodeKind, graph::FlushTier};

mod model {
    use octara::Store;

    #[derive(Debug, PartialEq)]
    pub struct Title(pub String);

    #[derive(Store)]
    pub struct Details<T> {
        pub count: T,
        pub enabled: bool,
    }

    #[derive(Store)]
    pub struct Panel<T> {
        pub title: Title,
        #[store(nested)]
        pub details: Details<T>,
        pub footer: u8,
    }
}

use model::{Details, Panel, Title};

fn panel(title: &str, count: u32, enabled: bool, footer: u8) -> Panel<u32> {
    Panel { title: Title(title.to_owned()), details: Details { count, enabled }, footer }
}

fn count_reads<T: 'static>(cx: &mut BuildCx<'_>, handle: impl Readable<T>) -> Rc<Cell<u32>> {
    let count = Rc::new(Cell::new(0));
    let observed = count.clone();
    cx.memo(move |r| {
        r.with(handle, |_| observed.set(observed.get() + 1));
        observed.get()
    });
    count
}

#[test]
fn store_field_edits_and_replacements_invalidate_only_changed_paths() {
    let mut world = ReactiveWorld::new();
    let scope = world.new_scope(world.root_scope());
    let mut cx = world.build(scope, None);
    let store = cx.store(panel("title", 0, true, 1));
    let title = store.title();
    let details = store.details();
    let count = details.count();
    let enabled = details.enabled();
    let footer = store.footer();
    let reads = [
        count_reads(&mut cx, store),
        count_reads(&mut cx, title),
        count_reads(&mut cx, details),
        count_reads(&mut cx, count),
        count_reads(&mut cx, enabled),
        count_reads(&mut cx, footer),
    ];
    let reads = || reads.each_ref().map(|count| count.get());
    world.flush_derivations();
    assert_eq!(reads(), [1, 1, 1, 1, 1, 1]);

    let mut w = world.write(scope, None);
    assert!(w.mutate(count, |value| {
        *value += 1;
        true
    }));
    assert!(!w.mutate(title, |_| false));
    world.flush_derivations();
    assert_eq!(reads(), [2, 1, 2, 2, 1, 1]);

    world.write(scope, None).set(store, panel("title", 1, false, 2));
    world.flush_derivations();
    assert_eq!(reads(), [3, 1, 3, 2, 2, 2]);

    world.write(scope, None).set(details, Details { count: 2, enabled: false });
    world.flush_derivations();
    assert_eq!(reads(), [4, 1, 4, 3, 2, 2]);

    let mut w = world.write(scope, None);
    w.set(store, panel("title", 2, false, 2));
    w.set(title, Title("title".to_owned()));
    assert!(!w.mutate(store, |_| false));
    world.flush_derivations();
    assert_eq!(reads(), [4, 1, 4, 3, 2, 2]);

    world.read().with(store, |value| assert_eq!((value.title.0.as_str(), value.details.count, value.footer), ("title", 2, 2)));
    world.read().with(title, |value| assert_eq!(value.0, "title"));

    world.write(scope, None).update(details, |value| value.count += 1);
    world.flush_derivations();
    assert_eq!(reads(), [5, 1, 5, 4, 3, 2]);
    assert!(world.validate_integrity());

    world.dispose(scope);
    assert_eq!(world.live_nodes(), 0);
    assert!(world.validate_integrity());
}

#[test]
fn store_sibling_writes_do_not_schedule_title_watch_or_binding() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let store = cx.store(panel("title", 0, true, 1));
    let title = store.title();
    let watch_reads = Rc::new(Cell::new(0));
    let watch_effects = Rc::new(Cell::new(0));
    cx.watch(
        {
            let watch_reads = watch_reads.clone();
            move |r| {
                watch_reads.set(watch_reads.get() + 1);
                r.with(title, |title| title.0.len())
            }
        },
        {
            let watch_effects = watch_effects.clone();
            move |_, _, _| watch_effects.set(watch_effects.get() + 1)
        },
    );
    let binding = world.create_observer(NodeKind::Binding, scope, None);
    let read_title = |r: &mut ReadCx<'_>| r.with(title, |title| title.0.len());

    assert_eq!(world.next_observer(FlushTier::Binding), Some(binding));
    assert_eq!(world.evaluate_observer(binding, read_title), Some(5));
    world.flush_watches();
    world.run_watch_effects();
    assert_eq!((watch_reads.get(), watch_effects.get()), (1, 1));

    world.write(scope, None).update(store.details().count(), |count| *count += 1);
    world.write(scope, None).set(store, panel("title", 2, false, 2));
    assert_eq!(world.next_observer(FlushTier::Binding), None);
    world.flush_watches();
    world.run_watch_effects();
    assert_eq!((watch_reads.get(), watch_effects.get()), (1, 1));

    world.write(scope, None).set(title, Title("changed".to_owned()));
    assert_eq!(world.next_observer(FlushTier::Binding), Some(binding));
    assert_eq!(world.evaluate_observer(binding, read_title), Some(7));
    world.flush_watches();
    world.run_watch_effects();
    assert_eq!((watch_reads.get(), watch_effects.get()), (2, 2));
}

#[test]
fn a_panicking_field_edit_still_invalidates_its_observers() {
    let mut world = ReactiveWorld::new();
    let scope = world.root_scope();
    let mut cx = world.build(scope, None);
    let store = cx.store(panel("title", 0, true, 1));
    let count = store.details().count();
    let count_reads = count_reads(&mut cx, count);
    world.flush_derivations();

    let panic = catch_unwind(AssertUnwindSafe(|| {
        world.write(scope, None).update(count, |value| {
            *value += 1;
            panic!("edit failed");
        });
    }));
    assert!(panic.is_err());
    world.flush_derivations();
    assert_eq!(count_reads.get(), 2);
    assert_eq!(world.read().get(count), 1);
}
