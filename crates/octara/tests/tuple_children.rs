use octara::model::input::ContactTarget;
use octara::prelude::*;
use octara::tree::TreeDelta;
use octara::{CommitOutput, HostEngine, HostReply, Input, NodeId, ProjectError, ProjectFeedback, Tree};
use std::{cell::Cell, rc::Rc};

struct TestHost;

impl HostEngine for TestHost {
    fn project(&mut self, _: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        Ok(ProjectFeedback::default())
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

#[ui]
fn component_children() -> (El, TextInput) {
    (el().test_id("component-element"), input().test_id("component-input"))
}

fn assert_order(tree: &Tree, parent: NodeId, expected: &[&str]) {
    let ordered: Vec<_> = expected.iter().map(|id| tree.test_ids(id)[0]).collect();
    assert_eq!(tree.children(parent), ordered);
}

#[test]
fn mounts_all_tuple_arities_alongside_iterator_children() {
    let mut app = App::new(|_, _| Box::new(TestHost));
    let window = app
        .open_window(WindowOptions::default(), |_| {
            let groups = [
                el().child(()),
                el().child(("one",)),
                el().child(("one", input())),
                el().child(("one", input(), el())),
                el().child(("one", input(), el(), "four")),
                el().child(("one", input(), el(), "four", "five")),
                el().child(("one", input(), el(), "four", "five", "six")),
                el().child(("one", input(), el(), "four", "five", "six", "seven")),
                el().child(("one", input(), el(), "four", "five", "six", "seven", "eight")),
                el().child(("one", input(), el(), "four", "five", "six", "seven", "eight", "nine")),
                el().child(("one", input(), el(), "four", "five", "six", "seven", "eight", "nine", "ten")),
                el().child(("one", input(), el(), "four", "five", "six", "seven", "eight", "nine", "ten", "eleven")),
                el().child((
                    "one",
                    input().test_id("twelve-input"),
                    el().test_id("twelve-element"),
                    View::fragment(["four"]),
                    String::from("five"),
                    "six",
                    "seven",
                    "eight",
                    "nine",
                    "ten",
                    "eleven",
                    "twelve",
                )),
            ];
            col().children(groups.into_iter().enumerate().map(|(arity, group)| group.test_id(arity.to_string()))).child(
                col()
                    .test_id("component-parent")
                    .children([el().test_id("before")])
                    .child(component_children())
                    .children([el().test_id("after")]),
            )
        })
        .unwrap();
    let tree = app.window(window).unwrap().runtime().tree();
    for arity in 0..=12 {
        let group = tree.test_ids(&arity.to_string())[0];
        assert_eq!(tree.children(group).len(), arity);
    }
    let children = tree.children(tree.test_ids("12")[0]);
    assert_eq!(children[1], tree.test_ids("twelve-input")[0]);
    assert_eq!(children[2], tree.test_ids("twelve-element")[0]);
    let texts: Vec<_> = children.iter().filter_map(|&node| tree.text(node).map(|slot| slot.text.as_ref())).collect();
    assert_eq!(texts, ["one", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"]);

    let parent = tree.test_ids("component-parent")[0];
    assert_order(tree, parent, &["before", "component-element", "component-input", "after"]);
    for &child in &tree.children(parent)[1..=2] {
        assert_eq!(tree.provenance(child).unwrap().component.unwrap().name, "component_children");
    }
}

#[test]
fn tuple_branches_preserve_list_order_and_retirement() {
    let mut app = App::new(|_, _| Box::new(TestHost));
    let (shown, list) = app.build(|cx| (cx.signal(true), cx.list::<usize, ()>())).unwrap();
    app.update(|cx| {
        for key in 1..=3 {
            cx.push_row(list, key, ());
        }
    })
    .unwrap();
    let branch_cleanups = Rc::new(Cell::new(0));
    let row_cleanups = Rc::new(Cell::new(0));
    let window = app
        .open_window(WindowOptions::default(), {
            let branch_cleanups = branch_cleanups.clone();
            let row_cleanups = row_cleanups.clone();
            move |_| {
                col().test_id("parent").child((
                    el().test_id("before"),
                    show(
                        move |read| read.get(shown),
                        move |cx| {
                            let branch_cleanups = branch_cleanups.clone();
                            cx.on_cleanup(move |_| branch_cleanups.set(branch_cleanups.get() + 1));
                            let row_cleanups = row_cleanups.clone();
                            (
                                el().test_id("fade").style(StyleProp::OpacityMotion(Curve::Fade(0.2))),
                                each(list, move |cx, key, _| {
                                    let row_cleanups = row_cleanups.clone();
                                    cx.on_cleanup(move |_| row_cleanups.set(row_cleanups.get() + 1));
                                    el().test_id(format!("row-{key}"))
                                }),
                                el().test_id("branch-end"),
                            )
                        },
                    ),
                    input().test_id("after"),
                ))
            }
        })
        .unwrap();
    let tree = app.window(window).unwrap().runtime().tree();
    let parent = tree.test_ids("parent")[0];
    let [fade, first, removed, third, branch_end, after] =
        ["fade", "row-1", "row-2", "row-3", "branch-end", "after"].map(|id| tree.test_ids(id)[0]);
    assert_order(tree, parent, &["before", "fade", "row-1", "row-2", "row-3", "branch-end", "after"]);

    app.update(|cx| {
        cx.move_row(list, &3, 0);
        cx.remove_row(list, &2);
    })
    .unwrap();
    let tree = app.window(window).unwrap().runtime().tree();
    assert_order(tree, parent, &["before", "fade", "row-3", "row-1", "branch-end", "after"]);
    assert_eq!(tree.children(parent)[2..4], [third, first]);
    assert!(tree.node(removed).is_none());
    assert_eq!(row_cleanups.get(), 1);

    app.update(|cx| cx.set(shown, false)).unwrap();
    let tree = app.window(window).unwrap().runtime().tree();
    let ticket = tree.pending_exit(fade).expect("the visible exit is still pending");
    assert!(tree.test_ids("fade").is_empty());
    assert!([first, third, branch_end].into_iter().all(|node| tree.node(node).is_none()));
    assert_eq!(branch_cleanups.get(), 1);
    assert_eq!(row_cleanups.get(), 3);
    app.dispatch(window, Input::Host(HostReply::ExitFinished(ticket))).unwrap();
    let tree = app.window(window).unwrap().runtime().tree();
    assert_order(tree, parent, &["before", "after"]);
    assert!(tree.node(fade).is_none());

    app.update(|cx| cx.set(shown, true)).unwrap();
    let tree = app.window(window).unwrap().runtime().tree();
    assert_order(tree, parent, &["before", "fade", "row-3", "row-1", "branch-end", "after"]);
    assert_ne!(tree.test_ids("row-1")[0], first);
    assert_eq!(tree.test_ids("after")[0], after);
}
