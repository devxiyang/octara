//! The reactive core: a fine-grained signal graph with named state, incremental collections and
//! live text. Design: `docs/design/reactive-core.md`.

pub mod graph;
pub mod list;
pub mod seq;
pub mod text;
