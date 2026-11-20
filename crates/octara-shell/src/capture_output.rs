//! Encode one capture at a time and publish its completed file after host validation.

use octara::capture::Capture;
use std::fs::{self, OpenOptions, Permissions};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use tempfile::NamedTempFile;

pub(crate) struct CaptureWriter {
    queue: Arc<Queue>,
    thread: Option<thread::JoinHandle<()>>,
}

#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

#[derive(Default)]
struct QueueState {
    closed: bool,
    pending: Option<Request>,
}

type WriteCapture = Box<dyn FnOnce(&str) -> Result<Capture, String> + Send>;

struct Request {
    path: String,
    write: WriteCapture,
    reply: OutputReply,
}

struct OutputReply {
    sender: mpsc::Sender<Result<StagedCapture, String>>,
    cancelled: Arc<AtomicBool>,
    wake: Box<dyn FnOnce() + Send>,
}

impl OutputReply {
    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn send(self, result: Result<StagedCapture, String>) {
        if !self.cancelled() && self.sender.send(result).is_ok() {
            let _ = catch_unwind(AssertUnwindSafe(self.wake));
        }
    }
}

impl CaptureWriter {
    pub(crate) fn new() -> Result<Self, String> {
        let queue = Arc::new(Queue::default());
        let worker = Arc::clone(&queue);
        let thread = thread::Builder::new()
            .name("octara-capture-output".into())
            .spawn(move || {
                while let Some(Request { path, write, reply }) = worker.next() {
                    if reply.cancelled() {
                        continue;
                    }
                    let result = catch_unwind(AssertUnwindSafe(|| stage(&path, write)))
                        .unwrap_or_else(|_| Err(format!("{path}: capture output worker panicked")));
                    reply.send(result);
                }
            })
            .map_err(|error| format!("cannot start the capture output worker: {error}"))?;
        Ok(Self { queue, thread: Some(thread) })
    }

    pub(crate) fn submit(
        &mut self,
        path: String,
        write: impl FnOnce(&str) -> Result<Capture, String> + Send + 'static,
        wake: impl FnOnce() + Send + 'static,
    ) -> CaptureOutput {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let reply = OutputReply { sender, cancelled: Arc::clone(&cancelled), wake: Box::new(wake) };
        let request = Request { path: path.clone(), write: Box::new(write), reply };
        let replaced = self.queue.state.lock().expect("capture writer queue poisoned").pending.replace(request);
        self.queue.ready.notify_one();
        if let Some(Request { path, reply, .. }) = replaced {
            reply.send(Err(format!("{path}: capture output was superseded")));
        }
        CaptureOutput { path, receiver: Some(receiver), cancelled }
    }

