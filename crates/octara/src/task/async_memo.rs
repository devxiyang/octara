use super::*;

// ---- async derivations ----

/// The current async derivation state. Refresh and failure retain the last successful value.
#[derive(Clone, PartialEq, Debug)]
pub enum AsyncState<T, E> {
    /// Computing a new result, optionally retaining the last successful value.
    Pending { previous: Option<T> },
    /// The current attempt succeeded.
    Ready(T),
    /// The current attempt failed; the last successful value remains available.
    Failed { error: E, previous: Option<T> },
}

impl<T, E> AsyncState<T, E> {
    /// The last successful value, including while refreshing or after failure.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Pending { previous } | Self::Failed { previous, .. } => previous.as_ref(),
        }
    }
    /// The current failure. Refresh clears it before starting another attempt.
    pub fn error(&self) -> Option<&E> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
    /// Whether the derivation is waiting for its current attempt.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

/// Commit the new state while borrowed, then retire user values after invalidation.
/// `None` begins a new attempt; `Some` publishes its outcome.
fn transition<T: 'static, E: 'static>(state: Signal<AsyncState<T, E>>, outcome: Option<Result<T, E>>) {
    let mut retired = (None, None);
    state.update(|s| {
        let old = std::mem::replace(s, AsyncState::Pending { previous: None });
        let previous = match old {
            AsyncState::Ready(value) => Some(value),
            AsyncState::Pending { previous } => previous,
            AsyncState::Failed { error, previous } => {
                retired.1 = Some(error);
                previous
            }
        };
        *s = match outcome {
            None => AsyncState::Pending { previous },
            Some(Ok(value)) => {
                retired.0 = previous;
                AsyncState::Ready(value)
            }
            Some(Err(error)) => AsyncState::Failed { error, previous },
        };
    });
    drop(retired);
}

/// A scope-owned asynchronous derivation (see [`async_memo`]). Reads never block or suspend UI.
pub struct AsyncMemo<T: 'static, E: 'static = String> {
    state: Signal<AsyncState<T, E>>,
    refreshes: Signal<u64>,
}

impl<T, E> Clone for AsyncMemo<T, E> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, E> Copy for AsyncMemo<T, E> {}

impl<T: 'static, E: 'static> AsyncMemo<T, E> {
    /// Borrows the current state and tracks this read. Values and errors need not be Clone.
    pub fn with<R>(&self, f: impl FnOnce(&AsyncState<T, E>) -> R) -> R {
        self.state.with(f)
    }
    /// Clones the current state and tracks this read.
    pub fn get(&self) -> AsyncState<T, E>
    where
        T: Clone,
        E: Clone,
    {
        self.state.get()
    }
    /// Recomputes the same input, cancelling the previous attempt and clearing its error.
    pub fn refresh(&self) {
        self.refreshes.update(|n| *n += 1);
    }
}

