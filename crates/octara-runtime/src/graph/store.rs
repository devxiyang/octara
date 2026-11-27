//! Struct state with revisions for individual fields and aggregate reads.

use std::any::Any;
use std::fmt;
use std::marker::PhantomData;
use std::ops::Deref;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use smallvec::SmallVec;

use crate::arena::{NodeKind, ReactiveId, ReactiveWorld};
use crate::cx::{BuildCx, PreparedRead, ReadCx, Readable, Writable, WriteCx};

/// A scope-owned struct whose fields can be read and written independently.
///
/// `#[derive(Store)]` generates the field accessors. A field annotated with
/// `#[store(nested)]` is itself a store. Reading an aggregate tracks changes
/// to all of its fields, and replacing one compares the leaf fields by
/// reference. An in-place aggregate update invalidates the entire subtree
/// when it reports a change. Use a field handle to edit that field alone.
pub struct Store<T: StoreSchema>(T::Accessors);

impl<T: StoreSchema> Store<T> {
    #[doc(hidden)]
    pub fn from_field(field: Field<T>) -> Self {
        Self(T::Accessors::from_field(field))
    }
}

impl<T: StoreSchema> Copy for Store<T> {}

impl<T: StoreSchema> Clone for Store<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: StoreSchema> fmt::Debug for Store<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Store").field(&self.0.as_field()).finish()
    }
}

impl<T: StoreSchema> Deref for Store<T> {
    type Target = T::Accessors;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: StoreSchema> Readable<T> for Store<T> {
    type Prepared = Field<T>;

    fn prepare(self, read: &mut ReadCx<'_>) -> Self::Prepared {
        self.0.as_field().prepare(read)
    }
}

impl<T: StoreSchema> Writable<T> for Store<T> {
    fn write(self, cx: &mut WriteCx<'_>, value: T) {
        self.0.as_field().write(cx, value);
    }

    fn edit<R>(self, cx: &mut WriteCx<'_>, f: impl FnOnce(&mut T) -> (R, bool)) -> R {
        self.0.as_field().edit(cx, f)
    }
}

/// A borrowed projection of a store's value with its own reactive revision.
pub struct Field<T> {
    root: ReactiveId,
    index: usize,
    storage: &'static StoreStorage,
    marker: PhantomData<fn() -> T>,
}

impl<T> Copy for Field<T> {}

impl<T> Clone for Field<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> fmt::Debug for Field<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field").field("root", &self.root).field("index", &self.index).finish()
    }
}

impl<T: 'static> Field<T> {
    #[doc(hidden)]
    pub fn child<U: 'static>(self, offset: usize) -> Field<U> {
        Field { root: self.root, index: self.index + offset, storage: self.storage, marker: PhantomData }
    }

    fn data(self, world: &ReactiveWorld) -> (&[StoreNode], &dyn Any) {
        (self.storage.read)(world, self.root)
    }

    fn data_mut(self, world: &mut ReactiveWorld) -> (&[StoreNode], &mut dyn Any) {
        (self.storage.write)(world, self.root)
    }

    fn notify_ancestors(self, world: &mut ReactiveWorld) {
        let mut index = self.index;
        while index > 0 {
            let (nodes, _) = self.data(world);
            index = nodes[index].parent;
            let id = nodes[index].id;
            world.mark_changed(id);
        }
    }

    fn notify_subtree(self, world: &mut ReactiveWorld) {
        let end = self.data(world).0[self.index].end;
        for index in self.index..end {
            let id = self.data(world).0[index].id;
            world.mark_changed(id);
        }
        self.notify_ancestors(world);
    }
}

impl<T: 'static> Readable<T> for Field<T> {
    type Prepared = Self;

    fn prepare(self, read: &mut ReadCx<'_>) -> Self {
        let id = self.data(read.world).0[self.index].id;
        read.track_id(id);
        self
    }
}

impl<T: 'static> PreparedRead<T> for Field<T> {
    fn value<'a>(&'a self, world: &'a ReactiveWorld) -> Option<&'a T> {
        let (nodes, value) = self.data(world);
        Some(project(nodes, self.index, value).downcast_ref().expect("store field type mismatch"))
    }
}

impl<T: 'static> Writable<T> for Field<T> {
    #[track_caller]
    fn write(self, cx: &mut WriteCx<'_>, value: T) {
        let (nodes, root) = self.data_mut(cx.world);
        let current = project_mut(nodes, self.index, root).downcast_mut::<T>().expect("store field type mismatch");
        let mut changed = SmallVec::new();
        collect_changes(nodes, self.index, current, &value, &mut changed);
        let previous = std::mem::replace(current, value);
        if !changed.is_empty() {
            for id in changed {
                cx.world.mark_changed(id);
            }
            self.notify_ancestors(cx.world);
        }
        // A value's destructor can panic. Publish any revisions before dropping it.
        drop(previous);
    }

    #[track_caller]
    fn edit<R>(self, cx: &mut WriteCx<'_>, f: impl FnOnce(&mut T) -> (R, bool)) -> R {
        let (nodes, root) = self.data_mut(cx.world);
        let value = project_mut(nodes, self.index, root).downcast_mut::<T>().expect("store field type mismatch");
        match catch_unwind(AssertUnwindSafe(|| f(value))) {
            Ok((result, changed)) => {
                if changed {
                    self.notify_subtree(cx.world);
                }
                result
            }
            Err(panic) => {
                self.notify_subtree(cx.world);
                resume_unwind(panic);
            }
        }
    }
}

