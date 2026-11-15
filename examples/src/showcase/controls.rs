//! Numeric drafts, slider input, progress and menus share ordinary application state.

use super::section;
use octara::prelude::*;
use octara_kit::{FieldInfo, button, field, menu_button, normalize_number, number_field, parse_number, progress, slider};

#[component]
pub fn card() -> El {
    let value = state("value", || 0.35);
    let draft = state("draft", || "0.35".to_owned());
    let disabled = state("disabled", || false);
    let unknown = state("unknown", || false);
    let menu = state("menu", || false);
    let status = state("status", || "尚未提交。".to_owned());
    let error = memo(move || {
        if parse_number(&draft.get()).is_some_and(|value| (0.0..=1.0).contains(&value)) {
            None
        } else {
            Some("请输入 0 到 1 之间的数字。".to_owned())
        }
    });
    let commit = move || {
        if error.get().is_none()
            && let Some(text) = normalize_number(&draft.get(), 0.0, 1.0, 0.05)
        {
            value.set(parse_number(&text).unwrap());
            draft.set(text.clone());
            status.set(format!("已提交：{text}"));
        }
    };
    on_command("controls.reset", move || {
        value.set(0.35);
        draft.set("0.35".into());
        status.set("已重置。".into());
    });
    on_command("controls.toggle_disabled", move || disabled.update(|value| *value = !*value));
    on_command("controls.minimum", move || {
        value.set(0.0);
        draft.set("0".into());
        status.set("已选择最小值。".into());
    });
    on_command("controls.maximum", move || {
        value.set(1.0);
        draft.set("1".into());
        status.set("已选择最大值。".into());
    });
    section("Numbers, progress and menus")
        .w_full()
        .child(text("数字输入保留原始草稿，提交后更新滑块和进度。").text_sm())
        .child(field(
            "controls-number",
            "强度",
            number_field(draft, 0.0, 1.0, 0.05).w_full().disabled(move || disabled.get()).on_submit(commit).on_blur(commit),
            move || FieldInfo {
                description: "范围 0–1，步长 0.05；Enter 或离开输入框时提交。".into(), error: error.get(), required: true
            },
        ))
        .child(
            slider(
                "controls-slider",
                (
                    move || value.get(),
                    move |next: f64| {
                        value.set(next);
                        draft.set(next.to_string());
                        status.set(format!("滑块：{next}"));
                    },
                ),
                0.0,
                1.0,
                0.05,
            )
            .label("强度滑块")
            .description("方向键调整，Home/End 到边界，也可点击轨道或拖动。")
            .w_full()
            .disabled(move || disabled.get()),
        )
        .child(
            progress(move || if unknown.get() { None } else { Some(value.get() * 100.0) }, 100.0)
                .id("controls-progress")
                .label("示例进度")
                .w_full(),
        )
        .child(
            row()
                .flex_wrap()
                .gap_2()
                .child(button("提交数字").id("controls-commit").disabled(move || disabled.get() || error.get().is_some()).on_press(commit))
                .child(button("切换未知进度").id("controls-progress-mode").on_press(move || unknown.update(|value| *value = !*value)))
                .child(menu_button("controls-menu", "操作菜单", menu, move || {
                    Menu::new("数值操作")
                        .item(MenuItem::action("重置", "controls.reset"))
                        .item(MenuItem::action("禁用编辑", "controls.toggle_disabled").checked(disabled.get()))
                        .item(MenuItem::separator())
                        .item(MenuItem::submenu(
                            Menu::new("边界")
                                .item(MenuItem::action("最小值", "controls.minimum"))
                                .item(MenuItem::action("最大值", "controls.maximum"))
                                .item(MenuItem::action("不可用", "controls.unavailable").disabled()),
                        ))
                })),
        )
        .child(text(move || status.get()).id("controls-status").role(Role::Status).live(Live::Polite).text_sm())
}
