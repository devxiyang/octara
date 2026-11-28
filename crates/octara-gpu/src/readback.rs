//! Mapped capture buffers and off-thread PNG encoding and publication.

use crate::generation::ReadbackLease;
use octara_model::capture::{PaintReceipt, PaintedPicture};
use octara_model::paint::Picture;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, BufWriter};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak, mpsc};
use std::thread;
use tempfile::NamedTempFile;

/// A mapped readback and the device capacity it still occupies.
pub struct CapturePixels {
    size: [u32; 2],
    // Drop the readback storage before releasing its capacity, including on cancellation.
    unpack: Prepare,
    _lease: Arc<ReadbackLease>,
}

impl CapturePixels {
    pub(super) fn new(
        size: [u32; 2],
        unpack: impl FnOnce() -> Result<PaintedPicture, String> + Send + 'static,
        lease: Arc<ReadbackLease>,
    ) -> Self {
        Self { size, unpack: Box::new(unpack), _lease: lease }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Unpack aligned GPU rows into the model's contiguous picture.
    pub fn finish(self) -> Result<PaintedPicture, String> {
        (self.unpack)()
    }
}

/// A single worker for capture preparation, PNG compression, and file I/O.
pub struct CaptureWriter {
    queue: Arc<WriterQueue>,
    worker: Option<thread::JoinHandle<()>>,
}

#[derive(Default)]
struct WriterQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
}

#[derive(Default)]
struct QueueState {
    closed: bool,
    pending: Option<Stage>,
    files: VecDeque<FileWork>,
}

type Prepare = Box<dyn FnOnce() -> Result<PaintedPicture, String> + Send>;

struct Stage {
    path: PathBuf,
    prepare: Prepare,
    reply: Reply<StagedCapture>,
}

enum FileWork {
    Publish { id: u64, reply: Reply<PathBuf> },
    Discard(u64),
}

impl CaptureWriter {
    pub fn spawn() -> io::Result<Self> {
        Self::spawn_with_thread_setup(|| {})
    }

    /// Run host thread setup, such as lowering scheduling priority, before encoding.
    pub fn spawn_with_thread_setup(setup: impl FnOnce() + Send + 'static) -> io::Result<Self> {
        let queue = Arc::new(WriterQueue::default());
        let inbox = queue.clone();
        let worker = thread::Builder::new().name("octara-capture-output".into()).spawn(move || {
            let _ = catch_unwind(AssertUnwindSafe(setup));
            worker(&inbox);
        })?;
        Ok(Self { queue, worker: Some(worker) })
    }

    /// Stage a capture in its destination directory. The worker invokes `prepare`, so
    /// it can also unpack a mapped GPU buffer or compose a capture before encoding.
    /// A new request supersedes any pending encode that has yet to start.
    pub fn stage(
        &self,
        path: impl Into<PathBuf>,
        prepare: impl FnOnce() -> Result<PaintedPicture, String> + Send + 'static,
        wake: impl FnOnce() + Send + 'static,
    ) -> CaptureOutput<StagedCapture> {
        let (reply, output) = Reply::new(wake);
        let stage = Stage { path: path.into(), prepare: Box::new(prepare), reply };
        let mut state = self.queue.state.lock().expect("capture writer queue");
        if state.closed {
            drop(state);
            stage.reply.send(Err("the capture output worker stopped".into()));
        } else {
            let replaced = state.pending.replace(stage);
            drop(state);
            self.queue.ready.notify_one();
            if let Some(replaced) = replaced {
                replaced.reply.send(Err(format!("{}: capture output was superseded", replaced.path.display())));
            }
        }
        output
    }

