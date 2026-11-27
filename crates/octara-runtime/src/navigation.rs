//! Scope-owned navigation history and leave guards.

use std::{
    cell::RefCell,
    sync::{Arc, Weak},
};

use rustc_hash::FxHashSet;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{DeserializeOwned, Error},
};

use crate::{BuildCx, ReactiveId, ReactiveWorld, ReadCx, Signal, WriteCx};

/// Back and forward history owned by the scope that created it.
pub struct Navigator<R> {
    history: Signal<RouteHistory<R>>,
    guards: Signal<Arc<LeaveGuards<R>>>,
}

impl<R> Copy for Navigator<R> {}

impl<R> Clone for Navigator<R> {
    fn clone(&self) -> Self {
        *self
    }
}

/// A snapshot used to synchronize a host's history with the navigator.
#[doc(hidden)]
#[derive(Clone, Serialize)]
pub struct RouteHistory<R> {
    pub entries: Vec<RouteEntry<R>>,
    pub cursor: usize,
    next_id: u64,
}

#[doc(hidden)]
#[derive(Clone, Serialize, Deserialize)]
pub struct RouteEntry<R> {
    /// Identifies the page scope. Replacing a route creates a fresh page.
    pub id: u64,
    /// Identifies the history position. Replacing a route preserves its position.
    pub position: u64,
    pub route: R,
}

impl<'de, R: Deserialize<'de>> Deserialize<'de> for RouteHistory<R> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored<R> {
            entries: Vec<RouteEntry<R>>,
            cursor: usize,
            next_id: u64,
        }

        let Stored { entries, cursor, next_id } = Stored::deserialize(deserializer)?;
        let mut ids = FxHashSet::default();
        let mut positions = FxHashSet::default();
        if cursor >= entries.len()
            || !entries
                .iter()
                .all(|entry| entry.position <= entry.id && entry.id < next_id && ids.insert(entry.id) && positions.insert(entry.position))
        {
            return Err(D::Error::custom("invalid navigator history"));
        }
        Ok(Self { entries, cursor, next_id })
    }
}

impl<R> RouteHistory<R> {
    fn new(route: R) -> Self {
        Self { entries: vec![RouteEntry { id: 0, position: 0, route }], cursor: 0, next_id: 1 }
    }

    fn current(&self) -> &RouteEntry<R> {
        &self.entries[self.cursor]
    }

    fn entry(&mut self, route: R, position: Option<u64>) -> RouteEntry<R> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).expect("route entry identifiers exhausted");
        RouteEntry { id, position: position.unwrap_or(id), route }
    }
}

type Guard<R> = dyn Fn(&mut ReadCx<'_>, &R, &R) -> bool;
type LeaveGuards<R> = RefCell<Vec<Weak<LeaveGuard<R>>>>;

struct LeaveGuard<R> {
    page: Option<u64>,
    allows: Box<Guard<R>>,
}

/// Nesting may put guards for several navigators under the same page scope.
struct RoutePage {
    navigator: ReactiveId,
    entry: u64,
    parent: Option<Arc<Self>>,
}

impl BuildCx<'_> {
    /// Creates a navigator at its root route.
    pub fn navigator<R: Clone + 'static>(&mut self, initial: R) -> Navigator<R> {
        Navigator { history: self.signal(RouteHistory::new(initial)), guards: self.signal(Arc::default()) }
    }

    /// Creates a navigator whose history and active position participate in `RestartState`.
    pub fn persisted_navigator<R: Clone + Serialize + DeserializeOwned + 'static>(
        &mut self,
        key: &'static str,
        initial: impl FnOnce() -> R,
    ) -> Navigator<R> {
        Navigator { history: self.persisted(key, || RouteHistory::new(initial())), guards: self.signal(Arc::default()) }
    }

    /// Registers a guard for this scope. Inside a route page it only runs while that page is active.
    /// Returning `false` vetoes navigation without changing the route or its history.
    pub fn on_before_leave<R: Clone + 'static>(&mut self, nav: Navigator<R>, allows: impl Fn(&mut ReadCx<'_>, &R, &R) -> bool + 'static) {
        let owner: Option<Arc<RoutePage>> = self.context();
        let page = std::iter::successors(owner.as_deref(), |page| page.parent.as_deref())
            .find(|page| page.navigator == nav.history.id())
            .map(|page| page.entry);
        let guard = Arc::new(LeaveGuard { page, allows: Box::new(allows) });
        let guards = self.sample(nav.guards);
        let mut guards = guards.borrow_mut();
        guards.retain(|guard| guard.strong_count() > 0);
        guards.push(Arc::downgrade(&guard));
        self.on_cleanup(move |_| drop(guard));
    }
}

