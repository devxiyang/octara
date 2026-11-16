use super::*;

// ---- background work ----

struct Oneshot<T> {
    value: Mutex<Option<T>>,
    waker: Mutex<Option<Waker>>,
}

impl<T> Oneshot<T> {
    /// The receiver alone owns result interest. A producer cannot prolong a cancelled wait.
    /// Return an unclaimed value so the producer can retire bookkeeping before dropping it.
    fn deliver(receiver: &Weak<Self>, value: T) -> Option<T> {
        let Some(slot) = receiver.upgrade() else { return Some(value) };
        *slot.value.lock().expect("oneshot") = Some(value);
        let waker = slot.waker.lock().expect("oneshot").take();
        if let Some(waker) = waker {
            waker.wake();
        }
        None
    }
}

/// The result of [`background`] work.
pub struct Background<T> {
    slot: Arc<Oneshot<std::thread::Result<T>>>,
}

impl<T> Future for Background<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut result = self.slot.value.lock().expect("oneshot").take();
        if result.is_none() {
            *self.slot.waker.lock().expect("oneshot") = Some(cx.waker().clone());
            // It may have landed between the check and storing the waker.
            result = self.slot.value.lock().expect("oneshot").take();
        }
        // Resume only after releasing the slot lock, on the awaiting task's thread.
        match result {
            Some(Ok(value)) => Poll::Ready(value),
            Some(Err(panic)) => std::panic::resume_unwind(panic),
            None => Poll::Pending,
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
#[track_caller]
pub fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Background<T> {
    let scheduler = sched();
    if !scheduler.accepts_work(Location::caller()) {
        panic!("[{}] background work requested after app shutdown", codes::WRITE_DISPOSED);
    }
    let slot = Arc::new(Oneshot { value: Mutex::new(None), waker: Mutex::new(None) });
    let shared = scheduler.shared.clone();
    *shared.inflight.lock().expect("inflight") += 1;
    let done = Arc::downgrade(&slot);
    let job: Job = Box::new(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        let unclaimed = Oneshot::deliver(&done, result);
        let mut n = shared.inflight.lock().expect("inflight");
        *n -= 1;
        if *n == 0 {
            shared.idle.notify_all();
        }
        drop(n);
        shared.notify();
        // User values can have destructors; completion accounting must already be retired.
        drop(unclaimed);
    });
    pool().lock().expect("pool").send(job).expect("background pool");
    Background { slot }
}

/// A value coming later from any thread (a platform's answer): the reply that sets it, and the
/// future that awaits it in a task.
pub(crate) fn later<T: Send + 'static>() -> (crate::platform::Reply<T>, impl Future<Output = T>) {
    let slot = Arc::new(Oneshot { value: Mutex::new(None), waker: Mutex::new(None) });
    let shared = Arc::downgrade(&sched().shared);
    let done = Arc::downgrade(&slot);
    let reply: crate::platform::Reply<T> = Box::new(move |v| {
        let unclaimed = Oneshot::deliver(&done, Ok(v));
        if unclaimed.is_none()
            && let Some(shared) = shared.upgrade()
        {
            shared.notify();
        }
    });
    (reply, Background { slot })
}
