//! The harness `octara check` / `inspect` / `run` build around an app: a small binary that
//! calls `octara::inspect::main_with` with the app's root view and the reference renderer.
//!
//! The app names its view in `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.octara]
//! view = "my_app::app"          # a `fn() -> View`
//! # optional, for `octara check`:
//! themes = ["light", "dark"]
//! sizes = [{ name = "phone", width = 390, height = 844, touch = true }]
//! accept = [{ code = "SMALL_TARGET", id = "dot-*", why = "page dots; swiping works too" }]
//! ```
//!
//! The harness lives in `<target>/octara/harness/`. It depends on the app by path and on
//! `octara` exactly as the app does (same source, version and features), starts from the app's
//! `Cargo.lock` and profiles, and builds into the app's target directory, so it links the same
//! `octara` and reuses what is already compiled.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// An app found through `cargo metadata`.
#[derive(Debug)]
pub struct Target {
    pub package: String,
    /// The library crate name (`my_app`).
    pub lib: String,
    pub manifest_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub target_dir: PathBuf,
    /// The root view's path (`my_app::app`).
    pub view: String,
    /// The `[package.metadata.octara]` table minus `view`: options for `check`.
    pub options: serde_json::Value,
    /// The app's `octara` dependency, as an inline TOML table.
    pub octara: String,
    /// `octara-render` from the same source (its reference renderer paints `capture`s).
    pub render: String,
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    // Run under `cargo run`/`cargo test`, the outer build's settings must not leak in.
    for v in ["CARGO_TARGET_DIR", "CARGO_MANIFEST_DIR", "CARGO_PKG_NAME", "RUSTFLAGS"] {
        c.env_remove(v);
    }
    c
}

/// Finds the app: the package in `manifest_path`'s workspace (or the current directory's) with
/// `[package.metadata.octara] view`, or the one named `package`.
pub fn find(manifest_path: Option<&Path>, package: Option<&str>) -> Result<Target, String> {
    let mut cmd = cargo();
    cmd.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(m) = manifest_path {
        cmd.arg("--manifest-path").arg(m);
    }
    let out = cmd.output().map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let packages = meta["packages"].as_array().cloned().unwrap_or_default();
    let with_view: Vec<&serde_json::Value> = packages.iter().filter(|p| p["metadata"]["octara"]["view"].is_string()).collect();
    let pkg = match package {
        Some(name) => packages.iter().find(|p| p["name"] == name).ok_or_else(|| format!("no package `{name}` in this workspace"))?,
        None => match with_view.as_slice() {
            [one] => one,
            [] => {
                return Err("no package names its root view: add to the app's Cargo.toml\n\n[package.metadata.octara]\nview = \"my_app::app\"   # a `fn() -> View`".into());
            }
            many => {
                let names: Vec<&str> = many.iter().filter_map(|p| p["name"].as_str()).collect();
                return Err(format!("several packages name a view ({}); pick one with --package", names.join(", ")));
            }
        },
    };
    let name = pkg["name"].as_str().unwrap_or_default().to_string();
    let view = pkg["metadata"]["octara"]["view"]
        .as_str()
        .ok_or_else(|| format!("`{name}` has no [package.metadata.octara] view = \"crate::path::to::view\""))?
        .to_string();
    if view.is_empty() || !view.split("::").all(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')) {
        return Err(format!("view = \"{view}\" is not a path like `my_app::app`"));
    }
    let lib = pkg["targets"]
        .as_array()
        .and_then(|ts| ts.iter().find(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "lib"))))
        .and_then(|t| t["name"].as_str())
        .ok_or_else(|| format!("`{name}` has no library target: put the view in `src/lib.rs` so the harness can use it"))?
        .replace('-', "_");
    let manifest_dir =
        crate::api::plain(Path::new(pkg["manifest_path"].as_str().unwrap_or_default())).parent().map(Path::to_path_buf).unwrap_or_default();
    let dep = pkg["dependencies"]
        .as_array()
        .and_then(|ds| ds.iter().find(|d| d["name"] == "octara" && d["kind"].is_null()))
        .ok_or_else(|| format!("`{name}` does not depend on `octara`"))?;
    let mut options = pkg["metadata"]["octara"].clone();
    options.as_object_mut().map(|o| o.remove("view"));
    Ok(Target {
        package: name,
        lib,
        manifest_dir,
        workspace_root: crate::api::plain(Path::new(meta["workspace_root"].as_str().unwrap_or_default())),
        target_dir: PathBuf::from(meta["target_directory"].as_str().unwrap_or_default()),
        view,
        options,
        octara: dependency(dep),
        render: render_dependency(dep),
    })
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A dependency from `cargo metadata` as an inline TOML table: path, git or registry.
fn dependency(d: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if let Some(path) = d["path"].as_str() {
        parts.push(format!("path = {}", toml_str(path)));
    } else if let Some(git) = d["source"].as_str().and_then(|s| s.strip_prefix("git+")) {
        let (url, query) = git.split_once('?').unwrap_or((git, ""));
        let url = url.split('#').next().unwrap_or(url);
        parts.push(format!("git = {}", toml_str(url)));
        for kv in query.split('&').filter(|kv| !kv.is_empty()) {
            if let Some((k, v)) = kv.split_once('=')
                && ["rev", "branch", "tag"].contains(&k)
            {
                parts.push(format!("{k} = {}", toml_str(v)));
            }
        }
    } else {
        parts.push(format!("version = {}", toml_str(d["req"].as_str().unwrap_or("*"))));
        if let Some(r) = d["registry"].as_str() {
            parts.push(format!("registry = {}", toml_str(r)));
        }
    }
    if d["uses_default_features"] == false {
        parts.push("default-features = false".into());
    }
    let features: Vec<String> = d["features"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).map(toml_str).collect();
    if !features.is_empty() {
        parts.push(format!("features = [{}]", features.join(", ")));
    }
    format!("{{ {} }}", parts.join(", "))
}