impl<R: Clone + 'static> Navigator<R> {
    /// Pushes a new page and drops forward history. Returns whether navigation succeeded.
    pub fn push(self, w: &mut WriteCx<'_>, route: R) -> bool {
        if !self.allows(w, &route) {
            return false;
        }
        w.update(self.history, |history| {
            let entry = history.entry(route, None);
            history.entries.truncate(history.cursor + 1);
            history.entries.push(entry);
            history.cursor += 1;
        });
        true
    }

    /// Replaces the active page at its current history position.
    pub fn replace(self, w: &mut WriteCx<'_>, route: R) -> bool {
        if !self.allows(w, &route) {
            return false;
        }
        w.update(self.history, |history| {
            let entry = history.entry(route, Some(history.current().position));
            history.entries[history.cursor] = entry;
        });
        true
    }

    /// Goes back one position, keeping the previous page available in forward history.
    pub fn pop(self, w: &mut WriteCx<'_>) -> bool {
        self.go(w, -1)
    }

    /// Goes to the first page in the stack.
    pub fn pop_to_root(self, w: &mut WriteCx<'_>) -> bool {
        self.seek(w, 0)
    }

    /// Replaces the history, with the last supplied route active. An empty list leaves it unchanged.
    pub fn reset(self, w: &mut WriteCx<'_>, routes: impl IntoIterator<Item = R>) -> bool {
        let routes: Vec<_> = routes.into_iter().collect();
        if !routes.last().is_some_and(|route| self.allows(w, route)) {
            return false;
        }
        w.update(self.history, |history| {
            history.entries = routes.into_iter().map(|route| history.entry(route, None)).collect();
            history.cursor = history.entries.len() - 1;
        });
        true
    }

    /// Moves by a signed history offset. Out-of-range and zero offsets leave history unchanged.
    pub fn go(self, w: &mut WriteCx<'_>, delta: isize) -> bool {
        w.with(self.history, |history| history.cursor.checked_add_signed(delta)).is_some_and(|target| self.seek(w, target))
    }

    pub fn current(self, r: &mut ReadCx<'_>) -> R {
        r.with(self.history, |history| history.current().route.clone())
    }

    /// Returns the root through the active page, excluding forward entries.
    pub fn stack(self, r: &mut ReadCx<'_>) -> Vec<R> {
        r.with(self.history, |history| history.entries[..=history.cursor].iter().map(|entry| entry.route.clone()).collect())
    }

    /// Returns the number of pages in the active stack, including the root.
    pub fn depth(self, r: &mut ReadCx<'_>) -> usize {
        r.with(self.history, |history| history.cursor + 1)
    }

    pub fn can_go_back(self, r: &mut ReadCx<'_>) -> bool {
        r.with(self.history, |history| history.cursor > 0)
    }

    pub fn can_go_forward(self, r: &mut ReadCx<'_>) -> bool {
        r.with(self.history, |history| history.cursor + 1 < history.entries.len())
    }

    fn seek(self, w: &mut WriteCx<'_>, target: usize) -> bool {
        let route = w.with(self.history, |history| {
            history.entries.get(target).filter(|_| target != history.cursor).map(|entry| entry.route.clone())
        });
        if !route.as_ref().is_some_and(|route| self.allows(w, route)) {
            return false;
        }
        w.update(self.history, |history| history.cursor = target);
        true
    }

    fn allows(self, w: &mut WriteCx<'_>, to: &R) -> bool {
        let (current, from) = w.with(self.history, |history| (history.current().id, history.current().route.clone()));
        let guards = w.get(self.guards);
        w.read(|r| {
            guards
                .borrow()
                .iter()
                .filter_map(Weak::upgrade)
                .filter(|guard| guard.page.is_none_or(|page| page == current))
                .all(|guard| (guard.allows)(r, &from, to))
        })
    }

    #[doc(hidden)]
    pub fn active_page(self, r: &mut ReadCx<'_>) -> u64 {
        r.with(self.history, |history| history.current().id)
    }

    #[doc(hidden)]
    pub fn page_keys(self, r: &mut ReadCx<'_>) -> Vec<u64> {
        r.with(self.history, |history| history.entries.iter().map(|entry| entry.id).collect())
    }

    #[doc(hidden)]
    pub fn page(self, b: &mut BuildCx<'_>, id: u64) -> Option<(R, bool)> {
        b.read(|r| {
            r.with(self.history, |history| {
                history.entries.iter().enumerate().find(|(_, entry)| entry.id == id).map(|(index, entry)| (entry.route.clone(), index > 0))
            })
        })
    }

    #[doc(hidden)]
    pub fn provide_page(self, b: &mut BuildCx<'_>, entry: u64) {
        b.provide(Arc::new(RoutePage { navigator: self.history.id(), entry, parent: b.context() }));
    }

    #[doc(hidden)]
    pub fn history(self, world: &ReactiveWorld) -> Option<RouteHistory<R>> {
        world.is_alive(self.history.id()).then(|| world.value::<RouteHistory<R>>(self.history.id()).clone())
    }

    #[doc(hidden)]
    pub fn history_revision(self, world: &ReactiveWorld) -> Option<u64> {
        world.is_alive(self.history.id()).then(|| world.revision(self.history.id()).get())
    }
}
