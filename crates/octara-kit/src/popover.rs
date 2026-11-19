//! A modal popup surface anchored to an ordinary element.

use crate::Model;
use octara::prelude::*;
use std::rc::Rc;

/// A themed popup anchored to `trigger`, controlled by `open`.
///
/// `id` names the trigger; the popup is `{id}-popover`. The supplied element defines its
/// accessible role, name, and control relationships. Use [`crate::button`] for a standard trigger.
/// The popover makes the trigger focusable and binds `expanded` to `open`. Its behavior updates
/// `open` before the trigger's application `on_press`, including callbacks added afterwards.
/// The content factory accepts any [`IntoView`]. The surface supplies padding, border and shadow.
///
/// This is a modal popup: while open, focus stays inside and the rest of the window is inert.
/// Escape or a press outside dismisses it, restoring the previous focus (the trigger when it
/// opened the popup). It can also open inside another modal; dismissing it leaves that enclosing
/// modal open.
#[ui]
#[track_caller]
pub fn popover<V: IntoView>(id: &str, open: impl Model<bool>, placement: Placement, trigger: El, content: impl Fn() -> V + 'static) -> El {
    let open = Rc::new(open);
    let (expanded, press, shown, dismiss) = (open.clone(), open.clone(), open.clone(), open);
    let panel_id = format!("{id}-popover");
    trigger
        .id(id.to_owned())
        .focusable()
        .expanded(move || expanded.get())
        .behavior(|b| b.on_press(move || press.set(!press.get())))
        .overlay(
            move || shown.get(),
            placement,
            move || {
                let dismiss = dismiss.clone();
                col()
                    .id(panel_id.clone())
                    .role(Role::Group)
                    .p_2()
                    .gap_2()
                    .rounded_md()
                    .border()
                    .border_color(Token::Border)
                    .bg(Token::Surface)
                    .text_color(Token::Text)
                    .shadow_lg()
                    .modal(true, move || dismiss.set(false))
                    .child(content())
            },
        )
}
