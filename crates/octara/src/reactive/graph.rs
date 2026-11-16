//! The reactive graph (design: `docs/research/reactive-design-v2.md`).
//!
//! Layers and their contexts:
//! - inputs: signals and named state, written only from handlers and `watch` callbacks;
//! - derive: `memo`, pure, tracked, may not write;
//! - observe: property bindings, structural regions and `watch` sources, tracked, may not write;
//! - effects: handlers and `watch` callbacks, untracked, may write.
//!
//! Algorithm: push marking (Dirty on direct observers, Check further down), pull evaluation, memo
//! early cutoff, structural bindings before properties before watches, creation order. A node is marked Clean *before* it runs,
//! so a re-mark during its own run (a watch writing its source) schedules another run.
//!
//! Invariants: no runtime borrow is held while user code runs or while a value is dropped (values
//! may own other nodes, e.g. a list owns its item signals); handles carry generations.

mod scope;
mod signal;
#[cfg(test)]
mod tests;

pub use signal::{ReadSignal, Signal, Watch, memo, provide, state, untrack, use_context, watch};
pub(crate) use signal::{binding, memo_with, structural_binding};

use crate::diagnostics::{self as codes, Code, Diagnostic};
use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::panic::Location;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A node handle (opaque).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub(crate) struct Idx {
    runtime: u64,
    i: u32,
    g: u32,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum State {
    Clean,
    Check,
    Dirty,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Kind {
    Source,
    Memo,
    Structure,
    Binding,
    Watch,
}

type Value = Rc<RefCell<Box<dyn Any>>>;

fn value_cell<T: 'static>(value: T) -> Value {
    Rc::new(RefCell::new(Box::new(value)))
}

/// Restore execution bookkeeping on return and unwind alike.
struct Restore<'a, T: Copy>(&'a Cell<T>, T);

impl<'a, T: Copy> Restore<'a, T> {
    fn set(cell: &'a Cell<T>, value: T) -> Self {
        Self(cell, cell.replace(value))
    }
}

impl<T: Copy> Drop for Restore<'_, T> {
    fn drop(&mut self) {
        self.0.set(self.1);
    }
}
/// Memos return `Some(new)` when their value changed; observers return `None`.
type Compute = Box<dyn FnMut(Option<&Value>) -> Option<Value>>;

