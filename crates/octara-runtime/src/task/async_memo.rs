use std::future::Future;
use std::sync::Arc;

use crate::cx::{BuildCx, ReadCx, Readable, WriteCx};
use crate::graph::value::{ReadSignal, Signal};

use super::TaskCx;

/// The state of a scope-owned asynchronous derivation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AsyncState<T, E = String> {
    Pending { previous: Option<T> },
    Ready(T),
    Failed { error: E, previous: Option<T> },
}

impl<T, E> AsyncState<T, E> {
    /// Returns the last successful value, including during refresh or after failure.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Pending { previous } | Self::Failed { previous, .. } => previous.as_ref(),
        }
    }

    pub fn error(&self) -> Option<&E> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }
}

/// An asynchronous derivation whose state can be read as a reactive value.
pub struct AsyncMemo<T: 'static, E: 'static = String> {
    state: Signal<AsyncState<T, E>>,
    refreshes: Signal<u64>,
}

impl<T, E> Copy for AsyncMemo<T, E> {}

impl<T, E> Clone for AsyncMemo<T, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, E> AsyncMemo<T, E> {
    pub fn read_only(self) -> ReadSignal<AsyncState<T, E>> {
        self.state.read_only()
    }

    /// Restarts the current request, clearing any error and retaining any successful value.
    pub fn refresh(self, write: &mut WriteCx<'_>) {
        write.update(self.refreshes, |refreshes| *refreshes = refreshes.checked_add(1).expect("async memo refreshes exhausted"));
    }
}

impl<T, E> Readable<AsyncState<T, E>> for AsyncMemo<T, E> {
    type Prepared = ReadSignal<AsyncState<T, E>>;

    fn prepare(self, read: &mut ReadCx<'_>) -> Self::Prepared {
        self.read_only().prepare(read)
    }
}

impl<T, E> From<AsyncMemo<T, E>> for ReadSignal<AsyncState<T, E>> {
    fn from(memo: AsyncMemo<T, E>) -> Self {
        memo.read_only()
    }
}

impl BuildCx<'_> {
    /// Starts a scope-owned load on the first watch flush and whenever the input
    /// changes. The fetch receives the sampled input and this attempt's task context.
    #[track_caller]
    pub fn async_memo<K, T, E, Fut>(
        &mut self,
        input: impl FnMut(&mut ReadCx<'_>) -> K + 'static,
        fetch: impl Fn(K, TaskCx) -> Fut + 'static,
    ) -> AsyncMemo<T, E>
    where
        K: Clone + PartialEq + 'static,
        T: 'static,
        E: 'static,
        Fut: Future<Output = Result<T, E>> + 'static,
    {
        async_memo(self, input, fetch)
    }
}

/// Creates an asynchronous derivation in the building scope.
#[track_caller]
pub fn async_memo<K, T, E, Fut>(
    cx: &mut BuildCx<'_>,
    mut input: impl FnMut(&mut ReadCx<'_>) -> K + 'static,
    fetch: impl Fn(K, TaskCx) -> Fut + 'static,
) -> AsyncMemo<T, E>
where
    K: Clone + PartialEq + 'static,
    T: 'static,
    E: 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
{
    let state = cx.signal(AsyncState::Pending { previous: None });
    let refreshes = cx.signal(0u64);
    let request = cx.memo(move |read| (input(read), read.get(refreshes)));
    let mut running = None;
    let fetch = Arc::new(fetch);
    cx.watch(
        move |read| read.get(request),
        move |write, requested, _| {
            if let Some(previous) = running.take() {
                write.cancel_task(previous);
            }
            transition(write, state, None);
            let requested = requested.clone();
            let fetch = fetch.clone();
            running = Some(write.spawn("async memo", move |task| async move {
                let outcome = fetch(requested.0.clone(), task.clone()).await;
                task.write(|write| {
                    // The load may have changed its own input during this poll,
                    // before the watch has had a chance to retire the attempt.
                    if write.with(request, |current| current == &requested) {
                        transition(write, state, Some(outcome));
                    }
                });
            }));
        },
    );
    AsyncMemo { state, refreshes }
}

fn transition<T: 'static, E: 'static>(write: &mut WriteCx<'_>, state: Signal<AsyncState<T, E>>, outcome: Option<Result<T, E>>) {
    let mut retired = (None, None);
    write.update(state, |state| {
        let previous = match std::mem::replace(state, AsyncState::Pending { previous: None }) {
            AsyncState::Ready(value) => Some(value),
            AsyncState::Pending { previous } => previous,
            AsyncState::Failed { error, previous } => {
                retired.1 = Some(error);
                previous
            }
        };
        *state = match outcome {
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
