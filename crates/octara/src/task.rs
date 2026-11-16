//! Async work owned by scopes: tasks, timers on the app's clock, channels from other threads,
//! background work, and async derivations with their state as signals.
//!
//! - A task (`spawn`) belongs to the scope it was spawned in and is dropped (cancelled) when the
//!   scope is disposed. Choose the app root with `App::current().run` before spawning work
//!   that must outlive its initiating UI scope. Tasks run on the UI thread and may write signals.
//! - Every result from another thread comes back one way: it wakes a task, which the runtime
//!   polls at a frame boundary. Each ready batch runs in spawn order; thread completion timing
//!   still determines which batch a result enters.
//! - Time is the app's clock (`App::advance`), never the wall clock: `sleep`, `interval` and
//!   `now` follow it, so tests decide when timers fire.
//! - Pending work is listed (`App::pending`), with the scope and source location of each.

mod async_memo;
mod background;
mod channel;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
mod time;

pub use async_memo::{AsyncMemo, AsyncState, async_memo};
pub(crate) use background::later;
pub use background::{Background, background};
pub use channel::{Receiver, Recv, Sender, channel};
pub use time::{Sleep, interval, now, sleep, sleep_until};
pub(crate) use time::{require_period, whole_ns};

use crate::diagnostics as codes;
use crate::reactive::graph::{Ctx, ScopeId, Signal, memo, rt, untrack, watch};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::panic::Location;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

// Maximum task poll attempts per outer App entry, shared by all its windows and nested calls.
const TASK_POLLS_PER_TURN: usize = 1024;

type BoxFuture = Pin<Box<dyn Future<Output = ()>>>;
/// Sends text, parsed, into a named channel.
type Injector = Rc<dyn Fn(&str) -> Result<(), String>>;

struct NamedInterest {
    scheduler: Weak<Shared>,
}

impl Drop for NamedInterest {
    fn drop(&mut self) {
        if let Some(shared) = self.scheduler.upgrade() {
            shared.notify();
        }
    }
}

struct NamedChannel {
    inject: Injector,
    receiver: Weak<NamedInterest>,
    _cleanup: Option<crate::reactive::graph::CleanupRegistration>,
}

/// A task's identity: slot and generation. Identity has no scheduling order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct TaskId(u64);

impl TaskId {
    fn new(slot: usize, generation: u32) -> TaskId {
        TaskId(((generation as u64) << 32) | slot as u64)
    }
    fn slot(self) -> usize {
        (self.0 & 0xffff_ffff) as usize
    }
}

/// What a piece of pending work is, for `App::pending`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorkKind {
    /// A task (`spawn`, a async derivation's load).
    Task,
    /// A long-lived listener (`interval`, a channel's `on`): not work that finishes.
    Subscription,
    /// A node's properties moving to their targets (`due`: when they come to rest).
    Motion,
    /// Reactive or window preparation work left unfinished by a bounded update attempt.
    /// No timer is implied; another explicit update boundary can attempt it again.
    Update,
}

/// One piece of pending work.
#[derive(Clone, PartialEq, Debug)]
pub struct PendingWork {
    pub kind: WorkKind,
    /// What it is: `task`, `async_memo`, `interval`, `channel "token"`, ….
    pub label: String,
    /// The scope that owns it (`app`, `app/window…/chat/…`).
    pub scope: String,
    /// Where it was started (`file:line`).
    pub at: String,
    /// When the timer it waits on is due, on the app's clock.
    pub due: Option<Duration>,
}

/// Ownership moves out of the table during polling. Cancellation revokes execution
/// immediately, but the polling guard still owns the future until that poll returns.
enum TaskState {
    Suspended(BoxFuture),
    Polling,
    Cancelled,
}

struct Entry {
    cleanup: Option<crate::reactive::graph::CleanupRegistration>,
    id: TaskId,
    spawn_order: u64,
    state: TaskState,
    scope: ScopeId,
    kind: WorkKind,
    label: String,
    at: &'static Location<'static>,
    /// The owner (window) it was spawned under, restored when it runs.
    owner: Option<crate::reactive::graph::Owner>,
}

