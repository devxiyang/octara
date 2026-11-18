//! The product Chat UI with the same seeded history and transport as the live fixture.

#[path = "../examples/support/long_session.rs"]
mod long_session;

use long_session::{LongSession, seed};
use octara::app::Event;
use octara::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("octara-chat-long-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path).expect("create this test's isolated store");
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove only this test's isolated store");
    }
}

fn open(count: usize) -> (Directory, Window, LongSession) {
    let directory = Directory::new();
    let store = seed(&directory.0, count).unwrap();
    assert_eq!(store.load()[0].1.len(), count);
    let model = LongSession::default();
    let shown = model.clone();
    let mut window = Window::headless(move || shown.view(Some(store.clone())));
    window.resize(1000.0, 720.0);
    window.app().poll();
    window.app().settle_motion();
    (directory, window, model)
}

fn mounted_messages(window: &Window) -> Vec<String> {
    window.kernel().child_ids("transcript").into_iter().filter(|id| id.starts_with("msg-")).collect()
}

fn following(window: &Window) -> bool {
    let kernel = window.kernel();
    kernel.is_following_tail(kernel.find("transcript").unwrap())
}

fn frame(window: &Window) -> (f32, f32, f32, f32) {
    let kernel = window.kernel();
    kernel.shown_rect(kernel.find("transcript").unwrap())
}

fn anchor(window: &Window) -> (String, f32) {
    let kernel = window.kernel();
    let transcript = kernel.find("transcript").unwrap();
    let top = kernel.content_origin_y(transcript);
    mounted_messages(window)
        .into_iter()
        .find_map(|id| {
            let (_, y, _, height) = kernel.rect(kernel.find(&id).unwrap());
            (y <= top && y + height > top).then_some((id, y))
        })
        .expect("a retained message covers the transcript's top edge")
}

fn assert_visible(window: &Window, id: &str) {
    let kernel = window.kernel();
    let node = kernel.find(id).expect("the reading anchor remains mounted");
    let (_, y, _, height) = kernel.rect(node);
    let (_, top, _, viewport) = frame(window);
    assert!(y < top + viewport && y + height > top, "reading anchor {id} left the visible transcript: {y}..{}", y + height);
}

fn reading_offset(window: &Window, id: &str) -> f32 {
    let kernel = window.kernel();
    let transcript = kernel.find("transcript").unwrap();
    kernel.rect(kernel.find(id).expect("the reading anchor remains mounted")).1 - kernel.content_origin_y(transcript)
}

fn assert_reading_offset(window: &Window, id: &str, before: f32) {
    assert_eq!(anchor(window).0, id, "reflow must retain the same first visible business key");
    let height = {
        let kernel = window.kernel();
        kernel.rect(kernel.find(id).unwrap()).3
    };
    // A row shortened by wrapping can no longer hold an offset past its new end.
    let expected = before.max(-(height - 1.0).max(0.0));
    let after = reading_offset(window, id);
    assert!((after - expected).abs() < 0.5, "reflow moved the reading offset for {id}: {before} -> {after}, expected {expected}");
}