/// The schema generated by `#[derive(Store)]`.
#[doc(hidden)]
pub trait StoreSchema: Sized + 'static {
    type Accessors: StoreAccessors<Self>;
    const OFFSETS: &'static [usize];
    const NODE_COUNT: usize = Self::OFFSETS[Self::OFFSETS.len() - 1];

    fn define(builder: &mut StoreBuilder<'_, '_>);
}

#[doc(hidden)]
pub trait StoreAccessors<T>: Copy {
    fn from_field(field: Field<T>) -> Self;
    fn as_field(self) -> Field<T>;
}

type Project = fn(&dyn Any) -> &dyn Any;
type ProjectMut = fn(&mut dyn Any) -> &mut dyn Any;
type Diff = fn(&dyn Any, &dyn Any) -> bool;

struct StoreNode {
    id: ReactiveId,
    parent: usize,
    end: usize,
    read: Project,
    write: ProjectMut,
    differs: Option<Diff>,
}

struct StoreValue<T> {
    nodes: Vec<StoreNode>,
    value: T,
}

// Keep the aggregate in its typed slab even when a field handle only knows
// the type of its projection. These functions are shared by all stores of T.
struct StoreStorage {
    read: fn(&ReactiveWorld, ReactiveId) -> (&[StoreNode], &dyn Any),
    write: fn(&mut ReactiveWorld, ReactiveId) -> (&[StoreNode], &mut dyn Any),
}

fn storage<T: 'static>() -> &'static StoreStorage {
    &StoreStorage {
        read: |world, root| {
            let state = world.value::<StoreValue<T>>(root);
            (&state.nodes, &state.value)
        },
        write: |world, root| {
            let state = world.value_mut::<StoreValue<T>>(root);
            (&state.nodes, &mut state.value)
        },
    }
}

#[doc(hidden)]
pub struct StoreBuilder<'a, 'world> {
    cx: &'a mut BuildCx<'world>,
    nodes: Vec<StoreNode>,
    parent: usize,
}

impl StoreBuilder<'_, '_> {
    pub fn leaf<T: PartialEq + 'static>(&mut self, read: Project, write: ProjectMut) {
        self.push(
            read,
            write,
            Some(|a, b| {
                let a = a.downcast_ref::<T>().expect("store field type mismatch");
                let b = b.downcast_ref::<T>().expect("store field type mismatch");
                a != b
            }),
        );
    }

    pub fn nested<T: StoreSchema>(&mut self, read: Project, write: ProjectMut) {
        let index = self.push(read, write, None);
        let parent = std::mem::replace(&mut self.parent, index);
        T::define(self);
        self.parent = parent;
        self.nodes[index].end = self.nodes.len();
    }

    fn push(&mut self, read: Project, write: ProjectMut, differs: Option<Diff>) -> usize {
        let id = self.cx.world.alloc_node(NodeKind::ValueSource, self.cx.scope, self.cx.window);
        let index = self.nodes.len();
        self.nodes.push(StoreNode { id, parent: self.parent, end: index + 1, read, write, differs });
        index
    }
}

impl BuildCx<'_> {
    /// Creates a reactive store from a struct declared with `#[derive(Store)]`.
    #[track_caller]
    pub fn store<T: StoreSchema>(&mut self, value: T) -> Store<T> {
        let root = self.world.alloc_slot::<StoreValue<T>>(NodeKind::ValueSource, self.scope, self.window, None);
        let mut nodes = Vec::with_capacity(T::NODE_COUNT);
        nodes.push(StoreNode { id: root, parent: 0, end: T::NODE_COUNT, read: |value| value, write: |value| value, differs: None });
        let mut builder = StoreBuilder { cx: self, nodes, parent: 0 };
        T::define(&mut builder);
        let StoreBuilder { nodes, .. } = builder;
        debug_assert_eq!(T::NODE_COUNT, nodes.len());
        *self.world.value_slot_mut(root) = Some(StoreValue { nodes, value });
        Store::from_field(Field { root, index: 0, storage: storage::<T>(), marker: PhantomData })
    }
}

fn project<'a>(nodes: &[StoreNode], index: usize, value: &'a dyn Any) -> &'a dyn Any {
    if index == 0 {
        return value;
    }
    let node = &nodes[index];
    (node.read)(project(nodes, node.parent, value))
}

fn project_mut<'a>(nodes: &[StoreNode], index: usize, value: &'a mut dyn Any) -> &'a mut dyn Any {
    if index == 0 {
        return value;
    }
    let node = &nodes[index];
    (node.write)(project_mut(nodes, node.parent, value))
}

fn collect_changes(nodes: &[StoreNode], index: usize, old: &dyn Any, new: &dyn Any, changed: &mut SmallVec<[ReactiveId; 8]>) -> bool {
    let node = &nodes[index];
    let differs = if let Some(differs) = node.differs {
        differs(old, new)
    } else {
        let mut differs = false;
        let mut index = index + 1;
        while index < node.end {
            let child = &nodes[index];
            differs |= collect_changes(nodes, index, (child.read)(old), (child.read)(new), changed);
            index = child.end;
        }
        differs
    };
    if differs {
        changed.push(node.id);
    }
    differs
}
