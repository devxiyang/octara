//! Launch an individual Octara example using the native host.

use octara::prelude::*;
use octara_examples::native;
use octara_examples::{apps, failure, gpu_lab, showcase, window_options};
use octara_host_native::ShellError;

fn main() -> Result<(), ShellError> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "showcase".into());
    if name == "counter" {
        return native::run_app(apps::counter_app());
    }
    if name == "gpu_lab" {
        let mut options = gpu_lab::options();
        return match args.next().as_deref() {
            Some("input-pressure") => {
                options.presentation.title = gpu_lab::input_pressure::TITLE.into();
                options.viewport.size = Size::new(1440.0, 900.0);
                native::run(options, gpu_lab::input_pressure::view)
            }
            Some("replay") => {
                let rate = match args.next().as_deref() {
                    None | Some("120") => 120,
                    Some("60") => 60,
                    _ => return Err(ShellError("Usage: window gpu_lab replay [60|120] [stress]".into())),
                };
                let stress = match args.next().as_deref() {
                    None => false,
                    Some("stress") => true,
                    _ => return Err(ShellError("Usage: window gpu_lab replay [60|120] [stress]".into())),
                };
                let (width, height) = if stress { gpu_lab::replay::STRESS_SIZE } else { gpu_lab::replay::SIZE };
                options.viewport.size = Size::new(width as f32, height as f32);
                native::run(
                    options,
                    move |b| {
                        if stress { gpu_lab::replay::stress_view(b, rate) } else { gpu_lab::replay::live_view(b, rate) }
                    },
                )
            }
            None => native::run(options, gpu_lab::view),
            Some(mode) => Err(ShellError(format!("Unknown Shader Lab mode {mode:?}"))),
        };
    }
    if name == "undo" {
        let (width, height) = apps::UNDO_DOCUMENT_SIZE;
        return native::run(window_options(apps::UNDO_DOCUMENT_TITLE, width, height), |b| apps::undo_document(b, "A"));
    }
    let options = window_options(format!("Octara — {name}"), 800.0, 700.0);
    match name.as_str() {
        "failure" => native::run(options, failure::view),
        "styles" => native::run(options, apps::styles),
        "gestures" => native::run(options, apps::gestures),
        "chat" => native::run(options, |b| apps::chat(b, 50)),
        "settings" => native::run(options, apps::settings),
        "contacts" => native::run(options, |b| apps::contacts(b, 200)),
        "contacts_virtual" => native::run(options, |b| apps::contacts_virtual(b, 100_000)),
        "chat_virtual" => native::run(options, |b| apps::chat_virtual(b, 100_000)),
        "todo" => native::run(options, |b| apps::todo(b, 30)),
        "windows" => native::run(options, apps::windows),
        "showcase" => match args.next() {
            Some(card) if showcase::CARDS.iter().any(|(name, _)| *name == card) => {
                native::run(options, move |b| showcase::page(b, &card).expect("a known showcase card"))
            }
            Some(card) => Err(ShellError(format!("Unknown showcase card {card:?}"))),
            None => native::run(options, showcase::showcase),
        },
        _ => Err(ShellError(format!("Unknown example {name:?}"))),
    }
}