/// What other threads can touch: the ready queue, the count of background jobs in flight, and
/// the shell's wake-up.
pub(crate) struct Shared {
    closed: AtomicBool,
    ready: Mutex<Vec<TaskId>>,
    inflight: Mutex<usize>,
    failures: Mutex<VecDeque<Box<dyn std::any::Any + Send>>>,
    idle: Condvar,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Shared {
    /// External completion has no UI stack to unwind through. Preserve its failure for
    /// the next App boundary; never retry a potentially failing host notification.
    fn complete(&self, f: impl FnOnce()) {
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            self.failures.lock().expect("background failures").push_back(panic);
        }
    }

    fn notify(&self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let wake = self.wake.lock().expect("wake").clone();
        if let Some(w) = wake.filter(|_| !self.closed.load(Ordering::Acquire)) {
            w();
        }
    }
}

struct TaskWaker {
    id: TaskId,
    shared: Weak<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(s) = self.shared.upgrade() {
            let mut ready = s.ready.lock().expect("ready");
            if s.closed.load(Ordering::Acquire) {
                return;
            }
            ready.push(self.id);
            drop(ready);
            s.notify();
        }
    }
}

/// Runs tasks for one app: its clock, tasks, timers and named channels.
pub(crate) struct Scheduler {
    /// The app's clock, in milliseconds since it started.
    now: Cell<f64>,
    slots: RefCell<Vec<Option<Entry>>>,
    generations: RefCell<Vec<u32>>,
    free: RefCell<Vec<usize>>,
    next_spawn_order: Cell<u64>,
    running: Cell<bool>,
    stopping: Cell<bool>,
    polls_left: Cell<usize>,
    deferred: RefCell<Vec<TaskId>>,
    shared: Arc<Shared>,
    /// Timers: (due, sequence, owner, notification). Ownership validates liveness;
    /// the polling Future's latest waker receives the deadline notification.
    timers: RefCell<Vec<(f64, u64, TaskId, Waker)>>,
    timer_seq: Cell<u64>,
    /// The task being polled (owns timers registered during its poll).
    current: Cell<Option<TaskId>>,
    /// Channels that take text by name (tests and agents send to them).
    names: RefCell<FxHashMap<String, NamedChannel>>,
}

/// An unwind must retain the ready tasks that have not been polled yet.
struct ReadyBatch<'a> {
    scheduler: &'a Scheduler,
    remaining: std::vec::IntoIter<TaskId>,
}

impl Drop for ReadyBatch<'_> {
    fn drop(&mut self) {
        if !self.scheduler.shared.closed.load(Ordering::Acquire) {
            self.scheduler.deferred.borrow_mut().extend(self.remaining.by_ref());
        }
    }
}

/// A future leaves its slot while it runs. Restore the polling context and retire that slot
/// even if user code unwinds; no borrow of the task table is held while the future drops.
struct PollingTask<'a> {
    scheduler: &'a Scheduler,
    id: TaskId,
    future: Option<BoxFuture>,
    previous: Option<TaskId>,
}

impl Drop for PollingTask<'_> {
    fn drop(&mut self) {
        self.scheduler.current.set(self.previous);
        if self.future.is_some() {
            let mut slots = self.scheduler.slots.borrow_mut();
            if slots.get(self.id.slot()).and_then(|e| e.as_ref()).is_some_and(|e| e.id == self.id) {
                let retired = slots[self.id.slot()].take();
                drop(slots);
                drop(retired);
                self.scheduler.free.borrow_mut().push(self.id.slot());
            }
        }
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler {
            now: Cell::new(0.0),
            slots: RefCell::default(),
            generations: RefCell::default(),
            free: RefCell::default(),
            next_spawn_order: Cell::new(0),
            running: Cell::new(false),
            stopping: Cell::new(false),
            polls_left: Cell::new(TASK_POLLS_PER_TURN),
            deferred: RefCell::default(),
            shared: Arc::new(Shared {
                closed: AtomicBool::new(false),
                ready: Mutex::default(),
                inflight: Mutex::new(0),
                failures: Mutex::default(),
                idle: Condvar::new(),
                wake: Mutex::new(None),
            }),
            timers: RefCell::default(),
            timer_seq: Cell::new(0),
            current: Cell::new(None),
            names: RefCell::default(),
        }
    }
}

fn sched() -> Rc<Scheduler> {
    crate::app::app_state().sched.clone()
}

