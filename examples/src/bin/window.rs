//! Opens an example app in a window: `cargo run -p octara-examples --bin window -- [showcase|counter|todo|chat|settings|contacts|chat_virtual]`.

use octara::view::View;
use octara_examples::apps;

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "showcase".into());
    let view: fn() -> View = match name.as_str() {
        "counter" => apps::counter,
        "chat" => || apps::chat(50),
        "settings" => apps::settings,
        "contacts" => || apps::contacts(200),
        "chat_virtual" => || apps::chat_virtual(100_000),
        "todo" => || apps::todo(30),
        _ => octara_examples::showcase::showcase,
    };
    octara_shell::run(&format!("Octara — {name}"), view);
}
