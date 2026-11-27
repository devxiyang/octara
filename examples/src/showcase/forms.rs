//! A small settings form: ordinary validation, field semantics and stable radio choices.

use super::section;
use octara::prelude::*;
use octara_kit::headless::Choice;
use octara_kit::{ButtonVariant, FieldInfo, Size as KitSize, button, field, radio_group, text_field};

fn name_error(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        Some("Enter a display name.".into())
    } else if name.trim().eq_ignore_ascii_case("admin") {
        Some("This display name is unavailable. Choose another name.".into())
    } else {
        None
    }
}

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let name = cx.signal("Alex Chen".to_owned());
    let style = cx.signal(Some("balanced".to_owned()));
    let detailed_help = cx.signal(false);
    let reordered = cx.signal(false);
    let saved = cx.signal(String::new());
    let error = cx.memo(move |r| name_error(&r.get(name)));
    let name_info = cx.memo(move |r| FieldInfo {
        description: if r.get(detailed_help) {
            "This name appears on your profile. You can use Chinese characters and emoji. Leading and trailing spaces are removed when you save."
        } else {
            "You can use Chinese characters and emoji."
        }
        .into(),
        error: r.get(error),
        required: true,
    });
    let options = cx.derive_list(move |r| {
        let reordered = r.get(reordered);
        let mut options = [
            ("fast", Choice::new("Fast").description("Prioritizes short replies.")),
            ("balanced", Choice::new(if reordered { "Balanced (recommended)" } else { "Balanced" }).description("For everyday questions.")),
            ("deep", Choice::new("In-depth").description("Currently unavailable.").disabled(true)),
        ];
        if reordered {
            options.reverse();
        }
        options.into_iter().map(|(key, option)| (key.to_owned(), option)).collect()
    });
    let save = move |w: &mut WriteCx<'_>| {
        if name_error(&w.get(name)).is_some() {
            return;
        }
        if let Some(style) = w.get(style).filter(|id| matches!(id.as_str(), "fast" | "balanced")) {
            let who = w.get(name);
            w.set(saved, format!("Saved: {} · {style}", who.trim()));
        }
    };
    let detail = move |w: &mut WriteCx<'_>| w.update(detailed_help, |shown| *shown = !*shown);
    let label = move |r: &mut ReadCx<'_>| if r.get(detailed_help) { "Profile display name".to_owned() } else { "Display name".to_owned() };
    let reorder = move |w: &mut WriteCx<'_>| w.update(reordered, |changed| *changed = !*changed);

    section("Fields and radio groups")
        .w(rel(1.0))
        .on_command("forms.help", detail)
        .on_command("forms.reorder", reorder)
        .child(field(cx, "profile-name", label, text_field(name.into()).w(rel(1.0)).on_submit(save), name_info))
        .child(radio_group("reply-style", "Reply style", style.into(), options))
        .child(
            row()
                .gap(sp(2.0))
                .wrap(FlexWrap::Wrap)
                .child(button("Toggle detailed help", KitSize::Md, ButtonVariant::Secondary).test_id("forms-help").on_press(detail))
                .child(button("Update options", KitSize::Md, ButtonVariant::Secondary).test_id("forms-reorder").on_press(reorder)),
        )
        .child(
            button("Save", KitSize::Md, ButtonVariant::Primary)
                .test_id("forms-save")
                .disabled(move |r: &mut ReadCx<'_>| r.get(error).is_some())
                .description(move |r: &mut ReadCx<'_>| r.get(error).unwrap_or_else(|| "Save your display name and reply style.".into()))
                .on_press(save),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| if r.get(saved).is_empty() { "No settings saved yet.".into() } else { r.get(saved) })
                .test_id("forms-status")
                .role(Role::Status)
                .live(Live::Polite)
                .text_size(FontSizeToken::Sm),
        )
}
