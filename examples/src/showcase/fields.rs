//! Text input: multi-line notes, a chat composer, a password.

use super::section;
use octara::prelude::*;
use octara_kit::{primary_button, switch, text_field};

#[component]
pub fn card() -> El {
    let notes_text =
        state("notes_text", || "Notes wrap at the field's width and the field grows with them.\nA second paragraph.".to_string());
    let message = state("message", String::new);
    let last_sent = state("last_sent", String::new);
    let password = state("password", || "correct horse".to_string());
    let show_password = state("show_password", || false);
    let send = move || {
        let m = message.get();
        if !m.trim().is_empty() {
            last_sent.set(m);
            message.set(String::new());
        }
    };
    section("Text input")
        .child(text("Multi-line: wraps, grows from 3 to 6 lines, then scrolls inside").text_xs().text_color(Token::TextMuted))
        .child(text_field(notes_text).id("notes").label("Notes").rows(3, 6).w(320).flex_shrink())
        .child(text("Chat composer: Enter sends, Shift+Enter breaks the line").text_xs().text_color(Token::TextMuted))
        .child(
            row()
                .gap_2()
                .items_end()
                .child(text_field(message).id("message").label("Message").submit_on_enter().rows(1, 4).w(260).flex_shrink().on_submit(send))
                .child(primary_button("Send").id("send").on_press(send)),
        )
        .child(
            text(move || if last_sent.get().is_empty() { "Nothing sent yet".to_string() } else { format!("Sent: {}", last_sent.get()) })
                .text_sm()
                .id("sent"),
        )
        .child(text("Password: bullets, no copy, no undo, no input method").text_xs().text_color(Token::TextMuted))
        .child(
            row()
                .gap_3()
                .items_center()
                .child(
                    text_field(password)
                        .id("password")
                        .label("Password")
                        .secure()
                        .revealed(move || show_password.get())
                        .w(200)
                        .flex_shrink(),
                )
                .child(switch(show_password).label("Show password"))
                .child(text("Show").text_sm()),
        )
}
