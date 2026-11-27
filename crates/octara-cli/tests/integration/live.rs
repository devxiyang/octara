//! The CLI talks to a real v2 Unix transport and guards script dispatch by build identity.

use octara::{AppDefinition, WindowDefinition, prelude::*};
use octara_inspect::{
    AgentCommand, AgentReceipt, AgentRequest, HeadlessDriver,
    protocol::{BuildIdentity, Delivery},
    transport::unix::UnixTransport,
};
use std::{
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

struct Server {
    stop: Arc<AtomicBool>,
    process: Option<thread::JoinHandle<()>>,
    actions: Arc<AtomicUsize>,
}

impl Server {
    fn bind(path: &Path, build: BuildIdentity) -> Self {
        let mut transport = UnixTransport::bind(path).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let actions = Arc::new(AtomicUsize::new(0));
        let seen = actions.clone();
        let process = thread::spawn(move || {
            let mut driver = HeadlessDriver::default();
            driver
                .launch(
                    AppDefinition::new()
                        .window(WindowDefinition::new(WindowOptions::default(), |_| text("live fixture").test_id("fixture"))),
                )
                .unwrap();
            let mut handle = |request: AgentRequest| {
                if matches!(request.command, AgentCommand::AdvanceClock { .. }) {
                    seen.fetch_add(1, Ordering::Relaxed);
                }
                let mut receipt = driver.execute(request);
                receipt.build = Some(build.clone());
                receipt
            };
            while !done.load(Ordering::Relaxed) {
                transport.pump(&mut handle).unwrap();
                thread::sleep(Duration::from_millis(2));
            }
        });
        Self { stop, process: Some(process), actions }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.process.take().unwrap().join().unwrap();
    }
}

#[test]
fn a_connected_session_reads_v2_and_checks_build_identity_before_dispatch() {
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temp.path().join("app.sock");
    let receipt_path = temp.path().join("receipt.json");
    let script = temp.path().join("script.jsonl");
    let build = BuildIdentity { digest: "1".repeat(64), pid: 42 };
    let server = Server::bind(&socket, build.clone());
    let start =
        |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_octara")).args(args).arg("--json").arg("--connect").arg(&socket).output().unwrap();
    let receipt = |output: std::process::Output| {
        serde_json::from_slice::<AgentReceipt>(&output.stdout)
            .unwrap_or_else(|error| panic!("{error}\n{}", String::from_utf8_lossy(&output.stderr)))
    };

    let output = start(&["inspect"]);
    assert!(output.status.success());
    let observed = receipt(output);
    assert_eq!(observed.build, Some(build.clone()));
    assert!(observed.snapshot.unwrap().nodes.iter().any(|node| node.test_id.as_deref() == Some("fixture")));

    std::fs::write(&script, "{\"protocol\":\"octara.inspect/2\",\"op\":\"advance_clock\",\"ms\":16}\n").unwrap();
    let write_receipt = |pid| {
        std::fs::write(
            &receipt_path,
            serde_json::json!({
                "schema": "octara.launch/2",
                "package": "fixture",
                "entry": "fixture::startup",
                "build": { "digest": build.digest, "pid": pid }
            })
            .to_string(),
        )
        .unwrap();
    };
    let execute = || start(&["run", "--script", script.to_str().unwrap(), "--require-build", receipt_path.to_str().unwrap()]);
    write_receipt(43);
    let refused = execute();
    assert_eq!(refused.status.code(), Some(1));
    let refused = receipt(refused);
    assert_eq!(refused.code.as_deref(), Some("BUILD_MISMATCH"));
    assert_eq!(refused.delivery, Delivery::Refused);
    assert_eq!(server.actions.load(Ordering::Relaxed), 0);
    write_receipt(42);
    let delivered = execute();
    assert!(delivered.status.success());
    assert!(receipt(delivered).ok);
    assert_eq!(server.actions.load(Ordering::Relaxed), 1);
    drop(server);
    assert!(!socket.exists());
}

#[test]
fn launch_requires_a_fresh_absolute_socket() {
    let tempfile = tempfile::NamedTempFile::new_in("/tmp").unwrap();
    for (path, reason) in [("relative.sock", "absolute"), (tempfile.path().to_str().unwrap(), "already exists")] {
        let output =
            Command::new(env!("CARGO_BIN_EXE_octara")).args(["launch", "--socket", path, "--receipt", "unused.json"]).output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains(reason));
    }
}
