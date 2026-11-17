//! Allocation lifetime shared by App and Kernel. Wire encodings belong to adapters.

use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::{DefaultHasher, RandomState};
use std::fmt::{self, Display};
use std::hash::{BuildHasher, Hash, Hasher};
use std::rc::{Rc, Weak};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) fn new_namespace() -> u128 {
    static NONCE: LazyLock<u64> = LazyLock::new(|| RandomState::new().hash_one(std::process::id()));
    static NEXT: AtomicU64 = AtomicU64::new(1);
    (u128::from(*NONCE) << 64)
        | u128::from(NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1)).expect("namespace identity exhausted"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TreeIdentity {
    pub namespace: u128,
    pub window: u64,
    pub generation: u64,
}

impl TreeIdentity {
    pub fn new(namespace: u128, window: u64) -> Self {
        Self { namespace, window, generation: 0 }
    }

    pub fn advance(&mut self) {
        self.generation = self.generation.checked_add(1).expect("tree incarnation exhausted");
    }
}

/// An opaque logical row identity used by document-selection paths.
/// Display labels are separate from the framework's canonical key and region identity.
#[derive(Clone)]
pub struct RowKey {
    identity: Rc<KeyIdentity>,
    value: Weak<KeyValue>,
    label: Rc<str>,
    namespace: Option<Rc<RowNamespace>>,
}

struct KeyIdentity(Rc<Cell<bool>>);

impl Drop for KeyIdentity {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

struct KeyValue {
    identity: Weak<KeyIdentity>,
    value: Box<dyn Any>,
    equals: fn(&dyn Any, &dyn Any) -> bool,
}

impl RowKey {
    pub(crate) fn value<K: 'static>(&self) -> Option<Rc<K>> {
        self.value.upgrade()?.value.downcast_ref::<Rc<K>>().cloned()
    }

    pub(crate) fn in_region(mut self, namespace: Rc<RowNamespace>) -> Self {
        self.namespace = Some(namespace);
        self
    }

    pub(crate) fn local(&self) -> Self {
        Self { namespace: None, ..self.clone() }
    }
}

impl PartialEq for RowKey {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.identity, &other.identity) && self.namespace == other.namespace
    }
}

impl Eq for RowKey {}

impl Hash for RowKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Rc::as_ptr(&self.identity).hash(state);
        self.namespace.hash(state);
    }
}

impl Display for RowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.label.fmt(f)
    }
}

impl fmt::Debug for RowKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RowKey").field(&self.label).finish()
    }
}

/// A runtime-local index: live path tokens retain keys through weak token registrations.
/// Tokens never own user values, so kernel and graph borrows can compare and release them.
/// Callers isolate user Hash/Eq/Display code; no such callback runs under a registry borrow.
#[derive(Default)]
pub(crate) struct KeyInterner(RefCell<KeyIndex>, Rc<Cell<bool>>);

struct KeyIndex {
    buckets: HashMap<(TypeId, u64), Vec<Rc<KeyValue>>>,
    count: usize,
    sweep_at: usize,
}

impl Default for KeyIndex {
    fn default() -> Self {
        Self { buckets: HashMap::new(), count: 0, sweep_at: 64 }
    }
}

impl KeyIndex {
    fn prune(&mut self) -> Vec<Rc<KeyValue>> {
        let mut retired = Vec::new();
        let mut count = 0;
        self.buckets.retain(|_, keys| {
            retired.extend(keys.extract_if(.., |key| key.identity.strong_count() == 0));
            count += keys.len();
            !keys.is_empty()
        });
        self.count = count;
        self.sweep_at = count.saturating_mul(2).max(64);
        retired
    }
}

