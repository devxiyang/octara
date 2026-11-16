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

struct Entry {
    cleanup: Option<crate::reactive::graph::CleanupRegistration>,
    id: TaskId,
    spawn_order: u64,
    future: Option<BoxFuture>,
    scope: ScopeId,
    kind: WorkKind,
    label: String,
    at: &'static Location<'static>,
    cancelled: bool,
    /// The owner (window) it was spawned under, restored when it runs.
    owner: Option<crate::reactive::graph::Owner>,
}

/// What other threads can touch: the ready queue, the count of background jobs in flight, and
/// the shell's wake-up.
pub(crate) struct Shared {
    closed: AtomicBool,
    ready: Mutex<Vec<TaskId>>,
    inflight: Mutex<usize>,
    idle: Condvar,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Shared {
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
    shared: Arc<Shared>,
    /// Timers: (due, sequence, task) — the task is woken when the clock reaches `due`.
    timers: RefCell<Vec<(f64, u64, TaskId)>>,
    timer_seq: Cell<u64>,
    /// The task being polled (timers started in it wake it).
    current: Cell<Option<TaskId>>,
    /// Channels that take text by name (tests and agents send to them).
    names: RefCell<FxHashMap<String, NamedChannel>>,
}

/// An unwind must retain the ready tasks that have not been polled yet.
struct ReadyBatch<'a> {
    shared: &'a Shared,
    remaining: std::vec::IntoIter<TaskId>,
}

