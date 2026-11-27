//! Styled modal dialogs with Headless focus and dismissal.

use crate::{KitLocale, icon_button};
use octara::prelude::*;
use octara_headless::{DialogState, close::close_button};

/// Supporting text and outside-press behavior for a dialog.
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

/// A centered, named modal controlled by the open binding.
/// Content receives a scoped close handle and is mounted fresh on each opening.
#[ui]
pub fn dialog<V: IntoView + 'static>(
    id: &str,
    title: impl Into<String>,
    open: Binding<bool>,
    options: DialogOptions,
    content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
) -> View {
    let state = DialogState::controlled(open).dismiss_on_outside_press(options.click_outside);
    modal(state, id, title.into(), options.description, content)
}

/// A modal for an important message or confirmation. Outside presses leave the dialog open.
#[ui]
pub fn alert_dialog<V: IntoView + 'static>(
    id: &str,
    title: impl Into<String>,
    open: Binding<bool>,
    description: impl Into<String>,
    content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
) -> View {
    let state = DialogState::controlled(open).alert().dismiss_on_outside_press(false);
    modal(state, id, title.into(), description.into(), content)
}

fn modal<V: IntoView + 'static>(
    state: DialogState,
    id: &str,
    title: String,
    description: String,
    mut content: impl FnMut(&mut BuildCx<'_>) -> V + 'static,
) -> View {
    let id = id.to_owned();
    let owner = el().position(Position::Absolute).size(0.0).a11y_hidden();
    state
        .root(owner, move |b| {
            let locale = KitLocale::get(b);
            let viewport = b.viewport();
            let title =
                state.title(text(title.clone()).test_id(format!("{id}-title")).level(2).grow(1.0).basis(0.0).min_w(0.0).text_weight(600));
            let helper = (!description.trim().is_empty()).then(|| {
                state.description(
                    text(description.clone())
                        .test_id(format!("{id}-description"))
                        .text_size(FontSizeToken::Sm)
                        .text_color(ColorToken::TextMuted),
                )
            });
            let panel = state.panel(
                col()
                    .test_id(id.clone())
                    .w(rel(1.0))
                    .max_w(480)
                    .min_w(0)
                    .style_with(move |r, p| p.max_h((r.get(viewport).height - 32.0).max(0.0)))
                    .rounded(RadiusToken::Lg)
                    .border(1)
                    .border_color(ColorToken::Border)
                    .bg(ColorToken::SurfaceElevated)
                    .text_color(ColorToken::Text)
                    .shadow(ShadowToken::Xl)
                    .child(
                        row()
                            .w(rel(1.0))
                            .min_w(0)
                            .shrink(0.0)
                            .align(AlignItems::Center)
                            .gap(sp(3.0))
                            .p(sp(4.0))
                            .child(title)
                            .child(close_button(icon_button(octara_icons::X, locale.close)).test_id(format!("{id}-close"))),
                    )
                    .child(
                        col()
                            .test_id(format!("{id}-body"))
                            .w(rel(1.0))
                            .min_w(0)
                            .shrink(1.0)
                            .overflow_y(Overflow::Scroll)
                            .gap(sp(3.0))
                            .px(sp(4.0))
                            .pb(sp(4.0))
                            .children(helper)
                            .child(content(b)),
                    ),
            );
            col()
                .position(Position::Absolute)
                .inset(sp(0.0))
                .size(rel(1.0))
                .p(sp(4.0))
                .align(AlignItems::Center)
                .justify(JustifyContent::Center)
                .child(
                    state.backdrop(
                        el().test_id(format!("{id}-scrim"))
                            .position(Position::Absolute)
                            .inset(sp(0.0))
                            .size(rel(1.0))
                            .bg(Color::BLACK.multiply_alpha(0.35)),
                    ),
                )
                .child(panel)
        })
        .into_view()
}
