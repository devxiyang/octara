use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, Weak, mpsc};
use std::task::{Context, Poll};

use super::scheduler::WakeInbox;
use super::{Receiver, channel};

pub(super) type Failure = Box<dyn std::any::Any + Send + 'static>;

type Job = Box<dyn FnOnce() + Send>;

fn workers() -> &'static mpsc::Sender<Job> {
    static WORKERS: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    WORKERS.get_or_init(|| {
        let (send, receive) = mpsc::channel::<Job>();
        let receive = Arc::new(Mutex::new(receive));
        for index in 0..4 {
            let receive = receive.clone();
            std::thread::Builder::new()
                .name(format!("octara-background-{index}"))
                .spawn(move || {
                    loop {
                        let job = receive.lock().expect("background work queue").recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .expect("background worker could not start");
        }
        send
    })
}

/// The result of native blocking work started by a task.
///
/// Dropping the wait discards the result. Work already running on the worker
/// can continue independently of the app.
pub struct Background<T> {
    receive: Receiver<std::thread::Result<T>>,
}

impl<T> Future for Background<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        match self.receive.poll_recv(cx) {
            Poll::Ready(Some(Ok(value))) => Poll::Ready(value),
            Poll::Ready(Some(Err(panic))) => resume_unwind(panic),
            Poll::Ready(None) => panic!("background future polled after completion"),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub(super) fn start<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, inbox: Weak<WakeInbox>) -> Background<T> {
    let (send, receive) = channel();
    workers()
        .send(Box::new(move || {
            // A panicking host wake or result destructor belongs to the app,
            // even if the task has already dropped its receiver.
            let completion = catch_unwind(AssertUnwindSafe(move || {
                if !send.is_closed() {
                    let outcome = catch_unwind(AssertUnwindSafe(work));
                    let _ = send.send(outcome);
                }
            }));
            if let Err(failure) = completion
                && let Some(inbox) = inbox.upgrade()
            {
                inbox.report_background_failure(failure);
            }
        }))
        .expect("background work queue closed");
    Background { receive }
}
