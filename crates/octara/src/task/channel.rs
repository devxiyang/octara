use super::*;

// ---- channels ----

struct ChannelState<T> {
    queue: VecDeque<T>,
    waker: Option<Waker>,
    receiver_alive: bool,
    senders: usize,
}

struct Chan<T> {
    state: Mutex<ChannelState<T>>,
}

/// Sends values into the app from any thread (an LLM's tokens, a socket's messages).
pub struct Sender<T> {
    chan: Arc<Chan<T>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.chan.state.lock().expect("chan").senders += 1;
        Sender { chan: self.chan.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut state = self.chan.state.lock().expect("chan");
            state.senders -= 1;
            if state.senders == 0 { state.waker.take() } else { None }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Sender<T> {
    /// Queues `value` for the receiver; `Err(value)` once the receiver (its scope) is gone.
    pub fn send(&self, value: T) -> Result<(), T> {
        let waker = {
            let mut state = self.chan.state.lock().expect("chan");
            if !state.receiver_alive {
                return Err(value);
            }
            state.queue.push_back(value);
            state.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }
}

/// Receives a channel's values on the UI thread: await them in a task (`recv`), or handle each
/// (`on`). Dropping it closes the channel and releases queued values and its waker immediately.
pub struct Receiver<T> {
    chan: Arc<Chan<T>>,
    name: Option<(String, Arc<NamedInterest>)>,
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let retired = {
            let mut state = self.chan.state.lock().expect("chan");
            state.receiver_alive = false;
            (std::mem::take(&mut state.queue), state.waker.take())
        };
        // User destructors and wakers must never run under the channel lock.
        drop(retired);
    }
}

/// A channel into the UI thread.
pub fn channel<T: Send + 'static>() -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Chan { state: Mutex::new(ChannelState { queue: VecDeque::new(), waker: None, receiver_alive: true, senders: 1 }) });
    (Sender { chan: chan.clone() }, Receiver { chan, name: None })
}

/// An exclusive wait for the channel's next value. Dropping it cancels the wait,
/// releases its waker, and leaves queued values available to the next wait.
pub struct Recv<'a, T> {
    rx: &'a mut Receiver<T>,
}

impl<T> Drop for Recv<'_, T> {
    fn drop(&mut self) {
        let waker = self.rx.chan.state.lock().expect("chan").waker.take();
        drop(waker);
    }
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let waker = cx.waker().clone();
        let mut state = self.rx.chan.state.lock().expect("chan");
        if let Some(value) = state.queue.pop_front() {
            return Poll::Ready(Some(value));
        }
        if state.senders == 0 {
            return Poll::Ready(None);
        }
        let old = state.waker.replace(waker);
        drop(state);
        drop(old);
        Poll::Pending
    }
}

impl<T: Send + 'static> Receiver<T> {
    /// The next value, or `None` when every sender is gone and nothing is left.
    /// Cancelling this wait releases its waker without consuming a queued value.
    /// The exclusive borrow prevents competing waits from overwriting each other's waker.
    ///
    /// ```compile_fail
    /// let (_, mut receiver) = octara::task::channel::<u32>();
    /// let first = receiver.recv();
    /// let second = receiver.recv(); // Only one receive may wait at a time.
    /// drop((first, second));
    /// ```
    pub fn recv(&mut self) -> Recv<'_, T> {
        Recv { rx: self }
    }

    /// Calls `f` with each value as it arrives, while the current scope lives.
    /// After 64 values it yields a poll to other ready tasks. This is task cooperation;
    /// the scheduler may continue polling in the same frame, without yielding to the OS.
    #[track_caller]
    pub fn on(mut self, f: impl Fn(T) + 'static) -> Task {
        let label = match &self.name {
            Some((n, _)) => format!("channel {n:?}"),
            None => "channel".to_owned(),
        };
        let fut = async move {
            let mut batch = 0;
            while let Some(v) = self.recv().await {
                f(v);
                batch += 1;
                if batch == 64 {
                    batch = 0;
                    let mut yielded = false;
                    std::future::poll_fn(|cx| {
                        if yielded {
                            Poll::Ready(())
                        } else {
                            yielded = true;
                            cx.waker().wake_by_ref();
                            Poll::Pending
                        }
                    })
                    .await;
                }
            }
        };
        spawn_with(fut, WorkKind::Subscription, label, Location::caller())
    }
}

impl<T: Send + std::str::FromStr + 'static> Receiver<T> {
    /// Names the channel so tests and agents can send it text (`App::send(name, payload)`,
    /// parsed with `FromStr`) while this receiver and its registration scope live.
    /// Replacing or renaming releases the old scope registration. Receiver closure retires
    /// the name at the next App boundary, including when it closes on another thread.
    #[track_caller]
    pub fn named(mut self, name: &str) -> Self {
        let s = sched();
        if !s.accepts_work(Location::caller()) {
            return self;
        }
        // Naming is App-scoped, but its interest token remains Send with the receiver.
        if !rt(|r| r.effect_allowed(Location::caller())) {
            return self;
        }
        self.name.take();
        s.retire_names();
        let interest = Arc::new(NamedInterest { scheduler: Arc::downgrade(&s.shared) });
        let receiver = Arc::downgrade(&interest);
        self.chan.state.lock().expect("chan").senders += 1;
        let sender = Sender { chan: self.chan.clone() };
        let key = name.to_owned();
        let inject: Injector = Rc::new(move |payload: &str| {
            let v = payload.parse::<T>().map_err(|_| format!("`{payload}` does not parse as the channel's type"))?;
            sender.send(v).map_err(|_| "the channel's receiver is gone".to_owned())
        });
        let weak = Rc::downgrade(&s);
        let expected = receiver.clone();
        let cleanup = rt(|r| {
            r.register_cleanup(Box::new(move || {
                if let Some(s) = weak.upgrade() {
                    let retired = {
                        let mut names = s.names.borrow_mut();
                        if names.get(&key).is_some_and(|entry| Weak::ptr_eq(&entry.receiver, &expected)) {
                            names.remove(&key)
                        } else {
                            None
                        }
                    };
                    drop(retired);
                }
            }))
        });
        let retired = s.names.borrow_mut().insert(name.to_owned(), NamedChannel { inject, receiver, _cleanup: cleanup });
        drop(retired);
        self.name = Some((name.to_owned(), interest));
        self
    }
}

#[cfg(test)]
mod tests;
