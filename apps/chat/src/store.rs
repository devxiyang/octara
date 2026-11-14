//! Conversations saved on disk: one JSON file per conversation in a directory, written on a
//! background thread when a turn ends, a conversation is renamed or deleted.

use crate::model::{SavedMessage, Thread, ThreadId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Serialize, Deserialize)]
struct Saved {
    thread: Thread,
    messages: Vec<SavedMessage>,
}

/// Where conversations are kept. Clones share write ordering; use clones of one Store for
/// concurrent writes to the same directory within an app.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
    writer: Arc<Writer>,
}

/// The UI only takes `latest` long enough to submit a revision. Disk work takes `serial`, then
/// checks freshness without keeping the UI's lock held during I/O. A late worker can neither
/// overwrite a newer save nor recreate a conversation whose delete was accepted later.
#[derive(Debug, Default)]
struct Writer {
    latest: Mutex<HashMap<ThreadId, u64>>,
    serial: Mutex<()>,
}

impl Writer {
    fn issue(&self, id: ThreadId) -> u64 {
        let revision = next_revision();
        self.latest.lock().expect("chat store revisions").insert(id, revision);
        revision
    }

    fn apply(&self, id: ThreadId, revision: u64, work: impl FnOnce()) {
        let _serial = self.serial.lock().expect("chat store writer");
        if self.latest.lock().expect("chat store revisions").get(&id) != Some(&revision) {
            return;
        }
        work();
        let mut latest = self.latest.lock().expect("chat store revisions");
        if latest.get(&id) == Some(&revision) {
            latest.remove(&id);
        }
    }
}

fn next_revision() -> u64 {
    // Unique across independent Store instances too: their temporary paths must not collide.
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into(), writer: Arc::default() }
    }

    /// The default place: the platform's application data directory (`OCTARA_CHAT_DIR` overrides it).
    pub fn default_dir() -> Option<PathBuf> {
        if let Some(d) = std::env::var_os("OCTARA_CHAT_DIR") {
            return Some(d.into());
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if cfg!(target_os = "macos") {
            home.map(|h| h.join("Library/Application Support/Octara Chat"))
        } else if cfg!(windows) {
            std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Octara Chat"))
        } else {
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or(home.map(|h| h.join(".local/share"))).map(|d| d.join("octara-chat"))
        }
    }

    fn file(&self, id: ThreadId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Every conversation saved, newest first (unreadable files are skipped).
    pub fn load(&self) -> Vec<(Thread, Vec<SavedMessage>)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out: Vec<(Thread, Vec<SavedMessage>)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|s| serde_json::from_str::<Saved>(&s).ok())
            .map(|s| (s.thread, s.messages))
            .collect();
        out.sort_by_key(|t| std::cmp::Reverse(t.0.updated));
        out
    }

    /// Writes a conversation, on a background thread.
    pub fn save(&self, thread: Thread, messages: Vec<SavedMessage>) {
        let id = thread.id;
        let (dir, file) = (self.dir.clone(), self.file(thread.id));
        let Ok(json) = serde_json::to_string_pretty(&Saved { thread, messages }) else { return };
        let writer = self.writer.clone();
        let revision = writer.issue(id);
        drop(octara::task::background(move || writer.apply(id, revision, || write(&dir, &file, &json, revision))));
    }

    /// Removes a conversation's file, on a background thread.
    pub fn delete(&self, id: ThreadId) {
        let file = self.file(id);
        let writer = self.writer.clone();
        let revision = writer.issue(id);
        drop(octara::task::background(move || {
            writer.apply(id, revision, || {
                let _ = std::fs::remove_file(file);
            })
        }));
    }
}

/// Writes through a temporary file, so a crash never leaves half a conversation.
fn write(dir: &Path, file: &Path, json: &str, revision: u64) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = file.with_extension(format!("json.{}-{revision}.tmp", std::process::id()));
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
    let _ = std::fs::remove_file(tmp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    struct Directory(PathBuf);

    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("octara-chat-store-{}-{}", std::process::id(), next_revision()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn save(&self, writer: &Writer, revision: u64, title: &str) {
            let json =
                serde_json::to_string(&Saved { thread: Thread { id: 1, title: title.into(), updated: 0 }, messages: Vec::new() }).unwrap();
            writer.apply(1, revision, || write(&self.0, &self.0.join("1.json"), &json, revision));
        }

        fn delete(&self, writer: &Writer, revision: u64) {
            writer.apply(1, revision, || {
                let _ = std::fs::remove_file(self.0.join("1.json"));
            });
        }

        fn title(&self) -> Option<String> {
            Store::new(&self.0).load().first().map(|(thread, _)| thread.title.clone())
        }
    }

    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn saves_follow_accepted_order_even_when_workers_run_backwards() {
        let dir = Directory::new();
        let writer = Writer::default();
        let first = writer.issue(1);
        let second = writer.issue(1);
        dir.save(&writer, second, "new");
        dir.save(&writer, first, "old");
        assert_eq!(dir.title().as_deref(), Some("new"));
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 1, "finished jobs leave no temporary files");
        assert!(writer.latest.lock().unwrap().is_empty());
    }

    #[test]
    fn an_older_save_cannot_recreate_a_deleted_conversation() {
        let dir = Directory::new();
        let writer = Writer::default();
        let existing = writer.issue(1);
        dir.save(&writer, existing, "existing");
        let save = writer.issue(1);
        let delete = writer.issue(1);
        dir.delete(&writer, delete);
        dir.save(&writer, save, "stale");
        assert_eq!(dir.title(), None);
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
    }

    #[test]
    fn a_new_save_after_delete_survives_an_older_delete_worker() {
        let dir = Directory::new();
        let writer = Writer::default();
        let delete = writer.issue(1);
        let save = writer.issue(1);
        dir.save(&writer, save, "restored");
        dir.delete(&writer, delete);
        assert_eq!(dir.title().as_deref(), Some("restored"));
    }

    #[test]
    fn submitting_a_new_revision_does_not_wait_for_disk_work() {
        let writer = Arc::new(Writer::default());
        let first = writer.issue(1);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let running = writer.clone();
        let worker = std::thread::spawn(move || {
            running.apply(1, first, || {
                entered_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (submitted_tx, submitted_rx) = mpsc::channel();
        let accepting = writer.clone();
        let submit = std::thread::spawn(move || submitted_tx.send(accepting.issue(1)).unwrap());
        let submitted = submitted_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        submit.join().unwrap();
        let latest = submitted.expect("submitting must not wait for the older disk operation");
        assert_eq!(writer.latest.lock().unwrap().get(&1), Some(&latest));
        let mut applied = false;
        writer.apply(1, latest, || applied = true);
        assert!(applied);
        assert!(writer.latest.lock().unwrap().is_empty());
    }
}
