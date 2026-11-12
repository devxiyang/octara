//! The AI chat reference app in a window: `cargo run -p octara-chat`. Conversations are kept in
//! the platform's application data directory (`OCTARA_CHAT_DIR` overrides it).

use octara_chat::{Options, root, store::Store};

fn main() {
    let options = || Options { store: Store::default_dir().map(Store::new), ..Options::default() };
    octara_shell::run_with(octara_shell::Options { title: "Octara Chat".into(), size: (1000.0, 720.0), ..Default::default() }, move || {
        root(options)
    });
}
