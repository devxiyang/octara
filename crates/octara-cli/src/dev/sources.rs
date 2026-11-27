use octara_cli::harness::Target;
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(PartialEq, Eq)]
pub(super) struct Fingerprint {
    modified: (i64, i64),
    changed: (i64, i64),
    size: u64,
    inode: u64,
}

pub(super) struct Sources {
    workspace: PathBuf,
    target_dir: PathBuf,
}

impl Sources {
    pub(super) fn new(target: &Target) -> Self {
        Self { workspace: target.workspace_root.clone(), target_dir: target.target_dir.clone() }
    }

    pub(super) fn scan(&self) -> Result<BTreeMap<PathBuf, Fingerprint>, String> {
        let mut files = BTreeMap::new();
        self.scan_dir(&self.workspace, &mut files)?;
        Ok(files)
    }

    fn scan_dir(&self, dir: &Path, files: &mut BTreeMap<PathBuf, Fingerprint>) -> Result<(), String> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("watching {}: {error}", dir.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|error| format!("watching {}: {error}", dir.display()))?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("watching {}: {error}", path.display())),
            };
            if kind.is_dir() {
                if !(path == self.target_dir
                    || matches!(name.as_ref(), "target" | "node_modules")
                    || name.starts_with('.') && name != ".cargo")
                {
                    self.scan_dir(&path, files)?;
                }
                continue;
            }
            if path.extension().is_some_and(|extension| extension == "rs")
                || matches!(name.as_ref(), "Cargo.toml" | "Cargo.lock")
                || dir.ends_with(".cargo") && matches!(name.as_ref(), "config" | "config.toml")
            {
                let metadata = match fs::metadata(&path) {
                    Ok(metadata) if metadata.is_file() => metadata,
                    Ok(_) => continue,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => return Err(format!("watching {}: {error}", path.display())),
                };
                files.insert(
                    path,
                    Fingerprint {
                        modified: (metadata.mtime(), metadata.mtime_nsec()),
                        changed: (metadata.ctime(), metadata.ctime_nsec()),
                        size: metadata.len(),
                        inode: metadata.ino(),
                    },
                );
            }
        }
        Ok(())
    }
}
