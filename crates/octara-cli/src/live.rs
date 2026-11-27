//! A serialized JSON-lines connection to an inspection host.

use octara_inspect::protocol::{AgentReceipt, PROTOCOL};
use std::io::BufRead;
use std::path::Path;

const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: usize = 32 * 1024 * 1024;

/// One connection. Each reply belongs to the preceding request.
pub struct Client {
    #[cfg(unix)]
    stream: std::io::BufReader<std::os::unix::net::UnixStream>,
}

impl Client {
    /// Connect to the app's Unix socket.
    pub fn connect(path: &Path) -> Result<Self, String> {
        Self::connect_with_timeout(path, std::time::Duration::from_secs(120))
    }

    #[doc(hidden)]
    pub fn connect_with_timeout(path: &Path, timeout: std::time::Duration) -> Result<Self, String> {
        #[cfg(unix)]
        {
            let stream =
                std::os::unix::net::UnixStream::connect(path).map_err(|error| format!("cannot connect to {}: {error}", path.display()))?;
            stream.set_read_timeout(Some(timeout)).map_err(|error| error.to_string())?;
            stream.set_write_timeout(Some(timeout)).map_err(|error| error.to_string())?;
            Ok(Self { stream: std::io::BufReader::new(stream) })
        }
        #[cfg(not(unix))]
        {
            let _ = (path, timeout);
            Err("live inspection requires a Unix domain socket".into())
        }
    }

    /// Exchange a protocol line. If transport fails, the action may already have run.
    pub fn request(&mut self, line: &str) -> Result<AgentReceipt, String> {
        #[cfg(unix)]
        {
            let uncertain = |error| format!("live connection failed: {error}. The action may have run. Observe before retrying");
            write_request(self.stream.get_mut(), line).map_err(uncertain)?;
            read_reply(&mut self.stream).map_err(uncertain)
        }
        #[cfg(not(unix))]
        {
            let _ = line;
            Err("live inspection requires a Unix domain socket".into())
        }
    }
}

pub fn write_request(mut stream: impl std::io::Write, line: &str) -> Result<(), String> {
    if line.len() > MAX_REQUEST_BYTES || line.contains(['\r', '\n']) {
        return Err("expected a single JSON request of at most 1 MiB".into());
    }
    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.write_all(b"\n"))
        .and_then(|()| stream.flush())
        .map_err(|error| error.to_string())
}

pub fn read_reply(stream: &mut impl BufRead) -> Result<AgentReceipt, String> {
    let mut reply = Vec::new();
    std::io::Read::take(stream, (MAX_REPLY_BYTES + 1) as u64).read_until(b'\n', &mut reply).map_err(|error| error.to_string())?;
    if reply.last() != Some(&b'\n') {
        return Err(if reply.len() > MAX_REPLY_BYTES { "reply exceeds 32 MiB" } else { "host disconnected before a complete reply" }.into());
    }
    let receipt: AgentReceipt = serde_json::from_slice(&reply).map_err(|error| format!("unreadable receipt: {error}"))?;
    if receipt.protocol != PROTOCOL {
        return Err(format!("unsupported receipt protocol `{}`", receipt.protocol));
    }
    Ok(receipt)
}
