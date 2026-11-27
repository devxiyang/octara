//! Evidence identifying the executable and the process that answers on the local socket.

use octara_inspect::{AgentReceipt, protocol::BuildIdentity};
use octara_model::diagnostics::codes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const SCHEMA: &str = "octara.launch/2";

#[derive(Deserialize, Serialize)]
pub(crate) struct Receipt {
    schema: String,
    package: String,
    entry: String,
    pub(crate) build: BuildIdentity,
}

impl Receipt {
    pub(crate) fn new(package: String, entry: String, build: BuildIdentity) -> Self {
        Self { schema: SCHEMA.into(), package, entry, build }
    }

    pub(crate) fn read(path: &Path) -> Result<Self, String> {
        let receipt: Self = File::open(path)
            .map_err(|error| error.to_string())
            .and_then(|file| serde_json::from_reader(io::BufReader::new(file)).map_err(|error| error.to_string()))
            .map_err(|error| format!("reading build receipt {}: {error}", path.display()))?;
        let digest = &receipt.build.digest;
        if receipt.schema != SCHEMA
            || receipt.build.pid == 0
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!("invalid {SCHEMA} build receipt at {}", path.display()));
        }
        Ok(receipt)
    }

    pub(crate) fn write(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let mut file =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| format!("creating receipt in {}: {error}", parent.display()))?;
        serde_json::to_writer(&mut file, self).map_err(|error| format!("writing receipt {}: {error}", path.display()))?;
        file.as_file().sync_all().map_err(|error| format!("syncing receipt {}: {error}", path.display()))?;
        file.persist_noclobber(path).map_err(|error| format!("saving receipt {}: {error}", path.display()))?;
        Ok(())
    }

    pub(crate) fn mismatch(&self, observed: &AgentReceipt) -> Option<AgentReceipt> {
        if observed.build.as_ref() == Some(&self.build) {
            return None;
        }
        let description = observed
            .build
            .as_ref()
            .map(|build| format!("{} (pid {})", build.digest, build.pid))
            .unwrap_or_else(|| "no build identity".into());
        let message = format!("expected {} (pid {}), observed {description}", self.build.digest, self.build.pid);
        let mut refusal = AgentReceipt::refused(&codes::BUILD_MISMATCH, message);
        refusal.build.clone_from(&observed.build);
        Some(refusal)
    }
}

pub(crate) fn digest(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 32 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            return Ok(hex(hash));
        }
        hash.update(&buffer[..n]);
    }
}

pub(crate) fn hex(hash: Sha256) -> String {
    use std::fmt::Write as _;
    let mut digest = String::with_capacity(64);
    for byte in hash.finalize() {
        write!(digest, "{byte:02x}").expect("writing to a String");
    }
    digest
}