/// `octara-render` from where the app's `octara` comes from (the sibling crate of a path, the
/// same git revision, the same version), without the GPU renderer.
fn render_dependency(d: &serde_json::Value) -> String {
    let mut parts = Vec::new();
    if let Some(path) = d["path"].as_str() {
        // The sibling directory, keeping the path's own separators (cargo writes `\` on Windows).
        let trimmed = path.trim_end_matches(['/', '\\']);
        let sibling = match trimmed.rfind(['/', '\\']) {
            Some(i) => format!("{}octara-render", &trimmed[..=i]),
            None => "octara-render".to_owned(),
        };
        parts.push(format!("path = {}", toml_str(&sibling)));
    } else {
        // Git and registry sources are written as for `octara` itself.
        let octara = dependency(&serde_json::json!({ "source": d["source"], "req": d["req"], "registry": d["registry"] }));
        parts.push(octara.trim_start_matches("{ ").trim_end_matches(" }").to_owned());
    }
    parts.push("default-features = false".into());
    format!("{{ {} }}", parts.join(", "))
}

/// The `[profile.*]` sections of a manifest, verbatim: the harness compiles dependencies the way
/// the app's workspace does, so their builds are reused.
fn profiles(manifest: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in manifest.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            inside = t.starts_with("[profile");
        }
        if inside {
            out += line;
            out += "\n";
        }
    }
    out
}

/// Writes the harness crate; returns its directory.
pub fn generate(t: &Target) -> Result<PathBuf, String> {
    let dir = t.target_dir.join("octara/harness");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let root_manifest = std::fs::read_to_string(t.workspace_root.join("Cargo.toml")).unwrap_or_default();
    let manifest = format!(
        "# Generated by `octara`; rewritten on every run.\n[package]\nname = \"octara-harness\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n\
         [[bin]]\nname = \"octara-harness\"\npath = \"main.rs\"\n\n[dependencies]\n{} = {{ package = {}, path = {} }}\noctara = {}\noctara-render = {}\n\n[workspace]\n\n{}",
        t.lib,
        toml_str(&t.package),
        toml_str(&t.manifest_dir.to_string_lossy()),
        t.octara,
        t.render,
        profiles(&root_manifest)
    );
    let main = format!(
        "// Generated by `octara`.\nfn main() {{\n    octara::inspect::main_with({}, octara_render::reference_painter);\n}}\n",
        t.view
    );
    let write = |name: &str, text: &str| -> Result<(), String> {
        let p = dir.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(text) {
            std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        Ok(())
    };
    write("Cargo.toml", &manifest)?;
    write("main.rs", &main)?;
    // Start from the app's resolved versions.
    let lock = t.workspace_root.join("Cargo.lock");
    if lock.exists() {
        std::fs::copy(&lock, dir.join("Cargo.lock")).map_err(|e| format!("{}: {e}", lock.display()))?;
    }
    Ok(dir)
}

/// Builds the harness; returns the binary. Compiler errors come back as the error.
pub fn build(t: &Target) -> Result<PathBuf, String> {
    let dir = generate(t)?;
    let out = cargo()
        .args(["build", "--quiet", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&t.target_dir)
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("building the harness for `{}` failed:\n{}", t.package, String::from_utf8_lossy(&out.stderr)));
    }
    let exe = t.target_dir.join("debug").join(format!("octara-harness{}", std::env::consts::EXE_SUFFIX));
    Ok(exe)
}

