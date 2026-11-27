//! Static distribution protocol for one publisher key and application target.

use super::{Version, invalid};
use serde::{Deserialize, Serialize};
use std::io;

pub(super) const HEAD: &str = "current.json";
pub(super) const HEAD_LIMIT: usize = 4096;
const SCHEMA: u32 = 1;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Head {
    schema: u32,
    pub(super) version: Version,
}

impl Head {
    pub(super) fn read(bytes: &[u8]) -> io::Result<Self> {
        let head: Self = serde_json::from_slice(bytes)?;
        if head.schema != SCHEMA || head.version.sequence == 0 {
            return Err(invalid("invalid update channel pointer"));
        }
        Ok(head)
    }
}

pub(super) fn package_path(namespace: &str, version: Version) -> String {
    format!("{namespace}/packages/{}", version.filename())
}

#[cfg(feature = "signing")]
pub use publisher::{Published, publish};

#[cfg(feature = "signing")]
mod publisher {
    use super::*;
    use crate::{PACKET_LIMIT, authenticate, namespace, read_limited};
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::path::Path;

    /// Relative paths to upload, in package-then-pointer order.
    #[derive(Debug, Serialize)]
    pub struct Published {
        pub version: Version,
        pub package: String,
        pub pointer: String,
    }

    /// Writes the immutable signed packet before installing the channel's current pointer.
    /// The publisher needs only the public key used to validate the supplied packet.
    pub fn publish(site: &Path, public_key: [u8; 32], packet: impl Read) -> io::Result<Published> {
        let packet = read_limited(packet, PACKET_LIMIT)?;
        let (version, document) = authenticate(&public_key, &packet)?;
        let namespace = namespace(&document.target, &public_key);
        let directory = site.join(&namespace);
        let pointer = format!("{namespace}/{HEAD}");
        let package = package_path(&namespace, version);
        let published = Published { version, package, pointer };
        fs::create_dir_all(directory.join("packages"))?;
        let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(directory.join(".publish.lock"))?;
        lock.lock()?;
        let current = match File::open(site.join(&published.pointer)) {
            Ok(file) => Some(Head::read(&read_limited(file, HEAD_LIMIT)?)?.version),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let duplicate = version.already_admitted(current)?;
        let path = site.join(&published.package);
        match File::open(&path) {
            Ok(file) if read_limited(&file, PACKET_LIMIT)? != packet => {
                return Err(invalid("published path contains different bytes"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => write(&path, &packet)?,
            Err(error) => return Err(error),
        }
        if !duplicate {
            write(&site.join(&published.pointer), &serde_json::to_vec(&Head { schema: SCHEMA, version })?)?;
        }
        Ok(published)
    }

    fn write(destination: &Path, bytes: &[u8]) -> io::Result<()> {
        let stage = tempfile::tempdir_in(destination.parent().expect("published files are in the site"))?;
        let path = stage.path().join("complete");
        let mut file = File::create(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(path, destination)
    }
}

#[cfg(all(test, feature = "signing"))]
mod tests {
    use super::*;
    use crate::signing::{generate_key, sign};
    use serde_json::json;
    use std::fs;

    fn packet(key: &[u8], channel: &str, sequence: u64, content: &str) -> Vec<u8> {
        let document = serde_json::to_vec(&json!({
            "format": "octara.resource.v1",
            "target": {
                "app": "dev.octara.review", "channel": channel, "platform": "macos", "runtime": "review/1",
                "resource_schema": "review.layout/1", "state_schema": "review.draft/1",
            },
            "sequence": sequence, "payload": content,
        }))
        .unwrap();
        sign(key, document.as_slice()).unwrap()
    }

    #[test]
    fn publishes_immutable_bytes_before_advancing_only_the_matching_channel() {
        let site = tempfile::tempdir().unwrap();
        let (key, public) = generate_key().unwrap();
        let public = crate::public_key(&public).unwrap();
        let first = packet(&key, "direct", 1, "A");
        assert!(publish(site.path(), [0; 32], first.as_slice()).is_err());

        let a = publish(site.path(), public, first.as_slice()).unwrap();
        assert_eq!(fs::read(site.path().join(&a.package)).unwrap(), first);
        assert_eq!(Head::read(&fs::read(site.path().join(&a.pointer)).unwrap()).unwrap().version, a.version);
        let other = publish(site.path(), public, packet(&key, "preview", 1, "Preview").as_slice()).unwrap();
        assert_ne!(a.pointer, other.pointer);

        let next = packet(&key, "direct", 2, "B");
        let b = publish(site.path(), public, next.as_slice()).unwrap();
        assert_eq!(a.pointer, b.pointer);
        assert_eq!(publish(site.path(), public, next.as_slice()).unwrap().version, b.version);
        assert!(publish(site.path(), public, packet(&key, "direct", 2, "Other bytes").as_slice()).is_err());
        assert!(publish(site.path(), public, first.as_slice()).is_err());
        assert_eq!(Head::read(&fs::read(site.path().join(&b.pointer)).unwrap()).unwrap().version, b.version);
        assert_eq!(fs::read(site.path().join(&a.package)).unwrap(), first);
    }
}
