//! Context-driven access to reactive values.

use super::{ReadCx, WriteCx};
use crate::ReactiveWorld;

/// A copyable input that can be read through a capability context.
pub trait Readable<T: 'static>: Copy + 'static {
    #[doc(hidden)]
    type Prepared: PreparedRead<T>;

    #[doc(hidden)]
    #[track_caller]
    fn prepare(self, read: &mut ReadCx<'_>) -> Self::Prepared;
}

/// A value made current by a read context. Preparing both inputs before borrowing
/// lets a context lend multiple values at once.
#[doc(hidden)]
pub trait PreparedRead<T: 'static>: Sized {
    fn value<'a>(&'a self, world: &'a ReactiveWorld) -> Option<&'a T>;

    fn into_value(self, world: &ReactiveWorld) -> Option<T>
    where
        T: Clone,
    {
        self.value(world).cloned()
    }
}

/// An input that accepts writes through a write context.
pub trait Writable<T: 'static>: Readable<T> {
    #[doc(hidden)]
    #[track_caller]
    fn write(self, cx: &mut WriteCx<'_>, value: T);

    #[doc(hidden)]
    #[track_caller]
    fn edit<R>(self, cx: &mut WriteCx<'_>, edit: impl FnOnce(&mut T) -> (R, bool)) -> R;

    #[doc(hidden)]
    #[track_caller]
    fn write_if_changed(self, cx: &mut WriteCx<'_>, value: T) -> bool
    where
        T: PartialEq,
    {
        if cx.with(self, |current| *current == value) {
            return false;
        }
        self.write(cx, value);
        true
    }
}
