//! Signal handles, derivations, observations and public reactive helpers.

use super::*;

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
    /// Releases a value-owned node in its creating runtime, without changing the current App.
    pub(crate) fn dispose_in(self, runtime: &Runtime) {
        runtime.dispose_node(self.idx);
    }

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
/// it changes. Only `f` collects dependencies; result comparison and retirement do not.
#[track_caller]
pub fn memo<T: PartialEq + 'static>(f: impl Fn() -> T + 'static) -> ReadSignal<T> {
    memo_with(f, PartialEq::eq)
}

/// Internal derived inputs need not implement value equality (for example a row containing
/// an opaque resource). They still use the same lazy graph and scope ownership as memos.
#[track_caller]
pub(crate) fn memo_with<T: 'static>(f: impl Fn() -> T + 'static, equal: impl Fn(&T, &T) -> bool + 'static) -> ReadSignal<T> {
    let at = Location::caller();
    let compute: Compute = Box::new(move |old| {
        let new = f();
        untrack(|| {
            let equal = old.is_some_and(|old| old.borrow().downcast_ref::<T>().is_some_and(|old| equal(old, &new)));
            if equal { None } else { Some(value_cell(new)) }
        })
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

/// A structural region runs before property bindings and effects of retained descendants.
pub(crate) fn structural_binding(at: &'static Location<'static>, mut f: impl FnMut() + 'static) {
    observer(
        Kind::Structure,
        at,
        Box::new(move |_| {
            f();
            None
        }),
    );
}

/// A property binding (view layer): runs now and on every change.
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
    /// afterwards. Tasks started by the callback belong to its registration scope, not this
    /// watch: stopping the watch does not cancel them. Cancel their Task handles explicitly
    /// when replacement or early cancellation is required.
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
/// Result comparison and retirement are untracked and retain the source's read-only permissions.
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
            untrack(|| {
                if last.as_ref() != Some(&v) {
                    rt(|r| r.with_frame(Ctx::Handler, None, None, || callback(&v, last.as_ref())));
                    last = Some(v);
                }
                None
            })
        }),
    );
    Watch { idx: Some(idx), _ns: PhantomData }
}

/// Named state in the current scope. Components declare keys once per build and retain values
/// across hot reload. In App::run, same-key/type lookups reuse one value until App shutdown;
/// conflicting types are rejected without replacing it. The initializer only runs for new state.
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
