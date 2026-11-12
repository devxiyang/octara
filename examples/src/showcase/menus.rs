//! Menus and prompts: a context menu (native on macOS, drawn in the window elsewhere) with a
//! check mark, a disabled item and a submenu; a prompt with the default last.

use super::section;
use octara::prelude::*;
use octara_kit::button;

#[component]
pub fn card() -> El {
    let wrap = state("wrap", || true);
    let said = state("said", || "Right-click the box, or press Delete…".to_string());
    section("Menus and prompts")
        .w(320.0)
        .child(
            el().id("menu-target")
                .h(80.0)
                .rounded_lg()
                .bg(Token::Surface)
                .border()
                .border_color(Token::Border)
                .items_center()
                .justify_center()
                .child(text("Right-click here").text_color(Token::TextMuted).select_none())
                .on_secondary_press(move || {
                    spawn(async move {
                        let menu = Menu::new("Box")
                            .item(MenuItem::action("Copy", "copy").shortcut("mod+c"))
                            .item(MenuItem::action("Paste", "paste").shortcut("mod+v").disabled())
                            .item(MenuItem::separator())
                            .item(MenuItem::action("Wrap lines", "wrap").checked(wrap.get_untracked()))
                            .item(MenuItem::submenu(
                                Menu::new("Share").item(MenuItem::action("Copy link", "link")).item(MenuItem::action("Email", "email")),
                            ));
                        let picked = popup_menu(menu, None).await;
                        if picked.as_deref() == Some("wrap") {
                            wrap.update(|w| *w = !*w);
                        }
                        said.set(format!("Menu: {}", picked.as_deref().unwrap_or("dismissed")));
                    });
                }),
        )
        .child(button("Delete…").id("delete").self_start().on_press(move || {
            spawn(async move {
                let answer = prompt(PromptLevel::Warning, "Delete this note?", Some("It cannot be undone."), &["Cancel", "Delete"]).await;
                said.set(format!(
                    "Prompt: {}",
                    match answer {
                        Some(1) => "deleted",
                        Some(_) => "kept",
                        None => "dismissed",
                    }
                ));
            });
        }))
        .child(
            text(move || format!("{} (wrap {})", said.get(), if wrap.get() { "on" } else { "off" }))
                .id("said")
                .text_sm()
                .text_color(Token::TextMuted),
        )
}
