//! The opt-in live Inspector: one local connection, ordered requests, UI-thread execution.

use super::{Shell, ShellEvent, ww};
use octara::app::{TargetGuard, Window, WindowId};
use octara::diagnostics as codes;
use octara::inspect::{Completion, Delivery, LayoutState, Reply, Request, SurfaceState};
use octara::serde_json::{self, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

const MAX_WAIT_MS: u64 = 30_000;
const OPERATIONS: &[&str] = &[
    "inspect",
    "issues",
    "tap",
    "double_tap",
    "long_press",
    "drag",
    "secondary_tap",
    "focus",
    "hover",
    "key",
    "type",
    "set_value",
    "scroll",
    "invoke",
    "settle",
    #[cfg(not(target_os = "ios"))]
    "size",
    "theme",
    "reduced_motion",
    "capture",
    "pick",
    "select",
    "copy",
    "answer",
    "expect",
    "reload",
];

/// Keeps the endpoint alive and removes only the socket it created.
pub(crate) struct Server {
    #[cfg(unix)]
    _wire: wire::Server,
    build: Option<Arc<str>>,
}

impl Server {
    pub(crate) fn from_env(proxy: winit::event_loop::EventLoopProxy<ShellEvent>) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os("OCTARA_INSPECT") else { return Ok(None) };
        #[cfg(unix)]
        {
            let path = std::path::PathBuf::from(path);
            if !path.is_absolute() {
                return Err("OCTARA_INSPECT needs an absolute Unix socket path".into());
            }
            let server = wire::Server::start(&path, proxy)?;
            let build = std::env::var("OCTARA_INSPECT_BUILD")
                .ok()
                .filter(|digest| digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
                .map(Arc::from);
            eprintln!("octara-shell: inspector {}", path.display());
            Ok(Some(Server { _wire: server, build }))
        }
        #[cfg(not(unix))]
        {
            let _ = (path, proxy);
            Err("OCTARA_INSPECT requires a host with Unix sockets".into())
        }
    }
}

/// One response belongs to its connection, even if that connection has since closed.
pub(crate) struct Responder {
    tx: Sender<String>,
    pub(crate) connected: Arc<AtomicBool>,
    route: &'static str,
    input: bool,
    build: Option<Arc<str>>,
    target: Option<(Window, Option<TargetGuard>)>,
    sent: std::cell::Cell<bool>,
    delivery: std::cell::Cell<Option<Delivery>>,
}

impl Responder {
    fn new(tx: Sender<String>, connected: Arc<AtomicBool>, route: &'static str, input: bool) -> Self {
        Self {
            tx,
            connected,
            route,
            input,
            build: None,
            target: None,
            sent: std::cell::Cell::new(false),
            delivery: std::cell::Cell::new(Some(Delivery::Refused)),
        }
    }

    fn with_target(mut self, window: &Window, guard: Option<&TargetGuard>) -> Self {
        self.target = Some((window.clone(), guard.cloned()));
        self
    }

    fn begin(&self) {
        self.delivery.set(None);
    }

    pub(crate) fn target_current(&self) -> bool {
        self.target.as_ref().is_none_or(|(window, guard)| guard.as_ref().is_none_or(|guard| guard.is_current(window)))
    }

    fn with_current_resize_target(self, resize: impl FnOnce(Self)) {
        if self.target_current() {
            resize(self);
            return;
        }
        let delivery = self.delivery.get().unwrap_or(Delivery::Refused);
        let message = match delivery {
            Delivery::Refused => "the observed window changed before the resize request; inspect again",
            Delivery::Delivered => "the observed window changed after the resize request; inspect again",
        };
        self.send(Reply { delivery: Some(delivery), ..Reply::fail(&codes::STALE_TARGET, message) });
    }

    pub(crate) fn send(self, reply: Reply) {
        self.send_with(reply, json!({}));
    }

    fn send_wait(self, reply: Reply, waiting: Option<(bool, Vec<String>)>) {
        let extra = waiting.map_or_else(|| json!({}), |(settled, pending)| json!({ "settled": settled, "pending": pending }));
        self.send_with(reply, extra);
    }

    fn send_gesture(self, mut reply: Reply, delivered: bool) {
        if reply.delivery.is_none() {
            reply.delivery = if delivered {
                Some(Delivery::Delivered)
            } else if reply.code.as_deref() == Some(codes::APP_FAILED.id) {
                None
            } else {
                Some(Delivery::Refused)
            };
        }
        self.send(reply);
    }

    fn send_with(self, mut reply: Reply, extra: serde_json::Value) {
        self.sent.set(true);
        if !self.connected.load(Ordering::Relaxed) {
            return;
        }
        let mut live = json!({ "clock": "realtime", "route": self.route, "pid": std::process::id(), "platform": std::env::consts::OS });
        if let Some(build) = &self.build {
            live["build"] = build.as_ref().into();
        }
        if self.input {
            if reply.delivery.is_none() {
                reply.delivery = match extra.get("delivery").and_then(serde_json::Value::as_str) {
                    Some("delivered") => Some(Delivery::Delivered),
                    Some("refused") => Some(Delivery::Refused),
                    _ if reply.ok => Some(Delivery::Delivered),
                    _ if matches!(reply.code.as_deref(), Some(c) if c == codes::BAD_REQUEST.id || c == codes::NO_SUCH_ELEMENT.id || c == codes::STALE_TARGET.id) => {
                        Some(Delivery::Refused)
                    }
                    _ => None,
                };
            }
            let delivery = match &reply.delivery {
                Some(Delivery::Delivered) => "delivered",
                Some(Delivery::Refused) => "refused",
                None => "unknown",
            };
            live["delivery"] = delivery.into();
        }
        // A delivered phase can replace its target. Sample the original window without
        // resolving or validating the target again.
        let reports_completion = if self.input { reply.delivery == Some(Delivery::Delivered) } else { reply.completion.is_some() };
        reply.completion = self.target.as_ref().filter(|_| reports_completion).map(|(window, _)| observe_completion(window));
        if let Some(snapshot) = &reply.snapshot {
            live["operations"] = if snapshot.failed { json!(["inspect", "issues"]) } else { json!(OPERATIONS) };
            live["transport"] = "unix".into();
            live["target_validation"] = "identity".into();
        }
        live.as_object_mut().expect("live metadata is an object").extend(extra.as_object().expect("extra metadata is an object").clone());
        let mut value = serde_json::to_value(reply).expect("an inspector reply serializes");
        value["live"] = live;
        let _ = self.tx.send(value.to_string());
    }
}

fn observe_completion(window: &Window) -> Completion {
    let mut completion = Completion::observe(window);
    if completion.layout != LayoutState::Unavailable
        && let Some((frames, submitted)) = ww(window, |host| {
            let submitted =
                window.published_revision().is_some_and(|revision| host.has_submitted(revision) && host.matches_drawable(window.scale()));
            (host.frames, submitted)
        })
    {
        let last_submission = (frames > 0).then_some(frames);
        completion.surface = match last_submission {
            Some(frame) if completion.layout == LayoutState::Settled && submitted => SurfaceState::Submitted { frame },
            _ => SurfaceState::Pending { last_submission },
        };
    }
    completion
}

pub(crate) struct Waiting {
    window: WindowId,
    until: Instant,
    reply: Responder,
}

/// Only one serialized live request waits at a time; captures retain their existing host job.
pub(crate) enum Pending {
    Settle(Waiting),
    Gesture { job: super::gesture::Job, reply: Responder },
    Resize { job: super::resize::Job, reply: Responder },
}

impl Pending {
    fn gesture(job: super::gesture::Job, reply: Responder) -> Self {
        reply.delivery.set(Some(if job.delivered() { Delivery::Delivered } else { Delivery::Refused }));
        Self::Gesture { job, reply }
    }

    /// Cancel our held pointer before ticking timers once transport has observed disconnect.
    pub(crate) fn cancel_disconnected(&self, app: &octara::app::App) {
        if app.is_failed() {
            return;
        }
        if let Self::Gesture { job, reply } = self
            && !reply.connected.load(Ordering::Relaxed)
            && let Some(mut window) = app.window(job.window())
        {
            job.cancel(&mut window);
        }
    }

    fn connected(&self) -> bool {
        match self {
            Self::Settle(waiting) => &waiting.reply,
            Self::Gesture { reply, .. } | Self::Resize { reply, .. } => reply,
        }
        .connected
        .load(Ordering::Relaxed)
    }
}

fn route(request: &Request) -> (&'static str, bool) {
    match request {
        Request::Tap { .. }
        | Request::DoubleTap { .. }
        | Request::LongPress { .. }
        | Request::Drag { .. }
        | Request::SecondaryTap { .. }
        | Request::Type { .. }
        | Request::Key { .. } => ("framework", true),
        Request::Focus { .. }
        | Request::SetValue { .. }
        | Request::Scroll { .. }
        | Request::Hover { .. }
        | Request::Invoke { .. }
        | Request::Select { .. }
        | Request::Copy
        | Request::Answer { .. }
        | Request::Theme { .. }
        | Request::ReducedMotion { .. } => ("semantic", true),
        Request::Reload => ("lifecycle", true),
        Request::Capture { .. } => ("capture", false),
        Request::Settle { .. } => ("wait", false),
        Request::Size { .. } => ("native-window", true),
        Request::Advance { .. } => ("unsupported", true),
        _ => ("query", false),
    }
}

/// Validation can refuse a request before it returns a typed operation. Preserve its input
/// route so a stale target reports a refusal instead of looking like an unrelated query.
fn route_line(line: &str) -> (&'static str, bool) {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) else { return ("query", false) };
    if let Some(fields) = value.as_object_mut() {
        fields.remove("window");
        fields.remove("target");
    }
    serde_json::from_value::<Request>(value).map_or(("query", false), |request| route(&request))
}

impl Shell {
    pub(crate) fn inspect_request(&mut self, line: String, tx: Sender<String>, connected: Arc<AtomicBool>) {
        if !connected.load(Ordering::Relaxed) {
            return;
        }
        let build = self.inspector.as_ref().and_then(|server| server.build.clone());
        let respond = move |route, input| {
            let mut response = Responder::new(tx, connected, route, input);
            response.build = build;
            response
        };
        let window = self.main_window().or_else(|| self.app.as_ref().and_then(|app| app.windows().first().and_then(|id| app.window(*id))));
        let Some(window) = window else {
            respond("query", false).send(Reply::fail(&codes::BAD_REQUEST, "the app has no open main window"));
            return;
        };
        let octara::inspect::ParsedRequest { window: mut target, request, guard } =
            match octara::inspect::parse_guarded_request(&window, &line) {
                Ok(parsed) => parsed,
                Err(reply) => {
                    let (route, input) = route_line(&line);
                    respond(route, input).send(*reply);
                    return;
                }
            };
        let (route, input) = route(&request);
        let response = respond(route, input).with_target(&target, guard.as_ref());
        if let Some(reply) = octara::inspect::failure_refusal(&target, &request) {
            response.send(reply);
            return;
        }
        if route != "query" && !self.app_failed() {
            self.tick();
            if let Some(app) = &self.app {
                app.poll();
            }
            self.show();
        }
        response.begin();
        match request {
            Request::Advance { .. } => response.send(Reply::fail(
                &codes::BAD_REQUEST,
                "this operation is not supported by the realtime live host; inspect live.operations for supported requests",
            )),
            Request::DoubleTap { .. } | Request::LongPress { .. } | Request::Drag { .. } => {
                let kind = match &request {
                    Request::DoubleTap { .. } => super::gesture::Kind::DoubleTap,
                    Request::Drag { dx, dy, .. } => super::gesture::Kind::Drag { dx: *dx, dy: *dy },
                    _ => super::gesture::Kind::LongPress,
                };
                let guard = guard.expect("a gesture has a bound target");
                match super::gesture::Job::start_guarded(&mut target, kind, Instant::now(), guard) {
                    Ok(job) => {
                        self.inspect_pending = Some(Pending::gesture(job, response));
                        self.show();
                        self.inspect_poll();
                    }
                    Err(reply) => response.send_gesture(*reply, false),
                }
            }
            Request::Size { width, height } => {
                response.with_current_resize_target(|response| match super::resize::Job::start(self, &mut target, width, height) {
                    Ok(job) => {
                        response.delivery.set(Some(Delivery::Delivered));
                        self.inspect_pending = Some(Pending::Resize { job, reply: response });
                        self.show();
                        self.inspect_poll();
                    }
                    Err(reply) if reply.code.as_deref() == Some(codes::STEP_FAILED.id) => {
                        let actual = reply.actual.clone();
                        response.send_with(*reply, json!({ "delivery": "delivered", "resize": actual }));
                    }
                    Err(reply) => response.send(*reply),
                })
            }
            Request::Capture { path, settle_ms, scale, pixels } => {
                let ms = settle_ms.unwrap_or(5000);
                let actual_scale = scale.unwrap_or_else(|| ww(&target, |w| w.scale()).unwrap_or(1.0));
                let invalid = if ms > MAX_WAIT_MS {
                    Some("capture settle_ms must not exceed 30000")
                } else if super::capture::validate_size(target.size(), actual_scale).is_err() {
                    Some("capture size and scale must be finite and positive, scale at most 8, and produce at most 32 megapixels")
                } else if pixels == octara::capture::Pixels::Display && scale.is_some() {
                    Some("display captures use the window's display scale; omit scale")
                } else if self.capture.is_some() {
                    Some("another window capture is still in progress")
                } else {
                    None
                };
                if let Some(why) = invalid {
                    response.send(Reply::fail(&codes::BAD_REQUEST, why));
                    return;
                }
                self.capture = Some(super::capture::Job::live(
                    target.id(),
                    path,
                    Duration::from_millis(ms),
                    scale,
                    pixels,
                    response,
                    &self.shared.proxy,
                ));
                self.capture_step();
            }
            Request::Settle { limit_ms } => {
                let ms = limit_ms.unwrap_or(10_000);
                if ms > MAX_WAIT_MS {
                    response.send(Reply::fail(&codes::BAD_REQUEST, "settle limit_ms must not exceed 30000"));
                    return;
                }
                self.inspect_pending = Some(Pending::Settle(Waiting {
                    window: target.id(),
                    until: Instant::now() + Duration::from_millis(ms),
                    reply: response,
                }));
                self.inspect_poll();
            }
            request => {
                let reply = octara::inspect::handle_guarded(&mut target, request, guard.as_ref());
                if input {
                    response.delivery.set(reply.delivery);
                    self.show();
                }
                if reply.snapshot.is_some() {
                    let host = ww(&target, |window| window.inspect_state()).unwrap_or_else(|| json!({}));
                    response.send_with(reply, host);
                } else {
                    response.send(reply);
                }
            }
        }
    }

    /// Called while a live request waits; the event loop continues to accept native replies.
    pub(crate) fn inspect_poll(&mut self) {
        if let Some(pending) = &self.inspect_pending
            && let Some(app) = &self.app
        {
            pending.cancel_disconnected(app);
        }
        let Some(pending) = self.inspect_pending.take() else { return };
        if !pending.connected() {
            return;
        }
        self.tick();
        if let Some(app) = &self.app {
            app.poll();
        }
        self.show();
        if let Some(app) = &self.app {
            pending.cancel_disconnected(app);
        }
        if !pending.connected() {
            return;
        }
        // Ready callbacks or rendering can take time. Sample wall time again before testing
        // gesture deadlines so a due recognizer is processed before its job checks feedback.
        self.tick();
        match pending {
            Pending::Settle(waiting) => {
                if !waiting.reply.target_current() {
                    waiting.reply.send(Reply::fail(&codes::STALE_TARGET, "the observed window changed while waiting; inspect again"));
                    return;
                }
                let Some(window) = self.app.as_ref().and_then(|a| a.window(waiting.window)) else {
                    waiting.reply.send(Reply::fail(&codes::BAD_REQUEST, "the target window closed while waiting"));
                    return;
                };
                let pending = window.unsettled();
                if pending.is_empty() {
                    waiting.reply.send_wait(Reply::ok(), Some((true, pending)));
                } else if Instant::now() >= waiting.until {
                    waiting.reply.send_wait(
                        Reply::fail(&codes::STEP_FAILED, "the live wait budget ended with work still pending"),
                        Some((false, pending)),
                    );
                } else {
                    self.inspect_pending = Some(Pending::Settle(waiting));
                }
            }
            Pending::Gesture { mut job, reply } => {
                let Some(mut window) = self.app.as_ref().and_then(|app| app.window(job.window())) else {
                    reply.send_gesture(Reply::fail(&codes::STEP_FAILED, "the target window closed during the gesture"), job.delivered());
                    return;
                };
                if !job.delivered() {
                    reply.begin();
                }
                match job.poll(&mut window, Instant::now()) {
                    Some(result) => reply.send_gesture(result, job.delivered()),
                    None => {
                        self.inspect_pending = Some(Pending::gesture(job, reply));
                    }
                }
                self.show();
            }
            Pending::Resize { job, reply } => reply.with_current_resize_target(|reply| match job.poll(self) {
                Some(done) => reply.send_with(done.reply, json!({ "resize": done.resize, "delivery": "delivered" })),
                None => self.inspect_pending = Some(Pending::Resize { job, reply }),
            }),
        }
    }
}

#[cfg(test)]
mod target_feedback {
    use super::*;

    fn resize_response(window: &octara::app::Window, guarded: bool) -> (Responder, std::sync::mpsc::Receiver<String>) {
        let mut request = json!({ "op": "size", "width": 700, "height": 500 });
        if guarded {
            request["target"] = window.target().into();
        }
        let parsed = octara::inspect::parse_guarded_request(window, &request.to_string()).unwrap();
        let (route, input) = route(&parsed.request);
        let (send, receive) = std::sync::mpsc::channel();
        let response =
            Responder::new(send, Arc::new(AtomicBool::new(true)), route, input).with_target(&parsed.window, parsed.guard.as_ref());
        (response, receive)
    }

    fn reload_on_poll(app: &octara::app::App) {
        let (trigger, mut reload) = octara::task::channel();
        app.run(|| {
            octara::task::spawn(async move {
                reload.recv().await.unwrap();
                octara::app::App::current().hot_reload();
            });
        });
        trigger.send(()).unwrap();
    }

    #[test]
    fn resizing_refuses_a_target_reloaded_during_pre_dispatch_poll() {
        let window = octara::app::Window::headless(|| octara::view::el());
        let app = window.app();
        let (response, receive) = resize_response(&window, true);
        reload_on_poll(&app);
        assert!(response.target_current());
        app.poll();
        response.begin();

        let mut native_calls = 0;
        response.with_current_resize_target(|_| native_calls += 1);
        assert_eq!(native_calls, 0);
        let reply: serde_json::Value = serde_json::from_str(&receive.try_recv().unwrap()).unwrap();
        assert_eq!(reply["code"], "STALE_TARGET");
        assert_eq!(reply["delivery"], "refused");
        assert_eq!(reply["live"]["route"], "native-window");
        assert_eq!(reply["live"]["delivery"], "refused");
        assert!(reply.get("completion").is_none());
        app.shutdown();
    }

    #[test]
    fn a_pending_resize_keeps_its_delivery_when_poll_reloads_the_target() {
        let window = octara::app::Window::headless(|| octara::view::el());
        let app = window.app();
        let (response, receive) = resize_response(&window, true);
        response.begin();

        let mut pending = None;
        response.with_current_resize_target(|response| {
            response.delivery.set(Some(Delivery::Delivered));
            pending = Some(response);
        });
        let response = pending.expect("a current target can dispatch the native resize and wait");
        reload_on_poll(&app);
        assert!(response.target_current());
        app.poll();
        response.with_current_resize_target(|_| panic!("a stale resize must stop waiting"));

        let reply: serde_json::Value = serde_json::from_str(&receive.try_recv().unwrap()).unwrap();
        assert_eq!(reply["code"], "STALE_TARGET");
        assert_eq!(reply["delivery"], "delivered");
        assert_eq!(reply["live"]["route"], "native-window");
        assert_eq!(reply["live"]["delivery"], "delivered");
        assert_eq!(reply["completion"]["window"], window.id().number());
        assert_eq!(reply["completion"]["layout"], "settled");
        assert_eq!(reply["completion"]["surface"]["state"], "unavailable");
        app.shutdown();
    }

    #[test]
    fn resizing_without_an_observed_target_can_dispatch_after_reload() {
        let window = octara::app::Window::headless(|| octara::view::el());
        let app = window.app();
        let (response, receive) = resize_response(&window, false);
        reload_on_poll(&app);
        app.poll();
        response.begin();
        response.with_current_resize_target(|response| response.send(Reply::ok()));

        let reply: serde_json::Value = serde_json::from_str(&receive.try_recv().unwrap()).unwrap();
        assert!(reply["ok"].as_bool().unwrap());
        assert_eq!(reply["live"]["delivery"], "delivered");
        app.shutdown();
    }

    #[test]
    fn a_panic_on_the_first_click_keeps_gesture_delivery_unknown() {
        use octara::prelude::*;

        let mut window = Window::headless(|| el().id("target").size_12().on_press(|| panic!("failed inside the click")));
        let parsed = octara::inspect::parse_guarded_request(&window, r#"{"op":"double_tap","id":"target"}"#).unwrap();
        let guard = parsed.guard.unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let response = Responder::new(send, Arc::new(AtomicBool::new(true)), "framework", true).with_target(&window, Some(&guard));
        response.begin();
        let reply = super::super::gesture::Job::start_guarded(&mut window, super::super::gesture::Kind::DoubleTap, Instant::now(), guard)
            .err()
            .expect("the first click panicked");
        response.send_gesture(*reply, false);

        let reply: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
        assert_eq!(reply["code"], "APP_FAILED");
        assert_eq!(reply["live"]["delivery"], "unknown");
        assert!(reply.get("delivery").is_none() && reply.get("completion").is_none());
        window.app().shutdown();
    }

    fn response(line: &str, reply: Reply, gesture: bool) -> serde_json::Value {
        let (send, receive) = std::sync::mpsc::channel();
        let (route, input) = route_line(line);
        let responder = Responder::new(send, Arc::new(AtomicBool::new(true)), route, input);
        if gesture {
            responder.send_gesture(reply, false)
        } else {
            responder.send(reply)
        }
        serde_json::from_str(&receive.recv().unwrap()).unwrap()
    }

    #[test]
    fn an_interrupted_response_reports_failure_once_without_claiming_delivery() {
        let app = octara::app::App::new(octara::platform::HeadlessPlatform::default());
        let window = app.open_window(octara::platform::WindowOptions::default(), || octara::view::el()).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let responder = Responder::new(send, Arc::new(AtomicBool::new(true)), "framework", true).with_target(&window, None);
        responder.begin();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run(|| panic!("request failed"))));
        drop(responder);
        let reply: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
        assert_eq!(reply["code"], "APP_FAILED");
        assert_eq!(reply["live"]["delivery"], "unknown");
        assert!(reply.get("completion").is_none());
        assert!(receive.try_recv().is_err());

        for (delivery, label) in [(Delivery::Refused, "refused"), (Delivery::Delivered, "delivered")] {
            let (send, receive) = std::sync::mpsc::channel();
            let responder = Responder::new(send, Arc::new(AtomicBool::new(true)), "framework", true).with_target(&window, None);
            responder.delivery.set(Some(delivery));
            drop(responder);
            let reply: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
            assert_eq!(reply["delivery"], label);
            assert_eq!(reply["live"]["delivery"], label);
            if delivery == Delivery::Delivered {
                assert_eq!(reply["completion"]["layout"], "unavailable");
                assert_eq!(reply["completion"]["surface"]["state"], "unavailable");
            } else {
                assert!(reply.get("completion").is_none());
            }
        }

        let (send, receive) = std::sync::mpsc::channel();
        Responder::new(send, Arc::new(AtomicBool::new(true)), "query", false)
            .with_target(&window, None)
            .send(Reply { snapshot: Some(Box::new(window.inspect())), ..Reply::ok() });
        let reply: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
        assert!(reply["ok"].as_bool().unwrap());
        assert_eq!(reply["live"]["operations"], json!(["inspect", "issues"]));
        assert!(receive.try_recv().is_err(), "normal send must not emit a second failure on drop");
        app.shutdown();
    }

    #[test]
    fn a_task_failure_retires_waiting_gestures_without_releasing_input_or_losing_delivery() {
        use octara::prelude::*;
        use std::cell::Cell;
        use std::rc::Rc;

        use super::super::gesture::{Job, Kind};
        for (kind, connected, expected_delivery) in
            [(Kind::LongPress, true, "delivered"), (Kind::LongPress, false, "delivered"), (Kind::DoubleTap, true, "refused")]
        {
            let callbacks = Rc::new(Cell::new(0));
            let capture = callbacks.clone();
            let mut window = Window::headless(move || {
                let capture = capture.clone();
                spawn(async {
                    sleep(Duration::from_millis(10)).await;
                    panic!("task failed while a gesture waited");
                });
                el().id("target").w(120).h(48).on_double_press(|| {}).on_long_press(move || capture.set(capture.get() + 1))
            });
            let app = window.app();
            let (send, receive) = std::sync::mpsc::channel();
            let connection = Arc::new(AtomicBool::new(true));
            let response = Responder::new(send, connection.clone(), "framework", true).with_target(&window, None);
            response.begin();
            if matches!(kind, Kind::DoubleTap) {
                window.tap("target").unwrap();
                assert!(!window.pointer_click_cooldown().is_zero());
            }
            let job = Job::start(&mut window, "target", kind, Instant::now()).unwrap();
            let pending = Pending::gesture(job, response);
            let held = window.held_pointer();
            assert_eq!(held.is_some(), matches!(kind, Kind::LongPress));
            let frames = window.frames_presented();
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.advance(Duration::from_millis(10))));
            assert!(failure.is_err());
            assert!(app.is_failed());
            connection.store(connected, Ordering::Relaxed);
            // The host must not dispatch cancellation into a failed App, even on disconnect.
            pending.cancel_disconnected(&app);
            drop(pending);
            assert_eq!(window.held_pointer(), held);
            assert_eq!(window.frames_presented(), frames);
            assert_eq!(callbacks.get(), 0);
            if connected {
                let reply: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
                assert_eq!(reply["code"], "APP_FAILED");
                assert_eq!(reply["delivery"], expected_delivery);
                assert_eq!(reply["live"]["delivery"], expected_delivery);
            }
            assert!(matches!(receive.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)));
            app.shutdown();
            assert!(window.is_closed());
            assert_eq!(window.held_pointer(), None);
            assert!(app.pending().is_empty());
            assert_eq!(callbacks.get(), 0, "teardown must not finish a held gesture");
        }
    }

    #[test]
    fn failure_before_request_admission_is_refused_and_does_not_run_input() {
        use octara::prelude::*;
        let app = App::new(octara::platform::HeadlessPlatform::default());
        let window = app.open_window(WindowOptions::default(), || el()).unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let response = Responder::new(send, Arc::new(AtomicBool::new(true)), "framework", true).with_target(&window, None);
        // Same order as live request preparation: tick/poll may fail before begin().
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.run(|| {
                spawn(async {
                    panic!("ready work failed before input");
                });
            });
            app.poll();
        }));
        assert!(failure.is_err());
        drop(response);
        let reply: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
        assert_eq!(reply["code"], "APP_FAILED");
        assert_eq!(reply["delivery"], "refused");
        assert_eq!(reply["live"]["delivery"], "refused");
        app.shutdown();
    }

    #[test]
    fn stale_target_refusal_keeps_the_requested_input_route() {
        let reply = response(
            r#"{"op":"tap","window":17,"id":"save","target":"old-observation"}"#,
            Reply::fail(&codes::STALE_TARGET, "the observed node was replaced"),
            false,
        );
        assert_eq!(reply["code"], "STALE_TARGET");
        assert_eq!(reply["live"]["route"], "framework");
        assert_eq!(reply["live"]["delivery"], "refused");
    }

    #[test]
    fn partial_input_is_not_reported_as_a_pre_dispatch_refusal() {
        let mut failure = Reply::fail(&codes::STALE_TARGET, "focus was delivered, but its target was replaced");
        failure.delivery = Some(Delivery::Delivered);
        let reply = response(r#"{"op":"type","id":"name","text":"hello","target":"old-observation"}"#, failure, false);
        assert_eq!(reply["live"]["delivery"], "delivered");
        assert!(!reply["ok"].as_bool().unwrap());
    }

    #[test]
    fn first_gesture_phase_preserves_its_partial_delivery_fact() {
        let mut failure = Reply::fail(&codes::STALE_TARGET, "the pointer move replaced the target before pointer down");
        failure.delivery = Some(Delivery::Delivered);
        let reply = response(r#"{"op":"double_tap","id":"save","target":"old-observation"}"#, failure, true);
        assert_eq!(reply["live"]["delivery"], "delivered");
    }
}

#[cfg(unix)]
mod wire {
    use super::*;
    use std::fs;
    use std::io::{ErrorKind, Read, Write};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, mpsc};
    use std::thread::{self, JoinHandle};

    const REQUEST_LIMIT: usize = 1024 * 1024;
    const REPLY_LIMIT: usize = 16 * 1024 * 1024;
    const POLL: Duration = Duration::from_millis(50);

    pub(super) struct Server {
        stop: Arc<AtomicBool>,
        client: Arc<Mutex<Option<UnixStream>>>,
        worker: Option<JoinHandle<()>>,
        _socket: Socket,
    }

    struct Socket {
        path: PathBuf,
        identity: (u64, u64),
        // Keep the bound inode alive until cleanup so it cannot be reused first.
        listener: UnixListener,
    }

    impl Socket {
        fn bind(path: &Path) -> Result<Self, String> {
            let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
            let socket = Self { path: path.into(), identity: (metadata.dev(), metadata.ino()), listener };
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
            socket.listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            Ok(socket)
        }
    }

    impl Drop for Socket {
        fn drop(&mut self) {
            if let Ok(metadata) = fs::symlink_metadata(&self.path)
                && metadata.file_type().is_socket()
                && (metadata.dev(), metadata.ino()) == self.identity
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    impl Server {
        pub(super) fn start(path: &Path, proxy: winit::event_loop::EventLoopProxy<ShellEvent>) -> Result<Self, String> {
            let socket = Socket::bind(path)?;
            let stop = Arc::new(AtomicBool::new(false));
            let client = Arc::new(Mutex::new(None));
            let mut server = Self { stop, client, worker: None, _socket: socket };
            let listener = server._socket.listener.try_clone().map_err(|e| e.to_string())?;
            let stop = server.stop.clone();
            let active = server.client.clone();
            server.worker = Some(thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Ok(clone) = stream.try_clone() {
                                *active.lock().expect("inspector client lock") = Some(clone);
                            }
                            connection(stream, &proxy, &stop);
                            *active.lock().expect("inspector client lock") = None;
                        }
                        Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(POLL),
                        Err(_) => break,
                    }
                }
            }));
            Ok(server)
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(client) = self.client.lock().expect("inspector client lock").as_ref() {
                let _ = client.shutdown(std::net::Shutdown::Both);
            }
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    fn take_line(buffer: &mut Vec<u8>) -> Result<Option<String>, ()> {
        let Some(end) = buffer.iter().position(|b| *b == b'\n') else { return Ok(None) };
        String::from_utf8(buffer.drain(..=end).collect()).map(Some).map_err(|_| ())
    }

    fn failure(message: &str) -> String {
        Reply::fail(&codes::BAD_REQUEST, message).to_json()
    }

    /// Read without blocking, including while awaiting a UI reply, so disconnect is noticed.
    fn receive(stream: &mut UnixStream, buffer: &mut Vec<u8>) -> Result<bool, ()> {
        let mut bytes = [0; 8192];
        loop {
            match stream.read(&mut bytes) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    buffer.extend_from_slice(&bytes[..n]);
                    // The limit applies to content, with one additional byte for the LF.
                    if buffer.len() > REQUEST_LIMIT + 1 || (buffer.len() == REQUEST_LIMIT + 1 && buffer.last() != Some(&b'\n')) {
                        return Err(());
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(true),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Ok(false),
            }
        }
    }

    fn write_reply(stream: &mut UnixStream, reply: &str, stop: &AtomicBool) -> bool {
        // The operation may already have run. Close rather than replace its result with a
        // misleading validation refusal; the client must treat the outcome as unknown.
        if reply.len() > REPLY_LIMIT {
            return false;
        }
        let bytes = format!("{reply}\n");
        let mut left = bytes.as_bytes();
        let until = Instant::now() + Duration::from_secs(5);
        while !left.is_empty() && !stop.load(Ordering::Relaxed) && Instant::now() < until {
            match stream.write(left) {
                Ok(0) => return false,
                Ok(n) => left = &left[n..],
                Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(5)),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        left.is_empty()
    }

    fn connection(mut stream: UnixStream, proxy: &winit::event_loop::EventLoopProxy<ShellEvent>, stop: &AtomicBool) {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let connected = Arc::new(AtomicBool::new(true));
        let mut buffer = Vec::new();
        'session: while !stop.load(Ordering::Relaxed) {
            match receive(&mut stream, &mut buffer) {
                Ok(false) => break,
                Err(()) => {
                    let _ = write_reply(&mut stream, &failure("a live request must not exceed 1 MiB"), stop);
                    break;
                }
                Ok(true) => {}
            }
            let line = match take_line(&mut buffer) {
                Ok(Some(line)) => line,
                Ok(None) => {
                    thread::sleep(POLL);
                    continue;
                }
                Err(()) => {
                    if !write_reply(&mut stream, &failure("a live request must be UTF-8"), stop) {
                        break;
                    }
                    continue;
                }
            };
            if line.trim().is_empty() || line.trim_start().starts_with("//") {
                continue;
            }
            let (tx, rx) = mpsc::channel();
            if proxy.send_event(ShellEvent::Inspect { line, reply: tx, connected: connected.clone() }).is_err() {
                break;
            }
            loop {
                if stop.load(Ordering::Relaxed) {
                    break 'session;
                }
                match rx.recv_timeout(POLL) {
                    Ok(reply) => {
                        if !write_reply(&mut stream, &reply, stop) {
                            break 'session;
                        }
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break 'session,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        match receive(&mut stream, &mut buffer) {
                            Ok(true) => {}
                            _ => break 'session,
                        }
                        if proxy.send_event(ShellEvent::Capture).is_err() {
                            break 'session;
                        }
                    }
                }
            }
        }
        connected.store(false, Ordering::Relaxed);
        let _ = proxy.send_event(ShellEvent::Capture);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicU64;

        fn path() -> PathBuf {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            std::env::temp_dir().join(format!("octara-inspect-{}-{}.sock", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
        }

        #[test]
        fn existing_endpoint_is_never_replaced() {
            let path = path();
            fs::write(&path, b"keep").unwrap();
            assert!(Socket::bind(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"keep");
            fs::remove_file(path).unwrap();
        }

        #[test]
        fn socket_is_private_and_cleans_up_its_own_path() {
            let path = path();
            let socket = Socket::bind(&path).unwrap();
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            drop(socket);
            assert!(!path.exists());
        }

        #[test]
        fn cleanup_preserves_a_replacement() {
            let path = path();
            let socket = Socket::bind(&path).unwrap();
            fs::remove_file(&path).unwrap();
            fs::write(&path, b"replacement").unwrap();
            drop(socket);
            assert_eq!(fs::read(&path).unwrap(), b"replacement");
            fs::remove_file(path).unwrap();
        }

        #[test]
        fn framing_waits_for_utf8_and_keeps_following_lines() {
            let mut input = vec![b'"', 0xe4, 0xbd];
            assert_eq!(take_line(&mut input), Ok(None));
            input.extend_from_slice(&[0xa0, b'"', b'\n', b'2', b'\n']);
            assert_eq!(take_line(&mut input).unwrap().as_deref(), Some("\"你\"\n"));
            assert_eq!(take_line(&mut input).unwrap().as_deref(), Some("2\n"));
            input.extend_from_slice(&[0xff, b'\n']);
            assert_eq!(take_line(&mut input), Err(()));
            assert!(input.is_empty());
        }

        #[test]
        fn oversized_input_is_bounded() {
            let (mut reader, mut writer) = UnixStream::pair().unwrap();
            reader.set_nonblocking(true).unwrap();
            let sending = thread::spawn(move || {
                let _ = writer.write_all(&vec![b'x'; REQUEST_LIMIT + 1]);
            });
            let until = Instant::now() + Duration::from_secs(5);
            let mut buffer = Vec::new();
            let refused = loop {
                match receive(&mut reader, &mut buffer) {
                    Err(()) => break true,
                    Ok(false) => break false,
                    Ok(true) if Instant::now() < until => thread::yield_now(),
                    Ok(true) => break false,
                }
            };
            drop(reader);
            sending.join().unwrap();
            assert!(refused);
            assert!(buffer.len() <= REQUEST_LIMIT + 8192);
        }

        #[test]
        fn exact_limit_content_allows_the_terminating_lf() {
            let (mut reader, mut writer) = UnixStream::pair().unwrap();
            reader.set_nonblocking(true).unwrap();
            let (release, hold) = mpsc::channel();
            let sending = thread::spawn(move || {
                let mut line = vec![b'x'; REQUEST_LIMIT];
                line.push(b'\n');
                writer.write_all(&line).unwrap();
                // Keep the socket open until the receiver has read the complete frame.
                let _ = hold.recv_timeout(Duration::from_secs(5));
            });
            let until = Instant::now() + Duration::from_secs(5);
            let mut buffer = Vec::new();
            while buffer.last() != Some(&b'\n') && Instant::now() < until {
                assert_eq!(receive(&mut reader, &mut buffer), Ok(true));
                thread::yield_now();
            }
            assert_eq!(buffer.len(), REQUEST_LIMIT + 1);
            assert_eq!(take_line(&mut buffer).unwrap().unwrap().len(), REQUEST_LIMIT + 1);
            release.send(()).unwrap();
            sending.join().unwrap();
        }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.sent.get()
            && self.connected.load(Ordering::Relaxed)
            && let Some((window, _)) = &self.target
            && window.app().is_failed()
        {
            let mut reply = Reply::fail(&codes::APP_FAILED, "App failed while the request was pending; do not retry");
            reply.delivery = self.delivery.get();
            if self.input && reply.delivery == Some(Delivery::Delivered) {
                reply.completion = Some(Completion::observe(window));
            }
            let mut value = serde_json::to_value(reply).expect("failure reply serializes");
            value["live"] = json!({
                "clock": "realtime", "route": self.route, "pid": std::process::id(),
                "platform": std::env::consts::OS, "delivery": match self.delivery.get() {
                    Some(Delivery::Refused) => "refused",
                    Some(Delivery::Delivered) => "delivered",
                    None => "unknown",
                }
            });
            let _ = self.tx.send(value.to_string());
        }
    }
}
