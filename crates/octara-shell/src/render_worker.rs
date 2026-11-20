//! Transfers the device's renderer to one worker job and returns it with the outcome.

use octara_render::GpuRenderer;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::thread;

pub(crate) type Work<R> = Box<dyn FnOnce(&mut GpuRenderer) -> R + Send>;
type Job<R> = (GpuRenderer, Work<R>);
type Finished<R> = (GpuRenderer, thread::Result<R>);

/// The caller dispatches again only after the renderer returns. Dropping the endpoints lets
/// the worker exit after any current GPU work without blocking the caller.
pub(crate) struct RenderWorker<R> {
    jobs: SyncSender<Job<R>>,
    finished: Receiver<Finished<R>>,
}

impl<R: Send + 'static> RenderWorker<R> {
    pub(crate) fn spawn(wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (jobs, queued) = sync_channel::<Job<R>>(1);
        let (replies, finished) = sync_channel(1);
        thread::Builder::new().name("octara-render".into()).spawn(move || {
            // Wake after disconnecting so a controller without its renderer can detect failure.
            let replies = std::mem::DropGuard::new(replies, |sender| {
                drop(sender);
                wake();
            });
            while let Ok((mut renderer, work)) = queued.recv() {
                let result = catch_unwind(AssertUnwindSafe(|| work(&mut renderer)));
                if replies.try_send((renderer, result)).is_err() {
                    break;
                }
                wake();
            }
        })?;
        Ok(Self { jobs, finished })
    }

    /// Returns the renderer unchanged when the job cannot be dispatched.
    #[allow(clippy::result_large_err, reason = "dispatch returns the same owned renderer without boxing it")]
    pub(crate) fn try_start(&self, renderer: GpuRenderer, work: Work<R>) -> Result<(), GpuRenderer> {
        self.jobs.try_send((renderer, work)).map_err(|error| match error {
            TrySendError::Full((renderer, _)) | TrySendError::Disconnected((renderer, _)) => renderer,
        })
    }

    /// Delivers the renderer even when the job panicked. A disconnected worker may have lost
    /// an accepted job, so the caller must handle that independently of an empty reply slot.
    pub(crate) fn try_finish(&self) -> io::Result<Option<Finished<R>>> {
        match self.finished.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(io::Error::from(io::ErrorKind::BrokenPipe)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disconnected_worker_is_distinct_from_a_pending_reply() {
        let (jobs, _queued) = sync_channel(1);
        let (replies, finished) = sync_channel(1);
        let worker = RenderWorker::<()> { jobs, finished };
        assert!(matches!(worker.try_finish(), Ok(None)));
        drop(replies);
        assert_eq!(worker.try_finish().err().unwrap().kind(), io::ErrorKind::BrokenPipe);
    }
}
