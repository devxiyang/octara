//! Searchable choices, a controlled popover, and a picker inside a dialog.

use super::section;
use octara::prelude::*;
use octara_kit::{Choice, button, combobox, popover, text_field};

fn cities() -> Vec<Choice> {
    let mut options = vec![
        Choice::new("beijing", "北京"),
        Choice::new("shanghai", "上海（暂不可选）").disabled(true),
        Choice::new("hangzhou", "杭州"),
        Choice::new("shenzhen", "深圳"),
        Choice::new("guangzhou", "广州"),
        Choice::new("chengdu", "成都"),
    ];
    options.extend((1..=24).map(|n| Choice::new(format!("city-{n}"), format!("示例城市 {n:02}"))));
    options
}

#[ui]
pub fn card() -> El {
    let selected = state("city", || Some("hangzhou".to_owned()));
    let options = state("cities", cities);
    let updated = state("updated", || false);
    let visible = state("visible", || true);
    let details = state("details", || false);
    let dialog = state("dialog", || false);
    let note = state("note", String::new);
    let update = move || {
        let next = !updated.get();
        updated.set(next);
        let mut items = cities();
        if next {
            items.retain(|o| o.id != "beijing");
            items.iter_mut().find(|o| o.id == "hangzhou").unwrap().label = "杭州 · 已更新".into();
            items.reverse();
        }
        options.set(items);
    };
    on_command("choices.update", update);
    on_command("choices.toggle", move || visible.update(|v| *v = !*v));
    bind_keys([("mod+shift+u", "choices.update", None), ("mod+shift+x", "choices.toggle", None)]).expect("fixed choice shortcuts");
    section("Popovers and searchable choices")
        .w_full()
        .child(text("城市 / City").font_medium())
        .child(show(
            move || visible.get(),
            move || {
                combobox("city", "城市 / City", selected, move || options.get(), |caption| button(caption).w_full().min_w(0.0).truncate())
            },
        ))
        .child(
            text(move || {
                let current = selected.get();
                let label = options.with(|items| items.iter().find(|o| Some(&o.id) == current.as_ref()).map(|o| o.label.clone()));
                format!("已选择：{}", label.as_deref().unwrap_or("无"))
            })
            .id("chosen-city")
            .text_sm(),
        )
        .child(
            row()
                .gap_2()
                .flex_wrap()
                .child(button("更新选项").id("update-cities").on_press(update))
                .child(button("显示 / 隐藏选择器").id("toggle-city").on_press(move || visible.update(|v| *v = !*v))),
        )
        .child(text("⌘⇧U 更新选项；⌘⇧X 显示或隐藏选择器。").text_sm().text_color(Token::TextMuted))
        .child(popover("details", details, Placement::BELOW, button("打开 Popover"), move || {
            col()
                .w(280.0)
                .max_w_full()
                .gap_3()
                .child(text("备注 / Note").font_medium())
                .child(text_field(note).id("choice-note").label("备注 / Note").w_full())
                .child(button("完成").id("close-details").on_press(move || details.set(false)))
        }))
        .child(button("在对话框中选择").id("open-choices-dialog").on_press(move || dialog.set(true)).overlay(
            move || dialog.get(),
            Placement::BELOW,
            move || {
                col()
                    .id("choices-dialog")
                    .role(Role::Dialog)
                    .label("选择目的地")
                    .w(320.0)
                    .p_4()
                    .gap_3()
                    .rounded_lg()
                    .border()
                    .border_color(Token::Border)
                    .bg(Token::Surface)
                    .shadow_lg()
                    .modal(false, move || dialog.set(false))
                    .child(text("选择目的地").font_semibold())
                    .child(combobox(
                        "dialog-city",
                        "目的地 / Destination",
                        selected,
                        move || options.get(),
                        |caption| button(caption).w_full().min_w(0.0).truncate(),
                    ))
                    .child(button("完成").id("close-choices-dialog").on_press(move || dialog.set(false)))
            },
        ))
        .child(text(move || if updated.get() { "选项已更新" } else { "原始选项" }).id("choices-revision").text_sm())
}
