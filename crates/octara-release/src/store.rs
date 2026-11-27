//! Candidate storage and the journal of admitted, running and confirmed versions.

use super::{PACKET_LIMIT, Policy, Version, invalid, read_limited};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const JOURNAL: &str = "selection.json";

/// The scoped, application-owned storage for one installed release policy.
#[derive(Clone)]
pub struct Store {
    directory: PathBuf,
    policy: Arc<Policy>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Staged {
    New(Version),
    AlreadyAdmitted(Version),
}

pub(super) struct Prepared {
    version: Version,
    bytes: Vec<u8>,
}

/// Resources selected for a new run and the lease used to confirm that run.
pub struct Startup<T> {
    /// Built-in resources are used when this is absent.
    pub selected: Option<(Version, T)>,
    pub recovery: Option<String>,
    pub session: Session,
}

/// One running application owns this lease. Keep it until the application stops.
pub struct Session {
    store: Store,
    _lease: File,
    pending: Option<Version>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    highest: Option<Version>,
    candidate: Option<Version>,
    pending: Option<Version>,
    confirmed: Option<Version>,
}

impl Store {
    pub fn new(directory: impl AsRef<Path>, policy: Policy) -> Self {
        Self { directory: directory.as_ref().join(policy.namespace()), policy: Arc::new(policy) }
    }

    /// Verifies the publisher and lets the installed application check the payload before admission.
    /// Complete bytes are installed before the new candidate and sequence floor become visible.
    pub fn stage<T>(&self, packet: impl Read, prepare: impl FnOnce(&str) -> io::Result<T>) -> io::Result<Staged> {
        let prepared = self.prepare(packet, None, prepare)?;
        self.admit(prepared)
    }

    pub(super) fn prepare<T>(
        &self,
        packet: impl Read,
        expected: Option<Version>,
        prepare: impl FnOnce(&str) -> io::Result<T>,
    ) -> io::Result<Prepared> {
        let bytes = read_limited(packet, PACKET_LIMIT)?;
        let (version, payload) = self.policy.verify(&bytes)?;
        if expected.is_some_and(|expected| expected != version) {
            return Err(invalid("the signed resource does not match the published version"));
        }
        prepare(payload)?;
        Ok(Prepared { version, bytes })
    }

    pub(super) fn admit(&self, prepared: Prepared) -> io::Result<Staged> {
        let Prepared { version, bytes } = prepared;
        let _lock = self.lock_journal()?;
        let mut journal = self.read_journal()?;
        if version.already_admitted(journal.highest)? {
            return Ok(Staged::AlreadyAdmitted(version));
        }
        let path = self.packet_path(version);
        match File::open(&path) {
            Ok(existing) if read_limited(&existing, PACKET_LIMIT)? != bytes => {
                return Err(invalid(format!("stored resource {version} differs from its signed bytes")));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = temporary(&self.directory, &bytes)?;
                file.persist_noclobber(path)?;
            }
            Err(error) => return Err(error),
        }
        journal.highest = Some(version);
        journal.candidate = Some(version);
        self.write_journal(&journal)?;
        Ok(Staged::New(version))
    }

    #[cfg(feature = "http")]
    pub(super) fn namespace(&self) -> String {
        self.policy.namespace()
    }

    #[cfg(feature = "http")]
    pub(super) fn already_admitted(&self, version: Version) -> io::Result<bool> {
        self.policy.check_embedded(version)?;
        let _lock = self.lock_journal()?;
        version.already_admitted(self.read_journal()?.highest)
    }