impl KeyInterner {
    pub(crate) fn intern<K: Eq + Hash + Display + 'static>(&self, key: Rc<K>) -> RowKey {
        let mut hash = DefaultHasher::new();
        key.hash(&mut hash);
        let bucket = (TypeId::of::<K>(), hash.finish());
        let label = key.to_string().into();
        let candidates: Vec<_> = {
            let index = self.0.borrow();
            index
                .buckets
                .get(&bucket)
                .into_iter()
                .flatten()
                .filter_map(|value| value.identity.upgrade().map(|identity| (identity, value.clone())))
                .collect()
        };
        for (identity, candidate) in candidates {
            if (candidate.equals)(&*candidate.value, &*key) {
                return RowKey { identity, value: Rc::downgrade(&candidate), label, namespace: None };
            }
        }
        let identity = Rc::new(KeyIdentity(self.1.clone()));
        let value = Rc::new(KeyValue {
            identity: Rc::downgrade(&identity),
            value: Box::new(key),
            equals: |a, b| a.downcast_ref::<Rc<K>>().map(Rc::as_ref) == b.downcast_ref::<K>(),
        });
        let weak = Rc::downgrade(&value);
        let retired = {
            let mut index = self.0.borrow_mut();
            let retired = if index.count >= index.sweep_at { index.prune() } else { Vec::new() };
            index.buckets.entry(bucket).or_default().push(value);
            index.count += 1;
            retired
        };
        drop(retired);
        RowKey { identity, value: weak, label, namespace: None }
    }

    pub(crate) fn prune(&self) {
        if !self.1.replace(false) {
            return;
        }
        let retired = self.0.borrow_mut().prune();
        drop(retired);
    }

    pub(crate) fn clear(&self) {
        let retired = std::mem::take(&mut *self.0.borrow_mut());
        drop(retired);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.0.borrow().count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RegionKind {
    Branch,
    Each,
    Virtual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RegionId {
    pub kind: RegionKind,
    pub index: u32,
}

/// A list's stable owner path and region, shared by all of its selection keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RowNamespace {
    pub owner: Vec<ScopeSegment>,
    pub region: RegionId,
}

/// Ownership identity is structural. Formatting a segment cannot create another segment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ScopeSegment {
    App,
    Window(u64),
    Scope(RowKey),
    Branch {
        region: RegionId,
        key: RowKey,
    },
    Row {
        region: RegionId,
        key: RowKey,
        offscreen: bool,
    },
    #[cfg(test)]
    Test(String),
}

#[cfg(test)]
impl From<&str> for ScopeSegment {
    fn from(value: &str) -> Self {
        Self::Test(value.to_owned())
    }
}

impl Display for ScopeSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::App => f.write_str("app"),
            Self::Window(id) => write!(f, "window{id}"),
            Self::Scope(key) => write!(f, "scope[{key}]"),
            Self::Branch { region, key } => write!(f, "branch#{}[{key}]", region.index),
            Self::Row { region, key, offscreen } => {
                write!(f, "{:?}#{}{}[{key}]", region.kind, region.index, if *offscreen { "/offscreen" } else { "" })
            }
            #[cfg(test)]
            Self::Test(name) => f.write_str(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Key {
        id: u32,
        label: &'static str,
        dropped: Rc<Cell<usize>>,
        interner: Weak<KeyInterner>,
    }

    impl PartialEq for Key {
        fn eq(&self, other: &Self) -> bool {
            assert!(self.interner.upgrade().unwrap().0.try_borrow_mut().is_ok());
            self.id == other.id
        }
    }

    impl Eq for Key {}

    impl Hash for Key {
        fn hash<H: Hasher>(&self, state: &mut H) {
            assert!(self.interner.upgrade().unwrap().0.try_borrow_mut().is_ok());
            0u8.hash(state);
        }
    }

    impl Display for Key {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            assert!(self.interner.upgrade().unwrap().0.try_borrow_mut().is_ok());
            f.write_str(self.label)
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            assert!(self.interner.upgrade().unwrap().0.try_borrow_mut().is_ok());
            self.dropped.set(self.dropped.get() + 1);
        }
    }

    #[test]
    fn collisions_resolve_by_equality_and_tokens_never_own_user_values() {
        let interner = Rc::new(KeyInterner::default());
        let dropped = Rc::new(Cell::new(0));
        let key = |id, label| Rc::new(Key { id, label, dropped: dropped.clone(), interner: Rc::downgrade(&interner) });
        let one = interner.intern(key(1, "same"));
        let equal = interner.intern(key(1, "renamed"));
        let two = interner.intern(key(2, "same"));
        assert_eq!(one, equal);
        assert_ne!(one, two);
        assert_eq!(equal.to_string(), "renamed");
        assert_eq!(dropped.get(), 1, "equal temporary keys retire outside the index borrow");
        drop((one, equal, two));
        assert_eq!(dropped.get(), 1, "dropping path/kernel tokens cannot drop a user key");
        interner.prune();
        assert_eq!(dropped.get(), 3);
        assert_eq!(interner.len(), 0);
        let external = interner.intern(key(3, "still observed"));
        interner.clear();
        assert!(external.value::<Key>().is_none(), "shutdown releases keys even if observers retain tokens");
        assert_eq!(dropped.get(), 4);
    }

    #[test]
    fn interner_churn_releases_unused_buckets_without_requiring_a_later_mount() {
        let interner = KeyInterner::default();
        for id in 0..1_000 {
            let token = interner.intern(Rc::new(id));
            drop(token);
            interner.prune();
            assert_eq!(interner.len(), 0);
        }
    }
}
