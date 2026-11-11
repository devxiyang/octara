//! Every event of every scenario must leave the same tree as a from-scratch rebuild.

use octara::app::App;
use octara::testing::{Checked, Host};
use octara_examples::apps;
use octara_examples::scenarios::{Factory, assert_all};

struct CheckedApps;

impl Factory for CheckedApps {
    const NAME: &'static str = "examples, checked against from-scratch";
    fn counter() -> Box<dyn Host> {
        Box::new(Checked(App::new(apps::counter)))
    }
    fn todo_with(n: usize) -> Box<dyn Host> {
        Box::new(Checked(App::new(move || apps::todo(n))))
    }
    fn chat_with(n: usize) -> Box<dyn Host> {
        Box::new(Checked(App::new(move || apps::chat(n))))
    }
    fn settings() -> Box<dyn Host> {
        Box::new(Checked(App::new(apps::settings)))
    }
}

#[test]
fn every_event_matches_a_from_scratch_rebuild() {
    assert_all::<CheckedApps>();
}
