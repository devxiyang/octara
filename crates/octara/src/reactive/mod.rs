//! The reactive core: a fine-grained signal graph with named state, incremental collections and
//! text buffers. Design: `docs/design/reactive-core.md`.

pub(crate) mod graph;
pub mod list;
pub(crate) mod log;
pub(crate) mod seq;
pub mod text;

pub use graph::{ReadSignal, Signal, Watch, memo, provide, state, untrack, use_context, watch};

/// Runs `cleanup` once when the current scope is disposed, including a hot reload.
/// Register during component construction to release work started later by its handlers.
/// Panics outside a scope. Cleanup runs before the scope's signals are disposed.
#[track_caller]
pub fn on_cleanup(cleanup: impl FnOnce() + 'static) {
    let at = std::panic::Location::caller();
    graph::rt(|runtime| {
        if !runtime.effect_allowed(at) {
            return;
        }
        assert!(runtime.current_scope().is_some(), "on_cleanup requires a component scope");
        runtime.on_cleanup(Box::new(cleanup));
    });
}
