//! The crate's integration tests in one binary: one link and one launch for all of them.

mod api;
mod cli;
mod harness;
#[cfg(unix)]
mod live;
mod skill;