/// Loads with `fetch(key())` now and again whenever the key changes (or on `refresh`): the
/// previous load is cancelled, so an old answer never overwrites a newer one; an unchanged key
/// loads nothing. Before publishing, the load rechecks its task and key/refresh input, including
/// changes made during its final poll. Only `key` is tracked; reads in `fetch` or its future do not subscribe.
/// Owned by the current scope.
#[track_caller]
pub fn async_memo<K, T, E, Fut>(key: impl Fn() -> K + 'static, fetch: impl Fn(K) -> Fut + 'static) -> AsyncMemo<T, E>
where
    K: Clone + PartialEq + 'static,
    T: 'static,
    E: 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
{
    let at = Location::caller();
    let state = Signal::scoped(AsyncState::Pending { previous: None });
    let refreshes = Signal::scoped(0u64);
    let request = memo(move || (key(), refreshes.get()));
    let running: Rc<RefCell<Option<Task>>> = Rc::default();
    let scope = rt(|r| r.current_scope());
    watch(
        move || request.get(),
        move |requested, _| {
            let previous = running.borrow_mut().take();
            if let Some(t) = previous {
                t.cancel();
            }
            transition(state, None);
            let requested = requested.clone();
            let load = fetch(requested.0.clone());
            let fut = async move {
                let r = load.await;
                // User polling can change the key/refresh input or retire this task.
                // Cancellation cannot interrupt an active poll, so validate at publication.
                let scheduler = sched();
                if !scheduler.current_is_live()
                    || untrack(|| request.with(|current| current != &requested))
                    // Comparing a user key can itself revoke the active task.
                    || !scheduler.current_is_live()
                {
                    return;
                }
                transition(state, Some(r));
            };
            // The load belongs to the async derivation's scope, not to whatever runs the watch.
            let task = rt(|r| r.with_frame(Ctx::Handler, None, Some(scope), || spawn_with(fut, WorkKind::Task, "async_memo".into(), at)));
            *running.borrow_mut() = Some(task);
        },
    );
    AsyncMemo { state, refreshes }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_releases_value_outside_state_borrow() {
        struct Value(Rc<dyn Fn()>);
        impl Drop for Value {
            fn drop(&mut self) {
                (self.0)();
            }
        }
        let app = crate::app::App::new(crate::platform::HeadlessPlatform::default());
        let handle = Rc::new(Cell::new(None::<AsyncMemo<Value, ()>>));
        let conflicted = Rc::new(Cell::new(false));
        let (h, c) = (handle.clone(), conflicted.clone());
        let memo = app.run(|| {
            async_memo(
                || (),
                move |_| {
                    let (h, c) = (h.clone(), c.clone());
                    async move {
                        Ok::<_, ()>(Value(Rc::new(move || {
                            if let Some(memo) = h.get() {
                                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| memo.with(|_| ())));
                                c.set(c.get() || result.is_err());
                            }
                        })))
                    }
                },
            )
        });
        handle.set(Some(memo));
        app.poll();
        app.run(|| memo.refresh());
        app.poll();
        handle.set(None);
        app.shutdown();
        assert!(!conflicted.get(), "internal async publication dropped the previous value during its state borrow");
    }
    #[test]
    fn refresh_retires_errors_and_success_retires_previous_values_after_commit() {
        struct Probe(Rc<dyn Fn()>);
        impl Drop for Probe {
            fn drop(&mut self) {
                (self.0)();
            }
        }
        let app = crate::app::App::new(crate::platform::HeadlessPlatform::default());
        let handle = Rc::new(Cell::new(None::<AsyncMemo<Probe, Probe>>));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let attempt = Rc::new(Cell::new(0));
        let (h, observed) = (handle.clone(), seen.clone());
        let memo = app.run(|| {
            async_memo(
                || (),
                move |_| {
                    let n = attempt.replace(attempt.get() + 1);
                    let (h, observed) = (h.clone(), observed.clone());
                    async move {
                        let probe = Probe(Rc::new(move || {
                            if let Some(memo) = h.get() {
                                memo.with(|state| {
                                    observed.borrow_mut().push(match state {
                                        AsyncState::Pending { previous: Some(_) } => "pending with previous",
                                        AsyncState::Ready(_) => "ready",
                                        _ => "unexpected",
                                    })
                                });
                            }
                        }));
                        if n == 1 { Err(probe) } else { Ok(probe) }
                    }
                },
            )
        });
        handle.set(Some(memo));
        app.poll();
        app.run(|| memo.refresh());
        app.poll();
        app.run(|| memo.with(|s| assert!(matches!(s, AsyncState::Failed { previous: Some(_), .. }))));
        assert!(seen.borrow().is_empty(), "a failure retains the successful value");
        app.run(|| memo.refresh());
        app.poll();
        assert_eq!(&*seen.borrow(), &["pending with previous", "ready"]);
        handle.set(None);
        app.shutdown();
    }
}
