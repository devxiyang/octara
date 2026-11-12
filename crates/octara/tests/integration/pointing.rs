//! What people and agents point at ([core](../../../../docs/design/core.md)): component
//! boundaries, numbered marks, picking by point and in a picking window, and why an element is
//! the size it is.

use octara::capture::{Painter, Picture};
use octara::inspect::{Node, Request, handle};
use octara::paint::{DisplayList, Item};
use octara::platform::HeadlessWindow;
use octara::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

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

fn walk(n: &Node) -> Vec<&Node> {
    n.walk()
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
    assert!(save.source.as_deref().is_some_and(|s| s.contains("pointing.rs:20:")), "its own line in the body: {:?}", save.source);
    assert!(made.called_at.contains("pointing.rs:15:"), "called in toolbar's body: {}", made.called_at);
    // Rows an `each` builds after `rows` returned are still `rows`'s.
    let k = w.kernel();
    assert_eq!(k.component(k.find("row-2").unwrap()).map(|c| c.name), Some("rows"));
    assert_eq!(k.component(k.find("rows").unwrap()).map(|c| c.name), Some("rows"));
}

#[test]
fn marks_number_what_can_be_addressed() {
    let w = Window::headless(app);
    let snap = w.inspect();
    let marked: Vec<(u32, Option<&str>)> = walk(&snap.root).into_iter().filter_map(|n| n.mark.map(|m| (m, n.id.as_deref()))).collect();
    let ids: Vec<Option<&str>> = marked.iter().map(|m| m.1).collect();
    // What can be done to, and where components start; not every element with an id.
    assert_eq!(ids, [Some("toolbar"), Some("save"), Some("rows")], "depth first");
    assert_eq!(marked.iter().map(|m| m.0).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(snap.marked(2).and_then(|n| n.id.as_deref()), Some("save"));
    // The numbers are the tree's: the same tree numbers the same.
    assert_eq!(Window::headless(app).inspect().marked(3).and_then(|n| n.id.clone()).as_deref(), Some("rows"));
}

/// A painter that keeps the lists it was given.
struct Keep(Rc<RefCell<Vec<DisplayList>>>);

impl Painter for Keep {
    fn renderer(&self) -> &'static str {
        "test"
    }
    fn paint(&mut self, list: &DisplayList, scale: f32) -> Result<Picture, String> {
        self.0.borrow_mut().push(list.clone());
        let (w, h) = ((list.size.0 * scale) as u32, (list.size.1 * scale) as u32);
        Ok(Picture { width: w, height: h, rgba: vec![0; (w * h * 4) as usize] })
    }
}

#[test]
fn a_marked_capture_draws_and_reports_the_numbers() {
    let mut w = Window::headless(app);
    let lists = Rc::new(RefCell::new(Vec::new()));
    w.app().set_painter(Box::new(Keep(lists.clone())));
    let shot = std::env::temp_dir().join(format!("octara-marks-{}.png", std::process::id())).to_string_lossy().into_owned();
    let plain = w.display_list().items.len();
    let reply =
        handle(&mut w, Request::Capture { path: shot.clone(), settle_ms: None, scale: Some(1.0), pixels: Default::default(), marks: true });
    std::fs::remove_file(&shot).unwrap();
    let report = reply.capture.expect("a report");
    assert_eq!(report.marks.len(), 3);
    assert_eq!((report.marks[1].mark, report.marks[1].id.as_deref(), report.marks[1].role.as_str()), (2, Some("save"), "button"));
    let painted = &lists.borrow()[0];
    // An outline, a badge and its digits for each.
    assert_eq!(painted.items.len(), plain + 3 * 3);
    assert!(matches!(painted.items.last(), Some(Item::Path(_))));
    assert_eq!(w.display_list().items.len(), plain, "the live frame has no marks");
}

#[test]
fn a_point_picks_an_element_with_where_it_comes_from() {
    let mut w = Window::headless(app);
    let save = w.inspect().find("save").unwrap().bounds;
    let reply = handle(&mut w, Request::Pick { x: save[0] + 4.0, y: save[1] + 4.0 });
    let p = reply.picked.expect("something is there");
    assert_eq!((p.node.id.as_deref(), p.node.mark), (Some("save"), Some(2)));
    assert_eq!(p.ancestors[0].id.as_deref(), Some("toolbar"));
    assert!(p.text.starts_with("#2 button \"Save\" (id save) at "), "{}", p.text);
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
    assert_eq!(w.display_list().items.len(), plain + 3, "the hovered element outlined with its number");
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

fn sized() -> View {
    row()
        .id("bar")
        .w(600.0)
        .h(100.0)
        .items_start()
        .child(el().id("fixed").w(120.0).h(40.0))
        .child(el().id("grow").flex_1().h(40.0).focusable())
        .child(col().id("capped").w(500.0).max_w(80.0).child(text("Hello").id("hello")))
        .into()
}

#[test]
fn a_size_is_explained() {
    let w = Window::headless(sized);
    let explain = |id: &str| {
        let k = w.kernel();
        k.explain_layout(k.find(id).unwrap())
    };
    let fixed = explain("fixed");
    assert_eq!(fixed.width.size, 120.0);
    assert!(fixed.width.because.contains("set") && fixed.width.rules == ["w(120)"], "{:?}", fixed.width);
    let parent = fixed.parent.as_ref().unwrap();
    assert_eq!((parent.id.as_deref(), parent.layout.as_str(), parent.align_items.as_str()), (Some("bar"), "row", "flex_start"));

    let grow = explain("grow");
    assert!(grow.width.because.contains("grows"), "{:?}", grow.width);
    assert!(grow.width.rules.contains(&"flex_grow(1)".to_owned()), "{:?}", grow.width.rules);

    let capped = explain("capped");
    assert_eq!(capped.width.size, 80.0);
    assert!(capped.width.because.contains("maximum"), "{:?}", capped.width);
    assert!(capped.height.because.contains("not stretched"), "align_items start: {:?}", capped.height);

    let hello = explain("hello");
    assert!(hello.width.because.contains("stretched"), "a column stretches its text: {:?}", hello.width);
    assert!(hello.width.content.is_some_and(|c| c > 0.0), "its natural width: {:?}", hello.width);

    // Through the protocol, by number.
    let mut w = Window::headless(sized);
    let n = w.inspect().find("grow").unwrap().mark.unwrap();
    let reply = handle(&mut w, Request::Layout { id: None, mark: Some(n) });
    assert_eq!(reply.layout.unwrap().id.as_deref(), Some("grow"));
    assert_eq!(handle(&mut w, Request::Layout { id: None, mark: None }).code.as_deref(), Some("BAD_REQUEST"));
}
