//! The opt-in live Inspector: one local connection, ordered requests, UI-thread execution.

use super::{Shell, ShellEvent, ww};
use octara::app::WindowId;
use octara::diagnostics as codes;
use octara::inspect::{Delivery, Reply, Request, TargetGuard};
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
    "secondary_tap",
    "focus",
    "hover",
    "key",
    "type",
    "invoke",
    "settle",
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
            eprintln!("octara-shell: inspector {}", path.display());
            Ok(Some(Server { _wire: server }))
        }
        #[cfg(not(unix))]
        {
            let _ = (path, proxy);
            Err("OCTARA_INSPECT currently supports Unix sockets on macOS and Linux only".into())
        }
    }
}

/// One response belongs to its connection, even if that connection has since closed.
pub(crate) struct Responder {
    tx: Sender<String>,
    pub(crate) connected: Arc<AtomicBool>,
    route: &'static str,
    input: bool,
    target: Option<(octara::app::Window, TargetGuard)>,
}

impl Responder {
    fn new(tx: Sender<String>, connected: Arc<AtomicBool>, route: &'static str, input: bool) -> Self {
        Self { tx, connected, route, input, target: None }
    }

    fn with_target(mut self, window: &octara::app::Window, guard: Option<&TargetGuard>) -> Self {
        self.target = guard.map(|guard| (window.clone(), guard.clone()));
        self
    }

    pub(crate) fn target_current(&self) -> bool {
        self.target.as_ref().is_none_or(|(window, guard)| guard.is_current(window))
    }

    pub(crate) fn send(self, reply: Reply) {
        self.send_with(reply, json!({}));
    }

    fn send_wait(self, reply: Reply, waiting: Option<(bool, Vec<String>)>) {
        let extra = waiting.map_or_else(|| json!({}), |(settled, pending)| json!({ "settled": settled, "pending": pending }));
        self.send_with(reply, extra);
    }

    fn send_gesture(self, reply: Reply, delivered: bool) {
        let delivered = reply.delivery.as_ref().map_or(delivered, |delivery| *delivery == Delivery::Delivered);
        self.send_with(reply, json!({ "delivery": if delivered { "delivered" } else { "refused" } }));
    }

    fn send_with(self, mut reply: Reply, extra: serde_json::Value) {
        if !self.connected.load(Ordering::Relaxed) {
            return;
        }
        let mut live = json!({ "clock": "realtime", "route": self.route, "pid": std::process::id(), "platform": std::env::consts::OS });
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
        if reply.snapshot.is_some() {
            live["operations"] = json!(OPERATIONS);
            live["transport"] = "unix".into();
            live["target_validation"] = "identity".into();
        }
        live.as_object_mut().expect("live metadata is an object").extend(extra.as_object().expect("extra metadata is an object").clone());
        let mut value = serde_json::to_value(reply).expect("an inspector reply serializes");
        value["live"] = live;
        let _ = self.tx.send(value.to_string());
    }
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
    /// Cancel our held pointer before ticking timers once transport has observed disconnect.
    pub(crate) fn cancel_disconnected(&self, app: &octara::app::App) {
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
        | Request::SecondaryTap { .. }
        | Request::Type { .. }
        | Request::Key { .. } => ("framework", true),
        Request::Focus { .. }
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
        self.tick();
        if let Some(app) = &self.app {
            app.poll();
        }
        self.show();
        let window = self.main_window().or_else(|| self.app.as_ref().and_then(|app| app.windows().first().and_then(|id| app.window(*id))));
        let Some(window) = window else {
            Responder::new(tx, connected, "query", false).send(Reply::fail(&codes::BAD_REQUEST, "the app has no open main window"));
            return;
        };
        let octara::inspect::ParsedRequest { window: mut target, request, guard } =
            match octara::inspect::parse_guarded_request(&window, &line) {
                Ok(parsed) => parsed,
                Err(reply) => {
                    let (route, input) = route_line(&line);
                    Responder::new(tx, connected, route, input).send(*reply);
                    return;
                }
            };
        let (route, input) = route(&request);
        let response = Responder::new(tx, connected, route, input).with_target(&target, guard.as_ref());
        match request {
            Request::Advance { .. } => response.send(Reply::fail(
                &codes::BAD_REQUEST,
                "this operation is not supported by the realtime live host; inspect live.operations for supported requests",
            )),
            Request::DoubleTap { ref id } | Request::LongPress { ref id } => {
                let kind = if matches!(&request, Request::DoubleTap { .. }) {
                    super::gesture::Kind::DoubleTap
                } else {
                    super::gesture::Kind::LongPress
                };
                match super::gesture::Job::start_guarded(&mut target, id, kind, Instant::now(), guard) {
                    Ok(job) => {
                        self.inspect_pending = Some(Pending::Gesture { job, reply: response });
                        self.show();
                        self.inspect_poll();
                    }
                    Err(reply) => response.send_gesture(*reply, false),
                }
            }
            Request::Size { width, height } => match super::resize::Job::start(self, &mut target, width, height) {
                Ok(job) => {
                    self.inspect_pending = Some(Pending::Resize { job, reply: response });
                    self.show();
                    self.inspect_poll();
                }
                Err(reply) if reply.code.as_deref() == Some(codes::STEP_FAILED.id) => {
                    let actual = reply.actual.clone();
                    response.send_with(*reply, json!({ "delivery": "delivered", "resize": actual }));
                }
                Err(reply) => response.send(*reply),
            },
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
                self.show();
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
                match job.poll(&mut window, Instant::now()) {
                    Some(result) => reply.send_gesture(result, job.delivered()),
                    None => self.inspect_pending = Some(Pending::Gesture { job, reply }),
                }
                self.show();
            }
            Pending::Resize { job, reply } => {
                if !reply.target_current() {
                    reply.send_with(
                        Reply::fail(&codes::STALE_TARGET, "the observed window changed after the resize request; inspect again"),
                        json!({ "delivery": "delivered" }),
                    );
                    return;
                }
                match job.poll(self) {
                    Some(done) => reply.send_with(done.reply, json!({ "resize": done.resize, "delivery": "delivered" })),
                    None => self.inspect_pending = Some(Pending::Resize { job, reply }),
                }
            }
        }
    }
}

#[cfg(test)]
mod target_feedback {
    use super::*;

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
