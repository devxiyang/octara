//! Searchable choices, a controlled popover, and a picker inside a dialog.

use super::section;
use octara::prelude::*;
use octara_kit::headless::{Choice, Selection};
use octara_kit::{ButtonVariant, ComboboxOptions, PopupOptions, Size as KitSize, button, choice_row, combobox, popover, text_field};

fn cities(updated: bool) -> Vec<(String, Choice)> {
    let mut options: Vec<_> = [
        ("beijing", Choice::new("北京")),
        ("shanghai", Choice::new("上海（暂不可选）").disabled(true)),
        ("hangzhou", Choice::new(if updated { "杭州 · 已更新" } else { "杭州" })),
        ("shenzhen", Choice::new("深圳")),
        ("guangzhou", Choice::new("广州")),
        ("chengdu", Choice::new("成都")),
    ]
    .into_iter()
    .filter(|(key, _)| !updated || *key != "beijing")
    .map(|(key, choice)| (key.to_owned(), choice))
    .collect();
    options.extend((1..=24).map(|n| (format!("city-{n}"), Choice::new(format!("示例城市 {n:02}")))));
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
        .child(text("城市 / City").text_weight(500))
        .child(show(
            move |r| r.get(visible),
            move |_| {
                combobox(
                    "city",
                    "城市 / City",
                    Selection::Single(selected.into()),
                    options,
                    ComboboxOptions::default(),
                    city_trigger,
                    choice_row,
                )
            },
        ))
        .child(
            text(move |r: &mut ReadCx<'_>| {
                let current = r.get(selected);
                let label = current.and_then(|id| r.with_row(options, &id, |item| item.map(|choice| choice.label.clone())));
                format!("已选择：{}", label.as_deref().unwrap_or("无"))
            })
            .test_id("chosen-city")
            .text_size(FontSizeToken::Sm),
        )
        .child(
            row()
                .gap(sp(2.0))
                .wrap(FlexWrap::Wrap)
                .child(button("更新选项", KitSize::Md, ButtonVariant::Secondary).test_id("update-cities").on_press(update))
                .child(
                    button("显示 / 隐藏选择器", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("toggle-city")
                        .on_press(move |w| w.update(visible, |v| *v = !*v)),
                ),
        )
        .child(popover(
            cx,
            "details",
            details.into(),
            PopupOptions::default(),
            button("打开 Popover", KitSize::Md, ButtonVariant::Secondary),
            move |_| {
                col()
                    .w(280.0)
                    .max_w(rel(1.0))
                    .gap(sp(3.0))
                    .child(text("备注 / Note").text_weight(500))
                    .child(text_field(note.into()).test_id("choice-note").label("备注 / Note").w(rel(1.0)))
                    .child(
                        button("完成", KitSize::Md, ButtonVariant::Secondary)
                            .test_id("close-details")
                            .on_press(move |w| w.set(details, false)),
                    )
            },
        ))
        .child(
            button("在对话框中选择", KitSize::Md, ButtonVariant::Secondary)
                .test_id("open-choices-dialog")
                .on_press(move |w| w.set(dialog, true))
                .overlay(
                    Placement::BELOW,
                    move |r| r.get(dialog),
                    move |_| {
                        col()
                            .test_id("choices-dialog")
                            .role(Role::Dialog)
                            .label("选择目的地")
                            .w(320.0)
                            .p(sp(4.0))
                            .gap(sp(3.0))
                            .rounded(RadiusToken::Lg)
                            .border(1)
                            .border_color(ColorToken::Border)
                            .bg(ColorToken::Surface)
                            .shadow(ShadowToken::Lg)
                            .layer(LayerPolicy { dismiss_on_outside_press: false, ..LayerPolicy::modal() }, move |w| w.set(dialog, false))
                            .child(text("选择目的地").text_weight(600))
                            .child(combobox(
                                "dialog-city",
                                "目的地 / Destination",
                                Selection::Single(selected.into()),
                                options,
                                ComboboxOptions::default(),
                                city_trigger,
                                choice_row,
                            ))
                            .child(
                                button("完成", KitSize::Md, ButtonVariant::Secondary)
                                    .test_id("close-choices-dialog")
                                    .on_press(move |w| w.set(dialog, false)),
                            )
                    },
                ),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| if r.get(updated) { "选项已更新" } else { "原始选项" })
                .test_id("choices-revision")
                .text_size(FontSizeToken::Sm),
        )
}

fn city_trigger(caption: ReadSignal<String>) -> El {
    button(caption, KitSize::Md, ButtonVariant::Secondary).w(rel(1.0)).min_w(0.0).text_overflow(TextOverflow::Ellipsis)
}
