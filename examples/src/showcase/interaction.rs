//! A dialog containing retained tab panels and editor-owned suggestions.

use super::section;
use octara::prelude::*;
use octara_kit::{DialogOptions, PopupItem, RadioOption, TabOption, button, dialog, popup_list, segmented, tabs, text_field};

#[ui]
pub fn card() -> El {
    let open = state("open", || false);
    let tab = state("tab", || Some("draft".to_owned()));
    let format = state("format", || Some("brief".to_owned()));
    let draft = state("draft", String::new);
    let suggestions = state("suggestions", || false);
    let updated = state("updated", || false);
    let chosen = state("chosen", || "尚未选择建议。".to_owned());
    let submissions = state("submissions", || 0);
    on_command("interaction.options", move || updated.update(|value| *value = !*value));
    bind_keys([("mod+shift+u", "interaction.options", None)]).expect("fixed interaction shortcut");
    section("Dialog, tabs and input suggestions")
        .w_full()
        .child(text("打开编辑器，输入 / 查看建议。切换标签会保留草稿和编辑历史。").text_sm())
        .child(button("打开编辑器").id("interaction-open").on_press(move || open.set(true)))
        .child(text(move || chosen.get()).id("interaction-chosen").role(Role::Status).live(Live::Polite).text_sm())
        .child(text(move || format!("提交次数：{}", submissions.get())).id("interaction-submissions").text_sm())
        .child(dialog(
            "editor-dialog",
            "编辑消息",
            open,
            DialogOptions {
                description: "方向键移动标签焦点，Enter 选择；建议列表保留输入焦点。".into(), ..Default::default()
            },
            move || {
                col()
                    .w_full()
                    .min_w(0.0)
                    .gap_3()
                    .child(tabs("editor-tabs", "编辑器页面", tab, || {
                        vec![
                            TabOption::new("draft", "草稿"),
                            TabOption::new("preview", "预览"),
                            TabOption::new("advanced", "高级").disabled(true),
                        ]
                    }))
                    .child(
                        col()
                            .id("editor-panel-draft")
                            .role(Role::TabPanel)
                            .label("草稿")
                            .w_full()
                            .min_w(0.0)
                            .gap_3()
                            .style_with(move |p| if tab.get().as_deref() == Some("draft") { p.flex() } else { p.hidden() })
                            .child(text("输入 /，使用上下键和 Enter 选择。⌘⇧U 更新选项。").text_sm())
                            .child(popup_list(
                                "suggest-editor",
                                "输入建议",
                                suggestions,
                                text_field((
                                    move || draft.get(),
                                    move |value: String| {
                                        suggestions.set(value.starts_with('/'));
                                        draft.set(value);
                                    },
                                ))
                                .label("消息")
                                .w_full()
                                .on_submit(move || submissions.update(|count| *count += 1)),
                                move || {
                                    let query = draft.get();
                                    if !query.starts_with('/') {
                                        return Vec::new();
                                    }
                                    let mut items = vec![
                                        PopupItem::new("review", if updated.get() { "/review · 审阅" } else { "/review" })
                                            .description("审阅当前改动"),
                                        PopupItem::new("deploy", "/deploy").description("当前不可用").disabled(true),
                                        PopupItem::new("summary", "/summary").description("概括当前内容"),
                                    ];
                                    items.retain(|item| item.label.to_lowercase().contains(&query.to_lowercase()));
                                    if updated.get() {
                                        items.reverse();
                                    }
                                    items
                                },
                                move |id| {
                                    chosen.set(format!("已选择：{id}"));
                                    draft.set(if id == "review" { "请审阅当前改动。" } else { "请概括当前内容。" }.into());
                                },
                            ))
                            .child(segmented("editor-format", "回复长度", format, || {
                                vec![
                                    RadioOption::new("brief", "简短"),
                                    RadioOption::new("full", "完整"),
                                    RadioOption::new("auto", "自动").disabled(true),
                                ]
                            })),
                    )
                    .child(
                        col()
                            .id("editor-panel-preview")
                            .role(Role::TabPanel)
                            .label("预览")
                            .w_full()
                            .min_w(0.0)
                            .gap_2()
                            .style_with(move |p| if tab.get().as_deref() == Some("preview") { p.flex() } else { p.hidden() })
                            .child(text(move || format!("预览：{}", draft.get())).id("editor-preview-text"))
                            .child(text("返回草稿后继续编辑。").text_sm().text_color(Token::TextMuted)),
                    )
                    .child(button("完成").id("editor-done").on_press(move || open.set(false)))
            },
        ))
}
