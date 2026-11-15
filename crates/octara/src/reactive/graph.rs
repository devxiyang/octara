//! The reactive graph (design: `docs/research/reactive-design-v2.md`).
//!
//! Layers and their contexts:
//! - inputs: signals and named state, written only from handlers and `watch` callbacks;
//! - derive: `memo`, pure, tracked, may not write;
//! - observe: property bindings, structural regions and `watch` sources, tracked, may not write;
//! - effects: handlers and `watch` callbacks, untracked, may write.
//!
//! Algorithm: push marking (Dirty on direct observers, Check further down), pull evaluation, memo
//! early cutoff, bindings before watches, creation order. A node is marked Clean *before* it runs,
//! so a re-mark during its own run (a watch writing its source) schedules another run.
//!
//! Invariants: no runtime borrow is held while user code runs or while a value is dropped (values
//! may own other nodes, e.g. a list owns its item signals); handles carry generations.

use crate::diagnostics::{self as codes, Code, Diagnostic};
use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
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

struct ScopeData {
    g: u32,
    alive: bool,
    parent: Option<ScopeId>,
    children: Vec<ScopeId>,
    nodes: Vec<Idx>,
    cleanups: Vec<Box<dyn FnOnce()>>,
    path: String,
    names: HashMap<String, u32>,
    state_keys: HashSet<&'static str>,
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

#[derive(Default)]
pub(crate) struct Runtime {
    identity: u64,
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
    let r = CURRENT.with(|c| c.borrow().clone()).expect("no reactive runtime: call inside Runtime::enter");
    f(&r)
}

const FLUSH_ROUNDS_LIMIT: u32 = 1000;

fn loc(at: &Location<'_>) -> String {
    format!("{}:{}", crate::diagnostics::source_file(at), at.line())
}

impl Runtime {
    pub fn new() -> Rc<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let identity = NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1)).expect("runtime identity exhausted");
        Rc::new(Runtime { identity, ..Runtime::default() })
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
                let g = nodes[i as usize].g + 1;
                nodes[i as usize] = make(g);
                Idx { runtime: self.identity, i, g }
            } else {
                nodes.push(make(0));
                Idx { runtime: self.identity, i: nodes.len() as u32 - 1, g: 0 }
            }
        };
        match owner.unwrap_or(self.scope.get()) {
            Some(s) if self.scope_alive(s) => self.scopes.borrow_mut()[s.i as usize].nodes.push(idx),
            None if owner.is_none() && matches!(kind, Kind::Binding | Kind::Watch) => {
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
            for o in observers {
                if let Some(n) = nodes.get_mut(o.i as usize).filter(|n| n.alive && n.g == o.g) {
                    n.sources.retain(|s| *s != idx);
                }
            }
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
            if matches!(n.kind, Kind::Binding | Kind::Watch) && !n.queued {
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
        let mut rounds = 0;
        loop {
            let mut batch = std::mem::take(&mut *self.queue.borrow_mut());
            if batch.is_empty() {
                break;
            }
            rounds += 1;
            if rounds > FLUSH_ROUNDS_LIMIT {
                let at = self.nodes.borrow()[batch[0].i as usize].defined_at;
                self.diag(
                    &codes::FLUSH_RUNAWAY,
                    format!("updates did not settle after {FLUSH_ROUNDS_LIMIT} rounds; still scheduled: {}", loc(at)),
                    at,
                );
                // Preserve the scheduled work so a later input can break the feedback loop.
                self.queue.borrow_mut().extend(batch);
                self.propagation_suspended.set(true);
                break;
            }
            {
                let nodes = self.nodes.borrow();
                batch.sort_by_key(|i| {
                    let n = &nodes[i.i as usize];
                    (n.kind != Kind::Binding, n.seq)
                });
            }
            struct Batch<'a>(&'a Runtime, std::vec::IntoIter<Idx>);
            impl Drop for Batch<'_> {
                fn drop(&mut self) {
                    // A callback panic must not strand unrelated observers marked queued.
                    self.0.queue.borrow_mut().extend(self.1.by_ref());
                }
            }
            let mut batch = Batch(self, batch.into_iter());
            for i in batch.1.by_ref() {
                if self.alive(i) {
                    self.nodes.borrow_mut()[i.i as usize].queued = false;
                    self.update_if_necessary(i);
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

    // ---- scopes ----

    pub(crate) fn scope_alive(&self, s: ScopeId) -> bool {
        s.runtime == self.identity && self.scopes.borrow().get(s.i as usize).is_some_and(|d| d.alive && d.g == s.g)
    }
    pub fn current_scope(&self) -> Option<ScopeId> {
        self.scope.get()
    }
    pub fn scope_path(&self, s: ScopeId) -> String {
        self.scopes.borrow()[s.i as usize].path.clone()
    }

    /// Registers `name` under `parent`; returns the segment (`name`, then `name#2`, ...).
    pub fn child_name(&self, parent: ScopeId, name: &str) -> String {
        let mut scopes = self.scopes.borrow_mut();
        let n = scopes[parent.i as usize].names.entry(name.to_owned()).or_insert(0);
        *n += 1;
        if *n == 1 { name.to_owned() } else { format!("{name}#{n}") }
    }

    pub fn create_scope(&self, parent: Option<ScopeId>, segment: &str) -> ScopeId {
        let path = match parent {
            Some(p) => format!("{}/{segment}", self.scope_path(p)),
            None => segment.to_owned(),
        };
        let make = |g| ScopeData {
            g,
            alive: true,
            parent,
            children: Vec::new(),
            nodes: Vec::new(),
            cleanups: Vec::new(),
            path,
            names: HashMap::new(),
            state_keys: HashSet::new(),
            contexts: Vec::new(),
            owner: self.owner.get(),
        };
        let id = {
            let mut scopes = self.scopes.borrow_mut();
            if let Some(i) = self.free_scopes.borrow_mut().pop() {
                let g = scopes[i as usize].g + 1;
                scopes[i as usize] = make(g);
                ScopeId { runtime: self.identity, i, g }
            } else {
                scopes.push(make(0));
                ScopeId { runtime: self.identity, i: scopes.len() as u32 - 1, g: 0 }
            }
        };
        if let Some(p) = parent {
            self.scopes.borrow_mut()[p.i as usize].children.push(id);
        }
        id
    }

    pub fn on_cleanup(&self, f: Box<dyn FnOnce()>) {
        if let Some(s) = self.scope.get().filter(|s| self.scope_alive(*s)) {
            self.scopes.borrow_mut()[s.i as usize].cleanups.push(f);
        }
    }

    /// Disposes a scope with its children, cleanups and nodes. During a hot reload named state
    /// stays in the store; otherwise it goes with the scope.
    pub fn dispose_scope(&self, s: ScopeId) {
        if !self.scope_alive(s) {
            return;
        }
        let (children, nodes, cleanups, parent, path, owner) = {
            let mut scopes = self.scopes.borrow_mut();
            let d = &mut scopes[s.i as usize];
            d.alive = false; // Retire the whole owner before invoking child or user teardown.
            (
                std::mem::take(&mut d.children),
                std::mem::take(&mut d.nodes),
                std::mem::take(&mut d.cleanups),
                d.parent,
                d.path.clone(),
                d.owner,
            )
        };
        self.with_owner(owner, || {
            self.with_frame(Ctx::Cleanup, None, Some(Some(s)), || {
                for child in children.into_iter().rev() {
                    self.dispose_scope(child);
                }
                for cleanup in cleanups.into_iter().rev() {
                    cleanup();
                }
                let reloading = self.reloading.get();
                for n in nodes {
                    let named = self.alive(n) && self.nodes.borrow()[n.i as usize].named;
                    if !(named && reloading) {
                        self.dispose_node(n);
                    }
                }
                if !reloading {
                    self.store.borrow_mut().retain(|(p, _), _| p != &path);
                }
                if let Some(p) = parent.filter(|p| self.scope_alive(*p)) {
                    self.scopes.borrow_mut()[p.i as usize].children.retain(|c| *c != s);
                }
                let contexts = std::mem::take(&mut self.scopes.borrow_mut()[s.i as usize].contexts);
                drop(contexts);
                self.free_scopes.borrow_mut().push(s.i);
            })
        });
    }

    fn provide(&self, ty: TypeId, value: Rc<dyn Any>) {
        let s = self.scope.get().expect("provide outside a component");
        let replaced = {
            let mut scopes = self.scopes.borrow_mut();
            let ctx = &mut scopes[s.i as usize].contexts;
            let replaced = ctx.iter().position(|(t, _)| *t == ty).map(|i| ctx.remove(i));
            ctx.push((ty, value));
            replaced
        };
        drop(replaced);
    }

    fn lookup(&self, ty: TypeId) -> Option<Rc<dyn Any>> {
        let scopes = self.scopes.borrow();
        let mut cur = self.scope.get();
        while let Some(s) = cur.filter(|s| {
            s.runtime == self.identity && scopes.get(s.i as usize).is_some_and(|d| d.g == s.g && (d.alive || self.ctx() == Ctx::Cleanup))
        }) {
            let d = &scopes[s.i as usize];
            if let Some((_, v)) = d.contexts.iter().find(|(t, _)| *t == ty) {
                return Some(v.clone());
            }
            cur = d.parent;
        }
        None
    }

    pub fn set_reloading(&self, on: bool) {
        self.reloading.set(on);
    }

    fn named_state(&self, key: &'static str, ty: TypeId, init: impl FnOnce() -> Value, at: &'static Location<'static>) -> Idx {
        let Some(scope) = self.scope.get() else {
            panic!("state(\"{key}\") outside a component at {}", loc(at));
        };
        let path = self.scope_path(scope);
        let duplicate = !self.scopes.borrow_mut()[scope.i as usize].state_keys.insert(key);
        if duplicate {
            self.diag(
                &codes::DUPLICATE_STATE_KEY,
                format!("state key \"{key}\" used twice in {path}; both calls would share one signal"),
                at,
            );
        }
        let existing = self.store.borrow().get(&(path.clone(), key)).copied();
        if let Some((idx, old_ty)) = existing {
            if self.alive(idx) && old_ty == ty {
                self.scopes.borrow_mut()[scope.i as usize].nodes.push(idx);
                self.nodes.borrow_mut()[idx.i as usize].scope = Some(scope);
                return idx;
            }
            // The same key for another type in this build: the first keeps it, this call gets
            // a state of its own (not kept across reloads); the diagnostic above says why.
            if self.alive(idx) && duplicate {
                return self.create_node(Kind::Source, Some(init()), None, true, None, at);
            }
            if self.alive(idx) {
                self.diag(&codes::STATE_TYPE_CHANGED, format!("state \"{key}\" in {path} changed type across a reload; reset"), at);
                self.dispose_node(idx);
            }
        }
        let idx = self.create_node(Kind::Source, Some(init()), None, true, None, at);
        self.store.borrow_mut().insert((path, key), (idx, ty));
        idx
    }
}

// ---------------------------------------------------------------------------------------------
// Handles
// ---------------------------------------------------------------------------------------------

type NotSend = PhantomData<*const ()>;

/// A writable reactive value. `Copy`, `!Send`.
pub struct Signal<T> {
    pub(crate) idx: Idx,
    _t: PhantomData<fn() -> T>,
    _ns: NotSend,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Signal<T> {}
impl<T> PartialEq for Signal<T> {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}
impl<T> std::fmt::Debug for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signal({:?})", self.idx)
    }
}

impl<T> Signal<T> {
    /// Disposes the node. May run from a value's `Drop`; does nothing once the runtime is gone.
    pub(crate) fn dispose(self) {
        if CURRENT.with(|c| c.borrow().is_some()) {
            rt(|rt| rt.dispose_node(self.idx));
        }
    }
}

impl<T: 'static> Signal<T> {
    fn from_idx(idx: Idx) -> Self {
        Signal { idx, _t: PhantomData, _ns: PhantomData }
    }

    /// A free-standing signal owned by the current scope. Inside a component prefer [`state`].
    #[track_caller]
    pub fn new(value: T) -> Self {
        let at = Location::caller();
        rt(|rt| {
            if rt.ctx() == Ctx::Build {
                rt.diag(&codes::UNKEYED_STATE, "Signal::new inside a component: lost on hot reload; use `state(key, init)`".into(), at);
            }
            Self::from_idx(rt.create_node(Kind::Source, Some(value_cell(value)), None, false, None, at))
        })
    }

    /// A framework-internal signal owned by the current scope (no diagnostics).
    #[track_caller]
    pub(crate) fn scoped(value: T) -> Self {
        let at = Location::caller();
        rt(|rt| Self::from_idx(rt.create_node(Kind::Source, Some(value_cell(value)), None, false, None, at)))
    }

    /// A signal owned by a value rather than a scope; the owner must call [`Signal::dispose`].
    #[track_caller]
    pub(crate) fn detached(value: T) -> Self {
        let at = Location::caller();
        rt(|rt| Self::from_idx(rt.create_node(Kind::Source, Some(value_cell(value)), None, false, Some(None), at)))
    }

    #[track_caller]
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.with(Clone::clone)
    }
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let at = Location::caller();
        rt(|rt| rt.with_value(self.idx, at, true, f))
    }
    #[track_caller]
    pub fn set(&self, value: T) {
        let at = Location::caller();
        rt(|rt| rt.write(self.idx, value, at, false));
    }
    #[track_caller]
    pub(crate) fn set_internal(&self, value: T) {
        let at = Location::caller();
        rt(|rt| rt.write(self.idx, value, at, true));
    }
    #[track_caller]
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        let at = Location::caller();
        rt(|rt| rt.update_value(self.idx, f, at));
    }

    pub fn read_only(&self) -> ReadSignal<T> {
        ReadSignal(*self)
    }
}