    /// Selects resources for one new application run. Interrupted trials recover the confirmed
    /// version first. The confirmation returned here belongs only to the selected candidate.
    pub fn start<T>(&self, prepare: impl Fn(&str) -> io::Result<T>) -> io::Result<Startup<T>> {
        let lease = self.lock_file("application.lock")?;
        lease.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => io::Error::new(io::ErrorKind::WouldBlock, "the application is already using this release store"),
            TryLockError::Error(error) => error,
        })?;
        let (confirmed, mut recovery) = loop {
            let lock = self.lock_journal()?;
            let mut journal = match self.read_journal() {
                Ok(journal) => journal,
                Err(error) => break (None, Some(format!("the resource journal could not be read: {error}"))),
            };
            if let Some(version) = journal.pending.take() {
                let reason = format!("resource {version} did not confirm its first frame");
                let notice = match self.write_journal(&journal) {
                    Ok(()) => reason,
                    Err(error) => format!("{reason}. Could not record recovery: {error}"),
                };
                break (journal.confirmed, Some(notice));
            }
            let Some(version) = journal.candidate else { break (journal.confirmed, None) };

            // The application's decoder runs outside the journal lock. Check that the
            // prepared bytes still correspond to the candidate before giving it the run.
            drop(lock);
            let prepared = self.load(version, &prepare);
            let _lock = self.lock_journal()?;
            let mut journal = match self.read_journal() {
                Ok(journal) => journal,
                Err(error) => break (None, Some(format!("the resource journal could not be read: {error}"))),
            };
            if journal.candidate != Some(version) {
                continue;
            }
            journal.candidate = None;
            let reason = match prepared {
                Ok(resources) => {
                    journal.pending = Some(version);
                    match self.write_journal(&journal) {
                        Ok(()) => {
                            return Ok(Startup {
                                selected: Some((version, resources)),
                                recovery: None,
                                session: Session { store: self.clone(), _lease: lease, pending: Some(version) },
                            });
                        }
                        Err(error) => format!("resource {version} could not be selected: {error}"),
                    }
                }
                Err(error) => {
                    let reason = format!("resource {version} could not start: {error}");
                    match self.write_journal(&journal) {
                        Ok(()) => reason,
                        Err(error) => format!("{reason}. Could not clear the candidate: {error}"),
                    }
                }
            };
            break (journal.confirmed, Some(reason));
        };

        let selected = if let Some(version) = confirmed {
            match self.load(version, &prepare) {
                Ok(resources) => Some((version, resources)),
                Err(error) => {
                    let reason = format!("confirmed resource {version} is unavailable: {error}");
                    recovery = Some(recovery.map_or_else(|| reason.clone(), |earlier| format!("{earlier}. {reason}")));
                    None
                }
            }
        } else {
            None
        };
        Ok(Startup { selected, recovery, session: Session { store: self.clone(), _lease: lease, pending: None } })
    }

    fn load<T>(&self, version: Version, prepare: impl FnOnce(&str) -> io::Result<T>) -> io::Result<T> {
        let bytes = read_limited(File::open(self.packet_path(version))?, PACKET_LIMIT)?;
        let (found, payload) = self.policy.verify(&bytes)?;
        if found != version {
            return Err(invalid("stored resource does not match its recorded sequence and digest"));
        }
        prepare(payload)
    }

    fn packet_path(&self, version: Version) -> PathBuf {
        self.directory.join(version.filename())
    }

    fn lock_file(&self, name: &str) -> io::Result<File> {
        fs::create_dir_all(&self.directory)?;
        OpenOptions::new().create(true).truncate(false).read(true).write(true).open(self.directory.join(name))
    }

    fn lock_journal(&self) -> io::Result<File> {
        let lock = self.lock_file("selection.lock")?;
        lock.lock()?;
        Ok(lock)
    }

    fn read_journal(&self) -> io::Result<Journal> {
        let file = match File::open(self.directory.join(JOURNAL)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Journal::default()),
            Err(error) => return Err(error),
        };
        let journal: Journal = serde_json::from_slice(&read_limited(file, 4096)?)?;
        let valid = journal.candidate.is_none_or(|candidate| journal.highest == Some(candidate))
            && [journal.highest, journal.pending, journal.confirmed].into_iter().flatten().all(|version| {
                version.sequence > 0 && journal.highest.is_some_and(|highest| version == highest || version.sequence < highest.sequence)
            });
        if !valid {
            return Err(invalid("inconsistent resource selection journal"));
        }
        Ok(journal)
    }

    fn write_journal(&self, journal: &Journal) -> io::Result<()> {
        let file = temporary(&self.directory, &serde_json::to_vec(journal)?)?;
        file.persist(self.directory.join(JOURNAL))?;
        Ok(())
    }
}

impl Session {
    /// Confirms the candidate selected by this run after its first useful frame. A newer staged
    /// candidate remains available for a later launch. Confirmation can be retried after an I/O error.
    pub fn confirm(&self) -> io::Result<Option<Version>> {
        let Some(version) = self.pending else { return Ok(None) };
        let _lock = self.store.lock_journal()?;
        let mut journal = self.store.read_journal()?;
        if journal.confirmed == Some(version) && journal.pending.is_none() {
            return Ok(Some(version));
        }
        if journal.pending != Some(version) {
            return Err(invalid("the running resource is no longer awaiting confirmation"));
        }
        journal.pending = None;
        journal.confirmed = Some(version);
        self.store.write_journal(&journal)?;
        Ok(Some(version))
    }
}

