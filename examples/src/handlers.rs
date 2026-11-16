//! Real-window checks for bound input and editor-owned suggestions with application callbacks.
use octara::prelude::*;
use octara_kit::{PopupItem, popup_list, text_field};

#[component]
pub fn view() -> View {
    let value = state("value", String::new);
    let seen = state("seen", String::new);
    let keys = state("keys", || 0u32);
    let blur = state("blur", String::new);
    let chosen = state("chosen", String::new);
    let open = state("open", || false);
    col()
        .size_full()
        .p_6()
        .gap_4()
        .bg(Token::Surface)
        .child(text("Component and application callbacks").text_xl())
        .child(text(move || format!("Model: {}", value.get())).id("model"))
        .child(text(move || format!("Seen: {}", seen.get())).id("seen"))
        .child(text(move || format!("Keys: {}", keys.get())).id("keys"))
        .child(text(move || format!("Blur: {}", blur.get())).id("blur"))
        .child(text(move || format!("Chosen: {}", chosen.get())).id("chosen"))
        .child(popup_list(
            "editor",
            "Editor",
            open,
            text_field(value)
                .w_full()
                .on_input(move |_| seen.set(value.get()))
                .on_key(move |_| {
                    keys.update(|n| *n += 1);
                    false
                })
                .on_blur(move || blur.set(format!("open={}", open.get()))),
            || vec![PopupItem::new("one", "One"), PopupItem::new("two", "Two")],
            move |id| chosen.set(id),
        ))
        .child(text_input().id("other").label("Other field").h(44).value("Move focus here"))
        .into()
}
