//! The `octara` command's library: the framework's API (`api.json`), its usage catalog and
//! diagnostics, answered from files checked into this crate.

pub mod api;
pub mod catalog;
pub mod skill;

/// `docs/start-here.md`, whose code blocks run as doctests.
#[doc = include_str!("../../../docs/start-here.md")]
#[cfg(doctest)]
pub struct StartHere;
