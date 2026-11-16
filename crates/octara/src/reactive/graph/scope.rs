//! Scope ownership, named state, context and resumable teardown.

use super::*;

impl Runtime {
    // ---- scopes ----

    pub(crate) fn scope_alive(&self, s: ScopeId) -> bool {
        s.runtime == self.identity && self.scopes.borrow().get(s.i as usize).is_some_and(|d| d.state == ScopeState::Live && d.g == s.g)
    }
    pub fn current_scope(&self) -> Option<ScopeId> {
        self.scope.get()
    }
    pub fn scope_path(&self, s: ScopeId) -> String {
        self.scopes.borrow()[s.i as usize].path.clone()
    }

    /// Registers `name` under `parent`; returns the segment (`name`, then `name#2`, ...).
    pub fn child_name(&self, parent: ScopeId, name: &str) -> String {
        let mut scopes = self.scopes.borrow_mut();
        let n = scopes[parent.i as usize].names.entry(name.to_owned()).or_insert(0);
        *n += 1;
        if *n == 1 { name.to_owned() } else { format!("{name}#{n}") }
    }

    pub fn create_scope(&self, parent: Option<ScopeId>, segment: &str) -> ScopeId {
        let path = match parent {
            Some(p) => format!("{}/{segment}", self.scope_path(p)),
            None => segment.to_owned(),
        };
        let make = |g| ScopeData {
            g,
            state: ScopeState::Live,
            parent,
            children: Vec::new(),
            nodes: Vec::new(),
            cleanups: BTreeMap::new(),
            next_cleanup: 0,
            path,
            names: HashMap::new(),
            state_keys: HashSet::new(),
            persistent_names: false,
            contexts: Vec::new(),
            owner: self.owner.get(),
        };
        let id = {
            let mut scopes = self.scopes.borrow_mut();
            if let Some(i) = self.free_scopes.borrow_mut().pop() {
                let g = scopes[i as usize].g.checked_add(1).expect("scope generation exhausted");
                scopes[i as usize] = make(g);
                ScopeId { runtime: self.identity, i, g }
            } else {
                let i = u32::try_from(scopes.len()).expect("scope slots exhausted");
                scopes.push(make(0));
                ScopeId { runtime: self.identity, i, g: 0 }
            }
        };
        if let Some(p) = parent {
            self.scopes.borrow_mut()[p.i as usize].children.push(id);
        }
        id
    }

    pub(crate) fn create_app_scope(&self) -> ScopeId {
        let scope = self.create_scope(None, "app");
        self.scopes.borrow_mut()[scope.i as usize].persistent_names = true;
        scope
    }

    pub fn on_cleanup(&self, f: Box<dyn FnOnce()>) {
        self.insert_cleanup(f);
    }

    fn insert_cleanup(&self, f: Box<dyn FnOnce()>) -> Option<(ScopeId, u64)> {
        let scope = self.scope.get().filter(|s| self.scope_alive(*s))?;
        let mut scopes = self.scopes.borrow_mut();
        let data = &mut scopes[scope.i as usize];
        let id = data.next_cleanup;
        data.next_cleanup = id.checked_add(1).expect("cleanup identity exhausted");
        data.cleanups.insert(id, f);
        Some((scope, id))
    }

    pub(crate) fn register_cleanup(&self, f: Box<dyn FnOnce()>) -> Option<CleanupRegistration> {
        self.insert_cleanup(f).map(|(scope, id)| CleanupRegistration { runtime: Rc::downgrade(&Self::current()), scope, id })
    }

    #[cfg(test)]
    pub(crate) fn cleanup_count(&self, scope: ScopeId) -> usize {
        self.scopes.borrow()[scope.i as usize].cleanups.len()
    }

    /// Disposes a scope with its children, cleanups and nodes. During a hot reload named state
    /// stays in the store; otherwise it goes with the scope.
    pub fn dispose_scope(&self, s: ScopeId) {
        let (parent, path, owner) = {
            let mut scopes = self.scopes.borrow_mut();
            let Some(d) = scopes.get_mut(s.i as usize).filter(|d| s.runtime == self.identity && d.g == s.g) else { return };
            if matches!(d.state, ScopeState::Disposing | ScopeState::Disposed) {
                return;
            }
            d.state = ScopeState::Disposing;
            (d.parent, d.path.clone(), d.owner)
        };
        let _retire = std::mem::DropGuard::new(s, |s| {
            let mut scopes = self.scopes.borrow_mut();
            if let Some(d) = scopes.get_mut(s.i as usize).filter(|d| d.g == s.g && d.state == ScopeState::Disposing) {
                d.state = ScopeState::Retiring;
            }
        });
        self.with_owner(owner, || {
            self.with_frame(Ctx::Cleanup, None, Some(Some(s)), || {
                loop {
                    let child = self.scopes.borrow()[s.i as usize].children.last().copied();
                    let Some(child) = child else { break };
                    self.dispose_scope(child);
                    // Keep the child reachable if its disposal unwinds.
                    self.scopes.borrow_mut()[s.i as usize].children.retain(|id| *id != child);
                }
                loop {
                    let cleanup = self.scopes.borrow_mut()[s.i as usize].cleanups.pop_last();
                    let Some((_, cleanup)) = cleanup else { break };
                    // A started callback is consumed even when it panics. Others remain owned.
                    cleanup();
                }
                loop {
                    let node = self.scopes.borrow_mut()[s.i as usize].nodes.pop();
                    let Some(n) = node else { break };
                    let named = self.alive(n) && self.nodes.borrow()[n.i as usize].named;
                    if !(named && self.reloading.get()) {
                        self.dispose_node(n);
                    }
                }
                if !self.reloading.get() {
                    self.store.borrow_mut().retain(|(p, _), _| p != &path);
                }
                loop {
                    let context = self.scopes.borrow_mut()[s.i as usize].contexts.pop();
                    let Some(context) = context else { break };
                    drop(context);
                }
                if let Some(p) = parent {
                    let mut scopes = self.scopes.borrow_mut();
                    if let Some(d) = scopes.get_mut(p.i as usize).filter(|d| d.g == p.g) {
                        d.children.retain(|c| *c != s);
                    }
                }
                self.scopes.borrow_mut()[s.i as usize].state = ScopeState::Disposed;
                self.free_scopes.borrow_mut().push(s.i);
            })
        });
    }

