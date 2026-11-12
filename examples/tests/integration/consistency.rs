//! Every event of every scenario must leave the same tree as a from-scratch rebuild.

use octara::app::Window;
use octara_examples::apps;
use octara_examples::scenarios::{Factory, assert_all};

struct CheckedApps;

/// A window whose app checks every operation against a from-scratch rebuild.
fn checked(w: Window) -> Window {
    w.app().set_check_consistency(true);
    w
}

impl Factory for CheckedApps {
    const NAME: &'static str = "examples, checked against from-scratch";
    fn counter() -> Window {
        checked(Window::headless(apps::counter))
    }
    fn todo_with(n: usize) -> Window {
        checked(Window::headless(move || apps::todo(n)))
    }
    fn chat_with(n: usize) -> Window {
        checked(Window::headless(move || apps::chat(n)))
    }
    fn settings() -> Window {
        checked(Window::headless(apps::settings))
    }
}

#[test]
fn every_event_matches_a_from_scratch_rebuild() {
    assert_all::<CheckedApps>();
}
