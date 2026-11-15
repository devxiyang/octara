//! A controlled dialog composed on the window's existing overlay and modal primitives.

use crate::{Model, icon_button};
use octara::prelude::*;
use std::rc::Rc;

/// Supporting text and outside-press behavior for a [`dialog`]. Escape and the close button
/// always dismiss through the supplied model. The default allows outside presses to dismiss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogOptions {
    pub description: String,
    pub click_outside: bool,
}

impl Default for DialogOptions {
    fn default() -> Self {
        Self { description: String::new(), click_outside: true }
    }
}

/// A centered, named modal controlled by `open`.
///
/// `id` names the actual dialog. Its visible heading is `{id}-title`, supporting text
/// `{id}-description`, close button `{id}-close`, scrollable body `{id}-body`, and decorative
/// scrim `{id}-scrim`. The panel fits the viewport with a 16-point margin and a maximum width
/// of 480 points. Its header stays visible while long content scrolls inside the body.
///
/// The runtime traps focus, makes the rest of the window inert, and restores previous focus
/// when it closes. A content control's `autofocus()` wins, otherwise the first text field or
/// Tab stop takes focus. Nested popovers and dialogs use the same modal stack; Escape closes
/// the innermost one, after any composing input method has handled it.
///
/// Closing unmounts `content`: its local state, tasks and editor history are disposed. Keep a
/// draft in the application's state and pass its model into the content to preserve it across
/// openings. A new opening creates new node targets. The component owns no draft or validation.
#[component]
#[track_caller]
pub fn dialog(
    id: &str,
    title: impl Into<String>,
    open: impl Model<bool>,
    options: DialogOptions,
    content: impl Fn() -> View + 'static,
) -> View {
    let id = id.to_owned();
    let title = title.into();
    let open = Rc::new(open);
    let shown = open.clone();
    // This anchor owns the overlay's scope without participating in the surrounding layout.
    // The panel itself is mounted in the window layer, even inside clipped or scrolled content.
    el().absolute()
        .size(0.0)
        .a11y_hidden()
        .overlay_window(
            move || shown.get(),
            move || {
                let (close, dismiss) = (open.clone(), open.clone());
                let description_id = format!("{id}-description");
                let description = options.description.clone();
                let panel = col()
                    .id(id.clone())
                    .role(Role::Dialog)
                    .label(title.clone())
                    .description(options.description.clone())
                    .w_full()
                    .max_w(480.0)
                    .min_w(0.0)
                    .style_with(|p| p.max_h((viewport().get().1 - 32.0).max(0.0)))
                    .rounded_lg()
                    .border()
                    .border_color(Token::Border)
                    .bg(Token::Surface)
                    .text_color(Token::Text)
                    .shadow_xl()
                    .modal(options.click_outside, move || dismiss.set(false))
                    .child(
                        row()
                            .w_full()
                            .min_w(0.0)
                            .flex_shrink_0()
                            .items_center()
                            .gap_3()
                            .p_4()
                            .child(
                                text(title.clone())
                                    .id(format!("{id}-title"))
                                    .role(Role::Heading)
                                    .level(2)
                                    .flex_1()
                                    .min_w(0.0)
                                    .font_semibold(),
                            )
                            .child(icon_button(octara_icons::X, "Close").id(format!("{id}-close")).on_press(move || close.set(false))),
                    )
                    .child(
                        col()
                            .id(format!("{id}-body"))
                            .w_full()
                            .min_w(0.0)
                            .flex_shrink()
                            .overflow_y_scroll()
                            .gap_3()
                            .px_4()
                            .pb_4()
                            .children(
                                (!description.trim().is_empty())
                                    .then(|| text(description).id(description_id).text_sm().text_color(Token::TextMuted)),
                            )
                            .child(content()),
                    );
                col()
                    .absolute()
                    .inset_0()
                    .size_full()
                    .p_4()
                    .items_center()
                    .justify_center()
                    .child(
                        el().id(format!("{id}-scrim")).absolute().inset_0().size_full().bg(Color::BLACK.multiply_alpha(0.35)).a11y_hidden(),
                    )
                    .child(panel)
                    .into()
            },
        )
        .into()
}
