//! Collect module manifest contributions for an app's packaging workflow.

mod manifest;
mod project;

use octara_model::diagnostics::Code;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand)]
pub(crate) enum Action {
    /// Generate Apple plist and entitlement files, a framework list, and Web headers.
    ///
    /// Reads variables from [modules."module.id"] in octara.toml beside the app's Cargo.toml.
    /// Optional base files must contain plist dictionaries. The command writes
    /// XML plists. Identical values for a key are kept once. Different values
    /// for the same key cause a conflict.
    /// Missing minimum OS versions default to iOS 18.4 and macOS 15.4.
    /// Relative base paths are resolved from the current directory.
    Merge {
        #[command(flatten)]
        project: project::Project,
        #[command(flatten)]
        base: BasePaths,
        /// A new directory for the generated files.
        #[arg(long, value_name = "DIR")]
        out_dir: PathBuf,
    },
}

#[derive(clap::Args)]
pub(crate) struct BasePaths {
    /// The app's base iOS Info.plist.
    #[arg(long, value_name = "PATH")]
    ios_info_plist: Option<PathBuf>,
    /// The app's base iOS entitlements plist.
    #[arg(long, value_name = "PATH")]
    ios_entitlements: Option<PathBuf>,
    /// The app's base macOS Info.plist.
    #[arg(long, value_name = "PATH")]
    macos_info_plist: Option<PathBuf>,
    /// The app's base macOS entitlements plist.
    #[arg(long, value_name = "PATH")]
    macos_entitlements: Option<PathBuf>,
}

impl Action {
    pub(crate) fn run(self, out: &mut String) -> Result<(), String> {
        let Self::Merge { project, base, out_dir } = self;
        let (app, packages) = project.resolve()?;
        let merged = manifest::merge(&app, packages, &base)?;
        write_output(&out_dir, merged).map_err(|error| format!("{}: {error}", out_dir.display()))?;
        writeln!(out, "wrote module metadata to {}", out_dir.display()).expect("a String can be written");
        Ok(())
    }
}

fn diagnostic(code: &Code, path: &Path, message: impl std::fmt::Display) -> String {
    format!("[{code}] {}: {message}", path.display())
}

fn write_output(out: &Path, merged: manifest::Merged) -> io::Result<()> {
    fn require_unused(path: &Path) -> io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(_) => Err(io::Error::new(io::ErrorKind::AlreadyExists, "choose a new output directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    require_unused(out)?;
    let parent = out.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let stage = tempfile::tempdir_in(parent)?;
    let directory = stage.path().join("metadata");
    for (name, apple) in [("ios", merged.ios), ("macos", merged.macos)] {
        let platform = directory.join(name);
        fs::create_dir_all(&platform)?;
        for (file, value) in [("Info.plist", apple.info_plist), ("Module.entitlements", apple.entitlements)] {
            write_plist(&platform.join(file), value)?;
        }
    }
    write_json(&directory.join("apple-frameworks.json"), &merged.frameworks)?;
    write_json(&directory.join("web-headers.json"), &merged.headers)?;
    require_unused(out)?;
    // A concurrent merge publishes a nonempty directory in one rename.
    fs::rename(directory, out)
}

fn write_plist(path: &Path, value: plist::Dictionary) -> io::Result<()> {
    let mut bytes = Vec::new();
    plist::Value::Dictionary(value).to_writer_xml(&mut bytes).map_err(io::Error::other)?;
    fs::write(path, bytes)
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes)
}
