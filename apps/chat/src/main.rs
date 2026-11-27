//! The AI chat reference app in a window: `cargo run -p octara-chat`. Conversations are kept in
//! the platform's application data directory (`OCTARA_CHAT_DIR` overrides it).

use std::process::ExitCode;

fn main() -> ExitCode {
    match octara_chat::app().and_then(|app| octara_host_native::run_app(app).map_err(|error| error.to_string())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("octara-chat: {error}");
            ExitCode::FAILURE
        }
    }
}
