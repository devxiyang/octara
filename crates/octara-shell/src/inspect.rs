//! The opt-in live Inspector: one local connection, ordered requests, UI-thread execution.

use super::{Shell, ShellEvent, ww};
use octara::app::{App, TargetGuard, Window, WindowId};
use octara::capture::PaintFrame;
use octara::diagnostics as codes;
use octara::inspect::{Completion, Delivery, LayoutState, PointerStep, Reply, Request, SurfaceState};
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
    "tap_at",
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
    wire: wire::Server,
    build: Option<Arc<str>>,
}

impl Server {
    pub(crate) fn from_env(proxy: winit::event_loop::EventLoopProxy<ShellEvent>, trace: bool) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os("OCTARA_INSPECT") else { return Ok(None) };
        #[cfg(unix)]
        {
            let path = std::path::PathBuf::from(path);
            if !path.is_absolute() {
                return Err("OCTARA_INSPECT needs an absolute Unix socket path".into());
            }
            let wire = wire::Server::start(&path, move |event| proxy.send_event(event).is_ok(), trace)?;
            let build = std::env::var("OCTARA_INSPECT_BUILD")
                .ok()
                .filter(|digest| digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
                .map(Arc::from);
            eprintln!("octara-shell: inspector {}", path.display());
            Ok(Some(Server { wire, build }))
        }
        #[cfg(not(unix))]
        {
            let _ = (path, proxy, trace);
            Err("OCTARA_INSPECT requires a host with Unix sockets".into())
        }
    }

    fn take_forwarded(&self, connected: &Arc<AtomicBool>) -> Option<Instant> {
        #[cfg(unix)]
        {
            self.wire.take_forwarded(connected)
        }
        #[cfg(not(unix))]
        {
            let _ = connected;
            None
        }
    }
}

/// One response belongs to its connection, even if that connection has since closed.
pub(crate) struct Responder {
    tx: Sender<String>,
    // Drop the sender before waking the wire, even when there is no reply to send.
    #[cfg(unix)]
    _wake: Option<wire::ReplyWake>,
    pub(crate) connected: Arc<AtomicBool>,
    route: &'static str,
    input: bool,
    build: Option<Arc<str>>,
    target: Option<(Window, Option<TargetGuard>)>,
    sent: std::cell::Cell<bool>,
    delivery: std::cell::Cell<Option<Delivery>>,
    // Keep completed gesture phases available if a later phase unwinds.
    pointer: Vec<PointerStep>,
}

