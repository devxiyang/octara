//! The AI chat reference app ([app guide](../README.md)): conversations in a
//! sidebar, a transcript following its answer as it streams, a composer that queues what is sent
//! while an answer streams. It is built only from Octara's general mechanisms and the kit's
//! components; the chat itself (its data, its model, its flows) lives here.

pub mod chat;
pub mod engine;
pub mod model;
pub mod network;
pub mod settings;
mod startup;
pub mod store;
mod ui;

pub use chat::{Chat, ModelEntry, Options, Queued};
pub use startup::app;
pub use ui::{root, view};
