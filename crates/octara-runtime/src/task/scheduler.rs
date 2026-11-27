use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, btree_set};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use octara_model::identity::WindowId;
use octara_tree::Tree;

use crate::app::AppCommands;
use crate::arena::{ReactiveWorld, ScopeId};

use super::{TaskCx, TaskState, VirtualClock};

#[cfg(not(target_family = "wasm"))]
use super::background::Failure;

const TASK_POLLS_PER_BATCH: usize = 256;
static NEXT_SCHEDULER: AtomicU32 = AtomicU32::new(1);

type LocalFuture = Pin<Box<dyn Future<Output = ()>>>;
type HostWake = Arc<dyn Fn() + Send + Sync>;

/// An app-specific task identity that is never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId {
    scheduler: u32,
    sequence: u64,
}

/// The owner and label of a queued or pending task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskInfo {
    pub id: TaskId,
    pub scope: ScopeId,
    pub window: Option<WindowId>,
    pub label: &'static str,
}

struct Task {
    state: Rc<TaskState>,
    future: LocalFuture,
    waker: Waker,
}

#[derive(Default)]
struct WakeState {
    live: BTreeSet<TaskId>,
    ready: BTreeSet<TaskId>,
    deferred: BTreeSet<TaskId>,
    external: BTreeSet<TaskId>,
    #[cfg(not(target_family = "wasm"))]
    background_failures: std::collections::VecDeque<Failure>,
    #[cfg(not(target_family = "wasm"))]
    closed: bool,
}

#[cfg_attr(target_family = "wasm", derive(Default))]
pub(super) struct WakeInbox {
    state: Mutex<WakeState>,
    host: Mutex<Option<HostWake>>,
    #[cfg(not(target_family = "wasm"))]
    owner: std::thread::ThreadId,
}

#[cfg(not(target_family = "wasm"))]
impl Default for WakeInbox {
    fn default() -> Self {
        Self { state: Mutex::default(), host: Mutex::default(), owner: std::thread::current().id() }
    }
}

impl WakeInbox {
    fn notify(&self) {
        let wake = self.host.lock().expect("task host wake").clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    fn register(&self, id: TaskId) {
        let mut state = self.state.lock().expect("task wakes");
        state.live.insert(id);
        state.ready.insert(id);
    }

    fn retire(&self, id: TaskId) {
        let mut state = self.state.lock().expect("task wakes");
        state.live.remove(&id);
        state.ready.remove(&id);
        state.deferred.remove(&id);
        state.external.remove(&id);
    }

    #[cfg(not(target_family = "wasm"))]
    pub(super) fn report_background_failure(&self, failure: Failure) {
        let mut state = self.state.lock().expect("task wakes");
        if !state.closed {
            state.background_failures.push_back(failure);
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn resume_background_failure(&self) {
        let failure = self.state.lock().expect("task wakes").background_failures.pop_front();
        if let Some(failure) = failure {
            resume_unwind(failure);
        }
    }
}

struct TaskWake {
    inbox: Weak<WakeInbox>,
    id: TaskId,
}

impl Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if let Some(inbox) = self.inbox.upgrade() {
            let admitted = {
                let mut state = inbox.state.lock().expect("task wakes");
                if !state.live.contains(&self.id) || state.deferred.contains(&self.id) {
                    return;
                }
                #[cfg(not(target_family = "wasm"))]
                let external = std::thread::current().id() != inbox.owner;
                #[cfg(target_family = "wasm")]
                let external = false;
                if external { state.external.insert(self.id) } else { state.ready.insert(self.id) }
            };
            if admitted {
                inbox.notify();
            }
        }
    }
}

struct ReadyBatch {
    inbox: Arc<WakeInbox>,
    remaining: btree_set::IntoIter<TaskId>,
}

impl Drop for ReadyBatch {
    fn drop(&mut self) {
        let mut state = self.inbox.state.lock().expect("task wakes");
        let WakeState { live, deferred, .. } = &mut *state;
        deferred.extend(self.remaining.by_ref().filter(|id| live.contains(id)));
    }
}

struct PendingTasks {
    scheduler: u32,
    next: Cell<u64>,
    closed: Cell<bool>,
    pending: RefCell<Vec<Task>>,
    live: RefCell<BTreeMap<TaskId, Rc<TaskState>>>,
    cancelled: RefCell<Vec<TaskId>>,
    inbox: Arc<WakeInbox>,
    clock: VirtualClock,
}

/// Local task registration shared with capability contexts.
#[derive(Clone)]
pub struct TaskSpawner(Rc<PendingTasks>);

impl TaskSpawner {
    pub(crate) fn host_wake(&self) -> Option<HostWake> {
        self.0.inbox.host.lock().expect("task host wake").clone()
    }

