//! Run the declared entry in its own native host with an Inspector transport.

use crate::{
    AppArgs,
    receipt::{Receipt, digest},
};
use octara_cli::{harness, live::Client};
use octara_inspect::{AgentCommand, AgentRequest, protocol::BuildIdentity};
use std::{
    io::ErrorKind,
    path::Path,
    process::{Child, Command, ExitCode},
    time::{Duration, Instant},
};

pub(crate) fn launch(app: &AppArgs, socket: &Path, receipt: &Path, locked: bool) -> Result<ExitCode, String> {
    if !cfg!(unix) {
        return Err("launch requires Unix domain sockets".into());
    }
    if !socket.is_absolute() {
        return Err("--socket must be an absolute path".into());
    }
    require_fresh(socket)?;
    require_fresh(receipt)?;
    let target = app.target()?;
    let artifact = harness::build_native(&target, locked)?;
    // A rebuild of the Cargo artifact must not alter the running executable or its receipt.
    let stage_root = target.target_dir.join("octara");
    std::fs::create_dir_all(&stage_root).map_err(|error| format!("{}: {error}", stage_root.display()))?;
    let stage = tempfile::tempdir_in(stage_root).map_err(|error| format!("staging executable: {error}"))?;
    let executable = stage.path().join(artifact.file_name().ok_or("Cargo artifact has no filename")?);
    std::fs::copy(&artifact, &executable).map_err(|error| format!("{}: {error}", artifact.display()))?;
    let digest = digest(&executable).map_err(|error| format!("{}: {error}", executable.display()))?;
    require_fresh(socket)?;
    let mut child = Command::new(&executable)
        .env("OCTARA_INSPECT", socket)
        .env("OCTARA_INSPECT_BUILD", &digest)
        .spawn()
        .map_err(|error| format!("{}: {error}", executable.display()))?;
    let identity = BuildIdentity { digest, pid: child.id() };
    let build = Receipt::new(target.package, target.entry, identity);
    let result = wait_ready(&mut child, socket, &build, || false).and_then(|()| build.write(receipt));
    if let Err(error) = result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let status = child.wait().map_err(|error| format!("waiting for app: {error}"))?;
    Ok(status.code().and_then(|code| u8::try_from(code).ok()).map_or(ExitCode::FAILURE, ExitCode::from))
}

fn require_fresh(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
        Ok(_) => Err(format!("{} already exists", path.display())),
    }
}

pub(crate) fn wait_ready(child: &mut Child, socket: &Path, build: &Receipt, interrupted: impl Fn() -> bool) -> Result<(), String> {
    let start = Instant::now();
    let request = serde_json::to_string(&AgentRequest::new(None, AgentCommand::ListWindows)).expect("a ListWindows request serializes");
    while start.elapsed() < Duration::from_secs(20) {
        if interrupted() {
            return Err("interrupted while waiting for Inspector".into());
        }
        if let Some(status) = child.try_wait().map_err(|error| format!("checking launched app: {error}"))? {
            return Err(format!("app exited before Inspector was ready ({status})"));
        }
        if let Ok(mut client) = Client::connect_with_timeout(socket, Duration::from_secs(1))
            && let Ok(response) = client.request(&request)
        {
            if let Some(refusal) = build.mismatch(&response) {
                return Err(refusal.message.unwrap_or_default());
            }
            if response.windows.is_some_and(|windows| windows.iter().any(|window| window.progress.settled)) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    Err(format!("app did not publish a settled window at {}", socket.display()))
}
