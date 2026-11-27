//! Keyed notifications and scope-owned auto-dismiss timers.

use crate::id::Id;
use octara::prelude::*;
use std::{collections::BTreeMap, fmt, time::Duration};

/// Identifies one push, even when several toasts contain the same value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToastId(Id);

impl fmt::Display for ToastId {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(output)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToastOptions {
    pub live: Live,
    /// Time before dismissal. Use `None` to keep the toast until dismissed.
    pub duration: Option<Duration>,
}

impl Default for ToastOptions {
    fn default() -> Self {
        Self { live: Live::Polite, duration: Some(Duration::from_secs(5)) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast<T> {
    pub value: T,
    pub live: Live,
}

#[derive(Clone, Copy)]
enum Timer {
    Running { deadline: Duration, task: TaskId },
    Paused { remaining: Duration },
}

/// A scope-owned notification list. Start or resume timers from its owning scope
/// or from one of its descendants.
pub struct ToastQueue<T> {
    owner: ScopeId,
    items: StateList<ToastId, Toast<T>>,
    timers: Signal<BTreeMap<ToastId, Timer>>,
}

impl<T> Copy for ToastQueue<T> {}
impl<T> Clone for ToastQueue<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> ToastQueue<T> {
    pub fn new(build: &mut BuildCx<'_>) -> Self {
        Self { owner: build.scope(), items: build.list(), timers: build.signal(BTreeMap::new()) }
    }

    /// Reads or renders the notifications in push order using their stable keys.
    pub fn items(self) -> ReadList<ToastId, Toast<T>> {
        self.items.read_only()
    }

    pub fn push(self, write: &mut WriteCx<'_>, value: T, options: ToastOptions) -> ToastId {
        let id = ToastId(Id::new());
        if let Some(duration) = options.duration {
            self.start_timer(write, id, duration);
        }
        write.push_row(self.items, id, Toast { value, live: options.live });
        id
    }

    pub fn dismiss(self, write: &mut WriteCx<'_>, id: ToastId) -> bool {
        if let Some(Timer::Running { task, .. }) = write.update(self.timers, |timers| timers.remove(&id)) {
            write.cancel_task(task);
        }
        write.remove_row(self.items, &id).is_some()
    }

    /// Stops an active timer and remembers its remaining time.
    pub fn pause(self, write: &mut WriteCx<'_>, id: ToastId) -> bool {
        let Some(Timer::Running { deadline, task }) = write.with(self.timers, |timers| timers.get(&id).copied()) else {
            return false;
        };
        let remaining = deadline.saturating_sub(write.now());
        write.cancel_task(task);
        write.update(self.timers, |timers| timers.insert(id, Timer::Paused { remaining }));
        true
    }

    /// Restarts a paused timer with the time it had left.
    pub fn resume(self, write: &mut WriteCx<'_>, id: ToastId) -> bool {
        let Some(Timer::Paused { remaining }) = write.with(self.timers, |timers| timers.get(&id).copied()) else {
            return false;
        };
        self.start_timer(write, id, remaining);
        true
    }

    /// Applies the item's announcement setting to the caller's element.
    pub fn item<C>(self, build: &mut BuildCx<'_>, element: El<C>, id: ToastId) -> El<C> {
        let live = build.read(|read| read.with_row(self.items, &id, |toast| toast.map_or(Live::Off, |toast| toast.live)));
        element.live(live)
    }

    fn start_timer(self, write: &mut WriteCx<'_>, id: ToastId, duration: Duration) {
        let deadline = write.now().saturating_add(duration);
        let task = write.spawn_in(self.owner, "toast auto-dismiss", move |task| async move {
            task.sleep(deadline.saturating_sub(task.now())).await;
            task.write(|write| self.dismiss(write, id));
        });
        write.update(self.timers, |timers| timers.insert(id, Timer::Running { deadline, task }));
    }
}
