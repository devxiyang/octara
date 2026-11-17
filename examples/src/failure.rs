//! Explicit fault injection for the native host's failure boundary.
//! Run the window example with `failure`; each panic case requires a fresh process.

use octara::prelude::*;

#[ui]
pub fn view() -> View {
    let count = state("count", || 0);
    let fail_binding = state("fail_binding", || false);
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(text("Runtime failure validation"))
        .child(
            text(move || {
                assert!(!fail_binding.get(), "injected binding panic");
                format!("Accepted: {}", count.get())
            })
            .id("status"),
        )
        .child(el().id("accept").w(180).h(40).child("Accept").on_press(move || count.update(|n| *n += 1)))
        .child(el().id("fail_event").w(180).h(40).child("Panic in handler").on_press(|| panic!("injected event panic")))
        .child(el().id("fail_task").w(180).h(40).child("Panic in task").on_press(|| {
            spawn(async { panic!("injected task panic") });
        }))
        .child(el().id("fail_binding").w(180).h(40).child("Panic in binding").on_press(move || fail_binding.set(true)))
        .into()
}
