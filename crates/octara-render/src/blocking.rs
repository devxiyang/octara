use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake};

#[derive(Default)]
struct Notification {
    pending: Mutex<bool>,
    ready: Condvar,
}

impl Wake for Notification {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        *self.pending.lock().expect("future notification") = true;
        self.ready.notify_one();
    }
}

/// Drives a native GPU future whose producer can wake the current task.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let notification = Arc::new(Notification::default());
    let waker = notification.clone().into();
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
        let pending = notification.pending.lock().expect("future notification");
        *notification.ready.wait_while(pending, |pending| !*pending).expect("future notification") = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_a_wake_before_the_wait_begins() {
        let mut polls = 0;
        let count = block_on(std::future::poll_fn(|cx| {
            polls += 1;
            if polls == 1 {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(polls)
            }
        }));
        assert_eq!(count, 2);
    }
}
