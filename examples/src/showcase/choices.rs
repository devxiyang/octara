//! Searchable choices, a controlled popover, and a picker inside a dialog.

use super::section;
use octara::prelude::*;
use octara_kit::headless::{Choice, Selection};
use octara_kit::{ButtonVariant, ComboboxOptions, PopupOptions, Size as KitSize, button, choice_row, combobox, popover, text_field};

fn cities(updated: bool) -> Vec<(String, Choice)> {
    let mut options: Vec<_> = [
        ("beijing", Choice::new("Beijing")),
        ("shanghai", Choice::new("Shanghai (unavailable)").disabled(true)),
        ("hangzhou", Choice::new(if updated { "Hangzhou (updated)" } else { "Hangzhou" })),
        ("shenzhen", Choice::new("Shenzhen")),
        ("guangzhou", Choice::new("Guangzhou")),
        ("chengdu", Choice::new("Chengdu")),
    ]
    .into_iter()
    .filter(|(key, _)| !updated || *key != "beijing")
    .map(|(key, choice)| (key.to_owned(), choice))
    .collect();
    options.extend((1..=24).map(|n| (format!("city-{n}"), Choice::new(format!("Sample city {n:02}")))));
    if updated {
        options.reverse();
    }
    options
}

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let selected = cx.signal(Some("hangzhou".to_owned()));
    let updated = cx.signal(false);
    let options = cx.derive_list(move |r| cities(r.get(updated)));
    let visible = cx.signal(true);
    let details = cx.signal(false);
    let dialog = cx.signal(false);
    let note = cx.signal(String::new());
    let update = move |w: &mut WriteCx<'_>| w.update(updated, |updated| *updated = !*updated);

    section("Popovers and searchable choices")
        .w(rel(1.0))
        .on_command("choices.update", update)
        .on_command("choices.toggle", move |w| w.update(visible, |v| *v = !*v))
        .child(text("City").text_weight(500))
        .child(show(
            move |r| r.get(visible),
            move |_| {
                combobox("city", "City", Selection::Single(selected.into()), options, ComboboxOptions::default(), city_trigger, choice_row)
            },
        ))
        .child(
            text(move |r: &mut ReadCx<'_>| {
                let current = r.get(selected);
                let label = current.and_then(|id| r.with_row(options, &id, |item| item.map(|choice| choice.label.clone())));
                format!("Selected city: {}", label.as_deref().unwrap_or("None"))
            })
            .test_id("chosen-city")
            .text_size(FontSizeToken::Sm),
        )
        .child(
            row()
                .gap(sp(2.0))
                .wrap(FlexWrap::Wrap)
                .child(button("Update options", KitSize::Md, ButtonVariant::Secondary).test_id("update-cities").on_press(update))
                .child(
                    button("Toggle city picker", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("toggle-city")
                        .on_press(move |w| w.update(visible, |v| *v = !*v)),
                ),
        )
        .child(popover(
            cx,
            "details",
            details.into(),
            PopupOptions::default(),
            button("Open popover", KitSize::Md, ButtonVariant::Secondary),
            move |_| {
                col()
                    .w(280.0)
                    .max_w(rel(1.0))
                    .gap(sp(3.0))
                    .child(text("Note").text_weight(500))
                    .child(text_field(note.into()).test_id("choice-note").label("Note").w(rel(1.0)))
                    .child(
                        button("Done", KitSize::Md, ButtonVariant::Secondary)
                            .test_id("close-details")
                            .on_press(move |w| w.set(details, false)),
                    )
            },
        ))
        .child(
            button("Choose in a dialog", KitSize::Md, ButtonVariant::Secondary)
                .test_id("open-choices-dialog")
                .on_press(move |w| w.set(dialog, true))
                .overlay(
                    Placement::BELOW,
                    move |r| r.get(dialog),
                    move |_| {
                        col()
                            .test_id("choices-dialog")
                            .role(Role::Dialog)
                            .label("Choose a destination")
                            .w(320.0)
                            .p(sp(4.0))
                            .gap(sp(3.0))
                            .rounded(RadiusToken::Lg)
                            .border(1)
                            .border_color(ColorToken::Border)
                            .bg(ColorToken::Surface)
                            .shadow(ShadowToken::Lg)
                            .layer(LayerPolicy { dismiss_on_outside_press: false, ..LayerPolicy::modal() }, move |w| w.set(dialog, false))
                            .child(text("Choose a destination").text_weight(600))
                            .child(combobox(
                                "dialog-city",
                                "Destination",
                                Selection::Single(selected.into()),
                                options,
                                ComboboxOptions::default(),
                                city_trigger,
                                choice_row,
                            ))
                            .child(
                                button("Done", KitSize::Md, ButtonVariant::Secondary)
                                    .test_id("close-choices-dialog")
                                    .on_press(move |w| w.set(dialog, false)),
                            )
                    },
                ),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| if r.get(updated) { "Options updated" } else { "Original options" })
                .test_id("choices-revision")
                .text_size(FontSizeToken::Sm),
        )
}

fn city_trigger(caption: ReadSignal<String>) -> El {
    button(caption, KitSize::Md, ButtonVariant::Secondary).w(rel(1.0)).min_w(0.0).text_overflow(TextOverflow::Ellipsis)
}
