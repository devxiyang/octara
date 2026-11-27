use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use super::{TaskId, TaskState};

type TimerKey = (Duration, u64);

struct Timer {
    owner: Option<TaskId>,
    waker: Waker,
}

#[derive(Default)]
struct TimerQueue {
    entries: BTreeMap<TimerKey, Timer>,
    by_owner: BTreeMap<TaskId, BTreeSet<TimerKey>>,
}

impl TimerQueue {
    fn insert(&mut self, key: TimerKey, timer: Timer) -> Option<Timer> {
        if let Some(owner) = timer.owner {
            self.by_owner.entry(owner).or_default().insert(key);
        }
        self.entries.insert(key, timer)
    }

    fn remove(&mut self, key: TimerKey) -> Option<Timer> {
        let timer = self.entries.remove(&key)?;
        if let Some(owner) = timer.owner
            && let std::collections::btree_map::Entry::Occupied(mut entry) = self.by_owner.entry(owner)
        {
            let keys = entry.get_mut();
            keys.remove(&key);
            if keys.is_empty() {
                entry.remove();
            }
        }
        Some(timer)
    }

    fn next(&self) -> Option<TimerKey> {
        self.entries.first_key_value().map(|(key, _)| *key)
    }

    fn remove_owner(&mut self, owner: TaskId) -> Vec<Timer> {
        self.by_owner.remove(&owner).into_iter().flatten().filter_map(|key| self.entries.remove(&key)).collect()
    }
}

#[derive(Default)]
struct ClockState {
    now: Cell<Duration>,
    sequence: Cell<u64>,
    timers: RefCell<TimerQueue>,
}

/// A monotonic clock advanced by the host or by a test.
#[derive(Clone, Default)]
pub struct VirtualClock {
    state: Rc<ClockState>,
}

impl VirtualClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn now(&self) -> Duration {
        self.state.now.get()
    }

    /// Advances to a timestamp on this clock and wakes all due waits.
    #[track_caller]
    pub fn advance_to(&self, now: Duration) {
        assert!(now >= self.now(), "virtual time cannot go backwards");
        self.state.now.set(now);
        loop {
            let timer = {
                let mut timers = self.state.timers.borrow_mut();
                timers.next().filter(|(deadline, _)| *deadline <= now).and_then(|key| timers.remove(key))
            };
            let Some(timer) = timer else { break };
            timer.waker.wake();
        }
    }

    /// Advances by an elapsed duration.
    pub fn advance(&self, by: Duration) {
        self.advance_to(self.now().saturating_add(by));
    }

    /// Returns the earliest registered wait, if any.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.state.timers.borrow().next().map(|(deadline, _)| deadline)
    }

    pub fn sleep(&self, duration: Duration) -> Sleep {
        self.sleep_until(self.now().saturating_add(duration))
    }

    pub fn sleep_until(&self, deadline: Duration) -> Sleep {
        self.wait(deadline, None)
    }

    #[track_caller]
    pub fn interval(&self, period: Duration) -> Interval {
        self.periodic_wait(period, None)
    }

    pub(super) fn sleep_for_task(&self, deadline: Duration, owner: &Rc<TaskState>) -> Sleep {
        self.wait(deadline, Some(Rc::downgrade(owner)))
    }

    #[track_caller]
    pub(super) fn interval_for_task(&self, period: Duration, owner: &Rc<TaskState>) -> Interval {
        self.periodic_wait(period, Some(Rc::downgrade(owner)))
    }

    fn wait(&self, deadline: Duration, owner: Option<Weak<TaskState>>) -> Sleep {
        Sleep { clock: self.clone(), deadline, owner, registration: None }
    }

    #[track_caller]
    fn periodic_wait(&self, period: Duration, owner: Option<Weak<TaskState>>) -> Interval {
        assert!(!period.is_zero(), "interval period must be greater than zero");
        Interval { clock: self.clone(), period, owner, next: self.now().checked_add(period) }
    }

    pub(super) fn cancel_task(&self, owner: TaskId) {
        let retired = self.state.timers.borrow_mut().remove_owner(owner);
        drop(retired);
    }
}

/// A cancellable wait on a virtual deadline.
pub struct Sleep {
    clock: VirtualClock,
    deadline: Duration,
    owner: Option<Weak<TaskState>>,
    registration: Option<TimerKey>,
}

impl Sleep {
    fn unregister(&mut self) {
        if let Some(key) = self.registration.take() {
            let retired = self.clock.state.timers.borrow_mut().remove(key);
            drop(retired);
        }
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        self.unregister();
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.clock.now() >= self.deadline {
            self.unregister();
            return Poll::Ready(());
        }

        let waker = cx.waker().clone();
        // Cloning a custom waker may execute code that advances the clock.
        if self.clock.now() >= self.deadline {
            self.unregister();
            return Poll::Ready(());
        }

        let owner = match &self.owner {
            Some(owner) => match owner.upgrade().filter(|task| task.live.get()) {
                Some(task) => Some(task.info.id),
                None => return Poll::Pending,
            },
            None => None,
        };

        let key = self.registration.unwrap_or_else(|| {
            let sequence = self.clock.state.sequence.get();
            self.clock.state.sequence.set(sequence.checked_add(1).expect("timer sequence exhausted"));
            (self.deadline, sequence)
        });
        self.registration = Some(key);
        let retired = self.clock.state.timers.borrow_mut().insert(key, Timer { owner, waker });
        drop(retired);
        Poll::Pending
    }
}

/// Periodically waits on the virtual clock without drifting after delayed polls.
pub struct Interval {
    clock: VirtualClock,
    period: Duration,
    owner: Option<Weak<TaskState>>,
    next: Option<Duration>,
}

impl Interval {
    /// Returns the scheduled timestamp of the next tick.
    pub async fn tick(&mut self) -> Duration {
        let Some(deadline) = self.next else {
            return std::future::pending().await;
        };
        self.clock.wait(deadline, self.owner.clone()).await;
        self.next = deadline.checked_add(self.period);
        deadline
    }
}
