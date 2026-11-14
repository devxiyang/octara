//! Correctness under queued real Model payloads; these are not native-loop latency tests.

#[path = "../examples/support/long_session.rs"]
mod long_session;

use long_session::{LongSession, MAX_REPLY_BYTES, MAX_REPLY_CHUNKS, Payload, Readback, seed};
use octara::prelude::*;
use octara_chat::model::Status;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Harness {
    directory: PathBuf,
    window: Window,
    model: LongSession,
}

impl Harness {
    fn new(payload: Payload) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory =
            std::env::temp_dir().join(format!("octara-chat-responsive-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let store = seed(&directory, 1000).unwrap();
        let model = LongSession::default();
        model.set_payload(payload, 1024).unwrap();
        let shown = model.clone();
        let mut window = Window::headless(move || shown.view(Some(store.clone())));
        window.resize(1000.0, 720.0);
        window.app().poll();
        window.invoke("chat.focus_composer").unwrap();
        window.type_text("Exercise the real model while I keep editing.");
        window.press_key("enter").unwrap();
        assert_eq!(model.requests(), 1);
        Self { directory, window, model }
    }

    fn read(&self) -> Readback {
        self.window.app().run(|| self.model.readback().expect("the live fixture captured the product Chat"))
    }

    fn drain(&self) -> Readback {
        for _ in 0..100 {
            let state = self.read();
            if state.consumed_text_bytes == state.sent_bytes {
                return state;
            }
            self.window.app().poll();
        }
        panic!("bounded queued payload did not reach the normal Chat message");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.model.pause();
        let _ = self.window.app().run_until_idle(Duration::from_secs(2));
        self.window.app().close_window(self.window.id());
        // Keep failed-test evidence instead of racing any unfinished background store write.
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
}

#[test]
fn queued_long_paragraph_and_fence_preserve_edits_selection_and_scroll() {
    for payload in [Payload::Paragraph, Payload::Code] {
        let mut h = Harness::new(payload);
        assert_eq!(h.model.emit_burst(64), 64);
        // The queue is populated before these product input operations. Their return may
        // include draining ready tasks; correctness here makes no CPU or frame-time promise.
        h.window.type_text("draft 😀");
        h.window.press_key("shift+left").unwrap();
        let field = h.window.text_field().unwrap();
        assert_eq!(field.text, "draft 😀");
        assert_eq!(field.selection, 6..10);
        assert!(field.reversed);
        let state = h.drain();
        assert_eq!(state.consumed_text_bytes, 64 * 1024);
        assert_eq!(state.status, Some(Status::Streaming));
        assert_eq!(state.draft, "draft 😀");
        let (x, y, width, height) = {
            let kernel = h.window.kernel();
            kernel.shown_rect(kernel.find("transcript").unwrap())
        };
        h.window.wheel(x + width / 2.0, y + height / 2.0, 0.0, -180.0);
        assert!(h.window.kernel().exists("jump"));
        assert_eq!(h.window.text_field().unwrap().selection, 6..10);
        assert_eq!(h.read().draft, "draft 😀");
        assert!(h.window.app().diagnostics().is_empty(), "{:?}", h.window.app().diagnostics());
    }
}

#[test]
fn stop_freezes_consumed_source_and_rejects_late_long_stream_events() {
    for payload in [Payload::Paragraph, Payload::Code] {
        let mut h = Harness::new(payload);
        assert_eq!(h.model.emit_burst(4), 4);
        let before = h.drain();
        assert_eq!(before.consumed_text_bytes, 4096);
        assert_eq!(h.model.emit_burst(64), 64);
        h.window.invoke("chat.stop").unwrap();
        let stopped = h.read();
        assert_eq!(stopped.status, Some(Status::Stopped));
        assert!(!stopped.answering);
        assert_eq!(stopped.consumed_text_bytes, before.consumed_text_bytes, "Stop retires queued, not-yet-consumed chunks");
        let sent = h.model.sent_bytes();
        assert!(!h.model.emit_chunk(), "the ordinary producer observes cancellation");
        h.model.attempt_late();
        h.window.app().poll();
        let late = h.read();
        assert_eq!(late.late_attempts, 2, "the transport actually tried text and Done");
        assert_eq!(late.status, Some(Status::Stopped));
        assert_eq!(late.consumed_text_bytes, stopped.consumed_text_bytes);
        assert_eq!(h.model.sent_bytes(), sent, "late attempts have separate counters");
        h.window.app().settle_motion();
        assert_eq!(h.read().consumed_text_bytes, stopped.consumed_text_bytes, "landing paced display cannot change accepted source");
    }
}

#[test]
fn fixture_caps_queued_bytes_before_any_presentation_work() {
    let mut h = Harness::new(Payload::Paragraph);
    for _ in 0..MAX_REPLY_CHUNKS / 256 {
        assert_eq!(h.model.emit_burst(256), 256);
    }
    assert_eq!(h.model.sent(), MAX_REPLY_CHUNKS);
    assert_eq!(h.model.sent_bytes(), MAX_REPLY_BYTES);
    assert!(!h.model.emit_chunk());
    h.window.invoke("chat.stop").unwrap();
    assert_eq!(h.read().consumed_text_bytes, 0, "queued stress data was retired before parsing/layout");
}

#[test]
fn explicit_rate_produces_outside_the_ui_clock_and_pause_retires_its_worker() {
    let h = Harness::new(Payload::Paragraph);
    h.model.set_rate(1000).unwrap();
    let clock = h.window.app().now();
    h.model.start_wall_producer().unwrap();
    // No App::advance or polling: only the bounded external worker can send these chunks.
    std::thread::sleep(Duration::from_millis(15));
    h.model.pause();
    let deadline = Instant::now() + Duration::from_secs(1);
    while h.model.workers() != 0 {
        assert!(Instant::now() < deadline, "cancelled worker did not retire");
        std::thread::yield_now();
    }
    assert!(h.model.sent() > 0);
    assert!(h.model.sent_bytes() <= MAX_REPLY_BYTES);
    assert_eq!(h.window.app().now(), clock, "wall-clock production never advances the app clock");
    let stopped = h.model.sent();
    std::thread::sleep(Duration::from_millis(3));
    assert_eq!(h.model.sent(), stopped);
}
