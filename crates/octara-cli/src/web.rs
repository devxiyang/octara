//! Publishes complete, content-addressed Web bundles into a static site.

use crate::receipt::{digest, hex};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SCHEMA: u32 = 1;
const MANIFEST: &str = "release.json";

#[derive(clap::Subcommand)]
pub(crate) enum Action {
    /// Publish a bundle, then select it as the site's current release.
    Publish {
        #[arg(long)]
        site: PathBuf,
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long)]
        entry: String,
        #[arg(long)]
        wasm: String,
    },
    /// Select a previously published release, including when rolling back.
    Activate {
        #[arg(long)]
        site: PathBuf,
        release: String,
    },
}

impl Action {
    pub(crate) fn run(self) -> io::Result<String> {
        match self {
            Self::Publish { site, bundle, entry, wasm } => publish(&site, &bundle, entry, wasm),
            Self::Activate { site, release } => {
                activate(&site, &release)?;
                Ok(release)
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    entry: String,
    wasm: String,
    files: Vec<Asset>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct Selection<'a> {
    schema: u32,
    release: &'a str,
}

fn publish(site: &Path, bundle: &Path, entry: String, wasm: String) -> io::Result<String> {
    let bundle = bundle.canonicalize()?;
    let paths = bundle_files(&bundle)?;
    if paths.iter().any(|path| path == MANIFEST) {
        return Err(invalid("release.json is reserved for the generated manifest"));
    }
    if entry == wasm || [&entry, &wasm].into_iter().any(|path| paths.binary_search(path).is_err()) {
        return Err(invalid("the entry and Wasm must be distinct files in the bundle"));
    }

    fs::create_dir_all(site)?;
    let site = site.canonicalize()?;
    if site.starts_with(&bundle) {
        return Err(invalid("the site must be outside the source bundle"));
    }
    let releases = site.join("releases");
    fs::create_dir_all(&releases)?;
    require_directory(&releases)?;
    let stage = tempfile::tempdir_in(&releases)?;
    let directory = stage.path().join("complete");
    fs::create_dir(&directory)?;

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let destination = directory.join(&path);
        fs::create_dir_all(destination.parent().expect("an asset has a bundle directory"))?;
        fs::copy(bundle.join(&path), &destination)?;
        files.push(Asset { path, sha256: digest(&destination)? });
    }
    let manifest = serde_json::to_vec(&Manifest { schema: SCHEMA, entry, wasm, files })?;
    let release = hex(Sha256::new_with_prefix(&manifest));
    fs::write(directory.join(MANIFEST), manifest)?;

    let destination = releases.join(&release);
    if !destination.try_exists()? {
        match fs::rename(directory, &destination) {
            Ok(()) => {
                select(&site, &release)?;
                return Ok(release);
            }
            Err(error) if !destination.try_exists()? => return Err(error),
            Err(_) => {}
        }
    }
    // Also validates an identical release installed by a previous or concurrent publisher.
    activate(&site, &release)?;
    Ok(release)
}

fn activate(site: &Path, release: &str) -> io::Result<()> {
    if release.len() != 64 || !release.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        return Err(invalid("expected a lowercase SHA-256 release ID"));
    }
    let releases = site.join("releases");
    require_directory(&releases)?;
    let directory = releases.join(release);
    let mut paths = bundle_files(&directory)?;
    let Ok(manifest_index) = paths.binary_search_by(|path| path.as_str().cmp(MANIFEST)) else {
        return Err(invalid(format!("release {release} has no manifest")));
    };
    paths.remove(manifest_index);
    let bytes = fs::read(directory.join(MANIFEST))?;
    if hex(Sha256::new_with_prefix(&bytes)) != release {
        return Err(invalid(format!("release {release} has a mismatched manifest")));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.schema != SCHEMA
        || manifest.entry == manifest.wasm
        || [&manifest.entry, &manifest.wasm].into_iter().any(|path| paths.binary_search(path).is_err())
        || paths.iter().ne(manifest.files.iter().map(|asset| &asset.path))
    {
        return Err(invalid(format!("release {release} has an invalid file list")));
    }
    for asset in manifest.files {
        if digest(&directory.join(&asset.path))? != asset.sha256 {
            return Err(invalid(format!("release {release} has a mismatched asset {}", asset.path)));
        }
    }

    select(site, release)
}

fn select(site: &Path, release: &str) -> io::Result<()> {
    let stage = tempfile::tempdir_in(site)?;
    let pointer = stage.path().join("current.json");
    fs::write(&pointer, serde_json::to_vec(&Selection { schema: SCHEMA, release })?)?;
    fs::rename(pointer, site.join("current.json"))
}

fn bundle_files(root: &Path) -> io::Result<Vec<String>> {
    fn collect(directory: &Path, prefix: &str, paths: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let filename = entry.file_name();
            let name = filename
                .to_str()
                .filter(|name| name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
                .ok_or_else(|| invalid(format!("unsupported bundle path {}", entry.path().display())))?;
            let path = format!("{prefix}{name}");
            let kind = entry.file_type()?;
            if kind.is_dir() {
                collect(&entry.path(), &format!("{path}/"), paths)?;
            } else if kind.is_file() {
                paths.push(path);
            } else {
                return Err(invalid(format!("bundle entry {} must be a regular file or directory", entry.path().display())));
            }
        }
        Ok(())
    }

    require_directory(root)?;
    let mut paths = Vec::new();
    collect(root, "", &mut paths)?;
    paths.sort_unstable();
    Ok(paths)
}

fn require_directory(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() { Ok(()) } else { Err(invalid(format!("{} must be a directory", path.display()))) }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_and_rollback_require_complete_immutable_releases() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("bundle");
        let site = root.path().join("site");
        fs::create_dir_all(bundle.join("assets")).unwrap();
        fs::write(bundle.join("start.js"), "entry").unwrap();
        fs::write(bundle.join("start.wasm"), "wasm").unwrap();
        let asset = bundle.join("assets/label.txt");
        fs::write(&asset, "A").unwrap();
        let publish = || publish(&site, &bundle, "start.js".into(), "start.wasm".into()).unwrap();
        let a = publish();
        assert_eq!(publish(), a);
        fs::write(asset, "B").unwrap();
        let b = publish();
        assert_ne!(a, b);

        activate(&site, &a).unwrap();
        let selected = fs::read(site.join("current.json")).unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&selected).unwrap()["release"], a);

        fs::write(site.join("releases").join(&b).join("assets/label.txt"), "wrong").unwrap();
        assert!(activate(&site, &b).is_err());
        assert_eq!(fs::read(site.join("current.json")).unwrap(), selected);
    }
}