struct Node {
    g: u32,
    alive: bool,
    kind: Kind,
    value: Option<Value>,
    compute: Option<Compute>,
    sources: Vec<Idx>,
    observers: Vec<Idx>,
    state: State,
    queued: bool,
    computing: bool,
    seq: u64,
    /// Named state: kept when its scope is disposed for a hot reload.
    named: bool,
    /// `(observer, epoch)` of the last run that tracked this node: O(1) edge dedupe.
    tracked: Option<(Idx, u64)>,
    defined_at: &'static Location<'static>,
    /// The owner it was made under, restored when it runs.
    owner: Option<Owner>,
    scope: Option<ScopeId>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ScopeId {
    runtime: u64,
    i: u32,
    g: u32,
}

/// Internal work releases its owner registration when it finishes early.
pub(crate) struct CleanupRegistration {
    runtime: std::rc::Weak<Runtime>,
    scope: ScopeId,
    id: u64,
}

impl Drop for CleanupRegistration {
    fn drop(&mut self) {
        let Some(runtime) = self.runtime.upgrade() else { return };
        let cleanup = {
            let mut scopes = runtime.scopes.borrow_mut();
            scopes.get_mut(self.scope.i as usize).filter(|s| s.g == self.scope.g).and_then(|s| s.cleanups.remove(&self.id))
        };
        drop(cleanup);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeState {
    Live,
    Disposing,
    Retiring,
    Disposed,
}

struct ScopeData {
    g: u32,
    state: ScopeState,
    parent: Option<ScopeId>,
    children: Vec<ScopeId>,
    nodes: Vec<Idx>,
    cleanups: BTreeMap<u64, Box<dyn FnOnce()>>,
    next_cleanup: u64,
    path: String,
    names: HashMap<String, u32>,
    state_keys: HashSet<&'static str>,
    /// App state is looked up repeatedly; mounted components declare keys once per build.
    persistent_names: bool,
    contexts: Vec<(TypeId, Rc<dyn Any>)>,
    owner: Option<Owner>,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Ctx {
    Idle,
    /// A component body or branch being built; runs once.
    Build,
    /// Inside a memo.
    Derive,
    /// Inside a binding, region or watch source.
    Observe,
    /// A handler or watch callback.
    Handler,
    /// A named value initializer: sampled reads, no mutation of existing state.
    Init,
    /// Teardown may sample retiring state but cannot register new work.
    Cleanup,
    /// Inside a view's closure (`filter`, `sort_by`, `group_by`, `map`, `join`): a pure function
    /// of the item, run per item event; reads are not tracked and are reported.
    View,
}

/// Named state by (scope path, key), with the value's type.
type NamedStore = HashMap<(String, &'static str), (Idx, TypeId)>;

type CollectionDelivery = Box<dyn FnOnce()>;

/// Synchronous delta delivery, ordered by consumer dependency depth. This is drained before
/// the originating collection write returns; it never runs UI bindings or effects.
#[derive(Default)]
struct CollectionQueue {
    active: Cell<bool>,
    work: RefCell<BTreeMap<usize, VecDeque<CollectionDelivery>>>,
}

#[derive(Default)]
pub(crate) struct Runtime {
    identity: u64,
    collections: CollectionQueue,
    sampling: Cell<bool>,
    nodes: RefCell<Vec<Node>>,
    free_nodes: RefCell<Vec<u32>>,
    scopes: RefCell<Vec<ScopeData>>,
    free_scopes: RefCell<Vec<u32>>,
    observer: Cell<Option<Idx>>,
    ctx: Cell<Option<Ctx>>,
    scope: Cell<Option<ScopeId>>,
    queue: RefCell<Vec<Idx>>,
    store: RefCell<NamedStore>,
    diagnostics: RefCell<Vec<Diagnostic>>,
    seen: RefCell<HashMap<(&'static str, String), u32>>,
    seq: Cell<u64>,
    epoch: Cell<u64>,
    epochs: Cell<u64>,
    flushing: Cell<bool>,
    propagation_suspended: Cell<bool>,
    reloading: Cell<bool>,
    closures_run: Cell<u64>,
    /// Who the running code belongs to (the view layer's window): set by the app's entry points,
    /// kept by every node and task made meanwhile and restored when it runs, so code runs as
    /// its owner whoever triggered it.
    owner: Cell<Option<Owner>>,
}

/// Who reactive code belongs to: an opaque number the layer above gives meaning (a window).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Owner(pub u64);

/// The current runtime's owner, if any (outside a runtime: none).
pub(crate) fn current_owner() -> Option<Owner> {
    CURRENT.with(|c| c.borrow().as_ref().and_then(|r| r.owner.get()))
}

thread_local! {
    static CURRENT: RefCell<Option<Rc<Runtime>>> = const { RefCell::new(None) };
}

/// The current runtime. Panics outside [`Runtime::enter`].
pub(crate) fn rt<R>(f: impl FnOnce(&Runtime) -> R) -> R {
    f(&Runtime::current())
}

const FLUSH_ROUNDS_LIMIT: u32 = 1000;

fn loc(at: &Location<'_>) -> String {
    format!("{}:{}", crate::diagnostics::source_file(at), at.line())
}

impl Runtime {
    pub(crate) fn current() -> Rc<Self> {
        CURRENT.with(|c| c.borrow().clone()).expect("no reactive runtime: call inside Runtime::enter")
    }

    pub fn new() -> Rc<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let identity = NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1)).expect("runtime identity exhausted");
        Rc::new(Runtime { identity, ..Runtime::default() })
    }

    pub(crate) fn deliver_collections(&self, deliveries: impl IntoIterator<Item = (usize, CollectionDelivery)>) {
        {
            let mut work = self.collections.work.borrow_mut();
            for (depth, delivery) in deliveries {
                work.entry(depth).or_default().push_back(delivery);
            }
        }
        if self.collections.active.replace(true) {
            return;
        }
        // An unwinding user projection must not strand callbacks or leave delivery active.
        let _reset = std::mem::DropGuard::new(&self.collections, |queue| {
            let abandoned = std::mem::take(&mut *queue.work.borrow_mut());
            queue.active.set(false);
            drop(abandoned);
        });
        loop {
            let next = {
                let mut work = self.collections.work.borrow_mut();
                work.first_entry().and_then(|mut entry| {
                    let next = entry.get_mut().pop_front();
                    if entry.get().is_empty() {
                        entry.remove();
                    }
                    next
                })
            };
            let Some(delivery) = next else { break };
            delivery();
        }
    }

    pub fn enter<R>(self: &Rc<Self>, f: impl FnOnce() -> R) -> R {
        struct Restore(Option<Rc<Runtime>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                let prev = self.0.take();
                CURRENT.with(|c| *c.borrow_mut() = prev);
            }
        }
        let _restore = Restore(CURRENT.with(|c| c.replace(Some(self.clone()))));
        f()
    }

    // ---- diagnostics ----

    /// Records a diagnostic once per (code, location); repeats are counted, not stored.
    pub fn diag(&self, code: &'static Code, message: String, at: &Location<'_>) {
        let location = loc(at);
        let mut seen = self.seen.borrow_mut();
        let n = seen.entry((code.id, location.clone())).or_insert(0);
        *n += 1;
        if *n == 1 {
            self.diagnostics.borrow_mut().push(Diagnostic { code, message, location: Some(location) });
        }
    }

    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.diagnostics.borrow().clone()
    }

