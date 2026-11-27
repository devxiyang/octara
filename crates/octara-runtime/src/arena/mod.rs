mod pending;
mod scope;
mod slab;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::panic::Location;
use std::sync::atomic::{AtomicU32, Ordering};

use octara_model::diagnostics::{Diagnostic, codes};
use octara_model::identity::WindowId;
use smallvec::SmallVec;

use self::pending::Pending;
use self::scope::{PersistedEntry, ScopeData};
use self::slab::SlabStore;
use crate::cx::{BuildCx, ReadCx, WriteCx};

pub use self::scope::{PersistedState, ScopeId};

static NEXT_RUNTIME: octara_model::CacheLine<AtomicU32> = octara_model::CacheLine(AtomicU32::new(1));

const GENERATION_BITS: u32 = u16::BITS;
const NODE_BITS: u32 = 24;
const RUNTIME_SHIFT: u32 = GENERATION_BITS + NODE_BITS;
const MAX_NODE_INDEX: u32 = (1 << NODE_BITS) - 1;
const MAX_RUNTIME: u32 = (1 << (u64::BITS - RUNTIME_SHIFT)) - 1;

fn next_runtime(counter: &AtomicU32) -> u32 {
    counter
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| (id <= MAX_RUNTIME).then(|| id + 1))
        .expect("reactive runtime IDs exhausted")
}

/// Identifies one allocation in one reactive world.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct ReactiveId(u64);

const _: () = assert!(size_of::<ReactiveId>() == 8);

impl ReactiveId {
    // Keep the runtime, node, and generation order for Ord. A node slot is retired
    // permanently after its final generation so that old handles cannot alias new ones.
    fn new(runtime: u32, node: u32, generation: u16) -> Self {
        debug_assert!(runtime <= MAX_RUNTIME && node <= MAX_NODE_INDEX);
        Self((u64::from(runtime) << RUNTIME_SHIFT) | (u64::from(node) << GENERATION_BITS) | u64::from(generation))
    }

    fn runtime(self) -> u32 {
        (self.0 >> RUNTIME_SHIFT) as u32
    }

    pub(crate) fn node(self) -> u32 {
        ((self.0 >> GENERATION_BITS) as u32) & MAX_NODE_INDEX
    }

    fn generation(self) -> u16 {
        self.0 as u16
    }
}

impl fmt::Debug for ReactiveId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReactiveId")
            .field("runtime", &self.runtime())
            .field("node", &self.node())
            .field("generation", &self.generation())
            .finish()
    }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Revision(u64);

impl Revision {
    const MAX: u64 = u64::MAX >> 1;

    pub(crate) fn get(self) -> u64 {
        self.0
    }

