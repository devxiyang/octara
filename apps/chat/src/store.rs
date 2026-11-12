//! Conversations saved on disk: one JSON file per conversation in a directory, written on a
//! background thread when a turn ends, a conversation is renamed or deleted.

use crate::model::{SavedMessage, Thread, ThreadId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Saved {
    thread: Thread,
    messages: Vec<SavedMessage>,
}

/// Where conversations are kept.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
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
        let (dir, file) = (self.dir.clone(), self.file(thread.id));
        let Ok(json) = serde_json::to_string_pretty(&Saved { thread, messages }) else { return };
        drop(octara::task::background(move || write(&dir, &file, &json)));
    }

    /// Removes a conversation's file, on a background thread.
    pub fn delete(&self, id: ThreadId) {
        let file = self.file(id);
        drop(octara::task::background(move || {
            let _ = std::fs::remove_file(file);
        }));
    }
}

/// Writes through a temporary file, so a crash never leaves half a conversation.
fn write(dir: &Path, file: &Path, json: &str) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = file.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}
