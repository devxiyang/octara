//! Async work owned by scopes: tasks, timers on the app's clock, channels from other threads,
//! background work, and resources (keyed async loads with their state as signals).
//!
//! - A task (`spawn`) belongs to the scope it was spawned in and is dropped (cancelled) when the
//!   scope is disposed; `detach` lets one outlive its scope. Tasks run on the UI thread, like
//!   handlers: they may write signals.
//! - Every result from another thread comes back one way: it wakes a task, which the runtime
//!   polls at a frame boundary. Woken tasks run in spawn order, so the order threads finish in
//!   does not change what happens.
//! - Time is the app's clock (`App::advance`), never the wall clock: `sleep`, `interval` and
//!   `now` follow it, so tests decide when timers fire.
//! - Pending work is listed (`App::pending`), with the scope and source location of each.

use crate::reactive::graph::{Ctx, ScopeId, Signal, rt, watch};
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
    /// A task (`spawn`, a resource's load).
    Task,
    /// A long-lived listener (`interval`, a channel's `on`): not work that finishes.
    Subscription,
    /// A node's properties moving to their targets (`due`: when they come to rest).
    Motion,
}

/// One piece of pending work.
#[derive(Clone, PartialEq, Debug)]
pub struct PendingWork {
    pub kind: WorkKind,
    /// What it is: `task`, `resource`, `interval`, `channel "token"`, ….
    pub label: String,
    /// The scope that owns it (`app/chat/…`), or `detached`.
    pub scope: String,
    /// Where it was started (`file:line`).
    pub at: String,
    /// When the timer it waits on is due, on the app's clock.
    pub due: Option<Duration>,
}

