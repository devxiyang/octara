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
    fn take_value(&mut self) -> Option<T> {
        match std::mem::replace(self, Self::Pending { previous: None }) {
            Self::Ready(value) => Some(value),
            Self::Pending { previous } | Self::Failed { previous, .. } => previous,
        }
    }
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
            state.update(|s| *s = AsyncState::Pending { previous: s.take_value() });
            let requested = requested.clone();
            let load = fetch(requested.0.clone());
            let fut = async move {
                let r = load.await;
                // User polling can change the key/refresh input or retire this task.
                // Cancellation cannot interrupt an active poll, so validate at publication.
                let scheduler = sched();
                if !scheduler.current.get().is_some_and(|id| scheduler.alive(id))
                    || untrack(|| request.with(|current| current != &requested))
                {
                    return;
                }
                state.update(|s| {
                    let previous = s.take_value();
                    *s = match r {
                        Ok(value) => AsyncState::Ready(value),
                        Err(error) => AsyncState::Failed { error, previous },
                    };
                });
            };
            // The load belongs to the async derivation's scope, not to whatever runs the watch.
            let task = rt(|r| r.with_frame(Ctx::Handler, None, Some(scope), || spawn_with(fut, WorkKind::Task, "async_memo".into(), at)));
            *running.borrow_mut() = Some(task);
        },
    );
    AsyncMemo { state, refreshes }
}
