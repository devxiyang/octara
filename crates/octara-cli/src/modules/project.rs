//! Select an application package and walk its normal resolved Cargo dependencies by package ID.

use octara_cli::api::plain;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(clap::Args)]
pub(crate) struct Project {
    /// The app or workspace Cargo.toml (default: the current Cargo project).
    #[arg(long, value_name = "PATH")]
    manifest_path: Option<PathBuf>,
    /// The app package name, when selecting it from a workspace.
    #[arg(short, long, value_name = "NAME")]
    package: Option<String>,
    /// Require Cargo.lock to stay unchanged.
    #[arg(long)]
    locked: bool,
    /// Space- or comma-separated Cargo features to activate.
    #[arg(short = 'F', long)]
    features: Vec<String>,
    /// Activate all Cargo features.
    #[arg(long)]
    all_features: bool,
    /// Disable the app's default Cargo features.
    #[arg(long)]
    no_default_features: bool,
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_default_members: Vec<String>,
    resolve: Option<Resolve>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    pkg: String,
    dep_kinds: Vec<DependencyKind>,
}

#[derive(Deserialize)]
struct DependencyKind {
    kind: Option<String>,
}

impl Project {
    pub(super) fn resolve(&self) -> Result<(PathBuf, Vec<PathBuf>), String> {
        let (here, manifest) = if let Some(manifest) = &self.manifest_path {
            let manifest = manifest.canonicalize().map_err(|error| format!("{}: {error}", manifest.display()))?;
            let manifest = plain(&manifest);
            (parent(&manifest)?.to_path_buf(), Some(manifest))
        } else {
            let cwd = std::env::current_dir().map_err(|error| format!("current directory: {error}"))?;
            (plain(&cwd.canonicalize().unwrap_or(cwd)), None)
        };
        let metadata = self.metadata(&here, manifest.as_deref(), true)?;
        let app = select_app(&metadata, &here, self.package.as_deref())?;
        let app_root = parent(&app.manifest_path)?;
        let resolution = self.metadata(app_root, Some(&app.manifest_path), false)?;
        let packages = dependency_manifests(&resolution, &app.id)?;
        Ok((app_root.to_path_buf(), packages))
    }

    fn metadata(&self, directory: &Path, manifest: Option<&Path>, no_deps: bool) -> Result<Metadata, String> {
        let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command.current_dir(directory).args(["metadata", "--format-version", "1"]);
        if let Some(manifest) = manifest {
            command.arg("--manifest-path").arg(manifest);
        }
        if self.locked {
            command.arg("--locked");
        }
        if no_deps {
            command.arg("--no-deps");
        } else {
            for features in &self.features {
                command.arg("--features").arg(features);
            }
            if self.no_default_features {
                command.arg("--no-default-features");
            }
            if self.all_features {
                command.arg("--all-features");
            }
        }
        let output = command.output().map_err(|error| format!("could not run cargo in {}: {error}", directory.display()))?;
        if !output.status.success() {
            return Err(format!("cargo metadata failed in {}:\n{}", directory.display(), String::from_utf8_lossy(&output.stderr)));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| format!("unreadable cargo metadata: {error}"))
    }
}

fn parent(manifest: &Path) -> Result<&Path, String> {
    manifest.parent().ok_or_else(|| format!("{} has no package directory", manifest.display()))
}

fn select_app<'a>(metadata: &'a Metadata, here: &Path, name: Option<&str>) -> Result<&'a Package, String> {
    let packages = &metadata.packages;
    if let Some(name) = name {
        return packages.iter().find(|package| package.name == name).ok_or_else(|| format!("no package `{name}` in this workspace"));
    }
    if let Some(package) = packages
        .iter()
        .filter(|package| package.manifest_path.parent().is_some_and(|directory| here.starts_with(directory)))
        .max_by_key(|package| package.manifest_path.components().count())
    {
        return Ok(package);
    }
    let defaults = &metadata.workspace_default_members;
    if let [id] = defaults.as_slice()
        && let Some(package) = packages.iter().find(|package| &package.id == id)
    {
        return Ok(package);
    }
    let mut names: Vec<_> = packages.iter().map(|package| package.name.as_str()).collect();
    names.sort_unstable();
    Err(format!("select an app with --package ({})", names.join(", ")))
}

fn dependency_manifests(metadata: &Metadata, root: &str) -> Result<Vec<PathBuf>, String> {
    let resolve = metadata.resolve.as_ref().ok_or("cargo metadata did not return a dependency graph")?;
    let packages: HashMap<_, _> = metadata.packages.iter().map(|package| (package.id.as_str(), package)).collect();
    let nodes: HashMap<_, _> = resolve.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    let mut manifests = Vec::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let node = nodes.get(id).ok_or_else(|| format!("cargo metadata has no resolve node for `{id}`"))?;
        let package = packages.get(id).ok_or_else(|| format!("cargo metadata has no package for `{id}`"))?;
        if id != root {
            manifests.push(parent(&package.manifest_path)?.join("octara-module.toml"));
        }
        pending.extend(node.deps.iter().filter(|dep| dep.dep_kinds.iter().any(|kind| kind.kind.is_none())).map(|dep| dep.pkg.as_str()));
    }
    manifests.sort_unstable();
    Ok(manifests)
}
