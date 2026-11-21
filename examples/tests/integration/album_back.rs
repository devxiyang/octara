use octara::app::{BackPhase, InputEvent};
use octara::inspect::{self, Delivery, Request};
use octara::prelude::Window;
use octara::serde_json::json;
use octara_examples::album_back::{self, BACK, COUNTERS, DETAIL, DETAIL_COUNTERS, DETAIL_SCROLL, LIST};

fn back(window: &mut Window, id: u64, phase: BackPhase) -> bool {
    window.event(InputEvent::BackGesture { id, phase })
}

fn target(window: &Window, name: &str) -> String {
    window.inspect().find(name).unwrap().target.clone()
}

fn scroll(window: &mut Window, name: &str, dy: f32) -> f32 {
    let reply = inspect::handle(window, Request::Scroll { id: Some(name.into()), dx: 0.0, dy });
    assert!(reply.ok, "{reply:?}");
    reply.scroll.unwrap().offset[1]
}

fn offset(window: &Window, name: &str) -> f32 {
    let kernel = window.kernel();
    kernel.scroll_offset(kernel.find(name).unwrap()).1
}

fn assert_back(window: &Window, id: u64, phase: BackPhase, delivered: bool) {
    let observed = window.inspect().back_gesture.unwrap();
    assert_eq!((observed.id, observed.phase, observed.delivered), (id, phase, delivered));
}

#[test]
fn a_cancelled_back_keeps_both_pages_and_a_completed_back_releases_only_the_detail() {
    let mut window = Window::headless(album_back::view);
    window.resize(390.0, 640.0);
    assert!(window.back_gesture_target().is_none());
    let list = target(&window, LIST);
    let list_offset = scroll(&mut window, LIST, 224.0);
    assert!(list_offset > 0.0);

    let photo = "album-photo-6";
    let opener = target(&window, photo);
    window.tap(photo).unwrap();
    assert!(window.back_gesture_target().is_some());
    let detail = target(&window, DETAIL);
    let detail_scroll = target(&window, DETAIL_SCROLL);
    let back_button = target(&window, BACK);
    let detail_offset = scroll(&mut window, DETAIL_SCROLL, 160.0);
    assert!(detail_offset > 0.0);

    assert!(back(&mut window, 1, BackPhase::Started));
    assert!(back(&mut window, 1, BackPhase::Progress(0.65)));
    assert_back(&window, 1, BackPhase::Progress(0.65), false);
    assert_eq!(target(&window, DETAIL), detail);
    assert_eq!(offset(&window, DETAIL_SCROLL), detail_offset);
    assert_eq!(window.kernel().text_of(DETAIL_COUNTERS).as_deref(), Some("Back 0 · Released 0"));

    assert!(back(&mut window, 1, BackPhase::Progress(0.1)));
    assert_back(&window, 1, BackPhase::Progress(0.1), false);
    assert!(back(&mut window, 1, BackPhase::Cancelled));
    assert!(!back(&mut window, 1, BackPhase::Completed));
    assert_back(&window, 1, BackPhase::Cancelled, false);
    assert_eq!(target(&window, BACK), back_button);
    assert_eq!(target(&window, DETAIL), detail);
    assert_eq!(target(&window, DETAIL_SCROLL), detail_scroll);
    assert_eq!(window.inspect().focus.as_deref(), Some(DETAIL_SCROLL));
    assert_eq!(offset(&window, DETAIL_SCROLL), detail_offset);
    assert_eq!(offset(&window, LIST), list_offset);
    assert_eq!(window.kernel().text_of(DETAIL_COUNTERS).as_deref(), Some("Back 0 · Released 0"));

    assert!(back(&mut window, 2, BackPhase::Started));
    assert!(back(&mut window, 2, BackPhase::Progress(0.8)));
    assert!(window.event(InputEvent::Active(false)));
    assert_back(&window, 2, BackPhase::Cancelled, false);
    assert_eq!(target(&window, DETAIL), detail);
    assert_eq!(window.inspect().focus.as_deref(), Some(DETAIL_SCROLL));
    assert_eq!(offset(&window, DETAIL_SCROLL), detail_offset);
    assert_eq!(window.kernel().text_of(DETAIL_COUNTERS).as_deref(), Some("Back 0 · Released 0"));
    assert!(!back(&mut window, 2, BackPhase::Progress(0.9)));

    assert!(window.event(InputEvent::Active(true)));
    assert!(back(&mut window, 3, BackPhase::Started));
    assert!(!back(&mut window, 2, BackPhase::Completed));
    assert_back(&window, 3, BackPhase::Started, false);
    assert!(back(&mut window, 3, BackPhase::Progress(0.8)));
    assert!(back(&mut window, 3, BackPhase::Completed));
    assert!(!back(&mut window, 3, BackPhase::Completed));
    assert_back(&window, 3, BackPhase::Completed, true);
    assert!(window.back_gesture_target().is_none());
    assert!(!window.kernel().exists(DETAIL));
    assert_eq!(target(&window, LIST), list);
    assert_eq!(target(&window, photo), opener);
    assert_eq!(window.inspect().focus.as_deref(), Some(photo));
    assert_eq!(offset(&window, LIST), list_offset);
    assert_eq!(window.kernel().text_of(COUNTERS).as_deref(), Some("Back 1 · Released 1"));

    let reply = inspect::handle_line(&mut window, &json!({"op": "tap", "target": back_button}).to_string());
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(reply.delivery, Some(Delivery::Refused));
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

#[test]
fn finishing_a_swipe_cannot_close_a_reopened_detail() {
    let mut window = Window::headless(album_back::view);
    window.tap("album-photo-1").unwrap();
    let first = target(&window, DETAIL);
    assert!(back(&mut window, 1, BackPhase::Started));

    window.tap(BACK).unwrap();
    window.tap("album-photo-1").unwrap();
    assert_ne!(target(&window, DETAIL), first);
    assert!(!back(&mut window, 1, BackPhase::Completed));
    assert!(window.back_gesture_target().is_some());
    assert_eq!(window.kernel().text_of(DETAIL_COUNTERS).as_deref(), Some("Back 1 · Released 1"));
}
