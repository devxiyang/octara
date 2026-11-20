//! The worker owns a Metal layer only while obtaining one drawable.

use crate::metal_frame::{MetalFrame, MetalLayer};
use crate::presentation::Acquisition;
use std::io;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

struct Request {
    token: Acquisition,
    layer: MetalLayer,
    delay: Duration,
}

pub(crate) struct Completion {
    pub(crate) token: Acquisition,
    pub(crate) started: Instant,
    pub(crate) finished: Instant,
    pub(crate) frame: Option<MetalFrame>,
}

/// A window's one outstanding worker request, including a result waiting for the host.
/// Dropping the endpoints lets the worker return on its own after any native wait.
pub(crate) struct AcquireWorker {
    requests: SyncSender<Request>,
    completions: Receiver<Completion>,
    pending: Option<Acquisition>,
}

impl AcquireWorker {
    pub(crate) fn spawn(wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (requests, work) = sync_channel::<Request>(1);
        let (replies, completions) = sync_channel(1);
        thread::Builder::new().name("octara-metal-acquire".into()).spawn(move || {
            // Wake after disconnecting so a failed worker can retire its outstanding token.
            let replies = std::mem::DropGuard::new(replies, |sender| {
                drop(sender);
                wake();
            });
            while let Ok(Request { token, layer, delay }) = work.recv() {
                let started = Instant::now();
                // The experimental delay can be interrupted when the host goes away.
                // A second request is admitted only after the first reply is consumed.
                match work.recv_timeout(delay) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                    Ok(_) => unreachable!("a second drawable request arrived before the first completed"),
                }
                let frame = layer.acquire();
                let finished = Instant::now();
                drop(layer);
                if replies.try_send(Completion { token, started, finished, frame }).is_err() {
                    break;
                }
                wake();
            }
        })?;
        Ok(Self { requests, completions, pending: None })
    }

    pub(crate) fn request(&mut self, token: Acquisition, layer: MetalLayer, delay: Duration) -> io::Result<()> {
        if self.pending.is_some() {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        }
        self.requests.try_send(Request { token, layer, delay }).map_err(|error| match error {
            TrySendError::Full(_) => io::Error::from(io::ErrorKind::WouldBlock),
            TrySendError::Disconnected(_) => io::Error::from(io::ErrorKind::BrokenPipe),
        })?;
        self.pending = Some(token);
        Ok(())
    }

    /// An error carries the outstanding token when the worker exits without a reply.
    pub(crate) fn take_ready(&mut self) -> Result<Option<Completion>, Acquisition> {
        match self.completions.try_recv() {
            Ok(completion) => {
                let pending = self.pending.take();
                debug_assert_eq!(pending, Some(completion.token));
                Ok(Some(completion))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => self.pending.take().map_or(Ok(None), Err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::{DrawableGeneration, Presentation};
    use octara::app::WindowId;

    fn pending_worker() -> (AcquireWorker, SyncSender<Completion>, Acquisition) {
        let mut presentation = Presentation::new(WindowId::from_number(1));
        let token = presentation.begin(DrawableGeneration { surface: 1, device: 1 }).unwrap();
        let (requests, _) = sync_channel(1);
        let (answers, completions) = sync_channel(1);
        (AcquireWorker { requests, completions, pending: Some(token) }, answers, token)
    }

    #[test]
    fn disconnected_worker_returns_its_outstanding_token_once() {
        let (mut worker, answers, token) = pending_worker();
        assert!(matches!(worker.take_ready(), Ok(None)));
        drop(answers);
        assert!(matches!(worker.take_ready(), Err(retired) if retired == token));
        assert!(matches!(worker.take_ready(), Ok(None)));
    }

    #[test]
    fn completed_timeout_is_delivered_even_if_worker_exits() {
        let (mut worker, answers, token) = pending_worker();
        let started = Instant::now();
        let finished = started + Duration::from_millis(10);
        assert!(answers.try_send(Completion { token, started, finished, frame: None }).is_ok());
        drop(answers);
        let completion = worker.take_ready().unwrap().unwrap();
        assert_eq!((completion.token, completion.started, completion.finished), (token, started, finished));
        assert!(completion.frame.is_none());
        assert!(matches!(worker.take_ready(), Ok(None)));
    }
}