struct Entry {
    id: TaskId,
    future: Option<BoxFuture>,
    scope: Option<ScopeId>,
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
    ready: Mutex<Vec<TaskId>>,
    inflight: Mutex<usize>,
    idle: Condvar,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Shared {
    fn notify(&self) {
        let wake = self.wake.lock().expect("wake").clone();
        if let Some(w) = wake {
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
            s.ready.lock().expect("ready").push(self.id);
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

impl Default for Scheduler {
    fn default() -> Self {
        Scheduler {
            now: Cell::new(0.0),
            slots: RefCell::default(),
            generations: RefCell::default(),
            free: RefCell::default(),
            shared: Arc::new(Shared { ready: Mutex::default(), inflight: Mutex::new(0), idle: Condvar::new(), wake: Mutex::new(None) }),
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
        self.now.set(ms);
    }
    pub(crate) fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.shared.wake.lock().expect("wake") = Some(wake);
    }

    fn insert(&self, future: BoxFuture, scope: Option<ScopeId>, kind: WorkKind, label: String, at: &'static Location<'static>) -> TaskId {
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
        self.slots.borrow_mut()[slot] = Some(Entry { id, future: Some(future), scope, kind, label, at, cancelled: false, owner });
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
            slots[id.slot()] = None;
            drop(slots);
            self.free.borrow_mut().push(id.slot());
            drop(f);
        }
    }

    fn detach(&self, id: TaskId) {
        if let Some(Some(e)) = self.slots.borrow_mut().get_mut(id.slot())
            && e.id == id
        {
            e.scope = None;
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
            for id in batch {
                ran |= self.poll(id);
            }
        }
        ran
    }

    fn poll(&self, id: TaskId) -> bool {
        let (mut future, scope, owner) = {
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
        let prev = self.current.replace(Some(id));
        let done =
            rt(|r| r.with_owner(owner, || r.with_frame(Ctx::Handler, None, Some(scope), || future.as_mut().poll(&mut cx).is_ready())));
        self.current.set(prev);
        let mut slots = self.slots.borrow_mut();
        let cancelled = slots.get(id.slot()).and_then(|e| e.as_ref()).is_none_or(|e| e.id != id || e.cancelled);
        if done || cancelled {
            if slots.get(id.slot()).and_then(|e| e.as_ref()).is_some_and(|e| e.id == id) {
                slots[id.slot()] = None;
                drop(slots);
                self.free.borrow_mut().push(id.slot());
            }
        } else if let Some(Some(e)) = slots.get_mut(id.slot()) {
            e.future = Some(future);
        }
        true
    }

    fn add_timer(&self, due: f64, id: TaskId) {
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
                    scope: e.scope.map_or_else(|| "detached".to_owned(), |s| rt(|r| r.scope_path(s))),
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
    id: TaskId,
    sched: Rc<Scheduler>,
}

impl Task {
    /// Stops the task: its future is dropped (and with it what it holds) at once.
    pub fn cancel(&self) {
        self.sched.cancel(self.id);
    }
    /// Lets the task outlive its scope; it runs until it finishes (or the app goes away).
    pub fn detach(self) {
        self.sched.detach(self.id);
    }
    pub fn is_finished(&self) -> bool {
        !self.sched.alive(self.id)
    }
}

fn spawn_with(future: impl Future<Output = ()> + 'static, kind: WorkKind, label: String, at: &'static Location<'static>) -> Task {
    let s = sched();
    let scope = rt(|r| r.current_scope());
    let id = s.insert(Box::pin(future), scope, kind, label, at);
    if scope.is_some() {
        let weak = Rc::downgrade(&s);
        rt(|r| {
            r.on_cleanup(Box::new(move || {
                if let Some(s) = weak.upgrade() {
                    // A detached task no longer belongs to the scope.
                    let owned = s.slots.borrow().get(id.slot()).and_then(|e| e.as_ref()).is_some_and(|e| e.id == id && e.scope.is_some());
                    if owned {
                        s.cancel(id);
                    }
                }
            }))
        });
    }
    Task { id, sched: s }
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
    Sleep { due: sched().now_ms() + d.as_secs_f64() * 1000.0 }
}

/// Waits until the app's clock reads `t`.
pub fn sleep_until(t: Duration) -> Sleep {
    Sleep { due: t.as_secs_f64() * 1000.0 }
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
pub fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Background<T> {
    let slot = Arc::new(Oneshot { value: Mutex::new(None), waker: Mutex::new(None) });
    let shared = sched().shared.clone();
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
    #[track_caller]
    pub fn on(self, f: impl Fn(T) + 'static) -> Task {
        let label = match &self.name {
            Some(n) => format!("channel {n:?}"),
            None => "channel".to_owned(),
        };
        let fut = async move {
            while let Some(v) = self.recv().await {
                f(v);
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

// ---- resources ----

/// A resource's state: loading or not, the last value, and the last error. A new load keeps the
/// value it replaces until it lands; an error keeps the last value too.
#[derive(Clone, PartialEq, Debug)]
pub struct Load<T, E> {
    pub loading: bool,
    pub value: Option<T>,
    pub error: Option<E>,
}

/// An async load keyed by reactive input (see [`resource`]).
pub struct Resource<T: 'static, E: 'static = String> {
    state: Signal<Load<T, E>>,
    refetches: Signal<u64>,
}

impl<T, E> Clone for Resource<T, E> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, E> Copy for Resource<T, E> {}

impl<T: Clone + 'static, E: Clone + 'static> Resource<T, E> {
    /// The whole state (tracked).
    pub fn get(&self) -> Load<T, E> {
        self.state.get()
    }
    pub fn loading(&self) -> bool {
        self.state.with(|s| s.loading)
    }
    /// The last value loaded, if any (kept while reloading and after an error).
    pub fn value(&self) -> Option<T> {
        self.state.with(|s| s.value.clone())
    }
    pub fn error(&self) -> Option<E> {
        self.state.with(|s| s.error.clone())
    }
    /// Loads again with the same key (the running load, if any, is cancelled).
    pub fn refetch(&self) {
        self.refetches.update(|n| *n += 1);
    }
}

/// Loads with `fetch(key())` now and again whenever the key changes (or on `refetch`): the
/// previous load is cancelled, so an old answer never overwrites a newer one; an unchanged key
/// loads nothing. Owned by the current scope.
#[track_caller]
pub fn resource<K, T, E, Fut>(key: impl Fn() -> K + 'static, fetch: impl Fn(K) -> Fut + 'static) -> Resource<T, E>
where
    K: Clone + PartialEq + 'static,
    T: Clone + 'static,
    E: Clone + 'static,
    Fut: Future<Output = Result<T, E>> + 'static,
{
    let at = Location::caller();
    let state = Signal::scoped(Load { loading: true, value: None, error: None });
    let refetches = Signal::scoped(0u64);
    let running: Rc<RefCell<Option<Task>>> = Rc::default();
    let scope = rt(|r| r.current_scope());
    watch(
        move || (key(), refetches.get()),
        move |(k, _), _| {
            if let Some(t) = running.borrow_mut().take() {
                t.cancel();
            }
            state.update(|s| s.loading = true);
            let load = fetch(k.clone());
            let fut = async move {
                let r = load.await;
                state.update(|s| {
                    s.loading = false;
                    match r {
                        Ok(v) => {
                            s.value = Some(v);
                            s.error = None;
                        }
                        Err(e) => s.error = Some(e),
                    }
                });
            };
            // The load belongs to the resource's scope, not to whatever runs the watch.
            let task = rt(|r| r.with_frame(Ctx::Handler, None, Some(scope), || spawn_with(fut, WorkKind::Task, "resource".into(), at)));
            *running.borrow_mut() = Some(task);
        },
    );
    Resource { state, refetches }
}
