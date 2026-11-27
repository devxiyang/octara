mod scope;
mod slab;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::Location;
use std::sync::atomic::{AtomicU32, Ordering};

use octara_model::diagnostics::{Diagnostic, codes};
use octara_model::identity::WindowId;
use smallvec::SmallVec;

use self::scope::{PersistedEntry, ScopeData};
use self::slab::SlabStore;
use crate::cx::{BuildCx, ReadCx, WriteCx};
use crate::graph::FlushTier;

pub use self::scope::{PersistedState, ScopeId};

static NEXT_RUNTIME: AtomicU32 = AtomicU32::new(1);

/// Identifies one allocation in one reactive world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReactiveId {
    runtime: u32,
    pub(crate) node: u32,
    generation: u32,
    slot: u32,
}

impl ReactiveId {
    pub(crate) const MIN: Self = Self { runtime: 0, node: 0, generation: 0, slot: 0 };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    ValueSource,
    Memo,
    ListSource,
    ListOp,
    RowSource,
    TextBuffer,
    Structure,
    Binding,
    Watch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeState {
    Clean,
    Check,
    Dirty,
}

pub(crate) type Compute = Box<dyn FnMut(&mut ReadCx<'_>) -> bool>;
pub(crate) type WatchEffect = Box<dyn FnMut(&mut WriteCx<'_>, ReactiveId)>;

#[derive(Clone, Copy)]
pub(crate) struct Source {
    pub id: ReactiveId,
    pub revision: u64,
    pub fixed: bool,
}

pub(crate) struct ReactiveNode {
    id: ReactiveId,
    alive: bool,
    // The node header supplies the slab bucket while validating a 16-byte handle.
    bucket: Option<u16>,
    pub kind: NodeKind,
    pub state: NodeState,
    pub computing: bool,
    pub depth: u32,
    pub revision: u64,
    scope: ScopeId,
    window: Option<WindowId>,
    scope_index: usize,
    pub sources: SmallVec<[Source; 4]>,
    pub observers: SmallVec<[ReactiveId; 4]>,
    pub compute: Option<Compute>,
    pub watch_effect: Option<WatchEffect>,
    pub watch_pending: bool,
    persisted_key: Option<String>,
    pub defined_at: &'static Location<'static>,
}

/// Owns the value and delta graph, scopes, and the state they contain.
pub struct ReactiveWorld {
    runtime: u32,
    slabs: SlabStore,
    nodes: Vec<ReactiveNode>,
    free_nodes: Vec<u32>,
    scopes: Vec<ScopeData>,
    free_scopes: Vec<u32>,
    persisted: PersistedState,
    live_persisted: BTreeMap<String, PersistedEntry>,
    pub(crate) text_streams: BTreeSet<ReactiveId>,
    pub(crate) pending: BTreeSet<(FlushTier, u32, ReactiveId)>,
    pub(crate) watch_effects: VecDeque<ReactiveId>,
    diagnostics: Vec<Diagnostic>,
}

impl Default for ReactiveWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl ReactiveWorld {
    pub fn new() -> Self {
        let runtime =
            NEXT_RUNTIME.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("reactive runtime IDs exhausted");
        let mut world = Self {
            runtime,
            slabs: SlabStore::default(),
            nodes: Vec::new(),
            free_nodes: Vec::new(),
            scopes: Vec::new(),
            free_scopes: Vec::new(),
            persisted: PersistedState::new(),
            live_persisted: BTreeMap::new(),
            text_streams: BTreeSet::new(),
            pending: BTreeSet::new(),
            watch_effects: VecDeque::new(),
            diagnostics: Vec::new(),
        };
        world.scopes.push(ScopeData::new(world.root_scope(), None, 0, String::new()));
        world
    }

    #[track_caller]
    pub fn build(&mut self, scope: ScopeId, window: Option<WindowId>) -> BuildCx<'_> {
        self.check_scope(scope);
        BuildCx::new(self, scope, window)
    }

    pub fn read(&mut self) -> ReadCx<'_> {
        ReadCx::new(self)
    }

    #[track_caller]
    pub fn write(&mut self, scope: ScopeId, window: Option<WindowId>) -> WriteCx<'_> {
        self.check_scope(scope);
        WriteCx::new(self, scope, window)
    }

    pub fn live_nodes(&self) -> usize {
        self.nodes.iter().filter(|node| node.alive).count()
    }

    /// Check that retained reactive references resolve to live allocations in both directions.
    pub fn validate_integrity(&self) -> bool {
        self.nodes.iter().filter(|node| node.alive).all(|node| {
            self.is_scope_alive(node.scope)
                && self.scopes[node.scope.index as usize].nodes.get(node.scope_index) == Some(&node.id)
                && node.sources.iter().all(|source| self.is_alive(source.id) && self.node(source.id).observers.contains(&node.id))
                && node
                    .observers
                    .iter()
                    .all(|&observer| self.is_alive(observer) && self.node(observer).sources.iter().any(|source| source.id == node.id))
        }) && self.scopes.iter().all(|scope| scope.nodes.iter().all(|&id| self.is_alive(id)))
    }

    pub fn is_alive(&self, id: ReactiveId) -> bool {
        id.runtime == self.runtime && self.nodes.get(id.node as usize).is_some_and(|node| node.alive && node.id == id)
    }

