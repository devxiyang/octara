//! Async work owned by scopes: tasks, timers on the app's clock, channels from other threads,
//! background work, and resources (keyed async loads with their state as signals).
//!
//! - A task (`spawn`) belongs to the scope it was spawned in and is dropped (cancelled) when the
//!   scope is disposed. Choose the app root with `App::current().run` before spawning work
//!   that must outlive its initiating UI scope. Tasks run on the UI thread and may write signals.
//! - Every result from another thread comes back one way: it wakes a task, which the runtime
//!   polls at a frame boundary. Woken tasks run in spawn order, so the order threads finish in
//!   does not change what happens.
//! - Time is the app's clock (`App::advance`), never the wall clock: `sleep`, `interval` and
//!   `now` follow it, so tests decide when timers fire.
//! - Pending work is listed (`App::pending`), with the scope and source location of each.

use crate::reactive::graph::{Ctx, ScopeId, Signal, memo, rt, untrack, watch};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::panic::Location;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

type BoxFuture = Pin<Box<dyn Future<Output = ()>>>;
/// Sends text, parsed, into a named channel.
type Injector = Rc<dyn Fn(&str) -> Result<(), String>>;

/// A task's id: slot and generation (a reused slot gets a new id).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
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
    shared: Arc<Shared>,
    /// Timers: (due, sequence, task) — the task is woken when the clock reaches `due`.
    timers: RefCell<Vec<(f64, u64, TaskId)>>,
    timer_seq: Cell<u64>,
    /// The task being polled (timers started in it wake it).
    current: Cell<Option<TaskId>>,
    /// Channels that take text by name (tests and agents send to them).
    names: RefCell<FxHashMap<String, Injector>>,
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
            rt(|rt| rt.diag(&crate::diagnostics::WRITE_DISPOSED, "cannot register work after app shutdown".into(), at));
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
            Some(Entry { cleanup: None, id, future: Some(future), scope, kind, label, at, cancelled: false, owner });
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
        let mut ran = false;
        for _ in 0..10_000 {
            let mut batch = std::mem::take(&mut *self.shared.ready.lock().expect("ready"));
            if batch.is_empty() {
                break;
            }
            batch.sort();
            batch.dedup();
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

    fn add_timer(&self, due: f64, id: TaskId) {
        if self.shared.closed.load(Ordering::Acquire) {
            return;
        }
        let seq = self.timer_seq.get() + 1;
        self.timer_seq.set(seq);
        self.timers.borrow_mut().push((due, seq, id));
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
                    at: format!("{}:{}", crate::diagnostics::source_file(e.at), e.at.line()),
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

    /// Sends `payload` to the channel named `name`.
    pub(crate) fn send_named(&self, name: &str, payload: &str) -> Result<(), String> {
        let f = self.names.borrow().get(name).cloned().ok_or_else(|| format!("no channel named `{name}`"))?;
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
        rt(|rt| rt.diag(&crate::diagnostics::WRITE_DISPOSED, "cannot spawn a task without an app or component scope".into(), at));
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

/// Completes when the app's clock reaches `due`.
pub struct Sleep {
    due: f64,
}

impl Future for Sleep {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let s = sched();
        if s.now_ms() >= self.due {
            return Poll::Ready(());
        }
        match s.current.get() {
            Some(id) => s.add_timer(self.due, id),
            None => drop(cx.waker().clone()), // Only tasks of this app can wait on its clock.
        }
        Poll::Pending
    }
}

/// Waits `d` on the app's clock.
pub fn sleep(d: Duration) -> Sleep {
    Sleep { due: whole_ns(sched().now_ms() + d.as_secs_f64() * 1000.0) }
}

/// Waits until the app's clock reads `t`.
pub fn sleep_until(t: Duration) -> Sleep {
    Sleep { due: whole_ns(t.as_secs_f64() * 1000.0) }
}

/// A time in milliseconds rounded to the clock's resolution, the nanosecond (`Duration`'s): the
/// clock and every due time go through it, so a due time handed out as a `Duration` and moved
/// to is reached exactly.
pub(crate) fn whole_ns(ms: f64) -> f64 {
    (ms * 1e6).round() / 1e6
}

/// Calls `f` every `period` on the app's clock while the current scope lives (ticks are counted
/// from now, so they do not drift).
#[track_caller]
pub fn interval(period: Duration, f: impl Fn() + 'static) -> Task {
    let start = now();
    let fut = async move {
        let mut k = 1u32;
        loop {
            sleep_until(start + period * k).await;
            f();
            k += 1;
        }
    };
    spawn_with(fut, WorkKind::Subscription, format!("interval {period:?}"), Location::caller())
}

// ---- background work ----

struct Oneshot<T> {
    value: Mutex<Option<T>>,
    waker: Mutex<Option<Waker>>,
}

/// The result of [`background`] work.
pub struct Background<T> {
    slot: Arc<Oneshot<T>>,
}

impl<T> Future for Background<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if let Some(v) = self.slot.value.lock().expect("oneshot").take() {
            return Poll::Ready(v);
        }
        *self.slot.waker.lock().expect("oneshot") = Some(cx.waker().clone());
        // It may have landed between the check and storing the waker.
        match self.slot.value.lock().expect("oneshot").take() {
            Some(v) => Poll::Ready(v),
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
#[track_caller]
pub fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Background<T> {
    let scheduler = sched();
    if !scheduler.accepts_work(Location::caller()) {
        panic!("[{}] background work requested after app shutdown", crate::diagnostics::WRITE_DISPOSED);
    }
    let slot = Arc::new(Oneshot { value: Mutex::new(None), waker: Mutex::new(None) });
    let shared = scheduler.shared.clone();
    *shared.inflight.lock().expect("inflight") += 1;
    let done = slot.clone();
    let job: Job = Box::new(move || {
        let v = work();
        *done.value.lock().expect("oneshot") = Some(v);
        if let Some(w) = done.waker.lock().expect("oneshot").take() {
            w.wake();
        }
        let mut n = shared.inflight.lock().expect("inflight");
        *n -= 1;
        if *n == 0 {
            shared.idle.notify_all();
        }
        drop(n);
        shared.notify();
    });
    pool().lock().expect("pool").send(job).expect("background pool");
    Background { slot }
}

/// A value coming later from any thread (a platform's answer): the reply that sets it, and the
/// future that awaits it in a task.
pub(crate) fn later<T: Send + 'static>() -> (crate::platform::Reply<T>, impl Future<Output = T>) {
    let slot = Arc::new(Oneshot { value: Mutex::new(None), waker: Mutex::new(None) });
    let shared = sched().shared.clone();
    let done = slot.clone();
    let reply: crate::platform::Reply<T> = Box::new(move |v| {
        *done.value.lock().expect("oneshot") = Some(v);
        if let Some(w) = done.waker.lock().expect("oneshot").take() {
            w.wake();
        }
        shared.notify();
    });
    (reply, Background { slot })
}

// ---- channels ----

struct Chan<T> {
    queue: Mutex<VecDeque<T>>,
    waker: Mutex<Option<Waker>>,
    receiver_alive: AtomicBool,
    senders: AtomicUsize,
}

/// Sends values into the app from any thread (an LLM's tokens, a socket's messages).
pub struct Sender<T> {
    chan: Arc<Chan<T>>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.chan.senders.fetch_add(1, Ordering::Relaxed);
        Sender { chan: self.chan.clone() }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        if self.chan.senders.fetch_sub(1, Ordering::AcqRel) == 1
            && let Some(w) = self.chan.waker.lock().expect("chan").take()
        {
            w.wake();
        }
    }
}

impl<T> Sender<T> {
    /// Queues `value` for the receiver; `Err(value)` once the receiver (its scope) is gone.
    pub fn send(&self, value: T) -> Result<(), T> {
        if !self.chan.receiver_alive.load(Ordering::Acquire) {
            return Err(value);
        }
        self.chan.queue.lock().expect("chan").push_back(value);
        if let Some(w) = self.chan.waker.lock().expect("chan").take() {
            w.wake();
        }
        Ok(())
    }
}

/// Receives a channel's values on the UI thread: await them in a task (`recv`), or handle each
/// (`on`). Dropped with its scope; senders then get their values back.
pub struct Receiver<T> {
    chan: Arc<Chan<T>>,
    name: Option<String>,
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.chan.receiver_alive.store(false, Ordering::Release);
    }
}

