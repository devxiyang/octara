//! Opens an example app in a window:
//! `cargo run -p octara-examples --bin window -- [showcase [card]|counter|todo|chat|settings|contacts|chat_virtual|windows]`
//! (`showcase web` shows the web view card alone; the cards are `showcase::CARDS`).

use octara::view::View;
use octara_examples::{apps, showcase};

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "showcase".into());
    let card = std::env::args().nth(2);
    let view: Box<dyn Fn() -> View> = match name.as_str() {
        "counter" => Box::new(apps::counter),
        "chat" => Box::new(|| apps::chat(50)),
        "settings" => Box::new(apps::settings),
        "contacts" => Box::new(|| apps::contacts(200)),
        "chat_virtual" => Box::new(|| apps::chat_virtual(100_000)),
        "todo" => Box::new(|| apps::todo(30)),
        "windows" => Box::new(apps::windows),
        _ => match card {
            Some(card) => {
                if !showcase::CARDS.iter().any(|(n, _)| *n == card) {
                    let names: Vec<_> = showcase::CARDS.iter().map(|(n, _)| *n).collect();
                    eprintln!("no showcase card {card:?}; the cards: {}", names.join(", "));
                    std::process::exit(2);
                }
                Box::new(move || showcase::page(&card).expect("a card"))
            }
            None => Box::new(showcase::showcase),
        },
    };
    octara_shell::run(&format!("Octara — {name}"), view);
}