/// Read-only view of a signal.
pub struct ReadSignal<T>(Signal<T>);

impl<T> Clone for ReadSignal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for ReadSignal<T> {}
impl<T> PartialEq for ReadSignal<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T> std::fmt::Debug for ReadSignal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReadSignal({:?})", self.0.idx)
    }
}

impl<T: 'static> ReadSignal<T> {
    #[track_caller]
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.0.get()
    }
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.0.with(f)
    }
}

/// A derived read-only value: lazy, recomputed when its inputs change, propagated only when
/// it changes.
#[track_caller]
pub fn memo<T: PartialEq + 'static>(f: impl Fn() -> T + 'static) -> ReadSignal<T> {
    let at = Location::caller();
    let compute: Compute = Box::new(move |old| {
        let new = f();
        let equal = old.is_some_and(|old| old.borrow().downcast_ref::<T>().is_some_and(|old| *old == new));
        if equal { None } else { Some(value_cell(new)) }
    });
    ReadSignal(Signal::from_idx(rt(|rt| rt.create_node(Kind::Memo, None, Some(compute), false, None, at))))
}

fn observer(kind: Kind, at: &'static Location<'static>, compute: Compute) -> Idx {
    rt(|rt| {
        let idx = rt.create_node(kind, None, Some(compute), false, None, at);
        rt.nodes.borrow_mut()[idx.i as usize].state = State::Clean;
        rt.run_node(idx);
        idx
    })
}

