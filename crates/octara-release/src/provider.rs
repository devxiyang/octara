use crate::Version;
use std::io::{self, Read};

/// Source of release pointers and exact signed packets for a release store.
///
/// The store derives the namespace from the installed target and publisher key.
/// Providers may select a version for a rollout cohort. The store authenticates
/// the returned packet and checks that the pointer still names it before admission.
pub trait ReleaseProvider: Send + Sync {
    /// Reads the current pointer, or returns `None` if no release is available.
    fn current(&self, namespace: &str) -> io::Result<Option<Version>>;

    /// Opens the signed packet for a pointer. The store bounds the number of bytes read.
    fn fetch(&self, namespace: &str, version: Version) -> io::Result<Box<dyn Read + '_>>;
}

/// Outcome of checking a provider for an update, separate from startup and confirmation.
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    Unpublished,
    AlreadyAdmitted(Version),
    Staged(Version),
    Superseded,
}
