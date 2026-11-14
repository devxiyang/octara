//! Real HTTP → SSE → Model channel → Chat state, with no virtual network time.

#[path = "support/network.rs"]
mod peer;

use octara::app::Window;
use octara::prelude::*;
use octara_chat::model::{Message, Status, ThreadId};
use octara_chat::network::{NetworkConfig, NetworkModel};
use octara_chat::{Chat, Options};
use peer::{End, Fixture, Plan};
use serde_json::json;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

type Action = Box<dyn FnOnce(&Chat)>;

struct Harness {
    window: Window,
    chat: Chat,
    action: Rc<RefCell<Option<Action>>>,
}

impl Harness {
    fn new(server: &Fixture, timeout: Duration) -> Self {
        Self::from_endpoint(server.endpoint(), timeout)
    }

    fn from_endpoint(endpoint: String, timeout: Duration) -> Self {
        let model = Rc::new(
            NetworkModel::new(NetworkConfig { endpoint, model: "fixture-model".into(), api_key: Some("test-only-key".into()), timeout })
                .unwrap(),
        );
        let exposed = Rc::new(RefCell::new(None));
        let slot = exposed.clone();
        let action: Rc<RefCell<Option<Action>>> = Rc::default();
        let command = action.clone();
        let window = Window::headless(move || {
            let chat = Chat::new(Options { models: vec![model.clone()], ..Options::default() });
            *slot.borrow_mut() = Some(chat.clone());
            let command = command.clone();
            on_command("test.chat", move || {
                if let Some(action) = command.borrow_mut().take() {
                    action(&chat);
                }
            });
            col().size_full().into()
        });
        let chat = exposed.borrow_mut().take().unwrap();
        Self { window, chat, action }
    }

    fn act(&mut self, action: impl FnOnce(&Chat) + 'static) {
        *self.action.borrow_mut() = Some(Box::new(action));
        self.window.invoke("test.chat").unwrap();
        self.window.app().poll();
    }

    fn send(&mut self, question: &str) -> ThreadId {
        let question = question.to_owned();
        self.act(move |chat| {
            chat.draft.set(question);
            chat.send();
        });
        self.read(|chat| chat.current.get_untracked().unwrap())
    }

    fn read<T>(&self, read: impl FnOnce(&Chat) -> T) -> T {
        self.window.app().run(|| read(&self.chat))
    }

    fn answer(&self, thread: ThreadId) -> Message {
        self.read(|chat| chat.thread_messages(thread).last().unwrap().clone())
    }

    fn until(&self, server: &Fixture, label: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.window.app().poll();
            let errors = server.inspect(|s| s.errors.clone());
            assert!(errors.is_empty(), "fixture errors: {errors:?}");
            if ready() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {label}");
            server.wait_for_progress();
        }
    }

    fn done(&self, server: &Fixture, thread: ThreadId) {
        self.until(server, "a terminal Chat status", || self.answer(thread).status != Status::Streaming);
        assert!(self.window.app().diagnostics().is_empty(), "{:?}", self.window.app().diagnostics());
    }
}

const TIMEOUT: Duration = Duration::from_secs(3);

#[test]
fn fragmented_unicode_sse_reaches_chat_and_followups_send_the_real_history() {
    // HTTP chunks split JSON, SSE CRLF, and the UTF-8 bytes inside 中文/emoji.
    let mut bytes = b": heartbeat\r\n\r\n".to_vec();
    bytes.extend(peer::text("你好，"));
    bytes.extend(peer::text("world 😀"));
    bytes.extend_from_slice(b"data: [DONE]\r\n\r\n");
    let server = Fixture::new(vec![
        Plan { status: 200, chunks: bytes.chunks(3).map(<[u8]>::to_vec).collect(), end: End::Complete },
        Plan::success("second answer"),
    ]);
    let mut h = Harness::new(&server, TIMEOUT);
    let thread = h.send("first question 中文");
    h.done(&server, thread);
    assert_eq!(h.answer(thread).status, Status::Done);
    assert_eq!(h.answer(thread).text(), "你好，world 😀");
    h.send("follow up");
    h.done(&server, thread);
    server.inspect(|s| {
        assert_eq!(s.requests.len(), 2, "local titles do not make a second HTTP request");
        assert_eq!(s.requests[1]["model"], "fixture-model");
        assert_eq!(s.requests[1]["stream"], true);
        assert_eq!(
            s.requests[1]["messages"],
            json!([
                {"role":"user","content":"first question 中文"},
                {"role":"assistant","content":"你好，world 😀"},
                {"role":"user","content":"follow up"},
            ])
        );
    });
}

#[test]
fn http_errors_are_terminal_and_do_not_expose_the_response_body() {
    for status in [401, 429, 503] {
        let server = Fixture::new(vec![Plan { status, chunks: vec![], end: End::Complete }]);
        let mut h = Harness::new(&server, TIMEOUT);
        let thread = h.send("question");
        h.done(&server, thread);
        let Status::Failed(error) = h.answer(thread).status else { panic!("HTTP {status} must fail") };
        assert!(error.contains(&status.to_string()), "{error}");
        assert!(!error.contains("private upstream"), "response bodies stay outside the conversation");
        assert!(!error.contains("test-only-key"));
        assert!(h.answer(thread).text().is_empty());
    }
}

