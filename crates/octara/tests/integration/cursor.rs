//! How the pointer looks: set on an element and its descendants, else by what is under it.

use octara::prelude::*;

#[test]
fn the_pointer_takes_the_look_of_what_it_is_over() {
    let mut w = Window::headless(|| {
        col()
            .p_4()
            .gap_4()
            .child(text_input().id("field").w(200.0).h(30.0))
            .child(col().id("doc").selectable().child(text(rich().text("Read ").link("the docs", "https://x.dev")).id("para")))
            .child(text("Not selectable").id("plain"))
            .child(el().id("handle").size_8().cursor_grab().child(text("grip").id("grip")))
            .into()
    });
    fn cursor_at(w: &mut Window, (x, y): (f32, f32)) -> CursorStyle {
        w.pointer_move(x, y);
        w.platform_window().as_any().downcast_ref::<octara::platform::HeadlessWindow>().unwrap().cursor
    }
    let at = |w: &Window, id: &str| {
        let (x, y, ww, h) = w.kernel().shown_rect(w.kernel().find(id).unwrap());
        (x + ww.min(20.0) / 2.0, y + h / 2.0)
    };
    let over = |w: &mut Window, id: &str| {
        let p = at(w, id);
        cursor_at(w, p)
    };
    assert_eq!(over(&mut w, "field"), CursorStyle::IBeam, "a field");
    assert_eq!(over(&mut w, "para"), CursorStyle::IBeam, "selectable text");
    let link = w.kernel().find("the docs").or_else(|| w.kernel().find_by_name("the docs").first().copied()).unwrap();
    let (x, y, lw, h) = w.kernel().shown_rect(link);
    assert_eq!(cursor_at(&mut w, (x + lw / 2.0, y + h / 2.0)), CursorStyle::PointingHand, "a link");
    assert_eq!(over(&mut w, "plain"), CursorStyle::Arrow, "text outside a document");
    assert_eq!(over(&mut w, "grip"), CursorStyle::OpenHand, "set on an ancestor");
    assert_eq!(w.cursor(), CursorStyle::OpenHand);
    w.event(InputEvent::PointerLeave);
    assert_eq!(w.cursor(), CursorStyle::Arrow, "off the window");
}
