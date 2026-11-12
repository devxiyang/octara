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

/// A node handle (opaque).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub(crate) struct Idx {
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

type Value = Rc<dyn Any>;
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
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ScopeId {
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
    /// Inside `untrack`.
    Untracked,
    /// Inside a view's closure (`filter`, `sort_by`, `group_by`, `map`, `join`): a pure function
    /// of the item, run per item event; reads are not tracked and are reported.
    View,
}

/// Named state by (scope path, key), with the value's type.
type NamedStore = HashMap<(String, &'static str), (Idx, TypeId)>;

#[derive(Default)]
pub(crate) struct Runtime {
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
    reloading: Cell<bool>,
    closures_run: Cell<u64>,
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
        Rc::new(Runtime::default())
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

    pub fn with_frame<R>(&self, ctx: Ctx, observer: Option<Idx>, scope: Option<Option<ScopeId>>, f: impl FnOnce() -> R) -> R {
        let prev_ctx = self.ctx.replace(Some(ctx));
        let prev_obs = self.observer.replace(observer);
        let prev_scope = scope.map(|s| self.scope.replace(s));
        let r = f();
        self.ctx.set(prev_ctx);
        self.observer.set(prev_obs);
        if let Some(s) = prev_scope {
            self.scope.set(s);
        }
        r
    }

    // ---- nodes ----

    fn alive(&self, idx: Idx) -> bool {
        self.nodes.borrow().get(idx.i as usize).is_some_and(|n| n.alive && n.g == idx.g)
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
        };
        let idx = {
            let mut nodes = self.nodes.borrow_mut();
            if let Some(i) = self.free_nodes.borrow_mut().pop() {
                let g = nodes[i as usize].g + 1;
                nodes[i as usize] = make(g);
                Idx { i, g }
            } else {
                nodes.push(make(0));
                Idx { i: nodes.len() as u32 - 1, g: 0 }
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
                     Pass a closure (`move || ..`), or use `get_untracked()` for a one-shot read"
                ),
                at,
            );
        }
    }

    pub(crate) fn read(&self, idx: Idx, at: &'static Location<'static>, tracked: bool) -> Value {
        if !self.alive(idx) {
            self.diag(&codes::READ_DISPOSED, "read of a value whose owner was disposed".into(), at);
            panic!("[{}] at {}: read of a disposed value", codes::READ_DISPOSED, loc(at));
        }
        if self.kind(idx) == Kind::Memo {
            if self.nodes.borrow()[idx.i as usize].computing {
                self.diag(&codes::CYCLE, format!("memo defined at {} reads itself", self.defined_at(idx)), at);
                if let Some(v) = self.nodes.borrow()[idx.i as usize].value.clone() {
                    return v;
                }
                panic!("[{}] at {}: memo read itself before it had a value", codes::CYCLE, loc(at));
            }
            self.update_if_necessary(idx);
        }
        if tracked {
            self.track(idx, at);
        }
        self.nodes.borrow()[idx.i as usize].value.clone().expect("value")
    }

    /// `internal` writes come from the framework (list views) and skip the layer checks.
    pub(crate) fn write(&self, idx: Idx, value: Value, at: &'static Location<'static>, internal: bool) {
        if !self.alive(idx) {
            self.diag(&codes::WRITE_DISPOSED, "write to a value whose owner was disposed; dropped".into(), at);
            return;
        }
        if !internal {
            let reject = match self.ctx() {
                Ctx::Build => Some((&codes::WRITE_IN_BUILD, "while building a view")),
                Ctx::Derive | Ctx::Observe => Some((&codes::WRITE_IN_DERIVE, "inside a memo, binding or watch source")),
                _ => None,
            };
            if let Some((code, where_)) = reject {
                let def = self.defined_at(idx);
                self.diag(
                    code,
                    format!("write to signal defined at {def} {where_}; rejected. Write from a handler or a `watch` callback"),
                    at,
                );
                return;
            }
        }
        let (observers, old) = {
            let mut nodes = self.nodes.borrow_mut();
            let n = &mut nodes[idx.i as usize];
            (n.observers.clone(), n.value.replace(value))
        };
        drop(old);
        for o in observers {
            self.mark(o, State::Dirty);
        }
    }

    pub(crate) fn update_value(
        &self,
        idx: Idx,
        f: &mut dyn FnMut(&mut dyn Any),
        clone: fn(&dyn Any) -> Value,
        at: &'static Location<'static>,
    ) {
        if !self.alive(idx) {
            return self.write(idx, Rc::new(()), at, false);
        }
        let current = self.nodes.borrow()[idx.i as usize].value.clone().expect("value");
        let mut owned = clone(&*current);
        drop(current);
        f(Rc::get_mut(&mut owned).expect("fresh value"));
        self.write(idx, owned, at, false);
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
        let (kind, mut compute, old, sources) = {
            let mut nodes = self.nodes.borrow_mut();
            let n = &mut nodes[idx.i as usize];
            n.computing = true;
            (n.kind, n.compute.take().expect("compute"), n.value.clone(), std::mem::take(&mut n.sources))
        };
        {
            let mut nodes = self.nodes.borrow_mut();
            for s in sources {
                if let Some(n) = nodes.get_mut(s.i as usize).filter(|n| n.alive && n.g == s.g) {
                    n.observers.retain(|o| *o != idx);
                }
            }
        }
        self.closures_run.set(self.closures_run.get() + 1);
        let ctx = if kind == Kind::Memo { Ctx::Derive } else { Ctx::Observe };
        let epoch = self.epochs.get() + 1;
        self.epochs.set(epoch);
        let prev_epoch = self.epoch.replace(epoch);
        let changed = self.with_frame(ctx, Some(idx), None, || compute(old.as_ref()));
        self.epoch.set(prev_epoch);
        let observers = {
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
                    n.value = Some(v);
                    n.observers.clone()
                }
                None => Vec::new(),
            }
        };
        let mut nodes = self.nodes.borrow_mut();
        for o in observers {
            if let Some(n) = nodes.get_mut(o.i as usize).filter(|n| n.alive && n.g == o.g) {
                n.state = State::Dirty;
            }
        }
    }

    pub fn flush(&self) {
        if self.flushing.replace(true) {
            return;
        }
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
                let mut nodes = self.nodes.borrow_mut();
                for i in batch {
                    nodes[i.i as usize].queued = false;
                }
                break;
            }
            {
                let nodes = self.nodes.borrow();
                batch.sort_by_key(|i| {
                    let n = &nodes[i.i as usize];
                    (n.kind != Kind::Binding, n.seq)
                });
            }
            for i in batch {
                if self.alive(i) {
                    self.nodes.borrow_mut()[i.i as usize].queued = false;
                    self.update_if_necessary(i);
                }
            }
        }
        self.flushing.set(false);
    }

    // ---- scopes ----

    fn scope_alive(&self, s: ScopeId) -> bool {
        self.scopes.borrow().get(s.i as usize).is_some_and(|d| d.alive && d.g == s.g)
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
        };
        let id = {
            let mut scopes = self.scopes.borrow_mut();
            if let Some(i) = self.free_scopes.borrow_mut().pop() {
                let g = scopes[i as usize].g + 1;
                scopes[i as usize] = make(g);
                ScopeId { i, g }
            } else {
                scopes.push(make(0));
                ScopeId { i: scopes.len() as u32 - 1, g: 0 }
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
        let children = std::mem::take(&mut self.scopes.borrow_mut()[s.i as usize].children);
        for c in children.into_iter().rev() {
            self.dispose_scope(c);
        }
        let (nodes, cleanups, parent, path) = {
            let mut scopes = self.scopes.borrow_mut();
            let d = &mut scopes[s.i as usize];
            d.alive = false;
            (std::mem::take(&mut d.nodes), std::mem::take(&mut d.cleanups), d.parent, d.path.clone())
        };
        for f in cleanups.into_iter().rev() {
            f();
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
        self.free_scopes.borrow_mut().push(s.i);
    }

    fn provide(&self, ty: TypeId, value: Rc<dyn Any>) {
        let s = self.scope.get().expect("provide outside a component");
        let mut scopes = self.scopes.borrow_mut();
        let ctx = &mut scopes[s.i as usize].contexts;
        ctx.retain(|(t, _)| *t != ty);
        ctx.push((ty, value));
    }

    fn lookup(&self, ty: TypeId) -> Option<Rc<dyn Any>> {
        let scopes = self.scopes.borrow();
        let mut cur = self.scope.get();
        while let Some(s) = cur.filter(|s| scopes.get(s.i as usize).is_some_and(|d| d.alive && d.g == s.g)) {
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

fn downcast<T: 'static>(v: Value) -> Rc<T> {
    v.downcast::<T>().unwrap_or_else(|_| panic!("reactive value has an unexpected type"))
}

fn clone_value<T: Clone + 'static>(v: &dyn Any) -> Value {
    Rc::new(v.downcast_ref::<T>().expect("type").clone())
}

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

impl<T: Clone + 'static> Signal<T> {
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
            Self::from_idx(rt.create_node(Kind::Source, Some(Rc::new(value)), None, false, None, at))
        })
    }

    /// A framework-internal signal owned by the current scope (no diagnostics).
    #[track_caller]
    pub(crate) fn scoped(value: T) -> Self {
        let at = Location::caller();
        rt(|rt| Self::from_idx(rt.create_node(Kind::Source, Some(Rc::new(value)), None, false, None, at)))
    }

    /// A signal owned by a value rather than a scope; the owner must call [`Signal::dispose`].
    #[track_caller]
    pub(crate) fn detached(value: T) -> Self {
        let at = Location::caller();
        rt(|rt| Self::from_idx(rt.create_node(Kind::Source, Some(Rc::new(value)), None, false, Some(None), at)))
    }

    #[track_caller]
    pub fn get(&self) -> T {
        let at = Location::caller();
        (*downcast::<T>(rt(|rt| rt.read(self.idx, at, true)))).clone()
    }
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let at = Location::caller();
        f(&downcast::<T>(rt(|rt| rt.read(self.idx, at, true))))
    }
    #[track_caller]
    pub fn get_untracked(&self) -> T {
        let at = Location::caller();
        (*downcast::<T>(rt(|rt| rt.read(self.idx, at, false)))).clone()
    }
    #[track_caller]
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let at = Location::caller();
        f(&downcast::<T>(rt(|rt| rt.read(self.idx, at, false))))
    }
    #[track_caller]
    pub fn set(&self, value: T) {
        let at = Location::caller();
        rt(|rt| rt.write(self.idx, Rc::new(value), at, false));
    }
    #[track_caller]
    pub(crate) fn set_internal(&self, value: T) {
        let at = Location::caller();
        rt(|rt| rt.write(self.idx, Rc::new(value), at, true));
    }
    #[track_caller]
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        let at = Location::caller();
        let mut f = Some(f);
        rt(|rt| {
            rt.update_value(self.idx, &mut |v: &mut dyn Any| (f.take().unwrap())(v.downcast_mut::<T>().unwrap()), clone_value::<T>, at)
        });
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

