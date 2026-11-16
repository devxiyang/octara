//! Allocation lifetime shared by App and Kernel. Wire encodings belong to adapters.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
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
