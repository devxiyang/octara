//! Rendering incremental views: rows follow changes, logs stay bounded, nodes are reclaimed.

use octara::prelude::*;
use octara_examples::scenarios::press;
use octara_kit::button;
use std::cell::Cell;
use std::rc::Rc;

fn app(probe: Rc<Cell<usize>>) -> App {
    App::new(move || {
        let items = list("items", || (0..10u32).collect::<Vec<_>>(), |x| *x);
        let evens = items.filter(|x| x % 2 == 0);
        let shown = state("shown", || true);
        let next = state("next", || 10u32);
        let probe = probe.clone();
        col()
            .child(show(
                move || shown.get(),
                move || {
                    col().child(each(move || evens.source(), |k, _| text(format!("{k}")).id(format!("e{k}")).into())).id("evens").into()
                },
            ))
            .child(button("add").id("add").on_press(move || {
                let n = next.get();
                next.set(n + 1);
                items.push(n);
                probe.set(items.all().log_len() + evens.source().log_len());
            }))
            .child(button("drop").id("drop").on_press(move || items.retain(|x| x % 4 != 0)))
            .child(button("hide").id("hide").on_press(move || shown.update(|s| *s = !*s)))
            .into()
    })
}

#[test]
fn filtered_rows_follow_changes_and_logs_stay_bounded() {
    let probe: Rc<Cell<usize>> = Rc::default();
    let mut a = app(probe.clone());
    for _ in 0..50 {
        press(&mut a, "add").unwrap();
    }
    assert!(probe.get() <= 1, "logs grew to {}", probe.get());
    let kids = a.kernel().child_ids("evens");
    assert_eq!(kids.len(), 30);
    assert_eq!(kids[..3], ["e0", "e2", "e4"]);
    press(&mut a, "drop").unwrap();
    assert_eq!(a.kernel().child_ids("evens")[..3], ["e2", "e6", "e10"]);
    assert!(a.diagnostics().is_empty(), "{:?}", a.diagnostics());
}

#[test]
fn items_are_reclaimed_and_reload_does_not_leak() {
    let mut a = app(Rc::default());
    let start = a.work().reactive_nodes;
    for _ in 0..3 {
        a.hot_reload();
    }
    assert_eq!(a.work().reactive_nodes, start, "hot reload leaked nodes");
    press(&mut a, "drop").unwrap();
    let after_drop = a.work().reactive_nodes;
    assert!(after_drop < start, "removed items were not reclaimed");
    press(&mut a, "hide").unwrap();
    press(&mut a, "hide").unwrap();
    assert_eq!(a.work().reactive_nodes, after_drop);
}