impl Scheduler {
    pub(crate) fn now_ms(&self) -> f64 {
        self.now.get()
    }
    pub(crate) fn set_now_ms(&self, ms: f64) {
        self.now.set(whole_ns(ms));
    }
    pub(crate) fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        let retired = {
            let mut slot = self.shared.wake.lock().expect("wake");
            if self.shared.closed.load(Ordering::Acquire) { Some(wake) } else { slot.replace(wake) }
        };
        drop(retired);
    }

    fn accepts_work(&self, at: &'static Location<'static>) -> bool {
        if self.shared.closed.load(Ordering::Acquire) {
            rt(|rt| rt.diag(&codes::WRITE_DISPOSED, "cannot register work after app shutdown".into(), at));
            return false;
        }
        true
    }

    /// Stops scheduling app work; a future currently being polled is dropped when it returns.
    pub(crate) fn shutdown(&self) {
        if self.stopping.replace(true) {
            return;
        }
        let _stopping = std::mem::DropGuard::new(&self.stopping, |stopping| stopping.set(false));
        self.shared.closed.store(true, Ordering::Release);
        let wake = self.shared.wake.lock().expect("wake").take();
        drop(wake);
        let ids: Vec<_> = self.slots.borrow().iter().flatten().map(|entry| entry.id).collect();
        for id in ids {
            self.cancel(id);
        }
        self.deferred.borrow_mut().clear();
        let timers = std::mem::take(&mut *self.timers.borrow_mut());
        drop(timers);
        self.shared.ready.lock().expect("ready").clear();
        let failures = std::mem::take(&mut *self.shared.failures.lock().expect("background failures"));
        drop(failures);
        let names = std::mem::take(&mut *self.names.borrow_mut());
        drop(names);
    }

    fn insert(&self, future: BoxFuture, scope: ScopeId, kind: WorkKind, label: String, at: &'static Location<'static>) -> TaskId {
        let spawn_order = self.next_spawn_order.get();
        self.next_spawn_order.set(spawn_order.checked_add(1).expect("task spawn order exhausted"));
        let slot = self.free.borrow_mut().pop().unwrap_or_else(|| {
            u32::try_from(self.slots.borrow().len()).expect("task slots exhausted");
            self.slots.borrow_mut().push(None);
            self.generations.borrow_mut().push(0);
            self.slots.borrow().len() - 1
        });
        let generation = {
            let mut g = self.generations.borrow_mut();
            g[slot] = g[slot].checked_add(1).expect("task generation exhausted");
            g[slot]
        };
        let id = TaskId::new(slot, generation);
        let owner = rt(|r| r.owner());
        self.slots.borrow_mut()[slot] =
            Some(Entry { cleanup: None, id, spawn_order, state: TaskState::Suspended(future), scope, kind, label, at, owner });
        self.shared.ready.lock().expect("ready").push(id);
        id
    }

    fn alive(&self, id: TaskId) -> bool {
        self.slots.borrow().get(id.slot()).and_then(|e| e.as_ref()).is_some_and(|e| e.id == id && !matches!(e.state, TaskState::Cancelled))
    }

    /// Drops the task's future (and what it holds); a task being polled is dropped after.
    fn cancel(&self, id: TaskId) {
        let mut slots = self.slots.borrow_mut();
        let Some(Some(e)) = slots.get_mut(id.slot()) else { return };
        if e.id != id {
            return;
        }
        let previous = std::mem::replace(&mut e.state, TaskState::Cancelled);
        if let TaskState::Suspended(f) = previous {
            let retired = slots[id.slot()].take();
            drop(slots);
            drop(retired);
            self.free.borrow_mut().push(id.slot());
            drop(f);
        }
    }

    pub(crate) fn has_ready(&self) -> bool {
        !self.deferred.borrow().is_empty() || !self.shared.ready.lock().expect("ready").is_empty()
    }

    pub(crate) fn resume_background_failure(&self) {
        let failure = self.shared.failures.lock().expect("background failures").pop_front();
        if let Some(failure) = failure {
            std::panic::resume_unwind(failure);
        }
    }

    pub(crate) fn begin_turn(&self) {
        self.polls_left.set(TASK_POLLS_PER_TURN);
    }

    pub(crate) fn yielded(&self) -> bool {
        self.polls_left.get() == 0 && self.has_ready()
    }

    pub(crate) fn finish_turn(&self) {
        if self.yielded() {
            self.shared.notify();
        }
    }

    /// Polls ready batches in spawn order until empty or the turn allowance ends. Returns whether any ran.
    pub(crate) fn run_ready(&self) -> bool {
        self.resume_background_failure();
        // User polling and future destructors can reenter the App. Only the outer drain may
        // consume wakes: an active future is temporarily absent from its slot.
        if self.running.replace(true) {
            return false;
        }
        let _running = std::mem::DropGuard::new(&self.running, |running| running.set(false));
        self.retire_names();
        let mut ran = false;
        while self.polls_left.get() > 0 {
            // Finish a suspended batch before admitting new wakes. Otherwise an early task
            // waking itself could repeatedly displace the later tasks at the budget boundary.
            let mut batch = std::mem::take(&mut *self.deferred.borrow_mut());
            if batch.is_empty() {
                batch = std::mem::take(&mut *self.shared.ready.lock().expect("ready"));
                if batch.is_empty() {
                    break;
                }
                let slots = self.slots.borrow();
                batch.retain(|id| {
                    slots
                        .get(id.slot())
                        .and_then(Option::as_ref)
                        .is_some_and(|entry| entry.id == *id && !matches!(entry.state, TaskState::Cancelled))
                });
                batch.sort_unstable_by_key(|id| slots[id.slot()].as_ref().expect("live ready task").spawn_order);
                batch.dedup();
            }
            let mut batch = ReadyBatch { scheduler: self, remaining: batch.into_iter() };
            while self.polls_left.get() > 0 {
                if self.shared.closed.load(Ordering::Acquire) {
                    return ran;
                }
                let Some(id) = batch.remaining.next() else { break };
                self.polls_left.set(self.polls_left.get() - 1);
                ran |= self.poll(id);
            }
        }
        ran
    }

    fn poll(&self, id: TaskId) -> bool {
        let (future, scope, owner) = {
            let mut slots = self.slots.borrow_mut();
            let Some(Some(e)) = slots.get_mut(id.slot()) else { return false };
            if e.id != id || !matches!(e.state, TaskState::Suspended(_)) {
                return false;
            }
            let TaskState::Suspended(f) = std::mem::replace(&mut e.state, TaskState::Polling) else { unreachable!() };
            (f, e.scope, e.owner)
        };
        let waker = Waker::from(Arc::new(TaskWaker { id, shared: Arc::downgrade(&self.shared) }));
        let mut cx = Context::from_waker(&waker);
        let mut polling = PollingTask { scheduler: self, id, future: Some(future), previous: self.current.replace(Some(id)) };
        let done = rt(|r| {
            r.with_owner(owner, || {
                r.with_frame(Ctx::Handler, None, Some(Some(scope)), || {
                    polling.future.as_mut().expect("polling future").as_mut().poll(&mut cx).is_ready()
                })
            })
        });
        let mut slots = self.slots.borrow_mut();
        let cancelled = slots.get(id.slot()).and_then(|e| e.as_ref()).is_none_or(|e| e.id != id || matches!(e.state, TaskState::Cancelled));
        if !done
            && !cancelled
            && let Some(Some(e)) = slots.get_mut(id.slot())
        {
            e.state = TaskState::Suspended(polling.future.take().expect("polling future"));
        }
        drop(slots);
        true
    }

    fn add_timer(&self, due: f64, id: TaskId, waker: Waker) -> u64 {
        let seq = self.timer_seq.get().checked_add(1).expect("timer identity exhausted");
        self.timer_seq.set(seq);
        self.timers.borrow_mut().push((due, seq, id, waker));
        seq
    }

    /// The earliest timer of a live task.
    pub(crate) fn next_timer(&self) -> Option<f64> {
        let retired: Vec<_> = self.timers.borrow_mut().extract_if(.., |t| !self.alive(t.2)).collect();
        drop(retired);
        self.timers.borrow().iter().map(|t| t.0).min_by(f64::total_cmp)
    }

    /// Wakes the tasks whose timers are due at the clock's time.
    pub(crate) fn fire_due(&self) {
        let now = self.now.get();
        let mut due: Vec<_> = self.timers.borrow_mut().extract_if(.., |t| t.0 <= now).collect();
        due.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, _, id, waker) in due {
            if self.alive(id) {
                waker.wake();
            }
        }
    }

    /// Waits (real time, at most `limit`) until no background job is in flight.
    pub(crate) fn wait_background(&self, limit: Duration) -> bool {
        let guard = self.shared.inflight.lock().expect("inflight");
        let (guard, _) = self.shared.idle.wait_timeout_while(guard, limit, |n| *n > 0).expect("inflight");
        *guard == 0
    }

    pub(crate) fn idle(&self) -> bool {
        !self.has_ready()
            && *self.shared.inflight.lock().expect("inflight") == 0
            && self.shared.failures.lock().expect("background failures").is_empty()
    }

    pub(crate) fn pending(&self) -> Vec<PendingWork> {
        let timers = self.timers.borrow();
        let slots = self.slots.borrow();
        let mut out: Vec<PendingWork> = slots
            .iter()
            .flatten()
            .filter(|e| !matches!(e.state, TaskState::Cancelled))
            .map(|e| {
                let due = timers.iter().filter(|t| t.2 == e.id).map(|t| t.0).min_by(f64::total_cmp);
                PendingWork {
                    kind: e.kind,
                    label: e.label.clone(),
                    scope: rt(|r| r.scope_path(e.scope)),
                    at: format!("{}:{}", codes::source_file(e.at), e.at.line()),
                    due: due.map(|ms| Duration::from_secs_f64(ms / 1000.0)),
                }
            })
            .collect();
        if !self.shared.failures.lock().expect("background failures").is_empty() {
            out.push(PendingWork {
                kind: WorkKind::Update,
                label: "background completion panicked".into(),
                scope: "background".into(),
                at: String::new(),
                due: None,
            });
        }
        let inflight = *self.shared.inflight.lock().expect("inflight");
        if inflight > 0 {
            out.push(PendingWork {
                kind: WorkKind::Task,
                label: format!("{inflight} background job(s)"),
                scope: "background".into(),
                at: String::new(),
                due: None,
            });
        }
        out
    }

    pub(crate) fn retire_names(&self) {
        let retired: Vec<_> = self.names.borrow_mut().extract_if(|_, entry| entry.receiver.strong_count() == 0).collect();
        drop(retired);
    }

    /// Sends `payload` to the channel named `name`.
    pub(crate) fn send_named(&self, name: &str, payload: &str) -> Result<(), String> {
        self.retire_names();
        let f = self.names.borrow().get(name).map(|entry| entry.inject.clone()).ok_or_else(|| format!("no channel named `{name}`"))?;
        f(payload)
    }
}

