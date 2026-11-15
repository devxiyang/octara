//! A real loopback HTTP peer. Every connection and wait has a finite lifetime.

use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub fn text(value: &str) -> Vec<u8> {
    format!("data: {}\r\n\r\n", serde_json::json!({"choices":[{"index":0,"delta":{"content":value}}]})).into_bytes()
}

pub struct Plan {
    pub status: u16,
    pub chunks: Vec<Vec<u8>>,
    pub end: End,
}

pub enum End {
    Complete,
    Disconnect,
    WaitForClient,
    WaitBeforeHeaders,
}

impl Plan {
    pub fn success(value: &str) -> Self {
        let mut chunks = vec![text(value)];
        chunks.push(b"data: [DONE]\r\n\r\n".to_vec());
        Self { status: 200, chunks, end: End::Complete }
    }

    pub fn hold(prefix: Option<&str>) -> Self {
        Self { status: 200, chunks: prefix.map(text).into_iter().collect(), end: End::WaitForClient }
    }

    pub fn before_headers() -> Self {
        Self { status: 200, chunks: vec![], end: End::WaitBeforeHeaders }
    }
}

#[derive(Default)]
pub struct Observed {
    pub requests: Vec<Value>,
    pub closed: Vec<usize>,
    pub errors: Vec<String>,
}

struct State {
    observed: Observed,
    plans: VecDeque<Plan>,
    stopping: bool,
    connections: Vec<TcpStream>,
    stream_events: HashMap<usize, VecDeque<StreamEvent>>,
}

enum StreamEvent {
    Text(String),
    Done,
}

pub struct Fixture {
    address: SocketAddr,
    shared: Arc<(Mutex<State>, Condvar)>,
    finished: mpsc::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    pub fn new(plans: Vec<Plan>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::new((
            Mutex::new(State {
                observed: Observed::default(),
                plans: plans.into(),
                stopping: false,
                connections: Vec::new(),
                stream_events: HashMap::new(),
            }),
            Condvar::new(),
        ));
        let peer = shared.clone();
        let (done, finished) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut handlers = Vec::new();
            for connection in listener.incoming() {
                let Ok(stream) = connection else { break };
                let mut state = peer.0.lock().unwrap();
                if state.stopping {
                    break;
                }
                state.connections.push(stream.try_clone().unwrap());
                drop(state);
                let peer = peer.clone();
                handlers.push(thread::spawn(move || {
                    if let Err(error) = serve(stream, &peer) {
                        let mut state = peer.0.lock().unwrap();
                        if !state.stopping {
                            state.observed.errors.push(error.to_string());
                        }
                        peer.1.notify_all();
                    }
                }));
            }
            for handler in handlers {
                let _ = handler.join();
            }
            let _ = done.send(());
        });
        Self { address, shared, finished, thread: Some(thread) }
    }

    pub fn endpoint(&self) -> String {
        format!("http://{}/v1/chat/completions", self.address)
    }

    pub fn inspect<T>(&self, read: impl FnOnce(&Observed) -> T) -> T {
        read(&self.shared.0.lock().unwrap().observed)
    }

    /// Extend a held response after a UI action, using the response's original socket.
    pub fn send_text(&self, ordinal: usize, text: &str) {
        self.stream_event(ordinal, StreamEvent::Text(text.into()));
    }

    /// Complete a held response so Chat can start the next queued request.
    pub fn finish(&self, ordinal: usize) {
        self.stream_event(ordinal, StreamEvent::Done);
    }

    fn stream_event(&self, ordinal: usize, event: StreamEvent) {
        let mut state = self.shared.0.lock().unwrap();
        assert!(ordinal < state.observed.requests.len(), "the request must arrive before extending its response");
        state.stream_events.entry(ordinal).or_default().push_back(event);
        self.shared.1.notify_all();
    }

    /// Wake early for server progress; the caller also polls the real UI channel.
    pub fn wait_for_progress(&self) {
        let state = self.shared.0.lock().unwrap();
        let _ = self.shared.1.wait_timeout(state, Duration::from_millis(2)).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let mut state = self.shared.0.lock().unwrap();
        state.stopping = true;
        for connection in &state.connections {
            let _ = connection.shutdown(Shutdown::Both);
        }
        drop(state);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_secs(1));
        self.finished.recv_timeout(Duration::from_secs(3)).expect("fixture threads retire after their sockets close");
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn serve(mut stream: TcpStream, shared: &Arc<(Mutex<State>, Condvar)>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let request = request(&mut stream)?;
    let (ordinal, plan) = {
        let mut state = shared.0.lock().unwrap();
        let ordinal = state.observed.requests.len();
        state.observed.requests.push(request);
        let plan = state.plans.pop_front().ok_or_else(|| io::Error::other("unexpected extra HTTP request"))?;
        shared.1.notify_all();
        (ordinal, plan)
    };
    match respond(&mut stream, shared, ordinal, plan) {
        Err(error)
            if matches!(error.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted) =>
        {
            // Stop may win after request admission and before the first header write.
            closed(shared, ordinal);
            Ok(())
        }
        result => result,
    }
}

