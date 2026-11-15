//! Real HTTP → SSE → Model channel → Chat state, with no virtual network time.

#[path = "support/network.rs"]
mod peer;

use octara::app::Window;
use octara::prelude::*;
use octara_chat::model::{Message, Status, ThreadId};
use octara_chat::network::{NetworkConfig, NetworkModel};
use octara_chat::{Chat, ModelEntry, Options};
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
        Self::from_models(vec![ModelEntry::new("fixture", model)])
    }

    fn from_models(models: Vec<ModelEntry>) -> Self {
        let exposed = Rc::new(RefCell::new(None));
        let slot = exposed.clone();
        let action: Rc<RefCell<Option<Action>>> = Rc::default();
        let command = action.clone();
        let window = Window::headless(move || {
            let chat = Chat::new(Options { models: models.clone(), ..Options::default() });
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
        self.read(|chat| untrack(|| chat.current.get()).unwrap())
    }

    fn read<T>(&self, read: impl FnOnce(&Chat) -> T) -> T {
        self.window.app().run(|| read(&self.chat))
    }

    fn answer(&self, thread: ThreadId) -> Message {
        self.read(|chat| chat.thread_messages(thread).last().unwrap().clone())
    }

    fn answer_text(&self, thread: ThreadId) -> String {
        self.read(|chat| chat.thread_messages(thread).last().unwrap().text())
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

fn model(server: &Fixture, id: &str, wire_model: &str) -> ModelEntry {
    ModelEntry::new(
        id,
        Rc::new(
            NetworkModel::new(NetworkConfig { endpoint: server.endpoint(), model: wire_model.into(), api_key: None, timeout: TIMEOUT })
                .unwrap(),
        ),
    )
}

#[test]
fn changing_models_keeps_the_active_stream_and_regenerates_with_the_new_selection() {
    let server = Fixture::new(vec![Plan::hold(Some("A first")), Plan::success("B replacement"), Plan::success("A followup")]);
    let mut h = Harness::from_models(vec![model(&server, "provider-a", "wire-model-a"), model(&server, "provider-b", "wire-model-b")]);
    let thread = h.send("first question");
    h.until(&server, "A's streamed prefix", || h.answer_text(thread) == "A first");

    h.act(|chat| chat.model.set(Some("provider-b".into())));
    assert_eq!(h.read(|chat| untrack(|| chat.model.get())).as_deref(), Some("provider-b"));
    assert_eq!(h.answer(thread).status, Status::Streaming);
    server.inspect(|s| {
        assert_eq!(s.requests.len(), 1);
        assert_eq!(s.requests[0]["model"], "wire-model-a", "application ids never replace the wire model name");
        assert!(s.closed.is_empty(), "changing selection leaves A's connection open");
    });
    server.send_text(0, " + A continued");
    h.until(&server, "A to continue after selecting B", || h.answer_text(thread) == "A first + A continued");

    let answer = h.answer(thread).id;
    h.act(move |chat| chat.stop(thread));
    h.until(&server, "A's cancelled socket to close", || server.inspect(|s| s.closed.contains(&0)));
    assert_eq!(h.answer(thread).status, Status::Stopped);
    assert_eq!(h.answer_text(thread), "A first + A continued");

    h.act(move |chat| chat.regenerate(answer));
    h.done(&server, thread);
    assert_eq!(h.answer_text(thread), "B replacement");
    assert_eq!(h.answer(thread).status, Status::Done);
    h.act(|chat| chat.model.set(Some("provider-a".into())));
    h.send("follow up on A");
    h.done(&server, thread);
    assert_eq!(h.answer_text(thread), "A followup");
    server.inspect(|s| {
        assert_eq!(s.requests.len(), 3, "network models generate titles locally");
        assert_eq!(s.requests[1]["model"], "wire-model-b");
        assert_eq!(s.requests[1]["messages"], json!([{"role":"user","content":"first question"}]));
        assert_eq!(s.requests[2]["model"], "wire-model-a");
        assert_eq!(
            s.requests[2]["messages"],
            json!([
                {"role":"user","content":"first question"},
                {"role":"assistant","content":"B replacement"},
                {"role":"user","content":"follow up on A"},
            ])
        );
    });
}

#[test]
fn queued_requests_keep_the_model_selected_at_enqueue_and_send_the_completed_history() {
    let server = Fixture::new(vec![
        Plan::hold(Some("A first answer")),
        Plan::hold(Some("B queued answer")),
        Plan::success("A queued answer"),
        Plan::success("B followup answer"),
    ]);
    let mut h = Harness::from_models(vec![model(&server, "provider-a", "wire-model-a"), model(&server, "provider-b", "wire-model-b")]);
    let thread = h.send("first question");
    h.until(&server, "A's first answer", || h.answer_text(thread) == "A first answer");
    h.act(|chat| chat.model.set(Some("provider-b".into())));
    h.send("queued for B");
    h.act(|chat| chat.model.set(Some("provider-a".into())));
    h.send("queued for A");
    assert_eq!(
        h.read(|chat| chat.queue.keys().into_iter().map(|id| untrack(|| chat.queue.item(&id).unwrap().get()).model_id).collect::<Vec<_>>()),
        ["provider-b", "provider-a"]
    );
    h.act(|chat| chat.model.set(Some("provider-b".into())));
    server.finish(0);
    h.until(&server, "B's queued answer", || server.inspect(|s| s.requests.len() == 2) && h.answer_text(thread) == "B queued answer");
    assert_eq!(h.answer(thread).status, Status::Streaming);
    server.finish(1);
    h.until(&server, "both queued requests to finish", || {
        h.read(|chat| chat.thread_messages(thread).len() == 6 && chat.queue.is_empty() && !chat.is_answering(thread))
    });
    assert_eq!(h.answer_text(thread), "A queued answer");
    assert_eq!(h.answer(thread).status, Status::Done);
    assert_eq!(h.read(|chat| untrack(|| chat.model.get())).as_deref(), Some("provider-b"));
    h.send("follow up on B");
    h.done(&server, thread);
    assert_eq!(h.answer_text(thread), "B followup answer");
    server.inspect(|s| {
        assert_eq!(s.requests.len(), 4);
        assert_eq!(
            s.requests.iter().map(|r| r["model"].as_str().unwrap()).collect::<Vec<_>>(),
            ["wire-model-a", "wire-model-b", "wire-model-a", "wire-model-b"]
        );
        assert_eq!(s.requests[0]["messages"], json!([{"role":"user","content":"first question"}]));
        assert_eq!(
            s.requests[1]["messages"],
            json!([
                {"role":"user","content":"first question"},
                {"role":"assistant","content":"A first answer"},
                {"role":"user","content":"queued for B"},
            ])
        );
        assert_eq!(
            s.requests[2]["messages"],
            json!([
                {"role":"user","content":"first question"},
                {"role":"assistant","content":"A first answer"},
                {"role":"user","content":"queued for B"},
                {"role":"assistant","content":"B queued answer"},
                {"role":"user","content":"queued for A"},
            ])
        );
        assert_eq!(
            s.requests[3]["messages"],
            json!([
                {"role":"user","content":"first question"},
                {"role":"assistant","content":"A first answer"},
                {"role":"user","content":"queued for B"},
                {"role":"assistant","content":"B queued answer"},
                {"role":"user","content":"queued for A"},
                {"role":"assistant","content":"A queued answer"},
                {"role":"user","content":"follow up on B"},
            ])
        );
    });
}

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
    assert_eq!(h.answer_text(thread), "你好，world 😀");
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
        assert!(h.answer_text(thread).is_empty());
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
    assert!(h.answer_text(thread).is_empty());
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
    assert_eq!(h.answer_text(thread), "received 中文");
    let answer = h.answer(thread).id;
    h.act(move |chat| chat.regenerate(answer));
    h.done(&server, thread);
    assert_eq!(h.answer_text(thread), "complete replacement");
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
    assert_eq!(h.answer_text(thread), "kept");
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
            h.until(&server, "the streamed prefix", || h.answer_text(thread) == prefix);
        }
        h.act(move |chat| chat.stop(thread));
        assert_eq!(h.answer(thread).status, Status::Stopped);
        h.send("new request");
        h.done(&server, thread);
        h.until(&server, "the cancelled socket to close", || server.inspect(|s| s.closed.contains(&0)));
        assert_eq!(h.answer(thread).status, Status::Done);
        assert_eq!(h.answer_text(thread), "fresh answer", "retired data cannot contaminate the new answer");
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
    assert_eq!(h.answer_text(thread), "kept before idle");
    assert_eq!(h.window.app().now(), clock, "wall-clock network timeout is not a virtual-time advance");
    h.until(&server, "the idle request to close", || server.inspect(|s| s.closed.contains(&0)));
}
