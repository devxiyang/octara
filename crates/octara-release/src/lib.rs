//! Signed resource data. The installed application owns the trust policy and decodes the payload.

#[cfg(any(feature = "http", feature = "signing"))]
mod channel;
#[cfg(feature = "http")]
mod http;
mod store;

use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, Read};

#[cfg(feature = "http")]
pub use http::{Channel, Check};
pub use store::{Session, Staged, Startup, Store};

const FORMAT: &str = "octara.resource.v1";
const DOCUMENT_LIMIT: usize = 1024 * 1024;
const SIGNATURE_LEN: usize = 64;
const PACKET_LIMIT: usize = SIGNATURE_LEN + DOCUMENT_LIMIT;
const PACKET_EXTENSION: &str = "octara-resource";

/// The application, installed runtime and data formats for which a package was issued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTarget {
    pub app: String,
    pub channel: String,
    pub platform: String,
    pub runtime: String,
    pub resource_schema: String,
    pub state_schema: String,
}

/// The publisher and embedded version trusted by an installed application.
pub struct Policy {
    pub target: ReleaseTarget,
    pub public_key: [u8; 32],
    pub embedded_sequence: u64,
}

/// A signed sequence and the SHA-256 of the exact signed document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub sequence: u64,
    digest: [u8; 32],
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.sequence, hex(&self.digest))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceDocument<'a> {
    format: String,
    target: ReleaseTarget,
    sequence: u64,
    #[serde(borrow)]
    payload: &'a RawValue,
}

fn document(bytes: &[u8]) -> io::Result<ResourceDocument<'_>> {
    if bytes.len() > DOCUMENT_LIMIT {
        return Err(invalid("resource document exceeds 1 MiB"));
    }
    let document: ResourceDocument<'_> = serde_json::from_slice(bytes)?;
    if document.format != FORMAT || document.sequence == 0 {
        return Err(invalid("expected an octara.resource.v1 document with a positive sequence"));
    }
    Ok(document)
}

impl Policy {
    fn verify<'a>(&self, packet: &'a [u8]) -> io::Result<(Version, &'a str)> {
        let (version, document) = authenticate(&self.public_key, packet)?;
        if document.target != self.target {
            return Err(invalid("resource target does not match the installed application, channel, platform, runtime and schemas"));
        }
        self.check_embedded(version)?;
        Ok((version, document.payload.get()))
    }

    fn check_embedded(&self, version: Version) -> io::Result<()> {
        if version.sequence <= self.embedded_sequence {
            return Err(invalid(format!(
                "resource sequence {} does not exceed embedded sequence {}",
                version.sequence, self.embedded_sequence
            )));
        }
        Ok(())
    }

    fn namespace(&self) -> String {
        namespace(&self.target, &self.public_key)
    }
}

impl Version {
    // An admitted sequence identifies exactly one version, even when an older
    // version is still running or was recovered locally.
    fn already_admitted(self, highest: Option<Self>) -> io::Result<bool> {
        let Some(highest) = highest else { return Ok(false) };
        if self.sequence < highest.sequence {
            return Err(invalid(format!("sequence {} is below admitted sequence {}", self.sequence, highest.sequence)));
        }
        if self.sequence == highest.sequence && self != highest {
            return Err(invalid(format!("sequence {} already names different signed bytes", self.sequence)));
        }
        Ok(self == highest)
    }

    fn filename(self) -> String {
        format!("{self}.{PACKET_EXTENSION}")
    }
}

fn authenticate<'a>(public_key: &[u8; 32], packet: &'a [u8]) -> io::Result<(Version, ResourceDocument<'a>)> {
    let (signature, bytes) = packet.split_at_checked(SIGNATURE_LEN).ok_or_else(|| invalid("incomplete signed resource"))?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(bytes, signature)
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "resource signature does not match the publisher key"))?;
    let document = document(bytes)?;
    let version = Version { sequence: document.sequence, digest: Sha256::digest(bytes).into() };
    Ok((version, document))
}

fn namespace(target: &ReleaseTarget, public_key: &[u8; 32]) -> String {
    let mut hash = Sha256::new_with_prefix(public_key);
    hash.update(serde_json::to_vec(target).expect("release targets serialize"));
    format!("v1-{}", hex(&hash.finalize()))
}

/// Parses the public key printed by the signing tool.
pub fn public_key(encoded: &str) -> io::Result<[u8; 32]> {
    if encoded.len() != 64 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("the Ed25519 public key must be 64 hex digits"));
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&encoded[index * 2..index * 2 + 2], 16).expect("validated public key");
    }
    Ok(bytes)
}

fn read_limited(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("file exceeds the size limit for a resource release"));
    }
    Ok(bytes)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(value, "{byte:02x}").expect("a String can be written");
    }
    value
}

/// Publisher-side operations. Applications can build the verifier without these entry points.
#[cfg(feature = "signing")]
pub mod signing {
    use super::{DOCUMENT_LIMIT, SIGNATURE_LEN, document, hex, invalid, read_limited};
    pub use crate::channel::{Published, publish};
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::io::{self, Read};

    /// Creates a PKCS#8 private key and the hex public key to embed in a build.
    pub fn generate_key() -> io::Result<(Vec<u8>, String)> {
        let private =
            Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| io::Error::other("could not generate an Ed25519 key"))?;
        let key = parse_key(private.as_ref())?;
        Ok((private.as_ref().to_vec(), hex(key.public_key().as_ref())))
    }

    /// Signs the document as supplied, including the target, sequence and payload.
    pub fn sign(private: &[u8], input: impl Read) -> io::Result<Vec<u8>> {
        let bytes = read_limited(input, DOCUMENT_LIMIT)?;
        document(&bytes)?;
        let signature = parse_key(private)?.sign(&bytes);
        let mut packet = Vec::with_capacity(SIGNATURE_LEN + bytes.len());
        packet.extend_from_slice(signature.as_ref());
        packet.extend_from_slice(&bytes);
        Ok(packet)
    }

    fn parse_key(private: &[u8]) -> io::Result<Ed25519KeyPair> {
        Ed25519KeyPair::from_pkcs8(private).map_err(|error| invalid(format!("invalid Ed25519 PKCS#8 key: {error}")))
    }
}
