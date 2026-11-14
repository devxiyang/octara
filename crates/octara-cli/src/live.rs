//! A serialized JSON-lines connection to an opted-in desktop window.

use std::path::Path;

/// Maximum request size, excluding the newline, accepted by the live host.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// Maximum reply size, excluding the newline.
pub const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;

/// One connection; the next reply belongs to the preceding request.
pub struct Client {
    #[cfg(unix)]
    stream: std::io::BufReader<std::os::unix::net::UnixStream>,
}

impl Client {
    /// Connect to the socket named by the window's `OCTARA_INSPECT` setting.
    pub fn connect(path: &Path) -> Result<Self, String> {
        #[cfg(unix)]
        {
            let stream = std::os::unix::net::UnixStream::connect(path).map_err(|e| {
                format!("cannot connect to {}: {e}; start the window with OCTARA_INSPECT set to this socket", path.display())
            })?;
            let timeout = Some(std::time::Duration::from_secs(120));
            stream.set_read_timeout(timeout).map_err(|e| format!("live read timeout: {e}"))?;
            stream.set_write_timeout(timeout).map_err(|e| format!("live write timeout: {e}"))?;
            Ok(Self { stream: std::io::BufReader::new(stream) })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err("live connections currently require Unix domain sockets (macOS or Linux)".into())
        }
    }

    /// Send one request and read its reply. A transport failure is never retried: delivery may
    /// already have happened, so repeating an action could execute it twice.
    pub fn request(&mut self, line: &str) -> Result<serde_json::Value, String> {
        if line.len() > MAX_REQUEST_BYTES {
            return Err("live request exceeds 1 MiB; it was not sent".into());
        }
        if line.contains(['\n', '\r']) {
            return Err("a live request must be one JSON line; it was not sent".into());
        }
        #[cfg(unix)]
        {
            use std::io::{BufRead, Read, Write};
            let uncertain =
                |e: String| format!("live connection failed: {e}; the action's outcome may be unknown; inspect before retrying");
            let stream = self.stream.get_mut();
            stream
                .write_all(line.as_bytes())
                .and_then(|()| stream.write_all(b"\n"))
                .and_then(|()| stream.flush())
                .map_err(|e| uncertain(e.to_string()))?;
            let mut reply = Vec::new();
            Read::take(&mut self.stream, (MAX_REPLY_BYTES + 2) as u64)
                .read_until(b'\n', &mut reply)
                .map_err(|e| uncertain(e.to_string()))?;
            if reply.last() != Some(&b'\n') {
                return Err(uncertain(
                    if reply.len() > MAX_REPLY_BYTES { "reply exceeds 16 MiB" } else { "the window disconnected before a complete reply" }
                        .into(),
                ));
            }
            reply.pop();
            if reply.last() == Some(&b'\r') {
                reply.pop();
            }
            if reply.len() > MAX_REPLY_BYTES {
                return Err(uncertain("reply exceeds 16 MiB".into()));
            }
            let value: serde_json::Value = serde_json::from_slice(&reply).map_err(|e| uncertain(format!("unreadable reply: {e}")))?;
            if !value["ok"].is_boolean() {
                return Err(uncertain("reply has no boolean `ok` field".into()));
            }
            Ok(value)
        }
        #[cfg(not(unix))]
        {
            Err("live connections currently require Unix domain sockets (macOS or Linux)".into())
        }
    }
}