    /// Wait for a running write to clean up after the host event loop exits.
    pub fn shutdown(mut self) {
        self.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn close(&self) {
        self.queue.state.lock().expect("capture writer queue").closed = true;
        self.queue.ready.notify_one();
    }
}

impl Drop for CaptureWriter {
    fn drop(&mut self) {
        self.close();
    }
}

fn worker(queue: &Arc<WriterQueue>) {
    let mut state = queue.state.lock().expect("capture writer queue");
    let mut staged = HashMap::<u64, StagedFile>::new();
    let mut next_id = 0u64;
    loop {
        state = queue
            .ready
            .wait_while(state, |state| !state.closed && state.pending.is_none() && state.files.is_empty())
            .expect("capture writer queue");
        if state.closed {
            let abandoned = std::mem::take(&mut *state);
            state.closed = true;
            drop(state);
            drop(abandoned);
            return;
        }
        if let Some(work) = state.files.pop_front() {
            drop(state);
            match work {
                FileWork::Publish { id, reply } => {
                    let file = staged.remove(&id).expect("the worker owns this staged capture");
                    if !reply.cancelled() {
                        reply.send(file.publish());
                    }
                }
                FileWork::Discard(id) => drop(staged.remove(&id)),
            }
        } else if let Some(Stage { path, prepare, reply }) = state.pending.take() {
            drop(state);
            if !reply.cancelled() {
                let result = catch_unwind(AssertUnwindSafe(|| stage(&path, prepare)))
                    .unwrap_or_else(|_| Err(format!("{}: capture output worker panicked", path.display())))
                    .map(|(file, size, effects)| {
                        let id = next_id;
                        next_id = next_id.checked_add(1).expect("capture output IDs exhausted");
                        staged.insert(id, file);
                        StagedCapture { size, effects, queue: Arc::downgrade(queue), id: Some(id) }
                    });
                reply.send(result);
            }
        } else {
            continue;
        }
        state = queue.state.lock().expect("capture writer queue");
    }
}

#[must_use = "keep the output handle alive to receive the capture result"]
pub struct CaptureOutput<T> {
    receiver: Option<mpsc::Receiver<Result<T, String>>>,
    cancelled: Arc<AtomicBool>,
}

impl<T> CaptureOutput<T> {
    pub fn try_take(&mut self) -> Option<Result<T, String>> {
        let result = match self.receiver.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("the capture output worker stopped".into()),
        };
        self.receiver = None;
        Some(result)
    }
}

impl<T> Drop for CaptureOutput<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

struct Reply<T> {
    sender: mpsc::Sender<Result<T, String>>,
    cancelled: Arc<AtomicBool>,
    wake: Box<dyn FnOnce() + Send>,
}

