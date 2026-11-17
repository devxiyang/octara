//! Evidence about the executable and Rust source files recorded by a local launch.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize, Serialize)]
pub(crate) struct Receipt {
    pub(crate) build: String,
    pub(crate) sources: Option<SourceSnapshot>,
}

impl Receipt {
    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let receipt: Self = fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("reading build receipt {}: {e}", path.display()))?;
        if receipt.build.len() != 64 || !receipt.build.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(format!("invalid build digest in {}", path.display()));
        }
        Ok(receipt)
    }

    pub(crate) fn write(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| format!("creating receipt in {}: {e}", parent.display()))?;
        serde_json::to_writer(&mut file, self).map_err(|e| format!("writing receipt {}: {e}", path.display()))?;
        file.persist_noclobber(path).map_err(|e| format!("saving receipt {}: {e}", path.display()))?;
        Ok(())
    }

    pub(crate) fn source_status(&self, running: Option<&str>) -> &'static str {
        if running == Some(&self.build)
            && let Some(built) = &self.sources
            && let Some(current) = SourceSnapshot::read(&built.workspace)
        {
            return if *built == current { "matches" } else { "changed" };
        }
        "unknown"
    }
}

#[derive(Deserialize, PartialEq, Serialize)]
pub(crate) struct SourceSnapshot {
    workspace: PathBuf,
    digest: String,
}

impl SourceSnapshot {
    /// Record tracked and nonignored untracked Rust files within the Cargo workspace. A Git
    /// worktree gives us a bounded source inventory without traversing build output directories.
    pub(crate) fn read(workspace: &Path) -> Option<Self> {
        let output = Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "*.rs"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let paths: BTreeSet<_> = str::from_utf8(&output.stdout).ok()?.split_terminator('\0').collect();
        if paths.is_empty() {
            return None;
        }
        let mut hash = Sha256::new();
        for path in paths {
            hash.update((path.len() as u64).to_le_bytes());
            hash.update(path.as_bytes());
            let file = workspace.join(path);
            match file.metadata() {
                Ok(meta) if meta.is_file() => hash.update(digest(&file).ok()?),
                Err(e) if e.kind() == io::ErrorKind::NotFound => hash.update(b"missing"),
                _ => return None,
            }
        }
        Some(Self { workspace: workspace.to_owned(), digest: hex(hash) })
    }
}

pub(crate) fn digest(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 32 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hex(hash));
        }
        hash.update(&buf[..n]);
    }
}

fn hex(hash: Sha256) -> String {
    let mut digest = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(digest, "{byte:02x}").expect("a String can be written");
    }
    digest
}
