use super::*;

// ---- background work ----

/// Result interest is independent of the producer's physical lifetime. A producer
/// temporarily upgrading Weak must not keep a cancelled receiver logically open.
enum Delivery<T> {
    Waiting(Option<Waker>),
    Ready(T),
    Closed,
}

struct Oneshot<T> {
    state: Mutex<Delivery<T>>,
}

impl<T> Oneshot<T> {
    fn new() -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(Delivery::Waiting(None)) })
    }

    /// Commit under the lock; notify or destroy user objects only after unlocking.
    fn commit(receiver: &Weak<Self>, value: T) -> Result<Option<Waker>, T> {
        let Some(slot) = receiver.upgrade() else { return Err(value) };
        let previous = {
            let mut state = slot.state.lock().expect("oneshot");
            if !matches!(*state, Delivery::Waiting(_)) {
                return Err(value);
            }
            std::mem::replace(&mut *state, Delivery::Ready(value))
        };
        let Delivery::Waiting(waker) = previous else { unreachable!() };
        Ok(waker)
    }

    #[cfg(test)]
    fn deliver(receiver: &Weak<Self>, value: T) -> Option<T> {
        match Self::commit(receiver, value) {
            Ok(waker) => {
                if let Some(waker) = waker {
                    waker.wake();
                }
                None
            }
            Err(value) => Some(value),
        }
    }

    fn close(&self) {
        let retired = std::mem::replace(&mut *self.state.lock().expect("oneshot"), Delivery::Closed);
        drop(retired);
    }

    fn poll(&self, cx: &Context<'_>) -> Poll<T> {
        // Cloning a raw waker can execute arbitrary code too.
        let mut waker = Some(cx.waker().clone());
        let previous = {
            let mut state = self.state.lock().expect("oneshot");
            let next = if matches!(*state, Delivery::Waiting(_)) { Delivery::Waiting(waker.take()) } else { Delivery::Closed };
            std::mem::replace(&mut *state, next)
        };
        match previous {
            Delivery::Waiting(old) => {
                drop(old);
                Poll::Pending
            }
            Delivery::Ready(value) => Poll::Ready(value),
            Delivery::Closed => panic!("background future polled after completion"),
        }
    }
}

/// The result of [`background`] work.
pub struct Background<T> {
    slot: Arc<Oneshot<std::thread::Result<T>>>,
}

impl<T> Drop for Background<T> {
    fn drop(&mut self) {
        self.slot.close();
    }
}

impl<T> Future for Background<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        // Resume only after releasing the slot lock, on the awaiting task's thread.
        match self.slot.poll(cx) {
            Poll::Ready(Ok(value)) => Poll::Ready(value),
            Poll::Ready(Err(panic)) => std::panic::resume_unwind(panic),
            Poll::Pending => Poll::Pending,
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// A few worker threads shared by every app (Rune's `Latest` pool): blocking work runs there.
fn pool() -> &'static Mutex<std::sync::mpsc::Sender<Job>> {
    static POOL: std::sync::OnceLock<Mutex<std::sync::mpsc::Sender<Job>>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..4 {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("octara-background-{i}"))
                .spawn(move || {
                    loop {
                        let job = rx.lock().expect("pool").recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .expect("background thread");
        }
        Mutex::new(tx)
    })
}

/// Runs blocking `work` (a file read, a decode, an HTTP call) on a background thread; await the
/// result in a task. `App::settle` waits for it, so tests see it land.
/// A work panic is transported to the awaiting task and resumes there; it does not kill a
/// pool worker or become a business error. Dropping the future discards its eventual outcome
/// but cannot stop already-running blocking work.
/// A panic in completion notification or unclaimed-result destruction is resumed at the
/// next App execution boundary, even if the receiver was cancelled. It does not kill a
/// shared worker. A failing host wake is not retried; the host must enter the App again
/// to observe that failure (it is also visible as pending work).
#[track_caller]
pub fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Background<T> {
    let scheduler = sched();
    if !scheduler.accepts_work(Location::caller()) {
        panic!("[{}] background work requested after app shutdown", codes::WRITE_DISPOSED);
    }
    let slot = Oneshot::new();
    let shared = scheduler.shared.clone();
    *shared.inflight.lock().expect("inflight") += 1;
    let done = Arc::downgrade(&slot);
    let job: Job = Box::new(move || {
        // Accounting is owned even if committing the result unwinds. Retire it before
        // notifying or dropping an unclaimed result; both can execute user code.
        shared.complete(|| {
            let completion = std::mem::DropGuard::new(&shared, |shared| {
                let mut n = shared.inflight.lock().expect("inflight");
                *n -= 1;
                if *n == 0 {
                    shared.idle.notify_all();
                }
            });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            let delivery = Oneshot::commit(&done, result);
            drop(completion);
            match delivery {
                Ok(waker) => {
                    if let Some(waker) = waker {
                        waker.wake();
                    }
                }
                Err(unclaimed) => drop(unclaimed),
            }
            shared.notify();
        });
    });
    pool().lock().expect("pool").send(job).expect("background pool");
    Background { slot }
}