    /// Final App teardown also owns allocations preserved by an interrupted hot reload.
    pub(crate) fn shutdown(&self) {
        self.reloading.set(false);
        let scopes: Vec<_> = self
            .scopes
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, d)| matches!(d.state, ScopeState::Live | ScopeState::Retiring))
            .map(|(i, d)| ScopeId { runtime: self.identity, i: i as u32, g: d.g })
            .collect();
        for scope in scopes {
            self.dispose_scope(scope);
        }
        let nodes: Vec<_> = self
            .nodes
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, n)| n.alive)
            .map(|(i, n)| Idx { runtime: self.identity, i: i as u32, g: n.g })
            .collect();
        for node in nodes {
            self.dispose_node(node);
        }
        self.store.borrow_mut().clear();
        self.queue.borrow_mut().clear();
    }

    pub(super) fn provide(&self, ty: TypeId, value: Rc<dyn Any>) {
        let s = self.scope.get().expect("provide outside a component");
        let replaced = {
            let mut scopes = self.scopes.borrow_mut();
            let ctx = &mut scopes[s.i as usize].contexts;
            let replaced = ctx.iter().position(|(t, _)| *t == ty).map(|i| ctx.remove(i));
            ctx.push((ty, value));
            replaced
        };
        drop(replaced);
    }

    pub(super) fn lookup(&self, ty: TypeId) -> Option<Rc<dyn Any>> {
        let scopes = self.scopes.borrow();
        let mut cur = self.scope.get();
        while let Some(s) = cur.filter(|s| {
            s.runtime == self.identity
                && scopes.get(s.i as usize).is_some_and(|d| {
                    d.g == s.g && (d.state == ScopeState::Live || (d.state != ScopeState::Disposed && self.ctx() == Ctx::Cleanup))
                })
        }) {
            let d = &scopes[s.i as usize];
            if let Some((_, v)) = d.contexts.iter().find(|(t, _)| *t == ty) {
                return Some(v.clone());
            }
            cur = d.parent;
        }
        None
    }

    pub fn set_reloading(&self, on: bool) {
        self.reloading.set(on);
    }

    pub(super) fn named_state(&self, key: &'static str, ty: TypeId, init: impl FnOnce() -> Value, at: &'static Location<'static>) -> Idx {
        let Some(scope) = self.scope.get() else {
            panic!("state(\"{key}\") outside a component at {}", loc(at));
        };
        if !self.scope_alive(scope) {
            self.diag(&codes::WRITE_DISPOSED, "cannot initialize state in a retired scope".into(), at);
            panic!("[{}] state owner was disposed", codes::WRITE_DISPOSED);
        }
        let path = self.scope_path(scope);
        let persistent = self.scopes.borrow()[scope.i as usize].persistent_names;
        let duplicate = !persistent && !self.scopes.borrow_mut()[scope.i as usize].state_keys.insert(key);
        if duplicate {
            self.diag(
                &codes::DUPLICATE_STATE_KEY,
                format!("state key \"{key}\" used twice in {path}; both calls would share one signal"),
                at,
            );
        }
        let existing = self.store.borrow().get(&(path.clone(), key)).copied();
        if let Some((idx, old_ty)) = existing {
            if self.alive(idx) && old_ty == ty {
                if self.nodes.borrow()[idx.i as usize].scope != Some(scope) {
                    self.scopes.borrow_mut()[scope.i as usize].nodes.push(idx);
                    self.nodes.borrow_mut()[idx.i as usize].scope = Some(scope);
                }
                return idx;
            }
            if persistent && self.alive(idx) {
                self.diag(&codes::STATE_TYPE_CHANGED, format!("App state \"{key}\" already has a different type; lookup rejected"), at);
                panic!("[{}] App state type mismatch", codes::STATE_TYPE_CHANGED);
            }
            // The same key for another type in this build: the first keeps it, this call gets
            // a state of its own (not kept across reloads); the diagnostic above says why.
            if self.alive(idx) && duplicate {
                return self.create_node(Kind::Source, Some(init()), None, true, None, at);
            }
            if self.alive(idx) {
                self.diag(&codes::STATE_TYPE_CHANGED, format!("state \"{key}\" in {path} changed type across a reload; reset"), at);
                self.dispose_node(idx);
            }
        }
        let idx = self.create_node(Kind::Source, Some(init()), None, true, None, at);
        self.store.borrow_mut().insert((path, key), (idx, ty));
        idx
    }
}
