//! Scope-owned futures and time driven exclusively by the app's clock.

mod async_memo;
#[cfg(not(target_family = "wasm"))]
mod background;
mod channel;
mod clock;
mod scheduler;

pub use async_memo::{AsyncMemo, AsyncState, async_memo};
#[cfg(not(target_family = "wasm"))]
pub use background::Background;
pub use channel::{Receiver, Recv, Sender, channel};
pub use clock::{Interval, Sleep, VirtualClock};
pub use scheduler::{TaskId, TaskInfo, TaskScheduler, TaskSpawner};

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use crate::arena::ScopeId;
use crate::cx::{BuildCx, WriteCx};

/// The capabilities available to a scope-owned future.
///
/// Keep or clone this handle across awaits. A call to [`Self::write`] borrows
/// the app only for its synchronous callback.
#[derive(Clone)]
pub struct TaskCx {
    state: Arc<TaskState>,
    clock: VirtualClock,
}

impl TaskCx {
    /// Runs `f` during this task's poll, returning `None` when the task or its
    /// scope has retired, the task isn't being polled, or a write is in progress.
    ///
    /// ```compile_fail
    /// # use octara_runtime::TaskCx;
    /// fn leak(task: TaskCx) {
    ///     let _escaped = task.write(|w| w);
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// # use octara_runtime::TaskCx;
    /// async fn across_await(task: TaskCx) {
    ///     task.write(|w| async move {
    ///         std::future::ready(()).await;
    ///         w.scope()
    ///     }).unwrap().await;
    /// }
    /// ```
    pub fn write<R>(&self, f: impl FnOnce(&mut WriteCx<'_>) -> R) -> Option<R> {
        if !self.state.live.get() {
            return None;
        }

        let pointer = self.state.write.take()?;
        let _restore = RestoreWrite { slot: &self.state.write, pointer };
        // SAFETY: The scheduler installed the pointer to this poll's live WriteCx.
        // Taking the slot excludes all cloned handles during this call. The callback
        // accepts a fresh borrow and cannot return anything that borrows the context.
        let write = unsafe { &mut *pointer.cast::<WriteCx<'_>>().as_ptr() };
        write.world.is_scope_alive(self.state.info.scope).then(|| f(write))
    }

    /// The app's elapsed virtual time.
    pub fn now(&self) -> Duration {
        self.clock.now()
    }

    /// Waits until this app's clock advances by `duration`.
    pub fn sleep(&self, duration: Duration) -> Sleep {
        self.sleep_until(self.now().saturating_add(duration))
    }

    /// Waits until this app's clock reaches `deadline`.
    pub fn sleep_until(&self, deadline: Duration) -> Sleep {
        self.clock.sleep_for_task(deadline, &self.state)
    }

    /// Returns a periodic wait whose deadlines are measured from the current time.
    #[track_caller]
    pub fn interval(&self, period: Duration) -> Interval {
        self.clock.interval_for_task(period, &self.state)
    }

    /// Borrows the channel's single receiver to wait for the next value.
    pub fn recv<'a, T>(&self, receiver: &'a mut Receiver<T>) -> Recv<'a, T> {
        receiver.recv()
    }

    /// Runs blocking work on a native background worker.
    #[cfg(not(target_family = "wasm"))]
    pub fn background<T: Send + 'static>(&self, work: impl FnOnce() -> T + Send + 'static) -> Background<T> {
        assert!(self.state.live.get(), "retired task cannot start background work");
        background::start(work, self.state.inbox.clone())
    }
}

struct TaskState {
    info: TaskInfo,
    live: Cell<bool>,
    write: Cell<Option<NonNull<()>>>,
    #[cfg(not(target_family = "wasm"))]
    inbox: std::sync::Weak<scheduler::WakeInbox>,
}

impl TaskState {
    fn enter<'poll>(&'poll self, write: &'poll mut WriteCx<'_>) -> PollWrite<'poll> {
        assert!(self.write.get().is_none(), "task poll reentered");
        self.write.set(Some(NonNull::from(write).cast()));
        PollWrite { slot: &self.write, borrow: PhantomData }
    }
}

struct PollWrite<'poll> {
    slot: &'poll Cell<Option<NonNull<()>>>,
    borrow: PhantomData<&'poll mut ()>,
}

impl Drop for PollWrite<'_> {
    fn drop(&mut self) {
        self.slot.set(None);
    }
}

struct RestoreWrite<'a> {
    slot: &'a Cell<Option<NonNull<()>>>,
    pointer: NonNull<()>,
}

impl Drop for RestoreWrite<'_> {
    fn drop(&mut self) {
        self.slot.set(Some(self.pointer));
    }
}

impl BuildCx<'_> {
    /// Schedules a future owned by the building scope.
    #[track_caller]
    pub fn spawn<Fut>(&mut self, label: &'static str, factory: impl FnOnce(TaskCx) -> Fut + 'static) -> TaskId
    where
        Fut: Future<Output = ()> + 'static,
    {
        self.tasks.as_ref().expect("BuildCx has no task scheduler").spawn(self.scope, self.window, label, factory)
    }
}

impl WriteCx<'_> {
    /// The app's elapsed virtual time.
    pub fn now(&self) -> Duration {
        self.tasks.as_ref().expect("WriteCx has no task scheduler").now()
    }

    /// Schedules a future owned by the writing scope.
    #[track_caller]
    pub fn spawn<Fut>(&mut self, label: &'static str, factory: impl FnOnce(TaskCx) -> Fut + 'static) -> TaskId
    where
        Fut: Future<Output = ()> + 'static,
    {
        self.tasks.as_ref().expect("WriteCx has no task scheduler").spawn(self.scope, self.window, label, factory)
    }

    /// Schedules work in this scope or an ancestor that must outlive the initiating handler.
    #[track_caller]
    pub fn spawn_in<Fut>(&mut self, owner: ScopeId, label: &'static str, factory: impl FnOnce(TaskCx) -> Fut + 'static) -> TaskId
    where
        Fut: Future<Output = ()> + 'static,
    {
        assert!(self.world.is_scope_alive(owner) && self.world.scope_is_within(self.scope, owner), "task owner must be a live ancestor");
        self.tasks.as_ref().expect("WriteCx has no task scheduler").spawn(owner, self.window, label, factory)
    }

    /// Cancels work from this app. The running callback can finish, but no
    /// subsequent task write or poll is admitted.
    pub fn cancel_task(&mut self, id: TaskId) -> bool {
        self.tasks.as_ref().is_some_and(|tasks| tasks.cancel(id))
    }
}
