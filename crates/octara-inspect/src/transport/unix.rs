//! Non-blocking local socket transport. Call `pump` from the host's owning event loop.

use super::{InspectTransport, REQUEST_LIMIT, malformed};
use std::{
    fs,
    io::{self, ErrorKind, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

const MAX_CONNECTIONS: usize = 16;
const READ_BUDGET: usize = 64 * 1024;

pub struct UnixTransport {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    clients: Vec<Client>,
}

impl UnixTransport {
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let listener = UnixListener::bind(path)?;
        let metadata = fs::symlink_metadata(path)?;
        let server = Self { listener, path: path.into(), identity: (metadata.dev(), metadata.ino()), clients: Vec::new() };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        server.listener.set_nonblocking(true)?;
        Ok(server)
    }

    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    /// The readiness interests for each accepted client. Buffered requests can be pumped
    /// again immediately when the prior call returns progress.
    pub fn connections(&self) -> impl Iterator<Item = (&UnixStream, bool)> {
        self.clients.iter().map(|client| (&client.stream, client.written < client.output.len()))
    }

    /// Accept and make bounded progress on connected clients without waiting for I/O.
    /// Incoming lines are executed on the caller's thread.
    pub fn pump(&mut self, handler: &mut impl InspectTransport) -> io::Result<usize> {
        for _ in self.clients.len()..MAX_CONNECTIONS {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(true)?;
                    self.clients.push(Client::new(stream));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        let mut progress = 0;
        self.clients.retain_mut(|client| match client.pump(handler) {
            Ok((keep, made_progress)) => {
                progress += usize::from(made_progress);
                keep
            }
            Err(_) => false,
        });
        Ok(progress)
    }
}

impl Drop for UnixTransport {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && self.identity == (metadata.dev(), metadata.ino())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct Client {
    stream: UnixStream,
    input: Vec<u8>,
    output: String,
    searched: usize,
    written: usize,
    close_after_write: bool,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        Self { stream, input: Vec::new(), output: String::new(), searched: 0, written: 0, close_after_write: false }
    }

    fn pump(&mut self, handler: &mut impl InspectTransport) -> io::Result<(bool, bool)> {
        let mut progressed = false;
        let mut budget = READ_BUDGET;
        while self.written == self.output.len() {
            if self.close_after_write {
                return Ok((false, progressed));
            }
            self.output.clear();
            self.written = 0;
            if let Some(index) = self.input[self.searched..].iter().position(|&byte| byte == b'\n') {
                let end = self.searched + index;
                self.output = handler.exchange(&self.input[..end]);
                self.input.drain(..=end);
                self.searched = 0;
                progressed = true;
                break;
            }
            if self.input.len() > REQUEST_LIMIT {
                self.output = malformed("request exceeds the line size limit");
                self.close_after_write = true;
                break;
            }
            if budget == 0 {
                return Ok((true, progressed));
            }
            self.searched = self.input.len();
            let mut buffer = [0_u8; 8192];
            match self.stream.read(&mut buffer) {
                Ok(0) if self.input.is_empty() => return Ok((false, progressed)),
                Ok(0) => {
                    self.output = malformed("incomplete JSON request line");
                    self.close_after_write = true;
                    break;
                }
                Ok(count) => {
                    self.input.extend_from_slice(&buffer[..count]);
                    budget -= count;
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok((true, progressed)),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        match self.stream.write(&self.output.as_bytes()[self.written..]) {
            Ok(0) => Ok((false, progressed)),
            Ok(count) => {
                self.written += count;
                let keep = !self.close_after_write || self.written < self.output.len();
                Ok((keep, true))
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => Ok((true, progressed)),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{AgentCommand, AgentReceipt, AgentRequest, Delivery};
    use std::{
        io::{BufRead, BufReader},
        time::Duration,
    };

    #[test]
    fn fragmented_lines_and_multiple_requests_share_the_same_connection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inspect.sock");
        let mut transport = UnixTransport::bind(&path).unwrap();
        let request = serde_json::to_string(&AgentRequest::new(None, AgentCommand::ListWindows)).unwrap();
        let client = UnixStream::connect(&path).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut client = BufReader::new(client);
        let mut called = 0;
        let mut handler = |request: AgentRequest| {
            assert_eq!(request.command, AgentCommand::ListWindows);
            called += 1;
            AgentReceipt::success(None)
        };
        let (first, remainder) = request.split_at(9);
        client.get_mut().write_all(first.as_bytes()).unwrap();
        transport.pump(&mut handler).unwrap();
        client.get_mut().write_all(format!("{remainder}\nmalformed\n{request}\n").as_bytes()).unwrap();
        for _ in 0..3 {
            transport.pump(&mut handler).unwrap();
        }
        let receipts: Vec<AgentReceipt> = (0..3)
            .map(|_| {
                let mut line = String::new();
                client.read_line(&mut line).unwrap();
                serde_json::from_str(&line).unwrap()
            })
            .collect();
        assert_eq!(called, 2);
        assert!(receipts[0].ok && receipts[2].ok);
        assert_eq!(receipts[1].delivery, Delivery::Refused);
        assert_eq!(receipts[1].code.as_deref(), Some("INSPECT_REQUEST"));
        drop(transport);
        assert!(!path.exists());
    }
}