    #[track_caller]
    pub fn owner(&self, id: ReactiveId) -> (ScopeId, Option<WindowId>) {
        let node = self.node(id);
        (node.scope, node.window)
    }

    #[track_caller]
    pub(crate) fn node(&self, id: ReactiveId) -> &ReactiveNode {
        self.check_node(id, &codes::READ_DISPOSED);
        &self.nodes[id.node as usize]
    }

    #[track_caller]
    pub(crate) fn node_mut(&mut self, id: ReactiveId) -> &mut ReactiveNode {
        self.check_node(id, &codes::WRITE_DISPOSED);
        &mut self.nodes[id.node as usize]
    }

    #[track_caller]
    fn check_node(&self, id: ReactiveId, disposed: &'static octara_model::diagnostics::Code) {
        assert_eq!(id.runtime, self.runtime, "{}: reactive handle belongs to another runtime", codes::WRONG_RUNTIME);
        assert!(self.is_alive(id), "{disposed}: reactive node {id:?} is no longer live");
    }

    #[track_caller]
    pub(crate) fn alloc_node(&mut self, kind: NodeKind, scope: ScopeId, window: Option<WindowId>) -> ReactiveId {
        self.alloc(kind, scope, window, |_, _| (None, u32::MAX))
    }

    #[track_caller]
    pub(crate) fn alloc_value<T: 'static>(&mut self, kind: NodeKind, scope: ScopeId, window: Option<WindowId>, value: T) -> ReactiveId {
        self.alloc_slot(kind, scope, window, Some(value))
    }

    #[track_caller]
    pub(crate) fn alloc_slot<T: 'static>(
        &mut self,
        kind: NodeKind,
        scope: ScopeId,
        window: Option<WindowId>,
        value: Option<T>,
    ) -> ReactiveId {
        self.alloc(kind, scope, window, |slabs, generation| {
            let (bucket, slot) = slabs.insert(generation, value);
            (Some(bucket), slot)
        })
    }

    #[track_caller]
    fn alloc(
        &mut self,
        kind: NodeKind,
        scope: ScopeId,
        window: Option<WindowId>,
        init: impl FnOnce(&mut SlabStore, u32) -> (Option<u16>, u32),
    ) -> ReactiveId {
        self.check_scope(scope);
        let spare = self.free_nodes.pop();
        let (index, generation) = if let Some(index) = spare {
            let generation = self.nodes[index as usize].id.generation.checked_add(1).expect("reactive node generation exhausted");
            (index, generation)
        } else {
            (u32::try_from(self.nodes.len()).expect("reactive node IDs exhausted"), 0)
        };
        let (bucket, slot) = init(&mut self.slabs, generation);
        let id = ReactiveId { runtime: self.runtime, node: index, generation, slot };
        let scope_data = &mut self.scopes[scope.index as usize];
        let scope_index = scope_data.nodes.len();
        scope_data.nodes.push(id);
        let node = ReactiveNode {
            id,
            alive: true,
            bucket,
            kind,
            state: NodeState::Clean,
            computing: false,
            depth: 0,
            revision: 0,
            scope,
            window,
            scope_index,
            sources: SmallVec::new(),
            observers: SmallVec::new(),
            compute: None,
            watch_effect: None,
            watch_pending: false,
            persisted_key: None,
            defined_at: Location::caller(),
        };
        if spare.is_some() {
            self.nodes[index as usize] = node;
        } else {
            self.nodes.push(node);
        }
        id
    }

    #[track_caller]
    pub(crate) fn value<T: 'static>(&self, id: ReactiveId) -> &T {
        let bucket = self.node(id).bucket.expect("reactive node has no value");
        self.slabs.get(bucket, id.slot, id.generation)
    }

    #[track_caller]
    pub(crate) fn value_mut<T: 'static>(&mut self, id: ReactiveId) -> &mut T {
        self.value_slot_mut(id).as_mut().expect("occupied typed slab slot")
    }

    #[track_caller]
    pub(crate) fn value_slot_mut<T: 'static>(&mut self, id: ReactiveId) -> &mut Option<T> {
        self.check_node(id, &codes::WRITE_DISPOSED);
        let bucket = self.nodes[id.node as usize].bucket.expect("reactive node has no value");
        self.slabs.get_mut(bucket, id.slot, id.generation)
    }

    pub fn retire_node(&mut self, id: ReactiveId) {
        if !self.is_alive(id) {
            return;
        }
        self.retire_persisted(id);
        self.unlink_node(id);
        let node = &mut self.nodes[id.node as usize];
        if node.kind == NodeKind::TextBuffer {
            self.text_streams.remove(&id);
        }
        node.alive = false;
        self.free_nodes.push(id.node);
        let siblings = &mut self.scopes[node.scope.index as usize].nodes;
        let index = node.scope_index;
        siblings.swap_remove(index);
        let value = node.bucket.take().and_then(|bucket| self.slabs.take(bucket, id.slot, id.generation));
        let retired = (node.compute.take(), node.watch_effect.take(), value);
        if let Some(swapped) = siblings.get(index) {
            self.nodes[swapped.node as usize].scope_index = index;
        }
        drop(retired);
    }

    pub fn report(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }
}
