//! Markdown streaming in: an answer arrives a few characters at a time; blocks before the one
//! growing keep their elements, open emphasis shows styled, and the whole answer selects and
//! copies (`mod+c` as text, `mod+alt+c` as Markdown).

use octara::prelude::*;
use octara_kit::markdown::markdown;
use octara_kit::primary_button;
use std::time::Duration;

const ANSWER: &str = "### Streaming Markdown\n\nEach block is an element of its own, so a token re-parses and re-lays out **only the block it lands in**; the ones above keep their nodes.\n\n- Open *emphasis*, `code` and [links](https://commonmark.org) show styled as they arrive\n- Markers never flash\n- [x] Tables, quotes and code blocks too\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\n| Block | Key |\n|---|---|\n| paragraph | `b{i}` |\n| list item | `i{j}` |\n\n> Select across blocks: `mod+c` copies the text, `mod+alt+c` the Markdown.\n";

#[component]
pub fn card() -> El {
    let answer = live_text("answer", || ANSWER.to_owned());
    let streaming = state("streaming", || false);
    let stream = answer.clone();
    super::section("Markdown")
        .w(520.0)
        .child(
            primary_button(move || if streaming.get() { "Streaming…".to_owned() } else { "Stream again".to_owned() })
                .id("stream")
                .self_start()
                .disabled(move || streaming.get())
                .on_press(move || {
                    streaming.set(true);
                    stream.set("");
                    let stream = stream.clone();
                    spawn(async move {
                        let chars: Vec<char> = ANSWER.chars().collect();
                        for piece in chars.chunks(3) {
                            sleep(Duration::from_millis(25)).await;
                            stream.append(&piece.iter().collect::<String>());
                        }
                        streaming.set(false);
                    });
                }),
        )
        .child(markdown(&answer).id("answer"))
}