// ---- tasks ----

/// A running task. The scope it was spawned in owns it: dropping the handle does nothing, and
/// the task is cancelled when that scope is disposed (or by [`Task::cancel`]).
#[derive(Clone)]
pub struct Task {
    id: Option<TaskId>,
    sched: Rc<Scheduler>,
}

impl Task {
    /// Stops the task: its future is dropped (and with it what it holds) at once.
    pub fn cancel(&self) {
        if let Some(id) = self.id {
            self.sched.cancel(id);
        }
    }
    pub fn is_finished(&self) -> bool {
        self.id.is_none_or(|id| !self.sched.alive(id))
    }
}

pub(crate) fn spawn_with(
    future: impl Future<Output = ()> + 'static,
    kind: WorkKind,
    label: String,
    at: &'static Location<'static>,
) -> Task {
    let s = sched();
    if !rt(|rt| rt.effect_allowed(at)) || !s.accepts_work(at) {
        return Task { id: None, sched: s };
    }
    let Some(scope) = rt(|r| r.current_scope()) else {
        rt(|rt| rt.diag(&codes::WRITE_DISPOSED, "cannot spawn a task without an app or component scope".into(), at));
        return Task { id: None, sched: s };
    };
    let id = s.insert(Box::pin(future), scope, kind, label, at);
    let weak = Rc::downgrade(&s);
    let cleanup = rt(|r| {
        r.register_cleanup(Box::new(move || {
            if let Some(s) = weak.upgrade() {
                s.cancel(id);
            }
        }))
    });
    s.slots.borrow_mut()[id.slot()].as_mut().expect("new task").cleanup = cleanup;
    Task { id: Some(id), sched: s }
}

/// Runs `future` on the UI thread, owned by the current scope: cancelled when the scope is
/// disposed. It may write signals; its first part runs at the end of the current frame.
#[track_caller]
pub fn spawn(future: impl Future<Output = ()> + 'static) -> Task {
    spawn_with(future, WorkKind::Task, "task".into(), Location::caller())
}