fn respond(stream: &mut TcpStream, shared: &Arc<(Mutex<State>, Condvar)>, ordinal: usize, plan: Plan) -> io::Result<()> {
    if matches!(plan.end, End::WaitBeforeHeaders) {
        return wait_for_client(stream, shared, ordinal, false);
    }
    if plan.status != 200 {
        let body = b"private upstream details must not reach the conversation";
        write!(stream, "HTTP/1.1 {} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", plan.status, body.len())?;
        stream.write_all(body)?;
        return Ok(());
    }
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")?;
    for bytes in plan.chunks {
        write_chunk(stream, &bytes)?;
    }
    match plan.end {
        End::Complete => stream.write_all(b"0\r\n\r\n")?,
        End::Disconnect => {
            stream.shutdown(Shutdown::Both)?;
        }
        End::WaitForClient => wait_for_client(stream, shared, ordinal, true)?,
        End::WaitBeforeHeaders => unreachable!("handled before writing response headers"),
    }
    Ok(())
}

fn write_chunk(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    write!(stream, "{:x}\r\n", bytes.len())?;
    stream.write_all(bytes)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

fn closed(shared: &Arc<(Mutex<State>, Condvar)>, ordinal: usize) {
    let mut state = shared.0.lock().unwrap();
    if !state.observed.closed.contains(&ordinal) {
        state.observed.closed.push(ordinal);
    }
    shared.1.notify_all();
}

fn wait_for_client(stream: &mut TcpStream, shared: &Arc<(Mutex<State>, Condvar)>, ordinal: usize, streaming: bool) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut byte = [0];
    loop {
        if shared.0.lock().unwrap().stopping {
            return Ok(());
        }
        if streaming {
            let events = shared.0.lock().unwrap().stream_events.remove(&ordinal).unwrap_or_default();
            for event in events {
                match event {
                    StreamEvent::Text(value) => write_chunk(stream, &text(&value))?,
                    StreamEvent::Done => {
                        write_chunk(stream, b"data: [DONE]\r\n\r\n")?;
                        stream.write_all(b"0\r\n\r\n")?;
                        return Ok(());
                    }
                }
            }
        }
        match stream.read(&mut byte) {
            Ok(0) => break,
            Err(error) if matches!(error.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted) => break,
            Err(error) if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(io::Error::other("client left the stream open"));
                }
            }
            Ok(_) => return Err(io::Error::other("unexpected pipelined request")),
            Err(error) => return Err(error),
        }
    }
    closed(shared, ordinal);
    Ok(())
}

fn request(stream: &mut TcpStream) -> io::Result<Value> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if !line.starts_with("POST /v1/chat/completions HTTP/1.1") {
        return Err(io::Error::other("unexpected request target"));
    }
    let mut length = None;
    let mut headers = line.len();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        headers += line.len();
        if headers > 16 * 1024 {
            return Err(io::Error::other("request headers too large"));
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let length = length.filter(|n| *n <= 1 << 20).ok_or_else(|| io::Error::other("missing or excessive request length"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}
