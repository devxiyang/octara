//! A modal popup surface anchored to an ordinary element.

use crate::Model;
use octara::prelude::*;
use std::rc::Rc;

/// A themed popup anchored to `trigger`, controlled by `open`.
///
/// `id` names the trigger; the popup is `{id}-popover`. Component behavior updates `open`
/// before the trigger's application `on_press`, preserving callbacks supplied on the trigger
/// or added afterwards. The component owns `expanded`. Chain styles and put ordinary elements in
/// `content` to customize its contents. The surface supplies padding, border and shadow.
///
/// This is a modal popup: while open, focus stays inside and the rest of the window is inert.
/// Escape or a press outside dismisses it, restoring the previous focus (the trigger when it
/// opened the popup). It can also open inside another modal; dismissing it leaves that enclosing
/// modal open.
#[component]
#[track_caller]
pub fn popover(id: &str, open: impl Model<bool>, placement: Placement, trigger: El, content: impl Fn() -> View + 'static) -> El {
    let open = Rc::new(open);
    let (expanded, press, shown, dismiss) = (open.clone(), open.clone(), open.clone(), open);
    let panel_id = format!("{id}-popover");
    trigger
        .id(id.to_owned())
        .role(Role::Button)
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
                    .into()
            },
        )
}
