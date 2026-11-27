use octara::model::geometry::{LayoutBox, Point, WindowViewport};
use octara::model::input::ContactTarget;
use octara::prelude::*;
use octara::tree::TreeDelta;
use octara::{CommitOutput, HostEngine, NodeId, ProjectError, ProjectFeedback, Tree};
use std::{
    cell::Cell,
    future::Future,
    num::ParseIntError,
    pin::pin,
    task::{Context, Poll, Waker},
};

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
fn branch(variant: u8) -> impl IntoView {
    if variant > 0 {
        return '__octara_body: {
            if variant > 1 {
                return el().test_id("nested-early");
            }
            break '__octara_body el().test_id("early");
        };
    }
    el().test_id("tail")
}

#[ui]
fn optional(message: Option<&str>) -> Option<El> {
    Some(text(message?).test_id("optional"))
}

#[ui]
fn macro_view(early: bool) -> El {
    macro_rules! view {
        ($id:literal) => {
            el().test_id($id)
        };
    }
    macro_rules! maybe_return {
        () => {
            if early {
                return view!("macro-early");
            }
        };
    }
    maybe_return!();
    view!("macro-tail")
}

fn poll_once<T>(future: impl Future<Output = T>) -> T {
    let Poll::Ready(value) = pin!(future).poll(&mut Context::from_waker(Waker::noop())) else { panic!("future should be ready") };
    value
}

#[ui]
fn nested(early: bool) -> El {
    fn helper(early: bool) -> &'static str {
        if early {
            return "fn";
        }
        "tail"
    }
    struct Helper;
    impl Helper {
        fn method(early: bool) -> &'static str {
            if early {
                return "method";
            }
            "tail"
        }
    }
    let closure = |input: &str| -> Result<&'static str, ParseIntError> {
        let value: u8 = input.parse()?;
        if value == 1 {
            return Ok("closure");
        }
        Ok("tail")
    };
    let asynchronous = poll_once(async {
        if early {
            return "async";
        }
        "tail"
    });
    let residual = if closure("invalid").is_err() { "residual" } else { "unexpected success" };
    text([helper(early), Helper::method(early), closure("1").unwrap(), asynchronous, residual].join(" ")).test_id("nested")
}

#[test]
fn components_mount_through_tail_early_and_nested_control_flow() {
    let tail_line = line!() + 1;
    let tail = branch(0);
    let early_line = line!() + 1;
    let early = branch(1);
    let nested_early_line = line!() + 1;
    let nested_early = branch(2);
    assert!(optional(None).is_none());

    let mut app = App::new(|_, _| Box::new(TestHost));
    let window = app
        .open_window(WindowOptions::default(), move |_| {
            col()
                .child(tail)
                .child(early)
                .child(nested_early)
                .child(optional(Some("hello")))
                .child(nested(true))
                .child(macro_view(false))
                .child(macro_view(true))
        })
        .unwrap();
    let tree = app.window(window).unwrap().runtime().tree();
    let node = |test_id: &str| tree.test_ids(test_id)[0];
    let component = |test_id: &str| tree.provenance(node(test_id)).unwrap().component.unwrap();
    for (test_id, line) in [("tail", tail_line), ("early", early_line), ("nested-early", nested_early_line)] {
        let component = component(test_id);
        assert_eq!(component.name, "branch");
        assert_eq!(component.module, module_path!());
        assert_eq!(component.at.file(), file!());
        assert_eq!(component.at.line(), line);
    }
    assert_eq!(tree.text(node("nested")).unwrap().text.as_ref(), "fn method closure async residual");
    assert_eq!(component("nested").name, "nested");
    assert_eq!(component("optional").name, "optional");
    assert_eq!(component("macro-tail").name, "macro_view");
    assert!(tree.provenance(node("macro-early")).unwrap().component.is_none());
}

struct MarkDropped<'a>(&'a Cell<bool>);

impl Drop for MarkDropped<'_> {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

struct InspectCleanup<'a>(&'a Cell<bool>);

impl IntoView for InspectCleanup<'_> {
    fn into_view(self) -> View {
        View::empty()
    }

    fn with_component(self, _: Component) -> Self {
        assert!(self.0.get());
        self
    }
}

#[ui]
fn cleanup(dropped: &Cell<bool>, early: bool) -> InspectCleanup<'_> {
    let _guard = MarkDropped(dropped);
    if early {
        return InspectCleanup(dropped);
    }
    InspectCleanup(dropped)
}

#[test]
fn body_locals_drop_before_component_provenance_is_attached() {
    for early in [false, true] {
        let dropped = Cell::new(false);
        cleanup(&dropped, early);
        assert!(dropped.get());
    }
}

#[ui]
fn mark(early: bool, seen: &Cell<bool>) {
    if early {
        return;
    }
    seen.set(true);
}

#[test]
fn bare_return_exits_a_unit_component() {
    let seen = Cell::new(false);
    mark(true, &seen);
    assert!(!seen.get());
    mark(false, &seen);
    assert!(seen.get());
}
