//! Opt-in, bounded persistent cache for eligible public HTTP images.

use octara::MediaCacheBackend;
use octara::runtime::media::HttpMediaEntry;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

#[derive(serde::Deserialize, serde::Serialize)]
struct Metadata {
    mime: Option<std::sync::Arc<str>>,
    etag: Option<std::sync::Arc<str>>,
    fresh_until: u64,
    stale_until: u64,
    max_age: Option<u64>,
    #[serde(default)]
    stale_while_revalidate: u64,
}

/// Filesystem cache for the native host. Give each application its own directory.
pub struct DiskMediaCache {
    directory: PathBuf,
    limit: u64,
    io: Mutex<()>,
}

impl DiskMediaCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self::with_limit(directory, 100 * 1024 * 1024)
    }

    pub fn with_limit(directory: impl Into<PathBuf>, byte_limit: u64) -> Self {
        Self { directory: directory.into(), limit: byte_limit, io: Mutex::new(()) }
    }

    fn path(&self, uri: &str) -> PathBuf {
        let digest = Sha256::digest(uri.as_bytes());
        let mut name = String::with_capacity(70);
        for byte in digest {
            let _ = write!(name, "{byte:02x}");
        }
        name.push_str(".media");
        self.directory.join(name)
    }

    fn trim(&self) {
        let Ok(files) = fs::read_dir(&self.directory) else { return };
        let mut files: Vec<_> = files
            .flatten()
            .filter_map(|file| {
                let path = file.path();
                if !is_entry(&path) || !file.file_type().ok()?.is_file() {
                    return None;
                }
                let meta = file.metadata().ok()?;
                Some((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), path, meta.len()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|entry| entry.2).sum();
        files.sort_unstable();
        for (_, path, size) in files {
            if total <= self.limit {
                break;
            }
            if fs::remove_file(path).is_ok() {
                total -= size;
            }
        }
    }
}

impl MediaCacheBackend for DiskMediaCache {
    fn get(&self, uri: &str) -> Option<HttpMediaEntry> {
        use std::io::{BufRead, Read};
        let _guard = self.io.lock().ok()?;
        let file = fs::File::open(self.path(uri)).ok()?;
        let size = file.metadata().ok()?.len();
        if size > self.limit || size > super::MAX_TRANSFER as u64 + 16 * 1024 {
            return None;
        }
        let mut reader = BufReader::new(file);
        let mut raw = Vec::new();
        (&mut reader).take(16 * 1024).read_until(b'\n', &mut raw).ok()?;
        let meta: Metadata = serde_json::from_slice(&raw).ok()?;
        let mut bytes = Vec::new();
        reader.take(size).read_to_end(&mut bytes).ok()?;
        let _ = fs::OpenOptions::new().write(true).open(self.path(uri)).and_then(|file| file.set_modified(SystemTime::now()));
        Some(HttpMediaEntry {
            bytes: bytes.into(),
            mime: meta.mime,
            etag: meta.etag,
            fresh_until: meta.fresh_until,
            stale_until: meta.stale_until,
            max_age: meta.max_age,
            stale_while_revalidate: meta.stale_while_revalidate,
        })
    }

    fn put(&self, uri: &str, entry: HttpMediaEntry) {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        if entry.bytes.len() as u64 > self.limit || entry.bytes.len() > super::MAX_TRANSFER {
            return;
        }
        let Ok(_guard) = self.io.lock() else { return };
        if fs::create_dir_all(&self.directory).is_err() {
            return;
        }
        let path = self.path(uri);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_extension(format!("{}.{}.tmp", std::process::id(), sequence));
        let result = (|| -> std::io::Result<()> {
            let meta = Metadata {
                mime: entry.mime,
                etag: entry.etag,
                fresh_until: entry.fresh_until,
                stale_until: entry.stale_until,
                max_age: entry.max_age,
                stale_while_revalidate: entry.stale_while_revalidate,
            };
            let mut f = BufWriter::new(fs::OpenOptions::new().write(true).create_new(true).open(&temp)?);
            serde_json::to_writer(&mut f, &meta)?;
            f.write_all(b"\n")?;
            f.write_all(&entry.bytes)?;
            f.flush()?;
            drop(f);
            fs::rename(&temp, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        } else {
            self.trim();
        }
    }

    fn remove(&self, uri: &str) {
        if let Ok(_guard) = self.io.lock() {
            let _ = fs::remove_file(self.path(uri));
        }
    }
}

fn is_entry(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "media")
        && path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
}