fn clean(window: &Window) {
    // The bounded picker intentionally ellipsizes this fixture's long model name; its full
    // value and hover tooltip remain available. Every other layout issue is still a failure.
    let issues = window.layout_issues();
    assert!(issues.iter().all(|issue| issue.code == "TEXT_TRUNCATED" && issue.test_id.as_deref() == Some("model")), "{issues:?}");
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

fn target(window: &Window, id: &str) -> String {
    let snapshot = window.inspect();
    snapshot.root.walk().into_iter().find(|node| node.id.as_deref() == Some(id)).expect("the retained node exists").target.clone()
}

fn value(window: &Window, id: &str) -> String {
    let kernel = window.kernel();
    kernel.value(kernel.find(id).unwrap()).to_owned()
}

fn visible_tool(window: &mut Window) -> String {
    for _ in 0..8 {
        let (x, top, width, height) = frame(window);
        let snapshot = window.inspect();
        let triggers: Vec<_> = snapshot
            .root
            .walk()
            .into_iter()
            .filter(|node| node.id.as_deref().is_some_and(|id| id.starts_with("tool-") && id.ends_with("-trigger")))
            .collect();
        if let Some(node) = triggers.iter().find(|node| node.bounds[1] >= top && node.bounds[1] + node.bounds[3] <= top + height) {
            return node.id.clone().unwrap();
        }
        // Markdown can be taller than the viewport: locate a retained header, then scroll it
        // into view instead of assuming a fixed wheel distance lands on one.
        let wanted = top + 48.0;
        let nearest = triggers
            .iter()
            .min_by(|a, b| (a.bounds[1] - wanted).abs().total_cmp(&(b.bounds[1] - wanted).abs()))
            .expect("overscan retains a historical tool header");
        window.wheel(x + width / 2.0, top + height / 2.0, 0.0, nearest.bounds[1] - wanted);
    }
    panic!("a measured historical tool header did not enter the visible transcript");
}

#[test]
fn both_history_sizes_use_the_product_virtual_transcript() {
    for count in [1_000, 10_000] {
        let (_directory, window, _) = open(count);
        let rows = mounted_messages(&window);
        assert!(!rows.is_empty() && rows.len() < 40, "{count} history messages mounted {} rows", rows.len());
        assert!(rows.contains(&format!("msg-{}", count + 1)), "the actual history's last answer is visible");
        assert!(following(&window));
        assert!(window.kernel().exists(&format!("tool-{}-1-trigger", count + 1)));
        clean(&window);
    }
}

#[test]
fn reading_survives_streaming_details_reflow_and_a_growing_composer() {
    let (_directory, mut window, model) = open(1_000);
    window.invoke("long.start").unwrap();
    window.invoke("long.stream.10").unwrap();
    window.app().settle_motion();
    assert_eq!(model.requests(), 1);
    assert_eq!(model.sent(), 10);
    assert!(model.sent_bytes() > 0);
    let (x, y, width, height) = frame(&window);
    window.wheel(x + width / 2.0, y + height / 2.0, 0.0, -1200.0);
    assert!(!following(&window));
    assert!(window.kernel().exists("jump"));

    let trigger = visible_tool(&mut window);
    window.tap(&trigger).unwrap();
    let content = format!("{}-content", trigger.strip_suffix("-trigger").unwrap());
    assert!(window.kernel().exists(&content), "the ordinary tool disclosure expanded");

    let (reading, old_y) = anchor(&window);
    window.invoke("long.stream.100").unwrap();
    window.app().settle_motion();
    assert_eq!(model.sent(), 110);
    let new_y = window.kernel().rect(window.kernel().find(&reading).unwrap()).1;
    assert!((new_y - old_y).abs() < 0.5, "off-screen streaming moved the reading anchor: {old_y} -> {new_y}");

    window.dispatch(Event::Input("composer".into(), "Retained draft".into())).unwrap();
    let composer_target = target(&window, "composer");
    for (width, height) in [(760.0, 640.0), (1000.0, 720.0), (480.0, 720.0), (1000.0, 720.0)] {
        let offset = reading_offset(&window, &reading);
        window.resize(width, height);
        window.app().settle_motion();
        assert!(!following(&window), "resize must not resume tail following");
        assert_visible(&window, &reading);
        assert_reading_offset(&window, &reading, offset);
        assert!(window.kernel().exists(&content), "the expanded historical tool remains open across breakpoints");
        assert_eq!(value(&window, "composer"), "Retained draft");
        assert_eq!(target(&window, "composer"), composer_target, "breakpoints retain the actual input node");
        clean(&window);
    }

    let offset = reading_offset(&window, &reading);
    let before = window.kernel().shown_rect(window.kernel().find("composer").unwrap()).3;
    window.dispatch(Event::Input("composer".into(), "line one\nline two\nline three\nline four".into())).unwrap();
    let after = window.kernel().shown_rect(window.kernel().find("composer").unwrap()).3;
    assert!(after > before, "the product composer grows through its multiline binding");
    assert!(!following(&window));
    assert_visible(&window, &reading);
    assert_reading_offset(&window, &reading, offset);
    window.tap("jump").unwrap();
    assert!(following(&window));
    window.invoke("long.chunk").unwrap();
    window.app().settle_motion();
    assert!(following(&window), "fresh chunks keep following after Jump");
    assert!(window.kernel().exists("msg-1003"), "the real Model answer is visible again");
    assert!(window.kernel().exists("text-1003-0"));
    assert!(!window.kernel().exists("jump"));
    clean(&window);
}

#[test]
fn compact_navigation_is_modal_and_desktop_resizing_survives() {
    let (_directory, mut window, _) = open(1_000);
    let composer_target = target(&window, "composer");
    window.drag("panes-divider", 32.0, 0.0).unwrap();
    assert_eq!(value(&window, "panes-divider"), "292");
    window.dispatch(Event::Focus("panes-divider".into())).unwrap();
    window.press_key("right").unwrap();
    assert_eq!(value(&window, "panes-divider"), "300");
    window.drag("panes-divider", 500.0, 0.0).unwrap();
    assert_eq!(value(&window, "panes-divider"), "420", "Chat owns its upper bound");
    window.drag("panes-divider", -500.0, 0.0).unwrap();
    assert_eq!(value(&window, "panes-divider"), "180", "Chat owns its lower bound");
    window.drag("panes-divider", 120.0, 0.0).unwrap();
    assert_eq!(value(&window, "panes-divider"), "300");
    window.resize(480.0, 720.0);
    assert!(!window.kernel().exists("panes-divider"));
    window.tap("conversations").unwrap();
    assert!(window.kernel().exists("chat-listing"));
    assert!(window.accessibility_tree().nodes.iter().any(|(_, node)| node.is_modal()));
    assert!(window.tap("composer").unwrap_err().contains("behind the open modal"));
    for _ in 0..3 {
        window.press_key("tab").unwrap();
        assert!(window.inspect().focus.is_some_and(|id| id == "new-chat" || id == "thread-1"), "Tab stays in the full-screen sidebar");
    }
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("chat-listing"));
    assert_eq!(window.inspect().focus.as_deref(), Some("conversations"));
    window.tap("conversations").unwrap();
    window.press_key("mod+l").unwrap();
    assert!(!window.kernel().exists("chat-listing"));
    assert_eq!(window.inspect().focus.as_deref(), Some("composer"));
    window.tap("conversations").unwrap();
    window.tap("new-chat").unwrap();
    assert!(!window.kernel().exists("chat-listing"));
    assert_eq!(window.kernel().text_of("title").as_deref(), Some("New chat"));
    window.tap("conversations").unwrap();
    window.tap("thread-1").unwrap();
    assert!(!window.kernel().exists("chat-listing"));
    assert_eq!(window.kernel().text_of("title").as_deref(), Some("Long session 1000"));
    window.resize(1000.0, 720.0);
    assert_eq!(value(&window, "panes-divider"), "300");
    assert_eq!(target(&window, "composer"), composer_target);
    clean(&window);
}
