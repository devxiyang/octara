//! Pull evaluation and ordered flushing for value and delta nodes.

pub mod delta;
pub mod list;
pub mod store;
pub mod text_buf;
pub mod text_stream;
pub mod value;

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use octara_model::diagnostics::{Diagnostic, codes};
use octara_model::identity::WindowId;
use octara_tree::Tree;
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

use crate::app::AppCommands;
use crate::arena::{NodeKind, NodeState, ReactiveId, ReactiveWorld, Revision, ScopeId, Source};
use crate::cx::{ReadCx, WriteCx};
use crate::task::TaskSpawner;

#[cfg(test)]
mod tests;

/// The order in which dirty computations and observers run within React.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlushTier {
    Derivation,
    Structure,
    Binding,
    Watch,
}

impl NodeKind {
    pub(crate) fn tier(self) -> Option<FlushTier> {
        match self {
            Self::Memo | Self::ListOp | Self::RowSource => Some(FlushTier::Derivation),
            Self::Structure => Some(FlushTier::Structure),
            Self::Binding => Some(FlushTier::Binding),
            Self::Watch => Some(FlushTier::Watch),
            Self::ValueSource | Self::ListSource | Self::TextBuffer => None,
        }
    }
}

impl ReactiveWorld {
    #[track_caller]
    pub fn create_observer(&mut self, kind: NodeKind, scope: ScopeId, window: Option<WindowId>) -> ReactiveId {
        assert!(matches!(kind, NodeKind::Structure | NodeKind::Binding), "external observers must be structures or bindings");
        let id = self.alloc_node(kind, scope, window);
        self.hot_mut(id).state = NodeState::Dirty;
        self.schedule(id);
        id
    }

    /// Requests a fresh sample when model work changes an external observer's input set.
    pub fn invalidate_observer(&mut self, id: ReactiveId) {
        if self.is_alive(id) {
            assert!(matches!(self.hot(id).kind, NodeKind::Structure | NodeKind::Binding), "expected a structure or binding");
            self.invalidate(id, NodeState::Dirty);
        }
    }

    /// Evaluates a dirty structure or binding and records precisely the dependencies it read.
    pub fn evaluate_observer<R>(&mut self, id: ReactiveId, f: impl FnOnce(&mut ReadCx<'_>) -> R) -> Option<R> {
        if !self.is_alive(id) {
            return None;
        }
        assert!(matches!(self.hot(id).kind, NodeKind::Structure | NodeKind::Binding), "expected a structure or binding");
        if !self.needs_evaluation(id) {
            return None;
        }
        let result = catch_unwind(AssertUnwindSafe(|| self.evaluate(id, f)));
        Some(self.complete_evaluation(id, result))
    }

    pub fn flush_derivations(&mut self) {
        while let Some(id) = self.next_observer(FlushTier::Derivation) {
            self.ensure_clean(id);
        }
    }

    /// Evaluates watch sources during React. Effects are deferred until after Commit.
    pub fn flush_watches(&mut self) {
        while let Some(id) = self.next_observer(FlushTier::Watch) {
            if self.ensure_clean(id) && !self.hot(id).watch_pending() {
                self.hot_mut(id).set_watch_pending(true);
                self.watch_effects.push_back(id);
            }
        }
    }

    pub fn run_watch_effects(&mut self) {
        self.run_effects(None, None, |_| None);
    }

    pub fn run_watch_effects_with(&mut self, commands: &mut AppCommands, tasks: &TaskSpawner) {
        self.run_effects(Some(commands), Some(tasks), |_| None);
    }

    pub(crate) fn run_watch_effects_with_tree<'a>(
        &mut self,
        commands: &mut AppCommands,
        tasks: &TaskSpawner,
        tree: impl Fn(WindowId) -> Option<&'a Tree>,
    ) {
        self.run_effects(Some(commands), Some(tasks), tree);
    }

