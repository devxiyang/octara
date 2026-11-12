//! The reactive core: a fine-grained signal graph with named state, incremental collections and
//! text buffers. Design: `docs/design/reactive-core.md`.

pub mod graph;
pub mod list;
pub mod seq;
pub mod text;
