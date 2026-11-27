//! Watch a Cargo workspace and restart its native application using explicit persisted state.

mod process;
mod sources;

use crate::{
    AppArgs, launch,
    receipt::{Receipt, digest},
};
use octara_cli::{harness, live::Client};
use octara_inspect::{AgentCommand, AgentReceipt, AgentRequest, protocol::BuildIdentity};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, ExitCode, ExitStatus},
    thread,
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(150);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn run(app: &AppArgs, locked: bool) -> Result<ExitCode, String> {
    process::install_signal_handlers().map_err(|error| format!("handling interrupt signals: {error}"))?;
    let result = run_session(app, locked);
    if let Some(signal) = process::received_signal() {
        return Ok(ExitCode::from(128 + signal as u8));
    }
    result
}

fn run_session(app: &AppArgs, locked: bool) -> Result<ExitCode, String> {
    // Unix socket paths are short (104 bytes on macOS), including when Cargo's target is deep.
    let session = tempfile::Builder::new()
        .prefix("octara-dev-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .map_err(|error| format!("creating dev session: {error}"))?;
    let socket = session.path().join("inspect.sock");
    let target = find(app)?;
    let sources = sources::Sources::new(&target);
    let mut observed = sources.scan()?;
    let mut changed_at = None;
    eprintln!("Building {}", target.package);
    let mut running = Running::start(stage(&target, locked)?, &target, &socket, None)?;
    let result = (|| {
        running.ready(&socket)?;
        eprintln!("Inspector socket {}", socket.display());
        while process::received_signal().is_none() {
            if let Some(status) = running.process.try_wait()? {
                return Ok(app_exit_code(status));
            }
            thread::sleep(POLL);
            let latest = sources.scan()?;
            if latest != observed {
                observed = latest;
                changed_at = Some(Instant::now());
                continue;
            }
            if changed_at.is_some_and(|at| at.elapsed() >= POLL) && process::received_signal().is_none() {
                changed_at = None;
                eprintln!("Rebuilding {}", target.package);
                let prepared = (|| {
                    let target = find(app)?;
                    let staged = stage(&target, locked)?;
                    if staged.digest == running.build.build.digest {
                        return Ok(None);
                    }
                    let state = running.snapshot(&socket, session.path())?;
                    running.shutdown(&socket)?;
                    Ok::<_, String>(Some((target, staged, state)))
                })();
                let (target, staged, state) = match prepared {
                    Ok(Some(next)) => next,
                    Ok(None) => continue,
                    Err(error) => {
                        if process::received_signal().is_none() {
                            eprintln!("Restart skipped\n{error}");
                        }
                        continue;
                    }
                };
                if process::received_signal().is_some() {
                    break;
                }
                running = Running::start(staged, &target, &socket, Some(state.path()))?;
                running.ready(&socket)?;
                eprintln!("Restarted {} (pid {})", target.package, running.build.build.pid);
            }
        }
        Ok(ExitCode::SUCCESS)
    })();
    if let Err(error) = running.shutdown(&socket) {
        eprintln!("App shutdown failed\n{error}");
    }
    drop(running);
    session.close().map_err(|error| format!("removing dev session: {error}"))?;
    result
}

fn find(app: &AppArgs) -> Result<harness::Target, String> {
    harness::find_using(app.manifest_path.as_deref(), app.package.as_deref(), process::output)
}

struct Staged {
    _dir: tempfile::TempDir,
    executable: PathBuf,
    digest: String,
}

fn stage(target: &harness::Target, locked: bool) -> Result<Staged, String> {
    let stage_root = target.target_dir.join("octara");
    fs::create_dir_all(&stage_root).map_err(|error| format!("{}: {error}", stage_root.display()))?;
    let dir = tempfile::tempdir_in(stage_root).map_err(|error| format!("creating dev executable staging directory: {error}"))?;
    let executable = harness::build_dev(target, locked, process::lock, process::output, |artifact| {
        let path = dir.path().join(artifact.file_name().ok_or("Cargo artifact has no filename")?);
        fs::copy(&artifact, &path).map_err(|error| format!("staging {}: {error}", artifact.display()))?;
        Ok(path)
    })?;
    let digest = digest(&executable).map_err(|error| format!("{}: {error}", executable.display()))?;
    Ok(Staged { _dir: dir, executable, digest })
}

struct Running {
    process: process::Process,
    build: Receipt,
    _staged: Staged,
}

impl Running {
    fn start(staged: Staged, target: &harness::Target, socket: &Path, state: Option<&Path>) -> Result<Self, String> {
        let mut command = Command::new(&staged.executable);
        command.env("OCTARA_INSPECT", socket).env("OCTARA_INSPECT_BUILD", &staged.digest).env_remove("OCTARA_RESTART_STATE");
        if let Some(state) = state {
            command.env("OCTARA_RESTART_STATE", state);
        }
        let process = process::Process::spawn(&mut command).map_err(|error| format!("{}: {error}", staged.executable.display()))?;
        let identity = BuildIdentity { pid: process.child.id(), digest: staged.digest.clone() };
        let build = Receipt::new(target.package.clone(), target.entry.clone(), identity);
        Ok(Self { process, build, _staged: staged })
    }

    fn ready(&mut self, socket: &Path) -> Result<(), String> {
        launch::wait_ready(&mut self.process.child, socket, &self.build, || process::received_signal().is_some())
    }

    fn request(&self, socket: &Path, command: AgentCommand) -> Result<AgentReceipt, String> {
        let mut client = Client::connect_with_timeout(socket, REQUEST_TIMEOUT)?;
        let request = serde_json::to_string(&AgentRequest::new(None, command)).expect("restart request serializes");
        let response = client.request(&request)?;
        if let Some(refusal) = self.build.mismatch(&response) {
            return Err(refusal.message.unwrap_or_default());
        }
        if !response.ok {
            return Err(response.message.unwrap_or_else(|| "Inspector refused the restart command".into()));
        }
        Ok(response)
    }

    fn snapshot(&self, socket: &Path, session: &Path) -> Result<tempfile::NamedTempFile, String> {
        let response = self.request(socket, AgentCommand::ExportRestartState)?;
        let state = response.restart_state.ok_or("Inspector did not return a restart snapshot")?;
        let mut file = tempfile::NamedTempFile::new_in(session).map_err(|error| format!("creating restart snapshot: {error}"))?;
        serde_json::to_writer(&mut file, &state).map_err(|error| format!("writing restart snapshot: {error}"))?;
        Ok(file)
    }

    fn shutdown(&mut self, socket: &Path) -> Result<(), String> {
        if self.process.try_wait()?.is_some() {
            return Ok(());
        }
        self.request(socket, AgentCommand::Shutdown)?;
        let start = Instant::now();
        while start.elapsed() < SHUTDOWN_TIMEOUT {
            if let Some(status) = self.process.try_wait()? {
                if !status.success() {
                    return Err(format!("app exited during shutdown ({status})"));
                }
                if socket.exists() {
                    return Err(format!("app left its Inspector socket at {}", socket.display()));
                }
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err("app did not exit after the shutdown request".into())
    }
}

fn app_exit_code(status: ExitStatus) -> ExitCode {
    status.code().and_then(|code| u8::try_from(code).ok()).map_or(ExitCode::FAILURE, ExitCode::from)
}
