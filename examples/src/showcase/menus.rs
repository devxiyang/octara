//! Native menu and prompt replies delivered to the application model.

use super::section;
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button};

pub(super) fn action(id: &str, label: &str, checked: Option<bool>) -> MenuItem {
    MenuItem::Action { id: id.to_owned(), label: label.to_owned(), enabled: true, checked, shortcut: None }
}

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let wrap = cx.signal(true);
    let status = cx.signal("Right-click the box, or press Delete…".to_owned());
    section("Menus and prompts")
        .w(320)
        .child(
            el().test_id("menu-target")
                .h(80)
                .rounded(RadiusToken::Lg)
                .bg(ColorToken::Surface)
                .border(1)
                .border_color(ColorToken::Border)
                .align(AlignItems::Center)
                .justify(JustifyContent::Center)
                .child(text("Right-click here").text_color(ColorToken::TextMuted))
                .on_secondary_press(move |w, pos| {
                    let menu = Menu {
                        name: "Box".into(),
                        items: vec![action("copy", "Copy", None), MenuItem::Separator, action("wrap", "Wrap lines", Some(w.get(wrap)))],
                    };
                    w.ask(Ask::Menu { menu, at: pos }, move |w, answer| {
                        let Answer::Menu(picked) = answer else { return };
                        if picked.as_deref() == Some("wrap") {
                            w.update(wrap, |value| *value = !*value);
                        }
                        w.set(status, format!("Menu: {}", picked.as_deref().unwrap_or("dismissed")));
                    });
                }),
        )
        .child(button("Delete…", KitSize::Md, ButtonVariant::Secondary).test_id("delete").align_self(AlignItems::Start).on_press(
            move |w| {
                let ask = Ask::Prompt {
                    level: PromptLevel::Warning,
                    message: "Delete this note?".into(),
                    detail: Some("It cannot be undone.".into()),
                    answers: vec!["Cancel".into(), "Delete".into()],
                };
                w.ask(ask, move |w, answer| {
                    w.set(
                        status,
                        match answer {
                            Answer::Prompt(Some(1)) => "Prompt: deleted",
                            Answer::Prompt(Some(_)) => "Prompt: kept",
                            _ => "Prompt: dismissed",
                        }
                        .to_owned(),
                    );
                });
            },
        ))
        .child(
            text(move |r: &mut ReadCx<'_>| format!("{} (wrap {})", r.get(status), if r.get(wrap) { "on" } else { "off" })).test_id("said"),
        )
}
