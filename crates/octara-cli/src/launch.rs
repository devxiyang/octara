//! Build and directly launch an ordinary local Cargo executable. The private copy lives until
//! the app exits, so rebuilding Cargo's artifact cannot change the running app's identity.

use crate::receipt::{Receipt, SourceSnapshot, digest};
use serde::Deserialize;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

#[derive(clap::Args)]
#[command(group(clap::ArgGroup::new("target").required(true).multiple(false).args(["bin", "example"])))]
pub(crate) struct LaunchArgs {
    /// The app's Cargo.toml (default: Cargo's current workspace).
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// The Cargo package containing the executable.
    #[arg(long, short)]
    package: Option<String>,
    /// Build and launch this binary target.
    #[arg(long, value_name = "NAME")]
    bin: Option<String>,
    /// Build and launch this example target.
    #[arg(long, value_name = "NAME")]
    example: Option<String>,
    /// An absolute, unused path for the app's inspection socket.
    #[arg(long, value_name = "SOCKET")]
    socket: PathBuf,
    /// Write a new build receipt here; an existing file is never overwritten.
    #[arg(long, value_name = "FILE")]
    receipt: PathBuf,
    /// Require Cargo.lock to stay unchanged.
    #[arg(long)]
    locked: bool,
    /// Arguments passed to the app after `--`.
    #[arg(last = true, value_name = "APP_ARG")]
    app_args: Vec<OsString>,
}

#[derive(Clone, Copy)]
struct Target<'a> {
    kind: &'static str,
    name: &'a str,
}

impl LaunchArgs {
    fn target(&self) -> Result<Target<'_>, String> {
        match (self.bin.as_deref(), self.example.as_deref()) {
            (Some(name), None) => Ok(Target { kind: "bin", name }),
            (None, Some(name)) => Ok(Target { kind: "example", name }),
            _ => Err("choose exactly one --bin or --example target".into()),
        }
    }
}

#[derive(Deserialize)]
struct Metadata {
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    target_directory: PathBuf,
}

pub(crate) fn launch(args: LaunchArgs) -> Result<ExitStatus, String> {
    if !cfg!(unix) {
        return Err("launch currently requires a Unix Inspector socket (macOS or Linux)".into());
    }
    let target = args.target()?;
    if !args.socket.is_absolute() {
        return Err("--socket must be an absolute path".into());
    }
    require_fresh(&args.socket, "inspection socket")?;
    require_fresh(&args.receipt, "receipt")?;
    let cwd = std::env::current_dir().map_err(|error| format!("current directory: {error}"))?;

    let output = cargo_command(&args, &cwd, "metadata")
        .args(["--format-version", "1", "--no-deps"])
        .output()
        .map_err(|error| format!("could not run cargo metadata: {error}"))?;
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Ok(output.status);
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout).map_err(|error| format!("cargo metadata: {error}"))?;
    let before = SourceSnapshot::read(&metadata.workspace_root);
    let (status, artifacts) = build(&args, &cwd, &metadata, target)?;
    if !status.success() {
        return Ok(status);
    }
    let executable = select_artifact(&artifacts, target)?;
    // Keep even an interrupted launch's staging files under Cargo's cleanup boundary.
    let stage_root = metadata.target_directory.join("octara");
    std::fs::create_dir_all(&stage_root).map_err(|error| format!("creating {}: {error}", stage_root.display()))?;
    let mut staging = tempfile::Builder::new();
    staging.prefix("octara-launch-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staging.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let staged = staging.tempdir_in(&stage_root).map_err(|error| format!("staging directory: {error}"))?;
    let name = executable.file_name().ok_or_else(|| format!("Cargo executable has no file name: {}", executable.display()))?;
    let private_executable = staged.path().join(name);
    std::fs::copy(&executable, &private_executable).map_err(|error| format!("could not stage {}: {error}", executable.display()))?;
    let build = digest(&private_executable).map_err(|error| format!("could not hash {}: {error}", private_executable.display()))?;
    let after = SourceSnapshot::read(&metadata.workspace_root);
    let sources = before.filter(|snapshot| after.as_ref() == Some(snapshot));
    require_fresh(&args.socket, "inspection socket")?;

    // Preserve Cargo run's caller working directory and the app's original executable name.
    // `staged` remains alive through wait(), then removes only this launch's private copy.
    let mut child = Command::new(&private_executable)
        .args(&args.app_args)
        .current_dir(cwd)
        .env("OCTARA_INSPECT", &args.socket)
        .env("OCTARA_INSPECT_BUILD", &build)
        .spawn()
        .map_err(|error| format!("could not launch {}: {error}", executable.display()))?;
    if let Err(error) = (Receipt { build, sources }).write(&args.receipt) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    child.wait().map_err(|error| format!("waiting for {}: {error}", executable.display()))
}

