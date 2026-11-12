//! The framework's public API, read from the source of the crates an app depends on (stable
//! Rust, `syn`), so it always matches the version the app builds with. Nothing is checked in:
//! a read is cached under the app's target directory until a source file changes.

mod expand;
mod model;
pub mod source;

pub use model::{Api, CrateApi, FORMAT, Item, Kind};
pub use source::Source;

use std::path::{Path, PathBuf};
use std::process::Command;

/// The framework's public crates, in the order the API lists them.
pub const CRATES: &[&str] = &["octara", "octara-kit", "octara-render", "octara-shell", "octara-fonts", "octara-macros"];

/// This workspace's directory, where the CLI was built from.
pub fn workspace_root() -> PathBuf {
    plain(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|_| PathBuf::from(".")))
}

/// A path without Windows' verbatim prefix (`\\?\`, which `canonicalize` adds there), so it
/// compares with the paths cargo and rustc write.
pub fn plain(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!(r"\\{rest}")),
        None => s.strip_prefix(r"\\?\").map_or_else(|| path.to_path_buf(), PathBuf::from),
    }
}

/// The framework's crates in this workspace (tests, and the CLI outside any project).
pub fn workspace_sources() -> Vec<Source> {
    let root = workspace_root();
    let dir_of = |c: &str| root.join("crates").join(c);
    CRATES
        .iter()
        .map(|c| Source { name: c.to_string(), dir: dir_of(c), lib: dir_of(c).join("src/lib.rs") })
        .filter(|s| s.lib.exists())
        .collect()
}

/// The framework's crates the project at `dir` depends on (`cargo metadata`), and its target
/// directory.
pub fn project_sources(dir: &Path) -> Result<(Vec<Source>, PathBuf), String> {
    let out = Command::new("cargo")
        .current_dir(dir)
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut sources = Vec::new();
    for name in CRATES {
        let Some(pkg) = meta["packages"].as_array().into_iter().flatten().find(|p| p["name"] == *name) else { continue };
        let manifest = PathBuf::from(pkg["manifest_path"].as_str().unwrap_or_default());
        let lib = pkg["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "lib" || k == "proc-macro")));
        let Some(lib) = lib.and_then(|t| t["src_path"].as_str()) else { continue };
        sources.push(Source {
            name: name.to_string(),
            dir: manifest.parent().unwrap_or(Path::new(".")).to_path_buf(),
            lib: PathBuf::from(lib),
        });
    }
    if sources.is_empty() {
        return Err("this project does not depend on octara".into());
    }
    let target = PathBuf::from(meta["target_directory"].as_str().unwrap_or("target"));
    Ok((sources, target))
}

/// Reads every crate of `sources`.
pub fn read(sources: &[Source]) -> Result<Api, String> {
    let crates = sources.iter().map(source::read).collect::<Result<Vec<_>, _>>()?;
    Ok(Api { format: FORMAT, crates })
}

/// What the sources' files are now (paths, sizes, times): a read stays valid while this does.
fn fingerprint(sources: &[Source]) -> String {
    let mut files = Vec::new();
    for s in sources {
        let mut stack = vec![s.dir.join("src")];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs")
                    && let Ok(m) = e.metadata()
                {
                    let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
                    files.push(format!("{}:{}:{t}", p.display(), m.len()));
                }
            }
        }
    }
    files.sort();
    format!("{} {FORMAT} {}", env!("CARGO_PKG_VERSION"), files.join("\n"))
}

/// Reads `sources`, answering from `<target>/octara/api.json` while no source file changed.
pub fn cached(sources: &[Source], target: &Path) -> Result<Api, String> {
    let file = target.join("octara").join("api.json");
    let key = fingerprint(sources);
    if let Ok(text) = std::fs::read_to_string(&file)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
        && v["key"] == key.as_str()
        && let Ok(api) = serde_json::from_value::<Api>(v["api"].clone())
    {
        return Ok(api);
    }
    let api = read(sources)?;
    if let Some(dir) = file.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        let _ = std::fs::write(&file, serde_json::json!({ "key": key, "api": api }).to_string());
    }
    Ok(api)
}

/// The API for the project at `dir`: the octara it depends on, else this workspace's.
pub fn for_project(dir: &Path) -> Result<Api, String> {
    match project_sources(dir) {
        Ok((sources, target)) => cached(&sources, &target),
        Err(_) => {
            let sources = workspace_sources();
            if sources.is_empty() {
                return Err("run inside a project that depends on octara".into());
            }
            cached(&sources, &workspace_root().join("target"))
        }
    }
}

/// This workspace's API, read once per process (tests).
pub fn load() -> &'static Api {
    static API: std::sync::OnceLock<Api> = std::sync::OnceLock::new();
    API.get_or_init(|| read(&workspace_sources()).unwrap_or_else(|e| panic!("{e}")))
}
