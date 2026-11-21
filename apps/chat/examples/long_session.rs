//! The ordinary Chat UI with isolated 1k/10k mixed history and a controlled live answer.

#[path = "support/long_session.rs"]
mod long_session;

use long_session::{LongSession, seed};
use octara::window::WindowOptions;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::var_os("OCTARA_CHAT_DIR").map(PathBuf::from).ok_or("set a new isolated OCTARA_CHAT_DIR")?;
    let count = std::env::var("OCTARA_CHAT_MESSAGES").unwrap_or_else(|_| "1000".into()).parse()?;
    let store = seed(&directory, count)?;
    let model = LongSession::default();
    octara_shell::run_with(
        octara_shell::Options {
            window: WindowOptions::new(format!("Octara Chat long session — {count}")).size(1000.0, 720.0),
            ..Default::default()
        },
        move || model.view(Some(store.clone())),
    );
    Ok(())
}