fn cargo_command(args: &LaunchArgs, cwd: &Path, subcommand: &str) -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    // Both Cargo invocations use the caller's directory and environment, including its normal
    // .cargo/config.toml, target directory, rustflags and selected toolchain.
    command.current_dir(cwd).arg(subcommand);
    if let Some(manifest) = &args.manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    if args.locked {
        command.arg("--locked");
    }
    command
}

fn require_fresh(path: &Path, description: &str) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(format!("{description} {} already exists; choose a fresh path", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{description} {}: {error}", path.display())),
    }
}

fn build(args: &LaunchArgs, cwd: &Path, metadata: &Metadata, target: Target<'_>) -> Result<(ExitStatus, Vec<PathBuf>), String> {
    let mut command = cargo_command(args, cwd, "build");
    command.arg("--message-format=json-render-diagnostics");
    if let Some(package) = &args.package {
        command.arg("--package").arg(package);
    }
    command.arg(format!("--{}", target.kind)).arg(target.name).stdout(Stdio::piped());
    let mut child = command.spawn().map_err(|error| format!("could not run cargo build: {error}"))?;
    let stdout = child.stdout.take().expect("Cargo stdout is piped");
    let mut artifacts = Vec::new();
    let mut read_error = None;
    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                read_error = Some(error);
                break;
            }
        };
        match serde_json::from_str(&line) {
            Ok(message) => {
                if let Some(path) = artifact(&message, metadata, target) {
                    artifacts.push(path);
                }
            }
            Err(_) => eprintln!("{line}"),
        }
    }
    let status = child.wait().map_err(|error| format!("waiting for cargo build: {error}"))?;
    if let Some(error) = read_error {
        return Err(format!("reading cargo build output: {error}"));
    }
    Ok((status, artifacts))
}

fn artifact(message: &serde_json::Value, metadata: &Metadata, target: Target<'_>) -> Option<PathBuf> {
    if message["reason"] != "compiler-artifact"
        || message["target"]["name"] != target.name
        || message["profile"]["test"] != false
        || !message["target"]["kind"].as_array()?.iter().any(|kind| kind == target.kind)
        || !metadata.workspace_members.iter().any(|package| message["package_id"] == *package)
    {
        return None;
    }
    message["executable"].as_str().map(PathBuf::from)
}

fn select_artifact(artifacts: &[PathBuf], target: Target<'_>) -> Result<PathBuf, String> {
    match artifacts {
        [executable] => Ok(executable.clone()),
        [] => Err(format!("cargo did not report an executable for --{} {}", target.kind, target.name)),
        _ => Err(format!("cargo reported several executables for --{} {}; select one package with --package", target.kind, target.name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_requested_workspace_executable_is_selected() {
        let metadata = Metadata {
            workspace_root: PathBuf::from("/workspace"),
            workspace_members: vec!["app".into()],
            target_directory: PathBuf::from("/workspace/target"),
        };
        let target = Target { kind: "bin", name: "app" };
        let mut message = serde_json::json!({
            "reason": "compiler-artifact", "package_id": "app", "target": { "kind": ["bin"], "name": "app" },
            "profile": { "test": false }, "executable": "/workspace/target/debug/app"
        });
        assert_eq!(artifact(&message, &metadata, target), Some(PathBuf::from("/workspace/target/debug/app")));
        message["package_id"] = "dependency".into();
        assert_eq!(artifact(&message, &metadata, target), None);
        message["package_id"] = "app".into();
        message["target"]["kind"] = serde_json::json!(["example"]);
        assert_eq!(artifact(&message, &metadata, target), None);
        message["target"]["kind"] = serde_json::json!(["bin"]);
        message["profile"]["test"] = true.into();
        assert_eq!(artifact(&message, &metadata, target), None);
    }

    #[test]
    fn colliding_package_outputs_remain_ambiguous() {
        let executable = PathBuf::from("/workspace/target/debug/app");
        let target = Target { kind: "bin", name: "app" };
        assert!(select_artifact(&[executable.clone(), executable], target).unwrap_err().contains("--package"));
    }
}
