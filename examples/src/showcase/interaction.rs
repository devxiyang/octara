//! A dialog with retained tabs, editor-owned suggestions and a nonmodal form.

use super::section;
use octara::prelude::*;
use octara_kit::headless::{Choice, Collection, ComboboxState, Selection, tabs::Activation};
use octara_kit::{
    ButtonVariant, ComboboxOptions, DialogOptions, PickerOptions, Size as KitSize, TabsOptions, button, combobox_input, dialog, segmented,
    tabs, text_field,
};

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let open = cx.signal(false);
    let tab = cx.signal(Some("draft".to_owned()));
    let format = cx.signal(Some("brief".to_owned()));
    let draft = cx.signal(String::new());
    let updated = cx.signal(false);
    let chosen = cx.signal("尚未选择建议。".to_owned());
    let submissions = cx.signal(0);

    section("Dialog, tabs and input suggestions")
        .w(rel(1.0))
        .on_command("interaction.options", move |w| w.update(updated, |v| *v = !*v))
        .child(text("打开编辑器，输入 / 查看建议。切换标签会保留草稿和编辑历史。").text_size(FontSizeToken::Sm))
        .child(button("打开编辑器", KitSize::Md, ButtonVariant::Secondary).test_id("interaction-open").on_press(move |w| w.set(open, true)))
        .child(text(chosen).test_id("interaction-chosen").role(Role::Status).live(Live::Polite).text_size(FontSizeToken::Sm))
        .child(
            text(move |r: &mut ReadCx<'_>| format!("提交次数：{}", r.get(submissions)))
                .test_id("interaction-submissions")
                .text_size(FontSizeToken::Sm),
        )
        .child(dialog(
            "editor-dialog",
            "编辑消息",
            open.into(),
            DialogOptions {
                description: "方向键移动标签焦点，Enter 选择；建议列表保留输入焦点。".into(), ..Default::default()
            },
            move |b| {
                let pages = b.list_from([
                    ("draft".to_owned(), Choice::new("草稿")),
                    ("preview".to_owned(), Choice::new("预览")),
                    ("advanced".to_owned(), Choice::new("高级").disabled(true)),
                ]);
                let formats = b.list_from([
                    ("brief".to_owned(), Choice::new("简短")),
                    ("full".to_owned(), Choice::new("完整")),
                    ("auto".to_owned(), Choice::new("自动").disabled(true)),
                ]);
                let suggestions = b.derive_list(move |r| {
                    let query = r.get(draft).to_lowercase();
                    if !query.starts_with('/') {
                        return Vec::new();
                    }
                    let updated = r.get(updated);
                    let mut items = [
                        ("review", Choice::new(if updated { "/review · 审阅" } else { "/review" }).description("审阅当前改动")),
                        ("deploy", Choice::new("/deploy").description("当前不可用").disabled(true)),
                        ("summary", Choice::new("/summary").description("概括当前内容")),
                    ];
                    if updated {
                        items.reverse();
                    }
                    items.into_iter().filter(|(_, item)| item.label.contains(&query)).collect()
                });
                let collection = Collection::choices(b, suggestions);
                let pick = b.binding(
                    |_| None,
                    move |w, key: Option<&'static str>| {
                        if let Some(key) = key {
                            w.set(chosen, format!("已选择：{key}"));
                            w.set(draft, if key == "review" { "请审阅当前改动。" } else { "请概括当前内容。" }.into());
                        }
                    },
                );
                let available = b.memo(|_| true);
                let suggestions = ComboboxState::with_collection(b, collection, Selection::Single(pick), draft, available);
                b.watch(
                    move |r| suggestions.is_open(r) && !r.with(draft, |draft| draft.starts_with('/')),
                    move |w, close, _| {
                        if *close {
                            suggestions.close(w);
                        }
                    },
                );
                col()
                    .w(rel(1.0))
                    .min_w(0.0)
                    .gap(sp(3.0))
                    .child(tabs(
                        b,
                        "editor-tabs",
                        "编辑器页面",
                        tab.into(),
                        pages.read_only(),
                        TabsOptions { activation: Activation::Manual, ..Default::default() },
                        move |b, key, _| match key.as_str() {
                            "draft" => {
                                let editor = input()
                                    .label("消息")
                                    .w(rel(1.0))
                                    .submit_on_enter()
                                    .on_submit(move |w| w.update(submissions, |count| *count += 1));
                                col()
                                    .w(rel(1.0))
                                    .min_w(0.0)
                                    .gap(sp(3.0))
                                    .child(text("输入 /，使用上下键和 Enter 选择。").text_size(FontSizeToken::Sm))
                                    .child(combobox_input(
                                        b,
                                        "suggest-editor",
                                        "输入建议",
                                        suggestions,
                                        editor,
                                        ComboboxOptions::default().picker(PickerOptions::Standard),
                                        |_, _, item| {
                                            col()
                                                .gap(sp(0.5))
                                                .description(move |r: &mut ReadCx<'_>| {
                                                    r.try_with(item, |item| item.description.clone()).unwrap_or_default()
                                                })
                                                .child(text(move |r: &mut ReadCx<'_>| {
                                                    r.try_with(item, |item| item.label.clone()).unwrap_or_default()
                                                }))
                                                .child(
                                                    text(move |r: &mut ReadCx<'_>| {
                                                        r.try_with(item, |item| item.description.clone()).unwrap_or_default()
                                                    })
                                                    .text_size(FontSizeToken::Xs)
                                                    .text_color(ColorToken::TextMuted),
                                                )
                                        },
                                    ))
                                    .child(
                                        button("更新建议", KitSize::Md, ButtonVariant::Secondary)
                                            .test_id("interaction-options")
                                            .on_press(move |w| w.update(updated, |v| *v = !*v)),
                                    )
                                    .child(segmented(b, "editor-format", "回复长度", format.into(), formats.read_only()))
                            }
                            "preview" => col()
                                .w(rel(1.0))
                                .min_w(0.0)
                                .gap(sp(2.0))
                                .child(text(move |r: &mut ReadCx<'_>| format!("预览：{}", r.get(draft))).test_id("editor-preview-text"))
                                .child(editor_tools(b)),
                            _ => col(),
                        },
                    ))
                    .child(
                        button("完成", KitSize::Md, ButtonVariant::Secondary).test_id("editor-done").on_press(move |w| w.set(open, false)),
                    )
            },
        ))
}

