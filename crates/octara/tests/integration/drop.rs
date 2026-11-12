//! Files dragged from the system: the element under them that takes them shows it, and gets
//! their paths when they are dropped.

use octara::app::FileDrag;
use octara::prelude::*;
use std::path::PathBuf;

#[test]
fn dropped_files_reach_the_element_under_them() {
    let mut w = Window::headless(|| {
        let got = state("got", String::new);
        col()
            .p_4()
            .gap_4()
            .child(
                col()
                    .id("zone")
                    .w(300.0)
                    .h(100.0)
                    .bg(Token::Surface)
                    .drag_over(|p| p.bg(Token::Accent))
                    .on_drop(move |paths| got.set(format!("{paths:?}")))
                    .child(text("Drop here").id("label")),
            )
            .child(text(move || got.get()).id("got"))
            .child(el().id("elsewhere").size_8())
            .into()
    });
    let (x, y, ..) = w.kernel().shown_rect(w.kernel().find("label").unwrap());
    let paths = vec![PathBuf::from("/tmp/a.png"), PathBuf::from("/tmp/b.png")];
    let zone = w.kernel().find("zone").unwrap();
    w.event(InputEvent::FileDrag(FileDrag::Over { x: x + 4.0, y: y + 4.0, paths: paths.clone() }));
    assert!(w.kernel().state(zone).contains(State::DRAG_OVER), "over a descendant: the zone takes them");
    let (ex, ey, ..) = w.kernel().shown_rect(w.kernel().find("elsewhere").unwrap());
    w.event(InputEvent::FileDrag(FileDrag::Over { x: ex + 2.0, y: ey + 2.0, paths: paths.clone() }));
    assert!(!w.kernel().state(zone).contains(State::DRAG_OVER), "moved off it");
    w.event(InputEvent::FileDrag(FileDrag::Drop { x: x + 4.0, y: y + 4.0, paths }));
    assert!(!w.kernel().state(zone).contains(State::DRAG_OVER));
    assert_eq!(w.kernel().text(w.kernel().find("got").unwrap()), r#"["/tmp/a.png", "/tmp/b.png"]"#);
}