/// A property binding or structural region (view layer): runs now and on every change.
pub(crate) fn binding(at: &'static Location<'static>, mut f: impl FnMut() + 'static) {
    observer(
        Kind::Binding,
        at,
        Box::new(move |_| {
            f();
            None
        }),
    );
}

/// Stops a scope-owned watch early. Ignoring or dropping this handle keeps the watch active.
/// The handle does not keep its scope alive and must be used inside its App's runtime.
#[derive(Clone, Copy, Debug)]
pub struct Watch {
    idx: Option<Idx>,
    _ns: NotSend,
}

impl Watch {
    /// Detaches dependencies and cancels queued invocations. Repeated stops do nothing;
    /// stopping inside the callback lets that invocation finish and releases its captures
    /// afterwards.
    #[track_caller]
    pub fn stop(&self) {
        let Some(idx) = self.idx else { return };
        let at = Location::caller();
        rt(|rt| {
            if idx.runtime != rt.identity {
                rt.diag(&codes::WRONG_RUNTIME, "watch belongs to another App; stop rejected".into(), at);
                return;
            }
            if rt.ctx() != Ctx::Build && !rt.write_allowed(at) {
                return;
            }
            rt.dispose_node(idx);
        });
    }
}

/// Runs `callback(new, old)` when the value of `source` changes, and once now with `old = None`.
/// Only `source` is tracked. The callback runs like a handler: untracked, and allowed to write,
/// including signals `source` reads; a write that leaves the value unchanged ends there.
#[track_caller]
pub fn watch<S: PartialEq + 'static>(source: impl Fn() -> S + 'static, callback: impl Fn(&S, Option<&S>) + 'static) -> Watch {
    let at = Location::caller();
    if !rt(|rt| rt.effect_allowed(at)) {
        return Watch { idx: None, _ns: PhantomData };
    }
    let mut last = None;
    let idx = observer(
        Kind::Watch,
        at,
        Box::new(move |_| {
            let v = source();
            if last.as_ref() != Some(&v) {
                rt(|r| r.with_frame(Ctx::Handler, None, None, || callback(&v, last.as_ref())));
                last = Some(v);
            }
            None
        }),
    );
    Watch { idx: Some(idx), _ns: PhantomData }
}

