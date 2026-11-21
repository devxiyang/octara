//! A dialog with retained tabs, editor-owned suggestions and a nonmodal form.

use super::section;
use octara::prelude::*;
use octara_kit::{Choice, DialogOptions, PopupItem, RadioOption, button, dialog, popup_list, segmented, tabs, text_field};

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
                        vec![Choice::new("draft", "草稿"), Choice::new("preview", "预览"), Choice::new("advanced", "高级").disabled(true)]
                    }))
                    .child(
                        col()
                            .id("editor-tabs-panel-draft")
                            .role(Role::TabPanel)
                            .labelled_by("editor-tabs-tab-draft")
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
                            .id("editor-tabs-panel-preview")
                            .role(Role::TabPanel)
                            .labelled_by("editor-tabs-tab-preview")
                            .w_full()
                            .min_w(0.0)
                            .gap_2()
                            .style_with(move |p| if tab.get().as_deref() == Some("preview") { p.flex() } else { p.hidden() })
                            .child(text(move || format!("预览：{}", draft.get())).id("editor-preview-text"))
                            .child(editor_tools()),
                    )
                    .child(button("完成").id("editor-done").on_press(move || open.set(false)))
            },
        ))
}

#[ui]
fn editor_tools() -> El {
    let open = state("editor-tools-open", || false);
    let note = state("editor-tools-note", String::new);
    let presses = state("editor-after-tools-presses", || 0);
    let shifted = state("editor-tools-shifted", || false);
    on_command("interaction.move-tools", move || shifted.update(|value| *value = !*value));
    let show_marker = std::env::var_os("OCTARA_MOVING_POPUP").is_some_and(|value| value == "1");
    col()
        .w_full()
        .gap_2()
        .child(
            row()
                .gap_2()
                .child(
                    button("编辑设置")
                        .id("editor-tools")
                        .controls("editor-tools-panel")
                        .expanded(open)
                        .layout_motion(Curve::Fade(1.8))
                        .style_with(move |style| style.ml(if shifted.get() { 128.0 } else { 0.0 }))
                        .on_press(move || open.update(|value| *value = !*value))
                        .overlay(
                            move || open.get(),
                            Placement::BELOW.fit(true),
                            move || {
                                col()
                                    .id("editor-tools-panel")
                                    .role(Role::Group)
                                    .label("编辑设置")
                                    .w(208)
                                    .max_w_full()
                                    .p_2()
                                    .gap_2()
                                    .bg(Token::Surface)
                                    .rounded_md()
                                    .border()
                                    .border_color(Token::Border)
                                    .shadow_lg()
                                    .opacity_motion(Curve::Fade(0.3))
                                    .dismissible(true, move || open.set(false))
                                    .child(text_field(note).id("editor-tools-field").label("备注").placeholder("备注").w_full())
                                    .child(button("关闭设置").id("editor-tools-close").on_press(move || open.set(false)))
                                    .children(
                                        show_marker.then(|| {
                                            el().id("editor-tools-marker").absolute().top(3).left(16).w(56).h(5).bg(rgb(0x19b785))
                                        }),
                                    )
                            },
                        ),
                )
                .child(button("后续操作").id("editor-after-tools").on_press(move || presses.update(|n| *n += 1))),
        )
        .child(text(move || format!("后续操作次数：{}", presses.get())).id("editor-tools-count").text_sm())
}