    /// How many times each stored diagnostic occurred.
    pub fn diagnostic_counts(&self) -> Vec<(&'static str, u32)> {
        self.diagnostics
            .borrow()
            .iter()
            .map(|d| (d.code.id, self.seen.borrow()[&(d.code.id, d.location.clone().unwrap_or_default())]))
            .collect()
    }

    pub fn closures_run(&self) -> u64 {
        self.closures_run.get()
    }
    pub fn reset_closures_run(&self) {
        self.closures_run.set(0);
    }
    pub fn live_nodes(&self) -> u64 {
        self.nodes.borrow().iter().filter(|n| n.alive).count() as u64
    }
    pub fn ctx(&self) -> Ctx {
        self.ctx.get().unwrap_or(Ctx::Idle)
    }

    /// The owner now.
    pub(crate) fn owner(&self) -> Option<Owner> {
        self.owner.get()
    }

    /// Restore the registration window; None deliberately clears the ambient window.
    pub(crate) fn with_owner<R>(&self, owner: Option<Owner>, f: impl FnOnce() -> R) -> R {
        let _owner = Restore::set(&self.owner, owner);
        f()
    }

    pub fn with_frame<R>(&self, ctx: Ctx, observer: Option<Idx>, scope: Option<Option<ScopeId>>, f: impl FnOnce() -> R) -> R {
        let _ctx = Restore::set(&self.ctx, Some(ctx));
        let _observer = Restore::set(&self.observer, observer);
        let _scope = scope.map(|scope| Restore::set(&self.scope, scope));
        let _sampling = Restore::set(&self.sampling, ctx == Ctx::Init);
        f()
    }

    fn sample<R>(&self, f: impl FnOnce() -> R) -> R {
        let _observer = Restore::set(&self.observer, None);
        let _sampling = Restore::set(&self.sampling, true);
        f()
    }

    pub(crate) fn with_scope<R>(&self, scope: Option<ScopeId>, f: impl FnOnce() -> R) -> R {
        let _scope = Restore::set(&self.scope, scope);
        f()
    }

    pub(crate) fn effect_allowed(&self, at: &'static Location<'static>) -> bool {
        if self.scope.get().is_some_and(|scope| !self.scope_alive(scope)) {
            self.diag(&codes::WRITE_DISPOSED, "cannot register work in a retired scope".into(), at);
            return false;
        }
        let code = match self.ctx() {
            Ctx::Derive | Ctx::Observe | Ctx::View => &codes::WRITE_IN_DERIVE,
            Ctx::Init => &codes::WRITE_IN_BUILD,
            Ctx::Cleanup => &codes::WRITE_DISPOSED,
            _ => return true,
        };
        self.diag(code, "effect registration inside a pure computation or initializer; rejected".into(), at);
        false
    }

