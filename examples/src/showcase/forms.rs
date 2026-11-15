//! A small settings form: ordinary validation, field semantics and stable radio choices.

use super::section;
use octara::prelude::*;
use octara_kit::{FieldInfo, RadioOption, button, field, primary_button, radio_group, text_field};

fn name_error(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        Some("请输入显示名称。".into())
    } else if name.trim().eq_ignore_ascii_case("admin") {
        Some("此名称不可用，请换一个名称。".into())
    } else {
        None
    }
}

#[component]
pub fn card() -> El {
    let name = state("profile_name", || "小明".to_owned());
    let style = state("reply_style", || Some("balanced".to_owned()));
    let detailed_help = state("detailed_help", || false);
    let reordered = state("reordered", || false);
    let saved = state("saved_settings", String::new);
    let error = memo(move || name_error(&name.get()));
    let save = move || {
        if name_error(&name.get()).is_some() {
            return;
        }
        if let Some(style) = style.get().filter(|id| matches!(id.as_str(), "fast" | "balanced")) {
            saved.set(format!("已保存：{} · {style}", name.get().trim()));
        }
    };
    let detail = move || detailed_help.update(|shown| *shown = !*shown);
    let reorder = move || reordered.update(|changed| *changed = !*changed);
    on_command("forms.help", detail);
    on_command("forms.reorder", reorder);
    bind_keys([("mod+shift+h", "forms.help", None), ("mod+shift+r", "forms.reorder", None)]).expect("fixed form shortcuts");

    section("Fields and radio groups")
        .w_full()
        .child(field("profile-name", "显示名称", text_field(name).w_full().on_submit(save), move || FieldInfo {
            description: if detailed_help.get() {
                "支持中文和表情；名称用于显示，保存时去掉两端空格。"
            } else {
                "支持中文和表情。"
            }
            .into(),
            error: error.get(),
            required: true,
        }))
        .child(radio_group("reply-style", "回复风格", style, move || {
            let mut options = vec![
                RadioOption::new("fast", "快捷").description("优先给出简短回复。"),
                RadioOption::new("balanced", if reordered.get() { "均衡 · 推荐" } else { "均衡" }).description("适合日常问题。"),
                RadioOption::new("deep", "深入").description("当前不可用。").disabled(true),
            ];
            if reordered.get() {
                options.reverse();
            }
            options
        }))
        .child(
            row()
                .gap_2()
                .flex_wrap()
                .child(button("切换说明").id("forms-help").on_press(detail))
                .child(button("更新选项").id("forms-reorder").on_press(reorder)),
        )
        .child(text("⌘⇧H 切换说明；⌘⇧R 更新选项。").text_xs().text_color(Token::TextMuted))
        .child(
            primary_button("保存")
                .id("forms-save")
                .disabled(move || error.get().is_some())
                .description(move || error.get().unwrap_or_else(|| "保存当前名称和回复风格。".into()))
                .on_press(save),
        )
        .child(
            text(move || if saved.get().is_empty() { "尚未保存。".into() } else { saved.get() })
                .id("forms-status")
                .role(Role::Status)
                .live(Live::Polite)
                .text_sm(),
        )
}
