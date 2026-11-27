//! The crate's integration tests in one binary: one link and one launch for all of them.

mod cpu;
mod fixtures;
#[cfg(all(feature = "gpu", not(target_arch = "wasm32")))]
mod gpu;
mod paths;
