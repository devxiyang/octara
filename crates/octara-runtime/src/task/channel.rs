use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

struct Channel<T> {
    queue: VecDeque<T>,
    waiter: Option<Waker>,
    senders: usize,
    open: bool,
}

/// A sender into a channel. It can be shared across threads when `T` is `Send`.
pub struct Sender<T> {
    channel: Arc<Mutex<Channel<T>>>,
}

impl<T> Sender<T> {
    /// Queues a value for the receiver, returning it if the receiver has closed.
    pub fn send(&self, value: T) -> Result<(), T> {
        let waiter = {
            let mut channel = self.channel.lock().expect("task channel");
            if !channel.open {
                return Err(value);
            }
            channel.queue.push_back(value);
            channel.waiter.take()
        };
        if let Some(waiter) = waiter {
            waiter.wake();
        }
        Ok(())
    }

    #[cfg(not(target_family = "wasm"))]
    pub(super) fn is_closed(&self) -> bool {
        !self.channel.lock().expect("task channel").open
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.channel.lock().expect("task channel").senders += 1;
        Self { channel: self.channel.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waiter = {
            let mut channel = self.channel.lock().expect("task channel");
            channel.senders -= 1;
            if channel.senders == 0 { channel.waiter.take() } else { None }
        };
        if let Some(waiter) = waiter {
            waiter.wake();
        }
    }
}

/// The single receiving end of a channel.
pub struct Receiver<T> {
    channel: Arc<Mutex<Channel<T>>>,
}

impl<T> Receiver<T> {
    /// Awaits the next value, or `None` when all senders and queued values are gone.
    pub fn recv(&mut self) -> Recv<'_, T> {
        Recv { receiver: self }
    }

    pub(super) fn poll_recv(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        let waiter = cx.waker().clone();
        let replaced = {
            let mut channel = self.channel.lock().expect("task channel");
            if let Some(value) = channel.queue.pop_front() {
                return Poll::Ready(Some(value));
            }
            if channel.senders == 0 {
                return Poll::Ready(None);
            }
            channel.waiter.replace(waiter)
        };
        drop(replaced);
        Poll::Pending
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let retired = {
            let mut channel = self.channel.lock().expect("task channel");
            channel.open = false;
            (std::mem::take(&mut channel.queue), channel.waiter.take())
        };
        drop(retired);
    }
}

/// Creates an unbounded channel with one receiver and clonable senders.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let channel = Arc::new(Mutex::new(Channel { queue: VecDeque::new(), waiter: None, senders: 1, open: true }));
    (Sender { channel: channel.clone() }, Receiver { channel })
}

/// An exclusive wait for the channel's next value.
pub struct Recv<'a, T> {
    receiver: &'a mut Receiver<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.receiver.poll_recv(cx)
    }
}

impl<T> Drop for Recv<'_, T> {
    fn drop(&mut self) {
        let retired = self.receiver.channel.lock().expect("task channel").waiter.take();
        drop(retired);
    }
}
