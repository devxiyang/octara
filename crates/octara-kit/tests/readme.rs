//! The README's example compiles and works.

use octara::prelude::*;
use octara_kit::{button, switch};

fn counter() -> View {
    let count = state("count", || 0);
    let fast = state("fast", || false);
    col()
        .child(text(move || format!("Count: {}", count.get())).id("count").text_lg())
        .child(
            row()
                .child(button("+1").id("inc").on_press(move || count.update(|n| *n += if fast.get() { 10 } else { 1 })))
                .child(switch(fast).id("fast").label("Count by ten"))
                .items_center()
                .gap_2(),
        )
        .p_4()
        .gap_3()
        .into()
}

#[test]
fn readme_counter() {
    let mut app = App::new(counter);
    app.dispatch(Event::Press("inc".into())).unwrap();
    app.dispatch(Event::Toggle("fast".into())).unwrap();
    app.dispatch(Event::Press("inc".into())).unwrap();
    assert_eq!(app.kernel().text_of("count").unwrap(), "Count: 11");
}
