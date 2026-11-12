//! The AI chat reference app ([core](../../../docs/design/core.md) step 7): conversations in a
//! sidebar, a transcript following its answer as it streams, a composer that queues what is sent
//! while an answer streams. It is built only from Octara's general mechanisms and the kit's
//! components; the chat itself (its data, its model, its flows) lives here.

pub mod chat;
pub mod engine;
pub mod model;
pub mod store;
mod ui;

pub use chat::{Chat, Options, Queued};
pub use ui::root;

/// The app with the scripted model and nothing saved (what `octara check` and tests drive).
pub fn app() -> octara::view::View {
    root(Options::default)
}