/// Source locations relative to the app's workspace (the harness sees them as absolute paths).
pub fn relative_sources(t: &Target, text: &str) -> String {
    // Sources are written with `/` on every platform; so is the root they are relative to.
    let root = format!("{}/", t.workspace_root.display().to_string().replace('\\', "/"));
    text.replace(&root, "")
}

/// Runs the harness with `args`, feeding `stdin`; returns (success, stdout, stderr).
pub fn run(exe: &Path, args: &[String], stdin: Option<&str>) -> Result<(bool, String, String), String> {
    use std::io::Write;
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    if let Some(input) = stdin {
        child.stdin.take().expect("piped").write_all(input.as_bytes()).map_err(|e| format!("harness stdin: {e}"))?;
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().map_err(|e| format!("harness: {e}"))?;
    Ok((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_dependency_is_copied_exactly() {
        let path = serde_json::json!({"name":"octara","path":"/w/crates/octara","req":"*","uses_default_features":true,"features":["system-fonts"]});
        assert_eq!(dependency(&path), r#"{ path = "/w/crates/octara", features = ["system-fonts"] }"#);
        let git = serde_json::json!({"name":"octara","source":"git+https://github.com/octoii-works/octara?branch=main","req":"*","uses_default_features":false,"features":[]});
        assert_eq!(dependency(&git), r#"{ git = "https://github.com/octoii-works/octara", branch = "main", default-features = false }"#);
        let registry = serde_json::json!({"name":"octara","source":"registry+https://github.com/rust-lang/crates.io-index","req":"^0.3","uses_default_features":true,"features":[]});
        assert_eq!(dependency(&registry), r#"{ version = "^0.3" }"#);
        assert_eq!(render_dependency(&path), r#"{ path = "/w/crates/octara-render", default-features = false }"#);
        assert_eq!(
            render_dependency(&git),
            r#"{ git = "https://github.com/octoii-works/octara", branch = "main", default-features = false }"#
        );
        assert_eq!(render_dependency(&registry), r#"{ version = "^0.3", default-features = false }"#);
        let windows =
            serde_json::json!({"name":"octara","path":"D:\\w\\crates\\octara","req":"*","uses_default_features":true,"features":[]});
        assert_eq!(render_dependency(&windows), r#"{ path = "D:\\w\\crates\\octara-render", default-features = false }"#);
    }

    #[test]
    fn windows_verbatim_paths_are_made_plain() {
        use crate::api::plain;
        assert_eq!(plain(Path::new(r"\\?\D:\a\octara")), PathBuf::from(r"D:\a\octara"));
        assert_eq!(plain(Path::new(r"\\?\UNC\server\share\x")), PathBuf::from(r"\\server\share\x"));
        assert_eq!(plain(Path::new("/w/octara")), PathBuf::from("/w/octara"));
    }

    #[test]
    fn profiles_are_taken_verbatim() {
        let manifest = "[workspace]\nmembers = []\n\n[profile.dev.package.\"*\"]\nopt-level = 2\n\n[dependencies]\nx = 1\n";
        assert_eq!(profiles(manifest), "[profile.dev.package.\"*\"]\nopt-level = 2\n\n");
    }
}
