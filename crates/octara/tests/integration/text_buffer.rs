//! Text handles belong to their creating App, even when dropped outside its entry points.
use octara::platform::HeadlessPlatform;
use octara::prelude::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[test]
fn foreign_text_edits_leave_content_version_and_render_log_unchanged() {
    let app = App::new(HeadlessPlatform::default());
    let buffer = app.run(|| TextBuffer::new("original"));
    let rendered = buffer.clone();
    let window = app.open_window(WindowOptions::default(), move || text(&rendered).id("text").into()).unwrap();
    let foreign = App::new(HeadlessPlatform::default());
    for edit in 0..3 {
        assert!(
            catch_unwind(AssertUnwindSafe(|| foreign.run(|| match edit {
                0 => buffer.append(" foreign"),
                1 => buffer.splice(0..1, "foreign"),
                _ => buffer.set("foreign"),
            })))
            .is_err()
        );
        assert_eq!(app.run(|| buffer.get()), "original");
        assert_eq!(app.run(|| buffer.version()), 0);
        assert_eq!(buffer.log_len(), 0, "rejected edits must not reach incremental readers");
    }
    app.run(|| buffer.append(" accepted"));
    assert_eq!(app.run(|| buffer.version()), 1);
    assert_eq!(window.kernel().text_of("text").as_deref(), Some("original accepted"));
    app.shutdown();
    foreign.shutdown();
}

#[test]
fn last_text_handle_releases_its_node_without_an_app_entry() {
    let app = App::new(HeadlessPlatform::default());
    let before = app.work().reactive_nodes;
    for _ in 0..128 {
        let buffer = app.run(|| TextBuffer::new("temporary"));
        let last = buffer.clone();
        drop(buffer);
        assert_eq!(app.work().reactive_nodes, before + 1);
        assert_eq!(app.run(|| last.get()), "temporary");
        drop(last);
        assert_eq!(app.work().reactive_nodes, before);
    }
    app.shutdown();
}

#[test]
fn dropping_text_in_a_foreign_app_releases_only_its_own_node() {
    let owner = App::new(HeadlessPlatform::default());
    let buffer = owner.run(|| TextBuffer::new("owned"));
    let foreign = App::new(HeadlessPlatform::default());
    let other = foreign.run(|| TextBuffer::new("other"));
    foreign.run(|| drop(buffer));
    assert_eq!(owner.work().reactive_nodes, 0);
    assert_eq!(foreign.work().reactive_nodes, 1);
    assert_eq!(foreign.run(|| other.get()), "other");
    owner.shutdown();
    foreign.shutdown();
    drop(foreign);
    drop(other); // A handle can also outlive its runtime.
}
