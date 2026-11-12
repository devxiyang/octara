//! Text editing: a note list with add, remove on hover, selection and undo.

use super::section;
use octara::prelude::*;
use octara_kit::{ghost_button, primary_button, text_field};

#[component]
pub fn card() -> El {
    let notes = list("notes", || vec![(1u32, "Try Tab, arrows and Escape".to_owned())], |n: &(u32, String)| n.0);
    let next = state("next", || 2u32);
    let draft = state("draft", String::new);
    let add = move || {
        let t = draft.get().trim().to_owned();
        if t.is_empty() {
            return;
        }
        let id = next.get();
        next.set(id + 1);
        notes.push((id, t));
        draft.set(String::new());
    };
    section("Text editing")
        .child(
            row()
                .gap_2()
                .child(text_field(draft).id("note").label("New note").w(260).flex_shrink().on_submit(add))
                .child(primary_button("Add").id("add").on_press(add)),
        )
        .child(col().gap_1().child(each(
            move || notes.source(),
            move |id, n| {
                let id = *id;
                // The Remove button shows while the row is hovered.
                row()
                    .group("note")
                    .gap_2()
                    .items_center()
                    .child(text(move || n.with(|n| n.1.clone())).flex_shrink().truncate().w(300))
                    .child(ghost_button("Remove").opacity(0.0).group_hover("note", |p| p.opacity(1.0)).on_press(move || notes.remove(&id)))
                    .into()
            },
        )))
        .child(text("Select text with the mouse, double-click a word, ⌘Z to undo, ⌘C / ⌘V.").text_xs().text_color(Token::TextMuted))
}