impl<T> Reply<T> {
    fn new(wake: impl FnOnce() + Send + 'static) -> (Self, CaptureOutput<T>) {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let reply = Self { sender, cancelled: cancelled.clone(), wake: Box::new(wake) };
        (reply, CaptureOutput { receiver: Some(receiver), cancelled })
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn send(self, result: Result<T, String>) {
        if !self.cancelled() && self.sender.send(result).is_ok() {
            let _ = catch_unwind(AssertUnwindSafe(self.wake));
        }
    }
}

/// A completed PNG awaiting host authorization to replace the destination.
pub struct StagedCapture {
    pub size: [u32; 2],
    pub effects: Vec<PaintReceipt>,
    queue: Weak<WriterQueue>,
    id: Option<u64>,
}

impl StagedCapture {
    /// Queue the atomic replacement on the same worker after validating the capture.
    pub fn publish(mut self, wake: impl FnOnce() + Send + 'static) -> CaptureOutput<PathBuf> {
        let (reply, output) = Reply::new(wake);
        let Some(queue) = self.queue.upgrade() else {
            reply.send(Err("the capture output worker stopped".into()));
            return output;
        };
        let mut state = queue.state.lock().expect("capture writer queue");
        if state.closed {
            drop(state);
            reply.send(Err("the capture output worker stopped".into()));
        } else {
            let id = self.id.take().expect("the capture owns its staged output");
            state.files.push_back(FileWork::Publish { id, reply });
            queue.ready.notify_one();
        }
        output
    }
}

impl Drop for StagedCapture {
    fn drop(&mut self) {
        if let Some(id) = self.id.take()
            && let Some(queue) = self.queue.upgrade()
        {
            let mut state = queue.state.lock().expect("capture writer queue");
            if !state.closed {
                state.files.push_back(FileWork::Discard(id));
                queue.ready.notify_one();
            }
        }
    }
}

struct StagedFile {
    requested: PathBuf,
    destination: PathBuf,
    file: NamedTempFile,
}

impl StagedFile {
    fn publish(self) -> Result<PathBuf, String> {
        self.file.persist(self.destination).map_err(|error| format!("{}: {}", self.requested.display(), error.error))?;
        Ok(self.requested)
    }
}

fn stage(path: &Path, prepare: Prepare) -> Result<(StagedFile, [u32; 2], Vec<PaintReceipt>), String> {
    let path_error = |error| format!("{}: {error}", path.display());
    let (destination, permissions) = capture_destination(path).map_err(path_error)?;
    let PaintedPicture { picture, effects } = prepare().map_err(|error| format!("{}: {error}", path.display()))?;
    let parent = destination.parent().expect("a capture destination has a parent directory");
    let mut file = tempfile::Builder::new().prefix(".octara-capture-").suffix(".png").tempfile_in(parent).map_err(path_error)?;
    write_png(&picture, BufWriter::new(file.as_file_mut())).map_err(|error| format!("{}: {error}", path.display()))?;
    if let Some(permissions) = permissions {
        file.as_file().set_permissions(permissions).map_err(path_error)?;
    }
    let file = StagedFile { requested: path.to_owned(), destination, file };
    Ok((file, [picture.width(), picture.height()], effects))
}

fn write_png(picture: &Picture, output: impl io::Write) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(output, picture.width(), picture.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(picture.rgba())?;
    writer.finish()
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
                Some(OpenOptions::new().write(true).open(&destination)?.metadata()?.permissions())
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
    use std::io::BufReader;
    use std::time::Duration;

    fn picture() -> Result<PaintedPicture, String> {
        Ok(PaintedPicture { picture: Picture::new(1, 2, vec![6, 41, 159, 70, 90, 36, 54, 255]).unwrap(), effects: vec![] })
    }

    #[test]
    fn staging_and_publication_preserve_straight_rgba_and_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capture.png");
        fs::write(&path, b"previous").unwrap();
        let writer = CaptureWriter::spawn().unwrap();
        let (wake, woke) = mpsc::channel();
        let mut output = writer.stage(&path, picture, move || wake.send(()).unwrap());
        woke.recv_timeout(Duration::from_secs(5)).unwrap();
        let staged = output.try_take().unwrap().unwrap();
        assert_eq!(staged.size, [1, 2]);
        assert_eq!(fs::read(&path).unwrap(), b"previous");

        let (wake, woke) = mpsc::channel();
        let mut output = staged.publish(move || wake.send(()).unwrap());
        woke.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(output.try_take().unwrap().unwrap(), path);
        writer.shutdown();
        let mut png = png::Decoder::new(BufReader::new(fs::File::open(path).unwrap())).read_info().unwrap();
        let mut bytes = vec![0; png.output_buffer_size().unwrap()];
        let image = png.next_frame(&mut bytes).unwrap();
        assert_eq!(&bytes[..image.buffer_size()], picture().unwrap().picture.rgba());
        assert_eq!(directory.path().read_dir().unwrap().count(), 1);
    }

    #[test]
    fn shutdown_cleans_staged_files_even_when_the_host_keeps_a_handle() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capture.png");
        let writer = CaptureWriter::spawn().unwrap();
        let (wake, woke) = mpsc::channel();
        let mut output = writer.stage(path, picture, move || wake.send(()).unwrap());
        woke.recv_timeout(Duration::from_secs(5)).unwrap();
        let staged = output.try_take().unwrap().unwrap();
        assert_eq!(directory.path().read_dir().unwrap().count(), 1);
        writer.shutdown();
        assert_eq!(directory.path().read_dir().unwrap().count(), 0);
        assert!(staged.publish(|| {}).try_take().unwrap().is_err());
    }
}
