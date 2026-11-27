use std::any::{Any, TypeId, type_name};
use std::collections::BTreeMap;

use octara_model::diagnostics::{Diagnostic, codes};
use octara_model::identity::WindowId;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::{NodeKind, ReactiveId, ReactiveWorld};
use crate::cx::CleanupCx;

/// Identifies a lifetime and a context lookup boundary in one reactive world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScopeId {
    runtime: u32,
    pub(super) index: u32,
    generation: u64,
}

/// The serialized state explicitly registered with `BuildCx::persisted`.
pub type PersistedState = BTreeMap<String, Value>;

type Cleanup = Box<dyn FnOnce(&mut CleanupCx<'_>)>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScopeState {
    Live,
    Retiring,
    Dead,
}

pub(super) struct ScopeData {
    id: ScopeId,
    state: ScopeState,
    parent: Option<ScopeId>,
    parent_index: usize,
    children: Vec<ScopeId>,
    pub(super) nodes: Vec<ReactiveId>,
    cleanups: Vec<Cleanup>,
    context: FxHashMap<TypeId, Box<dyn Any>>,
    path: String,
    next_child: u64,
}

impl ScopeData {
    pub(super) fn new(id: ScopeId, parent: Option<ScopeId>, parent_index: usize, path: String) -> Self {
        Self {
            id,
            state: ScopeState::Live,
            parent,
            parent_index,
            children: Vec::new(),
            nodes: Vec::new(),
            cleanups: Vec::new(),
            context: FxHashMap::default(),
            path,
            next_child: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct PersistedEntry {
    id: ReactiveId,
    snapshot: fn(&ReactiveWorld, ReactiveId) -> Value,
}

#[derive(Serialize, Deserialize)]
struct StoredValue<T> {
    #[serde(rename = "type")]
    value_type: String,
    value: T,
}

impl ReactiveWorld {
    pub fn live_scopes(&self) -> usize {
        self.scopes.iter().filter(|scope| scope.state == ScopeState::Live).count()
    }

    pub fn root_scope(&self) -> ScopeId {
        ScopeId { runtime: self.runtime, index: 0, generation: 0 }
    }

    pub fn is_scope_alive(&self, scope: ScopeId) -> bool {
        self.scope_data(scope).is_some_and(|data| data.state == ScopeState::Live)
    }

    pub(crate) fn scope_is_within(&self, scope: ScopeId, ancestor: ScopeId) -> bool {
        std::iter::successors(Some(scope), |&id| self.scope_data(id)?.parent).any(|id| id == ancestor)
    }

    #[track_caller]
    pub(super) fn check_scope(&self, scope: ScopeId) {
        assert_eq!(scope.runtime, self.runtime, "{}: scope belongs to another runtime", codes::WRONG_RUNTIME);
        assert!(self.is_scope_alive(scope), "{}: scope {scope:?} is no longer live", codes::WRITE_DISPOSED);
    }

    fn scope_data(&self, id: ScopeId) -> Option<&ScopeData> {
        if id.runtime == self.runtime { self.scopes.get(id.index as usize).filter(|data| data.id == id) } else { None }
    }

    /// Creates a child whose persisted state is addressed by its construction order.
    #[track_caller]
    pub fn new_scope(&mut self, parent: ScopeId) -> ScopeId {
        self.check_scope(parent);
        let data = &mut self.scopes[parent.index as usize];
        let ordinal = data.next_child;
        data.next_child = ordinal.checked_add(1).expect("scope child ordinals exhausted");
        self.insert_scope(parent, &ordinal.to_string())
    }

    /// Creates a child whose persisted state can be reopened using the same key.
    #[track_caller]
    pub fn new_keyed_scope(&mut self, parent: ScopeId, key: &str) -> ScopeId {
        self.check_scope(parent);
        let segment = format!("key:{}", escape_segment(key));
        self.insert_scope(parent, &segment)
    }

    fn insert_scope(&mut self, parent: ScopeId, segment: &str) -> ScopeId {
        let data = &self.scopes[parent.index as usize];
        let mut path = data.path.clone();
        path.push('/');
        path.push_str(segment);
        let parent_index = data.children.len();
        let id = if let Some(index) = self.free_scopes.pop() {
            let generation = self.scopes[index as usize].id.generation.checked_add(1).expect("scope generation exhausted");
            let id = ScopeId { runtime: self.runtime, index, generation };
            self.scopes[index as usize] = ScopeData::new(id, Some(parent), parent_index, path);
            id
        } else {
            let index = u32::try_from(self.scopes.len()).expect("scope IDs exhausted");
            let id = ScopeId { runtime: self.runtime, index, generation: 0 };
            self.scopes.push(ScopeData::new(id, Some(parent), parent_index, path));
            id
        };
        self.scopes[parent.index as usize].children.push(id);
        id
    }

    /// Runs child and local cleanups before releasing their values. If a cleanup panics its
    /// registration is consumed and a later `dispose` can resume the remaining teardown.
    pub fn dispose(&mut self, scope: ScopeId) {
        let Some(data) = self.scope_data(scope) else { return };
        if data.state == ScopeState::Dead {
            return;
        }
        if data.state == ScopeState::Live {
            let mut pending = vec![scope];
            while let Some(id) = pending.pop() {
                let data = &mut self.scopes[id.index as usize];
                if data.state == ScopeState::Live {
                    data.state = ScopeState::Retiring;
                    pending.extend(data.children.iter().copied());
                }
            }
        }

        let mut current = scope;
        loop {
            let index = current.index as usize;
            if let Some(child) = self.scopes[index].children.last() {
                current = *child;
                continue;
            }
            while let Some(cleanup) = self.scopes[index].cleanups.pop() {
                cleanup(&mut CleanupCx::new(self));
            }
            while let Some(id) = self.scopes[index].nodes.last().copied() {
                self.retire_node(id);
            }
            let data = &mut self.scopes[index];
            drop(std::mem::take(&mut data.context));
            data.state = ScopeState::Dead;
            let parent = data.parent;
            let parent_index = data.parent_index;
            if let Some(parent) = parent {
                let siblings = &mut self.scopes[parent.index as usize].children;
                siblings.swap_remove(parent_index);
                if let Some(swapped) = siblings.get(parent_index).copied() {
                    self.scopes[swapped.index as usize].parent_index = parent_index;
                }
                self.free_scopes.push(current.index);
            }
            if current == scope {
                break;
            }
            current = parent.expect("a child scope has a parent");
        }
    }

    pub(crate) fn on_cleanup(&mut self, scope: ScopeId, cleanup: Cleanup) {
        self.scopes[scope.index as usize].cleanups.push(cleanup);
    }

    pub(crate) fn provide<T: 'static>(&mut self, scope: ScopeId, value: T) {
        let old = self.scopes[scope.index as usize].context.insert(TypeId::of::<T>(), Box::new(value));
        drop(old);
    }

    pub(crate) fn context<T: Clone + 'static>(&self, scope: ScopeId) -> Option<T> {
        let mut scope = Some(scope);
        while let Some(id) = scope {
            let data = self.scope_data(id)?;
            if let Some(value) = data.context.get(&TypeId::of::<T>()) {
                return value.downcast_ref::<T>().cloned();
            }
            scope = data.parent;
        }
        None
    }

    /// Imports snapshots before building the scopes that declare their keys.
    pub fn import_persisted(&mut self, state: PersistedState) {
        self.persisted = state;
    }

    /// Exports the latest snapshots of declared values, including those in retired scopes.
    pub fn export_persisted(&self) -> PersistedState {
        let mut state = self.persisted.clone();
        state.extend(self.live_persisted.iter().map(|(key, entry)| (key.clone(), (entry.snapshot)(self, entry.id))));
        state
    }

    #[track_caller]
    pub(crate) fn persisted<T: Serialize + DeserializeOwned + 'static>(
        &mut self,
        scope: ScopeId,
        window: Option<WindowId>,
        key: &str,
        init: impl FnOnce() -> T,
    ) -> ReactiveId {
        let scope_path = &self.scopes[scope.index as usize].path;
        let path = if scope_path.is_empty() { escape_segment(key) } else { format!("{scope_path}/{}", escape_segment(key)) };
        assert!(!self.live_persisted.contains_key(&path), "{}: `{path}`", codes::DUPLICATE_PERSISTED_KEY);
        let value = self.persisted.remove(&path).and_then(|stored| {
            let saved = serde_json::from_value::<StoredValue<Value>>(stored);
            match saved {
                Ok(saved) if saved.value_type == type_name::<T>() => match serde_json::from_value(saved.value) {
                    Ok(value) => return Some(value),
                    Err(error) => self.report(Diagnostic::new(&codes::PERSISTED_TYPE_CHANGED, format!("`{path}`: {error}"))),
                },
                Ok(saved) => self.report(Diagnostic::new(
                    &codes::PERSISTED_TYPE_CHANGED,
                    format!("`{path}` changed from `{}` to `{}`", saved.value_type, type_name::<T>()),
                )),
                Err(error) => self.report(Diagnostic::new(&codes::PERSISTED_TYPE_CHANGED, format!("`{path}`: {error}"))),
            }
            None
        });
        let id = self.alloc_value(NodeKind::ValueSource, scope, window, value.unwrap_or_else(init));
        self.node_mut(id).persisted_key = Some(path.clone());
        self.live_persisted.insert(path, PersistedEntry { id, snapshot: snapshot::<T> });
        id
    }

    pub(super) fn retire_persisted(&mut self, id: ReactiveId) {
        let Some(path) = &self.node(id).persisted_key else { return };
        let entry = self.live_persisted[path];
        let value = (entry.snapshot)(self, id);
        let path = self.node_mut(id).persisted_key.take().expect("persisted value key");
        self.live_persisted.remove(&path);
        self.persisted.insert(path, value);
    }
}

fn escape_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn snapshot<T: Serialize + 'static>(world: &ReactiveWorld, id: ReactiveId) -> Value {
    serde_json::to_value(StoredValue { value_type: type_name::<T>().into(), value: world.value::<T>(id) })
        .expect("persisted state must be JSON serializable")
}
