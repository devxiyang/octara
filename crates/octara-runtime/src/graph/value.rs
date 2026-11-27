use std::fmt;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use octara_model::identity::WindowId;

use crate::arena::{NodeKind, ReactiveId, ReactiveWorld, ScopeId};
use crate::cx::{PreparedRead, ReadCx, Readable, Writable, WriteCx};

/// A writable value owned by a scope. Reads and writes are performed through a context.
pub struct Signal<T> {
    id: ReactiveId,
    value: PhantomData<fn() -> T>,
}

/// A value that can be read through a context, including memoized values.
pub struct ReadSignal<T> {
    id: ReactiveId,
    value: PhantomData<fn() -> T>,
}

/// The read handle of a memoized computation.
pub type Memo<T> = ReadSignal<T>;

macro_rules! value_handle {
    ($name:ident) => {
        impl<T> Copy for $name<T> {}

        impl<T> Clone for $name<T> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<T> PartialEq for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.id == other.id
            }
        }

        impl<T> Eq for $name<T> {}

        impl<T> std::hash::Hash for $name<T> {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.id.hash(state);
            }
        }

        impl<T> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.id).finish()
            }
        }

        impl<T> $name<T> {
            pub(crate) const fn new(id: ReactiveId) -> Self {
                Self { id, value: PhantomData }
            }

            pub const fn id(self) -> ReactiveId {
                self.id
            }
        }
    };
}

value_handle!(Signal);
value_handle!(ReadSignal);

impl<T> Signal<T> {
    pub const fn read_only(self) -> ReadSignal<T> {
        ReadSignal::new(self.id)
    }
}

impl<T> From<Signal<T>> for ReadSignal<T> {
    fn from(signal: Signal<T>) -> Self {
        signal.read_only()
    }
}

const _: () = assert!(size_of::<Signal<i32>>() == 8);
const _: () = assert!(size_of::<ReadSignal<i32>>() == 8);

impl<T: 'static> Readable<T> for ReadSignal<T> {
    type Prepared = Self;

    fn prepare(self, read: &mut ReadCx<'_>) -> Self::Prepared {
        read.track_id(self.id);
        self
    }
}

impl<T: 'static> PreparedRead<T> for ReadSignal<T> {
    fn value<'a>(&'a self, world: &'a ReactiveWorld) -> Option<&'a T> {
        Some(world.value(self.id))
    }
}

impl<T: 'static> Readable<T> for Signal<T> {
    type Prepared = ReadSignal<T>;

    fn prepare(self, read: &mut ReadCx<'_>) -> Self::Prepared {
        self.read_only().prepare(read)
    }
}

impl<T: 'static> Writable<T> for Signal<T> {
    fn write(self, cx: &mut WriteCx<'_>, value: T) {
        let old = std::mem::replace(cx.world.value_mut(self.id), value);
        cx.world.mark_changed(self.id);
        drop(old);
    }

    fn write_if_changed(self, cx: &mut WriteCx<'_>, value: T) -> bool
    where
        T: PartialEq,
    {
        if cx.world.value_mut::<T>(self.id) == &value {
            return false;
        }
        self.write(cx, value);
        true
    }

    fn edit<R>(self, cx: &mut WriteCx<'_>, edit: impl FnOnce(&mut T) -> (R, bool)) -> R {
        let value = cx.world.value_mut(self.id);
        let result = catch_unwind(AssertUnwindSafe(|| edit(value)));
        if result.as_ref().map_or(true, |(_, changed)| *changed) {
            cx.world.mark_changed(self.id);
        }
        result.unwrap_or_else(|panic| resume_unwind(panic)).0
    }
}

/// A scope-owned notification with no associated value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Trigger(ReactiveId);

impl Trigger {
    pub(crate) const fn new(id: ReactiveId) -> Self {
        Self(id)
    }

    pub const fn id(self) -> ReactiveId {
        self.0
    }
}

const _: () = assert!(size_of::<Trigger>() == 8);

/// A scope-owned watch registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Watch(ReactiveId);

impl Watch {
    pub const fn id(self) -> ReactiveId {
        self.0
    }
}

impl ReactiveWorld {
    #[track_caller]
    pub(crate) fn memo<T: PartialEq + 'static>(
        &mut self,
        scope: ScopeId,
        window: Option<WindowId>,
        mut f: impl FnMut(&mut ReadCx<'_>) -> T + 'static,
    ) -> ReadSignal<T> {
        let id = self.alloc_slot(NodeKind::Memo, scope, window, None::<T>);
        self.init_compute(id, move |r| {
            let value = f(r);
            let slot = r.world.value_slot_mut(id);
            if slot.as_ref() == Some(&value) {
                return false;
            }
            *slot = Some(value);
            true
        });
        ReadSignal::new(id)
    }

    #[track_caller]
    pub(crate) fn memo_with<T: 'static>(
        &mut self,
        scope: ScopeId,
        window: Option<WindowId>,
        init: T,
        mut compute: impl FnMut(&mut ReadCx<'_>, &mut T) -> bool + 'static,
    ) -> ReadSignal<T> {
        let id = self.alloc_value(NodeKind::Memo, scope, window, init);
        self.init_compute(id, move |read| {
            // Keep the value outside the slab while the computation pulls other nodes.
            // Moving ownership preserves any storage already owned by the value.
            let mut value = read.world.value_slot_mut::<T>(id).take().expect("memo has a value");
            let result = catch_unwind(AssertUnwindSafe(|| compute(read, &mut value)));
            *read.world.value_slot_mut(id) = Some(value);
            result.unwrap_or_else(|panic| resume_unwind(panic))
        });
        ReadSignal::new(id)
    }

    #[track_caller]
    pub(crate) fn watch<T: PartialEq + 'static>(
        &mut self,
        scope: ScopeId,
        window: Option<WindowId>,
        mut source: impl FnMut(&mut ReadCx<'_>) -> T + 'static,
        mut effect: impl FnMut(&mut WriteCx<'_>, &T, Option<&T>) + 'static,
    ) -> Watch {
        let id = self.alloc_value(NodeKind::Watch, scope, window, WatchValue::<T> { delivered: None, pending: None });
        self.init_compute(id, move |r| {
            let value = source(r);
            let state = r.world.value_mut::<WatchValue<T>>(id);
            let current = state.pending.as_ref().or(state.delivered.as_ref());
            if current == Some(&value) {
                return false;
            }
            state.pending = Some(value);
            true
        });
        self.cold_mut(id).watch_effect = Some(Box::new(move |w, id| {
            let state = w.world.value_mut::<WatchValue<T>>(id);
            let value = state.pending.take().expect("changed watch value");
            let delivered = state.delivered.take();
            if delivered.as_ref() != Some(&value) {
                effect(w, &value, delivered.as_ref());
            }
            w.world.value_mut::<WatchValue<T>>(id).delivered = Some(value);
        }));
        Watch(id)
    }
}

struct WatchValue<T> {
    delivered: Option<T>,
    pending: Option<T>,
}