    pub(crate) fn write_allowed(&self, at: &'static Location<'static>) -> bool {
        let code = match self.ctx() {
            Ctx::Build | Ctx::Init => &codes::WRITE_IN_BUILD,
            Ctx::Derive | Ctx::Observe | Ctx::View => &codes::WRITE_IN_DERIVE,
            _ => return true,
        };
        self.diag(code, "reactive mutation outside a handler or watch callback; rejected".into(), at);
        false
    }

    // ---- nodes ----

    fn alive(&self, idx: Idx) -> bool {
        idx.runtime == self.identity && self.nodes.borrow().get(idx.i as usize).is_some_and(|n| n.alive && n.g == idx.g)
    }
    fn kind(&self, idx: Idx) -> Kind {
        self.nodes.borrow()[idx.i as usize].kind
    }
    fn defined_at(&self, idx: Idx) -> String {
        loc(self.nodes.borrow()[idx.i as usize].defined_at)
    }

    /// `owner`: `None` = the current scope; `Some(None)` = no scope (owned by a value, e.g. a list).
    fn create_node(
        &self,
        kind: Kind,
        value: Option<Value>,
        compute: Option<Compute>,
        named: bool,
        owner: Option<Option<ScopeId>>,
        at: &'static Location<'static>,
    ) -> Idx {
        let seq = self.seq.get();
        self.seq.set(seq + 1);
        let run_as = self.owner.get();
        let scope = owner.unwrap_or(self.scope.get());
        if scope.is_some_and(|scope| !self.scope_alive(scope)) {
            self.diag(&codes::WRITE_DISPOSED, "cannot allocate state in a retired scope".into(), at);
            panic!("[{}] state owner was disposed", codes::WRITE_DISPOSED);
        }
        let make = |g| Node {
            g,
            alive: true,
            kind,
            value,
            compute,
            sources: Vec::new(),
            observers: Vec::new(),
            state: if kind == Kind::Source { State::Clean } else { State::Dirty },
            queued: false,
            computing: false,
            seq,
            named,
            tracked: None,
            defined_at: at,
            owner: run_as,
            scope,
        };
        let idx = {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(i) = self.free_nodes.borrow_mut().pop() {
                let g = nodes[i as usize].g.checked_add(1).expect("node generation exhausted");
                nodes[i as usize] = make(g);
                Idx { runtime: self.identity, i, g }
            } else {
                let i = u32::try_from(nodes.len()).expect("node slots exhausted");
                nodes.push(make(0));
                Idx { runtime: self.identity, i, g: 0 }
            }
        };
        match owner.unwrap_or(self.scope.get()) {
            Some(s) if self.scope_alive(s) => self.scopes.borrow_mut()[s.i as usize].nodes.push(idx),
            None if owner.is_none() && matches!(kind, Kind::Structure | Kind::Binding | Kind::Watch) => {
                self.diag(&codes::NO_OWNER, "an observer was created outside any scope and will never be disposed".into(), at)
            }
            _ => {}
        }
        idx
    }

    fn dispose_node(&self, idx: Idx) {
        if !self.alive(idx) {
            return;
        }
        let (sources, observers, value, compute) = {
            let mut nodes = self.nodes.borrow_mut();
            let n = &mut nodes[idx.i as usize];
            n.alive = false;
            n.queued = false;
            (std::mem::take(&mut n.sources), std::mem::take(&mut n.observers), n.value.take(), n.compute.take())
        };
        {
            let mut nodes = self.nodes.borrow_mut();
            for s in sources {
                if let Some(n) = nodes.get_mut(s.i as usize).filter(|n| n.alive && n.g == s.g) {
                    n.observers.retain(|o| *o != idx);
                }
            }
            for o in &observers {
                if let Some(n) = nodes.get_mut(o.i as usize).filter(|n| n.alive && n.g == o.g) {
                    n.sources.retain(|s| *s != idx);
                }
            }
        }
        // A cached derivation must not outlive a removed dependency's value. In particular,
        // row inputs must re-resolve a removed key even before structural propagation.
        for observer in observers {
            self.mark(observer, State::Dirty);
        }
        self.free_nodes.borrow_mut().push(idx.i);
        // Dropped with no borrow held: a value may dispose nodes it owns.
        drop(value);
        drop(compute);
    }