    pub(super) fn now(&self) -> Duration {
        self.0.clock.now()
    }

    pub(crate) fn spawn<Fut>(
        &self,
        scope: ScopeId,
        window: Option<WindowId>,
        label: &'static str,
        factory: impl FnOnce(TaskCx) -> Fut + 'static,
    ) -> TaskId
    where
        Fut: Future<Output = ()> + 'static,
    {
        let queue = &self.0;
        let sequence = queue.next.get();
        queue.next.set(sequence.checked_add(1).expect("task sequence exhausted"));
        let id = TaskId { scheduler: queue.scheduler, sequence };
        if queue.closed.get() {
            return id;
        }

        let state = Rc::new(TaskState {
            info: TaskInfo { id, scope, window, label },
            live: Cell::new(true),
            write: Cell::new(None),
            #[cfg(not(target_family = "wasm"))]
            inbox: Arc::downgrade(&queue.inbox),
        });
        let cx = TaskCx { state: state.clone(), clock: queue.clock.clone() };
        let future = Box::pin(async move { factory(cx).await });
        let waker = Waker::from(Arc::new(TaskWake { inbox: Arc::downgrade(&queue.inbox), id }));
        queue.live.borrow_mut().insert(id, state.clone());
        queue.pending.borrow_mut().push(Task { state, future, waker });
        queue.inbox.register(id);
        queue.inbox.notify();
        id
    }

    /// Media transport starts after the authored projection has been published.
    pub(crate) fn defer_until_next_turn(&self, id: TaskId) {
        let mut state = self.0.inbox.state.lock().expect("task wakes");
        if state.ready.remove(&id) {
            state.external.insert(id);
        }
    }