    /// Called once the host event loop has stopped. A canceled write can now clean up
    /// before the process exits, without ever waiting for it during UI updates.
    pub(crate) fn shutdown(mut self) {
        self.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn close(&self) {
        let pending = {
            let mut state = self.queue.state.lock().expect("capture writer queue poisoned");
            state.closed = true;
            state.pending.take()
        };
        self.queue.ready.notify_one();
        drop(pending);
    }
}

impl Queue {
    fn next(&self) -> Option<Request> {
        let state = self.state.lock().expect("capture writer queue poisoned");
        let mut state =
            self.ready.wait_while(state, |state| !state.closed && state.pending.is_none()).expect("capture writer queue poisoned");
        state.pending.take()
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) struct CaptureOutput {
    path: String,
    receiver: Option<mpsc::Receiver<Result<StagedCapture, String>>>,
    cancelled: Arc<AtomicBool>,
}

impl CaptureOutput {
    pub(crate) fn try_take(&mut self) -> Option<Result<StagedCapture, String>> {
        let result = match self.receiver.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(format!("{}: capture output worker disconnected", self.path)),
        };
        self.receiver = None;
        Some(result)
    }
}

impl Drop for CaptureOutput {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub(crate) struct StagedCapture {
    capture: Capture,
    file: NamedTempFile,
    destination: PathBuf,
}

impl StagedCapture {
    /// Replace the destination only after the host confirms the capture is current.
    pub(crate) fn publish(self) -> Result<Capture, String> {
        self.file.persist(self.destination).map_err(|error| format!("{}: {}", self.capture.path, error.error))?;
        Ok(self.capture)
    }
}

fn stage(path: &str, write: impl FnOnce(&str) -> Result<Capture, String>) -> Result<StagedCapture, String> {
    let (destination, permissions) = capture_destination(Path::new(path)).map_err(|error| format!("{path}: {error}"))?;
    let parent = destination.parent().expect("a capture destination has a parent directory");
    let file = tempfile::Builder::new()
        .prefix(".octara-capture-")
        .suffix(".png")
        .tempfile_in(parent)
        .map_err(|error| format!("{path}: {error}"))?;
    let temporary = file.path().to_str().ok_or_else(|| format!("{path}: the temporary capture path is not UTF-8"))?;
    let mut capture = write(temporary).map_err(|error| {
        let reason = error.strip_prefix(temporary).and_then(|reason| reason.strip_prefix(": ")).unwrap_or(&error);
        format!("{path}: {}", reason.replace(temporary, path))
    })?;
    if let Some(permissions) = permissions {
        file.as_file().set_permissions(permissions).map_err(|error| format!("{path}: {error}"))?;
    }
    capture.path = path.to_owned();
    Ok(StagedCapture { capture, file, destination })
}

fn capture_destination(path: &Path) -> io::Result<(PathBuf, Option<Permissions>)> {
    let mut destination = std::path::absolute(path)?;
    for _ in 0..40 {
        let permissions = match fs::symlink_metadata(&destination) {
            Ok(info) if info.file_type().is_symlink() => {
                destination = destination.with_file_name(fs::read_link(&destination)?);
                continue;
            }
            Ok(info) if info.is_file() => {
                // Check write access without truncating the previous capture.
                let original = OpenOptions::new().write(true).open(&destination)?;
                Some(original.metadata()?.permissions())
            }
            Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidInput, "the capture destination must be a regular file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let name =
            destination.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the capture path needs a filename"))?;
        let parent = destination.parent().expect("an absolute filename has a parent directory");
        return Ok((parent.canonicalize()?.join(name), permissions));
    }
    Err(io::Error::new(io::ErrorKind::InvalidInput, "too many symbolic links in the capture path"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::capture::Pixels;
    use octara_primitives::capture::Picture;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(5);

    fn write_picture(path: &str) -> Result<Capture, String> {
        Picture { width: 1, height: 1, rgba: vec![255, 0, 0, 255] }.save_png(path).map_err(|error| format!("{path}: {error}"))?;
        Ok(Capture {
            path: path.to_owned(),
            width: 1,
            height: 1,
            scale: 1.0,
            pixels: Pixels::Painted,
            fallback: None,
            renderer: "test".into(),
            settled: true,
            pending: vec![],
            native: vec![],
            paint: None,
        })
    }

    fn submit(
        writer: &mut CaptureWriter,
        path: &Path,
        write: impl FnOnce(&str) -> Result<Capture, String> + Send + 'static,
    ) -> (CaptureOutput, mpsc::Receiver<()>) {
        let (wake, woke) = mpsc::channel();
        let output = writer.submit(path.to_str().unwrap().to_owned(), write, move || {
            let _ = wake.send(());
        });
        (output, woke)
    }

    #[test]
    fn publishing_replaces_the_existing_capture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        fs::write(&path, b"previous capture").unwrap();
        let mut writer = CaptureWriter::new().unwrap();
        let (mut output, woke) = submit(&mut writer, &path, write_picture);
        woke.recv_timeout(WAIT).unwrap();

        let staged = output.try_take().unwrap().unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"previous capture");
        let capture = staged.publish().unwrap();
        assert_eq!(capture.path, path.to_str().unwrap());
        assert!(fs::read(&path).unwrap().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        assert!(output.try_take().is_none());
    }

    #[test]
    fn dropping_unclaimed_output_cleans_the_temporary_capture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        fs::write(&path, b"previous capture").unwrap();
        let mut writer = CaptureWriter::new().unwrap();
        let (output, woke) = submit(&mut writer, &path, write_picture);
        woke.recv_timeout(WAIT).unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);

        drop(output);
        assert_eq!(fs::read(&path).unwrap(), b"previous capture");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn newest_pending_capture_replaces_cancelled_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        let mut writer = CaptureWriter::new().unwrap();
        let (started, running) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let (active, _) = submit(&mut writer, &path, move |temporary| {
            started.send(()).unwrap();
            resumed.recv_timeout(WAIT).unwrap();
            write_picture(temporary)
        });
        running.recv_timeout(WAIT).unwrap();
        let old_ran = Arc::new(AtomicBool::new(false));
        let ran = Arc::clone(&old_ran);
        let (old, _) = submit(&mut writer, &path, move |temporary| {
            ran.store(true, Ordering::Release);
            write_picture(temporary)
        });
        drop(old);
        drop(active);
        let (mut latest, woke) = submit(&mut writer, &path, write_picture);
        resume.send(()).unwrap();
        woke.recv_timeout(WAIT).unwrap();

        assert!(!old_ran.load(Ordering::Acquire));
        assert!(!path.exists());
        latest.try_take().unwrap().unwrap().publish().unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn shutdown_cleans_an_output_cancelled_while_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        let mut writer = CaptureWriter::new().unwrap();
        let (started, running) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let (output, _) = submit(&mut writer, &path, move |temporary| {
            let capture = write_picture(temporary)?;
            started.send(()).unwrap();
            resumed.recv_timeout(WAIT).unwrap();
            Ok(capture)
        });
        running.recv_timeout(WAIT).unwrap();
        drop(output);
        resume.send(()).unwrap();
        writer.shutdown();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn publishing_through_a_symlink_preserves_the_link_and_the_file_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.png");
        let path = dir.path().join("capture.png");
        fs::write(&target, b"previous capture").unwrap();
        fs::set_permissions(&target, Permissions::from_mode(0o640)).unwrap();
        symlink("target.png", &path).unwrap();
        let mut writer = CaptureWriter::new().unwrap();
        let (mut output, woke) = submit(&mut writer, &path, write_picture);
        woke.recv_timeout(WAIT).unwrap();

        let capture = output.try_take().unwrap().unwrap().publish().unwrap();
        assert_eq!(capture.path, path.to_str().unwrap());
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        assert!(fs::read(&target).unwrap().starts_with(b"\x89PNG"));
        assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn failed_encoding_reports_the_requested_name_and_discards_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.png");
        let mut writer = CaptureWriter::new().unwrap();
        let (mut output, woke) = submit(&mut writer, &path, |temporary| {
            fs::write(temporary, b"partial file").unwrap();
            Err(format!("{temporary}: encoding failed"))
        });
        woke.recv_timeout(WAIT).unwrap();

        assert_eq!(output.try_take().unwrap().err().unwrap(), format!("{}: encoding failed", path.display()));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