#[ui]
fn editor_tools(cx: &mut BuildCx<'_>) -> El {
    let open = cx.signal(false);
    let note = cx.signal(String::new());
    let presses = cx.signal(0);
    let shifted = cx.signal(false);

    let show_marker = std::env::var_os("OCTARA_MOVING_POPUP").is_some_and(|value| value == "1");
    col()
        .w(rel(1.0))
        .gap(sp(2.0))
        .on_command("interaction.move-tools", move |w| w.update(shifted, |v| *v = !*v))
        .child(
            row()
                .gap(sp(2.0))
                .child(
                    button("编辑设置", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("editor-tools")
                        .controls("editor-tools-panel")
                        .expanded(open)
                        .layout_motion(Curve::Fade(1.8))
                        .style_with(move |r, style| style.ml(if r.get(shifted) { 128.0 } else { 0.0 }))
                        .on_press(move |w| w.update(open, |value| *value = !*value))
                        .overlay(
                            Placement::BELOW.fit(true),
                            move |r| r.get(open),
                            move |_| {
                                col()
                                    .test_id("editor-tools-panel")
                                    .role(Role::Group)
                                    .label("编辑设置")
                                    .w(208)
                                    .max_w(rel(1.0))
                                    .p(sp(2.0))
                                    .gap(sp(2.0))
                                    .bg(ColorToken::Surface)
                                    .rounded(RadiusToken::Md)
                                    .border(1)
                                    .border_color(ColorToken::Border)
                                    .shadow(ShadowToken::Lg)
                                    .layer(LayerPolicy::non_modal(), move |w| w.set(open, false))
                                    .child(
                                        text_field(note.into()).test_id("editor-tools-field").label("备注").placeholder("备注").w(rel(1.0)),
                                    )
                                    .child(
                                        button("关闭设置", KitSize::Md, ButtonVariant::Secondary)
                                            .test_id("editor-tools-close")
                                            .on_press(move |w| w.set(open, false)),
                                    )
                                    .children(show_marker.then(|| {
                                        el().test_id("editor-tools-marker")
                                            .position(Position::Absolute)
                                            .top(3)
                                            .left(16)
                                            .w(56)
                                            .h(5)
                                            .bg(rgb(0x19b785))
                                    }))
                            },
                        ),
                )
                .child(
                    button("后续操作", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("editor-after-tools")
                        .on_press(move |w| w.update(presses, |n| *n += 1)),
                ),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| format!("后续操作次数：{}", r.get(presses)))
                .test_id("editor-tools-count")
                .text_size(FontSizeToken::Sm),
        )
}