    pub(super) fn cancel(&self, id: TaskId) -> bool {
        let queue = &self.0;
        let retired = queue.live.borrow_mut().remove(&id);
        let Some(state) = retired else {
            return false;
        };
        state.live.set(false);
        queue.inbox.retire(id);
        queue.cancelled.borrow_mut().push(id);
        queue.clock.cancel_task(id);
        true
    }
}

/// The app's local task executor, with thread-safe wake delivery.
pub struct TaskScheduler {
    spawner: TaskSpawner,
    tasks: BTreeMap<TaskId, Task>,
}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskScheduler {
    pub fn new() -> Self {
        let scheduler = NEXT_SCHEDULER
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("task scheduler identity exhausted");
        let spawner = TaskSpawner(Rc::new(PendingTasks {
            scheduler,
            next: Cell::new(0),
            closed: Cell::new(false),
            pending: RefCell::default(),
            live: RefCell::default(),
            cancelled: RefCell::default(),
            inbox: Arc::default(),
            clock: VirtualClock::new(),
        }));
        Self { spawner, tasks: BTreeMap::new() }
    }

    pub fn spawner(&self) -> TaskSpawner {
        self.spawner.clone()
    }

    pub fn spawn<Fut>(
        &mut self,
        scope: ScopeId,
        window: Option<WindowId>,
        label: &'static str,
        factory: impl FnOnce(TaskCx) -> Fut + 'static,
    ) -> TaskId
    where
        Fut: Future<Output = ()> + 'static,
    {
        self.spawner.spawn(scope, window, label, factory)
    }

    /// Sets the notification called when work becomes ready. The host may receive
    /// this notification from another thread and should arrange an app turn.
    pub fn set_wake(&self, wake: Option<HostWake>) {
        if !self.spawner.0.closed.get() {
            let retired = std::mem::replace(&mut *self.spawner.0.inbox.host.lock().expect("task host wake"), wake);
            drop(retired);
        }
    }

    pub fn now(&self) -> Duration {
        self.spawner.0.clock.now()
    }

    pub fn advance_to(&self, now: Duration) {
        self.spawner.0.clock.advance_to(now);
    }

    pub fn next_deadline(&self) -> Option<Duration> {
        self.spawner.0.clock.next_deadline()
    }

    pub fn has_ready(&self) -> bool {
        self.ready(true)
    }

    pub(crate) fn has_turn_ready(&self) -> bool {
        self.ready(false)
    }

    fn ready(&self, include_external: bool) -> bool {
        let state = self.spawner.0.inbox.state.lock().expect("task wakes");
        #[cfg(not(target_family = "wasm"))]
        if !state.background_failures.is_empty() {
            return true;
        }
        !state.ready.is_empty() || !state.deferred.is_empty() || (include_external && !state.external.is_empty())
    }

    /// Snapshots producer-thread arrivals at the ingress of the next application turn.
    pub(crate) fn begin_turn(&self) {
        let mut state = self.spawner.0.inbox.state.lock().expect("task wakes");
        let WakeState { ready, external, .. } = &mut *state;
        ready.append(external);
    }

    pub fn pending(&self) -> Vec<TaskInfo> {
        self.spawner.0.live.borrow().values().map(|state| state.info).collect()
    }

    pub fn cancel(&mut self, id: TaskId) -> bool {
        let cancelled = self.spawner.cancel(id);
        self.sync_tasks();
        cancelled
    }

    pub fn cancel_scope(&mut self, scope: ScopeId) {
        self.cancel_matching(|task| task.scope == scope);
    }

    pub fn cancel_retired_scopes(&mut self, world: &ReactiveWorld) {
        self.cancel_matching(|task| !world.is_scope_alive(task.scope));
    }

    fn cancel_matching(&mut self, mut should_cancel: impl FnMut(TaskInfo) -> bool) {
        let retired: Vec<_> =
            self.spawner.0.live.borrow().values().filter(|state| should_cancel(state.info)).map(|state| state.info.id).collect();
        for id in retired {
            self.spawner.cancel(id);
        }
        self.sync_tasks();
    }

    /// Drops all task futures and rejects subsequent work from this scheduler.
    pub fn shutdown(&mut self) {
        self.spawner.0.closed.set(true);
        let retired = self.spawner.0.inbox.host.lock().expect("task host wake").take();
        #[cfg(not(target_family = "wasm"))]
        {
            let failures = {
                let mut state = self.spawner.0.inbox.state.lock().expect("task wakes");
                state.closed = true;
                std::mem::take(&mut state.background_failures)
            };
            drop(failures);
        }
        self.cancel_matching(|_| true);
        drop(retired);
    }

    pub fn poll_ready(&mut self, world: &mut ReactiveWorld, commands: &mut AppCommands) -> usize {
        self.begin_turn();
        self.poll_turn_ready(world, commands, |_| None)
    }

    /// Polls a bounded snapshot in spawn order. Local wakes remain part of this turn.
    pub(crate) fn poll_turn_ready<'a>(
        &mut self,
        world: &mut ReactiveWorld,
        commands: &mut AppCommands,
        tree: impl Fn(WindowId) -> Option<&'a Tree>,
    ) -> usize {
        #[cfg(not(target_family = "wasm"))]
        self.spawner.0.inbox.resume_background_failure();

        self.sync_tasks();
        let inbox = self.spawner.0.inbox.clone();
        let ready = {
            let mut state = inbox.state.lock().expect("task wakes");
            if state.deferred.is_empty() { std::mem::take(&mut state.ready) } else { std::mem::take(&mut state.deferred) }
        };
        let mut batch = ReadyBatch { inbox, remaining: ready.into_iter() };
        let mut polled = 0;
        while polled < TASK_POLLS_PER_BATCH {
            let Some(id) = batch.remaining.next() else { break };
            let Some(task) = self.tasks.get_mut(&id) else { continue };
            let TaskInfo { scope, window, .. } = task.state.info;
            if !world.is_scope_alive(scope) {
                self.cancel(id);
                continue;
            }

            batch.inbox.state.lock().expect("task wakes").ready.remove(&id);
            polled += 1;
            let result = {
                let mut cx = Context::from_waker(&task.waker);
                let mut write = world.write(scope, window).with_tasks(self.spawner.clone()).with_commands(commands);
                write.tree = window.and_then(&tree);
                let _registered = task.state.enter(&mut write);
                catch_unwind(AssertUnwindSafe(|| task.future.as_mut().poll(&mut cx)))
            };

            match result {
                Ok(Poll::Pending) => self.sync_tasks(),
                Ok(Poll::Ready(())) => {
                    self.cancel(id);
                }
                Err(payload) => {
                    self.cancel(id);
                    resume_unwind(payload);
                }
            }
        }
        polled
    }

    fn sync_tasks(&mut self) {
        let queue = &self.spawner.0;
        let pending = queue.pending.take();
        let cancelled = queue.cancelled.take();
        let mut retired = Vec::new();
        for task in pending {
            if task.state.live.get() {
                self.tasks.insert(task.state.info.id, task);
            } else {
                retired.push(task);
            }
        }
        for id in cancelled {
            retired.extend(self.tasks.remove(&id));
        }
        drop(retired);
    }
}

impl Drop for TaskScheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}