/// Named component state: keyed by `key` within the current component; survives hot reload.
#[track_caller]
pub fn state<T: 'static>(key: &'static str, init: impl FnOnce() -> T) -> Signal<T> {
    let at = Location::caller();
    Signal::from_idx(rt(|rt| rt.named_state(key, TypeId::of::<T>(), || rt.with_frame(Ctx::Init, None, None, || value_cell(init())), at)))
}

/// Makes `value` available to every descendant of the current component (the nearest wins).
/// Provide a `Signal` to share a value that changes.
pub fn provide<T: Clone + 'static>(value: T) {
    rt(|rt| rt.provide(TypeId::of::<T>(), Rc::new(value)));
}

/// The nearest value of type `T` provided by an enclosing component.
pub fn use_context<T: Clone + 'static>() -> Option<T> {
    rt(|rt| rt.lookup(TypeId::of::<T>())).and_then(|v| v.downcast_ref::<T>().cloned())
}

/// Runs `f` untracked: reads inside are intentional one-shot reads.
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    rt(|rt| rt.sample(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_scope<R>(f: impl FnOnce(&Rc<Runtime>) -> R) -> R {
        let rt = Runtime::new();
        let s = rt.create_scope(None, "t");
        rt.enter(|| rt.with_frame(Ctx::Idle, None, Some(Some(s)), || f(&rt)))
    }

    fn handler(rt: &Runtime, f: impl FnOnce()) {
        rt.with_frame(Ctx::Handler, None, None, f);
        rt.flush();
    }

    #[test]
    fn diamond_glitch_free_and_cutoff() {
        with_scope(|rt| {
            let s = Signal::new(1);
            let a = memo(move || s.get() * 2);
            let parity = memo(move || s.get() % 2);
            let seen = Rc::new(RefCell::new(Vec::new()));
            let s2 = seen.clone();
            binding(Location::caller(), move || s2.borrow_mut().push((a.get(), parity.get())));
            handler(rt, || s.set(3));
            handler(rt, || s.set(4));
            assert_eq!(*seen.borrow(), vec![(2, 1), (6, 1), (8, 0)]);
        });
    }

    #[test]
    fn watch_clamp_settles_and_callback_is_untracked() {
        with_scope(|rt| {
            let count = Signal::new(0);
            let other = Signal::new(0);
            let calls = Rc::new(Cell::new(0));
            let c = calls.clone();
            watch(
                move || count.get(),
                move |v, _| {
                    other.get(); // untracked: must not subscribe
                    c.set(c.get() + 1);
                    if *v > 3 {
                        count.set(3);
                    }
                },
            );
            for _ in 0..5 {
                handler(rt, || count.update(|n| *n += 1));
            }
            assert_eq!(untrack(|| count.get()), 3);
            let before = calls.get();
            handler(rt, || other.set(1));
            assert_eq!(calls.get(), before, "callback reads are untracked");
            assert!(rt.diagnostics().is_empty());
        });
    }

    #[test]
    fn layer_rules_and_dedupe() {
        with_scope(|rt| {
            let s = Signal::new(0);
            for _ in 0..3 {
                rt.with_frame(Ctx::Build, None, None, || s.set(1));
            }
            let m = memo(move || {
                s.set(2);
                0
            });
            m.get();
            let codes = rt.diagnostic_counts();
            assert_eq!(codes, vec![("WRITE_IN_BUILD", 3), ("WRITE_IN_DERIVE", 1)]);
            assert_eq!(untrack(|| s.get()), 0);
        });
    }

    #[test]
    fn switching_dependencies_drops_the_old_ones() {
        with_scope(|rt| {
            let flag = Signal::new(true);
            let a = Signal::new(1);
            let b = Signal::new(10);
            let runs = Rc::new(Cell::new(0));
            let r = runs.clone();
            binding(Location::caller(), move || {
                r.set(r.get() + 1);
                let _ = if flag.get() { a.get() } else { b.get() };
            });
            handler(rt, || flag.set(false));
            handler(rt, || a.set(2));
            assert_eq!(runs.get(), 2, "a is no longer a dependency");
            handler(rt, || b.set(11));
            assert_eq!(runs.get(), 3);
        });
    }

    #[test]
    fn disposed_scopes_stop_their_observers() {
        let rt = Runtime::new();
        rt.enter(|| {
            let root = rt.create_scope(None, "root");
            let child = rt.create_scope(Some(root), "child");
            let s = rt.with_frame(Ctx::Idle, None, Some(Some(root)), || Signal::new(0));
            let runs = Rc::new(Cell::new(0));
            let r = runs.clone();
            rt.with_frame(Ctx::Idle, None, Some(Some(child)), || {
                binding(Location::caller(), move || {
                    s.get();
                    r.set(r.get() + 1);
                })
            });
            rt.dispose_scope(child);
            handler(&rt, || s.set(1));
            assert_eq!(runs.get(), 1);
        });
    }

    #[test]
    fn runaway_watch_is_cut() {
        with_scope(|rt| {
            let n = Signal::new(0);
            watch(move || n.get(), move |v, _| n.set(v + 1));
            rt.flush();
            assert_eq!(rt.diagnostics()[0].code, "FLUSH_RUNAWAY");
        });
    }

    #[test]
    fn rejected_update_does_not_invoke_user_code() {
        with_scope(|rt| {
            let value = Signal::new(0);
            let called = Cell::new(false);
            rt.with_frame(Ctx::Build, None, None, || {
                value.update(|n| {
                    called.set(true);
                    *n = 7;
                });
            });
            assert!(!called.get());
            assert_eq!(value.get(), 0);
        });
    }

    #[test]
    fn untrack_cannot_grant_write_permission() {
        with_scope(|rt| {
            let value = Signal::new(0);
            let derived = memo(move || {
                untrack(|| {
                    value.set(8);
                    1
                })
            });
            assert_eq!(derived.get(), 1);
            assert_eq!(value.get(), 0);
            assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"));
        });
    }

    #[test]
    fn watch_registration_cannot_escape_a_pure_computation() {
        with_scope(|rt| {
            let value = Signal::new(0);
            let derived = memo(move || {
                watch(|| 1, move |_, _| value.set(9));
                1
            });
            derived.get();
            assert_eq!(value.get(), 0);
            assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"));
        });
    }

    #[test]
    fn foreign_runtime_handle_cannot_mutate_matching_slot() {
        let a = Runtime::new();
        let b = Runtime::new();
        let value = a.enter(|| Signal::new(1));
        b.enter(|| {
            let other = Signal::new(2);
            assert_ne!(value, other);
            value.set(99);
            assert_eq!(other.get(), 2);
        });
        assert_eq!(a.enter(|| value.get()), 1);
    }

    #[test]
    fn execution_context_restores_after_unwind() {
        with_scope(|rt| {
            let scope = rt.current_scope();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                rt.with_frame(Ctx::Derive, None, Some(None), || panic!("test unwind"));
            }));
            assert!(result.is_err());
            assert_eq!(rt.ctx(), Ctx::Idle);
            assert_eq!(rt.current_scope(), scope);
        });
    }

    #[test]
    fn non_clone_state_updates_in_place_and_memo_can_borrow_non_clone_results() {
        #[derive(PartialEq)]
        struct Form {
            names: Vec<String>,
        }
        with_scope(|rt| {
            let form = state("form", || Form { names: vec!["Ada".into()] });
            let names = memo(move || form.with(|f| Form { names: f.names.clone() }));
            let seen = Rc::new(RefCell::new(Vec::new()));
            let out = seen.clone();
            binding(Location::caller(), move || out.borrow_mut().push(names.with(|f| f.names.len())));
            let address = form.with(|f| f as *const Form);
            handler(rt, || form.update(|f| f.names.push("Lin".into())));
            assert_eq!(form.read_only().with(|f| f as *const Form), address);
            assert_eq!(*seen.borrow(), [1, 2]);
            form.set(Form { names: Vec::new() });
            rt.flush();
            assert_eq!(*seen.borrow(), [1, 2, 0]);
        });
    }

    #[test]
    fn borrowed_value_rejects_conflicts_without_borrowing_the_graph() {
        with_scope(|rt| {
            let value = Signal::new(vec![1]);
            let other = Signal::new(0);
            let called = Cell::new(false);
            value.with(|before| {
                other.update(|n| *n += 1);
                value.set(vec![9]);
                value.update(|_| called.set(true));
                assert_eq!(before, &[1]);
                assert_eq!(value.with(Vec::len), 1);
            });
            assert!(!called.get());
            assert_eq!(other.get(), 1);
            assert_eq!(value.get(), [1]);
            assert!(rt.diagnostics().iter().any(|d| d.code == "SIGNAL_BORROW"));
        });
    }

    #[test]
    fn mutation_unwind_releases_borrow_and_invalidates_changed_value() {
        with_scope(|rt| {
            let value = Signal::new(1);
            let doubled = memo(move || value.get() * 2);
            assert_eq!(doubled.get(), 2);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                value.update(|v| {
                    *v = 5;
                    panic!("partially mutated");
                })
            }));
            assert!(result.is_err());
            assert_eq!(doubled.get(), 10, "never leave the old memo clean after mutation");
            assert_eq!(rt.ctx(), Ctx::Idle);
        });
    }

    #[test]
    fn untracked_memo_read_still_tracks_its_own_inputs() {
        with_scope(|rt| {
            let source = Signal::new(1);
            let trigger = Signal::new(0);
            let inner = memo(move || source.get() * 2);
            let outer = memo(move || {
                trigger.get();
                untrack(|| inner.get())
            });
            assert_eq!(outer.get(), 2);
            handler(rt, || source.set(3));
            assert_eq!(outer.get(), 2, "untracked source did not subscribe outer");
            handler(rt, || trigger.set(1));
            assert_eq!(outer.get(), 6, "inner still follows source");
        });
    }

    #[test]
    fn initializers_sample_without_subscribing_or_writing_existing_state() {
        with_scope(|rt| {
            let source = Signal::new(1);
            let initialized = rt.with_frame(Ctx::Build, None, None, || {
                state("initial", || {
                    untrack(|| source.set(7));
                    source.get()
                })
            });
            assert_eq!(source.get(), 1);
            assert_eq!(initialized.get(), 1);
            assert!(!rt.diagnostics().iter().any(|d| d.code == "STRICT_READ_UNTRACKED"));
            assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_BUILD"));
        });
    }

    #[test]
    fn watch_and_cleanup_restore_registration_scope() {
        with_scope(|rt| {
            let parent = rt.current_scope().unwrap();
            let child = rt.create_scope(Some(parent), "child");
            let value = Signal::new(0);
            let seen = Rc::new(RefCell::new(Vec::new()));
            let cleanups = Rc::new(RefCell::new(Vec::new()));
            rt.with_frame(Ctx::Build, None, Some(Some(child)), || {
                provide("child");
                let out = seen.clone();
                watch(move || value.get(), move |_, _| out.borrow_mut().push(use_context::<&str>()));
                let out = cleanups.clone();
                rt.on_cleanup(Box::new(move || out.borrow_mut().push(use_context::<&str>())));
            });
            handler(rt, || value.set(1));
            assert_eq!(*seen.borrow(), [Some("child"), Some("child")]);
            rt.dispose_scope(child);
            handler(rt, || value.set(2));
            assert_eq!(seen.borrow().len(), 2);
            assert_eq!(*cleanups.borrow(), [Some("child")]);
        });
    }

    #[test]
    fn observer_unwind_keeps_the_rest_of_the_ready_batch() {
        with_scope(|rt| {
            let trigger = Signal::new(false);
            let observed = Rc::new(Cell::new(false));
            binding(Location::caller(), move || {
                assert!(!trigger.get(), "failing binding");
            });
            let out = observed.clone();
            binding(Location::caller(), move || out.set(trigger.get()));
            trigger.set(true);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.flush()));
            assert!(result.is_err());
            assert!(!rt.flushing.get());
            assert!(rt.has_pending());
            rt.flush();
            assert!(observed.get());
            assert!(!rt.has_pending());
        });
    }

    #[test]
    fn cleanup_cannot_register_work_after_its_owner_retires() {
        with_scope(|rt| {
            let child = rt.create_scope(rt.current_scope(), "child");
            let called = Rc::new(Cell::new(false));
            let out = called.clone();
            rt.with_scope(Some(child), || {
                rt.on_cleanup(Box::new(move || {
                    watch(|| 1, move |_, _| out.set(true));
                }))
            });
            rt.dispose_scope(child);
            assert!(!called.get());
            assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_DISPOSED"));
        });
    }

    #[test]
    fn replacing_context_drops_user_values_outside_the_scope_table_borrow() {
        struct Probe;
        impl Drop for Probe {
            fn drop(&mut self) {
                rt(|rt| {
                    let _ = rt.scope_path(rt.current_scope().unwrap());
                });
            }
        }
        with_scope(|rt| {
            provide(Rc::new(Probe));
            provide(Rc::new(Probe));
            rt.dispose_scope(rt.current_scope().unwrap());
        });
    }

    #[test]
    fn read_handles_share_identity_without_value_trait_bounds() {
        struct Payload;
        #[derive(PartialEq)]
        struct Derived(usize);
        with_scope(|rt| {
            let source = Signal::new(Payload);
            let read = source.read_only();
            assert_eq!(read, source.read_only());
            assert_ne!(read, Signal::new(Payload).read_only());
            assert!(format!("{read:?}").starts_with("ReadSignal("));
            let runs = Rc::new(Cell::new(0));
            let out = runs.clone();
            let derived: ReadSignal<Derived> = memo(move || {
                source.with(|_| ());
                out.set(out.get() + 1);
                Derived(42)
            });
            assert_eq!(runs.get(), 0, "the unified read handle keeps memo evaluation lazy");
            assert_eq!(derived.with(|value| value.0), 42);
            assert_eq!(runs.get(), 1);
            handler(rt, || source.set(Payload));
            assert_eq!(derived.with(|value| value.0), 42);
            assert_eq!(runs.get(), 2);
        });
    }

    #[test]
    fn non_clone_watch_borrows_previous_results_and_cuts_off_equal_values() {
        #[derive(PartialEq)]
        struct Snapshot(i32);
        with_scope(|rt| {
            let source = Signal::new(0);
            let seen = Rc::new(RefCell::new(Vec::new()));
            let out = seen.clone();
            let _ = watch(move || Snapshot(source.get() / 2), move |new, old| out.borrow_mut().push((new.0, old.map(|old| old.0))));
            handler(rt, || source.set(1));
            handler(rt, || source.set(2));
            handler(rt, || source.set(3));
            handler(rt, || source.set(4));
            assert_eq!(*seen.borrow(), [(0, None), (1, Some(0)), (2, Some(1))]);
            assert!(rt.diagnostics().is_empty());
        });
    }

    #[test]
    fn watch_stop_cancels_queued_work_and_cannot_stop_a_reused_slot() {
        with_scope(|rt| {
            let source = Signal::new(0);
            let calls = Rc::new(Cell::new(0));
            let out = calls.clone();
            let stopped = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
            source.set(1);
            assert!(rt.has_pending());
            stopped.stop();
            stopped.stop();
            assert!(!rt.has_pending(), "stopping cancels pending invocations");
            rt.flush();
            assert_eq!(calls.get(), 1);
            let replacement_calls = Rc::new(Cell::new(0));
            let out = replacement_calls.clone();
            let _ = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
            stopped.stop();
            handler(rt, || source.set(2));
            assert_eq!(calls.get(), 1);
            assert_eq!(replacement_calls.get(), 2);
        });
    }

    #[test]
    fn watch_can_stop_itself_without_dropping_its_running_callback() {
        struct Capture(Rc<Cell<bool>>);
        impl Drop for Capture {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        with_scope(|rt| {
            let source = Signal::new(0);
            let handle = Rc::new(Cell::new(None::<Watch>));
            let dropped = Rc::new(Cell::new(false));
            let calls = Rc::new(Cell::new(0));
            let (stop, out) = (handle.clone(), calls.clone());
            let capture = Capture(dropped.clone());
            let watch = watch(
                move || source.get(),
                move |new, old| {
                    assert!(!capture.0.get(), "the callback's capture remains alive while it runs");
                    out.set(out.get() + 1);
                    if *new == 1 {
                        assert_eq!(old, Some(&0));
                        source.set(2); // Queue another round before stopping in this callback.
                        stop.get().unwrap().stop();
                        assert!(!capture.0.get());
                    }
                },
            );
            handle.set(Some(watch));
            handler(rt, || source.set(1));
            assert_eq!(calls.get(), 2);
            assert!(dropped.get(), "stopped callback captures are released after it returns");
            assert!(!rt.has_pending());
            handler(rt, || source.set(3));
            assert_eq!(calls.get(), 2);
        });
    }

    #[test]
    fn watch_scope_disposal_releases_non_clone_results_and_queued_callbacks() {
        struct Snapshot(Rc<Cell<usize>>);
        impl PartialEq for Snapshot {
            fn eq(&self, _: &Self) -> bool {
                true
            }
        }
        impl Drop for Snapshot {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        with_scope(|rt| {
            let source = Signal::new(0);
            let child = rt.create_scope(rt.current_scope(), "watch owner");
            let dropped = Rc::new(Cell::new(0));
            let calls = Rc::new(Cell::new(0));
            let (snapshot, out) = (dropped.clone(), calls.clone());
            let stop = rt.with_scope(Some(child), || {
                watch(
                    move || {
                        source.get();
                        Snapshot(snapshot.clone())
                    },
                    move |_, _| out.set(out.get() + 1),
                )
            });
            source.set(1);
            rt.dispose_scope(child);
            assert_eq!(dropped.get(), 1, "the retained source result leaves with its scope");
            assert!(!rt.has_pending());
            stop.stop();
            handler(rt, || source.set(2));
            assert_eq!(calls.get(), 1);
        });
    }

    #[test]
    fn foreign_runtime_watch_stop_does_not_stop_either_runtime() {
        let a = Runtime::new();
        let b = Runtime::new();
        let a_scope = a.create_scope(None, "a");
        let b_scope = b.create_scope(None, "b");
        let a_calls = Rc::new(Cell::new(0));
        let b_calls = Rc::new(Cell::new(0));
        let out = a_calls.clone();
        let (a_source, a_watch) = a.enter(|| {
            a.with_scope(Some(a_scope), || {
                let source = Signal::new(0);
                let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
                (source, watch)
            })
        });
        let out = b_calls.clone();
        b.enter(|| {
            b.with_scope(Some(b_scope), || {
                let source = Signal::new(0);
                let _ = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
                a_watch.stop();
                handler(&b, || source.set(1));
                assert_eq!(b_calls.get(), 2);
                assert!(b.diagnostics().iter().any(|d| d.code == "WRONG_RUNTIME"));
            })
        });
        a.enter(|| {
            handler(&a, || a_source.set(1));
            assert_eq!(a_calls.get(), 2, "a foreign stop leaves the original watch active");
            a_watch.stop();
            handler(&a, || a_source.set(2));
            assert_eq!(a_calls.get(), 2);
        });
    }

    #[test]
    fn untrack_cannot_stop_a_watch_from_a_pure_computation() {
        with_scope(|rt| {
            let source = Signal::new(0);
            let calls = Rc::new(Cell::new(0));
            let out = calls.clone();
            let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
            let derived = memo(move || untrack(|| watch.stop()));
            derived.get();
            handler(rt, || source.set(1));
            assert_eq!(calls.get(), 2);
            assert!(rt.diagnostics().iter().any(|d| d.code == "WRITE_IN_DERIVE"));
        });
    }

    #[test]
    fn watch_stop_is_allowed_during_build_and_cleanup() {
        with_scope(|rt| {
            let source = Signal::new(0);
            let calls = Rc::new(Cell::new(0));
            let out = calls.clone();
            rt.with_frame(Ctx::Build, None, None, || {
                let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
                watch.stop();
            });
            handler(rt, || source.set(1));
            assert_eq!(calls.get(), 1);
            let out = calls.clone();
            let watch = watch(move || source.get(), move |_, _| out.set(out.get() + 1));
            let child = rt.create_scope(rt.current_scope(), "stop from cleanup");
            rt.with_scope(Some(child), || rt.on_cleanup(Box::new(move || watch.stop())));
            source.set(2);
            rt.dispose_scope(child);
            rt.flush();
            assert_eq!(calls.get(), 2, "a retiring child can stop another live watch");
            assert!(!rt.has_pending());
            assert!(rt.diagnostics().is_empty(), "{:?}", rt.diagnostics());
        });
    }
}
