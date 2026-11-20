//! Opens an example app in a window:
//! `cargo run -p octara-examples --bin window -- [showcase [card]|counter|styles|activation|handlers|bindings|gestures|todo|chat|settings|contacts|chat_virtual|windows|undo|gpu_lab]`
//! (`showcase web` shows the web view card alone; the cards are `showcase::CARDS`).
//! `gpu_lab replay [60|120]` waits for Inspector to invoke `shader-lab.replay.start`.
//! `gpu_lab replay 120 stress` repeats the workload in a larger window.
//! `gpu_lab input-pressure` runs native input probes next to the animated GPU canvas.

use octara::view::View;
use octara_examples::{apps, gpu_lab, showcase};

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "showcase".into());
    if name == "gpu_lab" {
        let mut args = std::env::args().skip(2);
        let mut options = gpu_lab::options();
        let mode = args.next();
        if mode.as_deref() == Some("input-pressure") {
            options.title = gpu_lab::input_pressure::TITLE.into();
            options.size = (1440.0, 900.0);
            octara_shell::run_with(options, gpu_lab::input_pressure::view);
        } else if mode.as_deref() == Some("replay") {
            let (hz, stress) = match (args.next().as_deref(), args.next().as_deref(), args.next()) {
                (None | Some("120"), None, None) => (120, false),
                (Some("60"), None, None) => (60, false),
                (Some("120"), Some("stress"), None) => (120, true),
                _ => {
                    eprintln!("Usage: window gpu_lab replay [60|120]\n       window gpu_lab replay 120 stress");
                    std::process::exit(2);
                }
            };
            let (width, height) = if stress { gpu_lab::replay::STRESS_SIZE } else { gpu_lab::replay::SIZE };
            options.size = (width as f32, height as f32);
            if stress {
                octara_shell::run_with(options, gpu_lab::replay::stress_view);
            } else {
                octara_shell::run_with(options, move || gpu_lab::replay::live_view(hz));
            }
        } else {
            octara_shell::run_with(options, gpu_lab::view);
        }
        return;
    }
    if name == "undo" {
        let options =
            octara_shell::Options { title: apps::UNDO_DOCUMENT_TITLE.into(), size: apps::UNDO_DOCUMENT_SIZE, ..Default::default() };
        octara_shell::run_with(options, || apps::undo_document("A"));
        return;
    }
    let card = std::env::args().nth(2);
    let view: Box<dyn Fn() -> View> = match name.as_str() {
        "failure" => Box::new(octara_examples::failure::view),
        "counter" => Box::new(|| apps::counter().into()),
        "styles" => Box::new(apps::styles),
        "activation" => Box::new(octara_examples::activation::view),
        "handlers" => Box::new(octara_examples::handlers::view),
        "bindings" => Box::new(octara_examples::handlers::bindings),
        "gestures" => Box::new(apps::gestures),
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
