//! Real-window checks for bound input and editor-owned suggestions with application callbacks.
use octara::prelude::*;
use octara_kit::{PopupItem, popup_list, text_field};

#[component]
pub fn view() -> View {
    let value = state("value", String::new);
    let seen = state("seen", String::new);
    let keys = state("keys", || 0u32);
    let blur = state("blur", String::new);
    let chosen = state("chosen", String::new);
    let open = state("open", || false);
    col()
        .size_full()
        .p_6()
        .gap_4()
        .bg(Token::Surface)
        .child(text("Component and application callbacks").text_xl())
        .child(text(move || format!("Model: {}", value.get())).id("model"))
        .child(text(move || format!("Seen: {}", seen.get())).id("seen"))
        .child(text(move || format!("Keys: {}", keys.get())).id("keys"))
        .child(text(move || format!("Blur: {}", blur.get())).id("blur"))
        .child(text(move || format!("Chosen: {}", chosen.get())).id("chosen"))
        .child(popup_list(
            "editor",
            "Editor",
            open,
            text_field(value)
                .w_full()
                .on_input(move |_| seen.set(value.get()))
                .on_key(move |_| {
                    keys.update(|n| *n += 1);
                    false
                })
                .on_blur(move || blur.set(format!("open={}", open.get()))),
            || vec![PopupItem::new("one", "One"), PopupItem::new("two", "Two")],
            move |id| chosen.set(id),
        ))
        .child(text_input().id("other").label("Other field").h(44).value("Move focus here"))
        .into()
}

/// Bound controls with application callbacks on every supported editing route.
#[component]
pub fn bindings() -> View {
    use octara_kit::{checkbox, number_field, slider, switch};
    let checked = state("checked", || false);
    let enabled = state("enabled", || false);
    let level = state("level", || 2.0);
    let raw = state("raw", || "2.00".to_owned());
    let seen = state("seen", String::new);
    let keys = state("keys", || 0u32);
    col()
        .size_full()
        .p_6()
        .gap_4()
        .bg(Token::Surface)
        .child(text("Bound controls and application callbacks").text_lg())
        .child(
            row()
                .gap_4()
                .items_center()
                .child(checkbox(checked).id("check").label("Checked").on_toggle(move || seen.set(format!("check:{}", checked.get()))))
                .child(switch(enabled).id("switch").label("Enabled").on_toggle(move || seen.set(format!("switch:{}", enabled.get())))),
        )
        .child(text(move || format!("Checked: {} / Enabled: {}", checked.get(), enabled.get())).id("toggles"))
        .child(
            slider("level", level, 0.0, 10.0, 0.5)
                .label("Level")
                .on_numeric_input(move |v| seen.set(format!("numeric:{v}:{}", level.get())))
                .on_press(move || seen.set(format!("press:{}", level.get())))
                .on_drag(move |event| seen.set(format!("drag:{:?}:{}", event.phase, level.get())))
                .on_key(move |_| {
                    keys.update(|n| *n += 1);
                    false
                }),
        )
        .child(text(move || format!("Level: {} / Keys: {}", level.get(), keys.get())).id("level-state"))
        .child(
            number_field(raw, 0.0, 10.0, 0.5)
                .id("number")
                .label("Quantity")
                .w_full()
                .on_input(move |_| seen.set(format!("input:{}", raw.get())))
                .on_submit(move || seen.set(format!("submit:{}", raw.get())))
                .on_blur(move || seen.set(format!("blur:{}", raw.get())))
                .on_numeric_input(move |v| seen.set(format!("number:{v}:{}", raw.get()))),
        )
        .child(text(move || format!("Raw: {}", raw.get())).id("raw"))
        .child(text(move || seen.get()).id("observed"))
        .child(text_input().id("outside").label("Other field").h(44).value("Move focus here"))
        .into()
}