impl<T: Clone + 'static> ReadSignal<T> {
    #[track_caller]
    pub fn get(&self) -> T {
        self.0.get()
    }
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.0.with(f)
    }
    #[track_caller]
    pub fn get_untracked(&self) -> T {
        self.0.get_untracked()
    }
    #[track_caller]
    pub fn with_untracked<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        self.0.with_untracked(f)
    }
}

/// A derived value: lazy, recomputed when its inputs change, propagated only when it changes.
pub struct Memo<T> {
    idx: Idx,
    _t: PhantomData<fn() -> T>,
    _ns: NotSend,
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Memo<T> {}

impl<T: Clone + 'static> Memo<T> {
    #[track_caller]
    pub fn get(&self) -> T {
        let at = Location::caller();
        (*downcast::<T>(rt(|rt| rt.read(self.idx, at, true)))).clone()
    }
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let at = Location::caller();
        f(&downcast::<T>(rt(|rt| rt.read(self.idx, at, true))))
    }
    #[track_caller]
    pub fn get_untracked(&self) -> T {
        let at = Location::caller();
        (*downcast::<T>(rt(|rt| rt.read(self.idx, at, false)))).clone()
    }
}

#[track_caller]
pub fn memo<T: Clone + PartialEq + 'static>(f: impl Fn() -> T + 'static) -> Memo<T> {
    let at = Location::caller();
    let compute: Compute = Box::new(move |old| {
        let new = f();
        match old.and_then(|o| o.downcast_ref::<T>()) {
            Some(o) if *o == new => None,
            _ => Some(Rc::new(new) as Value),
        }
    });
    Memo { idx: rt(|rt| rt.create_node(Kind::Memo, None, Some(compute), false, None, at)), _t: PhantomData, _ns: PhantomData }
}

