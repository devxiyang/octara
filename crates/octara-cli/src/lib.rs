//! The `octara` command's library: the framework's API (read from the source an app depends on),
//! its usage catalog and diagnostics, the API reference for agents, the skill and the harness.

pub mod api;
pub mod catalog;
pub mod harness;
pub mod live;
pub mod reference;
pub mod skill;

/// The examples below and in `docs/start-here.md` run as doctests.
///
/// A custom binding reads its current value and validates a proposed write.
///
/// ```
/// use octara::runtime::ReactiveWorld;
///
/// let mut world = ReactiveWorld::new();
/// let scope = world.root_scope();
/// let mut build = world.build(scope, None);
/// let level = build.signal(90);
/// let binding = build.binding(
///     move |read| read.get(level),
///     move |write, proposed| write.set(level, proposed.clamp(0, 100)),
/// );
/// let mut write = world.write(scope, None);
/// write.update(binding, |value| *value += 20);
/// assert_eq!(write.get(binding), 100);
/// assert!(!write.mutate(binding, |_| false));
/// ```
#[doc = include_str!(concat!(env!("OUT_DIR"), "/start-here.md"))]
#[cfg(doctest)]
pub struct StartHere;
