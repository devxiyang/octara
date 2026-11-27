use crate::{App, prelude::*};
use octara_model::input::ContactTarget;
use octara_model::style::Overflow;
use octara_runtime::{CommitOutput, HostEngine, ProjectError, ProjectFeedback};
use octara_tree::{NodeId, RuntimeOp, Tree, TreeDelta};
use std::{cell::Cell, rc::Rc};

struct Headless;

#[test]
fn element_builder_fits_in_one_pointer() {
    assert_eq!(size_of::<El>(), size_of::<usize>());
    assert_eq!(size_of::<TextInput>(), size_of::<usize>());
}

#[test]
fn style_patch_value_traits_and_combined_writes() {
    let step = sp(2.0);
    let patch = StylePatch::default().p(step).m(step).size(10).inset(0).flex_1().scroll_y().scroll_x().truncate();
    let expected = StylePatch::default()
        .pl(step)
        .pr(step)
        .pt(step)
        .pb(step)
        .ml(step)
        .mr(step)
        .mt(step)
        .mb(step)
        .w(10)
        .h(10)
        .top(0)
        .right(0)
        .bottom(0)
        .left(0)
        .grow(1.0)
        .shrink(1.0)
        .basis(0)
        .min_w(0)
        .min_h(0)
        .overflow_y(Overflow::Scroll)
        .min_h(0)
        .overflow_x(Overflow::Scroll)
        .min_w(0)
        .text_wrap(TextWrap::Truncate);
    assert_eq!(patch, expected);
    assert_eq!(patch.clone(), patch);
    assert_eq!(VirtualOptions::default(), VirtualOptions { estimated_row_height: 32.0, overscan: 4, maintain_scroll_anchor: false });
    assert_eq!(StylePatch::default().border(1), StylePatch::default().border_t(1).border_r(1).border_b(1).border_l(1));
    assert_eq!(StylePatch::default().border_x(2).border_y(3), StylePatch::default().border_l(2).border_r(2).border_t(3).border_b(3));
}

impl HostEngine for Headless {
    fn project(&mut self, tree: &Tree, delta: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        let updates = delta
            .scroll_dirty
            .iter()
            .filter_map(|&node| {
                let (_, offset) = tree.scroll(node)?.requested?;
                Some(RuntimeOp::SetScroll { node, offset, clamped: false })
            })
            .collect();
        Ok(ProjectFeedback { updates, ..Default::default() })
    }
    fn commit(&mut self) -> CommitOutput {
        CommitOutput::default()
    }
    fn rollback(&mut self) {}
    fn hit_test(&self, _: &Tree, _: Point) -> Option<ContactTarget> {
        None
    }
    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }
}

#[derive(Store)]
struct Model<T> {
    title: T,
}

#[ui]
fn titled_row(cx: &mut BuildCx<'_>, model: Signal<Model<String>>) -> El {
    text(cx.slice(model, Model::title())).test_id("heading")
}

#[test]
fn structure_regions_keep_sibling_order_and_keyed_identity() {
    let mut app = App::new(|_, _| Box::new(Headless));
    let (list, first, title) =
        app.build(|cx| (cx.list::<u64, String>(), cx.signal(true), cx.signal(Model { title: "heading".into() }))).unwrap();
    app.update(|w| {
        w.push_row(list, 1, "one".into());
        w.push_row(list, 2, "two".into());
    })
    .unwrap();
    let built = Rc::new(Cell::new(0));
    let counter = built.clone();
    let id = app
        .open_window(WindowOptions::default(), move |cx| {
            col()
                .test_id("parent")
                .child(show(move |r| r.get(first), |_| text("first").test_id("first")))
                .child(titled_row(cx, title))
                .child(each(list, move |_, key, item| {
                    counter.set(counter.get() + 1);
                    text(bind(move |r| item.read(r).unwrap_or_default())).test_id(key.to_string())
                }))
                .child(show(move |r| !r.get(first), |_| text("last").test_id("last")))
        })
        .unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    let original = tree.test_ids("1")[0];
    assert_eq!(tree.provenance(tree.test_ids("heading")[0]).unwrap().component.unwrap().name, "titled_row");
    app.update(|w| {
        w.set(first, false);
        w.move_row(list, &1, 1);
        w.update_row(list, &1, |value| value.push('!'));
        w.set_slice(title, Model::title(), "heading!".into());
    })
    .unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    let parent = tree.test_ids("parent")[0];
    let children: Vec<_> = tree.children(parent).iter().map(|&node| tree.provenance(node).unwrap().test_id.as_deref().unwrap()).collect();
    assert_eq!(children, ["heading", "2", "1", "last"]);
    assert_eq!(tree.test_ids("1")[0], original);
    assert_eq!(&*tree.text(original).unwrap().text, "one!");
    assert_eq!(&*tree.text(tree.test_ids("heading")[0]).unwrap().text, "heading!");
    assert_eq!(built.get(), 2);
}

#[test]
fn virtual_rows_stay_mounted_while_focused_and_reveal_by_key() {
    let mut app = App::new(|_, _| Box::new(Headless));
    let list = app.build(|cx| cx.list::<u64, ()>()).unwrap();
    app.update(|w| {
        for key in 0..100 {
            w.push_row(list, key, ());
        }
    })
    .unwrap();
    let mut options = WindowOptions::default();
    options.viewport.size.height = 40.0;
    let id = app
        .open_window(options, move |_| {
            col().h(40).overflow_y(Overflow::Scroll).test_id("rows").child(virtual_each(
                list,
                VirtualOptions { estimated_row_height: 20.0, overscan: 0, ..Default::default() },
                |_, key, _| el().focusable().h(20).test_id(format!("row-{key}")),
            ))
        })
        .unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    let first = tree.test_ids("row-0")[0];
    assert!(tree.test_ids("row-80").is_empty());
    app.update_window(id, |w| w.focus("row-0")).unwrap();
    app.update_window(id, |w| w.reveal_row("rows", 80)).unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    assert_eq!(tree.focus(), Some(first));
    assert_eq!(tree.test_ids("row-0"), &[first]);
    assert!(!tree.test_ids("row-80").is_empty());
    assert!(tree.test_ids("row-40").is_empty());
    app.update_window(id, |w| w.blur()).unwrap();
    assert!(app.window(id).unwrap().runtime().tree().test_ids("row-0").is_empty());
}

#[test]
fn retiring_nested_region_keeps_a_surviving_sibling_placement() {
    let mut app = App::new(|_, _| Box::new(Headless));
    let switch = app.build(|cx| cx.signal(false)).unwrap();
    let id = app
        .open_window(WindowOptions::default(), move |cx| {
            let outer = cx.memo(move |r| !r.get(switch));
            col()
                .child(show(move |r| r.get(outer), move |_| show(move |r| r.get(switch), |_| text("retiring").test_id("retiring"))))
                .child(show(move |r| r.get(switch), |_| text("alive").test_id("alive")))
        })
        .unwrap();
    app.update(|w| w.set(switch, true)).unwrap();
    let tree = app.window(id).unwrap().runtime().tree();
    assert!(tree.test_ids("retiring").is_empty());
    assert_eq!(tree.test_ids("alive").len(), 1);
}
