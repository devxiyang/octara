//! The `octara` command's library: the framework's API (read from the source an app depends on),
//! its usage catalog and diagnostics, the API reference for agents, the skill and the harness.

pub mod api;
pub mod catalog;
pub mod harness;
pub mod reference;
pub mod skill;

/// `docs/start-here.md`, whose code blocks run as doctests.
#[doc = include_str!("../../../docs/start-here.md")]
#[cfg(doctest)]
pub struct StartHere;
