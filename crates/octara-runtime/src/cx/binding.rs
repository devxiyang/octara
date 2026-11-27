use super::{BuildCx, ReadCx, WriteCx};
use crate::Signal;
use std::rc::Rc;

type Reader<T> = Box<dyn Fn(&mut ReadCx<'_>) -> T>;
type Writer<T> = Box<dyn Fn(&mut WriteCx<'_>, T)>;

struct Accessors<T> {
    read: Reader<T>,
    write: Writer<T>,
}

/// A copyable handle to a controlled value. A binding reads in a read context
/// and submits a proposed value in a write context.
pub struct Binding<T>(Source<T>);

enum Source<T> {
    Signal(Signal<T>),
    Accessors(Signal<Rc<Accessors<T>>>),
}

impl<T> Clone for Binding<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Binding<T> {}

impl<T> Clone for Source<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Source<T> {}

impl<T> From<Signal<T>> for Binding<T> {
    fn from(value: Signal<T>) -> Self {
        Self(Source::Signal(value))
    }
}

impl<T: 'static> Binding<T> {
    /// Whether this handle's value or custom accessor is still owned by the graph.
    pub fn is_alive(self, write: &WriteCx<'_>) -> bool {
        write.world.is_alive(match self.0 {
            Source::Signal(signal) => signal.id(),
            Source::Accessors(accessors) => accessors.id(),
        })
    }

    /// Borrow a signal's value or the result of a custom reader.
    pub fn with<R>(self, read: &mut ReadCx<'_>, f: impl FnOnce(&T) -> R) -> R {
        match self.0 {
            Source::Signal(signal) => read.with(signal, f),
            Source::Accessors(handle) => {
                let accessors = read.with(handle, Rc::clone);
                f(&(accessors.read)(read))
            }
        }
    }

    pub fn get(self, read: &mut ReadCx<'_>) -> T
    where
        T: Clone,
    {
        match self.0 {
            Source::Signal(signal) => read.get(signal),
            Source::Accessors(handle) => {
                let accessors = read.with(handle, Rc::clone);
                (accessors.read)(read)
            }
        }
    }

    /// Read the current value once at construction.
    pub fn sample(self, build: &mut BuildCx<'_>) -> T
    where
        T: Clone,
    {
        build.read(|read| self.get(read))
    }

    /// Submit a value to the model. A custom writer controls acceptance.
    pub fn set(self, write: &mut WriteCx<'_>, value: T) {
        match self.0 {
            Source::Signal(signal) => write.set(signal, value),
            Source::Accessors(handle) => {
                let accessors = write.with(handle, Rc::clone);
                (accessors.write)(write, value);
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
        Binding(Source::Accessors(self.signal(Rc::new(Accessors { read: Box::new(read), write: Box::new(write) }))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReactiveWorld;

    #[test]
    fn binding_reads_and_writes_through_its_owning_scope() {
        let mut world = ReactiveWorld::new();
        let root = world.root_scope();
        let value = world.build(root, None).signal(8);
        let scope = world.new_scope(root);
        let custom = world
            .build(scope, None)
            .binding(move |read| read.get(value) + 1, move |write, proposed| write.set(value, proposed.clamp(0, 10)));
        let direct = Binding::from(value);

        let before = world.live_nodes();
        assert_eq!(custom.sample(&mut world.build(scope, None)), 9);
        assert_eq!(world.live_nodes(), before);

        let memo = world.build(scope, None).memo(move |read| custom.get(read));
        let mut write = world.write(scope, None);
        direct.set(&mut write, 4);
        assert_eq!(write.get(memo), 5);
        custom.set(&mut write, 70);
        assert_eq!(write.get(value), 10);
        assert_eq!(write.get(memo), 11);

        world.dispose(scope);
        assert_eq!(world.live_nodes(), 1);
        assert_eq!(direct.get(&mut world.read()), 10);
        assert!(world.validate_integrity());
    }
}