/// A channel into the UI thread.
pub fn channel<T: Send + 'static>() -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Chan {
        queue: Mutex::default(),
        waker: Mutex::new(None),
        receiver_alive: AtomicBool::new(true),
        senders: AtomicUsize::new(1),
    });
    (Sender { chan: chan.clone() }, Receiver { chan, name: None })
}

/// The next value of a channel.
pub struct Recv<'a, T> {
    rx: &'a Receiver<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let chan = &self.rx.chan;
        if let Some(v) = chan.queue.lock().expect("chan").pop_front() {
            return Poll::Ready(Some(v));
        }
        if chan.senders.load(Ordering::Acquire) == 0 {
            return Poll::Ready(None);
        }
        *chan.waker.lock().expect("chan") = Some(cx.waker().clone());
        // A value or the last sender's drop may have come in between.
        if let Some(v) = chan.queue.lock().expect("chan").pop_front() {
            return Poll::Ready(Some(v));
        }
        if chan.senders.load(Ordering::Acquire) == 0 {
            return Poll::Ready(None);
        }
        Poll::Pending
    }
}

impl<T: Send + 'static> Receiver<T> {
    /// The next value, or `None` when every sender is gone and nothing is left.
    pub fn recv(&self) -> Recv<'_, T> {
        Recv { rx: self }
    }

    /// Calls `f` with each value as it arrives, while the current scope lives.
    /// After 64 values it yields a poll to other ready tasks. This is task cooperation;
    /// the scheduler may continue polling in the same frame, without yielding to the OS.
    #[track_caller]
    pub fn on(self, f: impl Fn(T) + 'static) -> Task {
        let label = match &self.name {
            Some(n) => format!("channel {n:?}"),
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
    /// parsed with `FromStr`) while the current scope lives.
    pub fn named(mut self, name: &str) -> Self {
        let s = sched();
        if !s.accepts_work(Location::caller()) {
            return self;
        }
        // The name is a sender: the channel stays open while it is registered.
        self.chan.senders.fetch_add(1, Ordering::Relaxed);
        let sender = Sender { chan: self.chan.clone() };
        let key = name.to_owned();
        let inject: Injector = Rc::new(move |payload: &str| {
            let v = payload.parse::<T>().map_err(|_| format!("`{payload}` does not parse as the channel's type"))?;
            sender.send(v).map_err(|_| "the channel's receiver is gone".to_owned())
        });
        s.names.borrow_mut().insert(key.clone(), inject.clone());
        let weak = Rc::downgrade(&s);
        rt(|r| {
            r.on_cleanup(Box::new(move || {
                if let Some(s) = weak.upgrade() {
                    let mut names = s.names.borrow_mut();
                    if names.get(&key).is_some_and(|f| Rc::ptr_eq(f, &inject)) {
                        names.remove(&key);
                    }
                }
            }))
        });
        self.name = Some(name.to_owned());
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
            scheduler.names.borrow_mut().insert("queued".into(), Rc::new(|_| Ok(())));

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
