//! The crate's integration tests in one binary: one link and one launch for all of them.

mod api;
mod cli;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod dev;
mod harness;
#[cfg(unix)]
mod live;
mod modules;
mod skill;
