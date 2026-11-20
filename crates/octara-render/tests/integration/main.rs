//! The crate's integration tests in one binary: one link and one launch for all of them.

mod cpu;
#[cfg(feature = "gpu")]
mod gpu;
mod paths;