    fn run_effects<'a>(
        &mut self,
        mut commands: Option<&mut AppCommands>,
        tasks: Option<&TaskSpawner>,
        tree: impl Fn(WindowId) -> Option<&'a Tree>,
    ) {
        while let Some(id) = self.watch_effects.pop_front() {
            if !self.is_alive(id) {
                continue;
            }
            let (scope, window) = self.owner(id);
            if !self.is_scope_alive(scope) {
                continue;
            }
            self.hot_mut(id).set_watch_pending(false);
            let mut effect = self.cold_mut(id).watch_effect.take().expect("watch effect");
            let mut cx = WriteCx::new(self, scope, window);
            cx.commands = commands.as_deref_mut();
            cx.tasks = tasks.cloned();
            cx.tree = window.and_then(&tree);
            let result = catch_unwind(AssertUnwindSafe(|| effect(&mut cx, id)));
            self.cold_mut(id).watch_effect = Some(effect);
            if let Err(panic) = result {
                resume_unwind(panic);
            }
        }
    }

    pub fn has_pending_work(&self) -> bool {
        self.has_pending_observers() || self.watch_effects.iter().any(|&id| self.is_alive(id))
    }

    #[track_caller]
    pub(crate) fn revision(&self, id: ReactiveId) -> Revision {
        self.hot(id).revision
    }

    pub(crate) fn init_compute(&mut self, id: ReactiveId, f: impl FnMut(&mut ReadCx<'_>) -> bool + 'static) {
        let node = self.cold_mut(id);
        assert!(node.compute.is_none(), "reactive compute is already registered");
        node.compute = Some(Box::new(f));
        self.hot_mut(id).state = NodeState::Dirty;
        self.schedule(id);
    }

    /// Declares a dependency that is retained across dynamic reads of a computation.
    pub(crate) fn add_dependency(&mut self, observer: ReactiveId, source: ReactiveId) {
        if observer == source || !self.cold(observer).observers.is_empty() && self.depends_on(source, observer) {
            self.cycle(observer);
        }
        let revision = self.revision(source);
        let sources = &mut self.cold_mut(observer).sources;
        if let Some(existing) = sources.iter_mut().find(|edge| edge.id == source) {
            existing.set_fixed();
            return;
        }
        sources.push(Source::new(source, revision, true));
        self.cold_mut(source).observers.push(observer);
        self.update_depths(observer);
    }

    fn depends_on(&self, start: ReactiveId, dependency: ReactiveId) -> bool {
        let mut pending = vec![start];
        let mut visited = FxHashSet::default();
        let depth = self.hot(dependency).depth;
        while let Some(id) = pending.pop() {
            if id == dependency {
                return true;
            }
            if self.hot(id).depth > depth && visited.insert(id) {
                pending.extend(self.cold(id).sources.iter().map(|source| source.id));
            }
        }
        false
    }

    #[track_caller]
    pub(crate) fn mark_changed(&mut self, id: ReactiveId) {
        let node = self.hot_mut(id);
        node.revision = node.revision.next();
        self.invalidate_observers(id);
    }

    fn invalidate_observers(&mut self, id: ReactiveId) {
        let mut pending = SmallVec::<[ReactiveId; 8]>::new();
        for index in 0..self.cold(id).observers.len() {
            let observer = self.cold(id).observers[index];
            if self.invalidate(observer, NodeState::Dirty) {
                pending.push(observer);
            }
        }
        let mut cursor = 0;
        while let Some(&id) = pending.get(cursor) {
            cursor += 1;
            for index in 0..self.cold(id).observers.len() {
                let observer = self.cold(id).observers[index];
                if self.invalidate(observer, NodeState::Check) {
                    pending.push(observer);
                }
            }
        }
    }

    fn invalidate(&mut self, id: ReactiveId, target: NodeState) -> bool {
        let node = self.hot_mut(id);
        let old = node.state;
        if old == NodeState::Clean || (old == NodeState::Check && target == NodeState::Dirty) {
            node.state = target;
            self.schedule(id);
        }
        // A Check node already propagated its invalidation to downstream nodes.
        old == NodeState::Clean
    }

    #[track_caller]
    pub(crate) fn ensure_clean(&mut self, id: ReactiveId) -> bool {
        if !self.needs_evaluation(id) {
            return false;
        }
        let mut compute = self.cold_mut(id).compute.take().expect("dirty reactive derivation has no computation");
        let result = catch_unwind(AssertUnwindSafe(|| self.evaluate(id, |r| compute(r))));
        self.cold_mut(id).compute = Some(compute);
        let changed = self.complete_evaluation(id, result);
        if changed {
            self.mark_changed(id);
        }
        changed
    }

    #[track_caller]
    fn needs_evaluation(&mut self, id: ReactiveId) -> bool {
        let node = self.hot(id);
        if node.computing() {
            self.cycle(id);
        }
        let state = node.state;
        if state == NodeState::Clean {
            return false;
        }
        self.unschedule(id);
        if state == NodeState::Check {
            self.hot_mut(id).set_computing(true);
            let result = catch_unwind(AssertUnwindSafe(|| {
                // Pulls may update this node's state and depth. Its source list is
                // only replaced when this node evaluates.
                for index in 0..self.cold(id).sources.len() {
                    let source = self.cold(id).sources[index];
                    self.ensure_clean(source.id);
                    if self.revision(source.id) != source.revision() {
                        return true;
                    }
                }
                self.hot(id).state == NodeState::Dirty
            }));
            if !self.complete_evaluation(id, result) {
                self.hot_mut(id).state = NodeState::Clean;
                return false;
            }
            self.hot_mut(id).state = NodeState::Dirty;
        }
        true
    }

    #[cold]
    #[inline(never)]
    fn cycle(&mut self, id: ReactiveId) -> ! {
        let defined_at = self.cold(id).defined_at;
        self.report(Diagnostic {
            code: &codes::CYCLE,
            message: "A reactive computation depends on its own output".into(),
            location: Some(defined_at.to_string()),
        });
        panic!("{}: reactive dependency cycle at {defined_at}", codes::CYCLE)
    }

    fn evaluate<R>(&mut self, id: ReactiveId, f: impl FnOnce(&mut ReadCx<'_>) -> R) -> R {
        self.hot_mut(id).set_computing(true);
        let fixed = self.cold(id).sources.iter().filter(|edge| edge.fixed()).copied().collect::<SmallVec<[_; 4]>>();
        for source in &fixed {
            self.ensure_clean(source.id);
        }
        let mut read = ReadCx::tracked(self, id);
        for source in fixed {
            read.record(source.id, true);
        }
        let result = f(&mut read);
        let sources = read.into_sources();
        self.update_sources(id, sources);
        self.hot_mut(id).state = NodeState::Clean;
        result
    }

    fn complete_evaluation<R>(&mut self, id: ReactiveId, result: std::thread::Result<R>) -> R {
        let node = self.hot_mut(id);
        node.set_computing(false);
        match result {
            Ok(value) => value,
            Err(panic) => {
                node.state = NodeState::Dirty;
                self.schedule(id);
                resume_unwind(panic)
            }
        }
    }

    fn update_sources(&mut self, id: ReactiveId, sources: SmallVec<[Source; 4]>) {
        let old = std::mem::take(&mut self.cold_mut(id).sources);
        let old_index = source_index(&old);
        let new_index = source_index(&sources);
        let mut changed = false;
        for source in &old {
            if !has_source(&sources, new_index.as_ref(), source.id) {
                self.cold_mut(source.id).observers.retain(|&mut observer| observer != id);
                changed = true;
            }
        }
        for source in &sources {
            if !has_source(&old, old_index.as_ref(), source.id) {
                self.cold_mut(source.id).observers.push(id);
                changed = true;
            }
        }
        self.cold_mut(id).sources = sources;
        if changed {
            self.update_depths(id);
        }
    }

    fn update_depths(&mut self, id: ReactiveId) {
        let mut pending = SmallVec::<[ReactiveId; 8]>::new();
        pending.push(id);
        while let Some(id) = pending.pop() {
            let depth = self.cold(id).sources.iter().map(|source| self.hot(source.id).depth + 1).max().unwrap_or(0);
            if depth == self.hot(id).depth {
                continue;
            }
            self.unschedule(id);
            let node = self.hot_mut(id);
            node.depth = depth;
            if node.state != NodeState::Clean {
                self.schedule(id);
            }
            pending.extend(self.cold(id).observers.iter().copied());
        }
    }

    pub(crate) fn unlink_node(&mut self, id: ReactiveId) {
        self.unschedule(id);
        self.invalidate_observers(id);
        let (sources, observers) = {
            let node = self.cold_mut(id);
            (std::mem::take(&mut node.sources), std::mem::take(&mut node.observers))
        };
        for source in sources {
            self.cold_mut(source.id).observers.retain(|&mut observer| observer != id);
        }
        for observer in observers {
            self.cold_mut(observer).sources.retain(|edge| edge.id != id);
            self.update_depths(observer);
        }
    }
}

fn source_index(sources: &[Source]) -> Option<FxHashSet<ReactiveId>> {
    (sources.len() > 4).then(|| sources.iter().map(|source| source.id).collect())
}

fn has_source(sources: &[Source], index: Option<&FxHashSet<ReactiveId>>, id: ReactiveId) -> bool {
    index.map_or_else(|| sources.iter().any(|source| source.id == id), |index| index.contains(&id))
}