    fn track(&self, src: Idx, at: &Location<'_>) {
        if self.sampling.get() {
            return;
        }
        if let Some(obs) = self.observer.get() {
            if obs == src {
                return;
            }
            let epoch = self.epoch.get();
            let mut nodes = self.nodes.borrow_mut();
            if nodes[src.i as usize].tracked != Some((obs, epoch)) {
                nodes[src.i as usize].tracked = Some((obs, epoch));
                nodes[obs.i as usize].sources.push(src);
                nodes[src.i as usize].observers.push(obs);
            }
        } else if self.ctx() == Ctx::View {
            let at_def = self.defined_at(src);
            self.diag(
                &codes::VIEW_READS_STATE,
                format!(
                    "value defined at {at_def} read inside a view's closure (filter, sort_by, group_by, map, join); \
                     the view runs per item change and will not follow this value. Put it in the item, join the \
                     data, or pick between views in a memo"
                ),
                at,
            );
        } else if self.ctx() == Ctx::Build {
            let at_def = self.defined_at(src);
            self.diag(
                &codes::STRICT_READ_UNTRACKED,
                format!(
                    "value defined at {at_def} read while building, outside any binding; that UI will not update. \
                     Pass a closure (`move || ..`), or use `untrack(|| value.get())` for a one-shot read"
                ),
                at,
            );
        }
    }

    pub(crate) fn read(&self, idx: Idx, at: &'static Location<'static>, tracked: bool) -> Value {
        if idx.runtime != self.identity {
            self.diag(&codes::WRONG_RUNTIME, "signal belongs to another App".into(), at);
            panic!("[{}] signal belongs to another App", codes::WRONG_RUNTIME);
        }
        if !self.alive(idx) {
            self.diag(&codes::READ_DISPOSED, "read of a value whose owner was disposed".into(), at);
            panic!("[{}] at {}: read of a disposed value", codes::READ_DISPOSED, loc(at));
        }
        if self.kind(idx) == Kind::Memo {
            if self.nodes.borrow()[idx.i as usize].computing {
                self.diag(&codes::CYCLE, format!("memo defined at {} reads itself", self.defined_at(idx)), at);
                panic!("[{}] at {}: memo read itself while computing", codes::CYCLE, loc(at));
            }
            self.update_if_necessary(idx);
        }
        if tracked {
            self.track(idx, at);
        }
        self.nodes.borrow()[idx.i as usize].value.clone().expect("value")
    }

    fn writable_node(&self, idx: Idx, at: &'static Location<'static>, internal: bool) -> Option<Value> {
        if idx.runtime != self.identity {
            self.diag(&codes::WRONG_RUNTIME, "signal belongs to another App; write rejected".into(), at);
            return None;
        }
        if !self.alive(idx) {
            self.diag(&codes::WRITE_DISPOSED, "write to a disposed value; rejected".into(), at);
            return None;
        }
        if self.nodes.borrow()[idx.i as usize].scope.is_some_and(|scope| !self.scope_alive(scope)) {
            self.diag(&codes::WRITE_DISPOSED, "write to retiring state; rejected".into(), at);
            return None;
        }
        if !internal && !self.write_allowed(at) {
            return None;
        }
        self.nodes.borrow()[idx.i as usize].value.clone()
    }

    fn invalidate(&self, idx: Idx) {
        if self.alive(idx) {
            let observers = self.nodes.borrow()[idx.i as usize].observers.clone();
            for observer in observers {
                self.mark(observer, State::Dirty);
            }
        }
    }

    /// The storage cell stays stable so replacing a value cannot evade an active borrow.
    fn write<T: 'static>(&self, idx: Idx, value: T, at: &'static Location<'static>, internal: bool) {
        let Some(cell) = self.writable_node(idx, at, internal) else { return };
        let old = {
            let Ok(mut current) = cell.try_borrow_mut() else {
                self.diag(&codes::SIGNAL_BORROW, "signal is already borrowed; write rejected".into(), at);
                return;
            };
            std::mem::replace(&mut *current, Box::new(value))
        };
        self.invalidate(idx);
        drop(old);
    }