impl Responder {
    fn new(tx: Sender<String>, connected: Arc<AtomicBool>, route: &'static str, input: bool) -> Self {
        Self {
            tx,
            #[cfg(unix)]
            _wake: None,
            connected,
            route,
            input,
            build: None,
            target: None,
            sent: std::cell::Cell::new(false),
            delivery: std::cell::Cell::new(Some(Delivery::Refused)),
            pointer: Vec::new(),
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

    fn send_gesture(mut self, mut reply: Reply, delivered: bool) {
        if reply.delivery.is_none() {
            reply.delivery = if delivered {
                Some(Delivery::Delivered)
            } else if reply.code.as_deref() == Some(codes::APP_FAILED.id) {
                None
            } else {
                Some(Delivery::Refused)
            };
        }
        self.pointer.append(&mut reply.pointer);
        reply.pointer = std::mem::take(&mut self.pointer);
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
        if let Some((window, _)) = self.target.as_ref().filter(|_| reports_completion) {
            let (completion, paint) = observe_completion(window);
            reply.completion = Some(completion);
            reply.paint = paint.map(Box::new);
        } else {
            reply.completion = None;
        }
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

impl Drop for Responder {
    fn drop(&mut self) {
        if !self.sent.get()
            && self.connected.load(Ordering::Relaxed)
            && let Some((window, _)) = &self.target
            && window.app().is_failed()
        {
            let mut reply = Reply::fail(&codes::APP_FAILED, "App failed while the request was pending; do not retry");
            reply.delivery = self.delivery.get();
            reply.pointer = std::mem::take(&mut self.pointer);
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

fn observe_completion(window: &Window) -> (Completion, Option<PaintFrame>) {
    let mut completion = Completion::observe(window);
    if completion.layout != LayoutState::Unavailable
        && let Some((last_submission, submitted, paint)) = ww(window, |host| {
            let submitted =
                window.published_revision().is_some_and(|revision| host.has_submitted(revision) && host.matches_drawable(window.scale()));
            (host.last_submission(), submitted, host.submitted_paint().cloned())
        })
    {
        completion.surface = match last_submission {
            Some(frame) if completion.layout == LayoutState::Settled && submitted => SurfaceState::Submitted { frame },
            _ => SurfaceState::Pending { last_submission },
        };
        return (completion, paint);
    }
    (completion, None)
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
    pub(crate) fn cancel_disconnected(&self, app: &App) {
        if app.is_failed() {
            return;
        }
        if let Self::Gesture { job, reply } = self
            && !reply.connected.load(Ordering::Relaxed)
            && let Some(mut window) = app.window(job.window())
        {
            job.cancel(&mut window, None);
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

    pub(crate) fn deadline(&self, app: &App, started: Instant) -> Option<Instant> {
        match self {
            Self::Settle(waiting) => Some(waiting.until),
            Self::Gesture { job, .. } => app.window(job.window()).map(|window| job.deadline(&window, started)),
            Self::Resize { job, .. } => Some(job.deadline()),
        }
    }
}

fn route(request: &Request) -> (&'static str, bool) {
    match request {
        Request::Tap { .. }
        | Request::TapAt { .. }
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
    pub(crate) fn inspect_request(&mut self, line: String, tx: Sender<String>, connected: Arc<AtomicBool>, entered: Option<Instant>) {
        if !connected.load(Ordering::Relaxed) {
            return;
        }
        let build = self.inspector.as_ref().and_then(|server| server.build.clone());
        #[cfg(unix)]
        let wake = self.inspector.as_ref().map(|server| server.wire.reply_wake());
        let respond = move |route, input| {
            let mut response = Responder::new(tx, connected, route, input);
            response.build = build;
            #[cfg(unix)]
            {
                response._wake = wake;
            }
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
        let mut response = respond(route, input).with_target(&target, guard.as_ref());
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
                match super::gesture::Job::start_guarded(&mut target, kind, Instant::now(), guard, &mut response.pointer) {
                    Ok(mut job) => {
                        job.trace_epoch = self.shared.trace.then_some(self.shared.trace_epoch);
                        self.inspect_pending = Some(Pending::gesture(job, response));
                        self.inspect_step();
                        self.show();
                    }
                    Err(reply) => response.send_gesture(*reply, false),
                }
            }
            Request::Size { width, height } => {
                let timing = entered.map(|handler| super::trace::ResizeRequest {
                    forwarded: self.inspector.as_ref().and_then(|server| server.take_forwarded(&response.connected)),
                    handler,
                });
                response.with_current_resize_target(|response| match super::resize::Job::start(self, &mut target, width, height, timing) {
                    Ok(job) => {
                        response.delivery.set(Some(Delivery::Delivered));
                        self.inspect_pending = Some(Pending::Resize { job, reply: response });
                        self.inspect_resize_ready();
                        self.show();
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
                self.capture = Some(super::capture::Job::live(target.id(), path, Duration::from_millis(ms), scale, pixels, response));
                self.capture_step();
                self.show();
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
                self.inspect_step();
            }
            request => {
                let trace = (input && self.shared.trace).then(|| super::trace::InspectInput::start(&target));
                let reply = octara::inspect::handle_guarded(&mut target, request, guard.as_ref());
                if input {
                    response.delivery.set(reply.delivery);
                    if let Some(trace) = trace
                        && reply.delivery == Some(Delivery::Delivered)
                    {
                        trace.log(self.shared.trace_epoch, &target, route, None);
                    }
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

    /// Resize receipts can go out as soon as they have geometry, before the next draw.
    pub(crate) fn inspect_resize_ready(&mut self) {
        if matches!(self.inspect_pending, Some(Pending::Resize { .. })) {
            self.inspect_step();
        }
    }

    /// Recheck the waiting operation after a host event or one of its own deadlines.
    pub(crate) fn inspect_step(&mut self) {
        let Some(pending) = self.inspect_pending.take() else { return };
        if !pending.connected() {
            if let Some(app) = &self.app {
                pending.cancel_disconnected(app);
            }
            return;
        }
        match pending {
            Pending::Settle(waiting) => {
                let now = Instant::now();
                if now >= waiting.until {
                    self.tick_at(now);
                }
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
                } else if now >= waiting.until {
                    waiting.reply.send_wait(
                        Reply::fail(&codes::STEP_FAILED, "the live wait budget ended with work still pending"),
                        Some((false, pending)),
                    );
                } else {
                    self.inspect_pending = Some(Pending::Settle(waiting));
                }
            }
            Pending::Gesture { mut job, mut reply } => {
                let now = Instant::now();
                self.tick_at(now);
                let Some(mut window) = self.app.as_ref().and_then(|app| app.window(job.window())) else {
                    reply.send_gesture(Reply::fail(&codes::STEP_FAILED, "the target window closed during the gesture"), job.delivered());
                    return;
                };
                if !reply.connected.load(Ordering::Relaxed) {
                    job.cancel(&mut window, None);
                    return;
                }
                if !job.delivered() {
                    reply.begin();
                }
                match job.poll(&mut window, now, &mut reply.pointer) {
                    Some(result) => reply.send_gesture(result, job.delivered()),
                    None => {
                        self.inspect_pending = Some(Pending::gesture(job, reply));
                    }
                }
            }
            Pending::Resize { job, reply } => reply.with_current_resize_target(|reply| match job.complete(self) {
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
        let mut response = Responder::new(send, Arc::new(AtomicBool::new(true)), "framework", true).with_target(&window, Some(&guard));
        response.begin();
        let reply = super::super::gesture::Job::start_guarded(
            &mut window,
            super::super::gesture::Kind::DoubleTap,
            Instant::now(),
            guard,
            &mut response.pointer,
        )
        .err()
        .expect("the first click panicked");
        response.send_gesture(*reply, false);

        let reply: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
        assert_eq!(reply["code"], "APP_FAILED");
        assert_eq!(reply["live"]["delivery"], "unknown");
        assert!(reply.get("delivery").is_none() && reply.get("completion").is_none());
        let steps = reply["pointer"].as_array().unwrap();
        assert_eq!(steps.iter().map(|step| step["phase"].as_str().unwrap()).collect::<Vec<_>>(), ["move", "down"]);
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
            let mut response = Responder::new(send, connection.clone(), "framework", true).with_target(&window, None);
            response.begin();
            if matches!(kind, Kind::DoubleTap) {
                window.tap("target").unwrap();
                assert!(!window.pointer_click_cooldown().is_zero());
            }
            let job = Job::start(&mut window, "target", kind, Instant::now(), &mut response.pointer).unwrap();
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
                if matches!(kind, Kind::LongPress) {
                    let steps = reply["pointer"].as_array().unwrap();
                    assert_eq!(steps.len(), 1);
                    assert_eq!(steps[0]["phase"], "down");
                } else {
                    assert!(reply.get("pointer").is_none());
                }
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
    fn a_bad_tap_at_target_keeps_the_framework_input_route() {
        let window = Window::headless(octara::view::el);
        let line = r#"{"op":"tap_at","x":12.5,"y":16,"drawing_revision":1,"target":false}"#;
        let failure = octara::inspect::parse_guarded_request(&window, line).err().expect("the target must be an opaque string");
        let reply = response(line, *failure, false);
        assert_eq!(reply["code"], "BAD_REQUEST");
        assert_eq!(reply["delivery"], "refused");
        assert_eq!(reply["live"]["route"], "framework");
        assert_eq!(reply["live"]["delivery"], "refused");
        assert!(reply.get("pointer").is_none());
        window.app().shutdown();
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
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use rustix::io::Errno;
    use std::fs;
    use std::io::{ErrorKind, Read, Write};
    use std::net::Shutdown;
    use std::os::fd::AsFd;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, Weak, mpsc};
    use std::thread::{self, JoinHandle};

    const REQUEST_LIMIT: usize = 1024 * 1024;
    const REPLY_LIMIT: usize = 16 * 1024 * 1024;
    const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

    // The wire forwards one request at a time. The connection identity prevents a stale
    // event from consuming the next connection's timestamp.
    type Forwarded = Mutex<Option<(Weak<AtomicBool>, Instant)>>;

    pub(super) struct Server {
        stopped: Arc<AtomicBool>,
        wake: Arc<UnixStream>,
        forwarded: Option<Arc<Forwarded>>,
        worker: Option<JoinHandle<()>>,
        _socket: Socket,
    }

    pub(super) struct ReplyWake(Arc<UnixStream>);

    impl Drop for ReplyWake {
        fn drop(&mut self) {
            let mut wake = self.0.as_ref();
            // A full wake socket already has notifications waiting to be read.
            while let Err(error) = wake.write(&[0]) {
                if error.kind() != ErrorKind::Interrupted {
                    break;
                }
            }
        }
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
        pub(super) fn start(path: &Path, post: impl Fn(ShellEvent) -> bool + Send + 'static, trace: bool) -> Result<Self, String> {
            let socket = Socket::bind(path)?;
            let listener = socket.listener.try_clone().map_err(|e| e.to_string())?;
            let (wake, notify) = UnixStream::pair().map_err(|e| e.to_string())?;
            wake.set_nonblocking(true).map_err(|e| e.to_string())?;
            notify.set_nonblocking(true).map_err(|e| e.to_string())?;
            let stopped = Arc::new(AtomicBool::new(false));
            let readiness = Readiness { wake, stopped: stopped.clone() };
            let forwarded = trace.then(|| Arc::new(Mutex::new(None)));
            let trace_timing = forwarded.clone();
            let worker = thread::spawn(move || {
                while let Some(ready) = readiness.wait(&listener, PollFlags::IN, None) {
                    if !ready.contains(PollFlags::IN) {
                        continue;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => connection(stream, &post, &readiness, trace_timing.as_deref()),
                        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                        Err(_) => break,
                    }
                }
            });
            Ok(Self { stopped, wake: Arc::new(notify), forwarded, worker: Some(worker), _socket: socket })
        }

        pub(super) fn reply_wake(&self) -> ReplyWake {
            ReplyWake(self.wake.clone())
        }

        pub(super) fn take_forwarded(&self, connected: &Arc<AtomicBool>) -> Option<Instant> {
            self.forwarded
                .as_ref()?
                .lock()
                .ok()?
                .take_if(|(connection, _)| std::ptr::eq(connection.as_ptr(), Arc::as_ptr(connected)))
                .map(|(_, at)| at)
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            // This delivers EOF even if a pending responder still owns a wake handle.
            let _ = self.wake.shutdown(Shutdown::Write);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    struct Readiness {
        wake: UnixStream,
        stopped: Arc<AtomicBool>,
    }

    impl Readiness {
        fn running(&self) -> bool {
            !self.stopped.load(Ordering::Acquire)
        }

        fn wait(&self, fd: &impl AsFd, interest: PollFlags, until: Option<Instant>) -> Option<PollFlags> {
            let mut fds = [PollFd::new(&self.wake, PollFlags::IN), PollFd::new(fd, interest)];
            while self.running() {
                let timeout =
                    until.map(|at| Timespec::try_from(at.saturating_duration_since(Instant::now())).expect("bounded wire timeout"));
                match poll(&mut fds, timeout.as_ref()) {
                    Ok(0) => return None,
                    Ok(_) => {
                        if !self.running() || (!fds[0].revents().is_empty() && !drain_wake(&self.wake)) {
                            return None;
                        }
                        let ready = fds[1].revents();
                        return (!ready.intersects(PollFlags::ERR | PollFlags::HUP | PollFlags::NVAL)).then_some(ready);
                    }
                    Err(Errno::INTR) => {}
                    Err(_) => return None,
                }
            }
            None
        }
    }

    fn drain_wake(mut wake: &UnixStream) -> bool {
        let mut bytes = [0; 64];
        loop {
            match wake.read(&mut bytes) {
                Ok(0) => return false,
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => return true,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return false,
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
            let room = REQUEST_LIMIT + 1 - buffer.len();
            let read = bytes.len().min(room + 1);
            match stream.read(&mut bytes[..read]) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    // The limit applies to content, with one additional byte for the LF.
                    if n > room || (n == room && bytes[n - 1] != b'\n') {
                        return Err(());
                    }
                    buffer.extend_from_slice(&bytes[..n]);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(true),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Ok(false),
            }
        }
    }

    fn write_reply(stream: &mut UnixStream, mut reply: String, readiness: &Readiness, until: Instant) -> bool {
        // The operation may already have run. Close rather than replace its result with a
        // misleading validation refusal; the client must treat the outcome as unknown.
        if reply.len() > REPLY_LIMIT {
            return false;
        }
        reply.push('\n');
        let mut left = reply.as_bytes();
        while !left.is_empty() && readiness.running() && Instant::now() < until {
            match stream.write(left) {
                Ok(0) => return false,
                Ok(n) => left = &left[n..],
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if readiness.wait(stream, PollFlags::OUT, Some(until)).is_none() {
                        return false;
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return false,
            }
        }
        left.is_empty()
    }

    fn await_reply(stream: &mut UnixStream, buffer: &mut Vec<u8>, readiness: &Readiness, rx: mpsc::Receiver<String>) -> Option<String> {
        while readiness.running() {
            match rx.try_recv() {
                Ok(reply) => return Some(reply),
                Err(mpsc::TryRecvError::Disconnected) => return None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if readiness.wait(stream, PollFlags::IN, None)?.contains(PollFlags::IN) && receive(stream, buffer) != Ok(true) {
                return None;
            }
        }
        None
    }

    fn connection(mut stream: UnixStream, post: &impl Fn(ShellEvent) -> bool, readiness: &Readiness, forwarded: Option<&Forwarded>) {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let connected = Arc::new(AtomicBool::new(true));
        let mut buffer = Vec::new();
        while readiness.running() {
            match receive(&mut stream, &mut buffer) {
                Ok(false) => break,
                Err(()) => {
                    let _ = write_reply(
                        &mut stream,
                        failure("a live request must not exceed 1 MiB"),
                        readiness,
                        Instant::now() + WRITE_TIMEOUT,
                    );
                    break;
                }
                Ok(true) => {}
            }
            let line = match take_line(&mut buffer) {
                Ok(Some(line)) => line,
                Ok(None) => {
                    if readiness.wait(&stream, PollFlags::IN, None).is_none() {
                        break;
                    }
                    continue;
                }
                Err(()) => {
                    if !write_reply(&mut stream, failure("a live request must be UTF-8"), readiness, Instant::now() + WRITE_TIMEOUT) {
                        break;
                    }
                    continue;
                }
            };
            if line.trim().is_empty() || line.trim_start().starts_with("//") {
                continue;
            }
            let (tx, rx) = mpsc::channel();
            let event = ShellEvent::Inspect { line, reply: tx, connected: connected.clone() };
            if let Some(mut forwarded) = forwarded.and_then(|forwarded| forwarded.lock().ok()) {
                *forwarded = Some((Arc::downgrade(&connected), Instant::now()));
            }
            if !post(event) {
                break;
            }
            let Some(reply) = await_reply(&mut stream, &mut buffer, readiness, rx) else {
                connected.store(false, Ordering::Relaxed);
                let _ = post(ShellEvent::Capture);
                return;
            };
            if !write_reply(&mut stream, reply, readiness, Instant::now() + WRITE_TIMEOUT) {
                break;
            }
        }
        connected.store(false, Ordering::Relaxed);
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::{BufRead, BufReader};
        use std::sync::atomic::AtomicU64;

        const TEST_TIMEOUT: Duration = Duration::from_secs(2);

        fn path() -> PathBuf {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            std::env::temp_dir().join(format!("octara-inspect-{}-{}.sock", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
        }

        fn server() -> (Server, mpsc::Receiver<ShellEvent>) {
            let (post, events) = mpsc::channel();
            let server = Server::start(&path(), move |event| post.send(event).is_ok(), false).unwrap();
            (server, events)
        }

        fn client(server: &Server) -> UnixStream {
            let stream = UnixStream::connect(&server._socket.path).unwrap();
            stream.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
            stream.set_write_timeout(Some(TEST_TIMEOUT)).unwrap();
            stream
        }

        fn readiness() -> (Readiness, UnixStream) {
            let (wake, notify) = UnixStream::pair().unwrap();
            wake.set_nonblocking(true).unwrap();
            (Readiness { wake, stopped: Arc::new(AtomicBool::new(false)) }, notify)
        }

        fn responder(server: &Server, events: &mpsc::Receiver<ShellEvent>, expected_line: &str) -> Responder {
            let event = events.recv_timeout(TEST_TIMEOUT).unwrap();
            let ShellEvent::Inspect { line, reply, connected } = event else { panic!("expected an Inspector request, got {event:?}") };
            assert_eq!(line, expected_line);
            let mut response = Responder::new(reply, connected, "query", false);
            response._wake = Some(server.reply_wake());
            response
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
            let (readiness, _notify) = readiness();
            let (release, hold) = mpsc::channel();
            let sending = thread::spawn(move || {
                writer.write_all(&vec![b'x'; REQUEST_LIMIT + 1]).unwrap();
                let _ = hold.recv_timeout(TEST_TIMEOUT);
            });
            let until = Instant::now() + TEST_TIMEOUT;
            let mut buffer = Vec::new();
            let refused = loop {
                assert!(readiness.wait(&reader, PollFlags::IN, Some(until)).is_some());
                match receive(&mut reader, &mut buffer) {
                    Err(()) => break true,
                    Ok(false) => break false,
                    Ok(true) => {}
                }
            };
            release.send(()).unwrap();
            sending.join().unwrap();
            assert!(refused);
            assert!(buffer.len() <= REQUEST_LIMIT);
        }

        #[test]
        fn exact_limit_content_allows_the_terminating_lf() {
            let (mut reader, mut writer) = UnixStream::pair().unwrap();
            reader.set_nonblocking(true).unwrap();
            let (readiness, _notify) = readiness();
            let (release, hold) = mpsc::channel();
            let sending = thread::spawn(move || {
                let mut line = vec![b'x'; REQUEST_LIMIT];
                line.push(b'\n');
                writer.write_all(&line).unwrap();
                // Keep the socket open until the receiver has read the complete frame.
                let _ = hold.recv_timeout(TEST_TIMEOUT);
            });
            let until = Instant::now() + TEST_TIMEOUT;
            let mut buffer = Vec::new();
            while buffer.last() != Some(&b'\n') {
                assert!(readiness.wait(&reader, PollFlags::IN, Some(until)).is_some());
                assert_eq!(receive(&mut reader, &mut buffer), Ok(true));
            }
            assert_eq!(buffer.len(), REQUEST_LIMIT + 1);
            assert_eq!(take_line(&mut buffer).unwrap().unwrap().len(), REQUEST_LIMIT + 1);
            release.send(()).unwrap();
            sending.join().unwrap();
        }

        #[test]
        fn pending_request_waits_for_reply_before_forwarding_the_next_line() {
            let (server, events) = server();
            let mut client = BufReader::new(client(&server));
            client.get_mut().write_all(b"1\n2\n").unwrap();
            let first = responder(&server, &events, "1\n");
            assert!(matches!(events.recv_timeout(Duration::from_millis(120)), Err(mpsc::RecvTimeoutError::Timeout)));

            first.send(Reply::ok());
            let mut reply = String::new();
            client.read_line(&mut reply).unwrap();
            assert_eq!(serde_json::from_str::<serde_json::Value>(&reply).unwrap()["ok"], true);

            let second = responder(&server, &events, "2\n");
            second.send(Reply::fail(&codes::BAD_REQUEST, "second"));
            reply.clear();
            client.read_line(&mut reply).unwrap();
            assert_eq!(serde_json::from_str::<serde_json::Value>(&reply).unwrap()["ok"], false);
        }

        #[test]
        fn peer_close_notifies_host_and_dropping_a_responder_closes_its_client() {
            let (server, events) = server();
            let mut first = client(&server);
            first.write_all(b"1\n").unwrap();
            let disconnected = responder(&server, &events, "1\n");

            drop(first);
            assert!(matches!(events.recv_timeout(TEST_TIMEOUT), Ok(ShellEvent::Capture)));
            assert!(!disconnected.connected.load(Ordering::Relaxed));

            let mut second = client(&server);
            second.write_all(b"2\n").unwrap();
            let dropped = responder(&server, &events, "2\n");
            drop(disconnected);
            drop(dropped);
            assert_eq!(second.read(&mut [0]).unwrap(), 0);
        }

        #[test]
        fn shutdown_wakes_waiting_worker_even_with_a_live_responder() {
            let (server, events) = server();
            let mut client = client(&server);
            client.write_all(b"1\n").unwrap();
            let pending = responder(&server, &events, "1\n");

            let endpoint = server._socket.path.clone();
            drop(server);
            assert!(!endpoint.exists());
            assert!(!pending.connected.load(Ordering::Relaxed));
            assert_eq!(client.read(&mut [0]).unwrap(), 0);
        }

        #[test]
        fn stopped_worker_does_not_process_queued_input() {
            let (stream, mut peer) = UnixStream::pair().unwrap();
            let (readiness, _notify) = readiness();
            peer.write_all(b"// queued\n1\n").unwrap();
            readiness.stopped.store(true, Ordering::Release);
            connection(stream, &|_| panic!("a stopped worker must not post an event"), &readiness, None);
        }

        #[test]
        fn writes_resume_after_backpressure() {
            let (mut stream, peer) = UnixStream::pair().unwrap();
            let (readiness, _notify) = readiness();
            stream.set_nonblocking(true).unwrap();
            peer.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
            let size = 4 * 1024 * 1024;
            let writing = thread::spawn(move || write_reply(&mut stream, "x".repeat(size), &readiness, Instant::now() + TEST_TIMEOUT));

            let mut reply = Vec::new();
            BufReader::new(peer).read_until(b'\n', &mut reply).unwrap();
            assert!(writing.join().unwrap());
            assert_eq!(reply.len(), size + 1);
            assert_eq!(reply.last(), Some(&b'\n'));
        }

        #[test]
        fn shutdown_interrupts_a_stalled_writer() {
            let (mut stream, mut peer) = UnixStream::pair().unwrap();
            let (readiness, notify) = readiness();
            stream.set_nonblocking(true).unwrap();
            peer.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
            let (complete, result) = mpsc::channel();
            let writing = thread::spawn(move || {
                let sent = write_reply(&mut stream, "x".repeat(REPLY_LIMIT), &readiness, Instant::now() + WRITE_TIMEOUT);
                complete.send(sent).unwrap();
            });

            peer.read_exact(&mut [0]).unwrap();
            notify.shutdown(Shutdown::Write).unwrap();
            assert!(!result.recv_timeout(TEST_TIMEOUT).unwrap());
            writing.join().unwrap();
        }
    }
}
