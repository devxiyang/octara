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

#[test]
fn an_append_backlog_is_bounded_and_keeps_unicode_selection_when_it_resynchronizes() {
    const PREFIX: &str = "已选😀 ";
    let app = App::new(HeadlessPlatform::default());
    let buffer = app.run(|| TextBuffer::new(PREFIX));
    let rendered = buffer.clone();
    let mut window = app
        .open_window(WindowOptions::default(), move || {
            col().size_full().overflow_y_scroll().selectable().child(text(&rendered).id("text")).into()
        })
        .unwrap();
    window.select(("text", 0), ("text", "已选😀".len())).unwrap();
    let node = window.kernel().find("text").unwrap();
    app.run(|| {
        // More edits than the retained-log limit, before the reader can update.
        for _ in 0..5_000 {
            buffer.append("中🙂");
        }
        assert!(buffer.log_len() <= 4_096, "a mounted reader must not pin an unbounded edit history");
    });
    assert_eq!(window.kernel().text_of("text"), Some(format!("{PREFIX}{}", "中🙂".repeat(5_000))));
    assert_eq!(window.kernel().find("text"), Some(node));
    assert_eq!(window.selected_text().as_deref(), Some("已选😀"));
    app.run(|| buffer.append("恢复"));
    assert_eq!(window.selected_text().as_deref(), Some("已选😀"));
    assert!(window.kernel().text_of("text").unwrap().ends_with("🙂恢复"));
    assert_eq!(buffer.log_len(), 0, "the reader resumes consuming deltas after its reset");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn a_lost_splice_resynchronizes_every_text_reader_before_more_edits() {
    let app = App::new(HeadlessPlatform::default());
    let buffer = app.run(|| TextBuffer::new("旧😀 tail"));
    let rendered = buffer.clone();
    let window = app
        .open_window(WindowOptions::default(), move || {
            col().size_full().overflow_y_scroll().child(text(&rendered).id("first")).child(text(&rendered).id("second")).into()
        })
        .unwrap();
    let nodes = [window.kernel().find("first"), window.kernel().find("second")];
    app.run(|| {
        buffer.splice(0.."旧😀".len(), "新🙂");
        for _ in 0..5_000 {
            buffer.append("a");
        }
        assert!(buffer.log_len() <= 4_096);
    });
    let expected = format!("新🙂 tail{}", "a".repeat(5_000));
    for id in ["first", "second"] {
        assert_eq!(window.kernel().text_of(id).as_deref(), Some(expected.as_str()));
    }
    app.run(|| {
        buffer.splice("新".len().."新🙂".len(), "界");
        buffer.append("终");
    });
    let expected = format!("新界 tail{}终", "a".repeat(5_000));
    for (index, id) in ["first", "second"].into_iter().enumerate() {
        assert_eq!(window.kernel().text_of(id).as_deref(), Some(expected.as_str()));
        assert_eq!(window.kernel().find(id), nodes[index]);
    }
    assert_eq!(buffer.log_len(), 0);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}

#[test]
fn an_oversized_edit_does_not_remain_in_a_mounted_readers_log() {
    let app = App::new(HeadlessPlatform::default());
    let buffer = app.run(|| TextBuffer::new("旧😀"));
    let rendered = buffer.clone();
    let window = app.open_window(WindowOptions::default(), move || text(&rendered).id("text").into()).unwrap();
    app.run(|| {
        buffer.append(&"x".repeat(1_024 * 1_024 + 1));
        assert_eq!(buffer.log_len(), 0, "a single payload beyond the byte limit must be discarded");
        // Keep the final snapshot small; the UI must not replay this edit against its stale
        // text, whose byte offsets do not include the discarded oversized append.
        buffer.set("恢复🙂");
        buffer.append("完成");
    });
    assert_eq!(window.kernel().text_of("text").as_deref(), Some("恢复🙂完成"));
    app.run(|| buffer.splice(0.."恢复".len(), "继续"));
    assert_eq!(window.kernel().text_of("text").as_deref(), Some("继续🙂完成"));
    assert_eq!(buffer.log_len(), 0);
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
}