#[test]
fn a_refused_loopback_connection_ends_the_chat_answer_without_exposing_the_endpoint() {
    // Reserve a dynamic port, then close its listener before the actual request.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let endpoint = format!("http://{}/v1/chat/completions?private=fixture-secret", listener.local_addr().unwrap());
    let observer = Fixture::new(vec![]);
    let mut h = Harness::from_endpoint(endpoint.clone(), TIMEOUT);
    drop(listener);
    let clock = h.window.app().now();
    let thread = h.send("connect to the closed port");
    // Reuse the bounded poll/progress wait; this separate peer must receive no HTTP.
    h.done(&observer, thread);
    let Status::Failed(error) = h.answer(thread).status else { panic!("a refused connection must fail") };
    assert!(error.to_lowercase().contains("connect"), "{error}");
    assert!(!error.contains(&endpoint));
    assert!(!error.contains("fixture-secret"));
    assert!(!error.contains("test-only-key"));
    assert!(h.answer(thread).text().is_empty());
    assert!(!h.read(|chat| chat.is_answering(thread)));
    assert_eq!(h.window.app().now(), clock, "network failures use real time");
    assert_eq!(observer.inspect(|s| s.requests.len()), 0);
}

#[test]
fn a_dropped_stream_keeps_its_prefix_and_regeneration_replaces_it() {
    let server = Fixture::new(vec![
        Plan { status: 200, chunks: vec![peer::text("received 中文")], end: End::Disconnect },
        Plan::success("complete replacement"),
    ]);
    let mut h = Harness::new(&server, TIMEOUT);
    let thread = h.send("regenerate this");
    h.done(&server, thread);
    assert!(matches!(h.answer(thread).status, Status::Failed(_)));
    assert_eq!(h.answer(thread).text(), "received 中文");
    let answer = h.answer(thread).id;
    h.act(move |chat| chat.regenerate(answer));
    h.done(&server, thread);
    assert_eq!(h.answer(thread).text(), "complete replacement");
    assert_eq!(h.answer(thread).status, Status::Done);
    server.inspect(|s| {
        assert_eq!(s.requests.len(), 2);
        assert_eq!(s.requests[0]["messages"], s.requests[1]["messages"], "regenerate sends the original question history");
    });
}

#[test]
fn an_unterminated_sse_event_cannot_become_a_successful_answer() {
    let mut partial = peer::text("kept");
    partial.extend_from_slice(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"unfinished");
    let server = Fixture::new(vec![Plan { status: 200, chunks: vec![partial], end: End::Complete }]);
    let mut h = Harness::new(&server, TIMEOUT);
    let thread = h.send("incomplete event");
    h.done(&server, thread);
    assert!(matches!(h.answer(thread).status, Status::Failed(_)));
    assert_eq!(h.answer(thread).text(), "kept");
}

#[test]
fn stop_before_the_first_token_and_during_a_stream_closes_the_http_peer() {
    for (plan, prefix) in
        [(Plan::before_headers(), None), (Plan::hold(None), None), (Plan::hold(Some("partial answer")), Some("partial answer"))]
    {
        let server = Fixture::new(vec![plan, Plan::success("fresh answer")]);
        let mut h = Harness::new(&server, TIMEOUT);
        let thread = h.send("old request");
        h.until(&server, "the first HTTP request", || server.inspect(|s| s.requests.len() == 1));
        if let Some(prefix) = prefix {
            h.until(&server, "the streamed prefix", || h.answer(thread).text() == prefix);
        }
        h.act(move |chat| chat.stop(thread));
        assert_eq!(h.answer(thread).status, Status::Stopped);
        h.send("new request");
        h.done(&server, thread);
        h.until(&server, "the cancelled socket to close", || server.inspect(|s| s.closed.contains(&0)));
        assert_eq!(h.answer(thread).status, Status::Done);
        assert_eq!(h.answer(thread).text(), "fresh answer", "retired data cannot contaminate the new answer");
        assert_eq!(h.read(|chat| chat.thread_messages(thread)[1].text()), prefix.unwrap_or_default());
    }
}

#[test]
fn closing_the_chat_scope_cancels_an_inflight_socket_with_the_app_still_alive() {
    let server = Fixture::new(vec![Plan::hold(None)]);
    let mut h = Harness::new(&server, TIMEOUT);
    h.send("close the owner");
    h.until(&server, "an open HTTP request", || server.inspect(|s| s.requests.len() == 1));
    let app = h.window.app();
    let survivor = app.open_window(octara::platform::WindowOptions::new("survivor"), || col().size_full().into()).unwrap();
    app.close_window(h.window.id());
    h.until(&server, "scope cancellation to close HTTP", || server.inspect(|s| s.closed.contains(&0)));
    assert!(h.window.is_closed());
    assert!(!survivor.is_closed());
}

#[test]
fn an_idle_http_stream_times_out_without_advancing_the_app_clock() {
    let server = Fixture::new(vec![Plan::hold(Some("kept before idle"))]);
    let mut h = Harness::new(&server, Duration::from_millis(150));
    let clock = h.window.app().now();
    let thread = h.send("stall after first token");
    h.done(&server, thread);
    assert!(matches!(h.answer(thread).status, Status::Failed(_)));
    assert_eq!(h.answer(thread).text(), "kept before idle");
    assert_eq!(h.window.app().now(), clock, "wall-clock network timeout is not a virtual-time advance");
    h.until(&server, "the idle request to close", || server.inspect(|s| s.closed.contains(&0)));
}