fn observer(kind: Kind, at: &'static Location<'static>, compute: Compute) {
    rt(|rt| {
        let idx = rt.create_node(kind, None, Some(compute), false, None, at);
        rt.nodes.borrow_mut()[idx.i as usize].state = State::Clean;
        rt.run_node(idx);
    });
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

/// Runs `callback(new, old)` when the value of `source` changes, and once now with `old = None`.
/// Only `source` is tracked. The callback runs like a handler: untracked, and allowed to write,
/// including signals `source` reads; a write that leaves the value unchanged ends there.
#[track_caller]
pub fn watch<S: Clone + PartialEq + 'static>(source: impl Fn() -> S + 'static, callback: impl Fn(&S, Option<&S>) + 'static) {
    let at = Location::caller();
    let last: RefCell<Option<S>> = RefCell::new(None);
    observer(
        Kind::Watch,
        at,
        Box::new(move |_| {
            let v = source();
            let prev = last.borrow().clone();
            if prev.as_ref() != Some(&v) {
                *last.borrow_mut() = Some(v.clone());
                rt(|r| r.with_frame(Ctx::Handler, None, None, || callback(&v, prev.as_ref())));
            }
            None
        }),
    );
}

/// Named component state: keyed by `key` within the current component; survives hot reload.
#[track_caller]
pub fn state<T: Clone + 'static>(key: &'static str, init: impl FnOnce() -> T) -> Signal<T> {
    let at = Location::caller();
    Signal::from_idx(rt(|rt| rt.named_state(key, TypeId::of::<T>(), || Rc::new(init()) as Value, at)))
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
    rt(|rt| rt.with_frame(Ctx::Untracked, None, None, f))
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
            assert_eq!(count.get_untracked(), 3);
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
            assert_eq!(s.get_untracked(), 0);
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
}
