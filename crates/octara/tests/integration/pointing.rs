//! What people point at ([core](../../../../docs/design/core.md)): component boundaries,
//! picking by point and in a picking window.

use octara::inspect::{Request, handle};
use octara::platform::HeadlessWindow;
use octara::prelude::*;

#[component]
fn toolbar(saved: Signal<u32>) -> El {
    row().id("toolbar").gap_2().child(save_button(saved)).child(text(move || format!("saved {}", saved.get())).id("count"))
}

#[component]
fn save_button(saved: Signal<u32>) -> El {
    el().id("save").role(Role::Button).label("Save").size_8().on_press(move || saved.update(|s| *s += 1))
}

#[component]
fn rows(items: List<u32, (u32, String)>) -> El {
    col().id("rows").child(each(move || items.source(), |k, t| text(move || t.with(|t| t.1.clone())).id(format!("row-{k}")).into()))
}

fn app() -> View {
    let saved = state("saved", || 0u32);
    let items = list("items", || vec![(1u32, "one".to_owned()), (2, "two".to_owned())], |t: &(u32, String)| t.0);
    col().child(toolbar(saved)).child(rows(items)).into()
}

#[test]
fn components_mark_their_boundaries() {
    let w = Window::headless(app);
    let snap = w.inspect();
    let toolbar = snap.find("toolbar").unwrap();
    let c = toolbar.component.as_ref().expect("the toolbar's root starts a component");
    assert_eq!((c.name.as_str(), c.module.as_str()), ("toolbar", "integration::pointing"));
    assert!(c.called_at.contains("pointing.rs:"), "where it was called: {}", c.called_at);
    assert_eq!(snap.find("count").unwrap().component, None, "inside the same component: no boundary");
    let save = snap.find("save").unwrap();
    let made = save.component.as_ref().unwrap();
    assert_eq!(made.name, "save_button");
    assert!(save.source.as_deref().is_some_and(|s| s.contains("pointing.rs:15:")), "its own line in the body: {:?}", save.source);
    assert!(made.called_at.contains("pointing.rs:10:"), "called in toolbar's body: {}", made.called_at);
    // Rows an `each` builds after `rows` returned are still `rows`'s.
    let k = w.kernel();
    assert_eq!(k.component(k.find("row-2").unwrap()).map(|c| c.name), Some("rows"));
    assert_eq!(k.component(k.find("rows").unwrap()).map(|c| c.name), Some("rows"));
}

#[test]
fn a_point_picks_an_element_with_where_it_comes_from() {
    let mut w = Window::headless(app);
    let save = w.inspect().find("save").unwrap().bounds;
    let reply = handle(&mut w, Request::Pick { x: save[0] + 4.0, y: save[1] + 4.0 });
    let p = reply.picked.expect("something is there");
    assert_eq!(p.node.id.as_deref(), Some("save"));
    assert_eq!(p.ancestors[0].id.as_deref(), Some("toolbar"));
    assert!(p.text.starts_with("button \"Save\" (id save) at "), "{}", p.text);
    assert!(p.text.contains("made by save_button() called at ") && p.text.contains(" in toolbar() called at "), "{}", p.text);
    assert_eq!(handle(&mut w, Request::Pick { x: -5.0, y: -5.0 }).code.as_deref(), Some("NO_SUCH_ELEMENT"));
}

#[test]
fn a_picking_window_outlines_then_picks_instead_of_pressing() {
    let mut w = Window::headless(app);
    let save = w.inspect().find("save").unwrap().bounds;
    let (x, y) = (save[0] + 4.0, save[1] + 4.0);
    let plain = w.display_list().items.len();
    w.set_picking(true);
    assert!(w.picking());
    w.pointer_move(x, y);
    assert_eq!(w.display_list().items.len(), plain + 1, "the hovered element outlined");
    w.click_at(x, y);
    assert!(!w.picking(), "a pick ends picking");
    assert_eq!(w.kernel().text_of("count").as_deref(), Some("saved 0"), "the click picked, it did not press");
    let picked = {
        let p = w.platform_window();
        p.as_any().downcast_ref::<HeadlessWindow>().unwrap().picked.clone()
    };
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].node.id.as_deref(), Some("save"));
    let clipboard = w.app().platform().read_text();
    assert_eq!(clipboard.as_deref(), Some(picked[0].text.as_str()), "its line is on the clipboard");
    assert_eq!(w.display_list().items.len(), plain);
    w.tap("save").unwrap();
    assert_eq!(w.kernel().text_of("count").as_deref(), Some("saved 1"), "after picking, input reaches the app");

    w.set_picking(true);
    w.press_key("escape").unwrap_or_default();
    assert!(!w.picking(), "Escape ends picking");
}

#[test]
fn a_recorded_pick_replays() {
    let mut a = Window::headless(app);
    let save = a.inspect().find("save").unwrap().bounds;
    a.app().start_recording();
    a.set_picking(true);
    a.click_at(save[0] + 4.0, save[1] + 4.0);
    a.tap("save").unwrap();
    let recording = a.app().stop_recording();
    let b = Window::headless(app);
    b.app().replay(&recording).unwrap();
    assert_eq!(b.kernel().text_of("count").as_deref(), Some("saved 1"), "the picked click did not press in the replay either");
}