/// A value coming later from any thread (a platform's answer): the reply that sets it, and the
/// future that awaits it in a task.
pub(crate) fn later<T: Send + 'static>() -> (crate::platform::Reply<T>, impl Future<Output = T>) {
    let slot = Oneshot::new();
    let shared = Arc::downgrade(&sched().shared);
    let done = Arc::downgrade(&slot);
    let reply: crate::platform::Reply<T> = Box::new(move |v| {
        let Some(shared) = shared.upgrade() else { return };
        shared.complete(|| match Oneshot::commit(&done, Ok(v)) {
            Ok(waker) => {
                if let Some(waker) = waker {
                    waker.wake();
                }
                shared.notify();
            }
            Err(unclaimed) => drop(unclaimed),
        });
    });
    (reply, Background { slot })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Probe {
        slot: Weak<Oneshot<std::thread::Result<()>>>,
        locked: Arc<AtomicBool>,
    }
    #[allow(clippy::manual_noop_waker, reason = "custom Drop probes whether replacement holds the registration lock")]
    impl Wake for Probe {
        fn wake(self: Arc<Self>) {}
    }
    impl Drop for Probe {
        fn drop(&mut self) {
            if let Some(slot) = self.slot.upgrade() {
                self.locked.store(slot.state.try_lock().is_err(), Ordering::Release);
            }
        }
    }
    #[test]
    fn replacing_a_result_waker_drops_it_outside_the_lock() {
        let slot = Oneshot::new();
        let locked = Arc::new(AtomicBool::new(false));
        let old = Waker::from(Arc::new(Probe { slot: Arc::downgrade(&slot), locked: locked.clone() }));
        let mut future = Box::pin(Background { slot });
        assert!(future.as_mut().poll(&mut Context::from_waker(&old)).is_pending());
        drop(old);
        assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        assert!(!locked.load(Ordering::Acquire), "replaced waker destructor ran under its registration lock");
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::super::test_support::WakeCounter;
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn closing_revokes_delivery_even_while_a_producer_holds_the_slot() {
        let slot = Oneshot::new();
        let producer = slot.clone();
        let future = Background { slot };
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        assert!(producer.poll(&Context::from_waker(&wake.clone().into())).is_pending());
        drop(future);
        assert!(matches!(Oneshot::deliver(&Arc::downgrade(&producer), Ok(42)), Some(Ok(42))));
        assert_eq!(wake.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn delivery_notifies_only_the_current_waiter_and_is_consumed_once() {
        let slot = Oneshot::new();
        let first = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let last = Arc::new(WakeCounter(AtomicUsize::new(0)));
        assert!(slot.poll(&Context::from_waker(&first.clone().into())).is_pending());
        assert!(slot.poll(&Context::from_waker(&last.clone().into())).is_pending());
        assert_eq!(Oneshot::deliver(&Arc::downgrade(&slot), 42), None);
        assert_eq!(first.0.load(Ordering::Relaxed), 0);
        assert_eq!(last.0.load(Ordering::Relaxed), 1);
        assert_eq!(slot.poll(&Context::from_waker(Waker::noop())), Poll::Ready(42));
        assert_eq!(Oneshot::deliver(&Arc::downgrade(&slot), 43), Some(43));
    }

    #[test]
    fn concurrent_completion_and_registration_cannot_lose_a_notification() {
        for _ in 0..64 {
            let slot = Oneshot::new();
            let done = Arc::downgrade(&slot);
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let start = barrier.clone();
            let worker = std::thread::spawn(move || {
                start.wait();
                assert_eq!(Oneshot::deliver(&done, 42), None);
            });
            let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
            barrier.wait();
            let result = slot.poll(&Context::from_waker(&wake.clone().into()));
            worker.join().unwrap();
            if result.is_pending() {
                assert_eq!(wake.0.load(Ordering::Relaxed), 1);
                assert_eq!(slot.poll(&Context::from_waker(Waker::noop())), Poll::Ready(42));
            } else {
                assert_eq!(result, Poll::Ready(42));
            }
        }
    }

    #[test]
    fn retiring_a_ready_value_allows_destructor_reentry() {
        struct Value(Weak<Oneshot<Value>>, Arc<AtomicBool>);
        impl Drop for Value {
            fn drop(&mut self) {
                let slot = self.0.upgrade().unwrap();
                let state = slot.state.try_lock().expect("destructor must run outside the slot lock");
                assert!(matches!(*state, Delivery::Closed));
                self.1.store(true, Ordering::Release);
            }
        }
        let slot = Oneshot::new();
        let dropped = Arc::new(AtomicBool::new(false));
        assert!(Oneshot::deliver(&Arc::downgrade(&slot), Value(Arc::downgrade(&slot), dropped.clone())).is_none());
        slot.close();
        assert!(dropped.load(Ordering::Acquire));
        slot.close();
    }
}