    fn update_value<T: 'static>(&self, idx: Idx, f: impl FnOnce(&mut T), at: &'static Location<'static>) {
        let Some(cell) = self.writable_node(idx, at, false) else { return };
        struct Notify<'a>(&'a Runtime, Idx, bool);
        impl Drop for Notify<'_> {
            fn drop(&mut self) {
                if self.2 {
                    self.0.invalidate(self.1);
                }
            }
        }
        // Drop the value borrow before invalidating, including if user code unwinds.
        let mut notify = Notify(self, idx, false);
        let Ok(mut current) = cell.try_borrow_mut() else {
            self.diag(&codes::SIGNAL_BORROW, "signal is already borrowed; update rejected".into(), at);
            return;
        };
        notify.2 = true;
        f(current.downcast_mut::<T>().expect("reactive value type"));
    }

    fn with_value<T: 'static, R>(&self, idx: Idx, at: &'static Location<'static>, tracked: bool, f: impl FnOnce(&T) -> R) -> R {
        let cell = self.read(idx, at, tracked);
        let current = cell.try_borrow().unwrap_or_else(|_| {
            self.diag(&codes::SIGNAL_BORROW, "signal is mutably borrowed; read rejected".into(), at);
            panic!("[{}] signal is mutably borrowed", codes::SIGNAL_BORROW);
        });
        f(current.downcast_ref::<T>().expect("reactive value type"))
    }

    fn mark(&self, idx: Idx, state: State) {
        let observers = {
            let mut nodes = self.nodes.borrow_mut();
            let Some(n) = nodes.get_mut(idx.i as usize).filter(|n| n.alive && n.g == idx.g) else { return };
            if n.state as u8 >= state as u8 {
                return;
            }
            n.state = state;
            if matches!(n.kind, Kind::Structure | Kind::Binding | Kind::Watch) && !n.queued {
                n.queued = true;
                self.queue.borrow_mut().push(idx);
            }
            n.observers.clone()
        };
        for o in observers {
            self.mark(o, State::Check);
        }
    }

    fn update_if_necessary(&self, idx: Idx) {
        if !self.alive(idx) {
            return;
        }
        if self.nodes.borrow()[idx.i as usize].state == State::Check {
            let sources = self.nodes.borrow()[idx.i as usize].sources.clone();
            for s in sources {
                if self.alive(s) && self.kind(s) == Kind::Memo {
                    self.update_if_necessary(s);
                }
                if self.nodes.borrow()[idx.i as usize].state == State::Dirty {
                    break;
                }
            }
        }
        if !self.alive(idx) {
            return;
        }
        let dirty = std::mem::replace(&mut self.nodes.borrow_mut()[idx.i as usize].state, State::Clean) == State::Dirty;
        if dirty {
            self.run_node(idx);
        }
    }

    fn run_node(&self, idx: Idx) {
        let (kind, mut compute, old, sources, owner, scope) = {
            let mut nodes = self.nodes.borrow_mut();
            let n = &mut nodes[idx.i as usize];
            n.computing = true;
            (n.kind, n.compute.take().expect("compute"), n.value.clone(), std::mem::take(&mut n.sources), n.owner, n.scope)
        };
        {
            let mut nodes = self.nodes.borrow_mut();
            for s in sources {
                if let Some(n) = nodes.get_mut(s.i as usize).filter(|n| n.alive && n.g == s.g) {
                    n.observers.retain(|o| *o != idx);
                }
            }
        }
        struct Run<'a>(&'a Runtime, Idx, bool);
        impl Drop for Run<'_> {
            fn drop(&mut self) {
                if !self.2 {
                    self.0.dispose_node(self.1);
                }
            }
        }
        let mut run = Run(self, idx, false);
        if scope.is_some_and(|scope| !self.scope_alive(scope)) {
            return;
        }
        self.closures_run.set(self.closures_run.get() + 1);
        let ctx = if kind == Kind::Memo { Ctx::Derive } else { Ctx::Observe };
        let epoch = self.epochs.get() + 1;
        self.epochs.set(epoch);
        let _epoch = Restore::set(&self.epoch, epoch);
        let changed = self.with_owner(owner, || self.with_frame(ctx, Some(idx), Some(scope), || compute(old.as_ref())));
        run.2 = true;
        let (observers, replaced) = {
            let mut nodes = self.nodes.borrow_mut();
            let n = &mut nodes[idx.i as usize];
            if !(n.alive && n.g == idx.g) {
                drop(nodes);
                drop(compute);
                return;
            }
            n.computing = false;
            n.compute = Some(compute);
            match changed {
                Some(v) => {
                    let replaced = n.value.replace(v);
                    (n.observers.clone(), replaced)
                }
                None => (Vec::new(), None),
            }
        };
        drop(replaced);
        let mut nodes = self.nodes.borrow_mut();
        for o in observers {
            if let Some(n) = nodes.get_mut(o.i as usize).filter(|n| n.alive && n.g == o.g) {
                n.state = State::Dirty;
            }
        }
    }

    pub fn flush(&self) {
        if self.flushing.get() || self.propagation_suspended.get() {
            return;
        }
        let _flushing = Restore::set(&self.flushing, true);
        let priority = |idx: Idx| {
            let nodes = self.nodes.borrow();
            let n = &nodes[idx.i as usize];
            (
                match n.kind {
                    Kind::Structure => 0,
                    Kind::Binding => 1,
                    _ => 2,
                },
                n.seq,
            )
        };
        let suspend = |idx: Idx| {
            let at = self.nodes.borrow()[idx.i as usize].defined_at;
            self.diag(
                &codes::FLUSH_RUNAWAY,
                format!("updates exceeded {FLUSH_ROUNDS_LIMIT} feedback rounds; still scheduled: {}", loc(at)),
                at,
            );
            self.propagation_suspended.set(true);
        };
        // New watches wait for the next feedback round. Structural/property work can preempt
        // the remaining watches without spending that budget. This also bounds a loop that
        // creates a fresh scope/watch on every turn, rather than counting only node identity.
        let mut rounds = 0;
        loop {
            let mut batch = std::mem::take(&mut *self.queue.borrow_mut());
            if batch.is_empty() {
                break;
            }
            rounds += 1;
            if rounds > FLUSH_ROUNDS_LIMIT {
                suspend(batch[0]);
                self.queue.borrow_mut().extend(batch);
                break;
            }
            batch.sort_by_key(|idx| priority(*idx));
            struct Batch<'a>(&'a Runtime, std::vec::IntoIter<Idx>);
            impl Drop for Batch<'_> {
                fn drop(&mut self) {
                    // Panics and budget exits must not strand observers marked queued.
                    self.0.queue.borrow_mut().extend(self.1.by_ref());
                }
            }
            let mut batch = Batch(self, batch.into_iter());
            let mut waves = 0;
            while let Some(i) = batch.1.next() {
                let queued = std::mem::take(&mut *self.queue.borrow_mut());
                let (mut earlier, later): (Vec<_>, Vec<_>) =
                    queued.into_iter().partition(|idx| self.alive(*idx) && self.kind(*idx) != Kind::Watch && priority(*idx) < priority(i));
                self.queue.borrow_mut().extend(later);
                if !earlier.is_empty() {
                    earlier.push(i);
                    earlier.extend(batch.1.by_ref());
                    earlier.sort_by_key(|idx| priority(*idx));
                    batch.1 = earlier.into_iter();
                    waves += 1;
                    if waves > FLUSH_ROUNDS_LIMIT {
                        suspend(i);
                        return;
                    }
                    continue;
                }
                if self.alive(i) {
                    let effect = self.kind(i) == Kind::Watch;
                    self.nodes.borrow_mut()[i.i as usize].queued = false;
                    self.update_if_necessary(i);
                    if effect {
                        waves = 0;
                    }
                }
            }
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.queue.borrow().iter().any(|idx| self.alive(*idx))
    }

    /// Retry limited propagation at a new outer operation, never inside the same layout loop.
    pub(crate) fn resume_propagation(&self) {
        self.propagation_suspended.set(false);
    }
}
