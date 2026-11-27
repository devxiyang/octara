//! Exercise the native dev runner with an actual Cargo source rebuild.

use octara_cli::live::Client;
use octara_inspect::{AgentAction, AgentCommand, AgentReceipt, AgentRequest};
use octara_model::semantics::A11yAction;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

const FIXTURE: &str = r#"
use octara::prelude::*;
use std::io::Write;

pub fn startup() -> AppDefinition {
    AppDefinition::new().state_schema("dev-fixture/1").window(WindowDefinition::new(WindowOptions::default(), |b| {
        let saved = b.persisted("counter", || 4_u32);
        let transient = b.signal(0_u32);
        b.on_cleanup(|_| {
            let path = std::env::var_os("OCTARA_DEV_FIXTURE_CLEANUP").unwrap();
            let mut log = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
            writeln!(log, "{}", std::process::id()).unwrap();
        });
        col()
            .child(text(move |r: &mut ReadCx<'_>| format!("alpha {} {}", r.get(saved), r.get(transient))).test_id("value"))
            .child(el().role(Role::Button).label("Increment").test_id("increment").w(100).h(40).on_press(move |w| {
                w.update(saved, |n| *n += 1);
                w.update(transient, |n| *n += 1);
            }))
    }))
}
"#;

struct DevProcess {
    child: Child,
    log: PathBuf,
}

impl DevProcess {
    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap()
    }

    fn until<T>(&mut self, timeout: Duration, mut ready: impl FnMut(&str) -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            let log = self.log();
            if let Some(value) = ready(&log) {
                return value;
            }
            assert_eq!(self.child.try_wait().unwrap(), None, "dev command exited\n{log}");
            assert!(start.elapsed() < timeout, "timed out waiting for dev command\n{log}");
            thread::sleep(Duration::from_millis(30));
        }
    }

    fn stop(&mut self, signal: libc::c_int) -> io::Result<ExitStatus> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(status);
        }
        // SAFETY: the child is the dev process created by this test.
        if unsafe { libc::kill(self.child.id() as libc::pid_t, signal) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(12) {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(30));
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "dev command did not finish"))
    }
}

impl Drop for DevProcess {
    fn drop(&mut self) {
        if self.stop(libc::SIGTERM).is_err() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn request(socket: &Path, command: AgentCommand) -> AgentReceipt {
    let line = serde_json::to_string(&AgentRequest::new(None, command)).unwrap();
    let receipt = Client::connect_with_timeout(socket, Duration::from_secs(4)).unwrap().request(&line).unwrap();
    assert!(receipt.ok, "{receipt:?}");
    receipt
}

fn observe(socket: &Path, expected: &str) -> AgentReceipt {
    let receipt = request(socket, AgentCommand::Observe);
    let nodes = &receipt.snapshot.as_ref().unwrap().nodes;
    let text = nodes.iter().find(|node| node.test_id.as_deref() == Some("value")).and_then(|node| node.text.as_deref());
    assert_eq!(text, Some(expected), "{nodes:?}");
    receipt
}

fn assert_gone(pid: u32) {
    // SAFETY: signal 0 checks for the recorded process without delivering a signal.
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, 0) }, -1, "app {pid} is still running");
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
}

#[test]
fn dev_rebuilds_with_persisted_state_and_reaps_each_native_app() {
    if cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("native dev test requires an X11 or Wayland display");
        return;
    }
    let fixture = tempfile::tempdir_in("/tmp").unwrap();
    let root = octara_cli::api::workspace_root();
    let workspace_target = octara_cli::harness::find(Some(&root.join("examples/Cargo.toml")), Some("octara-examples")).unwrap();
    let manifest = fixture.path().join("Cargo.toml");
    let source = fixture.path().join("src/lib.rs");
    let cleanup = fixture.path().join("cleanup-log");
    fs::create_dir(source.parent().unwrap()).unwrap();
    fs::write(&source, FIXTURE).unwrap();
    fs::copy(root.join("Cargo.lock"), fixture.path().join("Cargo.lock")).unwrap();
    fs::write(
        &manifest,
        format!(
            "[package]\nname = \"restart-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [package.metadata.octara]\nentry = \"restart_fixture::startup\"\n\
             [dependencies]\noctara = {{ path = {:?} }}\n\
             [profile.dev.package.\"*\"]\nopt-level = 2\n",
            root.join("crates/octara"),
        ),
    )
    .unwrap();
    let log = fixture.path().join("dev-log");
    let child = Command::new(env!("CARGO_BIN_EXE_octara"))
        .args(["dev", "--manifest-path"])
        .arg(&manifest)
        .env("CARGO_TARGET_DIR", workspace_target.target_dir)
        .env("OCTARA_DEV_FIXTURE_CLEANUP", &cleanup)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let mut dev = DevProcess { child, log };
    let socket = dev.until(Duration::from_secs(600), |log| {
        log.lines().find_map(|line| line.strip_prefix("Inspector socket ").map(PathBuf::from)).filter(|path| path.exists())
    });
    let initial = observe(&socket, "alpha 4 0");
    let build = initial.build.unwrap();
    let increment = initial.snapshot.unwrap().nodes.into_iter().find(|node| node.test_id.as_deref() == Some("increment")).unwrap();
    let action =
        request(&socket, AgentCommand::Act { target: increment.target, action: AgentAction::Accessibility { action: A11yAction::Press } });
    assert_eq!(action.handled, Some(true));
    observe(&socket, "alpha 5 1");

    fs::write(&source, format!("{FIXTURE}\ncompile_error!(\"broken fixture edit\");\n")).unwrap();
    dev.until(Duration::from_secs(90), |log| log.contains("broken fixture edit").then_some(()));
    assert_eq!(observe(&socket, "alpha 5 1").build, Some(build.clone()));
    assert!(!cleanup.exists());

    let replacement = source.with_extension("updated");
    fs::write(&replacement, FIXTURE.replace("alpha", "beta")).unwrap();
    fs::rename(replacement, &source).unwrap();
    dev.until(Duration::from_secs(90), |log| log.contains("Restarted restart-fixture").then_some(()));
    let restarted = observe(&socket, "beta 5 0").build.unwrap();
    assert_ne!(build.digest, restarted.digest);
    assert_ne!(build.pid, restarted.pid);
    assert_gone(build.pid);
    assert_eq!(fs::read_to_string(&cleanup).unwrap(), format!("{}\n", build.pid));

    let status = dev.stop(libc::SIGINT).unwrap();
    assert_eq!(status.code(), Some(130), "{}", dev.log());
    assert_gone(restarted.pid);
    assert_eq!(fs::read_to_string(&cleanup).unwrap(), format!("{}\n{}\n", build.pid, restarted.pid));
    assert!(!socket.parent().unwrap().exists(), "dev session directory was left behind");
}