    #[track_caller]
    pub(crate) fn next(self) -> Self {
        assert!(self.0 < Self::MAX, "reactive revision exhausted");
        Self(self.0 + 1)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Source {
    pub id: ReactiveId,
    // The least significant bit marks a dependency retained across evaluations.
    revision_and_fixed: u64,
}

const _: () = assert!(size_of::<Source>() == 16);

impl Source {
    pub(crate) fn new(id: ReactiveId, revision: Revision, fixed: bool) -> Self {
        Self { id, revision_and_fixed: (revision.0 << 1) | u64::from(fixed) }
    }

    pub(crate) fn revision(self) -> Revision {
        Revision(self.revision_and_fixed >> 1)
    }

    pub(crate) fn fixed(self) -> bool {
        self.revision_and_fixed & 1 != 0
    }

    pub(crate) fn set_fixed(&mut self) {
        self.revision_and_fixed |= 1;
    }
}

const ALIVE: u8 = 1 << 0;
const COMPUTING: u8 = 1 << 1;
const SCHEDULED: u8 = 1 << 2;
const WATCH_PENDING: u8 = 1 << 3;

pub(crate) struct ReactiveHot {
    // The runtime and node index are implicit in the enclosing world and vector slot.
    generation: u16,
    slot: u32,
    bucket: u16,
    flags: u8,
    // Position within the depth bucket while scheduled at a depth below 64.
    queue_index: u32,
    pub kind: NodeKind,
    pub state: NodeState,
    pub depth: u32,
    pub revision: Revision,
}

const _: () = assert!(size_of::<ReactiveHot>() <= 32);

impl ReactiveHot {
    fn id(&self, runtime: u32, node: u32) -> ReactiveId {
        ReactiveId::new(runtime, node, self.generation)
    }

    fn has_flag(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    fn set_flag(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    fn value_bucket(&self) -> Option<u16> {
        (self.slot != u32::MAX).then_some(self.bucket)
    }

    pub(crate) fn computing(&self) -> bool {
        self.has_flag(COMPUTING)
    }

    pub(crate) fn set_computing(&mut self, computing: bool) {
        self.set_flag(COMPUTING, computing);
    }

    pub(crate) fn watch_pending(&self) -> bool {
        self.has_flag(WATCH_PENDING)
    }

    pub(crate) fn set_watch_pending(&mut self, pending: bool) {
        self.set_flag(WATCH_PENDING, pending);
    }
}

pub(crate) struct ReactiveCold {
    scope: ScopeId,
    window: Option<WindowId>,
    scope_index: usize,
    pub sources: SmallVec<[Source; 4]>,
    pub observers: SmallVec<[ReactiveId; 4]>,
    pub compute: Option<Compute>,
    pub watch_effect: Option<WatchEffect>,
    persisted_key: Option<String>,
    pub defined_at: &'static Location<'static>,
}

/// Owns the value and delta graph, scopes, and the state they contain.
pub struct ReactiveWorld {
    runtime: u32,
    slabs: SlabStore,
    hot: Vec<ReactiveHot>,
    cold: Vec<ReactiveCold>,
    free_nodes: Vec<u32>,
    scopes: Vec<ScopeData>,
    free_scopes: Vec<u32>,
    persisted: PersistedState,
    live_persisted: BTreeMap<String, PersistedEntry>,
    pub(crate) text_streams: BTreeSet<ReactiveId>,
    pending: Pending,
    pub(crate) watch_effects: VecDeque<ReactiveId>,
    pub(crate) module_work: crate::module::ModuleWork,
    diagnostics: Vec<Diagnostic>,
}

impl Default for ReactiveWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl ReactiveWorld {
    pub fn new() -> Self {
        let runtime = next_runtime(&NEXT_RUNTIME);
        let mut world = Self {
            runtime,
            slabs: SlabStore::default(),
            hot: Vec::new(),
            cold: Vec::new(),
            free_nodes: Vec::new(),
            scopes: Vec::new(),
            free_scopes: Vec::new(),
            persisted: PersistedState::new(),
            live_persisted: BTreeMap::new(),
            text_streams: BTreeSet::new(),
            pending: Pending::default(),
            watch_effects: VecDeque::new(),
            module_work: crate::module::ModuleWork::default(),
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
        self.hot.iter().filter(|node| node.has_flag(ALIVE)).count()
    }

    /// Check that retained reactive references resolve to live allocations in both directions.
    pub fn validate_integrity(&self) -> bool {
        if self.hot.len() != self.cold.len() {
            return false;
        }
        self.hot.iter().zip(&self.cold).enumerate().filter(|(_, (hot, _))| hot.has_flag(ALIVE)).all(|(index, (hot, cold))| {
            let id = hot.id(self.runtime, index as u32);
            self.is_scope_alive(cold.scope)
                && self.scopes[cold.scope.index as usize].nodes.get(cold.scope_index) == Some(&id)
                && cold.sources.iter().all(|source| self.is_alive(source.id) && self.cold(source.id).observers.contains(&id))
                && cold
                    .observers
                    .iter()
                    .all(|&observer| self.is_alive(observer) && self.cold(observer).sources.iter().any(|source| source.id == id))
        }) && self.scopes.iter().all(|scope| scope.nodes.iter().all(|&id| self.is_alive(id)))
    }

    pub fn is_alive(&self, id: ReactiveId) -> bool {
        id.runtime() == self.runtime && self.live_node(id).is_some()
    }

    fn live_node(&self, id: ReactiveId) -> Option<&ReactiveHot> {
        self.hot.get(id.node() as usize).filter(|node| node.has_flag(ALIVE) && node.generation == id.generation())
    }

    #[track_caller]
    pub fn owner(&self, id: ReactiveId) -> (ScopeId, Option<WindowId>) {
        let node = self.cold(id);
        (node.scope, node.window)
    }

    #[track_caller]
    pub(crate) fn hot(&self, id: ReactiveId) -> &ReactiveHot {
        self.check_node(id, &codes::READ_DISPOSED)
    }

    #[track_caller]
    pub(crate) fn hot_mut(&mut self, id: ReactiveId) -> &mut ReactiveHot {
        self.check_node(id, &codes::WRITE_DISPOSED);
        &mut self.hot[id.node() as usize]
    }

    #[track_caller]
    pub(crate) fn cold(&self, id: ReactiveId) -> &ReactiveCold {
        self.check_node(id, &codes::READ_DISPOSED);
        &self.cold[id.node() as usize]
    }

    #[track_caller]
    pub(crate) fn cold_mut(&mut self, id: ReactiveId) -> &mut ReactiveCold {
        self.check_node(id, &codes::WRITE_DISPOSED);
        &mut self.cold[id.node() as usize]
    }

    #[track_caller]
    fn check_node(&self, id: ReactiveId, disposed: &'static octara_model::diagnostics::Code) -> &ReactiveHot {
        assert_eq!(id.runtime(), self.runtime, "{}: reactive handle belongs to another runtime", codes::WRONG_RUNTIME);
        let Some(node) = self.live_node(id) else {
            panic!("{disposed}: reactive node {id:?} is no longer live");
        };
        node
    }

    #[track_caller]
    pub(crate) fn alloc_node(&mut self, kind: NodeKind, scope: ScopeId, window: Option<WindowId>) -> ReactiveId {
        self.alloc(kind, scope, window, |_, _| (0, u32::MAX))
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
        self.alloc(kind, scope, window, |slabs, generation| slabs.insert(generation, value))
    }

    #[track_caller]
    fn alloc(
        &mut self,
        kind: NodeKind,
        scope: ScopeId,
        window: Option<WindowId>,
        init: impl FnOnce(&mut SlabStore, u32) -> (u16, u32),
    ) -> ReactiveId {
        self.check_scope(scope);
        let spare = self.free_nodes.last().copied();
        let (index, generation) = if let Some(index) = spare {
            let generation = self.hot[index as usize].generation.checked_add(1).expect("reactive node generation exhausted");
            (index, generation)
        } else {
            let index = u32::try_from(self.hot.len()).expect("reactive node IDs exhausted");
            assert!(index <= MAX_NODE_INDEX, "reactive node IDs exhausted");
            (index, 0)
        };
        let (bucket, slot) = init(&mut self.slabs, u32::from(generation));
        let id = ReactiveId::new(self.runtime, index, generation);
        let scope_data = &mut self.scopes[scope.index as usize];
        let scope_index = scope_data.nodes.len();
        scope_data.nodes.push(id);
        let hot = ReactiveHot {
            generation,
            slot,
            bucket,
            flags: ALIVE,
            queue_index: 0,
            kind,
            state: NodeState::Clean,
            depth: 0,
            revision: Revision::default(),
        };
        let cold = ReactiveCold {
            scope,
            window,
            scope_index,
            sources: SmallVec::new(),
            observers: SmallVec::new(),
            compute: None,
            watch_effect: None,
            persisted_key: None,
            defined_at: Location::caller(),
        };
        if spare.is_some() {
            self.free_nodes.pop();
            self.hot[index as usize] = hot;
            self.cold[index as usize] = cold;
        } else {
            self.hot.push(hot);
            self.cold.push(cold);
        }
        id
    }

    #[track_caller]
    pub(crate) fn value<T: 'static>(&self, id: ReactiveId) -> &T {
        let node = self.hot(id);
        let bucket = node.value_bucket().expect("reactive node has no value");
        self.slabs.get(bucket, node.slot, u32::from(node.generation))
    }

    #[track_caller]
    pub(crate) fn value_mut<T: 'static>(&mut self, id: ReactiveId) -> &mut T {
        self.value_slot_mut(id).as_mut().expect("occupied typed slab slot")
    }

    #[track_caller]
    pub(crate) fn value_slot_mut<T: 'static>(&mut self, id: ReactiveId) -> &mut Option<T> {
        let node = self.check_node(id, &codes::WRITE_DISPOSED);
        let (bucket, slot, generation) = (node.value_bucket().expect("reactive node has no value"), node.slot, u32::from(node.generation));
        self.slabs.get_mut(bucket, slot, generation)
    }

    pub fn retire_node(&mut self, id: ReactiveId) {
        if !self.is_alive(id) {
            return;
        }
        self.retire_persisted(id);
        self.unlink_node(id);
        let hot = &mut self.hot[id.node() as usize];
        let cold = &mut self.cold[id.node() as usize];
        if hot.kind == NodeKind::TextBuffer {
            self.text_streams.remove(&id);
        }
        hot.flags &= !ALIVE;
        if hot.generation < u16::MAX {
            self.free_nodes.push(id.node());
        }
        let siblings = &mut self.scopes[cold.scope.index as usize].nodes;
        let index = cold.scope_index;
        siblings.swap_remove(index);
        let value = hot.value_bucket().and_then(|bucket| self.slabs.take(bucket, hot.slot, u32::from(hot.generation)));
        let retired = (cold.compute.take(), cold.watch_effect.take(), value);
        if let Some(swapped) = siblings.get(index) {
            self.cold[swapped.node() as usize].scope_index = index;
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
