//! Native IME and virtual option selection in the same dialog.

use octara::prelude::*;
use octara_kit::{Choice, DialogOptions, button, combobox, dialog};

#[ui]
pub(super) fn view() -> El {
    let open = state("open", || false);
    let selected = state("selected", || None::<String>);
    let submissions = state("submissions", || 0);
    let choices = state("choices", || {
        (0..120)
            .map(|index| Choice::new(format!("greeting-{index:03}"), format!("你好 · {index:03}")))
            .chain([Choice::new("farewell", "再见")])
            .collect::<Vec<_>>()
    });
    on_command("ime-choice.reorder", move || choices.update(|choices| choices.rotate_right(2)));

    let report = move || selected.with(|selected| format!("提交次数 {} · {}", submissions.get(), selected.as_deref().unwrap_or("未选择")));
    let page = col().size_full().p_6().gap_3().bg(Token::Surface);
    #[cfg(target_os = "ios")]
    let page = page.pt_16();
    page.child(button("打开选择对话框").id("ime-choice-open").self_start().on_press(move || open.set(true)))
        .child(text(report).id("ime-choice-summary"))
        .child(dialog("ime-choice-dialog", "选择问候语", open, DialogOptions::default(), move || {
            col()
                .gap_3()
                .child(combobox(
                    "ime-choice",
                    "问候语",
                    (
                        move || selected.get(),
                        move |id| {
                            selected.set(id);
                            submissions.update(|count| *count += 1);
                        },
                    ),
                    move || choices.get(),
                    button,
                ))
                .child(text(report).id("ime-choice-result"))
        }))
}
