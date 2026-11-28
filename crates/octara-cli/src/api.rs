//! The framework's public API, read from the source of the crates an app depends on and
//! cached under its target directory. The installed CLI also carries its release's API for
//! use outside a Cargo project.

mod expand;
mod link;
mod model;
pub mod source;

pub use model::{Api, CrateApi, FORMAT, Item, Kind};
pub use source::Source;

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The framework's public crates, in the order the API lists them.
pub const CRATES: &[&str] = &[
    "octara",
    "octara-model",
    "octara-runtime",
    "octara-tree",
    "octara-headless",
    "octara-kit",
    "octara-markdown",
    "octara-inspect",
    "octara-host-native",
    "octara-host-web",
    "octara-pipeline",
    "octara-render",
    "octara-render-metal",
    "octara-gpu",
    "octara-release",
    "octara-icons",
    "octara-fonts",
    "octara-svg",
    "octara-macros",
    "octara-webview",
    "octara-video",
    "octara-picker",
    "octara-haptics",
];

/// This workspace's directory, where the CLI was built from.
pub fn workspace_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    plain(&root.canonicalize().unwrap_or(root))
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

/// The framework's crates in the workspace that builds the CLI.
pub fn workspace_sources() -> Vec<Source> {
    let root = workspace_root();
    CRATES
        .iter()
        .filter_map(|name| {
            for group in ["crates", "modules"] {
                let dir = root.join(group).join(name);
                let lib = dir.join("src/lib.rs");
                if lib.is_file() {
                    return Some(Source { name: name.to_string(), dir, lib });
                }
            }
            None
        })
        .collect()
}

/// The framework's crates the project at `dir` depends on (`cargo metadata`), and its target
/// directory.
pub fn project_sources(dir: &Path) -> Result<(Vec<Source>, PathBuf), String> {
    let out = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(dir)
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let meta: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let packages = resolved_packages(&meta);
    let mut sources = Vec::new();
    for name in CRATES {
        let Some(pkg) = packages.get(name) else { continue };
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

/// Walk from the active Cargo package so a direct dependency wins over another version used
/// further down the graph. At a virtual workspace root, include every member's crates.
fn resolved_packages(meta: &Value) -> HashMap<&str, &Value> {
    fn by_id(values: &Value) -> HashMap<&str, &Value> {
        values.as_array().into_iter().flatten().filter_map(|value| Some((value["id"].as_str()?, value))).collect::<HashMap<_, _>>()
    }
    fn ids(values: &Value) -> impl Iterator<Item = &str> {
        values.as_array().into_iter().flatten().filter_map(Value::as_str)
    }
    let packages = by_id(&meta["packages"]);
    let nodes = by_id(&meta["resolve"]["nodes"]);
    let mut pending: VecDeque<_> =
        if let Some(root) = meta["resolve"]["root"].as_str() { VecDeque::from([root]) } else { ids(&meta["workspace_members"]).collect() };
    let mut seen = HashSet::new();
    let mut by_name = HashMap::new();
    while let Some(id) = pending.pop_front() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(&package) = packages.get(id)
            && let Some(name) = package["name"].as_str()
            && CRATES.contains(&name)
        {
            by_name.entry(name).or_insert(package);
        }
        if let Some(node) = nodes.get(id) {
            pending.extend(ids(&node["dependencies"]));
        }
    }
    by_name
}

/// Reads every crate of `sources`.
pub fn read(sources: &[Source]) -> Result<Api, String> {
    let crates = sources.iter().map(source::read).collect::<Result<Vec<_>, _>>()?;
    let mut api = Api { format: FORMAT, crates };
    link::reexports(&mut api);
    Ok(api)
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
        && let Ok(staged) = tempfile::NamedTempFile::new_in(dir)
        && serde_json::to_writer(staged.as_file(), &serde_json::json!({ "key": key, "api": api })).is_ok()
    {
        let _ = staged.persist(file);
    }
    Ok(api)
}

/// The API for the project at `dir`: the Octara it depends on, else the CLI's bundled API.
pub fn for_project(dir: &Path) -> Result<Api, String> {
    let dir = std::path::absolute(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    if dir.ancestors().any(|parent| parent.join("Cargo.toml").is_file())
        && let Ok((sources, target)) = project_sources(&dir)
    {
        return cached(&sources, &target);
    }
    bundled()
}

/// The source catalog shipped with this CLI. Its paths are relative to the Octara repository.
pub fn bundled() -> Result<Api, String> {
    let compressed = include_bytes!("../skill/api.json.gz");
    let api: Api = serde_json::from_reader(GzDecoder::new(compressed.as_slice())).map_err(|error| format!("bundled API: {error}"))?;
    if api.format != FORMAT {
        return Err(format!("bundled API format {} differs from {FORMAT}", api.format));
    }
    Ok(api)
}

/// Regenerate the bundled catalog from a full Octara checkout.
pub fn export_bundle(path: &Path) -> Result<(), String> {
    let sources = workspace_sources();
    if sources.len() != CRATES.len() {
        return Err("export-api requires a full Octara checkout".into());
    }
    let mut api = read(&sources)?;
    let root = workspace_root();
    for (krate, source) in api.crates.iter_mut().zip(sources) {
        let path = source.dir.strip_prefix(&root).map_err(|error| format!("{}: {error}", source.dir.display()))?;
        krate.root = path.to_string_lossy().replace('\\', "/");
    }
    let mut output = GzEncoder::new(Vec::new(), Compression::best());
    serde_json::to_writer(&mut output, &api).map_err(|error| format!("bundled API: {error}"))?;
    let bytes = output.finish().map_err(|error| format!("bundled API: {error}"))?;
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// The CLI version's API, read once per process. A source checkout uses its live sources.
pub fn load() -> &'static Api {
    static API: std::sync::OnceLock<Api> = std::sync::OnceLock::new();
    API.get_or_init(|| {
        let sources = workspace_sources();
        let api = if sources.len() == CRATES.len() { read(&sources) } else { bundled() };
        api.unwrap_or_else(|error| panic!("{error}"))
    })
}
