use octara::prelude::*;
use octara_examples::apps::{MSG_ROW_HEIGHT, chat_virtual};
use octara_examples::scenarios::external;

fn rows(h: &Window) -> Vec<String> {
    h.kernel().child_ids("messages").into_iter().filter(|id| id.starts_with("msg-")).collect()
}

fn spacer(h: &Window, name: &str) -> f32 {
    let k = h.kernel();
    let id = k.find(name).unwrap();
    k.rect(id).3
}

#[test]
fn only_the_window_exists_and_scrolling_moves_it() {
    let mut h = Window::headless(|| chat_virtual(100_000));
    let r = rows(&h);
    // Rows partly or fully inside the scroll container, plus 5 overscan on each side.
    let client = {
        let k = h.kernel();
        k.client_size(k.find("messages").unwrap()).1
    };
    assert!(client < 600.0, "the list gets what the header and composer leave: {client}");
    let visible = (client / MSG_ROW_HEIGHT).ceil() as usize;
    assert_eq!(r.len(), visible + 5, "{r:?}");
    assert_eq!(r[0], "msg-1");
    assert_eq!(spacer(&h, "messages-top"), 0.0);
    assert_eq!(spacer(&h, "messages-bottom"), (100_000 - visible - 5) as f32 * MSG_ROW_HEIGHT);

    h.dispatch(Event::ScrollTo("messages".into(), 0.0, 50_000.0 * MSG_ROW_HEIGHT)).unwrap();
    let r = rows(&h);
    assert_eq!(r.len(), 5 + visible + 5);
    assert_eq!(r[0], "msg-49996");
    assert!(!h.kernel().exists("msg-1"));
    assert_eq!(spacer(&h, "messages-top"), 49_995.0 * MSG_ROW_HEIGHT);
    // The estimate matches real layout: the first visible row sits right below the spacer.
    let k = h.kernel();
    let first = k.find("msg-50001").unwrap();
    let top = k.find("messages-top").unwrap();
    assert_eq!(k.rect(first).1, k.rect(top).1 + 5.0 * MSG_ROW_HEIGHT + k.rect(top).3);
}

#[test]
fn changes_outside_the_window_touch_nothing_visible() {
    let mut h = Window::headless(|| chat_virtual(10_000));
    external(&mut h, "user", "hi").unwrap(); // appends far below the window
    h.kernel_mut().reset_counters();
    for _ in 0..10 {
        external(&mut h, "token", "x").unwrap();
    }
    let c = h.kernel().counters();
    assert_eq!(c.prop_changes, 0, "an off-screen item change must not write any property");
    assert_eq!(c.nodes_created, 0);
    assert_eq!(h.kernel().text_of("header").as_deref(), Some("10002 messages"));
    // Scrolling to the end shows the streamed text.
    h.dispatch(Event::ScrollTo("messages".into(), 0.0, 1e9)).unwrap();
    assert_eq!(h.kernel().text_of("msg-body-10002").as_deref(), Some("xxxxxxxxxx"));
    assert!(h.app().diagnostics().is_empty(), "{:?}", h.app().diagnostics());
}

#[test]
fn hot_reload_keeps_scroll_and_window() {
    let mut h = Window::headless(|| chat_virtual(1_000));
    h.dispatch(Event::ScrollTo("messages".into(), 0.0, 3200.0)).unwrap();
    let before = h.kernel().tree_dump();
    h.app().hot_reload();
    assert_eq!(h.kernel().tree_dump(), before);
}