impl Drop for ReadyBatch<'_> {
    fn drop(&mut self) {
        let mut ready = self.shared.ready.lock().expect("ready");
        if !self.shared.closed.load(Ordering::Acquire) {
            ready.extend(self.remaining.by_ref());
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
            shared: Arc::new(Shared {
                closed: AtomicBool::new(false),
                ready: Mutex::default(),
                inflight: Mutex::new(0),
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
        let mut slot = self.shared.wake.lock().expect("wake");
        if !self.shared.closed.load(Ordering::Acquire) {
            *slot = Some(wake);
        }
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
        if self.shared.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let wake = self.shared.wake.lock().expect("wake").take();
        drop(wake);
        let ids: Vec<_> = self.slots.borrow().iter().flatten().map(|entry| entry.id).collect();
        for id in ids {
            self.cancel(id);
        }
        self.timers.borrow_mut().clear();
        self.shared.ready.lock().expect("ready").clear();
        let names = std::mem::take(&mut *self.names.borrow_mut());
        drop(names);
    }

    fn insert(&self, future: BoxFuture, scope: ScopeId, kind: WorkKind, label: String, at: &'static Location<'static>) -> TaskId {
        let spawn_order = self.next_spawn_order.get();
        self.next_spawn_order.set(spawn_order.checked_add(1).expect("task spawn order exhausted"));
        let slot = self.free.borrow_mut().pop().unwrap_or_else(|| {
            self.slots.borrow_mut().push(None);
            self.generations.borrow_mut().push(0);
            self.slots.borrow().len() - 1
        });
        let generation = {
            let mut g = self.generations.borrow_mut();
            g[slot] += 1;
            g[slot]
        };
        let id = TaskId::new(slot, generation);
        let owner = rt(|r| r.owner());
        self.slots.borrow_mut()[slot] =
            Some(Entry { cleanup: None, id, spawn_order, future: Some(future), scope, kind, label, at, cancelled: false, owner });
        self.shared.ready.lock().expect("ready").push(id);
        id
    }

    fn alive(&self, id: TaskId) -> bool {
        self.slots.borrow().get(id.slot()).and_then(|e| e.as_ref()).is_some_and(|e| e.id == id && !e.cancelled)
    }

    /// Drops the task's future (and what it holds); a task being polled is dropped after.
    fn cancel(&self, id: TaskId) {
        let mut slots = self.slots.borrow_mut();
        let Some(Some(e)) = slots.get_mut(id.slot()) else { return };
        if e.id != id {
            return;
        }
        e.cancelled = true;
        if let Some(f) = e.future.take() {
            let retired = slots[id.slot()].take();
            drop(slots);
            drop(retired);
            self.free.borrow_mut().push(id.slot());
            drop(f);
        }
    }

    pub(crate) fn has_ready(&self) -> bool {
        !self.shared.ready.lock().expect("ready").is_empty()
    }

    /// Polls woken tasks until none is ready: each batch in spawn order. Returns whether any ran.
    pub(crate) fn run_ready(&self) -> bool {
        // User polling and future destructors can reenter the App. Only the outer drain may
        // consume wakes: an active future is temporarily absent from its slot.
        if self.running.replace(true) {
            return false;
        }
        let _running = std::mem::DropGuard::new(&self.running, |running| running.set(false));
        self.retire_names();
        let mut ran = false;
        for _ in 0..10_000 {
            let mut batch = std::mem::take(&mut *self.shared.ready.lock().expect("ready"));
            if batch.is_empty() {
                break;
            }
            {
                let slots = self.slots.borrow();
                batch.retain(|id| slots.get(id.slot()).and_then(Option::as_ref).is_some_and(|entry| entry.id == *id && !entry.cancelled));
                batch.sort_unstable_by_key(|id| slots[id.slot()].as_ref().expect("live ready task").spawn_order);
                batch.dedup();
            }
            let mut batch = ReadyBatch { shared: &self.shared, remaining: batch.into_iter() };
            for id in batch.remaining.by_ref() {
                if self.shared.closed.load(Ordering::Acquire) {
                    return ran;
                }
                ran |= self.poll(id);
            }
        }
        ran
    }

    fn poll(&self, id: TaskId) -> bool {
        let (future, scope, owner) = {
            let mut slots = self.slots.borrow_mut();
            let Some(Some(e)) = slots.get_mut(id.slot()) else { return false };
            if e.id != id || e.cancelled {
                return false;
            }
            let Some(f) = e.future.take() else { return false };
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
        let cancelled = slots.get(id.slot()).and_then(|e| e.as_ref()).is_none_or(|e| e.id != id || e.cancelled);
        if !done
            && !cancelled
            && let Some(Some(e)) = slots.get_mut(id.slot())
        {
            e.future = polling.future.take();
        }
        drop(slots);
        true
    }

    fn add_timer(&self, due: f64, id: TaskId) -> u64 {
        let seq = self.timer_seq.get().checked_add(1).expect("timer identity exhausted");
        self.timer_seq.set(seq);
        self.timers.borrow_mut().push((due, seq, id));
        seq
    }

    /// The earliest timer of a live task.
    pub(crate) fn next_timer(&self) -> Option<f64> {
        self.timers.borrow_mut().retain(|t| self.alive(t.2));
        self.timers.borrow().iter().map(|t| t.0).min_by(f64::total_cmp)
    }

    /// Wakes the tasks whose timers are due at the clock's time.
    pub(crate) fn fire_due(&self) {
        let now = self.now.get();
        let mut due: Vec<(f64, u64, TaskId)> = Vec::new();
        self.timers.borrow_mut().retain(|t| {
            if t.0 <= now {
                due.push(*t);
                false
            } else {
                true
            }
        });
        due.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut ready = self.shared.ready.lock().expect("ready");
        ready.extend(due.into_iter().map(|t| t.2));
    }

    /// Waits (real time, at most `limit`) until no background job is in flight.
    pub(crate) fn wait_background(&self, limit: Duration) -> bool {
        let guard = self.shared.inflight.lock().expect("inflight");
        let (guard, _) = self.shared.idle.wait_timeout_while(guard, limit, |n| *n > 0).expect("inflight");
        *guard == 0
    }

    pub(crate) fn idle(&self) -> bool {
        !self.has_ready() && *self.shared.inflight.lock().expect("inflight") == 0
    }

    pub(crate) fn pending(&self) -> Vec<PendingWork> {
        let timers = self.timers.borrow();
        let slots = self.slots.borrow();
        let mut out: Vec<PendingWork> = slots
            .iter()
            .flatten()
            .filter(|e| !e.cancelled)
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

// ---- time ----

/// The app's clock: time since it started, as the shell (or a test) advances it.
pub fn now() -> Duration {
    Duration::from_secs_f64(sched().now_ms() / 1000.0)
}

/// Completes on the creating App's clock. A pending wait must be polled by one of that
/// App's tasks; dropping it retires its timer.
pub struct Sleep {
    due: f64,
    scheduler: std::rc::Weak<Scheduler>,
    registration: Option<u64>,
}

impl Sleep {
    fn unregister(&mut self) {
        if let Some(registration) = self.registration.take()
            && let Some(scheduler) = self.scheduler.upgrade()
        {
            scheduler.timers.borrow_mut().retain(|timer| timer.1 != registration);
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
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        let s = sched();
        if !std::rc::Weak::ptr_eq(&self.scheduler, &Rc::downgrade(&s)) {
            rt(|r| r.diag(&codes::WRONG_RUNTIME, "Sleep belongs to another App".into(), Location::caller()));
            panic!("[{}] Sleep belongs to another App", codes::WRONG_RUNTIME);
        }
        if s.now_ms() >= self.due {
            self.unregister();
            return Poll::Ready(());
        }
        let id = s.current.get().expect("Sleep must be polled by an App task");
        if s.shared.closed.load(Ordering::Acquire) || !s.alive(id) {
            self.unregister();
            return Poll::Pending;
        }
        if let Some(registration) = self.registration {
            // A wait may move between tasks in the same App; retain its single timer.
            let mut timers = s.timers.borrow_mut();
            if let Some(timer) = timers.iter_mut().find(|timer| timer.1 == registration) {
                timer.2 = id;
                return Poll::Pending;
            }
        }
        self.registration = Some(s.add_timer(self.due, id));
        Poll::Pending
    }
}

/// Waits `d` on the creating App's clock. Cancelling the wait removes its timer.
pub fn sleep(d: Duration) -> Sleep {
    let s = sched();
    Sleep { due: whole_ns(s.now_ms() + d.as_secs_f64() * 1000.0), scheduler: Rc::downgrade(&s), registration: None }
}

/// Waits until the creating App's clock reads `t`.
pub fn sleep_until(t: Duration) -> Sleep {
    Sleep { due: whole_ns(t.as_secs_f64() * 1000.0), scheduler: Rc::downgrade(&sched()), registration: None }
}

#[track_caller]
pub(crate) fn require_period(period: Duration) {
    if period.is_zero() {
        let at = Location::caller();
        rt(|r| r.diag(&codes::INVALID_INTERVAL, "period must be greater than zero".into(), at));
        panic!("[{}] period must be greater than zero", codes::INVALID_INTERVAL);
    }
}

/// A time in milliseconds rounded to the clock's resolution, the nanosecond (`Duration`'s): the
/// clock and every due time go through it, so a due time handed out as a `Duration` and moved
/// to is reached exactly.
pub(crate) fn whole_ns(ms: f64) -> f64 {
    (ms * 1e6).round() / 1e6
}

/// Calls `f` every `period` on the app's clock while the current scope lives (ticks are counted
/// from now, so they do not drift).
///
/// # Panics
/// A zero period is rejected with INVALID_INTERVAL before registering work.
#[track_caller]
pub fn interval(period: Duration, f: impl Fn() + 'static) -> Task {
    require_period(period);
    let start = now();
    let fut = async move {
        let mut due = start + period;
        loop {
            sleep_until(due).await;
            f();
            due += period;
        }
    };
    spawn_with(fut, WorkKind::Subscription, format!("interval {period:?}"), Location::caller())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::platform::{HeadlessPlatform, WindowOptions};
    use crate::reactive::state;
    use crate::view::{Styled, el};
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn nested_app_poll_preserves_the_active_tasks_wake() {
        let app = App::new(HeadlessPlatform::default());
        let nested = app.clone();
        let polls = Rc::new(Cell::new(0));
        let count = polls.clone();
        app.run(|| {
            spawn(std::future::poll_fn(move |cx| {
                count.set(count.get() + 1);
                if count.get() == 1 {
                    cx.waker().wake_by_ref();
                    nested.poll();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            }));
        });
        app.poll();
        assert_eq!(polls.get(), 2, "nested polling must leave the wake for the outer scheduler");
        assert!(app.pending().is_empty());
        app.shutdown();
    }

    #[test]
    fn reused_slots_do_not_reorder_ready_tasks() {
        let app = App::new(HeadlessPlatform::default());
        let log = Rc::new(RefCell::new(Vec::new()));
        let wakes = Rc::new(RefCell::new(Vec::new()));
        let ready = Rc::new(Cell::new(false));
        app.run(|| {
            // Reusing slot zero must not make the next task younger than a fresh slot.
            for _ in 0..4 {
                spawn(std::future::pending()).cancel();
            }
            for name in ["older", "newer"] {
                let (log, wakes, ready) = (log.clone(), wakes.clone(), ready.clone());
                spawn(std::future::poll_fn(move |cx| {
                    if ready.get() {
                        log.borrow_mut().push(name);
                        Poll::Ready(())
                    } else {
                        wakes.borrow_mut().push((name, cx.waker().clone()));
                        Poll::Pending
                    }
                }));
            }
        });
        app.poll();
        ready.set(true);
        let wakes = wakes.borrow();
        for name in ["newer", "older"] {
            let waker = &wakes.iter().find(|(n, _)| *n == name).unwrap().1;
            waker.wake_by_ref();
            waker.wake_by_ref();
        }
        app.poll();
        assert_eq!(*log.borrow(), ["older", "newer"], "arrival order and slot generation must not override spawn order");
        assert!(app.pending().is_empty());
        app.shutdown();
    }

    #[test]
    fn creation_order_keeps_older_cancellation_ahead_of_newer_work_after_slot_reuse() {
        let app = App::new(HeadlessPlatform::default());
        let younger = Rc::new(RefCell::new(None::<Task>));
        let ran = Rc::new(Cell::new(false));
        app.run(|| {
            for _ in 0..4 {
                spawn(std::future::pending()).cancel();
            }
            let target = younger.clone();
            spawn(async move {
                target.borrow().as_ref().unwrap().cancel();
            });
            let ran = ran.clone();
            *younger.borrow_mut() = Some(spawn(async move {
                ran.set(true);
            }));
        });
        app.poll();
        assert!(!ran.get(), "the earlier cancellation must win even when its slot was reused");
        assert!(app.pending().is_empty());
        app.shutdown();
    }

    #[test]
    fn a_retired_waker_cannot_poll_the_task_reusing_its_slot() {
        let app = App::new(HeadlessPlatform::default());
        let saved = Rc::new(RefCell::new(None::<Waker>));
        let capture = saved.clone();
        let old = app.run(|| {
            spawn(std::future::poll_fn(move |cx| {
                *capture.borrow_mut() = Some(cx.waker().clone());
                Poll::<()>::Pending
            }))
        });
        app.poll();
        old.cancel();
        let polls = Rc::new(Cell::new(0));
        let capture = polls.clone();
        let replacement = app.run(|| {
            spawn(std::future::poll_fn(move |_| {
                capture.set(capture.get() + 1);
                Poll::<()>::Pending
            }))
        });
        assert_eq!(old.id.unwrap().slot(), replacement.id.unwrap().slot());
        app.poll();
        assert_eq!(polls.get(), 1);
        saved.borrow().as_ref().unwrap().wake_by_ref();
        app.poll();
        assert_eq!(polls.get(), 1, "filter stale identity before looking up creation order");
        app.shutdown();
    }

    #[test]
    fn a_sleep_has_one_timer_and_cancellation_removes_it_immediately() {
        let app = App::new(HeadlessPlatform::default());
        let (task, scheduler) = app.run(|| {
            let mut wait = Box::pin(sleep(Duration::from_secs(3600)));
            let mut polls = 0;
            let task = spawn(std::future::poll_fn(move |cx| {
                assert!(wait.as_mut().poll(cx).is_pending());
                polls += 1;
                if polls < 32 {
                    cx.waker().wake_by_ref();
                }
                Poll::<()>::Pending
            }));
            (task, sched())
        });
        app.poll();
        assert_eq!(scheduler.timers.borrow().len(), 1);
        task.cancel();
        assert!(scheduler.timers.borrow().is_empty());
        assert!(app.next_deadline().is_none());
        app.shutdown();
    }

    #[test]
    fn dropping_one_wait_preserves_other_waits_of_the_same_task() {
        let app = App::new(HeadlessPlatform::default());
        let completed = Rc::new(Cell::new(false));
        let out = completed.clone();
        let scheduler = app.run(|| {
            spawn(async move {
                let mut early = Box::pin(sleep(Duration::from_millis(10)));
                let mut later = Box::pin(sleep(Duration::from_millis(20)));
                std::future::poll_fn(|cx| {
                    assert!(early.as_mut().poll(cx).is_pending());
                    assert!(later.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
                drop(early);
                later.await;
                out.set(true);
            });
            sched()
        });
        app.poll();
        assert_eq!(scheduler.timers.borrow().len(), 1);
        assert_eq!(app.next_deadline(), Some(Duration::from_millis(20)));
        app.advance(Duration::from_millis(10));
        assert!(!completed.get());
        app.advance(Duration::from_millis(10));
        assert!(completed.get());
        assert!(scheduler.timers.borrow().is_empty());
        app.shutdown();
    }

    #[test]
    fn zero_periods_are_rejected_before_registration() {
        let app = App::new(HeadlessPlatform::default());
        app.run(|| {
            let scheduler = sched();
            for operation in [0, 1] {
                let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if operation == 0 {
                        interval(Duration::ZERO, || panic!("must not execute"));
                    } else {
                        crate::view::clock(Duration::ZERO);
                    }
                }));
                assert!(failure.is_err());
            }
            assert!(scheduler.pending().is_empty());
            rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0));
        });
        assert!(!app.is_failed(), "validation was caught before crossing the App boundary");
        assert!(app.diagnostics().iter().any(|d| d.code == "INVALID_INTERVAL"));
        app.shutdown();
    }

    #[test]
    fn a_sleep_rejects_a_different_apps_clock() {
        let first = App::new(HeadlessPlatform::default());
        let second = App::new(HeadlessPlatform::default());
        let wait = first.run(|| sleep(Duration::from_millis(10)));
        second.advance(Duration::from_secs(1));
        second.run(|| {
            spawn(wait);
        });
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| second.poll())).is_err());
        assert!(second.is_failed());
        assert!(!first.is_failed());
        first.shutdown();
        second.shutdown();
    }

    #[test]
    fn channel_close_releases_payloads_and_waker_while_senders_remain() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let (tx, mut rx) = channel::<DroppedResult>();
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut receive = rx.recv();
        assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
        drop(waker);
        assert_eq!(Arc::strong_count(&wake), 2);
        drop(receive);
        drop(rx);
        assert_eq!(Arc::strong_count(&wake), 1);
        assert_eq!(wake.0.load(Ordering::Relaxed), 0);
        drop(tx.send(DroppedResult(dropped.clone())).err().unwrap());
        assert_eq!(dropped.load(Ordering::Relaxed), 1);

        let (tx, rx) = channel::<DroppedResult>();
        for _ in 0..32 {
            assert!(tx.send(DroppedResult(dropped.clone())).is_ok());
        }
        drop(rx);
        assert_eq!(dropped.load(Ordering::Relaxed), 33);
        assert!(tx.chan.state.lock().unwrap().queue.is_empty());
    }

    #[test]
    fn cancelling_a_receive_releases_its_waker_and_preserves_fifo_for_the_next_wait() {
        let (tx, mut rx) = channel::<u32>();
        let old = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = Waker::from(old.clone());
        let mut receive = rx.recv();
        assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
        drop(waker);
        assert_eq!(Arc::strong_count(&old), 2);
        drop(receive);
        assert_eq!(Arc::strong_count(&old), 1);
        tx.send(7).unwrap();
        tx.send(8).unwrap();
        assert_eq!(old.0.load(Ordering::Relaxed), 0);
        assert_eq!(Pin::new(&mut rx.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Some(7)));
        assert_eq!(Pin::new(&mut rx.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Some(8)));

        let next = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = Waker::from(next.clone());
        let mut receive = rx.recv();
        assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
        drop(tx);
        assert_eq!(next.0.load(Ordering::Relaxed), 1, "last sender wakes the current waiter");
        assert_eq!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)), Poll::Ready(None));
    }

    #[test]
    fn cancelling_after_delivery_keeps_the_unconsumed_message() {
        let (tx, mut rx) = channel::<u32>();
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let mut receive = rx.recv();
        assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
        tx.send(42).unwrap();
        assert_eq!(wake.0.load(Ordering::Relaxed), 1);
        drop(receive);
        assert_eq!(Pin::new(&mut rx.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Some(42)));
    }

    #[test]
    fn send_racing_close_leaves_no_queued_values() {
        let dropped = Arc::new(AtomicUsize::new(0));
        for _ in 0..32 {
            let (tx, rx) = channel::<DroppedResult>();
            let gate = Arc::new(std::sync::Barrier::new(2));
            std::thread::scope(|scope| {
                let gate2 = gate.clone();
                let dropped = dropped.clone();
                let tx = &tx;
                scope.spawn(move || {
                    gate2.wait();
                    for _ in 0..32 {
                        drop(tx.send(DroppedResult(dropped.clone())));
                    }
                });
                gate.wait();
                drop(rx);
            });
            assert!(tx.chan.state.lock().unwrap().queue.is_empty());
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 32 * 32);
    }

    #[test]
    fn named_channel_replacement_and_cross_thread_close_release_registrations() {
        let app = App::new(HeadlessPlatform::default());
        app.run(|| {
            let mut receivers = Vec::new();
            for _ in 0..32 {
                receivers.push(channel::<u32>().1.named("same"));
                assert_eq!(sched().names.borrow().len(), 1);
                rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 1));
            }
            // Retiring an old receiver must not unregister its replacement.
            let last = receivers.pop().unwrap();
            drop(receivers);
            sched().retire_names();
            assert!(sched().send_named("same", "7").is_ok());
            assert_eq!(last.chan.state.lock().unwrap().queue.pop_front(), Some(7));
            std::thread::spawn(move || drop(last)).join().unwrap();
        });
        app.run(|| {
            assert!(sched().names.borrow().is_empty());
            rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0));
        });
        app.shutdown();
    }

    #[test]
    fn renaming_and_scope_exit_release_the_named_sender_even_if_the_receiver_survives() {
        let app = App::new(HeadlessPlatform::default());
        app.run(|| {
            let scope = rt(|r| r.create_scope(r.current_scope(), "named"));
            let mut receiver = rt(|r| r.with_scope(Some(scope), || channel::<u32>().1.named("old").named("new")));
            assert!(sched().send_named("old", "1").is_err());
            rt(|r| assert_eq!(r.cleanup_count(scope), 1));
            rt(|r| r.dispose_scope(scope));
            assert!(sched().names.borrow().is_empty());
            assert_eq!(Pin::new(&mut receiver.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(None));
        });
        app.shutdown();
    }

    #[test]
    fn closing_a_channel_drops_user_values_outside_its_lock() {
        struct Reentrant(Sender<Reentrant>, Arc<AtomicUsize>);
        impl Drop for Reentrant {
            fn drop(&mut self) {
                let state = self.0.chan.state.try_lock().expect("user destructor must be free to reenter the channel");
                assert!(!state.receiver_alive);
                self.1.fetch_add(1, Ordering::Relaxed);
            }
        }
        let (tx, rx) = channel::<Reentrant>();
        let dropped = Arc::new(AtomicUsize::new(0));
        assert!(tx.send(Reentrant(tx.clone(), dropped.clone())).is_ok());
        drop(rx);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(tx.chan.state.lock().unwrap().queue.is_empty());
    }

    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    struct DroppedResult(Arc<AtomicUsize>);

    impl Drop for DroppedResult {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn dropping_a_background_wait_releases_its_waker_before_the_worker_finishes() {
        let app = App::new(HeadlessPlatform::default());
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        let dropped = Arc::new(AtomicUsize::new(0));
        let output = dropped.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let (finished, discarded) = std::sync::mpsc::channel::<()>();
        let (mut future, scheduler) = app.run(|| {
            (
                Box::pin(background(move || {
                    wait.recv().unwrap();
                    (DroppedResult(output), finished)
                })),
                sched(),
            )
        });
        assert!(future.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        drop(waker);
        assert_eq!(Arc::strong_count(&wake), 2, "a pending receiver retains its task waker");
        drop(future);
        assert_eq!(Arc::strong_count(&wake), 1, "the blocked producer must not retain cancelled interest");
        release.send(()).unwrap();
        assert!(scheduler.wait_background(Duration::from_secs(5)));
        assert!(
            matches!(discarded.recv_timeout(Duration::from_secs(5)), Err(std::sync::mpsc::RecvTimeoutError::Disconnected)),
            "the unclaimed result must be dropped after completion accounting"
        );
        assert_eq!(wake.0.load(Ordering::Relaxed), 0, "late completion does not wake a retired receiver");
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(app.pending().is_empty());
        assert!(!app.is_failed());
        app.shutdown();
    }

    #[test]
    fn a_retained_platform_reply_does_not_retain_a_cancelled_receiver() {
        let app = App::new(HeadlessPlatform::default());
        let shared = app.run(|| Arc::downgrade(&sched().shared));
        let (reply, future) = app.run(later::<DroppedResult>);
        let mut future = Box::pin(future);
        let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
        let waker = Waker::from(wake.clone());
        assert!(future.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
        drop(waker);
        assert_eq!(Arc::strong_count(&wake), 2);
        drop(future);
        assert_eq!(Arc::strong_count(&wake), 1, "an unanswered platform callback does not pin the task waker");
        app.shutdown();
        drop(app);
        assert!(shared.upgrade().is_none(), "a retained platform callback must not prolong the retired scheduler");
        let dropped = Arc::new(AtomicUsize::new(0));
        reply(DroppedResult(dropped.clone()));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(wake.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn background_panics_reach_the_owner_without_exhausting_the_shared_pool() {
        for attempt in 0..6 {
            let app = App::new(HeadlessPlatform::default());
            let (job, scheduler) = app.run(|| (background(move || std::panic::panic_any(attempt)), sched()));
            assert!(scheduler.wait_background(Duration::from_secs(5)), "a panic must retire inflight work");
            let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                app.run(|| {
                    spawn(job);
                });
                app.poll();
            }))
            .expect_err("the awaiting task must observe the worker panic");
            assert_eq!(failure.downcast_ref::<i32>(), Some(&attempt), "preserve the original payload");
            assert!(app.is_failed());
            assert!(app.pending().iter().all(|work| work.kind == WorkKind::Update), "only App failure remains pending");
            app.shutdown();
            assert!(app.pending().is_empty());
        }

        let app = App::new(HeadlessPlatform::default());
        let result = Rc::new(Cell::new(None));
        let out = result.clone();
        let scheduler = app.run(|| {
            spawn(async move {
                out.set(Some(background(|| Err::<u32, _>("expected failure")).await));
            });
            sched()
        });
        app.poll();
        assert!(scheduler.wait_background(Duration::from_secs(5)));
        app.poll();
        assert_eq!(result.get(), Some(Err("expected failure")));
        assert!(!app.is_failed(), "business errors remain ordinary values");
        app.shutdown();
    }

    #[test]
    fn cancelled_background_panics_retire_without_resuming_the_cancelled_task() {
        let app = App::new(HeadlessPlatform::default());
        let continued = Rc::new(Cell::new(false));
        let out = continued.clone();
        let (release, wait) = std::sync::mpsc::channel();
        let scheduler = app.run(|| {
            let task = spawn(async move {
                background(move || {
                    wait.recv().unwrap();
                    panic!("cancelled work failed");
                })
                .await;
                out.set(true);
            });
            // Run until the worker has been submitted and the task is waiting on its result.
            let scheduler = sched();
            scheduler.run_ready();
            task.cancel();
            scheduler
        });
        release.send(()).unwrap();
        assert!(scheduler.wait_background(Duration::from_secs(5)));
        app.poll();
        assert!(!continued.get());
        assert!(!app.is_failed());
        assert!(app.pending().is_empty());
        app.shutdown();
    }

    #[test]
    fn finished_and_cancelled_tasks_release_scope_registrations() {
        let app = App::new(HeadlessPlatform::default());
        for _ in 0..256 {
            app.run(|| {
                let task = spawn(std::future::pending());
                rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 1));
                task.cancel();
                rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0));
                spawn(async {});
            });
            app.poll();
            app.run(|| rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0)));
        }
        assert!(app.pending().is_empty());
        app.shutdown();
    }

    #[test]
    fn shutdown_retires_the_current_poll_and_ignores_late_wakes() {
        struct Dropped(Rc<Cell<bool>>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let runtime = crate::reactive::graph::Runtime::new();
        runtime.enter(|| {
            let scope = runtime.create_scope(None, "app");
            let scheduler = Rc::new(Scheduler::default());
            let wakes = Arc::new(AtomicUsize::new(0));
            let notified = wakes.clone();
            scheduler.set_waker(Arc::new(move || {
                notified.fetch_add(1, Ordering::Relaxed);
            }));
            let late_waker = Rc::new(RefCell::new(None::<Waker>));
            let current_dropped = Rc::new(Cell::new(false));
            let sibling_ran = Rc::new(Cell::new(false));
            let (closing, saved, dropped) = (scheduler.clone(), late_waker.clone(), current_dropped.clone());
            let guard = Dropped(current_dropped.clone());
            let current = scheduler.insert(
                Box::pin(async move {
                    let _guard = guard;
                    std::future::poll_fn(move |cx| {
                        *saved.borrow_mut() = Some(cx.waker().clone());
                        closing.shutdown();
                        assert!(!dropped.get(), "a future is retained until its active poll returns");
                        cx.waker().wake_by_ref();
                        Poll::<()>::Pending
                    })
                    .await;
                }),
                scope,
                WorkKind::Task,
                "current".into(),
                Location::caller(),
            );
            let sibling = sibling_ran.clone();
            scheduler.insert(Box::pin(async move { sibling.set(true) }), scope, WorkKind::Task, "sibling".into(), Location::caller());
            scheduler.add_timer(100.0, current);
            let interest = Arc::new(NamedInterest { scheduler: Arc::downgrade(&scheduler.shared) });
            scheduler
                .names
                .borrow_mut()
                .insert("queued".into(), NamedChannel { inject: Rc::new(|_| Ok(())), receiver: Arc::downgrade(&interest), _cleanup: None });

            scheduler.run_ready();

            assert!(current_dropped.get());
            assert!(!sibling_ran.get());
            assert_eq!(scheduler.current.get(), None);
            assert!(scheduler.slots.borrow().iter().all(Option::is_none));
            assert!(scheduler.timers.borrow().is_empty());
            assert!(scheduler.names.borrow().is_empty());
            late_waker.borrow().as_ref().unwrap().wake_by_ref();
            scheduler.shutdown();
            assert!(!scheduler.run_ready());
            assert!(!scheduler.has_ready());
            assert_eq!(wakes.load(Ordering::Relaxed), 0);
        });
    }

    #[test]
    fn shutdown_rejects_new_background_work_without_dispatching_it() {
        let app = App::new(HeadlessPlatform::default());
        let ran = Arc::new(AtomicBool::new(false));
        let called = ran.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.run(|| {
                sched().shutdown();
                drop(background(move || {
                    called.store(true, Ordering::Release);
                }));
            });
        }));
        assert!(result.is_err());
        assert!(!ran.load(Ordering::Acquire));
        assert!(app.pending().iter().all(|work| work.kind == WorkKind::Update), "only App failure is pending, not a background job");
        assert!(app.is_failed());
        app.shutdown();
        assert!(app.diagnostics().iter().any(|diagnostic| diagnostic.code == "WRITE_DISPOSED"));
    }

    #[test]
    fn an_app_task_has_no_window_owner_and_outlives_the_last_window() {
        let app = App::new(HeadlessPlatform::default());
        let shared = app.run(|| state("shared", || 0));
        let owner = Rc::new(Cell::new(Some(crate::reactive::graph::Owner(0))));
        let seen = owner.clone();
        let window = app
            .open_window(WindowOptions::new("temporary"), move || {
                let seen = seen.clone();
                App::current().run(|| {
                    spawn(async move {
                        sleep(Duration::from_millis(10)).await;
                        seen.set(crate::reactive::graph::current_owner());
                        shared.set(1);
                    });
                });
                el().size_full().into()
            })
            .unwrap();
        assert_eq!(app.pending()[0].due, Some(Duration::from_millis(10)), "the task reached its first wait");
        assert_eq!(app.pending()[0].scope, "app");
        app.close_window(window.id());
        assert_eq!(app.pending().len(), 1, "closing the last window preserves the app task");
        app.advance(Duration::from_millis(10));
        assert_eq!(owner.get(), None, "app tasks run without a current window");
        assert_eq!(app.run(|| shared.get()), 1);
        assert!(app.pending().is_empty());
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    }

    #[test]
    fn a_completed_ui_tasks_cleanup_does_not_cancel_a_reused_app_task() {
        let app = App::new(HeadlessPlatform::default());
        let window = app
            .open_window(WindowOptions::new("temporary"), || {
                spawn(async {});
                el().size_full().into()
            })
            .unwrap();
        assert!(app.pending().is_empty());
        let finished = Rc::new(Cell::new(false));
        let done = finished.clone();
        app.run(|| {
            spawn(async move {
                sleep(Duration::from_millis(10)).await;
                done.set(true);
            });
        });

        app.close_window(window.id());

        assert_eq!(app.pending()[0].scope, "app");
        app.advance(Duration::from_millis(10));
        assert!(finished.get());
        assert!(app.pending().is_empty());
        assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    }
}
