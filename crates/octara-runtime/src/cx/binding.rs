use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use super::{BuildCx, PreparedRead, ReadCx, Readable, Writable, WriteCx};
use crate::arena::NodeKind;
use crate::{ReactiveId, ReactiveWorld, ReadSignal, Signal};

type Reader<T> = Box<dyn Fn(&mut ReadCx<'_>) -> T>;
type Writer<T> = Box<dyn Fn(&mut WriteCx<'_>, T)>;

struct Accessors<T> {
    read: Option<Reader<T>>,
    write: Option<Writer<T>>,
}

/// A copyable input to a controlled value. Read it through a read context and
/// submit proposals through a write context. A custom reader or writer
/// cannot invoke itself while it is running.
pub struct Binding<T>(Source<T>);

enum Source<T> {
    Signal(Signal<T>),
    Accessors(ReactiveId),
}

impl<T> Copy for Binding<T> {}
impl<T> Clone for Binding<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Source<T> {}
impl<T> Clone for Source<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> From<Signal<T>> for Binding<T> {
    fn from(signal: Signal<T>) -> Self {
        Self(Source::Signal(signal))
    }
}

impl<T> Binding<T> {
    fn id(self) -> ReactiveId {
        match self.0 {
            Source::Signal(signal) => signal.id(),
            Source::Accessors(id) => id,
        }
    }
}

#[doc(hidden)]
pub enum PreparedBinding<T> {
    Signal(ReadSignal<T>),
    Owned(T),
}

impl<T: 'static> PreparedRead<T> for PreparedBinding<T> {
    fn value<'a>(&'a self, world: &'a ReactiveWorld) -> Option<&'a T> {
        match self {
            Self::Signal(prepared) => prepared.value(world),
            Self::Owned(value) => Some(value),
        }
    }

    fn into_value(self, world: &ReactiveWorld) -> Option<T>
    where
        T: Clone,
    {
        match self {
            Self::Signal(prepared) => prepared.into_value(world),
            Self::Owned(value) => Some(value),
        }
    }
}

impl<T: 'static> Readable<T> for Binding<T> {
    type Prepared = PreparedBinding<T>;

    fn prepare(self, read: &mut ReadCx<'_>) -> Self::Prepared {
        match self.0 {
            Source::Signal(signal) => PreparedBinding::Signal(signal.prepare(read)),
            Source::Accessors(id) => {
                read.track_id(id);
                let reader = read.world.value_mut::<Accessors<T>>(id).read.take().expect("recursive binding reader");
                let result = catch_unwind(AssertUnwindSafe(|| reader(read)));
                read.world.value_mut::<Accessors<T>>(id).read = Some(reader);
                PreparedBinding::Owned(result.unwrap_or_else(|panic| resume_unwind(panic)))
            }
        }
    }
}

impl<T: 'static> Writable<T> for Binding<T> {
    fn write(self, cx: &mut WriteCx<'_>, value: T) {
        match self.0 {
            Source::Signal(signal) => signal.write(cx, value),
            Source::Accessors(id) => {
                let writer = cx.world.value_mut::<Accessors<T>>(id).write.take().expect("recursive binding writer");
                let result = catch_unwind(AssertUnwindSafe(|| writer(cx, value)));
                cx.world.value_mut::<Accessors<T>>(id).write = Some(writer);
                result.unwrap_or_else(|panic| resume_unwind(panic));
            }
        }
    }

    fn edit<R>(self, cx: &mut WriteCx<'_>, edit: impl FnOnce(&mut T) -> (R, bool)) -> R {
        match self.0 {
            Source::Signal(signal) => signal.edit(cx, edit),
            Source::Accessors(_) => {
                let PreparedBinding::Owned(mut value) = cx.read(|read| self.prepare(read)) else {
                    unreachable!();
                };
                let (result, changed) = edit(&mut value);
                if changed {
                    self.write(cx, value);
                }
                result
            }
        }
    }
}

impl BuildCx<'_> {
    /// Bind a controlled value to app-specific read and write logic owned by this scope.
    #[track_caller]
    pub fn binding<T: 'static>(
        &mut self,
        read: impl Fn(&mut ReadCx<'_>) -> T + 'static,
        write: impl Fn(&mut WriteCx<'_>, T) + 'static,
    ) -> Binding<T> {
        let accessors = Accessors { read: Some(Box::new(read)), write: Some(Box::new(write)) };
        Binding(Source::Accessors(self.world.alloc_value(NodeKind::ValueSource, self.scope, self.window, accessors)))
    }
}

impl WriteCx<'_> {
    /// Whether this binding's source still belongs to a live scope.
    pub fn is_binding_alive<T>(&self, binding: Binding<T>) -> bool {
        self.world.is_alive(binding.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_read_and_write_both_forms_of_binding() {
        let mut world = ReactiveWorld::new();
        let root = world.root_scope();
        let value = world.build(root, None).signal(8);
        let scope = world.new_scope(root);
        let custom = world
            .build(scope, None)
            .binding(move |read| read.get(value) + 1, move |write, proposed| write.set(value, proposed.clamp(0, 10)));
        let direct = Binding::from(value);

        let live = world.live_nodes();
        assert_eq!(world.build(scope, None).sample(custom), 9);
        assert_eq!(world.live_nodes(), live);
        assert_eq!(world.read().with2(custom, direct, |a, b| (*a, *b)), (9, 8));

        let memo = world.build(scope, None).memo(move |read| read.get(custom));
        let mut write = world.write(scope, None);
        write.set(direct, 4);
        assert_eq!(write.get(memo), 5);
        assert!(!write.mutate(custom, |_| false));
        assert_eq!(write.get(direct), 4);
        write.update(custom, |value| *value += 1);
        assert_eq!(write.get(direct), 6);
        assert!(!write.set_if_changed(custom, 7));
        write.set(custom, 70);
        assert_eq!(write.get(value), 10);
        assert_eq!(write.get(memo), 11);

        world.dispose(scope);
        assert_eq!(world.live_nodes(), 1);
        assert!(world.write(root, None).is_binding_alive(direct));
        assert!(!world.write(root, None).is_binding_alive(custom));
        assert_eq!(world.read().get(direct), 10);
        assert!(world.validate_integrity());
    }

    #[test]
    fn panicking_accessors_are_restored_before_unwinding() {
        let mut world = ReactiveWorld::new();
        let scope = world.root_scope();
        let mut build = world.build(scope, None);
        let value = build.signal(-1);
        let binding = build.binding(
            move |read| {
                let current = read.get(value);
                assert!(current >= 0);
                current
            },
            move |write, next| {
                assert!(next >= 0);
                write.set(value, next);
            },
        );
        assert!(catch_unwind(AssertUnwindSafe(|| world.read().get(binding))).is_err());
        let mut write = world.write(scope, None);
        assert!(catch_unwind(AssertUnwindSafe(|| write.set(binding, -2))).is_err());
        write.set(binding, 4);
        assert_eq!(write.get(binding), 4);
    }
}
