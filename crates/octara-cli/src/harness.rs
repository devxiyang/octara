//! Build a headless executable from a project's `[package.metadata.octara]` entry.
//! The entry is a library function of type `fn() -> octara::AppDefinition`.
//! The generated crate uses the application's Cargo lockfile and target directory.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// An app found through `cargo metadata`.
#[derive(Debug)]
pub struct Target {
    pub package: String,
    /// The library crate name (`my_app`).
    pub lib: String,
    pub manifest_path: PathBuf,
    pub manifest_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub target_dir: PathBuf,
    /// The startup function's path (`my_app::startup`).
    pub entry: String,
    /// The `[package.metadata.octara]` table minus `entry`: options for `check`.
    pub options: serde_json::Value,
    /// The app's `octara` dependency, as an inline TOML table.
    pub octara: String,
    /// `octara-inspect` from the same source as the app's facade.
    pub inspect: String,
    native: String,
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    // A nested build has its own package metadata and shares the caller's target configuration.
    for v in ["CARGO_MANIFEST_DIR", "CARGO_PKG_NAME"] {
        c.env_remove(v);
    }
    c
}

/// Finds the app by `package`, then by the package containing `manifest_path` or the current
/// directory if it names an entry, then by the workspace's only package naming an entry.
pub fn find(manifest_path: Option<&Path>, package: Option<&str>) -> Result<Target, String> {
    find_using(manifest_path, package, Command::output)
}

#[doc(hidden)]
pub fn find_using(
    manifest_path: Option<&Path>,
    package: Option<&str>,
    mut output: impl FnMut(&mut Command) -> io::Result<Output>,
) -> Result<Target, String> {
    let mut cmd = cargo();
    cmd.args(["metadata", "--format-version", "1", "--no-deps"]);
    let manifest_path = manifest_path
        .map(|path| {
            path.canonicalize().map(|canonical| crate::api::plain(&canonical)).map_err(|error| format!("{}: {error}", path.display()))
        })
        .transpose()?;
    if let Some(manifest) = &manifest_path {
        let directory = manifest.parent().ok_or("the app manifest must be inside a directory")?;
        cmd.current_dir(directory).arg("--manifest-path").arg(manifest);
    }
    let out = output(&mut cmd).map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let packages = meta["packages"].as_array().cloned().unwrap_or_default();
    let with_entry: Vec<&serde_json::Value> = packages.iter().filter(|p| p["metadata"]["octara"]["entry"].is_string()).collect();
    // As cargo does: the package `manifest_path` names, else the one the current directory is in.
    let here = manifest_path.or_else(|| std::env::current_dir().ok().map(|d| crate::api::plain(&d.canonicalize().unwrap_or(d))));
    let own = here.and_then(|here| {
        with_entry
            .iter()
            .filter(|p| {
                let manifest = crate::api::plain(Path::new(p["manifest_path"].as_str().unwrap_or_default()));
                manifest == here || manifest.parent().is_some_and(|dir| here.starts_with(dir))
            })
            .max_by_key(|p| p["manifest_path"].as_str().map_or(0, str::len))
            .copied()
    });
    let pkg = match package {
        Some(name) => packages.iter().find(|p| p["name"] == name).ok_or_else(|| format!("no package `{name}` in this workspace"))?,
        None if own.is_some() => own.expect("checked"),
        None => match with_entry.as_slice() {
            [one] => one,
            [] => {
                return Err("no package names an Octara startup entry. Add this to the app's Cargo.toml:\n\n[package.metadata.octara]\nentry = \"my_app::startup\"".into());
            }
            many => {
                let names: Vec<&str> = many.iter().filter_map(|p| p["name"].as_str()).collect();
                return Err(format!("several packages name an Octara startup entry ({}). Pick one with --package", names.join(", ")));
            }
        },
    };
    let name = pkg["name"].as_str().unwrap_or_default().to_string();
    let entry = pkg["metadata"]["octara"]["entry"]
        .as_str()
        .ok_or_else(|| format!("`{name}` needs [package.metadata.octara] entry = \"my_app::startup\""))?
        .to_string();
    if !entry.split("::").all(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')) {
        return Err(format!("entry = \"{entry}\" must be a Rust path like `my_app::startup`"));
    }
    let lib = pkg["targets"]
        .as_array()
        .and_then(|ts| ts.iter().find(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "lib"))))
        .and_then(|t| t["name"].as_str())
        .ok_or_else(|| format!("`{name}` needs a library target. Export the startup function from `src/lib.rs` so the harness can use it"))?
        .replace('-', "_");
    let manifest_path = crate::api::plain(Path::new(pkg["manifest_path"].as_str().unwrap_or_default()));
    let manifest_dir = manifest_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let dep = pkg["dependencies"]
        .as_array()
        .and_then(|ds| ds.iter().find(|d| d["name"] == "octara" && d["kind"].is_null()))
        .ok_or_else(|| format!("`{name}` does not depend on `octara`"))?;
    let mut options = pkg["metadata"]["octara"].clone();
    if let Some(options) = options.as_object_mut() {
        options.remove("entry");
    }
    Ok(Target {
        package: name,
        lib,
        manifest_path,
        manifest_dir,
        workspace_root: crate::api::plain(Path::new(meta["workspace_root"].as_str().unwrap_or_default())),
        target_dir: PathBuf::from(meta["target_directory"].as_str().unwrap_or_default()),
        entry,
        options,
        octara: dependency(dep, None),
        inspect: dependency(dep, Some("octara-inspect")),
        native: dependency(dep, Some("octara-host-native")),
    })
}

fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A dependency from `cargo metadata` as an inline TOML table. A sibling shares the facade's
/// source and version requirement and uses its own default features.
fn dependency(d: &serde_json::Value, sibling: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(path) = d["path"].as_str() {
        let path = if let Some(sibling) = sibling {
            let trimmed = path.trim_end_matches(['/', '\\']);
            let parent = trimmed.rfind(['/', '\\']).map_or("", |i| &trimmed[..=i]);
            format!("{parent}{sibling}")
        } else {
            path.to_owned()
        };
        parts.push(format!("path = {}", toml_str(&path)));
    } else if let Some(git) = d["source"].as_str().and_then(|s| s.strip_prefix("git+")) {
        let (url, query) = git.split_once('?').unwrap_or((git, ""));
        let url = url.split('#').next().unwrap_or(url);
        parts.push(format!("git = {}", toml_str(url)));
        if let Some((selector, value)) = query.split_once('=')
            && matches!(selector, "rev" | "branch" | "tag")
        {
            parts.push(format!("{selector} = {}", toml_str(value)));
        }
    } else {
        parts.push(format!("version = {}", toml_str(d["req"].as_str().unwrap_or("*"))));
        if let Some(r) = d["registry"].as_str() {
            parts.push(format!("registry = {}", toml_str(r)));
        }
    }
    if sibling.is_none() {
        if d["uses_default_features"] == false {
            parts.push("default-features = false".into());
        }
        let features: Vec<String> = d["features"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).map(toml_str).collect();
        if !features.is_empty() {
            parts.push(format!("features = [{}]", features.join(", ")));
        }
    }
    format!("{{ {} }}", parts.join(", "))
}

/// Copy the workspace's `[profile.*]` tables verbatim into the generated manifest.
/// Cargo evaluates package overrides in the harness workspace, where the app is a dependency.
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

#[derive(Clone, Copy)]
enum Host {
    Headless,
    Native,
    Dev,
}

impl Host {
    fn source(self) -> &'static str {
        match self {
            Self::Headless => include_str!("harness/driver.rs"),
            Self::Native | Self::Dev => include_str!("harness/native.rs"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::Native => "native",
            Self::Dev => "dev",
        }
    }
}