fn temporary(directory: &Path, bytes: &[u8]) -> io::Result<tempfile::NamedTempFile> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FORMAT, ReleaseTarget};
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::json;

    fn target() -> ReleaseTarget {
        ReleaseTarget {
            app: "dev.octara.review".into(),
            channel: "direct".into(),
            platform: "macos".into(),
            runtime: "review/1".into(),
            resource_schema: "review.layout/1".into(),
            state_schema: "review.draft/1".into(),
        }
    }

    fn key(seed: u8) -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap()
    }

    fn packet(key: &Ed25519KeyPair, target: ReleaseTarget, sequence: u64, title: &str) -> Vec<u8> {
        let body = serde_json::to_vec(&json!({ "format": FORMAT, "target": target, "sequence": sequence, "payload": title })).unwrap();
        let mut packet = key.sign(&body).as_ref().to_vec();
        packet.extend(body);
        packet
    }

    fn store(root: &Path, key: &Ed25519KeyPair) -> Store {
        Store::new(root, Policy { target: target(), public_key: key.public_key().as_ref().try_into().unwrap(), embedded_sequence: 1 })
    }

    fn prepare(payload: &str) -> io::Result<String> {
        let title: String = serde_json::from_str(payload)?;
        if title.is_empty() {
            return Err(invalid("a title is required"));
        }
        Ok(title)
    }

    fn stage(store: &Store, key: &Ed25519KeyPair, sequence: u64, title: &str) -> Version {
        match store.stage(packet(key, target(), sequence, title).as_slice(), prepare).unwrap() {
            Staged::New(version) => version,
            Staged::AlreadyAdmitted(version) => panic!("{version} was staged twice"),
        }
    }

    #[test]
    fn signature_target_and_application_checks_precede_admission() {
        let root = tempfile::tempdir().unwrap();
        let trusted = key(4);
        let store = store(root.path(), &trusted);
        let valid = packet(&trusted, target(), 2, "Review");
        let mut tampered = valid.clone();
        tampered[0] ^= 1;
        assert!(store.stage(tampered.as_slice(), prepare).is_err());
        assert!(store.stage(packet(&key(5), target(), 3, "Other issuer").as_slice(), prepare).is_err());
        let mut foreign = target();
        foreign.state_schema = "review.draft/2".into();
        assert!(store.stage(packet(&trusted, foreign, 3, "Other state").as_slice(), prepare).is_err());
        assert!(store.stage(packet(&trusted, target(), 3, "").as_slice(), prepare).is_err());
        assert!(store.stage(packet(&trusted, target(), 1, "Embedded replay").as_slice(), prepare).is_err());
        assert!(!store.directory.exists());
        let Staged::New(version) = store.stage(valid.as_slice(), prepare).unwrap() else { panic!("new release") };
        assert_eq!(store.stage(valid.as_slice(), prepare).unwrap(), Staged::AlreadyAdmitted(version));
        assert!(store.stage(packet(&trusted, target(), 2, "Different signed bytes").as_slice(), prepare).is_err());
    }

    #[test]
    fn confirming_a_running_candidate_keeps_a_later_candidate_for_next_launch() {
        let root = tempfile::tempdir().unwrap();
        let trusted = key(4);
        let store = store(root.path(), &trusted);
        let b = stage(&store, &trusted, 2, "B");
        let running = store.start(prepare).unwrap();
        assert_eq!(running.selected, Some((b, "B".into())));
        assert_eq!(store.start(prepare).err().unwrap().kind(), io::ErrorKind::WouldBlock);

        let c = stage(&store, &trusted, 3, "C");
        running.session.confirm().unwrap();
        running.session.confirm().unwrap();
        assert_eq!(store.read_journal().unwrap().confirmed, Some(b));
        assert_eq!(store.read_journal().unwrap().candidate, Some(c));
        drop(running);
        let interrupted = store.start(prepare).unwrap();
        assert_eq!(interrupted.selected, Some((c, "C".into())));
        drop(interrupted);

        let recovered = store.start(prepare).unwrap();
        assert_eq!(recovered.selected, Some((b, "B".into())));
        assert!(recovered.recovery.unwrap().contains("did not confirm"));
        assert!(store.stage(packet(&trusted, target(), 2, "B").as_slice(), prepare).is_err());
        assert_eq!(store.stage(packet(&trusted, target(), 3, "C").as_slice(), prepare).unwrap(), Staged::AlreadyAdmitted(c));
        assert!(store.read_journal().unwrap().candidate.is_none());
    }

    #[test]
    fn interrupted_recovery_prefers_the_confirmed_version_before_the_next_candidate() {
        let root = tempfile::tempdir().unwrap();
        let trusted = key(4);
        let store = store(root.path(), &trusted);
        stage(&store, &trusted, 2, "B");
        let b = store.start(prepare).unwrap();
        let c = stage(&store, &trusted, 3, "C");
        drop(b);

        let fallback = store.start(prepare).unwrap();
        assert!(fallback.selected.is_none());
        assert!(fallback.recovery.as_deref().is_some_and(|reason| reason.contains("did not confirm")));
        assert_eq!(store.read_journal().unwrap().candidate, Some(c));
        drop(fallback);
        assert_eq!(store.start(prepare).unwrap().selected, Some((c, "C".into())));
    }

    #[test]
    fn preparation_cannot_install_a_candidate_superseded_during_decoding() {
        let root = tempfile::tempdir().unwrap();
        let trusted = key(4);
        let store = store(root.path(), &trusted);
        stage(&store, &trusted, 2, "B");
        let startup = store
            .start(|payload| {
                let title = prepare(payload)?;
                if title == "B" {
                    stage(&store, &trusted, 3, "C");
                }
                Ok(title)
            })
            .unwrap();
        let (version, title) = startup.selected.unwrap();
        assert_eq!((version.sequence, title.as_str()), (3, "C"));
        assert_eq!(startup.session.confirm().unwrap(), Some(version));
    }

    #[test]
    fn verifies_the_stored_candidate_and_recovers_from_unusable_local_data() {
        let root = tempfile::tempdir().unwrap();
        let trusted = key(4);
        let store = store(root.path(), &trusted);
        let b = stage(&store, &trusted, 2, "B");
        store.start(prepare).unwrap().session.confirm().unwrap();
        let c = stage(&store, &trusted, 3, "C");
        fs::copy(store.packet_path(b), store.packet_path(c)).unwrap();
        let recovered = store.start(prepare).unwrap();
        assert_eq!(recovered.selected, Some((b, "B".into())));
        assert!(recovered.recovery.unwrap().contains("recorded sequence and digest"));
        drop(recovered.session);

        fs::write(store.directory.join(JOURNAL), "unreadable journal").unwrap();
        assert!(store.start(prepare).unwrap().selected.is_none());
        assert!(store.stage(packet(&trusted, target(), 4, "D").as_slice(), prepare).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn failed_selection_and_confirmation_leave_the_prior_record_intact() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let trusted = key(4);
        let store = store(root.path(), &trusted);
        let b = stage(&store, &trusted, 2, "B");
        let permissions = fs::metadata(&store.directory).unwrap().permissions();
        drop(store.lock_file("application.lock").unwrap());
        let deny_writes = || fs::set_permissions(&store.directory, fs::Permissions::from_mode(0o500)).unwrap();
        let restore_writes = || fs::set_permissions(&store.directory, permissions.clone()).unwrap();
        deny_writes();
        if tempfile::NamedTempFile::new_in(&store.directory).is_ok() {
            // A privileged test runner can bypass the directory's mode bits.
            restore_writes();
            return;
        }
        let refused = store.start(prepare);
        restore_writes();
        let refused = refused.unwrap();
        assert!(refused.selected.is_none());
        assert_eq!(store.read_journal().unwrap().candidate, Some(b));
        drop(refused);

        let pending = store.start(prepare).unwrap();
        deny_writes();
        let failed = pending.session.confirm();
        restore_writes();
        assert!(failed.is_err());
        assert_eq!(store.read_journal().unwrap().pending, Some(b));
        assert!(store.read_journal().unwrap().confirmed.is_none());
        drop(pending);
        assert!(store.start(prepare).unwrap().selected.is_none());
        assert_eq!(store.read_journal().unwrap().highest, Some(b));
    }
}
