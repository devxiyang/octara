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
    let chosen = cx.signal("No suggestion selected.".to_owned());
    let submissions = cx.signal(0);

    section("Dialog, tabs and input suggestions")
        .w(rel(1.0))
        .on_command("interaction.options", move |w| w.update(updated, |v| *v = !*v))
        .child(text("Open the editor and type / for suggestions. Switching tabs keeps your draft and edit history.").text_size(FontSizeToken::Sm))
        .child(button("Open editor", KitSize::Md, ButtonVariant::Secondary).test_id("interaction-open").on_press(move |w| w.set(open, true)))
        .child(text(chosen).test_id("interaction-chosen").role(Role::Status).live(Live::Polite).text_size(FontSizeToken::Sm))
        .child(
            text(move |r: &mut ReadCx<'_>| format!("Submissions: {}", r.get(submissions)))
                .test_id("interaction-submissions")
                .text_size(FontSizeToken::Sm),
        )
        .child(dialog(
            "editor-dialog",
            "Edit message",
            open.into(),
            DialogOptions {
                description: "Use the arrow keys to focus a tab and Enter to select it. Focus stays in the editor while suggestions are open."
                    .into(),
                ..Default::default()
            },
            move |b| {
                let pages = b.list_from([
                    ("draft".to_owned(), Choice::new("Draft")),
                    ("preview".to_owned(), Choice::new("Preview")),
                    ("advanced".to_owned(), Choice::new("Advanced").disabled(true)),
                ]);
                let formats = b.list_from([
                    ("brief".to_owned(), Choice::new("Brief")),
                    ("full".to_owned(), Choice::new("Full")),
                    ("auto".to_owned(), Choice::new("Auto").disabled(true)),
                ]);
                let suggestions = b.derive_list(move |r| {
                    let query = r.get(draft).to_lowercase();
                    if !query.starts_with('/') {
                        return Vec::new();
                    }
                    let updated = r.get(updated);
                    let mut items = [
                        ("review", Choice::new(if updated { "/review · review changes" } else { "/review" }).description("Review the current changes")),
                        ("deploy", Choice::new("/deploy").description("Currently unavailable").disabled(true)),
                        ("summary", Choice::new("/summary").description("Summarize the current content")),
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
                            w.set(chosen, format!("Selected: /{key}"));
                            w.set(
                                draft,
                                if key == "review" { "Please review the current changes." } else { "Please summarize the current content." }
                                    .into(),
                            );
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
                        "Editor tabs",
                        tab.into(),
                        pages.read_only(),
                        TabsOptions { activation: Activation::Manual, ..Default::default() },
                        move |b, key, _| match key.as_str() {
                            "draft" => {
                                let editor = input()
                                    .label("Message")
                                    .w(rel(1.0))
                                    .submit_on_enter()
                                    .on_submit(move |w| w.update(submissions, |count| *count += 1));
                                col()
                                    .w(rel(1.0))
                                    .min_w(0.0)
                                    .gap(sp(3.0))
                                    .child(text("Type /, then use the Up and Down arrow keys and Enter to choose a suggestion.").text_size(FontSizeToken::Sm))
                                    .child(combobox_input(
                                        b,
                                        "suggest-editor",
                                        "Input suggestions",
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
                                        button("Update suggestions", KitSize::Md, ButtonVariant::Secondary)
                                            .test_id("interaction-options")
                                            .on_press(move |w| w.update(updated, |v| *v = !*v)),
                                    )
                                    .child(segmented(b, "editor-format", "Reply length", format.into(), formats.read_only()))
                            }
                            "preview" => col()
                                .w(rel(1.0))
                                .min_w(0.0)
                                .gap(sp(2.0))
                                .child(text(move |r: &mut ReadCx<'_>| format!("Preview: {}", r.get(draft))).test_id("editor-preview-text"))
                                .child(editor_tools(b)),
                            _ => col(),
                        },
                    ))
                    .child(
                        button("Done", KitSize::Md, ButtonVariant::Secondary).test_id("editor-done").on_press(move |w| w.set(open, false)),
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
                    button("Editor settings", KitSize::Md, ButtonVariant::Secondary)
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
                                    .label("Editor settings")
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
                                        text_field(note.into()).test_id("editor-tools-field").label("Note").placeholder("Note").w(rel(1.0)),
                                    )
                                    .child(
                                        button("Close settings", KitSize::Md, ButtonVariant::Secondary)
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
                    button("Follow-up action", KitSize::Md, ButtonVariant::Secondary)
                        .test_id("editor-after-tools")
                        .on_press(move |w| w.update(presses, |n| *n += 1)),
                ),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| format!("Follow-up actions: {}", r.get(presses)))
                .test_id("editor-tools-count")
                .text_size(FontSizeToken::Sm),
        )
}