fn generate(t: &Target, host: Host, dir: &Path, name: &str) -> Result<(), String> {
    let root_manifest = std::fs::read_to_string(t.workspace_root.join("Cargo.toml")).unwrap_or_default();
    let native = match host {
        Host::Native | Host::Dev => format!("octara-host-native = {}\n\n[features]\ndev-restart = []\n", t.native),
        Host::Headless => String::new(),
    };
    let manifest = format!(
        "# Generated by `octara`.\n[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n\
         [[bin]]\nname = \"{name}\"\npath = \"main.rs\"\n\n[dependencies]\n{} = {{ package = {}, path = {} }}\noctara = {}\noctara-inspect = {}\nserde = {{ version = \"1\", features = [\"derive\"] }}\nserde_json = \"1\"\n{native}\n[workspace]\n\n{}",
        t.lib,
        toml_str(&t.package),
        toml_str(&t.manifest_dir.to_string_lossy()),
        t.octara,
        t.inspect,
        profiles(&root_manifest)
    );
    let main =
        format!("// Generated by `octara`.\nmod driver;\n\nfn main() -> std::process::ExitCode {{\n    driver::run({})\n}}\n", t.entry);
    let write = |name: &str, text: &str| -> Result<(), String> {
        let p = dir.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(text) {
            std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        Ok(())
    };
    write("Cargo.toml", &manifest)?;
    write("main.rs", &main)?;
    write("driver.rs", host.source())?;
    // Cargo adds the generated package to its lockfile. Keep that resolved lockfile
    // until the workspace's source lockfile changes.
    let workspace_lock = t.workspace_root.join("Cargo.lock");
    if workspace_lock.exists() {
        let bytes = std::fs::read(&workspace_lock).map_err(|error| format!("{}: {error}", workspace_lock.display()))?;
        let fingerprint = Sha256::digest(&bytes);
        let marker = dir.join("workspace-lock.sha256");
        let generated_lock = dir.join("Cargo.lock");
        if std::fs::read(&marker).ok().as_deref() != Some(fingerprint.as_slice()) || !generated_lock.is_file() {
            std::fs::write(&generated_lock, &bytes).map_err(|error| format!("{}: {error}", generated_lock.display()))?;
            std::fs::write(&marker, fingerprint).map_err(|error| format!("{}: {error}", marker.display()))?;
        }
    }
    Ok(())
}

/// Builds the target's headless harness using the application's Cargo target directory.
pub fn build(t: &Target) -> Result<PathBuf, String> {
    build_harness(t, Host::Headless, File::lock, Command::output, Ok)
}

/// Builds the target's native inspection host.
pub fn build_native(t: &Target, locked: bool) -> Result<PathBuf, String> {
    check_lock(t, locked, Command::output)?;
    build_harness(t, Host::Native, File::lock, Command::output, Ok)
}

#[doc(hidden)]
pub fn build_dev(
    t: &Target,
    locked: bool,
    lock: impl FnOnce(&File) -> io::Result<()>,
    mut output: impl FnMut(&mut Command) -> io::Result<Output>,
    stage: impl FnOnce(PathBuf) -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    check_lock(t, locked, &mut output)?;
    build_harness(t, Host::Dev, lock, output, stage)
}

fn check_lock(t: &Target, locked: bool, mut output: impl FnMut(&mut Command) -> io::Result<Output>) -> Result<(), String> {
    if locked {
        let output = output(
            cargo()
                .current_dir(&t.manifest_dir)
                .args(["metadata", "--format-version", "1", "--locked", "--manifest-path"])
                .arg(&t.manifest_path),
        )
        .map_err(|error| format!("could not check Cargo.lock: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned());
        }
    }
    Ok(())
}

fn build_harness(
    t: &Target,
    host: Host,
    acquire: impl FnOnce(&File) -> io::Result<()>,
    mut output: impl FnMut(&mut Command) -> io::Result<Output>,
    stage: impl FnOnce(PathBuf) -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    let hash = Sha256::digest(t.manifest_dir.as_os_str().as_encoded_bytes());
    let suffix = u64::from_le_bytes(hash[..8].try_into().expect("SHA-256 has 32 bytes"));
    let name = format!("octara-{}-{}-{suffix:016x}", host.name(), t.package);
    let dir = t.target_dir.join("octara/harness").join(&name);
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let lock = File::create(dir.join("build.lock")).map_err(|error| format!("harness build lock: {error}"))?;
    acquire(&lock).map_err(|error| format!("harness build lock: {error}"))?;
    generate(t, host, &dir, &name)?;
    let mut command = cargo();
    command
        .current_dir(&t.manifest_dir)
        .args(["build", "--quiet", "--message-format=json-render-diagnostics", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&t.target_dir);
    if matches!(host, Host::Dev) {
        command.args(["--features", "dev-restart"]);
    }
    let output = output(&mut command).map_err(|error| format!("could not run cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!("building the harness for `{}` failed:\n{}", t.package, String::from_utf8_lossy(&output.stderr)));
    }
    let artifact = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|message| {
            (message["reason"] == "compiler-artifact" && message["target"]["name"] == name)
                .then(|| message["executable"].as_str().map(PathBuf::from))
                .flatten()
        })
        .ok_or_else(|| format!("cargo did not report the {name} executable"))?;
    stage(artifact)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_dependency_is_copied_exactly() {
        let path = serde_json::json!({"name":"octara","path":"/w/crates/octara","req":"*","uses_default_features":true,"features":["system-fonts"]});
        assert_eq!(dependency(&path, None), r#"{ path = "/w/crates/octara", features = ["system-fonts"] }"#);
        let git = serde_json::json!({"name":"octara","source":"git+https://github.com/octoii-works/octara?branch=main","req":"*","uses_default_features":false,"features":[]});
        assert_eq!(
            dependency(&git, None),
            r#"{ git = "https://github.com/octoii-works/octara", branch = "main", default-features = false }"#
        );
        let registry = serde_json::json!({"name":"octara","source":"registry+https://github.com/rust-lang/crates.io-index","req":"^0.3","uses_default_features":true,"features":[]});
        assert_eq!(dependency(&registry, None), r#"{ version = "^0.3" }"#);
        assert_eq!(dependency(&path, Some("octara-inspect")), r#"{ path = "/w/crates/octara-inspect" }"#);
        assert_eq!(dependency(&git, Some("octara-inspect")), r#"{ git = "https://github.com/octoii-works/octara", branch = "main" }"#);
        assert_eq!(dependency(&registry, Some("octara-inspect")), r#"{ version = "^0.3" }"#);
        let windows =
            serde_json::json!({"name":"octara","path":"D:\\w\\crates\\octara","req":"*","uses_default_features":true,"features":[]});
        assert_eq!(dependency(&windows, Some("octara-inspect")), r#"{ path = "D:\\w\\crates\\octara-inspect" }"#);
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
        let dev = "[profile.dev.package.\"*\"]\nopt-level = 1\n\n";
        let app_override = "[profile.dev.package.\"my-app:0.1.0\"]\nopt-level = 3\n\n";
        let test = "[profile.test.package.\"*\"]\nopt-level = 1\n";
        let manifest = format!("[workspace]\nmembers = []\n\n{dev}{app_override}[workspace.dependencies]\nserde = \"1\"\n\n{test}");
        assert_eq!(profiles(&manifest), format!("{dev}{app_override}{test}"));
    }
}
