//! The ordinary Chat UI with isolated 1k/10k mixed history and a controlled live answer.

#[path = "support/long_session.rs"]
mod long_session;

use long_session::{LongSession, Payload, seed};
use octara::prelude::{Size, WindowOptions};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::var_os("OCTARA_CHAT_DIR").map(PathBuf::from).ok_or("set a new isolated OCTARA_CHAT_DIR")?;
    let count = std::env::var("OCTARA_CHAT_MESSAGES").unwrap_or_else(|_| "1000".into()).parse()?;
    let store = seed(&directory, count)?;
    let model = LongSession::new(true);
    model.set_payload(Payload::Mixed, 1024)?;
    let mut window = WindowOptions::default();
    window.presentation.title = format!("Octara Chat long session — {count}").into();
    window.viewport.size = Size::new(1000.0, 720.0);
    octara_host_native::run(window, move |b| model.view(b, Some(store)))?;
    Ok(())
}
