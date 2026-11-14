//! The CLI connects without discovering or building an app, preserves live replies, and never
//! retries an input whose delivery became unknown.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Server {
    path: PathBuf,
    script: PathBuf,
    task: Option<JoinHandle<Vec<Value>>>,
}

impl Server {
    fn new(replies: Vec<Option<Value>>) -> Self {
        // Keep the Unix socket name short enough for macOS's sockaddr_un.
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = PathBuf::from(format!("/tmp/octara-cli-{}-{nonce}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let task = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "CLI did not connect");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            // Accepted sockets inherit the listener's nonblocking mode on macOS.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut stream = BufReader::new(stream);
            let mut requests = Vec::new();
            for reply in replies {
                let mut line = String::new();
                if stream.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                requests.push(serde_json::from_str(&line).unwrap());
                let Some(reply) = reply else { break };
                writeln!(stream.get_mut(), "{reply}").unwrap();
                stream.get_mut().flush().unwrap();
            }
            requests
        });
        Self { script: path.with_extension("jsonl"), path, task: Some(task) }
    }

    fn path(&self) -> &str {
        self.path.to_str().unwrap()
    }

    fn script(&self, text: &str) -> &str {
        std::fs::write(&self.script, text).unwrap();
        self.script.to_str().unwrap()
    }

    fn finish(mut self) -> Vec<Value> {
        self.task.take().unwrap().join().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(&self.script);
    }
}

fn cli(args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_octara"))
        .args(args)
        .current_dir("/") // A live connection must work without a Cargo.toml.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn inspect_connect_reads_the_window_without_a_harness() {
    let snapshot = json!({
        "schema": "octara.inspect/1", "viewport": [640, 480], "theme": "light", "pending": 0,
        "root": {"role": "button", "id": "inc", "name": "Increase", "source": "src/counter.rs:12:9", "bounds": [10,20,80,40]}
    });
    let server = Server::new(vec![Some(json!({"ok":true,"snapshot":snapshot,"live":{"route":"read"}}))]);
    let output = cli(&["inspect", "--connect", server.path(), "--json"], "");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&output.stdout).unwrap(), snapshot);
    assert_eq!(server.finish(), [json!({"op":"inspect"})]);
}

#[test]
fn scripts_preserve_live_capture_metadata_and_stop_at_the_first_refusal() {
    let capture = json!({"ok":true,"capture":{"renderer":"gpu","settled":true},"live":{"route":"framework-painted"}});
    let server = Server::new(vec![
        Some(capture.clone()),
        Some(json!({"ok":false,"code":"NO_SUCH_ELEMENT","message":"no element named gone"})),
        Some(json!({"ok":true})),
    ]);
    let script = server.script(
        "// comment\n\n{\"op\":\"capture\",\"path\":\"shot.png\"}\n{\"op\":\"tap\",\"id\":\"gone\"}\n{\"op\":\"tap\",\"id\":\"inc\"}\n",
    );
    let output = cli(&["run", "--connect", server.path(), "--script", script, "--json"], "");
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    let replies: Vec<Value> = String::from_utf8(output.stdout).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["capture"], capture["capture"]);
    assert_eq!(replies[0]["live"], capture["live"]);
    assert_eq!(replies[0]["line"], 3);
    assert_eq!(replies[1]["line"], 4);
    assert_eq!(server.finish(), [json!({"op":"capture","path":"shot.png"}), json!({"op":"tap","id":"gone"})]);
}

#[test]
fn guarded_targets_reach_the_host_unchanged_and_stale_inputs_are_not_retried() {
    let refusal = json!({
        "ok": false, "code": "STALE_TARGET", "message": "the observed target was replaced; inspect again",
        "live": {"route": "framework", "delivery": "refused"}
    });
    let server = Server::new(vec![Some(refusal.clone()), Some(json!({"ok": true}))]);
    let input = json!({"op": "tap", "window": 17, "id": "save", "target": "opaque-observed-target"});
    let script = server.script(&format!("{input}\n{{\"op\":\"tap\",\"id\":\"save\"}}\n"));
    let output = cli(&["run", "--connect", server.path(), "--script", script, "--json"], "");
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    let replies: Vec<Value> = String::from_utf8(output.stdout).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["code"], refusal["code"]);
    assert_eq!(replies[0]["live"], refusal["live"]);
    assert_eq!(server.finish(), [input]);
}

#[test]
fn interactive_requests_reply_before_eof_and_continue_after_a_refusal() {
    let server = Server::new(vec![Some(json!({"ok":false,"code":"NO_SUCH_ELEMENT"})), Some(json!({"ok":true,"live":{"route":"read"}}))]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_octara"))
        .args(["run", "--connect", server.path()])
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (send, recv) = std::sync::mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            send.send(line.unwrap()).unwrap();
        }
    });
    writeln!(input, "{{\"op\":\"tap\",\"id\":\"gone\"}}").unwrap();
    input.flush().unwrap();
    let first: Value = serde_json::from_str(&recv.recv_timeout(Duration::from_secs(5)).expect("reply while stdin remains open")).unwrap();
    assert_eq!(first["ok"], false);
    writeln!(input, "{{\"op\":\"inspect\"}}").unwrap();
    input.flush().unwrap();
    let second: Value = serde_json::from_str(&recv.recv_timeout(Duration::from_secs(5)).expect("second reply")).unwrap();
    assert_eq!(second["ok"], true);
    assert_eq!(second["line"], 2);
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    reader.join().unwrap();
    assert_eq!(server.finish().len(), 2);
}

#[test]
fn disconnect_after_input_reports_unknown_outcome_without_retrying() {
    let server = Server::new(vec![None]);
    let script = server.script("{\"op\":\"tap\",\"id\":\"inc\"}\n");
    let output = cli(&["run", "--connect", server.path(), "--script", script], "");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr).unwrap().contains("outcome may be unknown"));
    assert_eq!(server.finish(), [json!({"op":"tap","id":"inc"})]);
}

#[test]
fn connection_arguments_cannot_silently_reconfigure_the_window() {
    for extra in [["--size", "400x300"], ["--theme", "dark"], ["--manifest-path", "missing.toml"], ["--package", "app"]] {
        let output = cli(&["inspect", "--connect", "/tmp/unused.sock", extra[0], extra[1]], "");
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8(output.stderr).unwrap().contains("cannot be used with"));
    }
}
